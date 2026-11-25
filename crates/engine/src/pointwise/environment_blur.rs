//! Source ea/ng and the shared af7 latitude-aware box workers.
use super::numeric::{add, mul, sub};
use super::{
    PointwiseError, PointwiseFilter, PointwiseKind, SharedImageMap, check_render_cancelled,
};
use crate::{FloatBuffer, ImageError, ImageGeometry};

fn cast(v: f64) -> i32 {
    crate::framework_cast::integer(v)
}
fn at(x: i32, mut y: i32, w: i32, h: i32) -> i32 {
    let mut x = x;
    if y < 0 {
        x = x.wrapping_add(w / 2);
        if x >= w {
            x = x.wrapping_sub(w);
        }
        y = y.wrapping_neg();
    }
    if y >= h {
        x = x.wrapping_sub(w / 2);
        if x < 0 {
            x = x.wrapping_add(w);
        }
        y = h.wrapping_sub(y.wrapping_sub(h.wrapping_sub(1)));
    }
    y.wrapping_mul(w).wrapping_add(x)
}
fn index(i: i32) -> Result<usize, ImageError> {
    usize::try_from(i).map_err(|_| ImageError::Index)
}
fn read(a: &FloatBuffer, i: i32) -> Result<f32, ImageError> {
    a.read(index(i)?)
}
fn fresh<E>(a: &[FloatBuffer]) -> Result<Vec<FloatBuffer>, PointwiseError<E>> {
    a.iter()
        .map(|p| {
            let mut v = Vec::new();
            v.try_reserve_exact(p.len())
                .map_err(|_| ImageError::Allocation)?;
            v.resize(p.len(), 0.0);
            if p.len() == 1 {
                v[0] = p.read(0)?;
            }
            Ok(FloatBuffer::new(v))
        })
        .collect()
}
fn pass<E>(
    src: &[FloatBuffer],
    rx: i32,
    ry: i32,
    w: i32,
    h: i32,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<FloatBuffer>, PointwiseError<E>> {
    if w <= 0 || h <= 0 {
        return Err(PointwiseError::UnsupportedCapability(
            "environment blur nonpositive geometry",
        ));
    }
    let horizontal = fresh(src)?;
    for y in 0..h {
        check_render_cancelled(cancelled)?;
        let half = h / 2;
        let mut lat = if y >= half {
            (y - half + 1) as f32
        } else {
            (half - y) as f32
        };
        // Keep the pole sample inside the domain of the latitude correction.
        if lat >= h as f32 * 0.5 {
            lat = sub(lat, 1.0);
        }
        lat /= h as f32 * 0.5;
        let r = cast(f64::from(
            rx as f32 * (1.0 / (1.0 - f64::from(mul(lat, lat))).sqrt() as f32),
        ));
        let r = r.min((w / 2 - 2).max(0));
        let lat_weight = ((y as f64 / h as f64 * 2.0 * std::f64::consts::PI).cos() * 0.5 + 0.5)
            * (2.3 - 0.8)
            + 0.8;
        let r = cast((r as f64 * lat_weight).round_ties_even());
        let inverse = 1.0_f32 / r.wrapping_mul(2).wrapping_add(1) as f32;
        let mut sum = vec![0.0_f32; src.len()];
        for x in r.wrapping_neg()..=r {
            let i = y.wrapping_mul(w).wrapping_add(x.rem_euclid(w));
            for (ch, p) in src.iter().enumerate() {
                if p.len() > 1 {
                    sum[ch] = add(sum[ch], read(p, i)?);
                }
            }
        }
        for x in 0..w {
            let out = y.wrapping_mul(w).wrapping_add(x);
            let old = y
                .wrapping_mul(w)
                .wrapping_add(x.wrapping_sub(r).rem_euclid(w));
            let new = y
                .wrapping_mul(w)
                .wrapping_add(x.wrapping_add(r).wrapping_add(1).rem_euclid(w));
            for (ch, p) in src.iter().enumerate() {
                if p.len() > 1 {
                    horizontal[ch].write(index(out)?, mul(sum[ch], inverse))?;
                    sum[ch] = add(sub(sum[ch], read(p, old)?), read(p, new)?);
                }
            }
        }
    }
    let output = fresh(&horizontal)?;
    for x in 0..w {
        check_render_cancelled(cancelled)?;
        let mut sum = vec![0.0_f32; src.len()];
        let mut count = 0.0_f32;
        for y in ry.wrapping_neg()..=ry {
            let i = at(x, y, w, h);
            for (ch, p) in horizontal.iter().enumerate() {
                if p.len() > 1 {
                    sum[ch] = add(sum[ch], read(p, i)?);
                }
            }
            count = add(count, 1.0);
        }
        for y in 0..h {
            let i = at(x, y, w, h);
            let old = at(x, y.wrapping_sub(ry), w, h);
            let new = at(x, y.wrapping_add(ry).wrapping_add(1), w, h);
            let inverse = 1.0_f32 / count;
            for (ch, p) in horizontal.iter().enumerate() {
                if p.len() > 1 {
                    output[ch].write(index(i)?, mul(sum[ch], inverse))?;
                    sum[ch] = add(sub(sum[ch], read(p, old)?), read(p, new)?);
                }
            }
        }
    }
    Ok(output)
}
fn blend_into<E>(
    output: &[FloatBuffer],
    blurred: &[FloatBuffer],
    original: &[FloatBuffer],
    strength: f32,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), PointwiseError<E>> {
    let other = sub(1.0, strength);
    for ((output, blurred), original) in output.iter().zip(blurred).zip(original) {
        for i in 0..output.len() {
            if i.is_multiple_of(4096) {
                check_render_cancelled(cancelled)?;
            }
            output.write(
                i,
                add(
                    mul(strength, blurred.read(i)?),
                    mul(other, original.read(i)?),
                ),
            )?;
        }
    }
    Ok(())
}
#[derive(Clone, Copy)]
pub(crate) enum EnvironmentBlur {
    Axes { horizontal: f64, vertical: f64 },
    Scatter { radius: f64, strength: f64 },
    LegacyAxes { horizontal: f64, vertical: f64 },
}

fn blur_axes<E>(
    mut data: Vec<FloatBuffer>,
    w: i32,
    h: i32,
    horizontal: f64,
    vertical: f64,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<FloatBuffer>, PointwiseError<E>> {
    let extent = h.wrapping_sub(1) as f32;
    let third = 1.0_f32 / 3.0;
    let mut x = sub(horizontal as f32 / 100.0 * extent, third);
    let mut y = sub(vertical as f32 / 100.0 * extent, third);
    for _ in 0..3 {
        let a = cast(f64::from(x).round_ties_even());
        let b = cast(f64::from(y).round_ties_even());
        if a > 0 || b > 0 {
            data = pass(&data, a, b, w, h, cancelled)?;
        }
        x = add(x, third);
        y = add(y, third);
    }
    Ok(data)
}

fn run<E>(
    mut data: Vec<FloatBuffer>,
    w: i32,
    h: i32,
    blur: EnvironmentBlur,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<FloatBuffer>, PointwiseError<E>> {
    match blur {
        EnvironmentBlur::Axes {
            horizontal,
            vertical,
        } => blur_axes(data, w, h, horizontal, vertical, cancelled),
        EnvironmentBlur::LegacyAxes {
            horizontal,
            vertical,
        } => {
            let across = blur_axes(data.clone(), w, h, horizontal, 0., cancelled)?;
            let down = blur_axes(data, w, h, 0., vertical, cancelled)?;
            let result = fresh(&down)?;
            blend_into(&result, &across, &down, 0.5, cancelled)?;
            Ok(result)
        }
        EnvironmentBlur::Scatter { radius, strength } => {
            let extent = h.wrapping_sub(1) as f32;
            let mut r = radius as f32 / 100.0 * extent;
            let strength = strength as f32 / 100.0;
            loop {
                let n = cast(f64::from(r).round_ties_even());
                if n == 0 {
                    break;
                }
                let blurred = pass(&data, n, n, w, h, cancelled)?;
                // The pass owns its result planes, so blending can reuse them.
                blend_into(&blurred, &blurred, &data, strength, cancelled)?;
                data = blurred;
                r /= 4.0;
            }
            Ok(data)
        }
    }
}

pub(super) fn retained<E: 'static>(
    f: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    time: f64,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    let input = inputs.first().ok_or(PointwiseError::Index)?;
    let diffuse = f.kind() == PointwiseKind::DiffuseEnvironment;
    let r = f.parameter(i32::from(diffuse))?.effective_value_at(time)? / f.factor();
    let r = if r > 100.0 { 100.0 } else { r };
    let strength = if diffuse {
        None
    } else {
        Some(f.parameter(1)?.effective_value_at(time)? / f.factor())
    };
    if !(r > 0.0 && strength.is_none_or(|v| v > 0.0)) {
        return Ok(input.clone());
    }
    let blur = match strength {
        Some(strength) => EnvironmentBlur::Scatter {
            radius: r,
            strength,
        },
        None => {
            let direction = f.parameter(0)?.effective_index_at(time)?;
            EnvironmentBlur::Axes {
                horizontal: if direction == 0 || direction == 1 {
                    r
                } else {
                    0.
                },
                vertical: if direction != 1 { r } else { 0. },
            }
        }
    };
    let input = input.as_ref().ok_or(PointwiseError::NullReference)?;
    blur_map(input, blur, cancelled).map(Some)
}

pub(crate) fn blur_map<E>(
    input: &SharedImageMap,
    blur: EnvironmentBlur,
    cancelled: &dyn Fn() -> bool,
) -> Result<SharedImageMap, PointwiseError<E>> {
    check_render_cancelled(cancelled)?;
    let geometry = input.geometry();
    let w = geometry.width();
    let h = geometry.height();
    let count = input.channel_count()?;
    if count == 0 {
        let result = input.geometry_copy();
        result.copy_auxiliary(input)?;
        return Ok(result);
    }
    let output = SharedImageMap::new(ImageGeometry::new(w, h));
    if w < 3000 || matches!(blur, EnvironmentBlur::LegacyAxes { .. }) {
        let data = (0..count)
            .map(|ch| input.channel(ch as i32, true))
            .collect::<Result<Vec<_>, _>>()?;
        let result = run(data, w, h, blur, cancelled)?;
        for (ch, data) in result.iter().enumerate() {
            output.set_buffer(ch as i32, Some(data))?;
            output.seal_channel(ch as i32)?;
        }
    } else {
        for ch in 0..count {
            let data = input.channel(ch as i32, true)?;
            if data.len() == 1 {
                output.set_constant(ch as i32, data.read(0)?)?;
            } else {
                let result = run(vec![data], w, h, blur, cancelled)?;
                output.set_buffer(ch as i32, Some(&result[0]))?;
                output.seal_channel(ch as i32)?;
            }
        }
    }
    output.copy_auxiliary(input)?;
    Ok(output)
}

pub(super) fn commit_scale<E: 'static>(
    filter: &PointwiseFilter<E>,
    scale: f64,
) -> Result<(), PointwiseError<E>> {
    let p = filter.parameter(if filter.kind() == PointwiseKind::DiffuseEnvironment {
        1
    } else {
        0
    })?;
    if super::parameter_ops::is_animated(|| p.retained_animation())? {
        super::point_light::scale_point_light_curve(&p, scale)
    } else {
        let n = p.number() / scale;
        p.set_number(if n > 100.0 { 100.0 } else { n })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PointwiseRenderOptions, ProceduralExecution, SharedFloatStore};
    fn render(f: &PointwiseFilter<()>, image: &SharedImageMap) -> SharedImageMap {
        f.render_supplied_at_time(
            &[Some(image.clone())],
            8,
            4,
            0.0,
            ProceduralExecution::CallingThread,
            PointwiseRenderOptions::new(false, false),
        )
        .unwrap()
        .unwrap()
    }
    #[test]
    fn environment_nodes_retain_source_properties_copy_and_wire() {
        for kind in [
            PointwiseKind::DiffuseEnvironment,
            PointwiseKind::ScatterEnvironment,
        ] {
            let node = crate::create_node_by_identity::<()>(Some(
                &kind.name().encode_utf16().collect::<Vec<_>>(),
            ))
            .unwrap();
            let f = node.filter().unwrap();
            let radius = if kind == PointwiseKind::DiffuseEnvironment {
                1
            } else {
                0
            };
            assert_eq!(f.parameter(radius).unwrap().number(), 30.0);
            assert_eq!(
                node.slot(0).unwrap().name(),
                "Environment".encode_utf16().collect::<Vec<_>>()
            );
            f.parameter(radius).unwrap().set_number(45.0).unwrap();
            assert_eq!(
                f.source_clone()
                    .unwrap()
                    .parameter(radius)
                    .unwrap()
                    .number(),
                45.0
            );
            let mut writer = crate::BinaryWriter::default();
            node.save_binary(&mut writer).unwrap();
            let mut reader = crate::BinaryCursor::new(writer.bytes()).unwrap();
            let name = reader.read_string().unwrap();
            let loaded = crate::load_node_binary::<()>(&mut reader, 3.75, Some(&name)).unwrap();
            assert_eq!(
                loaded.filter().unwrap().parameter(radius).unwrap().number(),
                45.0
            );
            f.commit_spatial_scale(2.0).unwrap();
            assert_eq!(f.parameter(radius).unwrap().number(), 22.5);
            let rendered = node.render_original(8, 4).unwrap();
            assert!(rendered.caught_errors().is_empty());
            assert_eq!(
                rendered.image().unwrap().geometry(),
                ImageGeometry::new(8, 4)
            );
        }
    }
    #[test]
    fn spherical_blur_varies_with_direction_and_preserves_auxiliary_owners() {
        let input = SharedImageMap::new(ImageGeometry::new(8, 4));
        let mut pixels = vec![0.0; 32];
        pixels[10] = 1.0;
        input
            .set_buffer(0, Some(&FloatBuffer::new(pixels.clone())))
            .unwrap();
        let region = SharedFloatStore::new(FloatBuffer::new(vec![0.5; 32])).unwrap();
        input.set_region_store(Some(region.clone()));
        let diffuse = PointwiseFilter::<()>::new(PointwiseKind::DiffuseEnvironment).unwrap();
        diffuse.parameter(1).unwrap().set_number(100.0).unwrap();
        diffuse.parameter(0).unwrap().set_index(1).unwrap();
        let horizontal = render(&diffuse, &input);
        diffuse.parameter(0).unwrap().set_index(2).unwrap();
        let vertical = render(&diffuse, &input);
        assert_ne!(horizontal.channel(0, true).unwrap().values(), pixels);
        assert_ne!(
            horizontal.channel(0, true).unwrap().values(),
            vertical.channel(0, true).unwrap().values()
        );
        assert!(horizontal.region_store().unwrap().same_store(&region));
        let scatter = PointwiseFilter::<()>::new(PointwiseKind::ScatterEnvironment).unwrap();
        scatter.parameter(0).unwrap().set_number(100.0).unwrap();
        assert_ne!(
            render(&scatter, &input).channel(0, true).unwrap().values(),
            pixels
        );
        scatter.parameter(1).unwrap().set_number(0.0).unwrap();
        assert!(render(&scatter, &input).same_reference(&input));
        diffuse.parameter(1).unwrap().set_number(0.0).unwrap();
        assert!(render(&diffuse, &input).same_reference(&input));
    }

    #[test]
    fn horizontal_blur_keeps_odd_pole_and_narrow_rows_in_range() {
        let blur = EnvironmentBlur::Axes {
            horizontal: 100.0,
            vertical: 0.0,
        };
        let odd = SharedImageMap::new(ImageGeometry::new(12, 5));
        let mut pixels = vec![0.0; 60];
        pixels[48] = 1.0;
        odd.set_buffer(0, Some(&FloatBuffer::new(pixels))).unwrap();
        let output = blur_map::<()>(&odd, blur, &|| false).unwrap();
        let output = output.channel(0, true).unwrap().values();
        assert!(output[..48].iter().all(|sample| *sample == 0.0));
        assert!(output[49] > 0.0);

        let narrow = SharedImageMap::new(ImageGeometry::new(3, 12));
        let pixels = (0..36).map(|value| value as f32 / 36.0).collect::<Vec<_>>();
        narrow
            .set_buffer(0, Some(&FloatBuffer::new(pixels.clone())))
            .unwrap();
        let output = blur_map::<()>(&narrow, blur, &|| false).unwrap();
        assert_eq!(output.channel(0, true).unwrap().values(), pixels);
    }

    #[test]
    fn spherical_blur_can_cancel_after_entering_the_first_pass() {
        let image = SharedImageMap::new(ImageGeometry::new(12, 8));
        let pixels = (0..96).map(|i| i as f32 / 96.0).collect();
        image
            .set_buffer(0, Some(&FloatBuffer::new(pixels)))
            .unwrap();
        let checks = std::cell::Cell::new(0);
        let cancel_during_pass = || {
            checks.set(checks.get() + 1);
            checks.get() == 3
        };
        let result = blur_map::<()>(
            &image,
            EnvironmentBlur::Axes {
                horizontal: 45.0,
                vertical: 20.0,
            },
            &cancel_during_pass,
        );
        assert!(matches!(result, Err(PointwiseError::Cancelled)));
    }

    #[test]
    fn environment_guarantees_follow_input_channels_and_range() {
        for kind in [
            PointwiseKind::DiffuseEnvironment,
            PointwiseKind::ScatterEnvironment,
        ] {
            let f = PointwiseFilter::<()>::new(kind).unwrap();
            let input = crate::PointwiseChannelGuarantees::new(
                [Some(0.25), None, Some(2.0), None],
                [false, true, true, false],
                false,
                None,
            );
            let result = f.node().calculate_image_guarantees(Some(input)).unwrap();
            assert_eq!(result.constants(), input.constants());
            assert_eq!(result.in_range(), [true, true, false, false]);
            let child = crate::PointwiseNode::<()>::solid_color().unwrap();
            f.node().slot(0).unwrap().set_node(Some(&child)).unwrap();
            f.node().prepare_image_guarantees(&mut |_| Ok(())).unwrap();
            assert!(f.node().prepared_image_guarantees().is_some());
        }
    }
}
