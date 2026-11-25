use super::{SynthesisEdit, patches, selection_with_knots};
use crate::ImageGeometry;
use crate::pointwise::canvas::synthesis::Source;

/// The source contours offered by the Patches Create tool.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SynthesisPatchShape {
    Rectangle,
    Circle,
    Triangle,
}

impl SynthesisEdit {
    /// Appends a source contour, fitting it against a frozen guide raster when available.
    /// `selected` copies the previous patch's current shaping values into independent controls.
    pub fn add_patch_shape(
        &mut self,
        layer: usize,
        shape: SynthesisPatchShape,
        selected: Option<usize>,
        source_geometry: Option<ImageGeometry>,
    ) -> Option<usize> {
        let patches = self.patches(layer)?;
        if matches!(patches.source, Source::Missing) {
            return None;
        }
        let mut created = selection_with_knots(shape.knots());
        if let Some(index) = selected {
            inherit_shaping(&mut created, patches.selections.get(index)?);
        } else if matches!(patches.source, Source::Asset(_)) {
            created.feather = 2.;
            created.jitter = 8.;
        }
        let source = source_geometry
            .filter(|size| size.width() > 0 && size.height() > 0)
            .and_then(|size| {
                Some(ImageGeometry::new(
                    size.width().checked_mul(1000)?,
                    size.height().checked_mul(1000)?,
                ))
            })
            .or_else(|| patches.selection_source_geometry_with_input(None));
        created.fit_new_shape(source)?;
        let patches = self.patches_mut(layer)?;
        let index = patches.selections.len();
        patches.selections.push(created);
        self.changed = true;
        Some(index)
    }
}

fn inherit_shaping(created: &mut patches::Selection, selected: &patches::Selection) {
    let value = |slot, limits: std::ops::RangeInclusive<f64>, default: f64| {
        let effective = selected.effective_number(slot);
        if limits.contains(&effective) {
            effective
        } else {
            default
        }
    };
    created.feather = value(1, 0. ..=100., f64::from(created.feather)) as f32;
    created.jitter = value(2, 0. ..=100., f64::from(created.jitter)) as f32;
    created.jitter_frequency = value(3, 1. ..=100., f64::from(created.jitter_frequency)) as i32;
    created.shadow_strength = value(4, 0. ..=100., f64::from(created.shadow_strength)) as f32;
    created.shadow_feather = value(5, 0. ..=100., f64::from(created.shadow_feather)) as f32;
}

impl SynthesisPatchShape {
    fn knots(self) -> Vec<patches::Knot> {
        match self {
            Self::Rectangle => corners(&[[0., 1.], [0., 0.], [1., 0.], [1., 1.]]),
            Self::Triangle => corners(&[[0., 1.], [0.5, 0.], [1., 1.]]),
            Self::Circle => {
                // Half the native 0.552285 quarter-arc handle, rounded as the
                // shape's cubic segments are written into the source contour.
                let [low, high] = [0.22386, 0.77614];
                [
                    ([0., 0.5], [0., high], [0., low]),
                    ([0.5, 0.], [low, 0.], [high, 0.]),
                    ([1., 0.5], [1., low], [1., high]),
                    ([0.5, 1.], [high, 1.], [low, 1.]),
                ]
                .map(|(point, before, after)| patches::Knot {
                    point,
                    incoming: [0, 1].map(|axis| before[axis] - point[axis]),
                    outgoing: [0, 1].map(|axis| after[axis] - point[axis]),
                    mode: 1,
                    flags: [false; 3],
                })
                .to_vec()
            }
        }
    }
}

fn corners(points: &[[f32; 2]]) -> Vec<patches::Knot> {
    points
        .iter()
        .enumerate()
        .map(|(index, &point)| {
            let offset =
                |neighbor: usize| [0, 1].map(|axis| (points[neighbor][axis] - point[axis]) / 4.);
            patches::Knot {
                point,
                incoming: offset((index + points.len() - 1) % points.len()),
                outgoing: offset((index + 1) % points.len()),
                mode: 0,
                flags: [false; 3],
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pointwise::canvas::synthesis::{
        SynthesisLayerKind, SynthesisNumberField, SynthesisTexture,
    };
    use crate::{AdjustmentRecord, EnvironmentImage};
    use std::sync::Arc;

    fn draft() -> SynthesisEdit {
        let mut edit = SynthesisEdit::new(Arc::new(SynthesisTexture::default()), true);
        edit.add_layer(SynthesisLayerKind::Patches);
        edit
    }

    fn near(actual: [f32; 2], expected: [f32; 2]) {
        assert!(
            actual
                .into_iter()
                .zip(expected)
                .all(|(a, b)| (a - b).abs() < 2e-6),
            "{actual:?} differs from {expected:?}"
        );
    }

    #[test]
    fn patch_shape_templates_keep_exact_zero_margin_anchors_and_handles() {
        use SynthesisPatchShape::*;
        let mut edit = draft();
        let source = Some(ImageGeometry::new(160, 100));
        for (shape, points, mode) in [
            (Rectangle, &[[0., 1.], [0., 0.], [1., 0.], [1., 1.]][..], 0),
            (Circle, &[[0., 0.5], [0.5, 0.], [1., 0.5], [0.5, 1.]][..], 1),
            (Triangle, &[[0., 1.], [0.5, 0.], [1., 1.]][..], 0),
        ] {
            let index = edit.add_patch_shape(0, shape, None, source).unwrap();
            let created = &edit.patches(0).unwrap().selections[index];
            assert_eq!(created.knots.len(), points.len());
            assert_eq!(
                (created.feather, created.jitter, created.frequency),
                (0., 0., 100)
            );
            assert_eq!(created.flipped, [false; 2]);
            for (knot, point) in created.knots.iter().zip(points) {
                assert_eq!(knot.point, *point);
                assert_eq!((knot.mode, knot.flags), (mode, [false; 3]));
            }
        }
        let rectangle = &edit.patches(0).unwrap().selections[1];
        assert_eq!(rectangle.knots[0].incoming, [0.25, 0.]);
        assert_eq!(rectangle.knots[0].outgoing, [0., -0.25]);
        let circle = &edit.patches(0).unwrap().selections[2];
        assert_eq!(circle.knots[0].incoming, [0., 0.77614_f32 - 0.5]);
        assert_eq!(circle.knots[0].outgoing, [0., 0.22386_f32 - 0.5]);
        assert_eq!(circle.knots[1].outgoing, [0.77614_f32 - 0.5, 0.]);
        let triangle = &edit.patches(0).unwrap().selections[3];
        assert_eq!(triangle.knots[0].incoming, [0.25, 0.]);
        assert_eq!(triangle.knots[0].outgoing, [0.125, -0.25]);
        assert_eq!(triangle.knots[1].incoming, [-0.125, 0.25]);
        assert_eq!(edit.patch_counts(0), Some((4, 0)));

        let generic = edit.add_patch(0).unwrap();
        assert_eq!(
            edit.patch_knots(0, generic).unwrap().next(),
            Some([0.3, 0.3])
        );
    }

    #[test]
    fn patch_shape_inherits_resolved_values_and_leaves_the_old_receiver_and_placement_intact() {
        use SynthesisNumberField::PatchFeather;
        let mut edit = draft();
        let source = Some(ImageGeometry::new(160, 100));
        let selected = &mut edit.patches_mut(0).unwrap().selections[0];
        selected.frequency = 25;
        selected.feather = 2.;
        selected.jitter = 8.;
        selected.jitter_frequency = 31;
        selected.shadow_strength = 30.;
        selected.shadow_feather = 7.;
        let mut record = AdjustmentRecord::new(&[[0., 30.]]);
        record.components_mut()[0].active = true;
        record.components_mut()[0].name = "Feather".encode_utf16().collect();
        assert_eq!(
            edit.set_number_adjustment_with_input_geometry(
                Some(0),
                PatchFeather(0),
                Some((record, 6.)),
                source
            ),
            Some(true)
        );
        assert_eq!(edit.add_placement(0, 0), Some(0));
        assert_eq!(
            edit.add_patch_shape(0, SynthesisPatchShape::Triangle, Some(99), source),
            None
        );
        assert_eq!(
            edit.add_patch_shape(0, SynthesisPatchShape::Triangle, Some(0), None),
            None
        );
        assert_eq!(
            edit.add_patch_shape(0, SynthesisPatchShape::Triangle, Some(0), source),
            Some(1)
        );
        let patches = edit.patches(0).unwrap();
        assert_eq!(patches.selections[0].feather, 2.);
        assert!(patches.selections[0].number_links.get(1).is_some());
        assert_eq!(
            [patches.manual[0].patch, patches.manual[0].sequence],
            [0, 1]
        );
        let created = &patches.selections[1];
        assert_eq!(created.frequency, 100);
        assert_eq!(
            [
                created.feather,
                created.jitter,
                created.shadow_strength,
                created.shadow_feather
            ],
            [6., 8., 30., 7.]
        );
        assert_eq!(created.jitter_frequency, 31);
        assert!((0..6).all(|slot| created.number_links.get(slot).is_none()
            && created.animations.get(slot).is_none()));
        // Native shape constructor and footprint fit on a 160 × 100 raster.
        near(created.knots[0].point, [0.059_368_733, 0.929_010_03]);
        near(created.knots[1].point, [0.5, 0.070_989_98]);
        near(created.knots[2].point, [0.940_631_2, 0.929_010_03]);
        assert_eq!(edit.patch_counts(0), Some((2, 1)));
    }

    #[test]
    fn a_photo_shape_without_prior_selection_uses_its_native_feather_and_jitter() {
        let mut edit = draft();
        let mut bytes = b"PF\n4 2\n-1.0\n".to_vec();
        bytes.resize(1024, 0);
        let image = EnvironmentImage::from_file_bytes(bytes, "pfm").unwrap();
        assert!(edit.set_image_source(0, image));
        let index = edit
            .add_patch_shape(0, SynthesisPatchShape::Circle, None, None)
            .unwrap();
        let created = &edit.patches(0).unwrap().selections[index];
        assert_eq!([created.feather, created.jitter], [2., 8.]);
        near(created.knots[0].point, [0.04675, 0.5]);
        near(created.knots[1].point, [0.5, 0.0535]);
        near(created.knots[2].point, [0.95325, 0.5]);
        near(created.knots[3].point, [0.5, 0.9465]);
        let [anchor, tangent] = [created.knots[1].point, created.knots[1].incoming];
        near(
            [0, 1].map(|axis| anchor[axis] + tangent[axis]),
            [0.249_679_09, 0.0535],
        );
    }
}
