//! Shared UV light-space depth rasterization.
use super::SceneTransform;
use super::uv_apply::{raster_triangle, unorm};
use super::uv_mesh::AtlasMesh;
use crate::{ImageGeometry, PointwiseError};

pub(super) fn raster_depth<E>(
    models: &[AtlasMesh],
    geometry: ImageGeometry,
    world_to_clip: SceneTransform,
    depth_maximum: f32,
    depth: &mut [f32],
    cancelled: &dyn Fn() -> bool,
) -> Result<(), PointwiseError<E>> {
    let scale = [f64::from(geometry.width()), f64::from(geometry.height())];
    for mesh in models {
        mesh.triangles(cancelled, |group, corners| {
            let mut clip = [[0.; 4]; 3];
            for (&corner, clip) in corners.iter().zip(&mut clip) {
                let position = mesh.position(group, corner)?.values().map(f64::from);
                *clip = world_to_clip.transform_homogeneous([
                    position[0],
                    position[1],
                    position[2],
                    1.,
                ]);
            }
            let (polygon, count) = clip_near(clip);
            for edge in 1..count.saturating_sub(1) {
                let triangle = [polygon[0], polygon[edge], polygon[edge + 1]];
                let point = triangle.map(|[x, y, _, w]| {
                    [
                        (x / w * 0.5 + 0.5) * scale[0],
                        (y / w * 0.5 + 0.5) * scale[1],
                    ]
                });
                let depths = triangle.map(|[_, _, z, w]| (z / w * 0.5 + 0.5) as f32);
                raster_triangle(point, geometry, cancelled, |index, weights| {
                    let value =
                        depths[0] * weights[0] + depths[1] * weights[1] + depths[2] * weights[2];
                    if (0. ..=1.).contains(&value) && value < depth[index] {
                        depth[index] = unorm(value, depth_maximum);
                    }
                    Ok(())
                })?;
            }
            Ok(())
        })?;
    }
    Ok(())
}

// Intersect a shadow triangle with the homogeneous near plane before
// dividing by W. This keeps surfaces at or behind the light well-defined.
fn clip_near(triangle: [[f64; 4]; 3]) -> ([[f64; 4]; 4], usize) {
    let mut clipped = [[0.; 4]; 4];
    let mut count = 0;
    let distance = |v: [f64; 4]| v[2] + v[3];
    let mut start = triangle[2];
    for end in triangle {
        let (a, b) = (distance(start), distance(end));
        if (a >= 0.) != (b >= 0.) {
            let t = a / (a - b);
            clipped[count] = std::array::from_fn(|i| start[i] + (end[i] - start[i]) * t);
            count += 1;
        }
        if b >= 0. {
            clipped[count] = end;
            count += 1;
        }
        start = end;
    }
    (clipped, count)
}
