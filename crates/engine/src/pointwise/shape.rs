//! si.k8: real Shape map reversal/CDF and adaptive weighted nearest sampling.
//! No shape-choice facade, private expected table, copied input map or RNG host.
use super::numeric::{add, div, mul, sub};
use super::{PointwiseError, PointwiseFilter, PointwiseRenderEffects, SharedImageMap};
use crate::{
    FloatBuffer, ImageError, ImageGeometry, ProceduralExecution, SeededLattice,
    seeded_random::SeededRandom,
};
fn ix(i: i32) -> Result<usize, ImageError> {
    usize::try_from(i).map_err(|_| ImageError::Index)
}
fn read(a: &FloatBuffer, i: i32) -> Result<f32, ImageError> {
    a.read(ix(i)?)
}
fn cast(x: f32) -> i32 {
    crate::framework_cast::integer(f64::from(x))
}
#[allow(clippy::manual_clamp)] // Literal zl.g comparisons, unordered retained.
fn clamp(x: f32) -> f32 {
    if x < 0. {
        0.
    } else if x > 1. {
        1.
    } else {
        x
    }
}
fn reverse<E>(
    source: &FloatBuffer,
    destination: &FloatBuffer,
    geometry: ImageGeometry,
    execution: ProceduralExecution,
) -> Result<(), PointwiseError<E>> {
    let width = geometry.width();
    let height = geometry.height();
    crate::procedural_schedule::for_each_chunk(
        width.wrapping_mul(height),
        execution,
        |start, count| {
            if width == 0 {
                return Err(PointwiseError::DivideByZero);
            }
            if start == i32::MIN && width == -1 {
                return Err(PointwiseError::Arithmetic);
            }
            let mut row = start / width;
            let mut column = start % width;
            let end = start.wrapping_add(count);
            let mut output = width
                .wrapping_sub(1)
                .wrapping_add(height.wrapping_sub(1).wrapping_sub(row).wrapping_mul(width));
            let mut at = start;
            while at < end {
                if column >= width {
                    column = 0;
                    row = row.wrapping_add(1);
                    output = width
                        .wrapping_sub(1)
                        .wrapping_add(height.wrapping_sub(1).wrapping_sub(row).wrapping_mul(width));
                }
                let value = read(source, at)?;
                destination.write(
                    ix(output.wrapping_sub(column))?,
                    if value < 0. { 0. } else { value },
                )?;
                at = at.wrapping_add(1);
                column = column.wrapping_add(1);
            }
            Ok(())
        },
    )
}
struct Cdf {
    xs: Vec<i32>,
    ys: Vec<i32>,
}
fn integers(count: i32) -> Result<Vec<i32>, ImageError> {
    let count = usize::try_from(count).map_err(|_| ImageError::Overflow)?;
    let mut result = Vec::new();
    result
        .try_reserve_exact(count)
        .map_err(|_| ImageError::Allocation)?;
    result.resize(count, 0);
    Ok(result)
}
fn cdf<E>(
    weights: &FloatBuffer,
    geometry: ImageGeometry,
    grid: i32,
    count: i32,
    execution: ProceduralExecution,
) -> Result<Cdf, PointwiseError<E>> {
    let temporary = SharedImageMap::new(geometry);
    temporary.set_buffer(0, Some(weights))?;
    let resized = super::image_resize::resize::<E>(&temporary, grid, grid, execution)?;
    let mass = resized.materialize(0)?;
    let mut total = 0.;
    for i in 0..mass.len() {
        total = add(total, mass.read(i)?);
    }
    // Source publishes the first out allocation before attempting the second.
    // Internal k8 supplies distinct locals; neither array is a mutable facade.
    let mut xs = integers(count)?;
    let mut ys = integers(count)?;
    let reciprocal = div(1., xs.len() as f32);
    let mut cumulative = mass.read(0)?;
    let mut at = 0i32;
    let mut x = 0i32;
    let mut y = 0i32;
    for (i, (out_x, out_y)) in xs.iter_mut().zip(&mut ys).enumerate() {
        let threshold = mul(mul(i as f32, reciprocal), total);
        while threshold >= cumulative {
            if at >= (mass.len() as i32).wrapping_sub(1) {
                break;
            }
            at = at.wrapping_add(1);
            x = x.wrapping_add(1);
            if x >= resized.geometry().width() {
                x = 0;
                y = y.wrapping_add(1);
            }
            cumulative = add(cumulative, read(&mass, at)?);
        }
        *out_x = x;
        *out_y = y;
    }
    Ok(Cdf { xs, ys })
}
struct Worker<'a> {
    destination: &'a FloatBuffer,
    source: &'a FloatBuffer,
    shape: &'a FloatBuffer,
    radius_map: &'a FloatBuffer,
    geometry: ImageGeometry,
    distribution: &'a Cdf,
    grid: i32,
    radius: f32,
    radius_present: bool,
    seamless: bool,
    lattice: SeededLattice,
    dense_shape: bool,
    dense_radius: bool,
    jitter: Vec<f32>,
}
impl<'a> Worker<'a> {
    #[allow(clippy::too_many_arguments)] // Exact retained worker roles; no parameter DTO authority.
    fn new(
        destination: &'a FloatBuffer,
        source: &'a FloatBuffer,
        shape: &'a FloatBuffer,
        radius_map: &'a FloatBuffer,
        channel: i32,
        geometry: ImageGeometry,
        distribution: &'a Cdf,
        grid: i32,
        radius: f32,
        radius_present: bool,
        seamless: bool,
    ) -> Result<Self, ImageError> {
        let lattice = SeededLattice::new(channel)?;
        let dense_shape = shape.len() > 1;
        let dense_radius = radius_present && radius_map.len() > 1;
        let mut random = SeededRandom::from_seed(channel).map_err(|_| ImageError::Overflow)?;
        let mut jitter = Vec::new();
        jitter
            .try_reserve_exact(500)
            .map_err(|_| ImageError::Allocation)?;
        jitter.resize(500, 0.);
        for value in &mut jitter {
            *value = random.sample() as f32;
        }
        Ok(Self {
            destination,
            source,
            shape,
            radius_map,
            geometry,
            distribution,
            grid,
            radius,
            radius_present,
            seamless,
            lattice,
            dense_shape,
            dense_radius,
            jitter,
        })
    }
    fn execute<E>(&self, execution: ProceduralExecution) -> Result<(), PointwiseError<E>> {
        let width = self.geometry.width();
        let height = self.geometry.height();
        let wf = width as f32;
        let hf = height as f32;
        crate::procedural_schedule::for_each_chunk(
            width.wrapping_mul(height),
            execution,
            |start, count| {
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
                let unit = div(1., self.grid as f32);
                let mut weight = read(self.shape, 0)?;
                if weight < 0. {
                    weight = 0.;
                }
                let mut radius = if self.radius_present {
                    mul(clamp(read(self.radius_map, 0)?), self.radius)
                } else {
                    self.radius
                };
                let mut at = start;
                while at < end {
                    if column >= width {
                        column = 0;
                        row = row.wrapping_add(1);
                        row_base = row.wrapping_mul(width);
                    }
                    let cell = column.wrapping_add(row_base);
                    if self.dense_radius {
                        radius = mul(clamp(read(self.radius_map, cell)?), self.radius);
                    }
                    let half_radius = mul(radius, 0.5);
                    let mut cursor = cast(mul(
                        self.lattice.sample2(column, row),
                        self.jitter.len() as f32,
                    ));
                    let mut previous = 0.;
                    let mut estimate = 0.;
                    let mut previous_count = 0i32;
                    let mut stable = 0i32;
                    let mut first = true;
                    let mut batch = 2i32;
                    while batch <= 200 {
                        let reciprocal = div(1., batch as f32);
                        let mut sum = 0.;
                        let mut total_weight = 0.;
                        let mut sample = 0i32;
                        while sample < batch {
                            let random = *self.jitter.get(ix(cursor)?).ok_or(ImageError::Index)?;
                            cursor = cursor.wrapping_add(1);
                            if cursor >= self.jitter.len() as i32 {
                                cursor = 0;
                            }
                            let mut position = mul(add(sample as f32, random), reciprocal);
                            if position >= 1. {
                                position = 0.;
                            }
                            let selected =
                                ix(cast(mul(position, self.distribution.xs.len() as f32)))?;
                            let x = *self
                                .distribution
                                .xs
                                .get(selected)
                                .ok_or(ImageError::Index)?;
                            let y = *self
                                .distribution
                                .ys
                                .get(selected)
                                .ok_or(ImageError::Index)?;
                            let nx = mul(add(x as f32, random), unit);
                            let ny = mul(add(y as f32, random), unit);
                            if self.dense_shape {
                                let x = cast(mul(nx, wf));
                                let y = cast(mul(ny, hf));
                                weight = read(self.shape, x.wrapping_add(y.wrapping_mul(width)))?;
                                if weight < 0. {
                                    weight = 0.;
                                }
                            }
                            if weight != 0. {
                                let mut x = add(
                                    add(column as f32, mul(sub(mul(nx, radius), half_radius), wf)),
                                    0.5,
                                );
                                let mut y = add(
                                    add(row as f32, mul(sub(mul(ny, radius), half_radius), hf)),
                                    0.5,
                                );
                                let mut accepted = true;
                                if x < 0. || x >= wf {
                                    if !self.seamless {
                                        accepted = false;
                                    } else {
                                        x %= wf;
                                        if x < 0. {
                                            x = add(x, wf);
                                            if x >= wf {
                                                x = sub(x, wf);
                                            }
                                        }
                                    }
                                }
                                if accepted && (y < 0. || y >= hf) {
                                    if !self.seamless {
                                        accepted = false;
                                    } else {
                                        y %= hf;
                                        if y < 0. {
                                            y = add(y, hf);
                                            if y >= hf {
                                                y = sub(y, hf);
                                            }
                                        }
                                    }
                                }
                                if accepted {
                                    sum = add(
                                        sum,
                                        mul(
                                            weight,
                                            read(
                                                self.source,
                                                cast(x).wrapping_add(cast(y).wrapping_mul(width)),
                                            )?,
                                        ),
                                    );
                                    total_weight = add(total_weight, weight);
                                }
                            }
                            sample = sample.wrapping_add(1);
                        }
                        estimate = if total_weight == 0. {
                            0.
                        } else {
                            div(sum, total_weight)
                        };
                        if first {
                            first = false;
                        } else {
                            estimate = div(
                                add(
                                    mul(estimate, batch as f32),
                                    mul(previous, previous_count as f32),
                                ),
                                batch.wrapping_add(previous_count) as f32,
                            );
                            if sub(previous, estimate) < 0.005 && sub(estimate, previous) < 0.005 {
                                let mut required = 12i32.wrapping_sub(batch);
                                if required < 2 {
                                    required = 2;
                                }
                                if stable >= required {
                                    break;
                                }
                                stable = stable.wrapping_add(1);
                            } else {
                                stable = 0;
                            }
                        }
                        previous = estimate;
                        previous_count = previous_count.wrapping_add(batch);
                        batch = batch.wrapping_add(1);
                    }
                    self.destination.write(ix(cell)?, estimate)?;
                    at = at.wrapping_add(1);
                    column = column.wrapping_add(1);
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
    effects: &mut PointwiseRenderEffects<'_, E>,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    let original = inputs.first().cloned().ok_or(PointwiseError::Index)?;
    let radius_map = inputs.get(1).cloned().ok_or(PointwiseError::Index)?;
    let shape_map = inputs.get(2).cloned().ok_or(PointwiseError::Index)?;
    let output = SharedImageMap::new(geometry);
    let radius_map = radius_map.as_ref().ok_or(PointwiseError::NullReference)?;
    let radius_values = radius_map.channel(0, false)?;
    let ratio = filter.parameter(0)?.effective_value_at(time)? / filter.factor();
    let radius = (if ratio > 1. { 1. } else { ratio }) as f32;
    let radius_present = filter.node().slot(1)?.node().is_some();
    let choice = filter.parameter(1)?;
    let selected = ix(choice.effective_index_at(time)?)?;
    let entries = choice.options()?.entries();
    let seamless = entries
        .get(selected)
        .ok_or(PointwiseError::Index)?
        .as_ref()
        .is_some_and(|s| s.iter().copied().eq("Yes".encode_utf16()));
    for channel in 0..4 {
        let original = original.as_ref().ok_or(PointwiseError::NullReference)?;
        let source = original.channel(channel, false)?;
        if source.len() == 1 {
            output.set_constant(channel, source.read(0)?)?;
        } else {
            let shape = shape_map.as_ref().ok_or(PointwiseError::NullReference)?;
            let values = shape.channel(channel, false)?;
            let mut weights = crate::procedural_sampling::allocate(
                i32::try_from(values.len()).map_err(|_| ImageError::Overflow)?,
            )?;
            if values.len() == 1 {
                weights.write(0, values.read(0)?)?;
            } else {
                reverse::<E>(&values, &weights, geometry, execution)?;
            }
            shape.seal_channel(channel)?;
            let destination = output.materialize(channel)?;
            let mut positive = false;
            for i in 0..weights.len() {
                if weights.read(i)? > 0. {
                    positive = true;
                    break;
                }
            }
            if !positive {
                weights = FloatBuffer::new(vec![1.]);
            }
            let distribution = cdf::<E>(&weights, geometry, 32, 5000, execution)?;
            Worker::new(
                &destination,
                &source,
                &weights,
                &radius_values,
                channel,
                geometry,
                &distribution,
                32,
                radius,
                radius_present,
                seamless,
            )?
            .execute::<E>(execution)?;
        }
        original.seal_channel(channel)?;
        output.seal_channel(channel)?;
        effects.preflight(geometry.width(), 2)?;
    }
    radius_map.seal_channel(0)?;
    output.copy_auxiliary(original.as_ref().ok_or(PointwiseError::NullReference)?)?;
    Ok(Some(output))
}
#[cfg(test)]
#[path = "shape_test.rs"]
mod tests;
