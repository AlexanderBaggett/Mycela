//! zg's protected render. A node read through zl.g uses the Point Light migration.
use super::{
    PointwiseError, PointwiseFilter, PointwiseParameter, PointwiseRenderOptions, SharedImageMap,
    check_render_cancelled,
};
use crate::{FloatBuffer, ImageError, ImageGeometry};

fn selected<E: 'static>(
    property: PointwiseParameter<E>,
    time: f64,
    name: &str,
) -> Result<bool, PointwiseError<E>> {
    let index =
        usize::try_from(property.effective_index_at(time)?).map_err(|_| PointwiseError::Index)?;
    Ok(property
        .options()?
        .entries()
        .get(index)
        .ok_or(PointwiseError::Index)?
        .as_ref()
        .is_some_and(|entry| entry.iter().copied().eq(name.encode_utf16())))
}

pub(super) fn retained<E: 'static>(
    filter: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    geometry: ImageGeometry,
    time: f64,
    options: PointwiseRenderOptions,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    let input = inputs
        .first()
        .ok_or(PointwiseError::Index)?
        .as_ref()
        .ok_or(PointwiseError::NullReference)?;
    let source = input.channel(0, false)?;
    let mut height_factor = filter.parameter(0)?.effective_value_at(time)? / filter.factor();
    if options.is_color_only() && selected(filter.parameter(3)?, time, "Yes")? {
        height_factor = 0.0;
    }
    let position = filter
        .parameter(1)?
        .custom_object()?
        .ok_or(PointwiseError::NullReference)?;
    let [light_x, light_y] = position.light_position()?;
    let (nx, ny, nz) = super::point_light::light_direction(light_x, light_y);
    let half = selected(filter.parameter(2)?, time, "Half")?;
    let image = SharedImageMap::new(geometry);
    let intensity = |light: f32| {
        if half {
            if light > 0.0 { light } else { 0.0 }
        } else {
            light * 0.5 + 0.5
        }
    };
    if source.len() == 1 {
        let normal = super::point_light::max_clr(0.0, -nz) as f32;
        image.set_constant(0, if half { normal } else { normal * 0.5 + 0.5 })?;
    } else {
        let width = usize::try_from(geometry.width()).map_err(|_| ImageError::Argument)?;
        let height = usize::try_from(geometry.height()).map_err(|_| ImageError::Argument)?;
        let source = source.values();
        let length = width.checked_mul(height).ok_or(ImageError::Overflow)?;
        if source.is_empty() || width == 0 || height == 0 {
            return Err(ImageError::Argument.into());
        }
        let source = source.get(..length).ok_or(ImageError::Index)?;
        let mut output = Vec::new();
        output
            .try_reserve_exact(length)
            .map_err(|_| ImageError::Allocation)?;
        let x_scale = width as f64 * height_factor;
        let y_scale = height as f64 * height_factor;
        for row in 0..height {
            check_render_cancelled(cancelled)?;
            let start = row * width;
            let next = ((row + 1) % height) * width;
            for col in 0..width {
                let current = f64::from(source[start + col]);
                let right = f64::from(source[start + (col + 1) % width]);
                let below = f64::from(source[next + col]);
                let gx = (right - current) * x_scale;
                let gy = (below - current) * y_scale;
                let normal = (gx * gx + gy * gy + 1.0).sqrt();
                output.push(intensity(((nx * gx + ny * gy - nz) / normal) as f32));
            }
        }
        image.set_buffer(0, Some(&FloatBuffer::new(output)))?;
    }
    image.copy_auxiliary(input)?;
    Ok(Some(image))
}
