//! Bake the angle between each surface normal and its incident face directions.
use super::uv_apply::allocated;
use super::uv_mesh::{Faces, at, bake_vertex_values};
use super::{MeshModel, MeshVector, SceneError};
use crate::pointwise::check_render_cancelled;
use crate::{ImageGeometry, PointwiseError, SharedImageMap};

fn for_face<E>(
    group: &Faces,
    face: &[i32],
    mut apply: impl FnMut(usize) -> Result<(), PointwiseError<E>>,
) -> Result<(), PointwiseError<E>> {
    for &corner in face {
        let index = *at(&group.positions, corner).map_err(PointwiseError::Scene)?;
        apply(index as usize)?;
    }
    Ok(())
}

fn measure<E>(
    groups: &[Faces],
    points: &[MeshVector],
    normals: &[MeshVector],
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<f32>, PointwiseError<E>> {
    let mut sums = allocated(points.len(), (0_f32, 0_f32))?;
    for group in groups {
        for face in group.iter() {
            check_render_cancelled(cancelled)?;
            let mut centroid = MeshVector::default();
            for_face(group, face, |i| {
                let point = points
                    .get(i)
                    .ok_or(PointwiseError::Scene(SceneError::IndexOutOfRange))?;
                centroid = MeshVector::add(centroid, *point);
                Ok(())
            })?;
            let size = face.len() as f32;
            let [x, y, z] = centroid.values();
            centroid = MeshVector::new(x / size, y / size, z / size);
            for_face(group, face, |i| {
                let mut radial = MeshVector::sub(points[i], centroid);
                radial.normalize();
                sums[i].0 += radial.dot(normals[i]);
                sums[i].1 += 1.;
                Ok(())
            })?;
        }
    }
    Ok(sums
        .into_iter()
        .map(|(sum, count)| {
            if count == 0. {
                0.5
            } else {
                0.5 * (sum / count) + 0.5
            }
        })
        .collect())
}

pub(crate) fn bake<E>(
    models: &[MeshModel],
    geometry: ImageGeometry,
    cancelled: &dyn Fn() -> bool,
) -> Result<SharedImageMap, PointwiseError<E>> {
    bake_vertex_values(models, geometry, cancelled, measure)
}
