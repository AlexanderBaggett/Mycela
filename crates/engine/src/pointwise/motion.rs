//! Actual av.k8/av+a supplied-map kernel, with real late retained slot presence.
use super::motion_patterns::{self, Point};
use super::numeric::{add, div, mul, sub};
use super::{PointwiseError, PointwiseFilter, PointwiseRenderEffects, SharedImageMap};
use crate::{FloatBuffer, ImageError, ImageGeometry, ProceduralExecution};
use std::f64::consts::PI;
fn index(value: i32) -> Result<usize, ImageError> {
    usize::try_from(value).map_err(|_| ImageError::Index)
}
fn read(source: &FloatBuffer, i: i32) -> Result<f32, ImageError> {
    source.read(index(i)?)
}
#[allow(clippy::manual_clamp)] // Exact ordered source branches, including unordered values.
fn clamp(value: f32) -> f32 {
    if value < 0. {
        0.
    } else if value > 1. {
        1.
    } else {
        value
    }
}
fn text<E: 'static>(
    filter: &PointwiseFilter<E>,
    parameter: i32,
    time: f64,
    wanted: &str,
) -> Result<bool, PointwiseError<E>> {
    let p = filter.parameter(parameter)?;
    let i = index(p.effective_index_at(time)?)?;
    let entries = p.options()?.entries();
    Ok(entries
        .get(i)
        .ok_or(PointwiseError::Index)?
        .as_ref()
        .is_some_and(|v| v.iter().copied().eq(wanted.encode_utf16())))
}
struct Settings {
    radius: f32,
    angle: f32,
    shape: i32,
    samples: i32,
    face_center: bool,
    radius_present: bool,
    seamless: bool,
}
struct Worker<'a> {
    source: &'a FloatBuffer,
    destination: &'a FloatBuffer,
    radius_map: &'a FloatBuffer,
    angle_map: &'a FloatBuffer,
    geometry: ImageGeometry,
    settings: Settings,
    dense_radius: bool,
    pattern: Vec<Point>,
}
impl<'a> Worker<'a> {
    fn new(
        source: &'a FloatBuffer,
        destination: &'a FloatBuffer,
        radius_map: &'a FloatBuffer,
        angle_map: &'a FloatBuffer,
        geometry: ImageGeometry,
        settings: Settings,
    ) -> Result<Self, ImageError> {
        let dense_radius = settings.radius_present && radius_map.len() > 1;
        // CIL NEG Single before conv.r8, not `0f-angle`.
        let pattern = motion_patterns::pattern(
            settings.samples,
            f64::from(-settings.angle),
            settings.shape,
            geometry.width() as f32,
        )?;
        Ok(Self {
            source,
            destination,
            radius_map,
            angle_map,
            geometry,
            settings,
            dense_radius,
            pattern,
        })
    }
    fn execute<E>(&self, execution: ProceduralExecution) -> Result<(), PointwiseError<E>> {
        let width = self.geometry.width();
        let height = self.geometry.height();
        let area = width.wrapping_mul(height);
        crate::procedural_schedule::for_each_chunk(area, execution, |start, count| {
            if width == 0 {
                return Err(PointwiseError::DivideByZero);
            }
            if start == i32::MIN && width == -1 {
                return Err(PointwiseError::Arithmetic);
            }
            let mut row = start / width;
            let mut column = start % width;
            let end = start.wrapping_add(count);
            let mut row_base = row.wrapping_mul(width);
            let mut at = start;
            let cx = div(width as f32, 2.);
            let cy = div(height as f32, 2.);
            while at < end {
                if column >= width {
                    column = 0;
                    row = row.wrapping_add(1);
                    row_base = row.wrapping_mul(width);
                }
                let cell = column.wrapping_add(row_base);
                let radius = if self.settings.radius_present {
                    mul(
                        clamp(read(
                            self.radius_map,
                            if self.dense_radius { cell } else { 0 },
                        )?),
                        self.settings.radius,
                    )
                } else {
                    self.settings.radius
                };
                let mut rotated = motion_patterns::blank(self.pattern.len())?; // fresh PointF[] per pixel BEFORE angle-map read
                if self.settings.face_center && (column as f32 != cx || row as f32 != cy) {
                    let mut phase =
                        f64::from(sub(column as f32, cx)).atan2(f64::from(sub(row as f32, cy)));
                    if phase < 0. {
                        phase += 2.0 * PI;
                    }
                    phase = 7.853981633974483 - phase;
                    let cosine = phase.cos() as f32;
                    let sine = phase.sin() as f32;
                    for (point, source) in rotated.iter_mut().zip(&self.pattern) {
                        *point = Point {
                            x: sub(mul(source.x, cosine), mul(source.y, sine)),
                            y: add(mul(source.x, sine), mul(source.y, cosine)),
                        };
                    }
                } else {
                    rotated.copy_from_slice(&self.pattern);
                }
                let angle = -PI
                    * f64::from(sub(
                        mul(
                            read(
                                self.angle_map,
                                if self.angle_map.len() > 1 { cell } else { 0 },
                            )?,
                            2.,
                        ),
                        1.,
                    ));
                let cosine = angle.cos() as f32;
                let sine = angle.sin() as f32;
                let mut sum = 0.;
                let mut accepted = 0i32;
                for i in 0..self.settings.samples {
                    let point = rotated.get(index(i)?).ok_or(ImageError::Index)?;
                    let x = sub(mul(point.x, radius), 0.001);
                    let y = sub(mul(point.y, radius), 0.001);
                    let dx = sub(mul(x, cosine), mul(y, sine));
                    let dy = add(mul(x, sine), mul(y, cosine));
                    let x = sub(column as f32, dx);
                    let y = sub(row as f32, dy);
                    if self.settings.seamless
                        || (x > -1. && x < width as f32 && y > -1. && y < height as f32)
                    {
                        sum = add(
                            sum,
                            super::motion_sampling::sample(
                                x,
                                y,
                                self.source,
                                width,
                                height,
                                self.settings.seamless,
                            )?,
                        );
                        accepted = accepted.wrapping_add(1);
                    }
                }
                self.destination.write(
                    index(cell)?,
                    if accepted > 0 {
                        div(sum, accepted as f32)
                    } else {
                        0.
                    },
                )?;
                at = at.wrapping_add(1);
                column = column.wrapping_add(1);
            }
            Ok(())
        })
    }
}
pub(super) fn retained<E: 'static>(
    filter: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    geometry: ImageGeometry,
    time: f64,
    execution: ProceduralExecution,
    effects: &mut PointwiseRenderEffects<'_, E>,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    // Actual sequential three-out resolution, independent of node topology.
    let original = inputs.first().cloned().ok_or(PointwiseError::Index)?;
    let radius_map = inputs.get(1).cloned().ok_or(PointwiseError::Index)?;
    let angle_map = inputs.get(2).cloned().ok_or(PointwiseError::Index)?;
    let output = SharedImageMap::new(geometry);
    let radius_map = radius_map.as_ref().ok_or(PointwiseError::NullReference)?;
    let radius_values = radius_map.channel(0, false)?;
    let angle_map = angle_map.as_ref().ok_or(PointwiseError::NullReference)?;
    let angle_values = angle_map.channel(0, false)?;
    let ratio = filter.parameter(1)?.effective_value_at(time)? / filter.factor();
    let radius = (if ratio > 1. { 1. } else { ratio }) as f32;
    let radius_present = filter.node().slot(1)?.node().is_some();
    let seamless = text(filter, 5, time, "Yes")?;
    let face_center = text(filter, 4, time, "Yes")?;
    let angle = crate::framework_cast::integer(filter.parameter(2)?.effective_value_at(time)?);
    let samples = crate::framework_cast::integer(filter.parameter(3)?.effective_value_at(time)?);
    let shape = filter.parameter(0)?.effective_index_at(time)?;
    let mut channel = 0;
    while channel < 4 {
        let original = original.as_ref().ok_or(PointwiseError::NullReference)?;
        let source = original.channel(channel, false)?;
        if source.len() == 1 {
            output.set_constant(channel, source.read(0)?)?;
        } else {
            let destination = output.materialize(channel)?;
            Worker::new(
                &source,
                &destination,
                &radius_values,
                &angle_values,
                geometry,
                Settings {
                    radius,
                    angle: angle as f32,
                    shape,
                    samples,
                    face_center,
                    radius_present,
                    seamless,
                },
            )?
            .execute::<E>(execution)?;
        }
        original.seal_channel(channel)?;
        output.seal_channel(channel)?;
        effects.preflight(geometry.width(), 2)?;
        channel += 1;
    }
    radius_map.seal_channel(0)?;
    angle_map.seal_channel(0)?;
    output.copy_auxiliary(original.as_ref().ok_or(PointwiseError::NullReference)?)?;
    Ok(Some(output))
}
#[cfg(test)]
#[path = "motion_test.rs"]
mod tests;
