//! Source fm stereogram workers, using the shared r7 affine and wrap sampler.
use super::numeric::{add, div, mul, sub};
use super::{
    PointwiseError, PointwiseFilter, PointwiseRenderOptions, SharedImageMap, check_render_cancelled,
};
use crate::{ImageError, ImageGeometry, ProceduralExecution};

pub(super) fn retained<E: 'static>(
    f: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    geometry: ImageGeometry,
    time: f64,
    execution: ProceduralExecution,
    options: PointwiseRenderOptions,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    check_render_cancelled(cancelled)?;
    let background = inputs
        .first()
        .ok_or(PointwiseError::Index)?
        .as_ref()
        .ok_or(PointwiseError::NullReference)?;
    let height = inputs
        .get(1)
        .ok_or(PointwiseError::Index)?
        .as_ref()
        .ok_or(PointwiseError::NullReference)?;
    let w = geometry.width();
    let h = geometry.height();
    if w <= 0 || h <= 0 {
        return Err(PointwiseError::UnsupportedCapability(
            "stereogram nonpositive geometry",
        ));
    }
    // Projection nodes can supply a square raster to a rectangular render.
    let background = super::image_resize::resize(background, w, h, execution)?;
    let repeat = (f.parameter(1)?.effective_value_at(time)? * f.factor()) as f32;
    let background = super::imported_render::affine(
        &background,
        [f64::from(repeat); 2],
        [0.0; 2],
        options.is_channel_rendering(),
        execution,
    )?;
    check_render_cancelled(cancelled)?;
    let heights = super::image_resize::resize(height, w, h, execution)?.materialize(0)?;
    let output = SharedImageMap::new(geometry);
    let depth = mul(
        div(
            (f.parameter(0)?.effective_value_at(time)? / f.factor()) as f32,
            10.0,
        ),
        w as f32,
    );
    let spacing = mul(div(1.0, repeat), w as f32);
    let half = super::transform::cast(f64::from(div(spacing, 2.0)).round_ties_even());
    let len = usize::try_from(h).map_err(|_| ImageError::Overflow)?;
    let mut first = Vec::new();
    first
        .try_reserve_exact(len)
        .map_err(|_| ImageError::Allocation)?;
    first.resize(len, 0_i32);
    for row in 0..h {
        check_render_cancelled(cancelled)?;
        let start = row.wrapping_mul(w);
        for col in 0..w {
            let value = add(
                sub(col as f32, spacing),
                mul(
                    super::transform::read(&heights, col.wrapping_add(start))?,
                    depth,
                ),
            );
            if value >= 0.0 {
                first[row as usize] = col;
                break;
            }
        }
    }
    let count = if options.is_channel_rendering() {
        background.all_channel_count()
    } else {
        background.channel_count()? as i32
    };
    for ch in 0..count {
        check_render_cancelled(cancelled)?;
        let source = background.channel_with_effects(ch, false)?;
        if source.len() == 1 {
            output.copy_indexed_constant(ch, source.read(0)?, &background)?;
        } else {
            let dest = output.allocate_indexed(ch, &background)?;
            for row in 0..h {
                check_render_cancelled(cancelled)?;
                let start = row.wrapping_mul(w);
                let start_height = mul(super::transform::read(&heights, start)?, depth);
                let first = first[row as usize] as f32;
                let inverse = div(1.0, first);
                let found = first != 0.0;
                for col in 0..w {
                    let mut before = col.wrapping_sub(half);
                    if before < 0 {
                        before = before.wrapping_add(w);
                    }
                    let mut x = add(
                        sub(col as f32, spacing),
                        mul(
                            super::transform::read(&heights, before.wrapping_add(start))?,
                            depth,
                        ),
                    );
                    if x > col.wrapping_sub(1) as f32 {
                        x = col.wrapping_sub(1) as f32;
                    }
                    let value = if x < 0.0 {
                        if found {
                            x = sub(x, mul(start_height, mul(sub(first, col as f32), inverse)));
                        }
                        if x == f32::NEG_INFINITY {
                            return Err(PointwiseError::UnsupportedCapability(
                                "stereogram nonfinite wrap",
                            ));
                        }
                        let mut wraps = 0_usize;
                        while x < 0.0 {
                            let next = add(x, w as f32);
                            if next == x {
                                return Err(PointwiseError::UnsupportedCapability(
                                    "stereogram nonprogressing wrap",
                                ));
                            }
                            x = next;
                            wraps += 1;
                            if wraps.is_multiple_of(4096) {
                                check_render_cancelled(cancelled)?;
                            }
                        }
                        super::transform::sample(x, row as f32, &source, w, h)?
                    } else {
                        super::transform::sample(x, row as f32, &dest, w, h)?
                    };
                    super::transform::write(&dest, col.wrapping_add(start), value)?;
                }
            }
        }
        output.seal_channel(ch)?;
    }
    Ok(Some(output))
}
pub(super) fn commit_scale<E: 'static>(
    f: &PointwiseFilter<E>,
    scale: f64,
) -> Result<(), PointwiseError<E>> {
    for (ix, multiplier) in [(0, 1.0 / scale), (1, scale)] {
        let p = f.parameter(ix)?;
        if super::parameter_ops::is_animated(|| p.retained_animation())? {
            let curve = p.animation_curve(0)?;
            let bounds = p.limits()?.snapshot();
            curve
                .scale_bounds(
                    multiplier as f32,
                    bounds.minimum.to_framework_f32(),
                    bounds.maximum.to_framework_f32(),
                    bounds.increment.to_framework_f32(),
                )
                .map_err(crate::AnimationError::Bounds)?;
        } else {
            p.set_number(p.number() * multiplier)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FloatBuffer, PointwiseKind, SharedFloatStore};
    fn filter() -> PointwiseFilter<()> {
        PointwiseFilter::new(PointwiseKind::Stereogram).unwrap()
    }
    fn image() -> SharedImageMap {
        let img = SharedImageMap::new(ImageGeometry::new(8, 4));
        img.set_buffer(
            0,
            Some(&FloatBuffer::new(
                (0..32).map(|i| (i % 8) as f32 / 8.0).collect(),
            )),
        )
        .unwrap();
        img
    }
    fn render(
        f: &PointwiseFilter<()>,
        bg: &SharedImageMap,
        h: &SharedImageMap,
        channels: bool,
    ) -> SharedImageMap {
        f.render_supplied_at_time(
            &[Some(bg.clone()), Some(h.clone())],
            8,
            4,
            0.0,
            ProceduralExecution::CallingThread,
            PointwiseRenderOptions::new(false, channels),
        )
        .unwrap()
        .unwrap()
    }
    #[test]
    fn stereo_constructor_factory_wire_clone_and_scale() {
        let node = crate::create_node_by_identity::<()>(Some(
            &"Stereogram".encode_utf16().collect::<Vec<_>>(),
        ))
        .unwrap();
        let f = node.filter().unwrap();
        assert_eq!(f.kind().input_count(), 2);
        assert_eq!(f.parameter(0).unwrap().number(), 0.2);
        assert_eq!(f.parameter(1).unwrap().number(), 5.0);
        assert_eq!(
            node.slot(0).unwrap().name(),
            "Background".encode_utf16().collect::<Vec<_>>()
        );
        assert_eq!(
            node.slot(1).unwrap().name(),
            "Heightmap".encode_utf16().collect::<Vec<_>>()
        );
        let mut writer = crate::BinaryWriter::default();
        node.save_binary(&mut writer).unwrap();
        let mut reader = crate::BinaryCursor::new(writer.bytes()).unwrap();
        let name = reader.read_string().unwrap();
        let copy = crate::load_node_binary::<()>(&mut reader, 3.75, Some(&name)).unwrap();
        assert_eq!(copy.filter().unwrap().parameter(1).unwrap().number(), 5.0);
        assert_eq!(
            f.source_clone().unwrap().parameter(0).unwrap().number(),
            0.2
        );
        f.commit_spatial_scale(2.0).unwrap();
        assert_eq!(f.parameter(0).unwrap().number(), 0.1);
        assert_eq!(f.parameter(1).unwrap().number(), 10.0);
        let rendered = node.render_original(8, 4).unwrap();
        assert!(rendered.caught_errors().is_empty());
        assert_eq!(
            rendered.image().unwrap().geometry(),
            ImageGeometry::new(8, 4)
        );
    }
    #[test]
    fn stereo_one_repetition_zero_depth_and_named_channels() {
        let f = filter();
        f.parameter(0).unwrap().set_number(0.0).unwrap();
        f.parameter(1).unwrap().set_number(1.0).unwrap();
        let bg = image();
        let named = SharedFloatStore::new(FloatBuffer::new(vec![0.75])).unwrap();
        bg.add_named_store(Some("Detail".encode_utf16().collect()), Some(named))
            .unwrap();
        let heights = SharedImageMap::new(ImageGeometry::new(8, 4));
        let no = render(&f, &bg, &heights, false);
        let yes = render(&f, &bg, &heights, true);
        assert_eq!(
            no.channel(0, true).unwrap().values(),
            bg.channel(0, true).unwrap().values()
        );
        assert_eq!(no.named_channels().len(), 0);
        assert_eq!(yes.named_channels().len(), 1);
        assert_eq!(
            yes.named_channel(0)
                .unwrap()
                .unwrap()
                .data()
                .unwrap()
                .data(true)
                .unwrap()
                .read(0)
                .unwrap(),
            0.75
        );
    }

    #[test]
    fn stereo_fits_both_input_rasters_to_the_requested_grid() {
        let filter = filter();
        let background = image();
        let height = image();
        let geometry = ImageGeometry::new(4, 8);
        let resize = |image: &SharedImageMap, size: ImageGeometry| {
            super::super::image_resize::resize::<()>(
                image,
                size.width(),
                size.height(),
                ProceduralExecution::CallingThread,
            )
            .unwrap()
        };
        let background = resize(&background, geometry);
        let height = resize(&height, geometry);
        let exact = render(&filter, &background, &height, false);
        let requested = ImageGeometry::new(8, 4);
        let resized = render(
            &filter,
            &resize(&background, requested),
            &resize(&height, requested),
            false,
        );
        assert_eq!(exact.geometry(), requested);
        assert_eq!(
            exact.channel(0, true).unwrap().values(),
            resized.channel(0, true).unwrap().values()
        );
    }

    #[test]
    fn stereo_checks_cancellation_during_disparity_rows() {
        use crate::PointwiseRenderEffects;

        let filter = filter();
        filter.parameter(1).unwrap().set_number(1.).unwrap();
        let checks = std::cell::Cell::new(0);
        let cancel_after_first_row = || {
            checks.set(checks.get() + 1);
            checks.get() == 4
        };
        let mut capacity = |_| Ok(true);
        let mut preflight = |_, _| Ok(());
        let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight)
            .with_cancel_check(&cancel_after_first_row);
        let result = filter.render_supplied_at_time_with_effects(
            &[Some(image()), Some(image())],
            8,
            4,
            0.,
            ProceduralExecution::CallingThread,
            PointwiseRenderOptions::new(false, false),
            &mut effects,
        );
        assert!(matches!(result, Err(PointwiseError::Cancelled)));
    }
    #[test]
    fn stereo_height_changes_disparity_and_detached_guarantees_follow_background() {
        let f = filter();
        f.parameter(0).unwrap().set_number(0.8).unwrap();
        f.parameter(1).unwrap().set_number(2.0).unwrap();
        let bg = image();
        let flat = SharedImageMap::new(ImageGeometry::new(8, 4));
        let varying = SharedImageMap::new(ImageGeometry::new(8, 4));
        varying
            .set_buffer(
                0,
                Some(&FloatBuffer::new(
                    (0..32).map(|i| (i % 8) as f32 / 8.0).collect(),
                )),
            )
            .unwrap();
        assert_ne!(
            render(&f, &bg, &flat, false)
                .channel(0, true)
                .unwrap()
                .values(),
            render(&f, &bg, &varying, false)
                .channel(0, true)
                .unwrap()
                .values()
        );
        let base = crate::PointwiseChannelGuarantees::new(
            [Some(0.25), None, Some(2.0), None],
            [true; 4],
            false,
            None,
        );
        let height = crate::PointwiseChannelGuarantees::new([None; 4], [false; 4], false, None);
        let state = f
            .node()
            .calculate_image_guarantees_for_inputs(&[Some(base), Some(height)])
            .unwrap();
        assert_eq!(state.constants(), base.constants());
        assert_eq!(state.in_range(), [true, true, false, true]);
        let child = crate::PointwiseNode::<()>::solid_color().unwrap();
        f.node().slot(0).unwrap().set_node(Some(&child)).unwrap();
        f.node().prepare_image_guarantees(&mut |_| Ok(())).unwrap();
        assert!(f.node().prepared_image_guarantees().is_some());
    }
}
