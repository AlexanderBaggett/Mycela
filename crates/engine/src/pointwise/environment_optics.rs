//! Source t5/avr and asp environment reflection/refraction workers.
use super::numeric::{add, mul};
use super::{
    PointwiseError, PointwiseFilter, PointwiseKind, PointwiseRenderOptions, SharedImageMap,
};
use crate::{FloatBuffer, ImageError, ImageGeometry, ProceduralExecution};
use std::f64::consts::PI;

fn allocate<E>(n: i32) -> Result<FloatBuffer, PointwiseError<E>> {
    let n = usize::try_from(n).map_err(|_| ImageError::Overflow)?;
    let mut v = Vec::new();
    v.try_reserve_exact(n).map_err(|_| ImageError::Allocation)?;
    v.resize(n, 0.0);
    Ok(FloatBuffer::new(v))
}
fn read(p: &FloatBuffer, i: i32) -> Result<f32, ImageError> {
    p.read(usize::try_from(i).map_err(|_| ImageError::Index)?)
}
fn write(p: &FloatBuffer, i: i32, x: f32) -> Result<(), ImageError> {
    p.write(usize::try_from(i).map_err(|_| ImageError::Index)?, x)
}

#[allow(clippy::manual_range_contains)] // Source ordered comparisons preserve NaN behavior.
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
    let height = inputs.first().ok_or(PointwiseError::Index)?.as_ref();
    let environment = inputs
        .get(1)
        .ok_or(PointwiseError::Index)?
        .as_ref()
        .ok_or(PointwiseError::NullReference)?;
    // Keep Genetica's conversion for its square render grid. A rectangular
    // render can sample longitude and latitude in the environment's own grid.
    let environment = if geometry.width() == geometry.height()
        && environment.geometry().width() != environment.geometry().height()
    {
        super::image_resize::resize_clamped(
            environment,
            geometry.width(),
            geometry.width(),
            execution,
        )?
    } else {
        environment.clone()
    };
    let mut factor = f.parameter(0)?.effective_value_at(time)? / f.factor();
    let skip_ix = if f.kind() == PointwiseKind::ReflectEnvironment {
        1
    } else {
        2
    };
    if options.is_color_only() {
        let p = f.parameter(skip_ix)?;
        let ix = usize::try_from(p.effective_index_at(time)?).map_err(|_| PointwiseError::Index)?;
        let entries = p.options()?.entries();
        let text = entries.get(ix).ok_or(PointwiseError::Index)?;
        if text
            .as_ref()
            .is_some_and(|s| s.as_slice() == [89, 101, 115])
        {
            factor = 0.0;
        }
    }
    let index = if f.kind() == PointwiseKind::RefractEnvironment {
        Some(f.parameter(1)?.effective_value_at(time)?)
    } else {
        None
    };
    let height = super::image_resize::resize(
        height.ok_or(PointwiseError::NullReference)?,
        geometry.width(),
        geometry.height(),
        execution,
    )?;
    let output = reflect_sample(&environment, &height, factor, index, cancelled)?;
    output.set_region_store(height.region_store());
    if options.is_channel_rendering() {
        output.merge_effects(&[Some(height)])?;
    }
    Ok(Some(output))
}
/// asp.a: reflect/refract the actual environment through height normals.
#[allow(clippy::manual_range_contains)] // Preserve ordered source NaN comparisons.
pub(super) fn reflect_sample<E>(
    environment: &SharedImageMap,
    height: &SharedImageMap,
    factor: f64,
    index: Option<f64>,
    cancelled: &dyn Fn() -> bool,
) -> Result<SharedImageMap, PointwiseError<E>> {
    let hw = height.geometry().width();
    let hh = height.geometry().height();
    let ew = environment.geometry().width();
    let eh = environment.geometry().height();
    if hw <= 0 || hh <= 0 || ew <= 0 || eh <= 0 {
        return Err(PointwiseError::UnsupportedCapability(
            "environment optics nonpositive geometry",
        ));
    }
    let xmap = allocate(hw.wrapping_mul(hh))?;
    let ymap = allocate(hw.wrapping_mul(hh))?;
    let xp = 1.0 / (2.0 * PI) * ew as f64;
    let yp = 1.0 / PI * eh as f64;
    visit_height_normals(height, factor, cancelled, |i, nx, ny| {
        let a = nx as f64 * 2.0 - 1.0;
        let b = ny as f64 * 2.0 - 1.0;
        let z = 1.0 - a * a - b * b;
        let z = if z > 0.0 {
            (1.0 - a * a - b * b).sqrt()
        } else {
            0.0
        };
        let angle = b.atan2(a);
        let mut theta = z.acos();
        if let Some(n) = index {
            let sin = theta.sin() / n;
            theta = if !((1.0 < sin) || (sin < -1.0) || n.abs() < 1e-5) {
                PI + theta - sin.asin()
            } else {
                theta * 2.0
            };
        } else {
            theta *= 2.0;
        }
        let sin = theta.sin();
        let xdir = sin * angle.cos();
        let ydir = sin * angle.sin();
        let zdir = theta.cos();
        xmap.write(i, ((xdir.atan2(zdir) + PI) * xp) as f32)?;
        ymap.write(i, (ydir.acos() * yp) as f32)?;
        Ok(())
    })?;
    let output = SharedImageMap::new(ImageGeometry::new(hw, hh));
    for ch in 0..environment.channel_count()? {
        let source = environment.channel(ch as i32, false)?;
        let target = output.materialize(ch as i32)?;
        for row in 0..hh {
            super::check_render_cancelled(cancelled)?;
            for i in row * hw..(row + 1) * hw {
                let x = read(&xmap, i)?;
                let y = read(&ymap, i)?;
                let value = if source.len() > 1 {
                    super::rotate_environment::sample(x, y, &source, ew, eh)?
                } else {
                    source.read(0)?
                };
                write(&target, i, value)?;
            }
        }
        output.seal_channel(ch as i32)?;
    }
    Ok(output)
}
// ImageMap.b(depth, false, false): the two stored normal components.
pub(super) fn visit_height_normals<E>(
    height: &SharedImageMap,
    factor: f64,
    cancelled: &dyn Fn() -> bool,
    mut visit: impl FnMut(usize, f32, f32) -> Result<(), PointwiseError<E>>,
) -> Result<(), PointwiseError<E>> {
    let hw = height.geometry().width();
    let hh = height.geometry().height();
    let src = height.channel(0, true)?;
    let sx = f64::from(hw) * factor;
    let sy = f64::from(hh) * factor;
    for y in 0..hh {
        super::check_render_cancelled(cancelled)?;
        let row = y.wrapping_mul(hw);
        let next = if y != hh - 1 {
            (y + 1).wrapping_mul(hw)
        } else {
            0
        };
        let mut last = if src.len() == 1 {
            src.read(0)? as f64
        } else {
            read(&src, row)? as f64
        };
        for x in (0..hw).rev() {
            let i = row.wrapping_add(x);
            let current = if src.len() == 1 {
                src.read(0)? as f64
            } else {
                read(&src, i)? as f64
            };
            let below = if src.len() == 1 {
                current
            } else {
                read(&src, next.wrapping_add(x))? as f64
            };
            let dx = (last - current) * sx;
            let dy = (below - current) * sy;
            let inv = 1.0 / (dx * dx + dy * dy + 1.0).sqrt();
            let nx = if src.len() == 1 {
                0.5
            } else {
                add(mul((-inv * dx) as f32, 0.5), 0.5)
            };
            let ny = if src.len() == 1 {
                0.5
            } else {
                add(mul((inv * dy) as f32, 0.5), 0.5)
            };
            visit(i as usize, nx, ny)?;
            last = current;
        }
    }
    Ok(())
}

pub(super) fn commit_scale<E: 'static>(
    f: &PointwiseFilter<E>,
    scale: f64,
) -> Result<(), PointwiseError<E>> {
    let p = f.parameter(0)?;
    if super::parameter_ops::is_animated(|| p.retained_animation())? {
        super::point_light::scale_point_light_curve(&p, scale)
    } else {
        p.set_number(p.number() * (1.0 / scale))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SharedFloatStore;
    fn render(
        f: &PointwiseFilter<()>,
        h: &SharedImageMap,
        e: &SharedImageMap,
        color: bool,
        channels: bool,
    ) -> SharedImageMap {
        f.render_supplied_at_time(
            &[Some(h.clone()), Some(e.clone())],
            4,
            4,
            0.0,
            ProceduralExecution::CallingThread,
            PointwiseRenderOptions::new(color, channels),
        )
        .unwrap()
        .unwrap()
    }
    fn fixtures() -> (SharedImageMap, SharedImageMap) {
        let h = SharedImageMap::new(ImageGeometry::new(4, 4));
        h.set_buffer(
            0,
            Some(&FloatBuffer::new(
                (0..16).map(|v| (v % 4) as f32 / 3.0).collect(),
            )),
        )
        .unwrap();
        let e = SharedImageMap::new(ImageGeometry::new(4, 4));
        e.set_buffer(
            0,
            Some(&FloatBuffer::new((0..16).map(|v| v as f32).collect())),
        )
        .unwrap();
        (h, e)
    }
    #[test]
    fn optics_construct_and_roundtrip_source_properties() {
        for kind in [
            PointwiseKind::ReflectEnvironment,
            PointwiseKind::RefractEnvironment,
        ] {
            let node = crate::create_node_by_identity::<()>(Some(
                &kind.name().encode_utf16().collect::<Vec<_>>(),
            ))
            .unwrap();
            let f = node.filter().unwrap();
            assert_eq!(kind.input_count(), 2);
            assert_eq!(
                node.slot(0).unwrap().name(),
                "Heightmap".encode_utf16().collect::<Vec<_>>()
            );
            assert_eq!(
                node.slot(1).unwrap().name(),
                "Environment".encode_utf16().collect::<Vec<_>>()
            );
            assert_eq!(f.parameter(0).unwrap().number(), 0.05);
            if kind == PointwiseKind::RefractEnvironment {
                assert_eq!(f.parameter(1).unwrap().number(), 1.333);
            }
            f.parameter(0).unwrap().set_number(0.2).unwrap();
            assert_eq!(
                f.source_clone().unwrap().parameter(0).unwrap().number(),
                0.2
            );
            let mut writer = crate::BinaryWriter::default();
            node.save_binary(&mut writer).unwrap();
            let mut reader = crate::BinaryCursor::new(writer.bytes()).unwrap();
            let name = reader.read_string().unwrap();
            let copy = crate::load_node_binary::<()>(&mut reader, 3.75, Some(&name)).unwrap();
            assert_eq!(copy.filter().unwrap().parameter(0).unwrap().number(), 0.2);
            f.commit_spatial_scale(2.0).unwrap();
            assert_eq!(f.parameter(0).unwrap().number(), 0.1);
            let (height, environment) = fixtures();
            let expected = render(&f, &height, &environment, false, false);
            for (slot, image) in node.slots().iter().zip([height, environment]) {
                let input = crate::PointwiseNode::input();
                input.set_input_image(Some(image)).unwrap();
                slot.set_node(Some(&input)).unwrap();
            }
            let result = node.render_original(4, 4).unwrap();
            assert!(result.caught_errors().is_empty());
            assert_eq!(
                result.image().unwrap().channel(0, false).unwrap().values(),
                expected.channel(0, false).unwrap().values(),
            );
        }
    }
    #[test]
    fn reflection_refraction_sample_the_environment_and_honor_skip() {
        let (h, e) = fixtures();
        let region = SharedFloatStore::new(FloatBuffer::new(vec![0.25; 16])).unwrap();
        h.set_region_store(Some(region.clone()));
        let f = PointwiseFilter::<()>::new(PointwiseKind::ReflectEnvironment).unwrap();
        f.parameter(0).unwrap().set_number(0.5).unwrap();
        let reflected = render(&f, &h, &e, false, false);
        let flat = render(&f, &h, &e, true, false);
        assert!(reflected.region_store().unwrap().same_store(&region));
        assert_ne!(
            reflected.channel(0, true).unwrap().values(),
            flat.channel(0, true).unwrap().values()
        );
        assert!(
            flat.channel(0, true)
                .unwrap()
                .values()
                .iter()
                .all(|v| *v == 10.0)
        );
        let refract = PointwiseFilter::<()>::new(PointwiseKind::RefractEnvironment).unwrap();
        let flat_refract = render(&refract, &h, &e, true, false);
        assert!(
            flat_refract
                .channel(0, true)
                .unwrap()
                .values()
                .iter()
                .all(|v| *v == 8.0)
        );
        let full = render(&refract, &h, &e, false, false);
        assert_ne!(
            full.channel(0, true).unwrap().values(),
            flat_refract.channel(0, true).unwrap().values()
        );
    }

    #[test]
    fn environment_refraction_cancels_during_height_normal_preparation() {
        use crate::PointwiseRenderEffects;
        use std::cell::Cell;

        let refract = PointwiseFilter::<()>::new(PointwiseKind::RefractEnvironment).unwrap();
        let (height, environment) = fixtures();
        let checks = Cell::new(0);
        let cancelled = || {
            checks.set(checks.get() + 1);
            checks.get() >= 3
        };
        let mut capacity = |_| Ok(true);
        let mut preflight = |_, _| Ok(());
        let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight)
            .with_cancel_check(&cancelled);
        let result = refract.render_supplied_at_time_with_effects(
            &[Some(height), Some(environment)],
            4,
            4,
            0.,
            ProceduralExecution::CallingThread,
            PointwiseRenderOptions::new(false, true),
            &mut effects,
        );
        assert!(matches!(result, Err(PointwiseError::Cancelled)));
        assert_eq!(checks.get(), 3);
    }
    #[test]
    fn optics_resize_non_square_environment_and_preserve_conditional_maps() {
        let (h, _) = fixtures();
        let env = SharedImageMap::new(ImageGeometry::new(4, 2));
        env.set_constant(0, 0.6).unwrap();
        let data = SharedFloatStore::new(FloatBuffer::new(vec![0.2; 16])).unwrap();
        h.add_named_store(Some("Detail".encode_utf16().collect()), Some(data))
            .unwrap();
        let f = PointwiseFilter::<()>::new(PointwiseKind::ReflectEnvironment).unwrap();
        let absent = render(&f, &h, &env, false, false);
        let present = render(&f, &h, &env, false, true);
        assert_eq!(present.geometry(), ImageGeometry::new(4, 4));
        assert_eq!(absent.named_channels().len(), 0);
        assert_eq!(present.named_channels().len(), 1);
        assert!(
            present
                .channel(0, true)
                .unwrap()
                .values()
                .iter()
                .all(|v| *v == 0.6)
        );
    }

    #[test]
    fn rectangular_environment_optics_keep_surface_and_lookup_grids_independent() {
        let environment = SharedImageMap::new(ImageGeometry::new(32, 16));
        let pixels = (0..32 * 16).map(|i| (i % 32 + i / 32) as f32 / 48.0);
        environment
            .set_buffer(0, Some(&FloatBuffer::new(pixels.collect())))
            .unwrap();
        for kind in [
            PointwiseKind::ReflectEnvironment,
            PointwiseKind::RefractEnvironment,
        ] {
            let filter = PointwiseFilter::<()>::new(kind).unwrap();
            filter.parameter(0).unwrap().set_number(0.25).unwrap();
            let samples = [(16, 8), (8, 16), (32, 16)].map(|(w, h)| {
                let geometry = ImageGeometry::new(w, h);
                let surface = SharedImageMap::new(geometry);
                let pixels = (0..geometry.area()).map(|i| {
                    let x = (i % w) as f32 / w as f32;
                    let y = (i / w) as f32 / h as f32;
                    0.25 + 0.25 * x + 0.5 * y
                });
                surface
                    .set_buffer(0, Some(&FloatBuffer::new(pixels.collect())))
                    .unwrap();
                let region =
                    SharedFloatStore::new(FloatBuffer::new(vec![0.75; geometry.area() as usize]))
                        .unwrap();
                surface.set_region_store(Some(region.clone()));
                surface
                    .add_named_store(
                        Some("Detail".encode_utf16().collect()),
                        Some(region.clone()),
                    )
                    .unwrap();
                let result = filter
                    .render_supplied_at_time(
                        &[Some(surface), Some(environment.clone())],
                        w,
                        h,
                        0.0,
                        ProceduralExecution::CallingThread,
                        PointwiseRenderOptions::new(false, true),
                    )
                    .unwrap()
                    .unwrap();
                assert_eq!(result.geometry(), geometry);
                assert_eq!(result.named_channels().len(), 1);
                assert!(result.region_store().unwrap().same_store(&region));
                let red = result.channel(0, false).unwrap();
                [h / 4, 3 * h / 4].map(|y| red.read((y * w + w / 4) as usize).unwrap())
            });
            assert_eq!(samples, [samples[0]; 3], "{kind:?}");
        }
    }
}
