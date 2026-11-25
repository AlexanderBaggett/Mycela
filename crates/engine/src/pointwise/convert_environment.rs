//! `ady.k8` and `abf` environment projection paths.
use super::{PointwiseError, PointwiseFilter, SharedImageMap, check_render_cancelled};
use crate::{FloatBuffer, ImageError, ImageGeometry, ProceduralExecution};
use std::f64::consts::PI;

fn sign(value: f64) -> f64 {
    if value > 0.0 {
        1.0
    } else if value < 0.0 {
        -1.0
    } else {
        0.0
    }
}
pub(super) fn sample<E>(
    source: &FloatBuffer,
    x: f32,
    y: f32,
    w: i32,
    h: i32,
) -> Result<f32, PointwiseError<E>> {
    if source.len() == 1 {
        Ok(source.read(0)?)
    } else {
        super::rotate_environment::sample(x, y, source, w, h)
    }
}

// abf.h projects the source on the caller's latitude–longitude grid.
pub(crate) fn project_to_latlong<E>(
    source: &SharedImageMap,
    format: i32,
    geometry: ImageGeometry,
    cancelled: &dyn Fn() -> bool,
) -> Result<SharedImageMap, PointwiseError<E>> {
    let sw = source.geometry().width();
    let sh = source.geometry().height();
    let w = geometry.width();
    let h = geometry.height();
    if sw <= 0 || sh <= 0 || h <= 0 {
        return Err(PointwiseError::UnsupportedCapability(
            "Convert Environment nonpositive projection geometry",
        ));
    }
    let result = SharedImageMap::new(geometry);
    let count = if source.has_transparency()? { 4 } else { 3 };
    let padded = if format >= 3 {
        Some(super::convert_environment_cross::padded::<E>(
            source, format, cancelled,
        )?)
    } else {
        None
    };
    let sampled = padded.as_ref().unwrap_or(source);
    let sample_width = sampled.geometry().width();
    let sample_height = sampled.geometry().height();
    let planes = (0..count)
        .map(|ch| sampled.channel(ch, true))
        .collect::<Result<Vec<_>, _>>()?;
    let mut output = (0..count).map(|_| Vec::<f32>::new()).collect::<Vec<_>>();
    let area = usize::try_from(w.checked_mul(h).ok_or(ImageError::Overflow)?)
        .map_err(|_| ImageError::Overflow)?;
    for plane in &mut output {
        plane
            .try_reserve_exact(area)
            .map_err(|_| ImageError::Allocation)?;
    }
    for y in 0..h {
        check_render_cancelled(cancelled)?;
        let latitude = (f64::from(y) * (2.0 / f64::from(h)) - 1.0) * (PI / 2.0);
        let cos = latitude.cos();
        for x in 0..w {
            let longitude = (f64::from(x) * (2.0 / f64::from(w)) - 1.0) * PI;
            let dx = cos * longitude.sin();
            let dy = latitude.sin();
            let dz = cos * longitude.cos();
            let (u, v) = if format >= 3 {
                let (u, v) = super::convert_environment_cross::direction_coordinate(
                    dx, dy, dz, sw, sh, format,
                );
                (super::numeric::add(u, 1.0), super::numeric::add(v, 1.0))
            } else {
                let angle = (dx * dx + dy * dy).sqrt().atan2(dz);
                let radius = if format == 1 {
                    angle / PI
                } else {
                    (angle * 0.5).sin()
                };
                let phi = dy.atan2(dx);
                (
                    (f64::from(sw - 1) * (radius * phi.cos() + 1.0) * 0.5) as f32,
                    (f64::from(sh - 1) * (radius * phi.sin() + 1.0) * 0.5) as f32,
                )
            };
            for (dest, plane) in output.iter_mut().zip(&planes) {
                dest.push(sample::<E>(plane, u, v, sample_width, sample_height)?);
            }
        }
    }
    for (ch, values) in output.into_iter().enumerate() {
        result.set_buffer(ch as i32, Some(&FloatBuffer::new(values)))?;
    }
    // abf.a seals its stores temporarily, then ady.k8 seals the intermediate.
    result.seal_stores_temporarily()?;
    result.seal_permanently()?;
    Ok(result)
}

// abf.a/j generate source coordinates; asp.c leaves pixels outside the disk at zero.
fn disk_coordinate(format: i32, x: i32, y: i32, size: i32, sw: i32, sh: i32) -> Option<(f32, f32)> {
    let a = (f64::from(x) + 0.5) * (2.0 / f64::from(size)) - 1.0;
    let b = (f64::from(y) + 0.5) * (2.0 / f64::from(size)) - 1.0;
    let radius_squared = a * a + b * b;
    let radius = radius_squared.sqrt();
    let outside = if format == 1 {
        radius > 1.02
    } else {
        radius_squared > 1.02
    };
    if outside {
        return None;
    }
    let (z, dx, dy) = if format == 1 {
        let tan2 = (radius * PI).tan().powi(2);
        let mut ratio = (b / a).powi(2);
        if a.abs() < 1e-5 {
            ratio = if b > 0.0 {
                f64::INFINITY
            } else {
                f64::NEG_INFINITY
            };
        }
        let z2 = 1.0 / (1.0 + tan2);
        let x2 = (1.0 - z2) / (1.0 + ratio);
        let y2 = 1.0 - z2 - x2;
        let mut z = z2.sqrt();
        if radius * PI >= PI / 2.0 {
            z = -z;
        }
        (z, sign(a) * x2.sqrt(), sign(b) * y2.sqrt())
    } else {
        let tan = (radius.min(1.0).asin() * 2.0).tan();
        let tan2 = tan * tan;
        let ratio = if a.abs() < 1e-5 {
            if b > 0.0 {
                f64::INFINITY
            } else {
                f64::NEG_INFINITY
            }
        } else {
            (b / a).powi(2)
        };
        let z2 = 1.0 / (1.0 + tan2);
        let x2 = (1.0 - z2) / (1.0 + ratio);
        let y2 = 1.0 - z2 - x2;
        let mut z = z2.sqrt();
        if radius > (PI / 4.0).sin() {
            z = -z;
        }
        let mut dx = x2.sqrt();
        if a < 0.0 {
            dx = -dx;
        }
        (z, dx, sign(b) * y2.sqrt())
    };
    let longitude = dx.atan2(z);
    let sine = longitude.sin();
    let latitude = if sine.abs() > 1e-6 {
        dy.atan2(dx / sine)
    } else {
        dy.atan2(z / longitude.cos())
    };
    Some((
        ((longitude / PI + 1.0) / (2.0 / f64::from(sw))) as f32,
        ((latitude / (PI / 2.0) + 1.0) / (2.0 / f64::from(sh))) as f32,
    ))
}
pub(crate) fn from_latlong<E>(
    source: &SharedImageMap,
    format: i32,
    size: i32,
    cancelled: &dyn Fn() -> bool,
) -> Result<SharedImageMap, PointwiseError<E>> {
    let sw = source.geometry().width();
    let sh = source.geometry().height();
    if size <= 0 || sw <= 0 || sh <= 0 {
        return Err(PointwiseError::UnsupportedCapability(
            "Convert Environment nonpositive projection geometry",
        ));
    }
    let result = SharedImageMap::new(ImageGeometry::new(size, size));
    let count = source.channel_count()?;
    let planes = (0..count)
        .map(|ch| source.channel(ch as i32, true))
        .collect::<Result<Vec<_>, _>>()?;
    let mut output = (0..count).map(|_| Vec::<f32>::new()).collect::<Vec<_>>();
    let area = usize::try_from(size.checked_mul(size).ok_or(ImageError::Overflow)?)
        .map_err(|_| ImageError::Overflow)?;
    for plane in &mut output {
        plane
            .try_reserve_exact(area)
            .map_err(|_| ImageError::Allocation)?;
    }
    for y in 0..size {
        check_render_cancelled(cancelled)?;
        for x in 0..size {
            let coordinate = disk_coordinate(format, x, y, size, sw, sh);
            for (dest, plane) in output.iter_mut().zip(&planes) {
                dest.push(if let Some((u, v)) = coordinate {
                    sample::<E>(plane, u, v, sw, sh)?
                } else {
                    0.0
                });
            }
        }
    }
    for (ch, values) in output.into_iter().enumerate() {
        result.set_buffer(ch as i32, Some(&FloatBuffer::new(values)))?;
        result.seal_channel(ch as i32)?;
    }
    Ok(result)
}

pub(super) fn retained<E: 'static>(
    filter: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    geometry: ImageGeometry,
    time: f64,
    execution: ProceduralExecution,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    check_render_cancelled(cancelled)?;
    let input = inputs
        .first()
        .ok_or(PointwiseError::Index)?
        .as_ref()
        .ok_or(PointwiseError::NullReference)?;
    let from = filter.parameter(0)?.effective_index_at(time)?;
    let to = filter.parameter(1)?.effective_index_at(time)?;
    if from == to {
        return Ok(Some(input.clone()));
    }
    if !(0..=4).contains(&from) || !(0..=4).contains(&to) {
        return Err(PointwiseError::UnsupportedCapability(
            "Convert Environment unknown projection",
        ));
    }
    let source = if from >= 3 {
        super::convert_environment_cross::crop::<E>(input, from, cancelled)?
    } else {
        input.clone()
    };
    let latlong = if from == 0 {
        source.clone()
    } else {
        let width = source.geometry().width();
        project_to_latlong::<E>(
            &source,
            from,
            ImageGeometry::new(width, width / 2),
            cancelled,
        )?
    };
    let width = geometry.width();
    let result = match to {
        0 => {
            // abf first fits the latitude/longitude grid to the requested width.
            let sized = if latlong.geometry().width() != width {
                super::image_resize::resize_clamped(&latlong, width, width / 2, execution)?
            } else {
                latlong
            };
            check_render_cancelled(cancelled)?;
            super::image_resize::resize_clamped(&sized, width, width, execution)?
        }
        1 | 2 => from_latlong::<E>(&latlong, to, width, cancelled)?,
        3 | 4 => {
            let cross_width = if to == 4 {
                (f64::from(width) / 4.0 * 3.0) as i32
            } else {
                width
            };
            let cross = super::convert_environment_cross::from_latlong::<E>(
                &latlong,
                to,
                cross_width,
                cancelled,
            )?;
            super::convert_environment_cross::square::<E>(&cross, to, width, execution, cancelled)?
        }
        _ => unreachable!(),
    };
    check_render_cancelled(cancelled)?;
    result.set_region_store(source.region_store());
    Ok(Some(result))
}
