//! The four ImageMap resamplers reached by Noise with both flags false, plus
//! the same Cubic authority reached by Shape resize with half-pixel true.
//! Periodic boundaries and cubic's distinct edge/interior accumulation orders
//! follow the selected bodies. Caller arrays stay aliased; no readonly copy.
use crate::procedural_schedule::{ProceduralExecution, for_each_chunk};
use crate::{FloatBuffer, ImageError, PointwiseError, framework_cast::integer};

pub(crate) fn allocate(count: i32) -> Result<FloatBuffer, ImageError> {
    let count = usize::try_from(count).map_err(|_| ImageError::Overflow)?;
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| ImageError::Allocation)?;
    values.resize(count, 0.0);
    Ok(FloatBuffer::new(values))
}
fn doubles(count: i32) -> Result<Vec<f64>, ImageError> {
    let count = usize::try_from(count).map_err(|_| ImageError::Overflow)?;
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| ImageError::Allocation)?;
    values.resize(count, 0.0);
    Ok(values)
}
fn read_raw(source: Option<&FloatBuffer>, index: i32) -> Result<f32, ImageError> {
    let source = source.ok_or(ImageError::NullReference)?;
    source.read(usize::try_from(index).map_err(|_| ImageError::Index)?)
}
fn read(source: Option<&FloatBuffer>, index: i32) -> Result<f64, ImageError> {
    // Arithmetic samplers perform conv.r8 after the reached ldelem.r4.
    Ok(f64::from(read_raw(source, index)?))
}
fn write(destination: Option<&FloatBuffer>, index: i32, value: f32) -> Result<(), ImageError> {
    destination.ok_or(ImageError::NullReference)?.write(
        usize::try_from(index).map_err(|_| ImageError::Index)?,
        value,
    )
}
fn at(x: i32, y: i32, width: i32) -> i32 {
    x.wrapping_add(y.wrapping_mul(width))
}
fn periodic<E>(value: i32, extent: i32) -> Result<i32, PointwiseError<E>> {
    let mut value = value.checked_rem(extent).ok_or(if extent == 0 {
        PointwiseError::DivideByZero
    } else {
        PointwiseError::Image(ImageError::Overflow)
    })?;
    if value < 0 {
        value = value.wrapping_add(extent);
    }
    Ok(value)
}
// ImageMap.b(Double,out4). Its internal q calls use distinct real array cells;
// this does not expose or replace the public helper's aliased-out contract.
fn coefficients(x: f64) -> [f64; 4] {
    let mut n = x + 2.0;
    let mut cube = n * n * n;
    let a = cube * (1.0 / 6.0);
    let mut b = (-cube) * (2.0 / 3.0);
    let mut c = cube;
    let mut d = b;
    n = x + 3.0;
    cube = n * n * n;
    b += cube * (1.0 / 6.0);
    c -= cube * (2.0 / 3.0);
    d += cube;
    n = x + 4.0;
    cube = n * n * n;
    c += cube * (1.0 / 6.0);
    d -= cube * (2.0 / 3.0);
    n = x + 5.0;
    d += n * n * n * (1.0 / 6.0);
    [a, b, c, d]
}
fn row_parameters(mode: i32, y: i32, ratio: f64, height: i32, offset: f64) -> (i32, i32, f64) {
    // Noise passes the actual +0.0 flag operand; Shape resize passes -0.5.
    let mut position = f64::from(y) * ratio + offset;
    if position < 0.0 {
        position += f64::from(height);
    }
    let row = integer(position);
    let mut fraction = position - f64::from(row);
    if mode == 2 {
        fraction = (1.0 - libm::cos(fraction * std::f64::consts::PI)) / 2.0;
    }
    let mut next = row.wrapping_add(1);
    if next >= height {
        next = next.wrapping_sub(height);
    }
    (row, next, fraction)
}
/// Noise-selected flags: periodic, no half-pixel offset. Unknown numeric choice
/// performs no worker call and leaves destination untouched. Null/short arrays
/// fail at the reached read/write, retaining the written prefix. No source array
/// snapshot or eager shape validation changes that order.
///
/// The explicit source profile supplies actual nz chunk boundaries and per-bx
/// scratch lifetime. Chunks execute sequentially; cross-worker alias/error races,
/// nested-dispatch failures and other flag profiles remain unproved.
#[allow(clippy::too_many_arguments)] // Original width/height pairs and actual caller arrays.
pub fn resample_noise<E>(
    mode: i32,
    source: Option<&FloatBuffer>,
    sw: i32,
    sh: i32,
    destination: Option<&FloatBuffer>,
    dw: i32,
    dh: i32,
    execution: ProceduralExecution,
) -> Result<(), PointwiseError<E>> {
    resample(
        mode,
        source,
        sw,
        sh,
        destination,
        dw,
        dh,
        execution,
        0.0,
        false,
    )
}
/// Actual ImageMap.b6 resize selects Cubic with half-pixel=true; noise retains
/// its existing false/false seam. No copied source array or second kernel.
#[allow(clippy::too_many_arguments)]
pub(crate) fn resample_resize_cubic<E>(
    source: Option<&FloatBuffer>,
    sw: i32,
    sh: i32,
    destination: Option<&FloatBuffer>,
    dw: i32,
    dh: i32,
    execution: ProceduralExecution,
    clamp_y: bool,
) -> Result<(), PointwiseError<E>> {
    resample(
        3,
        source,
        sw,
        sh,
        destination,
        dw,
        dh,
        execution,
        if clamp_y { 0.0 } else { -0.5 },
        clamp_y,
    )
}
/// Shared ImageMap b3 upsample selection, preserving its actual half-pixel flag.
#[allow(clippy::too_many_arguments)]
pub(crate) fn resample_resize<E>(
    mode: i32,
    source: Option<&FloatBuffer>,
    sw: i32,
    sh: i32,
    destination: Option<&FloatBuffer>,
    dw: i32,
    dh: i32,
    execution: ProceduralExecution,
    clamp_y: bool,
) -> Result<(), PointwiseError<E>> {
    if mode == 3 {
        return resample_resize_cubic(source, sw, sh, destination, dw, dh, execution, clamp_y);
    }
    resample(
        mode,
        source,
        sw,
        sh,
        destination,
        dw,
        dh,
        execution,
        if clamp_y { 0.0 } else { -0.5 },
        clamp_y,
    )
}
#[allow(clippy::too_many_arguments)]
fn resample<E>(
    mode: i32,
    source: Option<&FloatBuffer>,
    sw: i32,
    sh: i32,
    destination: Option<&FloatBuffer>,
    dw: i32,
    dh: i32,
    execution: ProceduralExecution,
    offset: f64,
    clamp_y: bool,
) -> Result<(), PointwiseError<E>> {
    if !(0..=3).contains(&mode) {
        return Ok(());
    }
    let rx = f64::from(sw) / f64::from(dw);
    let ry = f64::from(sh) / f64::from(dh);
    // q constructs four real X arrays before z5, even for zero output area.
    let weights = if mode == 3 {
        let mut weights = [doubles(dw)?, doubles(dw)?, doubles(dw)?, doubles(dw)?];
        for x in 0..dw {
            let mut position = f64::from(x) * rx + offset;
            if position < 0.0 {
                position += f64::from(sw);
            }
            let fraction = position - f64::from(integer(position));
            let values = coefficients(-1.0 - fraction);
            for i in 0..4 {
                weights[i][x as usize] = values[i];
            }
        }
        Some(weights)
    } else {
        None
    };
    let area = dw.wrapping_mul(dh);
    for_each_chunk::<E>(area, execution, |start, count| {
        let mut y = start.checked_div(dw).ok_or(if dw == 0 {
            PointwiseError::DivideByZero
        } else {
            PointwiseError::Image(ImageError::Overflow)
        })?;
        let mut x = start.checked_rem(dw).ok_or(if dw == 0 {
            PointwiseError::DivideByZero
        } else {
            PointwiseError::Image(ImageError::Overflow)
        })?;
        // q bx allocates both scratch arrays, even when its loop has no iterations.
        let mut wy = if mode == 3 { Some(doubles(4)?) } else { None };
        let mut edge_x = if mode == 3 { Some(doubles(4)?) } else { None };
        let mut row = if mode == 0 {
            (0, 0, 0.0)
        } else {
            row_parameters(mode, y, ry, sh, offset)
        };
        if let Some(wy) = wy.as_mut() {
            let values = coefficients(row.2 - 2.0);
            for i in 0..4 {
                wy[3 - i] = values[i];
            }
        }
        for index in start..start.wrapping_add(count) {
            if x >= dw {
                x = 0;
                y = y.wrapping_add(1);
                if mode != 0 {
                    row = row_parameters(mode, y, ry, sh, offset);
                }
                if let Some(wy) = wy.as_mut() {
                    let values = coefficients(row.2 - 2.0);
                    for i in 0..4 {
                        wy[3 - i] = values[i];
                    }
                }
            }
            let value = if mode == 0 {
                read_raw(
                    source,
                    at(integer(f64::from(x) * rx), integer(f64::from(y) * ry), sw),
                )?
            } else {
                let (sy, ny, fy) = row;
                let mut position = f64::from(x) * rx + offset;
                if position < 0.0 {
                    position += f64::from(sw);
                }
                let sx = integer(position);
                if mode == 3 {
                    let weights = weights.as_ref().ok_or(ImageError::NullReference)?;
                    let wy = wy.as_ref().ok_or(ImageError::NullReference)?;
                    let edge_x = edge_x.as_mut().ok_or(ImageError::NullReference)?;
                    let wx = [
                        weights[0][x as usize],
                        weights[1][x as usize],
                        weights[2][x as usize],
                        weights[3][x as usize],
                    ];
                    let ey = sy < 1 || sy >= sh.wrapping_sub(2);
                    let ex = sx < 1 || sx >= sw.wrapping_sub(2);
                    let mut sum = 0.0;
                    if ey || ex {
                        if ex {
                            edge_x.copy_from_slice(&wx);
                        }
                        for dy in -1i32..=2 {
                            let row = sy.wrapping_add(dy);
                            let offset = (if clamp_y {
                                if row >= sh {
                                    sh.wrapping_sub(1)
                                } else if row < 0 {
                                    0
                                } else {
                                    row
                                }
                            } else {
                                periodic::<E>(row, sh)?
                            })
                            .wrapping_mul(sw);
                            let weight_y = wy[(dy + 1) as usize];
                            if ex {
                                for dx in -1i32..=2 {
                                    let column = periodic::<E>(sx.wrapping_add(dx), sw)?;
                                    sum += read(source, column.wrapping_add(offset))?
                                        * edge_x[(dx + 1) as usize]
                                        * weight_y;
                                }
                            } else {
                                let p = sx.wrapping_add(offset);
                                sum += (read(source, p.wrapping_sub(1))? * wx[0]
                                    + read(source, p)? * wx[1]
                                    + read(source, p.wrapping_add(1))? * wx[2]
                                    + read(source, p.wrapping_add(2))? * wx[3])
                                    * weight_y;
                            }
                        }
                    } else {
                        for dy in -1i32..=2 {
                            let p = at(sx, sy.wrapping_add(dy), sw);
                            sum += (read(source, p.wrapping_sub(1))? * wx[0]
                                + read(source, p)? * wx[1]
                                + read(source, p.wrapping_add(1))? * wx[2]
                                + read(source, p.wrapping_add(2))? * wx[3])
                                * wy[(dy + 1) as usize];
                        }
                    }
                    sum as f32
                } else {
                    let mut fraction = position - f64::from(sx);
                    if mode == 2 {
                        fraction = (1.0 - libm::cos(fraction * std::f64::consts::PI)) / 2.0;
                    }
                    let mut nx = sx.wrapping_add(1);
                    if nx >= sw {
                        nx = nx.wrapping_sub(sw);
                    }
                    (read(source, at(sx, sy, sw))? * (1.0 - fraction) * (1.0 - fy)
                        + read(source, at(sx, ny, sw))? * (1.0 - fraction) * fy
                        + read(source, at(nx, sy, sw))? * fraction * (1.0 - fy)
                        + read(source, at(nx, ny, sw))? * fraction * fy) as f32
                }
            };
            write(destination, index, value)?;
            x = x.wrapping_add(1);
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    const EXECUTION: ProceduralExecution = ProceduralExecution::WorkerPool { processor_count: 2 };
    #[test]
    fn nearest_copies_raw_single_bits_without_an_arithmetic_round_trip() {
        // Static raw-copy law, not an observed CLR/JIT sNaN golden.
        let bits = [
            0x80000000, 0x00000000, 0x7fc00001, 0xffc00002, 0x7f800001, 0xff800123, 0x7f800000,
            0xff800000,
        ];
        let source = FloatBuffer::new(bits.into_iter().map(f32::from_bits).collect());
        let output = allocate(bits.len() as i32).unwrap();
        resample_noise::<()>(0, Some(&source), 8, 1, Some(&output), 8, 1, EXECUTION).unwrap();
        assert_eq!(
            output
                .values()
                .into_iter()
                .map(f32::to_bits)
                .collect::<Vec<_>>(),
            bits
        );
    }
    #[test]
    fn periodic_sampling_keeps_source_grids_and_reached_partial_writes() {
        let source = FloatBuffer::new(vec![0.0, 1.0]);
        let output = allocate(4).unwrap();
        resample_noise::<()>(0, Some(&source), 2, 1, Some(&output), 4, 1, EXECUTION).unwrap();
        assert_eq!(output.values(), vec![0.0, 0.0, 1.0, 1.0]);
        resample_noise::<()>(1, Some(&source), 2, 1, Some(&output), 4, 1, EXECUTION).unwrap();
        assert_eq!(output.values(), vec![0.0, 0.5, 1.0, 0.5]);
        let short = FloatBuffer::new(vec![-9.0]);
        assert!(matches!(
            resample_noise::<()>(0, Some(&source), 2, 1, Some(&short), 4, 1, EXECUTION),
            Err(PointwiseError::Image(ImageError::Index))
        ));
        assert_eq!(short.values(), vec![0.0]);
        // All four cells are read even when their coefficient is zero.
        assert!(matches!(
            resample_noise::<()>(1, Some(&source), 2, 2, Some(&output), 1, 1, EXECUTION),
            Err(PointwiseError::Image(ImageError::Index))
        ));
        resample_noise::<()>(99, None, -1, -1, None, -1, -1, EXECUTION).unwrap();
    }
    #[test]
    fn cubic_has_true_division_error_and_does_not_hide_array_aliases() {
        let source = FloatBuffer::new(vec![0.5; 6]);
        let output = allocate(35).unwrap();
        for mode in [2, 3] {
            resample_noise::<()>(mode, Some(&source), 3, 2, Some(&output), 7, 5, EXECUTION)
                .unwrap();
            assert!(
                output
                    .values()
                    .into_iter()
                    .all(|x| (x - 0.5).abs() < 0.00001)
            );
        }
        assert!(matches!(
            resample_noise::<()>(3, None, 0, 1, None, 1, 1, EXECUTION),
            Err(PointwiseError::DivideByZero)
        ));
        let alias = FloatBuffer::new(vec![0.0, 1.0, 2.0, 3.0]);
        resample_noise::<()>(0, Some(&alias), 2, 2, Some(&alias), 4, 1, EXECUTION).unwrap();
        assert_eq!(alias.values(), vec![0.0; 4]);
        let invalid = FloatBuffer::new(vec![f32::NAN]);
        resample_noise::<()>(99, None, 0, 0, Some(&invalid), 1, 1, EXECUTION).unwrap();
        assert!(invalid.read(0).unwrap().is_nan());
    }
}
