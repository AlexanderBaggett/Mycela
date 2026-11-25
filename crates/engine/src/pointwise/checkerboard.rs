use super::{PointwiseError, PointwiseFilter, SharedImageMap, check_render_cancelled};
use crate::{ImageError, ImageGeometry};

pub(super) const MAX_CELL_SIZE: i32 = 2_000;
pub(super) const MAX_SCALE: i32 = 20_000;

const BLOCK: usize = 256;

fn cell_counts(x: u64, y: u64, scale: u64) -> [u64; 2] {
    let wide = x.max(y);
    let narrow = x.min(y);
    let fitted_pairs = (scale * wide + narrow / 2) / narrow;
    if x >= y {
        [2 * scale, 2 * fitted_pairs]
    } else {
        [2 * fitted_pairs, 2 * scale]
    }
}

fn even_cell_coverage(pixel: u64, dimension: u64, cells: u64) -> f32 {
    // In these units each cell spans one image dimension and a pixel spans
    // one cell count. Count the portion of the pixel covered by even cells.
    let covered_through = |position: u64| {
        let pair = 2 * dimension;
        (position / pair) * dimension + (position % pair).min(dimension)
    };
    let covered = covered_through((pixel + 1) * cells) - covered_through(pixel * cells);
    covered as f32 / cells as f32
}

pub(super) fn render<E: 'static>(
    filter: &PointwiseFilter<E>,
    geometry: ImageGeometry,
    time: f64,
    cancelled: &dyn Fn() -> bool,
) -> Result<SharedImageMap, PointwiseError<E>> {
    check_render_cancelled(cancelled)?;
    let width = geometry.width();
    let height = geometry.height();
    if width <= 0 || height <= 0 {
        return Err(ImageError::Argument.into());
    }
    width.checked_mul(height).ok_or(ImageError::Overflow)?;
    let color = |index| -> Result<[f32; 3], PointwiseError<E>> {
        let components = filter
            .parameter(index)?
            .effective_color_at(time)?
            .components();
        if components.iter().any(|value| !value.is_finite()) {
            return Err(ImageError::Argument.into());
        }
        Ok(components.map(|value| value.clamp(0.0, 1.0) as f32))
    };
    let number = |index, maximum| -> Result<u64, PointwiseError<E>> {
        let value = filter.parameter(index)?.effective_value_at(time)?;
        if !value.is_finite() {
            return Err(ImageError::Argument.into());
        }
        Ok(value.round().clamp(1.0, f64::from(maximum)) as u64)
    };
    let first = color(0)?;
    let second = color(1)?;
    let [columns, rows] = cell_counts(
        number(2, MAX_CELL_SIZE)?,
        number(3, MAX_CELL_SIZE)?,
        number(4, MAX_SCALE)?,
    );

    let width = width as usize;
    let height = height as usize;
    let output = SharedImageMap::new(geometry);
    output.set_constant(3, 0.0)?;
    if first == second {
        for (channel, value) in first.into_iter().enumerate() {
            output.set_constant(channel as i32, value)?;
        }
        return Ok(output);
    }

    let mut horizontal_coverage = Vec::new();
    horizontal_coverage
        .try_reserve_exact(width)
        .map_err(|_| ImageError::Allocation)?;
    for x in 0..width {
        if x % BLOCK == 0 {
            check_render_cancelled(cancelled)?;
        }
        horizontal_coverage.push(even_cell_coverage(x as u64, width as u64, columns));
    }
    let planes = [
        output.materialize(0)?,
        output.materialize(1)?,
        output.materialize(2)?,
    ];
    let mut samples = [[0.0; BLOCK]; 3];
    for y in 0..height {
        let vertical = even_cell_coverage(y as u64, height as u64, rows);
        for (block, horizontal) in horizontal_coverage.chunks(BLOCK).enumerate() {
            check_render_cancelled(cancelled)?;
            for (index, &horizontal) in horizontal.iter().enumerate() {
                let weight = horizontal * vertical + (1.0 - horizontal) * (1.0 - vertical);
                for channel in 0..3 {
                    samples[channel][index] =
                        first[channel] * weight + second[channel] * (1.0 - weight);
                }
            }
            for (plane, channel) in planes.iter().zip(&samples) {
                plane.write_block(y * width + block * BLOCK, 1, &channel[..horizontal.len()])?;
            }
        }
    }
    Ok(output)
}

#[cfg(test)]
#[path = "checkerboard_test.rs"]
mod test;
