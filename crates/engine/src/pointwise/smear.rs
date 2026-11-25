//! ana inherited Size, repeated retained getters and actual two-map advection.
use super::adaptive::{read, text, write};
use super::numeric::{add, div, mul};
use super::{PointwiseError, PointwiseFilter, PointwiseRenderOptions, SharedImageMap};
use crate::{FloatBuffer, ImageGeometry, ProceduralExecution};
fn multiply64(a: f64, b: f64) -> f64 {
    match crate::source_float::first_nan(a, b) {
        Some(value) => value,
        None => a * b,
    }
}
fn subtract64(a: f64, b: f64) -> f64 {
    match crate::source_float::first_nan(a, b) {
        Some(value) => value,
        None => a - b,
    }
}
struct Worker<'a> {
    source: &'a FloatBuffer,
    dest: &'a FloatBuffer,
    direction: &'a FloatBuffer,
    distance: &'a FloatBuffer,
    geometry: ImageGeometry,
    steps: [f64; 2],
    samples: i32,
    aa: bool,
    seamless: bool,
}
impl Worker<'_> {
    fn execute<E>(&self, execution: ProceduralExecution) -> Result<(), PointwiseError<E>> {
        let width = self.geometry.width();
        let height = self.geometry.height();
        crate::procedural_schedule::for_each_chunk(
            width.wrapping_mul(height),
            execution,
            |start, count| {
                if width == 0 {
                    return Err(PointwiseError::DivideByZero);
                }
                let mut row = start / width;
                let mut col = start % width;
                let inverse = div(1., self.samples as f32);
                for counter in start..start.wrapping_add(count) {
                    if col >= width {
                        col = 0;
                        row = row.wrapping_add(1);
                    }
                    let mut x = f64::from(col);
                    let mut y = f64::from(row);
                    let mut sum = 0.;
                    let mut accepted = 1.;
                    for sample in 0..self.samples {
                        let value = if self.aa {
                            super::motion_sampling::sample::<E>(
                                x as f32,
                                y as f32,
                                self.source,
                                width,
                                height,
                                self.seamless,
                            )?
                        } else {
                            read(
                                self.source,
                                crate::framework_cast::integer(x).wrapping_add(
                                    crate::framework_cast::integer(y).wrapping_mul(width),
                                ),
                            )?
                        };
                        sum = add(sum, value);
                        if sample == self.samples.wrapping_sub(1) {
                            break;
                        }
                        let at = crate::framework_cast::integer(x)
                            .wrapping_add(crate::framework_cast::integer(y).wrapping_mul(width));
                        let direction = if self.direction.len() <= 1 {
                            read(self.direction, 0)?
                        } else if sample <= 0 || !self.aa {
                            read(self.direction, at)?
                        } else {
                            super::motion_sampling::sample::<E>(
                                x as f32,
                                y as f32,
                                self.direction,
                                width,
                                height,
                                true,
                            )?
                        };
                        let angle = multiply64(f64::from(direction), std::f64::consts::PI * 2.);
                        let distance = if self.distance.len() <= 1 {
                            read(self.distance, 0)?
                        } else if sample > 0 && self.aa {
                            super::motion_sampling::sample::<E>(
                                x as f32,
                                y as f32,
                                self.distance,
                                width,
                                height,
                                true,
                            )?
                        } else {
                            read(self.distance, at)?
                        };
                        let dx = multiply64(f64::from(distance), self.steps[0]);
                        let dy = multiply64(f64::from(distance), self.steps[1]);
                        x = subtract64(x, multiply64(angle.cos(), dx));
                        y = subtract64(y, multiply64(angle.sin(), dy));
                        if self.seamless {
                            if x < 0. || x >= f64::from(width) {
                                x %= f64::from(width);
                                if x < 0. {
                                    x += f64::from(width);
                                }
                                if x >= f64::from(width) {
                                    x = 0.;
                                }
                            }
                            if y < 0. || y >= f64::from(height) {
                                y %= f64::from(height);
                                if y < 0. {
                                    y += f64::from(height);
                                }
                                if y >= f64::from(height) {
                                    y = 0.;
                                }
                            }
                        } else {
                            if x < 0. || x >= f64::from(width) || y < 0. || y >= f64::from(height) {
                                break;
                            }
                            accepted = add(accepted, 1.);
                        }
                    }
                    write(
                        self.dest,
                        counter,
                        if self.seamless {
                            mul(sum, inverse)
                        } else {
                            div(sum, accepted)
                        },
                    )?;
                    col = col.wrapping_add(1);
                }
                Ok(())
            },
        )
    }
}
pub(super) fn retained<E: 'static>(
    filter: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    geometry: ImageGeometry,
    time: f64,
    execution: ProceduralExecution,
    options: PointwiseRenderOptions,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    let original = inputs.first().cloned().ok_or(PointwiseError::Index)?;
    let direction = inputs.get(1).cloned().ok_or(PointwiseError::Index)?;
    let distance = inputs.get(2).cloned().ok_or(PointwiseError::Index)?;
    let source = original.as_ref().ok_or(PointwiseError::NullReference)?;
    if source.is_empty()? {
        return Ok(original);
    }
    let distance = distance.as_ref().ok_or(PointwiseError::NullReference)?;
    if distance.is_empty()? {
        return Ok(original);
    }
    if filter.parameter(0)?.value_at(0, time)? >= 10000.
        && filter.parameter(0)?.value_at(1, time)? >= 10000.
    {
        return Ok(original);
    }
    let output = SharedImageMap::new(geometry);
    let direction = direction.as_ref().ok_or(PointwiseError::NullReference)?;
    let directions = direction.channel(0, false)?;
    let distances = distance.channel(0, false)?;
    let mut x = filter.parameter(0)?.value_at(0, time)?;
    let mut y = filter.parameter(0)?.value_at(1, time)?;
    if x == 0. {
        x = 1e-6;
    }
    if y == 0. {
        y = 1e-6;
    }
    let dx = f64::from(geometry.width()) / x / filter.parameter(1)?.effective_value_at(time)?;
    let dy = f64::from(geometry.height()) / y / filter.parameter(1)?.effective_value_at(time)?;
    let samples = crate::framework_cast::integer(filter.parameter(1)?.effective_value_at(time)?);
    directions.read(0)?;
    distances.read(0)?;
    distances.read(0)?;
    let aa = text(filter, 2, time, "Yes")?;
    let seamless = text(filter, 3, time, "Yes")?;
    let channels = if options.is_channel_rendering() {
        source.all_channel_count()
    } else {
        i32::try_from(source.channel_count()?).map_err(|_| crate::ImageError::Overflow)?
    };
    for channel in 0..channels {
        let values = source.channel_with_effects(channel, false)?;
        if values.len() == 1 {
            output.copy_indexed_constant(channel, values.read(0)?, source)?;
        } else {
            let dest = output.allocate_indexed(channel, source)?;
            Worker {
                source: &values,
                dest: &dest,
                direction: &directions,
                distance: &distances,
                geometry,
                steps: [dx, dy],
                samples,
                aa,
                seamless,
            }
            .execute::<E>(execution)?;
            output.seal_channel(channel)?;
        }
        source.seal_channel(channel)?;
    }
    direction.seal_channel(0)?;
    distance.seal_channel(0)?;
    output.set_region_store(source.region_store());
    Ok(Some(output))
}
