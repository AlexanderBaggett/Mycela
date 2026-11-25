use super::{
    CanvasSymmetry,
    adjust::AdjustRgb,
    area,
    blend::{BlendGrid, StyleBlend},
    distance, environment,
    height_map::StyleHeightMap,
    named, noise,
    shape::{Outline, OutlineSampling},
    style::{Paint, StylePass},
};
use crate::pointwise::{
    PointwiseError, PointwiseFilter, PointwiseRenderEffects, PointwiseRenderOptions,
    SharedImageMap, blur, planar_optics,
};
use crate::{
    FloatBuffer, GradientRecord, ImageError, ImageGeometry, ProceduralExecution, SharedGradient,
};

pub(in crate::pointwise) fn render<E: 'static>(
    filter: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    geometry: ImageGeometry,
    time: f64,
    execution: ProceduralExecution,
    options: PointwiseRenderOptions,
    effects: &mut PointwiseRenderEffects<'_, E>,
) -> Result<SharedImageMap, PointwiseError<E>> {
    let cancelled = effects.cancel_check();
    let document = filter
        .canvas_document()
        .ok_or(PointwiseError::NullReference)?;
    let image = SharedImageMap::new(geometry);
    image.set_constant(3, 1.)?;
    if !document
        .layers
        .iter()
        .any(|layer| layer.header.container_visible())
    {
        return Ok(image);
    }
    let width = usize::try_from(geometry.width()).map_err(|_| ImageError::Argument)?;
    let height = usize::try_from(geometry.height()).map_err(|_| ImageError::Argument)?;
    let active_inputs = filter.node().reference_inputs()?.len();
    let length = width.checked_mul(height).ok_or(ImageError::Overflow)?;
    if length == 0 {
        return Ok(image);
    }
    let mut pixels = filled(length, [0., 0., 0., 1.])?;
    let mut height_map = None;
    let seed = filter.parameter(4)?.effective_value_at(time)? as i32;
    for layer in document
        .layers
        .iter()
        .rev()
        .filter(|layer| layer.header.container_visible())
    {
        crate::pointwise::check_render_cancelled(cancelled)?;
        let mut shapes = Vec::new();
        for shape in layer.shapes.iter().rev() {
            shape.append_outlines(&mut shapes, OutlineSampling::Scanline)?;
        }
        if shapes.is_empty() {
            continue;
        }
        let has_strokes = shapes.iter().any(|shape| shape.stroke.is_some());
        let needs_distance = |pass: &StylePass| {
            pass.area.as_ref().is_some_and(|area| {
                layer.symmetry != CanvasSymmetry::None
                    || pass.needs_prepared_area()
                    || pass.noise_mask.is_some()
                    || has_strokes
                    || !area.is_plain()
            })
        };
        let mask_height = if layer.style.passes.iter().any(needs_distance) {
            height.max(width)
        } else {
            height
        };
        let mask_length = width.checked_mul(mask_height).ok_or(ImageError::Overflow)?;
        let mut mask = filled(mask_length, false)?;
        let mut shape_mask = filled(mask_length, false)?;
        for shape in shapes.iter().filter(|shape| shape.stroke.is_none()) {
            rasterize(shape, &mut shape_mask, width, mask_height, cancelled)?;
            for (covered, &inside) in mask.iter_mut().zip(&shape_mask) {
                *covered = match shape.combine {
                    0 => *covered && !inside,
                    1 => *covered && inside,
                    2 => *covered != inside,
                    3..=7 => *covered || inside,
                    _ => {
                        return Err(PointwiseError::UnsupportedCapability(
                            "Canvas shape combination",
                        ));
                    }
                };
            }
        }
        let source = ((layer.input as usize) < active_inputs)
            .then(|| inputs.get(layer.input as usize).and_then(Option::as_ref))
            .flatten();
        let mut field = None;
        for pass in layer.style.passes.iter().rev() {
            if skip_pass(pass, source, &image, options) {
                continue;
            }
            let pass_has_field = needs_distance(pass);
            if pass_has_field && field.is_none() {
                let mut curves = Vec::new();
                for shape in layer.shapes.iter().rev() {
                    shape.append_outlines(&mut curves, OutlineSampling::Distance)?;
                }
                let mut distance = distance::render(&curves, &mask, width, cancelled)?;
                layer.symmetry.apply(&mut distance, width, cancelled)?;
                field = Some(distance);
            }
            StylePaintRender {
                geometry,
                area_geometry: ImageGeometry::new(geometry.width(), geometry.width()),
                scale: layer.scale,
                seed,
                execution,
                options,
            }
            .paint(
                pass,
                source,
                PassOutput {
                    image: &image,
                    pixels: &mut pixels,
                    height_map: &mut height_map,
                    original_background: None,
                },
                PassArea {
                    default_coverage: |index| f32::from(pass.area.is_none() || mask[index]),
                    field: field.as_deref().filter(|_| pass_has_field),
                },
                effects,
            )?;
        }
    }
    fill_image(&image, &pixels)?;
    if let Some(height_map) = height_map {
        height_map.attach(&image, execution, cancelled)?;
    }
    if options.is_canvas_tile(filter.node().id) {
        return Ok(image);
    }
    let size = filter.parameter(3)?;
    transform(
        image,
        [size.value_at(0, time)?, size.value_at(1, time)?],
        [0.; 2],
        execution,
        options,
        cancelled,
    )
}

pub(in crate::pointwise) fn render_style<E: 'static>(
    filter: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    geometry: ImageGeometry,
    time: f64,
    execution: ProceduralExecution,
    options: PointwiseRenderOptions,
    effects: &mut PointwiseRenderEffects<'_, E>,
) -> Result<SharedImageMap, PointwiseError<E>> {
    let style = filter.style().ok_or(PointwiseError::NullReference)?;
    let [design, background, texture] =
        std::array::from_fn(|index| inputs.get(index).and_then(Option::as_ref));
    let image = SharedImageMap::new(geometry);
    let width = usize::try_from(geometry.width()).map_err(|_| ImageError::Argument)?;
    let height = usize::try_from(geometry.height()).map_err(|_| ImageError::Argument)?;
    let length = width.checked_mul(height).ok_or(ImageError::Overflow)?;
    if length == 0 {
        return Ok(image);
    }
    let mut pixels = filled(length, [0., 0., 0., 1.])?;
    let mut height_map = None;
    if let Some(background) = background {
        if background.geometry() != geometry {
            return Err(PointwiseError::Argument);
        }
        let planes = read_planes(background)?;
        for (index, pixel) in pixels.iter_mut().enumerate() {
            *pixel = std::array::from_fn(|channel| planes[channel][index]);
        }
        image.copy_auxiliary(background)?;
    }
    let scale = filter.parameter(0)?.effective_value_at(time)? as f32;
    let seed = filter.parameter(1)?.effective_value_at(time)? as i32;
    let mut design_pixels = None;
    let mut original_background = background;
    for pass in style.passes.iter().rev() {
        crate::pointwise::check_render_cancelled(effects.cancel_check())?;
        if skip_pass(pass, texture, &image, options) {
            continue;
        }
        if pass.area.is_some() && design_pixels.is_none() {
            design_pixels = Some(match design {
                Some(input) if input.geometry() == geometry => input.materialize(0)?.values(),
                Some(_) => return Err(PointwiseError::Argument),
                None => filled(pixels.len(), 0.)?,
            });
        }
        StylePaintRender {
            geometry,
            area_geometry: geometry,
            scale,
            seed,
            execution,
            options,
        }
        .paint(
            pass,
            texture,
            PassOutput {
                image: &image,
                pixels: &mut pixels,
                height_map: &mut height_map,
                original_background: original_background.take(),
            },
            PassArea {
                default_coverage: |_| 1.,
                field: design_pixels.as_deref(),
            },
            effects,
        )?;
    }
    fill_image(&image, &pixels)?;
    if let Some(height_map) = height_map {
        height_map.attach(&image, execution, effects.cancel_check())?;
    }
    Ok(image)
}

fn has_named_paint(
    pass: &StylePass,
    input: Option<&SharedImageMap>,
    target: &SharedImageMap,
) -> bool {
    match pass.paint {
        Paint::Input { .. } => input.is_some_and(|image| image.all_channel_count() > 4),
        Paint::Copy { .. } | Paint::Blur { .. } => target.all_channel_count() > 4,
        _ => false,
    }
}

fn skip_pass(
    pass: &StylePass,
    source: Option<&SharedImageMap>,
    target: &SharedImageMap,
    options: PointwiseRenderOptions,
) -> bool {
    if options.is_color_only() && pass.blend.skip_for_color
        || matches!(pass.paint, Paint::Input { .. }) && source.is_none()
    {
        return true;
    }
    pass.blend.opacity as f32 / 100. == 0.
        && pass.blend.mode != 0
        && !pass.blend.has_spatial_shading()
        && !matches!(pass.paint, Paint::Refract(_))
        && !(options.is_channel_rendering() && has_named_paint(pass, source, target))
}

pub(super) fn fill_image(image: &SharedImageMap, pixels: &[[f32; 4]]) -> Result<(), ImageError> {
    if pixels.is_empty() {
        return Ok(());
    }
    for channel in 0..4 {
        image.set_buffer(
            channel as i32,
            Some(&FloatBuffer::new(plane(pixels, channel)?)),
        )?;
        image.seal_channel(channel as i32)?;
    }
    Ok(())
}

fn plane(pixels: &[[f32; 4]], channel: usize) -> Result<Vec<f32>, ImageError> {
    let mut values = filled(pixels.len(), 0.)?;
    for (value, pixel) in values.iter_mut().zip(pixels) {
        *value = pixel[channel];
    }
    Ok(values)
}

pub(super) fn plane_image(
    pixels: &[[f32; 4]],
    geometry: ImageGeometry,
    channel: usize,
) -> Result<SharedImageMap, ImageError> {
    area::as_image(plane(pixels, channel)?, geometry)
}

struct StylePaintRender {
    geometry: ImageGeometry,
    area_geometry: ImageGeometry,
    scale: f32,
    seed: i32,
    execution: ProceduralExecution,
    options: PointwiseRenderOptions,
}

struct PassOutput<'a> {
    image: &'a SharedImageMap,
    pixels: &'a mut [[f32; 4]],
    height_map: &'a mut Option<StyleHeightMap>,
    original_background: Option<&'a SharedImageMap>,
}

struct PassArea<'a, C> {
    default_coverage: C,
    field: Option<&'a [f32]>,
}

impl StylePaintRender {
    fn paint<E: 'static>(
        &self,
        pass: &StylePass,
        source: Option<&SharedImageMap>,
        PassOutput {
            image,
            pixels,
            height_map,
            original_background,
        }: PassOutput<'_>,
        PassArea {
            default_coverage,
            field,
        }: PassArea<'_, impl Fn(usize) -> f32>,
        effects: &mut PointwiseRenderEffects<'_, E>,
    ) -> Result<(), PointwiseError<E>> {
        let blend = StyleBlend::new(&pass.blend)?;
        let opacity = pass.blend.opacity as f32 / 100.;
        let cancelled = effects.cancel_check();
        let scale = style_scale(self.scale);
        let mask_noise = pass.noise_mask.as_ref().filter(|noise| noise.strength > 0.);
        let mask_noise_samples = mask_noise
            .map(|noise| self.noise_samples(&noise.lab, source, self.area_geometry, effects))
            .transpose()?;
        let rendered_area = pass
            .area
            .as_ref()
            .zip(field)
            .map(|(area, field)| {
                let displacement = mask_noise
                    .zip(mask_noise_samples.as_deref())
                    .filter(|(noise, _)| !noise.affects_opacity)
                    .map(|(noise, samples)| (samples, noise.strength));
                area::render(
                    area,
                    field,
                    self.area_geometry,
                    self.scale,
                    self.execution,
                    effects,
                    area::AreaEffects {
                        keep_prepared: pass.needs_prepared_area(),
                        displacement,
                    },
                )
            })
            .transpose()?;
        let coverage_at = |index| {
            rendered_area.as_ref().map_or_else(
                || default_coverage(index),
                |area| area.coverage[index % area.coverage.len()],
            )
        };
        let prepared = rendered_area
            .as_ref()
            .and_then(|area| area.prepared.as_deref());
        let gradient_mask = pass
            .gradient_mask
            .as_ref()
            .map(|field| field.render(prepared, self.area_geometry, scale, self.execution, effects))
            .transpose()?;
        let opacity_noise = mask_noise
            .zip(mask_noise_samples.as_deref())
            .filter(|(noise, _)| noise.affects_opacity)
            .map(|(noise, samples)| (noise.strength as f32 / 100., samples));
        let spatial = pass.blend.has_spatial_shading();
        let mut staged = if spatial {
            Some(filled(pixels.len(), [0., 0., 0., 1.])?)
        } else {
            None
        };
        let apply_masks = |index, mut coverage: f32| {
            if let Some(mask) = &gradient_mask {
                let factor = mask[index % mask.len()];
                coverage *= factor;
            }
            if let Some((strength, values)) = opacity_noise {
                let factor = values[index % values.len()] * strength + (1. - strength);
                coverage *= factor;
            }
            coverage
        };
        let track_named =
            self.options.is_channel_rendering() && has_named_paint(pass, source, image);
        let mut named_transparency = track_named.then(|| filled(pixels.len(), 1.)).transpose()?;
        let mut named_paint = None;
        let mut composite =
            |index, pixel: &mut [f32; 4], color: [f32; 3], alpha: f32, intrinsic: f32| {
                let area = coverage_at(index);
                let coverage = apply_masks(index, area * alpha);
                let replacement_coverage = apply_masks(index, area);
                if let Some(transparency) = &mut named_transparency {
                    transparency[index] = 1. - coverage;
                }
                if let Some(staged) = &mut staged {
                    staged[index] = [color[0], color[1], color[2], 1. - coverage];
                } else if pass.blend.opacity != 0. || blend.replaces_transparency() {
                    blend.composite(pixel, color, coverage, replacement_coverage, intrinsic);
                }
            };
        match pass.paint {
            Paint::Environment(ref paint) => {
                let reflected = environment::reflect(
                    paint,
                    environment::BevelInputs {
                        field: prepared,
                        noise: paint
                            .bevel
                            .noise
                            .as_ref()
                            .filter(|noise| noise.strength != 0.)
                            .map(|noise| {
                                self.noise_samples(&noise.lab, source, self.geometry, effects)
                            })
                            .transpose()?,
                    },
                    self.geometry,
                    style_scale(self.scale),
                    self.execution,
                    self.options,
                    effects,
                )?;
                if let Some((height, depth)) = reflected.normal_source {
                    blend_height(
                        height_map,
                        pixels.len(),
                        &height,
                        depth,
                        coverage_at,
                        opacity,
                        cancelled,
                    )?;
                }
                if reflected.has_transparency {
                    paint_image(&reflected.image, pixels, cancelled, &mut composite)?;
                } else {
                    paint_image(
                        &reflected.image,
                        pixels,
                        cancelled,
                        |index, pixel, rgb, _, _| {
                            composite(index, pixel, rgb, 1., 0.);
                        },
                    )?;
                }
            }
            Paint::Adjust(ref settings) => {
                let adjust = AdjustRgb::from(settings);
                for (row_index, row) in pixels
                    .chunks_exact_mut(self.geometry.width() as usize)
                    .enumerate()
                {
                    crate::pointwise::check_render_cancelled(cancelled)?;
                    for (column, pixel) in row.iter_mut().enumerate() {
                        let index = row_index * self.geometry.width() as usize + column;
                        let [r, g, b, intrinsic] = *pixel;
                        composite(
                            index,
                            pixel,
                            adjust.apply([r, g, b]),
                            1. - intrinsic,
                            intrinsic,
                        );
                    }
                }
            }
            Paint::Refract(ref paint) => {
                let height = environment::prepare_bevel(
                    &paint.bevel,
                    environment::BevelInputs {
                        field: prepared,
                        noise: paint
                            .bevel
                            .noise
                            .as_ref()
                            .filter(|noise| noise.strength != 0.)
                            .map(|noise| {
                                self.noise_samples(&noise.lab, source, self.geometry, effects)
                            })
                            .transpose()?,
                    },
                    self.geometry,
                    scale,
                    true,
                    self.execution,
                    effects,
                )?;
                let height_values = height.channel(0, true)?;
                if height_values.len() == 1 {
                    // The original returns the accumulated image itself and
                    // the subsequent paint masks operate on its alpha plane.
                    for (index, pixel) in pixels.iter_mut().enumerate() {
                        let [r, g, b, alpha] = *pixel;
                        pixel[3] = 1. - apply_masks(index, alpha);
                        composite(index, pixel, [r, g, b], alpha, 0.);
                    }
                } else {
                    let depth = f64::from((paint.depth / (100. * f64::from(scale))) as f32);
                    let backdrop = SharedImageMap::new(self.geometry);
                    fill_image(&backdrop, pixels)?;
                    let refracted = planar_optics::refract(
                        &backdrop,
                        &height_values,
                        depth,
                        paint.index,
                        cancelled,
                    )?;
                    if paint.affects_height && self.options.is_channel_rendering() {
                        blend_height(
                            height_map,
                            pixels.len(),
                            &height,
                            depth,
                            coverage_at,
                            opacity,
                            cancelled,
                        )?;
                    }
                    paint_image(&refracted, pixels, cancelled, &mut composite)?;
                }
            }
            Paint::Blur { radius } => {
                let backdrop = if let Some(background) = original_background {
                    background.clone()
                } else {
                    let backdrop = SharedImageMap::new(self.geometry);
                    fill_image(&backdrop, pixels)?;
                    backdrop
                };
                let settings = blur::AdaptiveSettings {
                    horizontal: true,
                    vertical: true,
                    radius: f64::from((radius / 100.) as f32),
                    factor: f64::from(scale),
                    periodic: true,
                    all_channels: false,
                    force_expansion: false,
                };
                let blurred = if radius == 0. {
                    backdrop.clone()
                } else {
                    blur::adaptive(&backdrop, self.geometry, settings, self.execution, effects)?
                };
                if track_named {
                    named_paint = Some(if radius == 0. {
                        image.clone()
                    } else {
                        named::blurred(
                            image,
                            settings,
                            blurred.same_reference(&backdrop),
                            self.execution,
                            effects,
                        )?
                    });
                }
                paint_image(&blurred, pixels, cancelled, &mut composite)?;
            }
            Paint::Gradient(ref paint) => {
                let mut values = if let Some(field) = &paint.field {
                    field.render(prepared, self.area_geometry, scale, self.execution, effects)?
                } else if let Some(prepared) = prepared {
                    prepared.to_vec()
                } else {
                    filled(self.area_geometry.area() as usize, 1.)?
                };
                if let Some(noise) = paint.noise.as_ref().filter(|noise| noise.strength != 0.) {
                    let field_noise =
                        self.noise_samples(&noise.lab, source, self.area_geometry, effects)?;
                    let strength = noise.strength as f32 / 100.;
                    for (value, noise) in values.iter_mut().zip(field_noise) {
                        *value = *value * (1. - strength) + noise * strength;
                    }
                }
                paint_gradient(
                    &paint.gradient,
                    &values,
                    pixels,
                    self.geometry,
                    cancelled,
                    &mut composite,
                )?;
            }
            Paint::Synthesis(ref paint) => {
                let synthesis = paint.texture().render_style(
                    source,
                    self.geometry,
                    self.scale,
                    self.seed,
                    self.execution,
                    effects,
                )?;
                paint_image(&synthesis, pixels, cancelled, &mut composite)?;
            }
            Paint::Noise(ref paint) => {
                let values = self.noise_samples(&paint.lab, source, self.area_geometry, effects)?;
                paint_gradient(
                    &paint.gradient,
                    &values,
                    pixels,
                    self.geometry,
                    cancelled,
                    &mut composite,
                )?;
            }
            Paint::Solid {
                argb: [_, r, g, b],
                transparency,
            } => {
                let color = [r as f32, g as f32, b as f32];
                let alpha = (1. - transparency / 100.) as f32;
                for (index, pixel) in pixels.iter_mut().enumerate() {
                    composite(index, pixel, color, alpha, 1. - alpha);
                }
            }
            Paint::Input { offset, rotation } => {
                if let Some(source) = source {
                    if source.geometry() != self.geometry {
                        return Err(PointwiseError::Argument);
                    }
                    let source = self.rotate_input(source, rotation, cancelled)?;
                    let source = transform(
                        source,
                        [1.; 2],
                        input_offset(self.scale, offset),
                        self.execution,
                        self.options,
                        cancelled,
                    )?;
                    paint_image(&source, pixels, cancelled, &mut composite)?;
                    named_paint = track_named.then_some(source);
                }
            }
            Paint::Copy { offset, .. } => {
                if offset != [0.; 2] {
                    let source = if track_named {
                        image.shallow_copy()?
                    } else {
                        SharedImageMap::new(self.geometry)
                    };
                    fill_image(&source, pixels)?;
                    let percent = 100. * f64::from(style_scale(self.scale));
                    let shifted = transform(
                        source,
                        [1.; 2],
                        offset.map(|value| -value / percent),
                        self.execution,
                        self.options,
                        cancelled,
                    )?;
                    paint_image(&shifted, pixels, cancelled, &mut composite)?;
                    named_paint = track_named.then_some(shifted);
                } else {
                    named_paint = track_named.then(|| image.clone());
                    for (index, pixel) in pixels.iter_mut().enumerate() {
                        let coverage = coverage_at(index);
                        if coverage != 0. || blend.replaces_transparency() || spatial {
                            composite(
                                index,
                                pixel,
                                [pixel[0], pixel[1], pixel[2]],
                                1. - pixel[3],
                                pixel[3],
                            );
                        }
                    }
                }
            }
        }
        let grid = BlendGrid {
            geometry: self.geometry,
            execution: self.execution,
        };
        if let Some(mut staged) = staged {
            grid.composite(&blend, &pass.blend, pixels, &mut staged, effects)?;
        }
        if let (Some(paint), Some(transparency)) = (named_paint, named_transparency) {
            named::composite(&grid, image, &paint, &transparency, &pass.blend, effects)?;
        }
        Ok(())
    }

    fn noise_samples<E: 'static>(
        &self,
        record: &[u8],
        source: Option<&SharedImageMap>,
        geometry: ImageGeometry,
        effects: &mut PointwiseRenderEffects<'_, E>,
    ) -> Result<Vec<f32>, PointwiseError<E>> {
        let mut values = noise::render(
            record,
            source,
            geometry.width(),
            self.scale,
            self.seed,
            self.execution,
            effects,
        )?;
        let len = geometry.area() as usize;
        let tile_len = values.len();
        if tile_len == 0 {
            return Err(PointwiseError::Index);
        }
        if tile_len < len {
            values
                .try_reserve_exact(len - tile_len)
                .map_err(|_| ImageError::Allocation)?;
            // The embedded Lab renders one square tile at the requested width.
            while values.len() < len {
                values.extend_from_within(..tile_len.min(len - values.len()));
            }
        }
        values.truncate(len);
        Ok(values)
    }

    fn rotate_input<E: 'static>(
        &self,
        input: &SharedImageMap,
        rotation: i32,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        let quarter_turns = match rotation {
            0 | 7 => 0,
            1 | 2 => 1,
            3 | 4 => 2,
            5 | 6 => 3,
            _ => return Err(PointwiseError::Argument),
        };
        let rotated = crate::pointwise::render_rotate(
            Some(input),
            self.geometry,
            quarter_turns,
            self.execution,
        )?;
        if rotation % 2 == 0 {
            return rotated.ok_or(PointwiseError::NullReference);
        }
        let tiled = crate::pointwise::transform_tile::tile(rotated, 1, self.execution)?
            .ok_or(PointwiseError::NullReference)?;
        transform(
            tiled,
            [1.; 2],
            [0.5, 0.],
            self.execution,
            self.options,
            cancelled,
        )
    }
}

fn blend_height<E>(
    target: &mut Option<StyleHeightMap>,
    length: usize,
    height: &SharedImageMap,
    depth: f64,
    coverage: impl Fn(usize) -> f32,
    opacity: f32,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), PointwiseError<E>> {
    if target.is_none() {
        *target = Some(StyleHeightMap::new(length)?);
    }
    target
        .as_mut()
        .expect("height map initialized")
        .blend(height, depth, coverage, opacity, cancelled)
}

fn paint_image<E>(
    image: &SharedImageMap,
    pixels: &mut [[f32; 4]],
    cancelled: &dyn Fn() -> bool,
    mut composite: impl FnMut(usize, &mut [f32; 4], [f32; 3], f32, f32),
) -> Result<(), PointwiseError<E>> {
    let [r, g, b, a] = read_planes(image)?;
    let width = image.geometry().width() as usize;
    for (row_index, row) in pixels.chunks_exact_mut(width).enumerate() {
        crate::pointwise::check_render_cancelled(cancelled)?;
        for (column, pixel) in row.iter_mut().enumerate() {
            let index = row_index * width + column;
            composite(
                index,
                pixel,
                [r[index], g[index], b[index]],
                1. - a[index],
                a[index],
            );
        }
    }
    Ok(())
}

fn paint_gradient<E>(
    record: &GradientRecord,
    values: &[f32],
    pixels: &mut [[f32; 4]],
    geometry: ImageGeometry,
    cancelled: &dyn Fn() -> bool,
    mut composite: impl FnMut(usize, &mut [f32; 4], [f32; 3], f32, f32),
) -> Result<(), PointwiseError<E>> {
    let gradient = SharedGradient::from_wire_record(record.clone());
    gradient.rebuild()?;
    let transparent = gradient.has_transparency()?;
    let width = geometry.width() as usize;
    for (row_index, row) in pixels.chunks_exact_mut(width).enumerate() {
        crate::pointwise::check_render_cancelled(cancelled)?;
        for (column, pixel) in row.iter_mut().enumerate() {
            let index = row_index * width + column;
            let [a, r, g, b] = gradient
                .sample(f64::from(values[index % values.len()]))?
                .argb();
            let alpha = if transparent { a as f32 } else { 1. };
            composite(
                index,
                pixel,
                [r as f32, g as f32, b as f32],
                alpha,
                1. - alpha,
            );
        }
    }
    Ok(())
}

fn input_offset(scale: f32, offset: [f64; 2]) -> [f64; 2] {
    offset.map(|value| -value / f64::from(100. * style_scale(scale)))
}

pub(super) fn style_scale(scale: f32) -> f32 {
    if scale < 0. {
        -1. / (scale - 1.)
    } else {
        scale + 1.
    }
}

pub(super) fn filled<T: Clone>(length: usize, value: T) -> Result<Vec<T>, ImageError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(length)
        .map_err(|_| ImageError::Allocation)?;
    values.resize(length, value);
    Ok(values)
}

pub(super) fn read_planes(map: &SharedImageMap) -> Result<[Vec<f32>; 4], ImageError> {
    let length = usize::try_from(map.geometry().area()).map_err(|_| ImageError::Overflow)?;
    let mut planes = std::array::from_fn(|_| Vec::new());
    for (channel, values) in planes.iter_mut().enumerate() {
        let samples = map.channel(channel as i32, true)?;
        *values = if samples.len() == 1 {
            filled(length, samples.read(0)?)?
        } else if samples.len() == length {
            samples.values()
        } else {
            return Err(ImageError::Argument);
        };
    }
    Ok(planes)
}

fn rasterize<E>(
    outline: &Outline,
    mask: &mut [bool],
    width: usize,
    height: usize,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), PointwiseError<E>> {
    let mut min_y = f64::INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for &[x, y] in outline.contours.iter().flatten() {
        if !x.is_finite() || !y.is_finite() {
            return Err(PointwiseError::Argument);
        }
        min_y = min_y.min(y);
        max_y = max_y.max(y);
    }
    // The original scans an atlas sized by the output width. A wide texture
    // consumes the first rows of that atlas.
    let grid = f64::from(100. / width as f32);
    let mut crossings = Vec::new();
    mask.fill(false);
    if max_y - min_y > 102400. {
        return Err(PointwiseError::UnsupportedCapability(
            "Canvas shape tile span",
        ));
    }
    for row in 0..height {
        crate::pointwise::check_render_cancelled(cancelled)?;
        let line = &mut mask[row * width..(row + 1) * width];
        let center = (0.5 * grid + row as f64 * grid).rem_euclid(100.);
        let first_tile = ((min_y - center) / 100.).ceil() as i32;
        let last_tile = ((max_y - center) / 100.).floor() as i32;
        for tile in first_tile..=last_tile {
            let y = center + f64::from(tile) * 100.;
            crossings.clear();
            for contour in &outline.contours {
                let edges = contour.iter().zip(contour.iter().cycle().skip(1));
                for (&[x0, y0], &[x1, y1]) in edges {
                    if (y0 < y && y <= y1) || (y1 < y && y <= y0) {
                        crossings.push(x0 + (y - y0) * (x1 - x0) / (y1 - y0));
                    }
                }
            }
            crossings.sort_by(f64::total_cmp);
            for pair in crossings.chunks_exact(2) {
                let length = pair[1] - pair[0];
                if length >= 100. {
                    line.fill(true);
                    break;
                }
                let start = pair[0].rem_euclid(100.);
                let end = start + length;
                let column = |value: f64| ((value / grid + 0.5).floor() as usize).min(width);
                line[column(start)..column(end)].fill(true);
                if end > 100. {
                    line[..column(end - 100.)].fill(true);
                }
            }
        }
    }
    Ok(())
}

pub(super) fn transform<E: 'static>(
    mut image: SharedImageMap,
    scale: [f64; 2],
    offset: [f64; 2],
    execution: ProceduralExecution,
    options: PointwiseRenderOptions,
    cancelled: &dyn Fn() -> bool,
) -> Result<SharedImageMap, PointwiseError<E>> {
    use crate::pointwise::{blur, transform};
    if scale == [1.; 2] && offset == [0.; 2] {
        return Ok(image);
    }
    let w = image.geometry().width();
    let h = image.geometry().height();
    if w <= 0
        || h <= 0
        || scale
            .into_iter()
            .chain(offset)
            .any(|value| !value.is_finite())
    {
        return Err(PointwiseError::Argument);
    }
    let [sx, sy] = scale;
    if sx.abs() > 1. || sy.abs() > 1. {
        image = blur::affine_preblur(
            &image,
            sx.abs() / 2. / f64::from(w) / 1.3,
            sy.abs() / 2. / f64::from(h) / 1.3,
            execution,
        )?;
    }
    let [mut x, mut y] = std::array::from_fn(|axis| {
        let value = if scale[axis] < 0. {
            -offset[axis]
        } else {
            offset[axis]
        };
        value.rem_euclid(1.)
    });
    x *= f64::from(w);
    y = (y + sy - 1.) * f64::from(h);
    if sx.abs() == 1. {
        x = x.round_ties_even();
    }
    if sy.abs() == 1. {
        y = y.round_ties_even();
    }
    if sx < 0. {
        x -= 1.;
    }
    if sy < 0. {
        y += 1.;
    }
    let output = SharedImageMap::new(image.geometry());
    for channel in 0..transform::count(&image, options.is_channel_rendering(), true)? {
        let source = image.channel_with_effects(channel, true)?;
        if source.len() == 1 {
            output.copy_indexed_constant(channel, source.read(0)?, &image)?;
            continue;
        }
        let target = output.allocate_indexed(channel, &image)?;
        let mut row = filled(w as usize, 0.)?;
        for j in 0..h {
            crate::pointwise::check_render_cancelled(cancelled)?;
            for (i, value) in row.iter_mut().enumerate() {
                *value = transform::sample(
                    i as f32 * sx as f32 + x as f32,
                    j as f32 * sy as f32 - y as f32,
                    &source,
                    w,
                    h,
                )?;
            }
            target.write_block(j as usize * w as usize, 1, &row)?;
        }
        output.seal_channel(channel)?;
    }
    Ok(output)
}
