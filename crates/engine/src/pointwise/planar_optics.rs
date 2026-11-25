//! Source uy/at8 planar heightmap reflection and refraction workers.
use super::{
    PointwiseError, PointwiseFilter, PointwiseKind, PointwiseRenderOptions, SharedImageMap,
};
use crate::{FloatBuffer, ImageGeometry, ProceduralExecution};
use std::f64::consts::PI;

struct Surface {
    factor: f64,
    elevation: f64,
    inverse_index: Option<f64>,
}

#[allow(clippy::manual_range_contains)] // Source ordered comparisons preserve NaNs.
fn render_group<E>(
    height: &FloatBuffer,
    channels: &[(FloatBuffer, FloatBuffer)],
    geometry: ImageGeometry,
    Surface {
        factor,
        elevation,
        inverse_index,
    }: Surface,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), PointwiseError<E>> {
    // The original final renderer supplies square grids. On a rectangular
    // grid the worker visits width * height pixels with a width-sized stride.
    let (w, h) = (geometry.width(), geometry.height());
    let count = w.wrapping_mul(h);
    let sx = w as f64 * factor;
    let sy = h as f64 * factor;
    for i in 0..count {
        let x = i % w;
        if x == 0 {
            super::check_render_cancelled(cancelled)?;
        }
        let y = i / w;
        let row = y.wrapping_mul(w);
        let value = super::transform::read(height, i)? as f64;
        let depth = if inverse_index.is_some() {
            value * factor + elevation
        } else {
            1.0 + elevation - value * factor
        };
        if depth <= 0.0 {
            if inverse_index.is_some() {
                for (src, dst) in channels {
                    super::transform::write(dst, i, super::transform::read(src, i)?)?;
                }
            }
            continue;
        }
        let right = if x != w - 1 { i.wrapping_add(1) } else { row };
        let below = if y != h - 1 { i.wrapping_add(w) } else { x };
        let dx = (super::transform::read(height, right)? as f64 - value) * sx;
        let dy = (super::transform::read(height, below)? as f64 - value) * sy;
        let inv = 1.0 / (dx * dx + dy * dy + 1.0).sqrt();
        let nx = dx * inv;
        let ny = dy * inv;
        let nz = -inv;
        let norm = nx * nx + ny * ny;
        let (ox, oy) = if norm == 0.0 {
            (0.0, 0.0)
        } else {
            let n = norm.sqrt();
            let length = (norm + nz * nz).sqrt();
            let ratio = length / n;
            if let Some(index) = inverse_index {
                let sine = n / length;
                let refracted = sine * index;
                if !(-1.0..=1.0).contains(&refracted) && !refracted.is_nan() {
                    (0.0, 0.0)
                } else {
                    let angle = sine.asin() - refracted.asin();
                    let offset = angle.tan() * depth;
                    (offset * (nx * ratio), offset * (ny * ratio))
                }
            } else {
                let a = (1.0 / ratio).asin();
                let mut reflected = -2.0 * a;
                if reflected == -PI / 2.0 || reflected == PI / 2.0 {
                    continue;
                }
                if reflected < -PI / 2.0 || reflected > PI / 2.0 {
                    reflected = -reflected;
                }
                let offset = reflected.tan() * depth;
                (offset * (nx * ratio), offset * (ny * ratio))
            }
        };
        let mut px = ((x as f64 + ox * w as f64) % w as f64) as f32;
        if px < 0.0 {
            px += w as f32;
        }
        let mut py = ((y as f64 + oy * h as f64) % h as f64) as f32;
        if py < 0.0 {
            py += h as f32;
        }
        for (src, dst) in channels {
            let value = super::transform::sample(px, py, src, w, h)?;
            super::transform::write(dst, i, value)?;
        }
    }
    Ok(())
}

pub(super) fn refract<E>(
    input: &SharedImageMap,
    height: &FloatBuffer,
    factor: f64,
    index: f64,
    cancelled: &dyn Fn() -> bool,
) -> Result<SharedImageMap, PointwiseError<E>> {
    let geometry = input.geometry();
    let output = SharedImageMap::new(geometry);
    let mut channels = Vec::new();
    for channel in 0..4 {
        let source = input.channel(channel, true)?;
        if source.len() == 1 {
            output.set_constant(channel, source.read(0)?)?;
        } else {
            channels.push((source, output.materialize(channel)?));
        }
    }
    let index = if index == 0. { 0.000001 } else { index };
    let surface = Surface {
        factor,
        elevation: 1.,
        inverse_index: Some(index.recip()),
    };
    render_group(height, &channels, geometry, surface, cancelled)?;
    Ok(output)
}

pub(super) fn retained<E: 'static>(
    f: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    geometry: ImageGeometry,
    time: f64,
    execution: ProceduralExecution,
    options: PointwiseRenderOptions,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    super::check_render_cancelled(cancelled)?;
    let base = inputs.first().ok_or(PointwiseError::Index)?.as_ref();
    let height = inputs
        .get(1)
        .ok_or(PointwiseError::Index)?
        .as_ref()
        .ok_or(PointwiseError::NullReference)?;
    let w = geometry.width();
    let h = geometry.height();
    let rectangular = w > 0 && h > 0 && w != h;
    let height = if rectangular {
        super::image_resize::resize(height, w, h, execution)?
    } else {
        height.clone()
    };
    let map = height.channel(0, false)?;
    let base = match base {
        Some(base) if rectangular => Some(super::image_resize::resize(base, w, h, execution)?),
        base => base.cloned(),
    };
    if map.len() == 1 {
        return Ok(base);
    }
    let base = base.ok_or(PointwiseError::NullReference)?;
    if w <= 0 || h <= 0 {
        return Err(PointwiseError::UnsupportedCapability(
            "planar optics nonpositive geometry",
        ));
    }
    let output = SharedImageMap::new(geometry);
    let refraction = f.kind() == PointwiseKind::Refract;
    let (factor, index, elevation) = if refraction {
        let factor = f.parameter(2)?.effective_value_at(time)? / f.factor();
        let index = f.parameter(0)?.effective_value_at(time)?;
        let index = if index == 0.0 { 0.000001 } else { index };
        let elevation = f.parameter(1)?.effective_value_at(time)? / f.factor();
        base.seal_stores_temporarily()?;
        (factor, Some(1.0 / index), elevation)
    } else {
        // Reflection's worker reads properties after selecting its channel buffers.
        (0.0, None, 0.0)
    };
    let large = if refraction {
        super::transform::cast(((w.wrapping_mul(h)) as f64).sqrt()) >= 4000
    } else {
        w >= 4700
    };
    let mut channels = Vec::new();
    for ch in 0..4 {
        let source = base.channel(ch, refraction)?;
        if source.len() > 1 {
            let dest = output.materialize(ch)?;
            if large {
                let (factor, elevation) = if refraction {
                    (factor, elevation)
                } else {
                    (
                        f.parameter(1)?.effective_value_at(time)? / f.factor(),
                        f.parameter(0)?.effective_value_at(time)?,
                    )
                };
                let surface = Surface {
                    factor,
                    elevation,
                    inverse_index: index,
                };
                render_group(&map, &[(source, dest)], geometry, surface, cancelled)?;
            } else {
                channels.push((source, dest));
            }
        } else {
            output.set_constant(ch, source.read(0)?)?;
        }
        if large {
            output.seal_channel(ch)?;
        }
    }
    if !large {
        let (factor, elevation) = if refraction {
            (factor, elevation)
        } else {
            (
                f.parameter(1)?.effective_value_at(time)? / f.factor(),
                f.parameter(0)?.effective_value_at(time)?,
            )
        };
        let surface = Surface {
            factor,
            elevation,
            inverse_index: index,
        };
        render_group(&map, &channels, geometry, surface, cancelled)?;
    }
    if options.is_channel_rendering() {
        output.merge_effects(&[Some(base), Some(height)])?;
    }
    Ok(Some(output))
}
pub(super) fn commit_scale<E: 'static>(
    f: &PointwiseFilter<E>,
    scale: f64,
) -> Result<(), PointwiseError<E>> {
    let ix: &[i32] = if f.kind() == PointwiseKind::Reflect {
        &[1]
    } else {
        &[2, 1]
    };
    for i in ix {
        let p = f.parameter(*i)?;
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
    fn filter(kind: PointwiseKind) -> PointwiseFilter<()> {
        PointwiseFilter::new(kind).unwrap()
    }
    fn dense(value: impl Fn(usize) -> f32) -> SharedImageMap {
        let image = SharedImageMap::new(ImageGeometry::new(4, 4));
        image
            .set_buffer(0, Some(&FloatBuffer::new((0..16).map(value).collect())))
            .unwrap();
        image
    }
    fn render(
        f: &PointwiseFilter<()>,
        base: &SharedImageMap,
        height: &SharedImageMap,
    ) -> SharedImageMap {
        f.render_supplied_at_time(
            &[Some(base.clone()), Some(height.clone())],
            4,
            4,
            0.0,
            ProceduralExecution::CallingThread,
            PointwiseRenderOptions::new(false, false),
        )
        .unwrap()
        .unwrap()
    }
    #[test]
    fn planar_optics_factory_wire_scale_and_flat_height_bypass() {
        for kind in [PointwiseKind::Reflect, PointwiseKind::Refract] {
            let node = crate::create_node_by_identity::<()>(Some(
                &kind.name().encode_utf16().collect::<Vec<_>>(),
            ))
            .unwrap();
            let f = node.filter().unwrap();
            assert_eq!(kind.input_count(), 2);
            assert_eq!(
                node.slot(0).unwrap().name(),
                "Base".encode_utf16().collect::<Vec<_>>()
            );
            assert_eq!(
                node.slot(1).unwrap().name(),
                "Heightmap".encode_utf16().collect::<Vec<_>>()
            );
            let ix = if kind == PointwiseKind::Reflect { 1 } else { 2 };
            f.parameter(ix).unwrap().set_number(0.2).unwrap();
            let mut writer = crate::BinaryWriter::default();
            node.save_binary(&mut writer).unwrap();
            let mut reader = crate::BinaryCursor::new(writer.bytes()).unwrap();
            let name = reader.read_string().unwrap();
            let loaded = crate::load_node_binary::<()>(&mut reader, 3.75, Some(&name)).unwrap();
            assert_eq!(
                loaded.filter().unwrap().parameter(ix).unwrap().number(),
                0.2
            );
            f.commit_spatial_scale(2.0).unwrap();
            assert_eq!(f.parameter(ix).unwrap().number(), 0.1);
            let base = dense(|i| i as f32);
            let height = SharedImageMap::new(ImageGeometry::new(4, 4));
            assert!(render(&f, &base, &height).same_reference(&base));
            let base_input = crate::PointwiseNode::input();
            base_input.set_input_image(Some(base.clone())).unwrap();
            node.slot(0).unwrap().set_node(Some(&base_input)).unwrap();
            let result = node.render_original(4, 4).unwrap();
            assert!(result.caught_errors().is_empty());
            assert_eq!(
                result.image().unwrap().channel(0, false).unwrap().values(),
                base.channel(0, false).unwrap().values(),
            );
        }
    }
    #[test]
    fn workers_handle_slopes_and_below_surface_differently() {
        let base = dense(|i| i as f32 / 16.0);
        let height = dense(|i| (i % 4) as f32 / 4.0 + 0.1);
        let reflected = filter(PointwiseKind::Reflect);
        reflected.parameter(1).unwrap().set_number(0.25).unwrap();
        let refracted = filter(PointwiseKind::Refract);
        refracted.parameter(2).unwrap().set_number(0.25).unwrap();
        let a = render(&reflected, &base, &height)
            .channel(0, true)
            .unwrap()
            .values();
        let b = render(&refracted, &base, &height)
            .channel(0, true)
            .unwrap()
            .values();
        assert_ne!(a, base.channel(0, true).unwrap().values());
        assert_ne!(a, b);
        reflected.parameter(0).unwrap().set_number(-1.0).unwrap();
        refracted.parameter(1).unwrap().set_number(-1.0).unwrap();
        assert_eq!(
            render(&reflected, &base, &height)
                .channel(0, true)
                .unwrap()
                .values(),
            vec![0.0; 16]
        );
        assert_eq!(
            render(&refracted, &base, &height)
                .channel(0, true)
                .unwrap()
                .values(),
            base.channel(0, true).unwrap().values()
        );
    }

    #[test]
    fn planar_optics_cancel_between_rows() {
        use crate::PointwiseRenderEffects;
        use std::cell::Cell;

        let base = dense(|i| i as f32 / 16.0);
        let height = dense(|i| (i % 4) as f32 / 4.0);
        for kind in [PointwiseKind::Reflect, PointwiseKind::Refract] {
            let checks = Cell::new(0);
            let cancelled = || {
                checks.set(checks.get() + 1);
                checks.get() >= 3
            };
            let mut capacity = |_| Ok(true);
            let mut preflight = |_, _| Ok(());
            let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight)
                .with_cancel_check(&cancelled);
            let result = filter(kind).render_supplied_at_time_with_effects(
                &[Some(base.clone()), Some(height.clone())],
                4,
                4,
                0.,
                ProceduralExecution::CallingThread,
                PointwiseRenderOptions::new(false, true),
                &mut effects,
            );
            assert!(matches!(result, Err(PointwiseError::Cancelled)), "{kind:?}");
            assert_eq!(checks.get(), 3);
        }
    }

    #[test]
    fn planar_optics_use_both_axes_of_the_render_grid() {
        let surface = |w: i32, h: i32, value: fn(f32, f32) -> f32| {
            let image = SharedImageMap::new(ImageGeometry::new(w, h));
            let pixels = (0..w * h)
                .map(|i| value((i % w) as f32 / w as f32, (i / w) as f32 / h as f32))
                .collect();
            image
                .set_buffer(0, Some(&FloatBuffer::new(pixels)))
                .unwrap();
            image
        };
        for kind in [PointwiseKind::Reflect, PointwiseKind::Refract] {
            let filter = filter(kind);
            let height_index = if kind == PointwiseKind::Reflect { 1 } else { 2 };
            filter
                .parameter(height_index)
                .unwrap()
                .set_number(0.125)
                .unwrap();
            let samples = [(16, 16), (32, 16), (16, 32)].map(|(w, h)| {
                let base = surface(w, h, |_, y| y);
                let height = surface(w, h, |x, y| 0.25 + 0.25 * x + 0.5 * y);
                let result = filter
                    .render_supplied_at_time(
                        &[Some(base), Some(height)],
                        w,
                        h,
                        0.0,
                        ProceduralExecution::CallingThread,
                        PointwiseRenderOptions::new(false, false),
                    )
                    .unwrap()
                    .unwrap();
                assert_eq!(result.geometry(), ImageGeometry::new(w, h));
                let red = result.channel(0, false).unwrap();
                assert_eq!(red.len(), (w * h) as usize);
                // These positions test the vertical derivative and both output halves.
                [h / 4, 3 * h / 4].map(|y| red.read((y * w + w / 4) as usize).unwrap())
            });
            assert!((samples[0][0] - 0.25).abs() > 0.01);
            for rectangular in &samples[1..] {
                for (actual, expected) in rectangular.iter().zip(samples[0]) {
                    assert!((actual - expected).abs() < 1e-6);
                }
            }
        }
    }
    #[test]
    fn guarantees_short_circuit_on_height_uniformity_and_elevation() {
        for kind in [PointwiseKind::Reflect, PointwiseKind::Refract] {
            let f = filter(kind);
            let base = crate::PointwiseChannelGuarantees::new([None; 4], [true; 4], false, None);
            let flat = crate::PointwiseChannelGuarantees::new(
                [Some(0.5), None, None, None],
                [true; 4],
                false,
                None,
            );
            assert_eq!(
                f.node()
                    .calculate_image_guarantees_for_inputs(&[Some(base), Some(flat)])
                    .unwrap()
                    .passthrough_slot(),
                Some(0)
            );
            let varying = crate::PointwiseChannelGuarantees::new([None; 4], [true; 4], false, None);
            assert_eq!(
                f.node()
                    .calculate_image_guarantees_for_inputs(&[Some(base), Some(varying)])
                    .unwrap()
                    .passthrough_slot(),
                None
            );
            let h = crate::PointwiseNode::<()>::solid_color().unwrap();
            let b = crate::PointwiseNode::<()>::solid_color().unwrap();
            f.node().slot(0).unwrap().set_node(Some(&b)).unwrap();
            f.node().slot(1).unwrap().set_node(Some(&h)).unwrap();
            f.node().prepare_image_guarantees(&mut |_| Ok(())).unwrap();
            assert_eq!(
                f.node()
                    .prepared_image_guarantees()
                    .unwrap()
                    .passthrough_slot(),
                Some(0)
            );
        }
    }
}
