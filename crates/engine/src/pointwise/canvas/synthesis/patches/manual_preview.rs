//! The Manual editor stamps once into the square authoring tile.
use super::{LayerColor, Patches, Photo, Source, area, draw, filled};
use crate::{
    ImageGeometry, PointwiseError, PointwiseRenderEffects, ProceduralExecution, SeededLattice,
    SharedImageMap, environment_record::ImageAsset, pointwise::check_render_cancelled,
};

const LIBRARY_PREVIEW_WIDTH: i32 = 512;

type ManualPreviewPixels = (Vec<[f32; 4]>, Option<ImageGeometry>);

impl Patches {
    pub(in crate::pointwise::canvas::synthesis) fn render_manual_preview<E: 'static>(
        &self,
        input: Option<&SharedImageMap>,
        geometry: ImageGeometry,
        seed: i32,
        execution: ProceduralExecution,
        effects: &mut PointwiseRenderEffects<'_, E>,
    ) -> Result<ManualPreviewPixels, PointwiseError<E>> {
        let mut pixels = filled(area(geometry)?, [0., 0., 0., 1.])?;
        let photo = match &self.source {
            Source::Missing => None,
            Source::Input => input.cloned(),
            Source::Asset(asset) => {
                let width = if matches!(asset.as_ref(), ImageAsset::Library { .. }) {
                    LIBRARY_PREVIEW_WIDTH
                } else {
                    let actual = asset.source_geometry()?;
                    let [width, height] =
                        [actual.width(), actual.height()].map(|value| value as f32);
                    if width <= 0. || height <= 0. {
                        return Ok((pixels, None));
                    }
                    let ratio = width / height;
                    let aspect = if ratio < 1. { 1. / ratio } else { ratio };
                    (geometry.width() as f32 * 1.2)
                        .ceil()
                        .max(aspect.ceil())
                        .min(width) as i32
                };
                Some(asset.synthesis_source(
                    width,
                    effects.environment_library(),
                    execution,
                    effects.cancel_check(),
                )?)
            }
        };
        let Some(photo) = photo.filter(|photo| {
            let geometry = photo.geometry();
            geometry.width() > 0 && geometry.height() > 0
        }) else {
            return Ok((pixels, None));
        };
        let source = Some(photo.geometry());
        if self.selections_parked
            || self.selections.is_empty()
            || (!self.automatic.enabled && self.manual.is_empty())
        {
            return Ok((pixels, source));
        }
        // The Manual editor uses the raw source rather than the final layer's
        // photo fitting and Equalize stage. Its stamp color is neutral.
        let photo = Photo::read(&photo)?;
        let patches = self.prepare_patches(&photo, seed, execution, effects)?;
        let neutral = LayerColor::default();
        let mut canvas = draw::Target {
            size: [geometry.width() as usize, geometry.height() as usize],
            pixels: &mut pixels,
            photo: &photo,
            color: &neutral,
            cancelled: effects.cancel_check(),
        };
        if self.automatic.enabled {
            let lattice = SeededLattice::new(seed)?;
            self.stamp_automatic(&mut canvas, &patches, 1., seed, &lattice)?;
            fade_automatic(&mut canvas)?;
        }
        for placement in self.manual.iter().rev() {
            check_render_cancelled(canvas.cancelled)?;
            let Some(patch) = patches
                .get(placement.patch as usize)
                .and_then(Option::as_ref)
            else {
                continue;
            };
            let origin = [0, 1].map(|axis| placement.point[axis] * canvas.size[axis] as f32);
            let stretch = [0, 1]
                .map(|axis| (f64::from(placement.size[axis]) / patch.signed_extent[axis]) as f32);
            canvas.place(patch, origin, stretch, placement.angle, 1.)?;
        }
        Ok((pixels, source))
    }
}

fn fade_automatic<E>(canvas: &mut draw::Target<'_>) -> Result<(), PointwiseError<E>> {
    for row in canvas.pixels.chunks_exact_mut(canvas.size[0]) {
        check_render_cancelled(canvas.cancelled)?;
        for [red, green, blue, transparency] in row {
            // anx.a(ImageMap, true) puts 50% opaque white above the raw Auto
            // raster, including its transparent gaps.
            let below = 1. - *transparency;
            let opacity = 0.5 + below * 0.5;
            let reciprocal = 1. / opacity;
            for channel in [red, green, blue] {
                *channel = (0.5 + below * *channel * 0.5) * reciprocal;
            }
            *transparency = 1. - opacity;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pointwise::canvas::synthesis::{
        Paint, SynthesisEdit, SynthesisLayerKind, SynthesisPatchResizeHandle,
        SynthesisPlacementTransform, SynthesisRecord, SynthesisTexture,
    };
    use crate::{Gtx375GraphDocument, SynthesisGeometry};
    use sha2::{Digest, Sha256};
    use std::sync::Arc;

    const SIDE: i32 = 64;
    const EXECUTION: ProceduralExecution = ProceduralExecution::CallingThread;
    const PHOTO_SIZE: ImageGeometry = ImageGeometry::new(80, 40);
    const SOURCE: [f32; 4] = [0.25, 0.5, 0.75, 0.];

    fn fixture() -> SynthesisTexture {
        let mut draft = SynthesisEdit::new(Arc::new(SynthesisTexture::default()), true);
        draft.add_layer(SynthesisLayerKind::Patches);
        assert_eq!(draft.add_placement(0, 0), Some(0));
        let mut texture = draft.record().texture().as_ref().clone();
        let patches = patches(&mut texture);
        patches.source = Source::Input;
        patches.automatic.enabled = false;
        patches.manual[0].size = [0.35, 0.4];
        texture
    }

    fn patches(texture: &mut SynthesisTexture) -> &mut Patches {
        let Paint::Patches(patches) = &mut Arc::make_mut(&mut texture.layers[0]).paint else {
            unreachable!("Patches fixture");
        };
        patches
    }

    fn constant_photo(color: [f32; 4]) -> SharedImageMap {
        let photo = SharedImageMap::new(PHOTO_SIZE);
        for (channel, value) in (0..4).zip(color) {
            photo.set_constant(channel, value).unwrap();
        }
        photo
    }

    fn preview(texture: SynthesisTexture, input: Option<&SharedImageMap>) -> SharedImageMap {
        let mut capacity = |_| Ok(true);
        let mut preflight = |_, _| Ok(());
        let mut effects = PointwiseRenderEffects::<()>::new(&mut capacity, &mut preflight);
        SynthesisRecord::from_texture(Arc::new(texture))
            .render_manual_placement_preview_with_effects(
                0,
                input,
                SIDE,
                None,
                EXECUTION,
                &mut effects,
            )
            .unwrap()
    }

    fn pixel(image: &SharedImageMap, x: usize, y: usize) -> [f32; 4] {
        [0, 1, 2, 3].map(|channel| {
            image
                .channel(channel, true)
                .unwrap()
                .read(y * SIDE as usize + x)
                .unwrap()
        })
    }

    #[test]
    fn manual_empty_tile_reports_geometry_when_a_photo_is_available() {
        let photo = constant_photo(SOURCE);
        let mut texture = fixture();
        patches(&mut texture).manual.clear();
        let record = SynthesisRecord::from_texture(Arc::new(texture));
        let mut capacity = |_| Ok(true);
        let mut preflight = |_, _| Ok(());
        let mut effects = PointwiseRenderEffects::<()>::new(&mut capacity, &mut preflight);
        for (input, expected) in [(Some(&photo), Some(PHOTO_SIZE)), (None, None)] {
            let (image, actual) = record
                .render_manual_placement_preview_with_geometry(
                    0,
                    input,
                    SIDE,
                    None,
                    EXECUTION,
                    &mut effects,
                )
                .unwrap();
            assert_eq!(actual, expected);
            assert_eq!(pixel(&image, 32, 32), [0., 0., 0., 1.]);
        }
    }

    #[test]
    fn manual_authoring_uses_square_raw_input_and_ignores_final_layer_controls() {
        let photo = constant_photo(SOURCE);
        let mut original = fixture();
        let patches = patches(&mut original);
        patches.equalize = false;
        patches.repeat = [1, 1];
        let mut altered = original.clone();
        altered.master.scale = 0.8;
        let layer = Arc::make_mut(&mut altered.layers[0]);
        layer.color_controls.tint = [0.1, 0.4, 0.8, 1.];
        let Paint::Patches(patches) = &mut layer.paint else {
            unreachable!()
        };
        patches.equalize = true;
        patches.strength = 1.;
        patches.repeat = [7, 3];
        let standard = preview(original, Some(&photo));
        let altered = preview(altered, Some(&photo));
        assert_eq!(standard.geometry(), ImageGeometry::new(SIDE, SIDE));
        assert_eq!(pixel(&standard, 32, 32), SOURCE);
        assert_eq!(pixel(&standard, 32, 20), [0., 0., 0., 1.]);
        assert_eq!(pixel(&standard, 0, 0), [0., 0., 0., 1.]);
        for channel in 0..4 {
            assert_eq!(
                standard.channel(channel, true).unwrap().values(),
                altered.channel(channel, true).unwrap().values()
            );
        }
    }

    #[test]
    fn manual_authoring_fades_auto_behind_manual_foreground_once() {
        let photo = constant_photo(SOURCE);
        let mut texture = fixture();
        let settings = patches(&mut texture);
        settings.automatic.enabled = true;
        settings.automatic.frequency = [1, 1];
        settings.automatic.offset = [0., 0.];
        settings.automatic.position_jitter = [0., 0.];
        settings.automatic.scale_jitter = 0.;
        settings.automatic.angle_jitter = 0.;
        let image = preview(texture.clone(), Some(&photo));
        assert_eq!(pixel(&image, 32, 32), SOURCE);
        assert_eq!(pixel(&image, 24, 50), [1., 1., 1., 0.5]);
        assert_eq!(pixel(&image, 0, 0), [0.625, 0.75, 0.875, 0.]);
        assert_eq!(pixel(&preview(texture, None), 32, 32), [0., 0., 0., 1.]);
    }

    #[test]
    fn manual_authoring_places_copies_in_reverse_order_and_wraps_stamp_pixels() {
        let photo = SharedImageMap::new(PHOTO_SIZE);
        let channel = photo.materialize(0).unwrap();
        let width = PHOTO_SIZE.width() as usize;
        for index in 0..PHOTO_SIZE.area() as usize {
            channel
                .write(
                    index,
                    if index % width < width / 2 {
                        0.25
                    } else {
                        0.75
                    },
                )
                .unwrap();
        }
        photo.set_constant(3, 0.).unwrap();
        let mut texture = fixture();
        let settings = patches(&mut texture);
        let original = settings.selections[0].clone();
        let mut left = original.clone();
        let mut right = original;
        for (selection, offset) in [(&mut left, 0.), (&mut right, 0.5)] {
            for knot in &mut selection.knots {
                knot.point[0] = knot.point[0] * 0.5 + offset;
                knot.incoming[0] *= 0.5;
                knot.outgoing[0] *= 0.5;
            }
        }
        settings.selections = vec![left, right];
        let mut lower = settings.manual[0].clone();
        settings.manual[0].point[0] = 0.02;
        lower.point = settings.manual[0].point;
        lower.patch = 1;
        settings.manual.push(lower);
        let first = preview(texture.clone(), Some(&photo));
        patches(&mut texture).manual.reverse();
        let reversed = preview(texture, Some(&photo));
        assert_eq!(pixel(&first, 2, 32)[0], 0.25);
        assert_eq!(pixel(&reversed, 2, 32)[0], 0.75);
        assert_eq!(pixel(&first, 62, 32)[0], 0.25);
    }

    #[test]
    fn manual_authoring_draft_has_its_own_raster_and_can_cancel() {
        let photo = constant_photo(SOURCE);
        let initial = fixture();
        let original = preview(initial.clone(), Some(&photo));
        let mut working = SynthesisEdit::new(Arc::new(initial), true);
        let initial_center = working.placement_geometry(0, 0).unwrap().center();
        assert!(working.transform_placement(
            0,
            0,
            SynthesisPlacementTransform::Resize {
                handle: SynthesisPatchResizeHandle::East,
                delta: [-0.23, 0.],
                preserve_aspect: false,
                from_center: true
            },
            PHOTO_SIZE
        ));
        assert_eq!(
            working.placement_geometry(0, 0).unwrap().center(),
            initial_center
        );
        let record = working.record();
        let mut capacity = |_| Ok(true);
        let mut preflight = |_, _| Ok(());
        let mut effects = PointwiseRenderEffects::<()>::new(&mut capacity, &mut preflight);
        let draft = record
            .render_manual_placement_preview_with_effects(
                0,
                Some(&photo),
                SIDE,
                None,
                EXECUTION,
                &mut effects,
            )
            .unwrap();
        assert_eq!(pixel(&original, 40, 32), SOURCE);
        assert_eq!(pixel(&draft, 40, 32), [0., 0., 0., 1.]);
        let mut effects = effects.with_cancel_check(&|| true);
        assert!(
            record
                .render_manual_placement_preview_with_effects(
                    0,
                    Some(&photo),
                    SIDE,
                    None,
                    EXECUTION,
                    &mut effects
                )
                .is_err()
        );
    }

    #[test]
    fn manual_authoring_matches_four_original_embedded_photo_tiles() {
        // These are the raw float planes from the original ln compositor
        // replay in port/.scratch/mycela-synthesis-manual-geometry/original.
        let fixture = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../port/evidence/genetica-ux/live-synthesis-patches.gtx"
        ));
        let document = Gtx375GraphDocument::<()>::load(fixture).unwrap();
        let node = document.branch(0).flatten().unwrap();
        let filter = node.filter().unwrap();
        filter.parameter(1).unwrap().set_number(17.).unwrap();
        let seed = filter.synthesis_manual_preview_seed_at(0.).unwrap();
        assert_eq!(seed, 17);
        let mut draft = filter.synthesis_edit().unwrap();
        for index in (0..draft.patch_counts(0).unwrap().1).rev() {
            assert!(draft.remove_placement(0, index));
        }
        let placement = draft.add_placement(0, 0).unwrap();
        draft
            .set_geometry_fields(
                0,
                SynthesisGeometry::Placement(placement),
                &[0.35, 0.58, 1., 23., 0.41, 0.37],
            )
            .unwrap();
        draft.set_equalize_source(0, true);
        let mut capacity = |_| Ok(true);
        let mut preflight = |_, _| Ok(());
        let mut effects = PointwiseRenderEffects::<()>::new(&mut capacity, &mut preflight);
        for (side, automatic, expected) in [
            (
                64,
                false,
                "fa6604d2eb47be81aa2973d334f0819a2767b66198256e6a2ec3980c5990e3b5",
            ),
            (
                64,
                true,
                "223434ae876fbbbce216cdf5747c4a94bafc2bca28e142d09a89df1d78477250",
            ),
            (
                128,
                false,
                "b60f6aa4b278c4744be69be571a9d09199896a0dd14a9a19acf21bf11c9983ad",
            ),
            (
                128,
                true,
                "4e2a30eda044124d7bbdaa77c7d970632cb0dc896e2f72633224540e182f1328",
            ),
        ] {
            let mut draft = draft.clone();
            draft.set_automatic_placement(0, automatic);
            let image = draft
                .record()
                .render_manual_placement_preview_with_effects(
                    0,
                    None,
                    side,
                    Some(seed),
                    EXECUTION,
                    &mut effects,
                )
                .unwrap();
            let mut hash = Sha256::new();
            for channel in 0..4 {
                let values = image.channel(channel, true).unwrap().values();
                hash.update((values.len() as u32).to_le_bytes());
                for value in values {
                    hash.update(value.to_le_bytes());
                }
            }
            assert_eq!(
                format!("{:x}", hash.finalize()),
                expected,
                "side {side}, Auto {automatic}"
            );
        }
    }
}
