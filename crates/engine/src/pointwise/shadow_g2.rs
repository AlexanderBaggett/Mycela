//! atr's legacy Shadow G2 projection used by saved textures.
use super::{
    PointwiseError, PointwiseFilter, PointwiseRenderOptions, SharedImageMap, check_render_cancelled,
};
use crate::{FloatBuffer, ImageError, ImageGeometry, framework_cast::integer};

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
    let source = input.materialize(0)?;
    let mut height_factor =
        (filter.parameter(0)?.effective_value_at(time)? / filter.factor()) as f32;
    if options.is_color_only() {
        let skip = filter.parameter(4)?;
        let choices = skip.options()?.entries();
        let choice =
            usize::try_from(skip.effective_index_at(time)?).map_err(|_| PointwiseError::Index)?;
        if choices
            .get(choice)
            .ok_or(PointwiseError::Index)?
            .as_ref()
            .is_some_and(|name| name.as_slice() == [89, 101, 115])
        {
            height_factor = 0.0;
        }
    }
    let light = filter
        .parameter(1)?
        .custom_object()?
        .ok_or(PointwiseError::NullReference)?;
    let [light_x, light_y] = light.light_position()?;
    let strength = super::point_light::max_clr(
        f64::from(filter.parameter(2)?.effective_value_at(time)? as f32),
        f64::from(0.0001f32),
    ) as f32;
    let quality = super::point_light::max_clr(
        f64::from(filter.parameter(3)?.effective_value_at(time)? as f32),
        f64::from(0.0001f32),
    ) as f32;
    let image = SharedImageMap::new(geometry);
    let width = geometry.width();
    let height = geometry.height();
    if width <= 0 || height <= 0 {
        return Err(PointwiseError::Argument);
    }
    let light_length = (1.0 / (light_x * light_x + light_y * light_y).sqrt() * 2.0) as f32;
    let height_step = 1.0 / (width.max(height) as f32 * quality / light_length);
    let shade_step = height_step / strength;
    if shade_step == 0.0 {
        return Err(PointwiseError::UnsupportedCapability(
            "Shadow G2 ray step rounded to zero",
        ));
    }
    let x_step = -(light_x as f32) / quality;
    let y_step = light_y as f32 / quality;
    let miss_limit = integer(f64::from(quality)).wrapping_mul(10);
    let source = source.values();
    let length = width.checked_mul(height).ok_or(ImageError::Overflow)? as usize;
    if source.len() < length {
        return Err(PointwiseError::Index);
    }
    let mut colors = Vec::new();
    colors
        .try_reserve_exact(length)
        .map_err(|_| ImageError::Allocation)?;
    colors.resize(length, 1.0);
    for y in 0..height {
        check_render_cancelled(cancelled)?;
        for x in 0..width {
            let mut ray_height = source[x as usize + y as usize * width as usize] * height_factor;
            let mut ray_x = x as f32;
            let mut ray_y = y as f32;
            let mut misses = 0;
            let mut shade = 0.0f32;
            let mut steps = 0u32;
            while shade < 1.0 {
                steps = steps.wrapping_add(1);
                if steps.is_multiple_of(64) {
                    check_render_cancelled(cancelled)?;
                }
                ray_x += x_step;
                ray_y += y_step;
                ray_height -= height_step;
                let sample_x = integer(f64::from(ray_x).round_ties_even()).rem_euclid(width);
                let sample_y = integer(f64::from(ray_y).round_ties_even()).rem_euclid(height);
                let sample = (sample_x + sample_y * width) as usize;
                let sample_height = source.get(sample).ok_or(ImageError::Index)? * height_factor;
                let color = colors.get_mut(sample).ok_or(ImageError::Index)?;
                if ray_height < sample_height {
                    if misses >= miss_limit {
                        break;
                    }
                    misses += 1;
                } else if shade < *color {
                    *color = shade;
                }
                let next = shade + shade_step;
                if next == shade {
                    return Err(PointwiseError::UnsupportedCapability(
                        "Shadow G2 ray step stopped advancing",
                    ));
                }
                shade = next;
            }
        }
    }
    image.set_buffer(0, Some(&FloatBuffer::new(colors)))?;
    image.copy_auxiliary(input)?;
    Ok(Some(image))
}
