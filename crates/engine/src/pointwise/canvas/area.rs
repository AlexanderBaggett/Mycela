//! Turn the layer's encoded signed distance into a style pass's coverage.
use super::{raster, style::ShapeArea};
use crate::pointwise::{
    PointwiseRenderEffects, PointwiseRenderOptions, SharedImageMap, blur, tone_curve,
};
use crate::{
    BinaryCursor, BinaryWriter, CurveRecord, FloatBuffer, ImageError, ImageGeometry,
    PointwiseError, ProceduralExecution, SharedCurve,
};

#[derive(Default)]
pub(super) struct AreaEffects<'a> {
    pub(super) keep_prepared: bool,
    pub(super) displacement: Option<(&'a [f32], f64)>,
}

pub(super) struct RenderedArea {
    pub(super) coverage: Vec<f32>,
    pub(super) prepared: Option<Vec<f32>>,
}

pub(super) fn render<E: 'static>(
    area: &ShapeArea,
    field: &[f32],
    geometry: ImageGeometry,
    layer_scale: f32,
    execution: ProceduralExecution,
    effects: &mut PointwiseRenderEffects<'_, E>,
    area_effects: AreaEffects<'_>,
) -> Result<RenderedArea, PointwiseError<E>> {
    let scale = raster::style_scale(layer_scale);
    let percent = 100. * scale;
    let thickness = area.thickness as f32 / percent;
    let outline = area.outline_thickness as f32 / percent;
    let mut mask = Vec::new();
    mask.try_reserve_exact(field.len())
        .map_err(|_| ImageError::Allocation)?;
    let cancelled = effects.cancel_check();
    let width = geometry.width() as usize;
    for (row_index, row) in field.chunks_exact(width).enumerate() {
        crate::pointwise::check_render_cancelled(cancelled)?;
        mask.extend(row.iter().enumerate().map(|(column, value)| {
            let shifted = (value + thickness).clamp(0., 1.);
            let shifted = if area.outline {
                (shifted.min(1. - shifted) + outline).clamp(0., 1.)
            } else {
                shifted
            };
            if let Some((noise, strength)) = area_effects.displacement {
                let strength = strength as f32 / percent;
                shifted + noise[row_index * width + column] * strength - strength / 2.
            } else {
                shifted
            }
        }));
    }
    let prepared = translate(mask, geometry, area.offset, percent, execution, cancelled)?;
    let bevel_source = area_effects.keep_prepared.then(|| prepared.clone());
    let shadow_source = area.secondary.as_ref().map(|_| prepared.clone());
    let mut primary = feather(
        prepared,
        geometry,
        [area.hard_feather, area.soft_feather],
        scale,
        &area.feather_curve,
        execution,
        effects,
    )?;
    if let Some((shadow, mut source)) = area.secondary.as_ref().zip(shadow_source) {
        if !matches!(shadow.mode, 1 | 2) {
            return Err(PointwiseError::UnsupportedCapability(
                "Canvas area shadow mode",
            ));
        }
        let inner = shadow.mode == 2;
        if inner {
            for value in &mut source {
                *value = 1. - *value;
            }
        }
        let shadow_mask = feather(
            source,
            geometry,
            [shadow.hard_feather, shadow.soft_feather],
            scale,
            &shadow.curve,
            execution,
            effects,
        )?;
        let shifted = translate(
            shadow_mask,
            geometry,
            shadow.offset,
            percent,
            execution,
            cancelled,
        )?;
        for (primary, shadow) in primary.iter_mut().zip(shifted) {
            *primary = shadow * if inner { *primary } else { 1. - *primary };
        }
    }
    Ok(RenderedArea {
        coverage: primary,
        prepared: bevel_source,
    })
}

pub(super) fn as_image(
    values: Vec<f32>,
    geometry: ImageGeometry,
) -> Result<SharedImageMap, ImageError> {
    let image = SharedImageMap::new(geometry);
    image.set_buffer(0, Some(&FloatBuffer::new(values)))?;
    image.seal_channel(0)?;
    Ok(image)
}

pub(super) fn translate<E: 'static>(
    values: Vec<f32>,
    geometry: ImageGeometry,
    offset: [f64; 2],
    percent: f32,
    execution: ProceduralExecution,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<f32>, PointwiseError<E>> {
    if offset == [0.; 2] {
        return Ok(values);
    }
    let shifted = raster::transform(
        as_image(values, geometry)?,
        [1.; 2],
        offset.map(|value| -value / f64::from(percent)),
        execution,
        PointwiseRenderOptions::new(false, false),
        cancelled,
    )?;
    Ok(shifted.materialize(0)?.values())
}

pub(super) fn feather<E: 'static>(
    mut values: Vec<f32>,
    geometry: ImageGeometry,
    [hard, soft]: [f64; 2],
    scale: f32,
    curve: &CurveRecord,
    execution: ProceduralExecution,
    effects: &mut PointwiseRenderEffects<'_, E>,
) -> Result<Vec<f32>, PointwiseError<E>> {
    let cancelled = effects.cancel_check();
    let edge = (1. - hard / (100. * f64::from(scale))) as f32 / 2.;
    let slope = 1. / (1. - 2. * edge);
    for row in values.chunks_exact_mut(geometry.width() as usize) {
        crate::pointwise::check_render_cancelled(cancelled)?;
        for value in row {
            *value = if hard == 0. {
                f32::from(*value >= 0.5)
            } else {
                ((*value - edge) * slope).clamp(0., 1.)
            };
        }
    }
    let mut values = soft_blur(values, geometry, soft, scale, execution, effects)?;
    remap(&mut values, geometry, curve, cancelled)?;
    Ok(values)
}

pub(super) fn soft_blur<E: 'static>(
    values: Vec<f32>,
    geometry: ImageGeometry,
    soft: f64,
    scale: f32,
    execution: ProceduralExecution,
    effects: &mut PointwiseRenderEffects<'_, E>,
) -> Result<Vec<f32>, PointwiseError<E>> {
    if soft == 0. {
        return Ok(values);
    }
    let image = as_image(values, geometry)?;
    let blurred = blur::adaptive(
        &image,
        geometry,
        blur::AdaptiveSettings {
            horizontal: true,
            vertical: true,
            radius: soft / 100.,
            factor: f64::from(scale),
            periodic: true,
            all_channels: false,
            force_expansion: false,
        },
        execution,
        effects,
    )?;
    Ok(blurred.materialize(0)?.values())
}

pub(super) fn remap<E: 'static>(
    values: &mut [f32],
    geometry: ImageGeometry,
    curve: &CurveRecord,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), PointwiseError<E>> {
    if !curve.layers.is_empty() {
        let mut writer = BinaryWriter::default();
        curve
            .save(&mut writer)
            .map_err(|_| PointwiseError::InvalidOperation)?;
        let curve = SharedCurve::<E>::new(0., 1., false);
        let mut reader = BinaryCursor::new(writer.bytes()).map_err(|_| ImageError::Overflow)?;
        curve.load(&mut reader, 3.75)?;
        let table = tone_curve::samples(&curve)?.values();
        let last = table.len() - 1;
        for row in values.chunks_exact_mut(geometry.width() as usize) {
            crate::pointwise::check_render_cancelled(cancelled)?;
            for value in row {
                *value = if *value >= 1. {
                    table[last]
                } else if *value <= 0. {
                    table[0]
                } else {
                    let position = *value * last as f32;
                    let index = position as usize;
                    let lower = table[index];
                    lower + (position - index as f32) * (table[index + 1] - lower)
                };
            }
        }
    }
    Ok(())
}
