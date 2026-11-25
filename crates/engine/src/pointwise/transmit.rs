//! Source x5 refractive image splatting, including screen transparency.
use super::numeric::{add, div, mul, sub};
use super::{PointwiseError, PointwiseFilter, PointwiseRenderOptions, SharedImageMap};
use crate::{FloatBuffer, ImageGeometry, SeededLattice};

fn selected<E: 'static>(
    f: &PointwiseFilter<E>,
    ix: i32,
    time: f64,
    expected: &str,
) -> Result<bool, PointwiseError<E>> {
    let p = f.parameter(ix)?;
    let i = usize::try_from(p.effective_index_at(time)?).map_err(|_| PointwiseError::Index)?;
    let options = p.options()?.entries();
    Ok(options
        .get(i)
        .ok_or(PointwiseError::Index)?
        .as_ref()
        .is_some_and(|s| s.iter().copied().eq(expected.encode_utf16())))
}
#[allow(clippy::manual_range_contains)] // Source ordered comparisons preserve NaNs.
pub(super) fn retained<E: 'static>(
    f: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    geometry: ImageGeometry,
    time: f64,
    options: PointwiseRenderOptions,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    super::check_render_cancelled(cancelled)?;
    let original = inputs
        .first()
        .ok_or(PointwiseError::Index)?
        .as_ref()
        .ok_or(PointwiseError::NullReference)?;
    let height = inputs
        .get(1)
        .ok_or(PointwiseError::Index)?
        .as_ref()
        .ok_or(PointwiseError::NullReference)?;
    let height_data = height.channel(0, false)?;
    let output = SharedImageMap::new(geometry);
    let alpha = original.has_transparency()?;
    let mut channels: Vec<(FloatBuffer, FloatBuffer)> = Vec::new();
    for ch in 0..4 {
        let src = original.channel(ch, false)?;
        if src.len() > 1 || src.read(0)? != 0.0 {
            channels.push((src, output.materialize(ch)?));
        }
    }
    let factor = f.parameter(2)?.effective_value_at(time)? / f.factor();
    let sx = geometry.width() as f64 * factor;
    let sy = geometry.height() as f64 * factor;
    let index = f.parameter(0)?.effective_value_at(time)?;
    let inverse = 1.0 / if index == 0.0 { 0.000001 } else { index };
    let elevation = f.parameter(1)?.effective_value_at(time)? / f.factor();
    let w = geometry.width();
    let h = geometry.height();
    if w <= 0 || h <= 0 {
        return Err(PointwiseError::UnsupportedCapability(
            "transmit nonpositive geometry",
        ));
    }
    let density = super::transform::cast(f.parameter(3)?.effective_value_at(time)?);
    // A density sampled from an animation can leave the grid empty.
    let step = div(1.0, density as f32);
    let jitter = div(step, 2.0);
    let intensity = (1.0 / f64::from(density.wrapping_mul(density) as f32)) as f32;
    let random = selected(f, 4, time, "Random")?;
    let screen = selected(f, 5, time, "Screen")?;
    let (hw, hh) = if height_data.len() <= 1 {
        (1, 1)
    } else {
        (w, h)
    };
    if !channels.is_empty() {
        let lattice = random.then(|| SeededLattice::new(0)).transpose()?;
        for row in 0..density.wrapping_mul(h) {
            super::check_render_cancelled(cancelled)?;
            for col in 0..density.wrapping_mul(w) {
                let mut x = mul(col as f32, step);
                let mut y = mul(row as f32, step);
                if let Some(lattice) = &lattice {
                    x = sub(add(x, mul(lattice.sample3(col, row, 0), step)), jitter);
                    y = sub(add(y, mul(lattice.sample3(col, row, 1), step)), jitter);
                    if x < 0.0 {
                        x = add(x, w as f32);
                    }
                    if y < 0.0 {
                        y = add(y, h as f32);
                    }
                }
                let center = super::transform::sample(x, y, &height_data, hw, hh)?;
                let depth = f64::from(center) * factor + elevation;
                if depth <= 0.0 {
                    continue;
                }
                let gx = f64::from(sub(
                    super::transform::sample(add(x, 1.0), y, &height_data, hw, hh)?,
                    center,
                )) * sx;
                let gy = f64::from(sub(
                    super::transform::sample(x, add(y, 1.0), &height_data, hw, hh)?,
                    center,
                )) * sy;
                let inv = 1.0 / (gx * gx + gy * gy + 1.0).sqrt();
                let nx = gx * inv;
                let ny = gy * inv;
                let nz = -inv;
                let norm = nx * nx + ny * ny;
                let (ox, oy) = if norm == 0.0 {
                    (0.0, 0.0)
                } else {
                    let n = norm.sqrt();
                    let length = (norm + nz * nz).sqrt();
                    let sine = n / length;
                    let ratio = length / n;
                    let refracted = sine * inverse;
                    if refracted < -1.0 || refracted > 1.0 {
                        (0.0, 0.0)
                    } else {
                        let offset = (sine.asin() - refracted.asin()).tan() * depth;
                        (offset * (nx * ratio), offset * (ny * ratio))
                    }
                };
                let mut px = ((f64::from(x) + ox * w as f64) % w as f64) as f32;
                if px < 0.0 {
                    px = add(px, w as f32);
                }
                let mut py = ((f64::from(y) + oy * h as f64) % h as f64) as f32;
                if py < 0.0 {
                    py = add(py, h as f32);
                }
                for (i, (src, dst)) in channels.iter().enumerate() {
                    let color = if src.len() > 1 {
                        super::transform::sample(x, y, src, w, h)?
                    } else {
                        src.read(0)?
                    };
                    if color != 0.0 {
                        super::caustics::splat(
                            dst,
                            w,
                            h,
                            mul(intensity, color),
                            px,
                            py,
                            screen || (alpha && i == channels.len() - 1),
                        )?;
                    }
                }
            }
        }
    }
    if options.is_channel_rendering() {
        output.merge_effects(&[Some(original.clone()), Some(height.clone())])?;
    }
    Ok(Some(output))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PointwiseKind, ProceduralExecution, SharedFloatStore};
    use std::cell::Cell;
    fn filter() -> PointwiseFilter<()> {
        PointwiseFilter::new(PointwiseKind::Transmit).unwrap()
    }
    fn map(color: [f32; 4]) -> SharedImageMap {
        let result = SharedImageMap::new(ImageGeometry::new(4, 4));
        for (ix, value) in color.into_iter().enumerate() {
            result.set_constant(ix as i32, value).unwrap();
        }
        result
    }
    fn render(
        f: &PointwiseFilter<()>,
        source: &SharedImageMap,
        height: &SharedImageMap,
        channels: bool,
    ) -> SharedImageMap {
        f.render_supplied_at_time(
            &[Some(source.clone()), Some(height.clone())],
            4,
            4,
            0.0,
            ProceduralExecution::CallingThread,
            PointwiseRenderOptions::new(false, channels),
        )
        .unwrap()
        .unwrap()
    }
    #[test]
    fn transmit_retains_factory_wire_scale_and_source_zero_guarantees() {
        let node = crate::create_node_by_identity::<()>(Some(
            &"Transmit".encode_utf16().collect::<Vec<_>>(),
        ))
        .unwrap();
        let f = node.filter().unwrap();
        assert_eq!(f.kind().input_count(), 2);
        assert_eq!(f.parameter(3).unwrap().number(), 3.0);
        assert_eq!(f.parameter(4).unwrap().index(), 1);
        assert_eq!(f.parameter(5).unwrap().index(), 0);
        assert_eq!(
            node.slot(0).unwrap().name(),
            "Original".encode_utf16().collect::<Vec<_>>()
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
        assert_eq!(copy.filter().unwrap().parameter(4).unwrap().index(), 1);
        f.commit_spatial_scale(2.0).unwrap();
        assert_eq!(f.parameter(2).unwrap().number(), 0.05);
        assert_eq!(f.parameter(1).unwrap().number(), 0.5);
        let base = crate::PointwiseChannelGuarantees::new(
            [Some(0.0), None, Some(0.4), Some(-0.0)],
            [true; 4],
            false,
            None,
        );
        let result = node
            .calculate_image_guarantees_for_inputs(&[Some(base), None])
            .unwrap();
        assert_eq!(result.constants(), [Some(0.0), None, None, Some(0.0)]);
        assert_eq!(result.in_range(), [false; 4]);
        let child = crate::PointwiseNode::<()>::solid_color().unwrap();
        node.slot(0).unwrap().set_node(Some(&child)).unwrap();
        node.prepare_image_guarantees(&mut |_| Ok(())).unwrap();
        assert!(node.prepared_image_guarantees().is_some());
        assert!(
            node.render_original(4, 4)
                .unwrap()
                .caught_errors()
                .is_empty()
        );
    }
    #[test]
    fn regular_single_sample_preserves_nonzero_color_and_requested_maps() {
        let f = filter();
        f.parameter(3).unwrap().set_number(1.0).unwrap();
        f.parameter(4).unwrap().set_index(0).unwrap();
        let src = map([0.25, 0.5, 0.75, 0.0]);
        let height = map([0.0; 4]);
        let named = SharedFloatStore::new(FloatBuffer::new(vec![0.3])).unwrap();
        src.add_named_store(Some("Detail".encode_utf16().collect()), Some(named))
            .unwrap();
        let out = render(&f, &src, &height, false);
        for (ch, v) in [0.25, 0.5, 0.75, 0.0].into_iter().enumerate() {
            assert!(
                out.channel(ch as i32, true)
                    .unwrap()
                    .values()
                    .iter()
                    .all(|x| *x == v)
            );
        }
        assert!(out.named_channels().is_empty());
        assert_eq!(render(&f, &src, &height, true).named_channels().len(), 1);
    }

    #[test]
    fn propagated_cancellation_stops_the_splat_rows() {
        let f = filter();
        let source = map([1., 0., 0., 0.]);
        let height = map([0.; 4]);
        let checks = Cell::new(0);
        let cancelled = || {
            checks.set(checks.get() + 1);
            checks.get() == 3
        };
        let result = retained(
            &f,
            &[Some(source), Some(height)],
            ImageGeometry::new(4, 4),
            0.,
            PointwiseRenderOptions::new(false, false),
            &cancelled,
        );
        assert!(matches!(result, Err(PointwiseError::Cancelled)));
    }
    #[test]
    fn screen_changes_accumulation_and_alpha_uses_screen_even_in_add_mode() {
        let f = filter();
        f.parameter(3).unwrap().set_number(2.0).unwrap();
        f.parameter(4).unwrap().set_index(0).unwrap();
        let src = map([0.8, 0.0, 0.0, 0.8]);
        let height = map([0.0; 4]);
        let add = render(&f, &src, &height, false);
        let red = add.channel(0, true).unwrap().read(0).unwrap();
        let alpha = add.channel(3, true).unwrap().read(0).unwrap();
        assert!(alpha < red);
        f.parameter(5).unwrap().set_index(1).unwrap();
        let screen = render(&f, &src, &height, false);
        assert_eq!(screen.channel(0, true).unwrap().read(0).unwrap(), alpha);
        assert!(screen.channel(0, true).unwrap().read(0).unwrap() < red);
        let buffer = FloatBuffer::new(vec![0.0]);
        super::super::caustics::splat::<()>(&buffer, 1, 1, 0.8, 0.5, 0.5, true).unwrap();
        assert_eq!(buffer.read(0).unwrap(), 0.2);
    }
}
