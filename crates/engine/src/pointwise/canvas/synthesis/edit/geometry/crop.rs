use super::{Arc, Paint, SynthesisEdit};
use crate::ImageGeometry;
use crate::environment_record::ImageAsset;
use crate::pointwise::canvas::synthesis::Source;

const MIN_EXTENT: f32 = 0.0001;

/// A visible edge, corner, or interior of the Blend Synthesis source crop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SynthesisCropHandle {
    Move,
    NorthWest,
    North,
    NorthEast,
    West,
    East,
    SouthWest,
    South,
    SouthEast,
}

impl SynthesisCropHandle {
    fn direction(self) -> [f32; 2] {
        match self {
            Self::Move => [0., 0.],
            Self::NorthWest => [1., 1.],
            Self::North => [0., 1.],
            Self::NorthEast => [-1., 1.],
            Self::West => [1., 0.],
            Self::East => [-1., 0.],
            Self::SouthWest => [1., -1.],
            Self::South => [0., -1.],
            Self::SouthEast => [-1., -1.],
        }
    }
}

impl SynthesisEdit {
    /// Reads the retained `[x, y, width, height]` in normalized source coordinates.
    /// Negative extents are valid and describe a reversed axis.
    pub fn crop_rectangle(&self, layer: usize) -> Option<[f32; 4]> {
        let Paint::Blend(blend) = &self.value.layers.get(layer)?.paint else {
            return None;
        };
        Some(blend.rectangle)
    }

    /// Resolves a drag from the crop at mouse-down without changing the draft.
    /// Delta is relative to the press location in normalized source coordinates.
    /// Aspect lock applies to corners. Centered resizing moves opposite edges outward or inward together.
    pub fn crop_for_drag(
        &self,
        layer: usize,
        handle: SynthesisCropHandle,
        delta: [f32; 2],
        keep_aspect: bool,
        centered: bool,
    ) -> Option<[f32; 4]> {
        let rectangle = self.crop_rectangle(layer)?;
        if !delta.into_iter().all(f32::is_finite) {
            return None;
        }
        if delta == [0.; 2] {
            return Some(rectangle);
        }
        let fitted = fit_rectangle(rectangle)?;
        let axes = [0, 1].map(|axis| {
            let ends = [fitted[axis], fitted[axis] + fitted[axis + 2]];
            [ends[0].min(ends[1]), ends[0].max(ends[1])]
        });
        let span = [fitted[2].abs(), fitted[3].abs()];
        if handle == SynthesisCropHandle::Move {
            return Some([
                (axes[0][0] + delta[0]).clamp(0., 1. - span[0]),
                (axes[1][0] + delta[1]).clamp(0., 1. - span[1]),
                span[0],
                span[1],
            ]);
        }

        let direction = handle.direction();
        let limits = [0, 1].map(|axis| travel_limits(axes[axis], direction[axis], centered));
        let mut inward = [0, 1].map(|axis| delta[axis] * direction[axis]);
        if keep_aspect && !direction.contains(&0.) {
            let ratio = span[1] / span[0];
            let min = limits[0][0].max(limits[1][0] / ratio);
            let max = limits[0][1].min(limits[1][1] / ratio);
            let x = inward[0].max(inward[1] / ratio).clamp(min, max);
            inward = [x, x * ratio];
        } else {
            inward = [0, 1].map(|axis| inward[axis].clamp(limits[axis][0], limits[axis][1]));
        }
        let [x, y] = [0, 1].map(|axis| {
            let [low, high] = axes[axis];
            let distance = inward[axis];
            let ends = match direction[axis] {
                d if d > 0. => [low + distance, high - if centered { distance } else { 0. }],
                d if d < 0. => [low + if centered { distance } else { 0. }, high - distance],
                _ => [low, high],
            };
            fit_axis(ends)
        });
        Some([x[0], y[0], x[1], y[1]])
    }

    /// Writes a normalized crop with source bounds and a nonzero extent.
    pub fn set_crop_rectangle(&mut self, layer: usize, rectangle: [f32; 4]) -> bool {
        let Some(rectangle) = fit_rectangle(rectangle) else {
            return false;
        };
        if self
            .crop_rectangle(layer)
            .is_none_or(|before| before == rectangle)
        {
            return false;
        }
        let Paint::Blend(blend) = &mut Arc::make_mut(&mut self.value.layers[layer]).paint else {
            return false;
        };
        blend.rectangle = rectangle;
        self.changed = true;
        true
    }

    /// Centers the largest square crop in a newly assigned source's pixel geometry.
    /// Call after an explicit source change, once its image dimensions are available.
    pub fn reset_crop_for_source_geometry(&mut self, layer: usize, source: ImageGeometry) -> bool {
        let [width, height] = [source.width(), source.height()].map(|side| side as f32);
        if width <= 0. || height <= 0. {
            return false;
        }
        let width_fraction = (height / width).min(1.);
        let height_fraction = (width / height).min(1.);
        self.set_crop_rectangle(
            layer,
            [
                (1. - width_fraction) / 2.,
                (1. - height_fraction) / 2.,
                width_fraction,
                height_fraction,
            ],
        )
    }

    /// Resets a crop using the decoded dimensions of a newly assigned embedded photo.
    pub fn reset_crop_for_embedded_photo(&mut self, layer: usize) -> bool {
        let Some(Source::Asset(asset)) = self.layer_source(layer) else {
            return false;
        };
        if !matches!(asset.as_ref(), ImageAsset::Embedded(_)) {
            return false;
        }
        let Ok(source) = asset.source_geometry::<()>() else {
            return false;
        };
        self.reset_crop_for_source_geometry(layer, source)
    }
}

pub(super) fn transform(rectangle: [f32; 4]) -> [f64; 4] {
    let [x, y, width, height] = rectangle;
    [x + width / 2., y + height / 2., width, height].map(|value| f64::from(value * 100.))
}

pub(super) fn with_transform(mut rectangle: [f32; 4], values: &[f64]) -> [f32; 4] {
    let before = transform(rectangle);
    for axis in 0..2 {
        let center = values[axis];
        let side = values[axis + 2];
        if center == before[axis] && side == before[axis + 2] {
            continue;
        }
        let mut center = center as f32 / 100.;
        let extent = if side == before[axis + 2] {
            let extent = rectangle[axis + 2];
            let half = (extent.abs() / 2.).min(0.5);
            center = center.clamp(half, 1. - half);
            extent
        } else {
            center = center.clamp(MIN_EXTENT / 2., 1. - MIN_EXTENT / 2.);
            let maximum = 2. * center.min(1. - center);
            (side as f32 / 100.).min(maximum).max(MIN_EXTENT)
        };
        rectangle[axis] = center - extent / 2.;
        rectangle[axis + 2] = extent;
    }
    rectangle
}

fn travel_limits([low, high]: [f32; 2], direction: f32, centered: bool) -> [f32; 2] {
    let near_low = [-low, 1. - low];
    let near_high = [high - 1., high];
    if direction == 0. {
        [0., 0.]
    } else if centered {
        [near_low[0].max(near_high[0]), near_low[1].min(near_high[1])]
    } else if direction > 0. {
        near_low
    } else {
        near_high
    }
}

fn fit_axis([start, end]: [f32; 2]) -> [f32; 2] {
    let min = start.min(end).clamp(0., 1.);
    let max = start.max(end).clamp(0., 1.);
    let span = (max - min).max(MIN_EXTENT);
    [min.min(1. - span), span]
}

fn fit_rectangle(mut rectangle: [f32; 4]) -> Option<[f32; 4]> {
    if !rectangle.iter().all(|value| value.is_finite()) {
        return None;
    }
    for axis in 0..2 {
        let start = rectangle[axis];
        let extent = rectangle[axis + 2];
        let end = start + extent;
        if (0. ..=1.).contains(&start) && (0. ..=1.).contains(&end) && extent.abs() >= MIN_EXTENT {
            continue;
        }
        let [low, span] = fit_axis([start, end]);
        let reversed = extent < 0.;
        rectangle[axis] = if reversed { low + span } else { low };
        rectangle[axis + 2] = if reversed { -span } else { span };
    }
    Some(rectangle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pointwise::canvas::synthesis::{
        SynthesisGeometry, SynthesisLayerKind, SynthesisTexture,
    };
    use SynthesisCropHandle::*;

    fn draft() -> SynthesisEdit {
        let mut edit = SynthesisEdit::new(Arc::new(SynthesisTexture::default()), false);
        edit.add_layer(SynthesisLayerKind::Blend);
        edit
    }

    fn near(actual: [f32; 4], expected: [f32; 4]) {
        assert!(
            actual
                .into_iter()
                .zip(expected)
                .all(|(a, b)| (a - b).abs() < 1e-6),
            "crop {actual:?} differs from {expected:?}"
        );
    }

    #[test]
    fn crop_drags_hold_the_opposite_anchor_or_center_and_keep_corner_aspect() {
        let mut edit = draft();
        let initial = [0.2, 0.2, 0.4, 0.2];
        assert!(edit.set_crop_rectangle(0, initial));
        let drag = |handle, delta, shift, control| {
            edit.crop_for_drag(0, handle, delta, shift, control)
                .unwrap()
        };
        near(drag(Move, [1., -1.], false, false), [0.6, 0., 0.4, 0.2]);
        near(
            drag(NorthWest, [0.04, 0.06], true, false),
            [0.32, 0.26, 0.28, 0.14],
        );
        near(
            drag(NorthEast, [-0.04, 0.06], true, false),
            [0.2, 0.26, 0.28, 0.14],
        );
        near(
            drag(SouthWest, [0.04, -0.06], true, false),
            [0.32, 0.2, 0.28, 0.14],
        );
        near(
            drag(SouthEast, [-0.04, -0.06], true, false),
            [0.2, 0.2, 0.28, 0.14],
        );
        near(
            drag(NorthWest, [0.04, 0.06], true, true),
            [0.32, 0.26, 0.16, 0.08],
        );
        near(drag(East, [0.8, 0.4], true, true), [0., 0.2, 0.8, 0.2]);
        near(
            drag(North, [0.7, -0.12], true, false),
            [0.2, 0.08, 0.4, 0.32],
        );
        near(drag(South, [0.7, 0.12], true, false), [0.2, 0.2, 0.4, 0.32]);
        near(drag(West, [0.6, 0.], false, false), [0.6, 0.2, 0.2, 0.2]);
        assert_eq!(edit.crop_rectangle(0), Some(initial));
        assert!(
            edit.crop_for_drag(0, West, [f32::NAN, 0.], false, false)
                .is_none()
        );
    }

    #[test]
    fn typed_crop_clamps_to_source_and_keeps_untouched_legacy_sign() {
        let mut edit = draft();
        let crop = SynthesisGeometry::Crop;
        let values = |edit: &SynthesisEdit| {
            edit.geometry_fields(0, crop)
                .iter()
                .map(|field| field.value())
                .collect::<Vec<_>>()
        };
        assert_eq!(values(&edit), [50., 50., 100., 100.]);
        assert!(edit.set_crop_rectangle(0, [0.7, 0.3, -0.4, 0.4]));
        near(edit.crop_rectangle(0).unwrap(), [0.7, 0.3, -0.4, 0.4]);
        let mut moved = values(&edit);
        moved[0] = 0.;
        edit.set_geometry_fields(0, crop, &moved).unwrap();
        near(edit.crop_rectangle(0).unwrap(), [0.4, 0.3, -0.4, 0.4]);
        near(
            edit.crop_for_drag(0, East, [0.1, 0.], false, false)
                .unwrap(),
            [0., 0.3, 0.5, 0.4],
        );

        let mut resized = values(&edit);
        resized[2] = -99.;
        edit.set_geometry_fields(0, crop, &resized).unwrap();
        near(
            edit.crop_rectangle(0).unwrap(),
            [0.19995, 0.3, MIN_EXTENT, 0.4],
        );
        resized = values(&edit);
        resized[3] = 0.;
        edit.set_geometry_fields(0, crop, &resized).unwrap();
        near(
            edit.crop_rectangle(0).unwrap(),
            [0.19995, 0.49995, MIN_EXTENT, MIN_EXTENT],
        );
        assert!(
            edit.set_geometry_fields(0, crop, &[0., f64::NAN, 20., 20.])
                .is_err()
        );
        assert!(edit.crop_rectangle(1).is_none());
    }

    #[test]
    fn replacing_the_source_resets_to_a_centered_square_in_its_pixels() {
        let mut edit = draft();
        assert!(edit.reset_crop_for_source_geometry(0, ImageGeometry::new(160, 100)));
        near(edit.crop_rectangle(0).unwrap(), [0.1875, 0., 0.625, 1.]);
        assert!(edit.reset_crop_for_source_geometry(0, ImageGeometry::new(100, 160)));
        near(edit.crop_rectangle(0).unwrap(), [0., 0.1875, 1., 0.625]);
        assert!(edit.reset_crop_for_source_geometry(0, ImageGeometry::new(200, 200)));
        near(edit.crop_rectangle(0).unwrap(), [0., 0., 1., 1.]);
        assert!(!edit.reset_crop_for_source_geometry(0, ImageGeometry::new(0, 200)));

        assert!(!edit.reset_crop_for_embedded_photo(0));
        let mut bytes = b"PF\n4 2\n-1.0\n".to_vec();
        bytes.resize(1024, 0);
        let photo = crate::EnvironmentImage::from_file_bytes(bytes, "pfm").unwrap();
        assert!(edit.set_image_source(0, photo));
        assert!(edit.reset_crop_for_embedded_photo(0));
        near(edit.crop_rectangle(0).unwrap(), [0.25, 0., 0.5, 1.]);
    }
}
