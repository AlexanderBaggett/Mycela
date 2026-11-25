//! The Style Environment pass uses its prepared area as a reflection height field.
use super::{
    area, raster,
    style::{Bevel, EnvironmentPaint},
};
use crate::pointwise::{
    PointwiseRenderEffects, PointwiseRenderOptions, SharedImageMap, environment_optics,
};
use crate::{ImageError, ImageGeometry, PointwiseError, ProceduralExecution};

pub(super) struct Reflection {
    pub(super) image: SharedImageMap,
    pub(super) has_transparency: bool,
    pub(super) normal_source: Option<(SharedImageMap, f64)>,
}

pub(super) struct BevelInputs<'a> {
    pub(super) field: Option<&'a [f32]>,
    pub(super) noise: Option<Vec<f32>>,
}

pub(super) fn reflect<E: 'static>(
    paint: &EnvironmentPaint,
    BevelInputs { field, noise }: BevelInputs<'_>,
    geometry: ImageGeometry,
    scale: f32,
    execution: ProceduralExecution,
    options: PointwiseRenderOptions,
    effects: &mut PointwiseRenderEffects<'_, E>,
) -> Result<Reflection, PointwiseError<E>> {
    let surface_depth = f64::from((paint.depth / (100. * f64::from(scale))) as f32);
    let reflect_depth = if options.is_color_only() && paint.flatten_color {
        0.
    } else {
        surface_depth
    };
    let affects_height = options.is_channel_rendering() && paint.affects_height;
    let width = geometry.width();
    let limit = match paint.resolution {
        // Genetica Studio's automatic environment limit.
        -1 => 46000,
        100 | 200 | 400 | 800 | 1600 | 3200 => paint.resolution,
        _ => return Err(PointwiseError::Argument),
    };
    let width_upsampled = (1.6 * f64::from(width)) as i32;
    let even = width_upsampled + width_upsampled % 2;
    let resolution = (if even > limit { width } else { even }).min(limit);
    let resolution = if reflect_depth == 0. {
        resolution.min(300)
    } else {
        resolution
    };
    let cancelled = effects.cancel_check();
    let map = paint.map.render_latlong(
        ImageGeometry::new(resolution.max(2), (resolution / 2).max(1)),
        execution,
        effects.environment_library(),
        cancelled,
    )?;
    let height = prepare_bevel(
        &paint.bevel,
        BevelInputs {
            field: field.filter(|_| surface_depth != 0. && affects_height || reflect_depth != 0.),
            noise,
        },
        geometry,
        scale,
        false,
        execution,
        effects,
    )?;
    let image = environment_optics::reflect_sample(&map, &height, reflect_depth, None, cancelled)?;
    Ok(Reflection {
        image,
        has_transparency: map.has_transparency()?,
        normal_source: affects_height.then_some((height, surface_depth)),
    })
}

pub(super) fn prepare_bevel<E: 'static>(
    bevel: &Bevel,
    BevelInputs { field, noise }: BevelInputs<'_>,
    geometry: ImageGeometry,
    scale: f32,
    keep_dense: bool,
    execution: ProceduralExecution,
    effects: &mut PointwiseRenderEffects<'_, E>,
) -> Result<SharedImageMap, PointwiseError<E>> {
    let [hard, soft] = bevel.feather;
    let uses_bevel = hard >= 0.0001 || soft >= 0.0001;
    let cancelled = effects.cancel_check();
    let length = usize::try_from(geometry.area()).map_err(|_| ImageError::Overflow)?;
    let mut height = field
        .filter(|_| uses_bevel)
        .map(|field| -> Result<Vec<f32>, PointwiseError<E>> {
            // Canvas computes its shape field in a square tile before painting.
            let mut repeated = raster::filled(length, 0.)?;
            for row in repeated.chunks_mut(field.len()) {
                row.copy_from_slice(&field[..row.len()]);
            }
            area::feather(
                repeated,
                geometry,
                bevel.feather,
                scale,
                &bevel.curve,
                execution,
                effects,
            )
        })
        .transpose()?;
    if let Some((parameters, noise)) = bevel.noise.as_ref().zip(noise) {
        if height.is_none() {
            let mut flat = raster::filled(length, 1.)?;
            area::remap(&mut flat, geometry, &bevel.curve, cancelled)?;
            height = Some(flat);
        }
        let strength = (parameters.strength as f32 / 100.).min(1.);
        for (value, noise) in height
            .as_mut()
            .expect("bevel initialized")
            .iter_mut()
            .zip(noise)
        {
            *value = *value * (1. - strength) + noise * strength;
        }
    }
    if let Some(height) = height {
        Ok(area::as_image(height, geometry)?)
    } else if keep_dense && field.is_some() {
        let mut flat = raster::filled(length, 1.)?;
        area::remap(&mut flat, geometry, &bevel.curve, cancelled)?;
        Ok(area::as_image(flat, geometry)?)
    } else {
        let height = SharedImageMap::new(geometry);
        height.set_constant(0, 1.)?;
        Ok(height)
    }
}
