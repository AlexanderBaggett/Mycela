//! Source bd. The Environment value renders at its own latitude–longitude size.
use super::{PointwiseError, PointwiseFilter, SharedImageMap};
use crate::{EnvironmentResourceLibrary, ImageError, ImageGeometry, ImageMap, ProceduralExecution};

pub(super) fn retained<E: 'static>(
    filter: &PointwiseFilter<E>,
    width: i32,
    height: i32,
    time: f64,
    execution: ProceduralExecution,
    library: Option<&EnvironmentResourceLibrary>,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    super::check_render_cancelled(cancelled)?;
    if width <= 0 || height <= 0 {
        return Err(PointwiseError::Argument);
    }
    filter.ensure_property_evaluation_supported()?;
    let environment = filter.parameter(0)?.effective_environment()?;
    let limit = match filter.parameter(1)?.effective_index_at(time)? {
        0 => width,
        1 => 3200,
        2 => 1600,
        3 => 800,
        4 => 400,
        5 => 200,
        6 => 100,
        _ => return Err(PointwiseError::Index),
    };
    let native_width = width.min(limit);
    let native_height = (native_width / 2).max(1);
    let native = environment.render_latlong(
        ImageGeometry::new(native_width, native_height),
        execution,
        library,
        cancelled,
    )?;
    let image = if width == native_width && height == native_height * 2 {
        double_latitude(&native, cancelled)?
    } else {
        super::image_resize::resize_clamped(&native, width, height, execution)?
    };
    super::check_render_cancelled(cancelled)?;
    Ok(Some(image))
}

// ImageMap.b(map, true) copies original rows to even positions. The odd
// positions interpolate adjacent rows, repeating the final sample at the pole.
fn double_latitude<E>(
    native: &SharedImageMap,
    cancelled: &dyn Fn() -> bool,
) -> Result<SharedImageMap, PointwiseError<E>> {
    let width = native.geometry().width();
    let height = native.geometry().height();
    let mut image = ImageMap::new(ImageGeometry::new(width, height * 2));
    let width = width as usize;
    let height = height as usize;
    let area = width.checked_mul(height * 2).ok_or(ImageError::Overflow)?;
    for channel in 0..4 {
        let samples = native.channel(channel, true)?;
        if samples.len() == 1 {
            image.set_constant(channel, samples.read(0)?)?;
            continue;
        }
        let mut output = Vec::new();
        output
            .try_reserve_exact(area)
            .map_err(|_| ImageError::Allocation)?;
        let samples = samples.values();
        for (row, current) in samples.chunks_exact(width).enumerate() {
            super::check_render_cancelled(cancelled)?;
            let next_row = (row + 1).min(height - 1);
            let next = &samples[next_row * width..][..width];
            output.extend_from_slice(current);
            output.extend(current.iter().zip(next).map(|(a, b)| (a + b) * 0.5));
        }
        image.set_array(channel, output)?;
    }
    Ok(SharedImageMap::from_image(image))
}
