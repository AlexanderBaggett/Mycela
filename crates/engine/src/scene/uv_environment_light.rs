//! Environment color follows the averaged surface normal. Geometry coverage
//! is independent of the sampled transparency and drives island padding.
use super::uv_apply::{BACKGROUND, TexturePlane, allocated, pad_coverage, unorm};
use super::uv_mesh::{area, read_meshes};
use super::{MeshModel, MeshVector};
use crate::pointwise::check_render_cancelled;
use crate::{FloatBuffer, ImageGeometry, PointwiseError, SharedImageMap};
use std::f32::consts::PI;

fn environment_uv(mut normal: MeshVector) -> [f64; 2] {
    normal.normalize();
    let [x, y, z] = normal.values();
    // The reference OpenGL profile resolves its undefined atan(0, 0) to
    // this longitude. Keeping it explicit makes polar samples portable.
    let mut u = if x == 0. && z == 0. {
        0.375
    } else {
        z.atan2(x) * (0.5 / PI)
    };
    if u <= 0. {
        u += 1.;
    }
    let v = 0.5 - y.clamp(-1., 1.).asin() / PI;
    [f64::from(u), f64::from(v)]
}

pub(crate) fn material_geometry<E>(
    request: ImageGeometry,
    limit: u32,
) -> Result<ImageGeometry, PointwiseError<E>> {
    let [width, height] = [request.width(), request.height()];
    if width <= 0 || height <= 0 || limit == 0 {
        return Err(PointwiseError::Argument);
    }
    let largest = i64::from(width.max(height));
    if largest <= i64::from(limit) {
        return Ok(request);
    }
    let fit = |axis| (i64::from(axis) * i64::from(limit) / largest).max(1) as i32;
    Ok(ImageGeometry::new(fit(width), fit(height)))
}

pub(crate) fn bake<E>(
    models: &[MeshModel],
    texture: Option<&SharedImageMap>,
    geometry: ImageGeometry,
    cancelled: &dyn Fn() -> bool,
) -> Result<SharedImageMap, PointwiseError<E>> {
    let image = SharedImageMap::new(geometry);
    image.set_constant(3, 1.)?;
    if models.is_empty() {
        return Ok(image);
    }
    let count = area(geometry)?;
    let mut coverage = allocated(count, BACKGROUND)?;
    let mut coordinates = allocated(count, [0.; 2])?;
    for mesh in read_meshes(models, cancelled)? {
        mesh.raster_surfaces(geometry, cancelled, |index, surface| {
            if coverage[index] == BACKGROUND {
                coordinates[index] = environment_uv(surface.normal);
                coverage[index] = index;
            }
            Ok(())
        })?;
    }
    pad_coverage(&mut coverage, geometry, cancelled)?;
    for channel in 0..4 {
        let source = texture
            .map(|texture| TexturePlane::new(texture, channel))
            .transpose()?;
        if source.is_none() && channel != 3 {
            continue;
        }
        let mut target = allocated(count, if channel == 3 { 1. } else { 0. })?;
        for (index, (target, &sample)) in target.iter_mut().zip(&coverage).enumerate() {
            if index % 1024 == 0 {
                check_render_cancelled(cancelled)?;
            }
            if sample == BACKGROUND {
                continue;
            }
            let value = if let Some(source) = &source {
                source.sample(coordinates[sample], [true, false])?
            } else {
                1.
            };
            let value = unorm(value, 65535.);
            *target = if channel == 3 { 1. - value } else { value };
        }
        image.set_buffer(channel, Some(&FloatBuffer::new(target)))?;
    }
    Ok(image)
}
