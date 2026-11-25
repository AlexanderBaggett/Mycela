//! Rotate a tile on the requested render grid.
use super::{PointwiseError, PointwiseFilter, SharedImageMap};
use crate::procedural_schedule::for_each_chunk;
use crate::{FloatBuffer, ImageError, ImageGeometry, ProceduralExecution, SharedFloatStore};

fn allocated(length: i32) -> Result<FloatBuffer, ImageError> {
    let length = usize::try_from(length).map_err(|_| ImageError::Overflow)?;
    let mut values = Vec::new();
    values
        .try_reserve_exact(length)
        .map_err(|_| ImageError::Allocation)?;
    values.resize(length, 0.0);
    Ok(FloatBuffer::new(values))
}
fn worker<E>(
    source: &FloatBuffer,
    destination: &FloatBuffer,
    geometry: ImageGeometry,
    mode: i32,
    start: i32,
    count: i32,
) -> Result<(), PointwiseError<E>> {
    let width = geometry.width();
    let height = geometry.height();
    let mut row = start.checked_div(width).ok_or(if width == 0 {
        PointwiseError::DivideByZero
    } else {
        ImageError::Overflow.into()
    })?;
    let mut column = start.checked_rem(width).ok_or(if width == 0 {
        PointwiseError::DivideByZero
    } else {
        ImageError::Overflow.into()
    })?;
    let end = start.wrapping_add(count);
    let mut index = start;
    while index < end {
        if column >= width {
            column = 0;
            row = row.wrapping_add(1);
        }
        let target = match mode {
            1 => width
                .wrapping_sub(1)
                .wrapping_sub(row)
                .wrapping_add(column.wrapping_mul(width)),
            2 => width
                .wrapping_sub(1)
                .wrapping_add(height.wrapping_sub(1).wrapping_sub(row).wrapping_mul(width))
                .wrapping_sub(column),
            3 => row.wrapping_add(
                height
                    .wrapping_sub(1)
                    .wrapping_sub(column)
                    .wrapping_mul(width),
            ),
            _ => unreachable!("worker selected only by actual three switch arms"),
        };
        // ldelem.r4 happens before stelem.r4 validates the destination index.
        let value = source.read(usize::try_from(index).map_err(|_| ImageError::Index)?)?;
        destination.write(
            usize::try_from(target).map_err(|_| ImageError::Index)?,
            value,
        )?;
        index = index.wrapping_add(1);
        column = column.wrapping_add(1);
    }
    Ok(())
}
fn dispatch<E>(
    source: &FloatBuffer,
    destination: &FloatBuffer,
    geometry: ImageGeometry,
    input_geometry: ImageGeometry,
    mode: i32,
    execution: ProceduralExecution,
    labels: bool,
) -> Result<(), PointwiseError<E>> {
    let width = geometry.width();
    let height = geometry.height();
    let input_width = input_geometry.width();
    let input_height = input_geometry.height();
    if matches!(mode, 1 | 3) && needs_resampling(geometry, input_geometry) {
        if width <= 0 || height <= 0 || input_width <= 0 || input_height <= 0 {
            return Err(PointwiseError::Argument);
        }
        let input_area = input_width
            .checked_mul(input_height)
            .ok_or(ImageError::Overflow)?;
        let area = width.checked_mul(height).ok_or(ImageError::Overflow)?;
        if source.len() < input_area as usize {
            return Err(ImageError::Index.into());
        }
        // Inverse-map the destination's texel centers through the rotated tile.
        let x_scale = f64::from(input_height) / f64::from(width);
        let y_scale = f64::from(input_width) / f64::from(height);
        let clockwise = mode == 1;
        for_each_chunk(area, execution, |start, count| {
            for cell in start..start + count {
                let x = (f64::from(cell % width) + 0.5) * x_scale;
                let y = (f64::from(cell / width) + 0.5) * y_scale;
                let value = if labels {
                    // Choose a texel in the rotated grid before undoing the turn.
                    let x = (x as i32).min(input_height - 1);
                    let y = (y as i32).min(input_width - 1);
                    let (sx, sy) = if clockwise {
                        (y, input_height - 1 - x)
                    } else {
                        (input_width - 1 - y, x)
                    };
                    source.read((sy * input_width + sx) as usize)?
                } else {
                    let (sx, sy) = if clockwise {
                        (y - 0.5, f64::from(input_height) - x - 0.5)
                    } else {
                        (f64::from(input_width) - y - 0.5, x - 0.5)
                    };
                    if sx.fract() == 0. && sy.fract() == 0. {
                        source.read((sy as i32 * input_width + sx as i32) as usize)?
                    } else {
                        super::transform::sample_double(sx, sy, source, input_width, input_height)?
                    }
                };
                destination.write(cell as usize, value)?;
            }
            Ok(())
        })?;
    } else if matches!(mode, 1..=3) {
        for_each_chunk(geometry.area(), execution, |start, count| {
            worker(source, destination, geometry, mode, start, count)
        })?;
    }
    Ok(())
}
fn needs_resampling(geometry: ImageGeometry, input: ImageGeometry) -> bool {
    geometry.width() != geometry.height()
        || geometry.width() != input.width()
        || geometry.height() != input.height()
}

/// Rotates all image planes. Square turns use the original exact permutation.
/// Rectangular quarter turns sample the rotated tile on the requested grid.
/// Zero returns the nullable input before geometry/map access. Unknown nonzero values
/// still allocate/read/seal channels and allocate/install region storage.
/// Uses the existing resident store/allocation authority; full original memory
/// emergency/spill, actual threads and parallel error races remain open.
pub fn render_rotate<E>(
    input: Option<&SharedImageMap>,
    geometry: ImageGeometry,
    mode: i32,
    execution: ProceduralExecution,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    if mode == 0 {
        return Ok(input.cloned());
    }
    let out = SharedImageMap::new(ImageGeometry::new(geometry.width(), geometry.height()));
    let input = input.ok_or(PointwiseError::NullReference)?;
    let input_geometry = input.geometry();
    let mut channel = 0i32;
    while channel < input.all_channel_count() {
        let source = input.channel_with_effects(channel, true)?;
        if source.len() == 1 {
            out.copy_indexed_constant(channel, source.read(0)?, input)?;
        } else {
            let destination = out.allocate_indexed(channel, input)?;
            dispatch(
                &source,
                &destination,
                geometry,
                input_geometry,
                mode,
                execution,
                false,
            )?;
        }
        out.seal_channel(channel)?;
        channel = channel.wrapping_add(1);
    }
    let source = if input.region_store().is_none() {
        None
    } else {
        Some(
            input
                .region_store()
                .ok_or(PointwiseError::NullReference)?
                .data(true)?,
        )
    };
    if let Some(source) = source {
        let length = if matches!(mode, 1 | 3) && needs_resampling(geometry, input_geometry) {
            geometry.area()
        } else {
            source.len() as i32
        };
        let destination = allocated(length)?;
        dispatch(
            &source,
            &destination,
            geometry,
            input_geometry,
            mode,
            execution,
            true,
        )?;
        out.set_region_store(Some(SharedFloatStore::new(destination)?));
    }
    Ok(Some(out))
}
pub(super) fn retained<E: 'static>(
    filter: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    geometry: ImageGeometry,
    time: f64,
    execution: ProceduralExecution,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    // k8 resolves the optional Input before the actual h(context) text getter.
    let input = inputs.first().ok_or(PointwiseError::Index)?;
    let p = filter.parameter(0)?;
    let index = p.effective_index_at(time)?;
    let options = p.options()?.entries();
    let text = usize::try_from(index)
        .ok()
        .and_then(|i| options.get(i))
        .ok_or(PointwiseError::Index)?
        .as_deref();
    let mode = if text.is_some_and(|s| s.iter().copied().eq("90 Clockwise".encode_utf16())) {
        1
    } else if text.is_some_and(|s| s.iter().copied().eq("180".encode_utf16())) {
        2
    } else if text.is_some_and(|s| s.iter().copied().eq("90 Counterclockwise".encode_utf16())) {
        3
    } else {
        0
    };
    render_rotate(input.as_ref(), geometry, mode, execution)
}

#[cfg(test)]
#[path = "rotate_test.rs"]
mod tests;
