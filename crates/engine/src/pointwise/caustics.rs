//! Source mj heightmap caustics and ordered r7 additive splat.
use super::numeric::{add, div, mul, sub};
use super::{PointwiseError, PointwiseFilter, PointwiseRenderOptions, SharedImageMap};
use crate::{FloatBuffer, ImageGeometry, SeededLattice};

pub(super) fn splat<E>(
    dst: &FloatBuffer,
    w: i32,
    h: i32,
    value: f32,
    x: f32,
    y: f32,
    screen: bool,
) -> Result<(), PointwiseError<E>> {
    let mut x0 = super::transform::cast(f64::from(x));
    let fx = sub(x, x0 as f32);
    let one_x = sub(1.0, fx);
    let mut x1 = x0.wrapping_add(1);
    if x1 >= w {
        x1 = x1.wrapping_sub(w);
        if x0 >= w {
            x0 = x0.wrapping_sub(w);
        }
    }
    let mut y0 = super::transform::cast(f64::from(y));
    let fy = sub(y, y0 as f32);
    let one_y = sub(1.0, fy);
    let mut y1 = y0.wrapping_add(1);
    if y1 >= h {
        y1 = y1.wrapping_sub(h);
        if y0 >= h {
            y0 = y0.wrapping_sub(h);
        }
    }
    let weights = [
        mul(mul(value, one_x), one_y),
        mul(mul(value, fx), one_y),
        mul(mul(value, one_x), fy),
        mul(mul(value, fx), fy),
    ];
    let cells = [
        x0.wrapping_add(y0.wrapping_mul(w)),
        x1.wrapping_add(y0.wrapping_mul(w)),
        x0.wrapping_add(y1.wrapping_mul(w)),
        x1.wrapping_add(y1.wrapping_mul(w)),
    ];
    if screen {
        let old = [
            super::transform::read(dst, cells[0])?,
            super::transform::read(dst, cells[1])?,
            super::transform::read(dst, cells[2])?,
            super::transform::read(dst, cells[3])?,
        ];
        for (i, (cell, weight)) in cells.into_iter().zip(weights).enumerate() {
            super::transform::write(dst, cell, add(old[i], mul(weight, sub(1.0, old[i]))))?;
        }
    } else {
        for (cell, weight) in cells.into_iter().zip(weights) {
            let old = super::transform::read(dst, cell)?;
            super::transform::write(dst, cell, add(old, weight))?;
        }
    }
    Ok(())
}
#[allow(clippy::manual_range_contains)] // Ordered comparisons retain source NaN behavior.
pub(super) fn retained<E: 'static>(
    f: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    geometry: ImageGeometry,
    time: f64,
    options: PointwiseRenderOptions,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    super::check_render_cancelled(cancelled)?;
    let input = inputs
        .first()
        .ok_or(PointwiseError::Index)?
        .as_ref()
        .ok_or(PointwiseError::NullReference)?;
    let source = input.channel(0, false)?;
    let output = SharedImageMap::new(geometry);
    let dense = source.len() != 1;
    let dest = if dense {
        output.materialize(0)?
    } else {
        output.channel(0, false)?
    };
    let factor = f.parameter(2)?.effective_value_at(time)? / f.factor();
    let dx = geometry.width() as f64 * factor;
    let dy = geometry.height() as f64 * factor;
    let index = f.parameter(0)?.effective_value_at(time)?;
    let inverse_index = 1.0 / if index == 0.0 { 0.000001 } else { index };
    let elevation = f.parameter(1)?.effective_value_at(time)? / f.factor();
    let (w, h) = if dense {
        (geometry.width(), geometry.height())
    } else {
        (1, 1)
    };
    if w <= 0 || h <= 0 {
        return Err(PointwiseError::UnsupportedCapability(
            "caustics nonpositive geometry",
        ));
    }
    let density = super::transform::cast(f.parameter(4)?.effective_value_at(time)?);
    // Animated density can fall below its static minimum. The original
    // worker visits no samples when the resulting grid is empty.
    let step = div(1.0, density as f32);
    let density_sq = density.wrapping_mul(density) as f32;
    let intensity = (f.parameter(3)?.effective_value_at(time)? / f64::from(density_sq)) as f32;
    let jitter = div(step, 2.0);
    let pattern = f.parameter(5)?;
    let selected =
        usize::try_from(pattern.effective_index_at(time)?).map_err(|_| PointwiseError::Index)?;
    let options_root = pattern.options()?;
    let choices = options_root.entries();
    let random = choices
        .get(selected)
        .ok_or(PointwiseError::Index)?
        .as_ref()
        .is_some_and(|x| x.as_slice() == [82, 97, 110, 100, 111, 109]);
    let slant = f.parameter(6)?;
    let sx = div(slant.value_at(0, time)? as f32, geometry.width() as f32);
    let sy = div(slant.value_at(1, time)? as f32, geometry.height() as f32);
    let lattice = SeededLattice::new(0)?;
    for row in 0..density.wrapping_mul(h) {
        super::check_render_cancelled(cancelled)?;
        for col in 0..density.wrapping_mul(w) {
            let mut x = mul(col as f32, step);
            let mut y = mul(row as f32, step);
            if random {
                x = sub(add(x, mul(lattice.sample3(col, row, 0), step)), jitter);
                y = sub(add(y, mul(lattice.sample3(col, row, 1), step)), jitter);
                if x < 0.0 {
                    x = add(x, w as f32);
                }
                if y < 0.0 {
                    y = add(y, h as f32);
                }
            }
            let center = super::transform::sample(x, y, &source, w, h)?;
            let depth = f64::from(center) * factor + elevation;
            if depth <= 0.0 {
                continue;
            }
            let right = add(super::transform::sample(add(x, 1.0), y, &source, w, h)?, sx);
            let below = add(super::transform::sample(x, add(y, 1.0), &source, w, h)?, sy);
            let gx = f64::from(sub(right, center)) * dx;
            let gy = f64::from(sub(below, center)) * dy;
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
                let refracted = sine * inverse_index;
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
            splat(&dest, w, h, intensity, px, py, false)?;
        }
    }
    if options.is_channel_rendering() {
        output.merge_effects(&[Some(input.clone())])?;
    }
    Ok(Some(output))
}
pub(super) fn commit_scale<E: 'static>(
    f: &PointwiseFilter<E>,
    scale: f64,
) -> Result<(), PointwiseError<E>> {
    for ix in [2, 1] {
        let p = f.parameter(ix)?;
        if super::parameter_ops::is_animated(|| p.retained_animation())? {
            super::point_light::scale_point_light_curve(&p, scale)?;
        } else {
            p.set_number(p.effective_value_at(0.0)? / scale)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PointwiseKind, ProceduralExecution, SharedFloatStore};
    fn filter() -> PointwiseFilter<()> {
        PointwiseFilter::new(PointwiseKind::Caustics).unwrap()
    }
    fn render(f: &PointwiseFilter<()>, input: &SharedImageMap, channels: bool) -> SharedImageMap {
        f.render_supplied_at_time(
            &[Some(input.clone())],
            8,
            8,
            0.0,
            ProceduralExecution::CallingThread,
            PointwiseRenderOptions::new(false, channels),
        )
        .unwrap()
        .unwrap()
    }
    #[test]
    fn retired_caustics_render_stops_between_sample_rows() {
        let height = SharedImageMap::new(ImageGeometry::new(8, 8));
        height.materialize(0).unwrap();
        let checks = std::cell::Cell::new(0);
        let cancelled = || {
            checks.set(checks.get() + 1);
            checks.get() == 4
        };
        let result = retained(
            &filter(),
            &[Some(height)],
            ImageGeometry::new(8, 8),
            0.,
            PointwiseRenderOptions::new(false, true),
            &cancelled,
        );
        assert!(matches!(result, Err(PointwiseError::Cancelled)));
        assert_eq!(checks.get(), 4);
    }
    #[test]
    fn caustics_factory_wire_scale_and_detached_guarantees() {
        let node = crate::create_node_by_identity::<()>(Some(
            &"Caustics".encode_utf16().collect::<Vec<_>>(),
        ))
        .unwrap();
        let f = node.filter().unwrap();
        assert_eq!(
            node.slot(0).unwrap().name(),
            "Heightmap".encode_utf16().collect::<Vec<_>>()
        );
        assert_eq!(f.parameter(4).unwrap().number(), 2.0);
        assert_eq!(f.parameter(6).unwrap().pair().unwrap(), [0.0; 2]);
        assert_eq!(
            f.parameter(4)
                .unwrap()
                .limits()
                .unwrap()
                .snapshot()
                .big_increment
                .to_framework_f32(),
            1.0
        );
        f.parameter(6).unwrap().set_pair([1.0, -2.0]).unwrap();
        let mut writer = crate::BinaryWriter::default();
        node.save_binary(&mut writer).unwrap();
        let mut reader = crate::BinaryCursor::new(writer.bytes()).unwrap();
        let name = reader.read_string().unwrap();
        let copy = crate::load_node_binary::<()>(&mut reader, 3.75, Some(&name)).unwrap();
        assert_eq!(
            copy.filter().unwrap().parameter(6).unwrap().pair().unwrap(),
            [1.0, -2.0]
        );
        f.commit_spatial_scale(2.0).unwrap();
        assert_eq!(f.parameter(2).unwrap().number(), 0.05);
        assert_eq!(f.parameter(1).unwrap().number(), 0.5);
        let guarantees = node.calculate_image_guarantees(None).unwrap();
        assert_eq!(
            guarantees.constants(),
            [None, Some(0.0), Some(0.0), Some(0.0)]
        );
        node.prepare_image_guarantees(&mut |_| Ok(())).unwrap();
        assert_eq!(node.prepared_image_guarantees().unwrap(), guarantees);
        let rendered = node.render_original(8, 8).unwrap();
        assert!(rendered.caught_errors().is_empty());
        assert_eq!(
            rendered.image().unwrap().geometry(),
            ImageGeometry::new(8, 8)
        );
    }
    #[test]
    fn scalar_height_accumulates_exposure_and_copies_only_requested_maps() {
        let f = filter();
        let input = SharedImageMap::new(ImageGeometry::new(8, 8));
        let named = SharedFloatStore::new(FloatBuffer::new(vec![0.75])).unwrap();
        input
            .add_named_store(Some("Detail".encode_utf16().collect()), Some(named))
            .unwrap();
        let output = render(&f, &input, false);
        assert!((output.channel(0, true).unwrap().read(0).unwrap() - 0.1).abs() < 0.00001);
        assert!(output.named_channels().is_empty());
        let output = render(&f, &input, true);
        assert_eq!(output.named_channels().len(), 1);
        f.parameter(3).unwrap().set_number(0.0).unwrap();
        assert_eq!(
            render(&f, &input, false)
                .channel(0, true)
                .unwrap()
                .read(0)
                .unwrap(),
            0.0
        );
    }
    #[test]
    fn varying_height_and_seeded_pattern_change_real_accumulation() {
        let f = filter();
        let input = SharedImageMap::new(ImageGeometry::new(8, 8));
        input
            .set_buffer(
                0,
                Some(&FloatBuffer::new(
                    (0..64).map(|i| ((i % 8) * (i / 8)) as f32 / 49.0).collect(),
                )),
            )
            .unwrap();
        let regular = render(&f, &input, false).channel(0, true).unwrap().values();
        f.parameter(5).unwrap().set_index(1).unwrap();
        let random = render(&f, &input, false).channel(0, true).unwrap().values();
        assert_ne!(regular, random);
        assert!(random.iter().any(|v| *v > 0.0));
        assert_eq!(
            random,
            render(&f, &input, false).channel(0, true).unwrap().values()
        );
    }
}
