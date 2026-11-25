//! al0's lattice, octave accumulation and range-remap kernel. Its caller supplies
//! evaluated retained properties and an explicit source scheduler profile.
//! Resource bootstrap/OOM retry/spill/finalizer and concurrent error races remain
//! separate work, not implied by sequential chunks and fallible allocations.
use crate::procedural_lattice::{SeededLattice, coordinate_hash};
use crate::procedural_sampling::{allocate, resample_noise};
use crate::procedural_schedule::{ProceduralExecution, for_each_chunk};
use crate::{
    FloatBuffer, ImageError, ImageGeometry, PointwiseError, SharedImageMap, framework_cast::integer,
};

#[derive(Clone, Copy, Debug)]
pub struct NoiseOptions {
    size: [f64; 2],
    location: [f64; 2],
    seed: f64,
    iterations: f64,
    smoothing: i32,
    execution: ProceduralExecution,
}
impl NoiseOptions {
    pub const fn new(
        size: [f64; 2],
        location: [f64; 2],
        seed: f64,
        iterations: f64,
        smoothing: i32,
        execution: ProceduralExecution,
    ) -> Self {
        Self {
            size,
            location,
            seed,
            iterations,
            smoothing,
            execution,
        }
    }
    pub const fn size(&self) -> [f64; 2] {
        self.size
    }
    pub const fn location(&self) -> [f64; 2] {
        self.location
    }
    pub const fn seed(&self) -> f64 {
        self.seed
    }
    pub const fn iterations(&self) -> f64 {
        self.iterations
    }
    pub const fn smoothing_index(&self) -> i32 {
        self.smoothing
    }
    pub const fn execution(&self) -> ProceduralExecution {
        self.execution
    }
}
fn pixel_index<E>(index: i32, width: i32) -> Result<(i32, i32), PointwiseError<E>> {
    let error = || {
        if width == 0 {
            PointwiseError::DivideByZero
        } else {
            PointwiseError::Image(ImageError::Overflow)
        }
    };
    let row = index.checked_div(width).ok_or_else(error)?;
    let column = index.checked_rem(width).ok_or_else(error)?;
    Ok((column, row))
}
#[allow(clippy::too_many_arguments)] // Exact retained worker fields plus required scheduler profile.
fn fill_lattice<E>(
    destination: &FloatBuffer,
    width: i32,
    height: i32,
    offset: [i32; 2],
    seed: f64,
    hash_offset: f32,
    negative_first: bool,
    execution: ProceduralExecution,
) -> Result<(), PointwiseError<E>> {
    let lattice = if negative_first {
        None
    } else {
        // The discarded ahr precedes the positive worker setter's own real ahr.
        // Both allocate a float[256] before constructing their seeded awm stream.
        let discarded = SeededLattice::new(integer(seed))?;
        drop(discarded);
        Some(SeededLattice::new(integer(seed))?)
    };
    for_each_chunk::<E>(width.wrapping_mul(height), execution, |start, count| {
        let (mut x, mut y) = pixel_index::<E>(start, width)?;
        for index in start..start.wrapping_add(count) {
            if x >= width {
                x = 0;
                y = y.wrapping_add(1);
            }
            let value = if negative_first {
                let px = (x.wrapping_add(offset[0]) as f32) / 100.0f32 + hash_offset;
                let py = (y.wrapping_sub(offset[1]) as f32) / 100.0f32;
                coordinate_hash(px, py)
            } else {
                lattice
                    .as_ref()
                    .ok_or(ImageError::NullReference)?
                    .sample2(x.wrapping_add(offset[0]), y.wrapping_sub(offset[1]))
            };
            destination.write(
                usize::try_from(index).map_err(|_| ImageError::Index)?,
                value,
            )?;
            x = x.wrapping_add(1);
        }
        Ok(())
    })
}
fn extrema(values: &FloatBuffer) -> Result<(f32, f32), ImageError> {
    let mut low = f32::MAX;
    let mut high = -f32::MAX;
    for i in 0..values.len() {
        let value = values.read(i)?;
        if value < low {
            low = value;
        }
        if value > high {
            high = value;
        }
    }
    Ok((low, high))
}
/// Render evaluated source values. This callback is REQUIRED: later non-scalar
/// octaves invoke za.b(width) after resampling and before parameter doubling,
/// seed increment, publication and accumulation, at quality ordinal2.
/// Production runs actual memory preflight and reports unsupported GC/finalizer
/// recovery when unavailable. There is no fixed-success production wrapper.
/// Explicit no-op callbacks in the static numerical laws are caller fixtures.
pub fn render_noise_with_preflight<E>(
    options: NoiseOptions,
    width: i32,
    height: i32,
    preflight: &mut impl FnMut(i32, i32) -> Result<(), PointwiseError<E>>,
) -> Result<SharedImageMap, PointwiseError<E>> {
    render_noise_with_inputs(
        width,
        height,
        || Ok(options),
        &mut || Ok(options.smoothing),
        preflight,
    )
}
/// Retained getter seam: image allocation occurs before source operands, and
/// Smoothing is read repeatedly only inside a nonscalar resampling branch.
pub(crate) fn render_noise_with_inputs<E>(
    width: i32,
    height: i32,
    inputs: impl FnOnce() -> Result<NoiseOptions, PointwiseError<E>>,
    smoothing: &mut impl FnMut() -> Result<i32, PointwiseError<E>>,
    preflight: &mut impl FnMut(i32, i32) -> Result<(), PointwiseError<E>>,
) -> Result<SharedImageMap, PointwiseError<E>> {
    let output = SharedImageMap::new(ImageGeometry::new(width, height));
    let options = inputs()?;
    let [mut sx, mut sy] = options.size;
    let [mut px, mut py] = options.location;
    let mut seed = options.seed;
    let negative = seed < 0.0;
    if negative {
        seed = -(seed + 1.0);
    }
    let count = integer(options.iterations);
    let mut total_weight = 1.0f32;
    let mut original_low = f32::MAX;
    let mut original_high = -f32::MAX;
    for octave in 0..count {
        let mut nx = integer(sx * 10.0);
        let mut ny = integer(sy * 10.0);
        if nx == 0 {
            nx = 1;
        }
        let cap = 3000i32.min(width.wrapping_mul(if octave == 0 { 8 } else { 2 }));
        nx = nx.min(cap).max(cap.wrapping_neg());
        if ny == 0 {
            ny = 1;
        }
        ny = ny.min(cap).max(cap.wrapping_neg());
        let mut ox = integer(px * 10.0);
        let mut oy = integer(py * 10.0).wrapping_add(ny);
        if nx < 0 {
            nx = nx.wrapping_neg();
            ox = ox.wrapping_sub(nx);
        }
        if ny < 0 {
            ny = ny.wrapping_neg();
            oy = oy.wrapping_add(ny);
        }
        let mut reduced = seed;
        if reduced == f64::INFINITY {
            // The literal source reduction never terminates. Do not invent an
            // output or silently replace this nonterminating input with a seed.
            return Err(PointwiseError::UnsupportedCapability(
                "Noise infinite seed reduction has no finite completion",
            ));
        }
        while reduced > 10000.0 {
            reduced /= 4.942381635;
        }
        let hash_offset = (reduced as f32) / f32::from_bits(0x3f9e0755);
        let grid = allocate(nx.wrapping_mul(ny))?;
        fill_lattice::<E>(
            &grid,
            nx,
            ny,
            [ox, oy],
            seed,
            hash_offset,
            negative && octave == 0,
            options.execution,
        )?;
        // Both discarded Double divisions are present in the selected body.
        let _ = f64::from(nx) / f64::from(width);
        let _ = f64::from(ny) / f64::from(height);
        let samples = if grid.len() == 1 {
            let array = allocate(1)?;
            array.write(0, grid.read(0)?)?;
            array
        } else {
            let array = allocate(width.wrapping_mul(height))?;
            resample_noise::<E>(
                {
                    let mut selected = 99;
                    for expected in 0..4 {
                        if smoothing()? == expected {
                            selected = expected;
                            break;
                        }
                    }
                    selected
                },
                Some(&grid),
                nx,
                ny,
                Some(&array),
                width,
                height,
                options.execution,
            )?;
            if octave > 0 {
                preflight(width, 2)?;
            }
            array
        };
        sx *= 2.0;
        sy *= 2.0;
        px *= 2.0;
        py *= 2.0;
        seed += 1.0;
        if octave == 0 {
            output.set_buffer(0, Some(&samples))?;
            if count <= 1 {
                continue;
            }
            (original_low, original_high) = extrema(&samples)?;
        } else {
            let weight = 1.0f32 / (libm::pow(2.0, f64::from(octave)) as f32);
            let destination = output.materialize(0)?;
            // The worker count is db.Length, so a later scalar lattice updates
            // only the first dense cell, not every output pixel.
            for_each_chunk::<E>(samples.len() as i32, options.execution, |start, count| {
                for index in start..start.wrapping_add(count) {
                    let index = usize::try_from(index).map_err(|_| ImageError::Index)?;
                    // Destination's old value is read BEFORE source db[index].
                    let old = destination.read(index)?;
                    destination.write(index, old + samples.read(index)? * weight)?;
                }
                Ok(())
            })?;
            total_weight += weight;
            if nx >= width || ny >= height {
                break;
            }
        }
    }
    if total_weight != 1.0 {
        let destination = output.materialize(0)?;
        let (low, high) = extrema(&destination)?;
        let factor = (1.0f32 / (high - low)) * (original_high - original_low);
        for_each_chunk::<E>(
            destination.len() as i32,
            options.execution,
            |start, count| {
                for index in start..start.wrapping_add(count) {
                    let index = usize::try_from(index).map_err(|_| ImageError::Index)?;
                    destination.write(
                        index,
                        (destination.read(index)? - low) * factor + original_low,
                    )?;
                }
                Ok(())
            },
        )?;
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    const EXECUTION: ProceduralExecution = ProceduralExecution::WorkerPool { processor_count: 2 };
    fn fixture(_: i32, _: i32) -> Result<(), PointwiseError<()>> {
        Ok(())
    }
    #[test]
    fn zero_iterations_and_single_cell_skip_smoothing_and_resource_queries() {
        let mut never =
            |_, _| -> Result<(), PointwiseError<()>> { panic!("resource callback must not run") };
        let empty = render_noise_with_preflight(
            NoiseOptions::new([1.0; 2], [0.0; 2], 0.0, 0.0, 3, EXECUTION),
            5,
            3,
            &mut never,
        )
        .unwrap();
        assert!(!empty.channel_observation().flags().unwrap()[0]);
        let a = render_noise_with_preflight(
            NoiseOptions::new([0.0; 2], [0.0; 2], 0.0, 1.0, 99, EXECUTION),
            5,
            3,
            &mut never,
        )
        .unwrap();
        let b = render_noise_with_preflight(
            NoiseOptions::new([0.0; 2], [0.0; 2], 0.0, 1.0, 3, EXECUTION),
            5,
            3,
            &mut never,
        )
        .unwrap();
        assert_eq!(
            a.channel(0, true).unwrap().values(),
            b.channel(0, true).unwrap().values()
        );
        assert_eq!(a.channel(0, true).unwrap().len(), 1);
    }
    #[test]
    fn actual_octave_resource_boundary_is_required_and_preserves_errors() {
        let mut calls = Vec::new();
        let options = NoiseOptions::new([0.2; 2], [0.0; 2], 17.0, 3.0, 1, EXECUTION);
        let mut preflight = |w, q| {
            calls.push((w, q));
            Err(PointwiseError::<()>::UnsupportedCapability(
                "GC fixture boundary",
            ))
        };
        assert!(matches!(
            render_noise_with_preflight(options, 7, 5, &mut preflight),
            Err(PointwiseError::UnsupportedCapability("GC fixture boundary"))
        ));
        assert_eq!(calls, vec![(7, 2)]);
        let image = render_noise_with_preflight(options, 7, 5, &mut fixture).unwrap();
        assert_eq!(image.geometry(), ImageGeometry::new(7, 5));
        assert_eq!(image.channel(0, true).unwrap().len(), 35);
    }
    #[test]
    fn seed_branches_unknown_smoothing_and_range_remap_are_not_normalized() {
        for smoothing in 0..4 {
            let image = render_noise_with_preflight(
                NoiseOptions::new([0.2, 0.3], [-0.2, 0.1], -3.0, 1.0, smoothing, EXECUTION),
                7,
                5,
                &mut fixture,
            )
            .unwrap();
            assert_eq!(image.channel(0, true).unwrap().len(), 35);
            assert!(!image.is_permanently_sealed());
        }
        let zero = render_noise_with_preflight(
            NoiseOptions::new([0.2; 2], [0.0; 2], 1.0, 1.0, 99, EXECUTION),
            7,
            5,
            &mut fixture,
        )
        .unwrap();
        assert_eq!(zero.channel(0, true).unwrap().values(), vec![0.0; 35]);
        assert!(matches!(
            render_noise_with_preflight(
                NoiseOptions::new([1.0; 2], [0.0; 2], f64::NAN, 1.0, 3, EXECUTION),
                7,
                5,
                &mut fixture
            ),
            Err(PointwiseError::Image(ImageError::Overflow))
        ));
    }
}
