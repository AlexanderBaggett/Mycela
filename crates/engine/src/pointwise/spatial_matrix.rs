//! Evaluated internal aip.g06002e00 and worker2e02. This is convolution with
//! actual wu coefficients, not the six-double affine transform matrix.
use super::super::numeric::{add, mul};
use super::{PointwiseError, SharedImageMap, named_tail, payload, read, write};
use crate::procedural_schedule::for_each_row_chunk;
use crate::{
    FloatBuffer, ImageError, ImageGeometry, MatrixKernel, ProceduralExecution,
    framework_cast::integer,
};

#[derive(Clone, Copy, Debug)]
pub struct MatrixRenderOptions {
    iterations: i32,
    angle_radians: f64,
    radius: f64,
    execution: ProceduralExecution,
}
impl MatrixRenderOptions {
    pub const fn new(
        iterations: i32,
        angle_radians: f64,
        radius: f64,
        execution: ProceduralExecution,
    ) -> Self {
        Self {
            iterations,
            angle_radians,
            radius,
            execution,
        }
    }
    pub const fn iterations(&self) -> i32 {
        self.iterations
    }
    pub const fn angle_radians(&self) -> f64 {
        self.angle_radians
    }
    pub const fn radius(&self) -> f64 {
        self.radius
    }
    pub const fn execution(&self) -> ProceduralExecution {
        self.execution
    }
}
fn doubles(count: usize) -> Result<Vec<f64>, ImageError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| ImageError::Allocation)?;
    values.resize(count, 0.0);
    Ok(values)
}
fn singles(count: usize) -> Result<Vec<f32>, ImageError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| ImageError::Allocation)?;
    values.resize(count, 0.0);
    Ok(values)
}
fn clone_buffer(source: &FloatBuffer) -> Result<FloatBuffer, ImageError> {
    // Source SafelyCloneArray: fresh allocation then copy BEFORE next worker.
    // Allocator emergency/retry/spill remains an explicit unported resource
    // capability; this never fakes a successful GC/provider callback.
    let values = source.shared_values();
    let mut copy = singles(values.len())?;
    copy.copy_from_slice(&values);
    Ok(FloatBuffer::new(copy))
}
/// Exact evaluated internal helper, including direct nonpositive iterations:
/// dense destinations still allocate/seal with no worker run; scalar copies.
/// Optional input/kernel fail only at their actual reached dereference sites.
pub fn render_matrix<E>(
    input: Option<&SharedImageMap>,
    kernel: Option<&MatrixKernel>,
    options: &MatrixRenderOptions,
    geometry: ImageGeometry,
) -> Result<SharedImageMap, PointwiseError<E>> {
    let out = SharedImageMap::new(geometry);
    let mut xs = doubles(9)?;
    let mut ys = doubles(9)?;
    xs[0] = 0.0;
    ys[0] = 0.0;
    for index in 0..8 {
        // Original IL71/130 NEG, not 0.0-angle. Do not factor the multiplies
        // or reuse a reordered shared trigonometric angle expression.
        xs[index + 1] = libm::cos(
            (-options.angle_radians) - (index as f64) * std::f64::consts::PI / 4.0
                + std::f64::consts::PI,
        ) * f64::from(geometry.width())
            * options.radius;
        ys[index + 1] = libm::sin(
            (-options.angle_radians) - (index as f64) * std::f64::consts::PI / 4.0
                + std::f64::consts::PI,
        ) * f64::from(geometry.height())
            * options.radius;
    }
    let mut weights = singles(9)?;
    for (index, weight) in weights.iter_mut().enumerate() {
        // Actual public coefficient root is read anew per source ldfld, not
        // eagerly snapshotted or padded if a caller replaced it with null/short.
        *weight = kernel
            .ok_or(PointwiseError::NullReference)?
            .coefficients()
            .ok_or(PointwiseError::NullReference)?
            .read(index)
            .map_err(payload)? as f32;
    }
    let mut taps = 0usize;
    for index in 0..9 {
        // Coefficients narrow FIRST: both zeros (and f64 underflow) drop;
        // NaN remains. No sum normalization or original-f64 compaction.
        if f64::from(weights[index]) != 0.0 {
            weights[taps] = weights[index];
            xs[taps] = xs[index];
            ys[taps] = ys[index];
            taps += 1;
        }
    }
    let mut dense_plan = None;
    let mut channel = 0i32;
    loop {
        // Dynamic input.ng is re-read after each processed/sealed channel.
        // The real channel() rejects >=4 even when public state grew ng>4.
        let input = input.ok_or(PointwiseError::NullReference)?;
        if i64::from(channel)
            >= i64::try_from(input.channel_count()?).map_err(|_| ImageError::Overflow)?
        {
            break;
        }
        let mut source = input.channel(channel, false)?;
        if source.len() == 1 {
            out.set_constant(channel, source.read(0)?)?;
        } else {
            let destination = out.materialize(channel)?;
            // Source constructs the worker even when iterations<=0, before
            // these field assignments. This pure leaf does not emulate its nz
            // native bootstrap/resources or fake their success with a provider.
            let area = geometry
                .width()
                .checked_mul(geometry.height())
                .filter(|area| *area > 0)
                .map(|area| area as usize);
            let dense = options.iterations > 0
                && area == Some(source.len())
                && area == Some(destination.len())
                && !matches!(
                    options.execution,
                    ProceduralExecution::WorkerPool {
                        processor_count: ..=0
                    }
                );
            if dense && dense_plan.is_none() {
                dense_plan =
                    DenseMatrix::new(geometry, &weights[..taps], &xs[..taps], &ys[..taps])?;
            }
            for iteration in 0..options.iterations {
                if iteration > 0 {
                    source = clone_buffer(&destination)?;
                }
                if dense && let Some(plan) = &dense_plan {
                    let source = source.shared_values();
                    destination
                        .with_mut_slice(|pixels| plan.run(&source, pixels, options.execution));
                    continue;
                }
                for_each_row_chunk(geometry.height(), options.execution, |start, count| {
                    convolve(
                        Some(&source),
                        Some(&destination),
                        &weights[..taps],
                        &xs[..taps],
                        &ys[..taps],
                        geometry,
                        start,
                        count,
                    )
                })?;
            }
        }
        out.seal_channel(channel)?;
        channel = channel.wrapping_add(1);
    }
    named_tail(&out, input.ok_or(PointwiseError::NullReference)?)?;
    Ok(out)
}
/// Evaluated k8 branch wrapper. Iterations is the already resolved scalar,
/// truncated before the zero shortcut. The original input getter itself and
/// actual os object-property/Factor ownership are outside this leaf API.
/// The internal helper above remains separately callable and does NOT bypass0.
pub fn render_matrix_filter<E>(
    input: Option<&SharedImageMap>,
    kernel: Option<&MatrixKernel>,
    iterations: f64,
    factor: f64,
    execution: ProceduralExecution,
    geometry: ImageGeometry,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    let iterations = integer(iterations);
    if iterations == 0 {
        return Ok(input.cloned());
    }
    let kernel = kernel.ok_or(PointwiseError::NullReference)?;
    let angle = kernel.angle() / 180.0 * std::f64::consts::PI;
    let radius = kernel.radius() / factor;
    render_matrix(
        input,
        Some(kernel),
        &MatrixRenderOptions::new(iterations, angle, radius, execution),
        geometry,
    )
    .map(Some)
}
fn coordinate(position: i32, offset: f64, extent: i32) -> i32 {
    let mut value = (f64::from(position) + offset) % f64::from(extent);
    if value < 0.0 {
        value -= 1.0;
    }
    let mut index = integer(value);
    if index < 0 {
        index = index.wrapping_add(extent);
    }
    index
}
/// Cache wrapping coordinates for dense, ordinary rasters. A valid plane can
/// be divided into disjoint row bands while retaining every tap's arithmetic.
struct DenseMatrix<'a> {
    width: usize,
    weights: &'a [f32],
    columns: Vec<[usize; 9]>,
    rows: Vec<[usize; 9]>,
}

impl<'a> DenseMatrix<'a> {
    fn new(
        geometry: ImageGeometry,
        weights: &'a [f32],
        xs: &[f64],
        ys: &[f64],
    ) -> Result<Option<Self>, ImageError> {
        let width = geometry.width();
        let height = geometry.height();
        if width <= 0 || height <= 0 {
            return Ok(None);
        }
        let build = |extent: i32, stride: usize, offsets: &[f64]| {
            let mut result = Vec::new();
            result
                .try_reserve_exact(extent as usize)
                .map_err(|_| ImageError::Allocation)?;
            for i in 0..extent {
                let mut band = [0; 9];
                for (entry, &offset) in band.iter_mut().zip(offsets) {
                    let sample = coordinate(i, offset, extent);
                    if !(0..extent).contains(&sample) {
                        return Ok(None);
                    }
                    *entry = sample as usize * stride;
                }
                result.push(band);
            }
            Ok(Some(result))
        };
        let Some(columns) = build(width, 1, xs)? else {
            return Ok(None);
        };
        let Some(rows) = build(height, width as usize, ys)? else {
            return Ok(None);
        };
        Ok(Some(Self {
            width: width as usize,
            weights,
            columns,
            rows,
        }))
    }

    fn run(&self, source: &[f32], output: &mut [f32], execution: ProceduralExecution) {
        let weights = self.weights;
        let render = |first_row, output: &mut [f32]| {
            for (row, offsets) in output
                .chunks_exact_mut(self.width)
                .zip(&self.rows[first_row..])
            {
                for (pixel, columns) in row.iter_mut().zip(&self.columns) {
                    let mut value = 0.;
                    for (index, &weight) in weights.iter().enumerate() {
                        value = add(value, mul(source[offsets[index] + columns[index]], weight));
                    }
                    *pixel = value;
                }
            }
        };
        let workers = match execution {
            ProceduralExecution::WorkerPool { processor_count } if output.len() >= 512 * 512 => {
                (processor_count as usize).min(8).min(self.rows.len())
            }
            _ => 1,
        };
        if workers <= 1 {
            render(0, output);
            return;
        }
        let rows_per_worker = self.rows.len().div_ceil(workers);
        std::thread::scope(|scope| {
            for (band, output) in output.chunks_mut(rows_per_worker * self.width).enumerate() {
                scope.spawn(move || render(band * rows_per_worker, output));
            }
        });
    }
}

#[allow(clippy::too_many_arguments)] // Actual worker roots/count/geometry and bx interval.
fn convolve<E>(
    source: Option<&FloatBuffer>,
    destination: Option<&FloatBuffer>,
    weights: &[f32],
    xs: &[f64],
    ys: &[f64],
    geometry: ImageGeometry,
    start: i32,
    count: i32,
) -> Result<(), PointwiseError<E>> {
    // One scratch per ACTUAL bx chunk, allocated before even an empty interval.
    let mut rows = Vec::new();
    rows.try_reserve_exact(weights.len())
        .map_err(|_| ImageError::Allocation)?;
    rows.resize(weights.len(), 0i32);
    let mut y = start;
    let end = start.wrapping_add(count);
    while y < end {
        let row = y.wrapping_mul(geometry.width());
        for (index, value) in rows.iter_mut().enumerate() {
            *value = coordinate(y, ys[index], geometry.height()).wrapping_mul(geometry.width());
        }
        let mut x = 0i32;
        while x < geometry.width() {
            let mut value = 0.0f32;
            for index in 0..weights.len() {
                let column = coordinate(x, xs[index], geometry.width());
                let sample = read(source, column.wrapping_add(rows[index]))?;
                value = add(value, mul(sample, weights[index]));
            }
            write(destination, x.wrapping_add(row), value)?;
            x = x.wrapping_add(1);
        }
        y = y.wrapping_add(1);
    }
    Ok(())
}

#[cfg(test)]
#[path = "spatial_matrix_test.rs"]
mod tests;
