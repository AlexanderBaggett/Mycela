//! aa5's Double path integration and distinct nearest Regions worker.
use super::transform::*;
use super::{PointwiseError, PointwiseFilter, PointwiseRenderOptions, SharedImageMap};
use crate::{
    FloatBuffer, ImageGeometry, ProceduralExecution, SharedFloatStore,
    image_map::{read_pixel, write_pixel},
};
#[allow(clippy::too_many_arguments, clippy::manual_range_contains)] // Ordered source comparisons deliberately pass NaN through the wrap gate.
fn worker<E>(
    source: &FloatBuffer,
    dest: &FloatBuffer,
    direction: &FloatBuffer,
    distance: &FloatBuffer,
    geometry: ImageGeometry,
    steps: i32,
    dx: f64,
    dy: f64,
    aa: bool,
    regions: bool,
    execution: ProceduralExecution,
) -> Result<(), PointwiseError<E>> {
    let source = source.shared_values();
    let direction = direction.shared_values();
    let distance = distance.shared_values();
    dest.with_mut_slice(|pixels| {
        let w = geometry.width();
        let h = geometry.height();
        let wf = f64::from(w);
        let hf = f64::from(h);
        let turn = std::f64::consts::PI * 2.;
        crate::procedural_schedule::for_each_chunk(w.wrapping_mul(h), execution, |start, count| {
            let mut row = start.checked_div(w).ok_or(PointwiseError::DivideByZero)?;
            let mut col = rem(start, w)?;
            for cell in start..start.wrapping_add(count) {
                if col >= w {
                    col = 0;
                    row = row.wrapping_add(1);
                }
                let mut x = f64::from(col);
                let mut y = f64::from(row);
                for step in 0..steps {
                    let at = cast(x).wrapping_add(cast(y).wrapping_mul(w));
                    let angle = if direction.len() > 1 {
                        if step > 0 && aa && !regions {
                            f64::from(super::motion_sampling::sample_pixels(
                                x as f32, y as f32, &direction, w, h, true,
                            )?) * turn
                        } else {
                            f64::from(read_pixel(&direction, at)?) * turn
                        }
                    } else {
                        f64::from(read_pixel(&direction, 0)?) * turn
                    };
                    let (sx, sy) = if distance.len() > 1 {
                        if step > 0 && aa && !regions {
                            let d = f64::from(super::motion_sampling::sample_pixels(
                                x as f32, y as f32, &distance, w, h, true,
                            )?);
                            (d * dx, d * dy)
                        } else if regions {
                            (
                                f64::from(read_pixel(&distance, at)?) * dx,
                                f64::from(read_pixel(&distance, at)?) * dy,
                            )
                        } else {
                            let d = f64::from(read_pixel(&distance, at)?);
                            (d * dx, d * dy)
                        }
                    } else if regions {
                        (
                            f64::from(read_pixel(&distance, 0)?) * dx,
                            f64::from(read_pixel(&distance, 0)?) * dy,
                        )
                    } else {
                        let d = f64::from(read_pixel(&distance, 0)?);
                        (d * dx, d * dy)
                    };
                    let (sin, cos) = mycela_framework_math::sin_cos(angle);
                    x -= cos * sx;
                    y -= sin * sy;
                    if regions {
                        x %= wf;
                        y %= hf;
                        if x < 0. {
                            x += wf;
                        }
                        if y < 0. {
                            y += hf;
                        }
                        if x >= wf {
                            x = 0.;
                        }
                        if y >= hf {
                            y = 0.;
                        }
                    } else {
                        if x < 0. || x >= wf {
                            x %= wf;
                            if x < 0. {
                                x += wf;
                            }
                            if x >= wf {
                                x = 0.;
                            }
                        }
                        if y < 0. || y >= hf {
                            y %= hf;
                            if y < 0. {
                                y += hf;
                            }
                            if y >= hf {
                                y = 0.;
                            }
                        }
                    }
                }
                let value = if aa && !regions {
                    super::motion_sampling::sample_pixels(x as f32, y as f32, &source, w, h, true)?
                } else {
                    read_pixel(&source, cast(x).wrapping_add(cast(y).wrapping_mul(w)))?
                };
                write_pixel(pixels, cell, value)?;
                col = col.wrapping_add(1);
            }
            Ok(())
        })
    })
}
pub(super) fn retained<E: 'static>(
    f: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    geometry: ImageGeometry,
    time: f64,
    execution: ProceduralExecution,
    options: PointwiseRenderOptions,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    let original = get(inputs, 0)?;
    let direction = get(inputs, 1)?;
    let distance = get(inputs, 2)?;
    let source = required(original.as_ref())?;
    if source.is_empty()?
        || required(distance.as_ref())?.is_empty()?
        || (f.parameter(0)?.value_at(0, time)? >= 10000.
            && f.parameter(0)?.value_at(1, time)? >= 10000.)
    {
        return Ok(original);
    }
    let output = SharedImageMap::new(geometry);
    let direction = required(direction.as_ref())?.channel(0, false)?;
    let distance = required(distance.as_ref())?.channel(0, false)?;
    let mut sx = f.parameter(0)?.value_at(0, time)?;
    let mut sy = f.parameter(0)?.value_at(1, time)?;
    if sx == 0. {
        sx = 1E-6;
    }
    if sy == 0. {
        sy = 1E-6;
    }
    let dx = f64::from(geometry.width()) / sx / f.parameter(1)?.effective_value_at(time)?;
    let dy = f64::from(geometry.height()) / sy / f.parameter(1)?.effective_value_at(time)?;
    let steps = cast(f.parameter(1)?.effective_value_at(time)?);
    let _ = direction.read(0)?;
    let _ = distance.read(0)?;
    let _ = distance.read(0)?;
    let aa = super::adaptive::text(f, 2, time, "Yes")?;
    for ch in 0..count(source, options.is_channel_rendering(), false)? {
        let values = source.channel_with_effects(ch, false)?;
        if values.len() == 1 {
            output.copy_indexed_constant(ch, values.read(0)?, source)?;
            continue;
        }
        let dest = output.allocate_indexed(ch, source)?;
        worker(
            &values, &dest, &direction, &distance, geometry, steps, dx, dy, aa, false, execution,
        )?;
        output.seal_channel(ch)?;
    }
    if let Some(region) = source.region_store() {
        let values = region.data(true)?;
        let dest = super::adaptive::floats(
            i32::try_from(values.len()).map_err(|_| crate::ImageError::Overflow)?,
        )?;
        worker(
            &values, &dest, &direction, &distance, geometry, steps, dx, dy, false, true, execution,
        )?;
        output.set_region_store(Some(SharedFloatStore::new(dest)?));
    }
    Ok(Some(output))
}
