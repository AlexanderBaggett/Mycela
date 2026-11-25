//! Mesh inputs and opaque atlas completion shared by geometry-only UV effects.
use super::uv_apply::{BACKGROUND, Viewport, allocated, pad_coverage, raster_triangle, unorm};
use super::{MeshAttribute, MeshModel, MeshVector, PolygonGroup, PolygonTopology, SceneError};
use crate::pointwise::check_render_cancelled;
use crate::{FloatBuffer, ImageGeometry, PointwiseError, SharedImageMap};

pub(super) fn area<E>(geometry: ImageGeometry) -> Result<usize, PointwiseError<E>> {
    let (width, height) = (geometry.width(), geometry.height());
    width
        .checked_mul(height)
        .filter(|_| width > 0 && height > 0)
        .map(|area| area as usize)
        .ok_or(PointwiseError::Argument)
}

pub(super) struct AtlasMesh {
    pub(super) points: Vec<MeshVector>,
    pub(super) groups: Vec<Faces>,
    pub(super) model: MeshModel,
}

impl AtlasMesh {
    pub(super) fn position<E>(
        &self,
        group: &Faces,
        corner: i32,
    ) -> Result<MeshVector, PointwiseError<E>> {
        let index = *at(&group.positions, corner).map_err(PointwiseError::Scene)?;
        at(&self.points, index)
            .copied()
            .map_err(PointwiseError::Scene)
    }

    pub(super) fn triangles<E>(
        &self,
        cancelled: &dyn Fn() -> bool,
        mut draw: impl FnMut(&Faces, [i32; 3]) -> Result<(), PointwiseError<E>>,
    ) -> Result<(), PointwiseError<E>> {
        for group in &self.groups {
            for triangle in group.triangles() {
                check_render_cancelled(cancelled)?;
                draw(group, triangle)?;
            }
        }
        Ok(())
    }
}

pub(super) fn read_meshes<E>(
    models: &[MeshModel],
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<AtlasMesh>, PointwiseError<E>> {
    models
        .iter()
        .map(|model| {
            check_render_cancelled(cancelled)?;
            let points = model.try_positions().map_err(PointwiseError::Scene)?;
            let groups = model
                .groups()
                .iter()
                .map(Faces::read)
                .collect::<Result<Vec<_>, _>>()
                .map_err(PointwiseError::Scene)?;
            Ok(AtlasMesh {
                points,
                groups,
                model: model.clone(),
            })
        })
        .collect()
}

#[derive(Clone, Copy, Default)]
pub(super) struct Surface {
    pub(super) position: MeshVector,
    pub(super) normal: MeshVector,
}

impl AtlasMesh {
    pub(super) fn raster_surfaces<E>(
        &self,
        geometry: ImageGeometry,
        cancelled: &dyn Fn() -> bool,
        mut emit: impl FnMut(usize, Surface) -> Result<(), PointwiseError<E>>,
    ) -> Result<(), PointwiseError<E>> {
        let uvs = self.model.try_uvs().map_err(PointwiseError::Scene)?;
        let normals = self.model.try_normals().map_err(PointwiseError::Scene)?;
        let normals = vertex_normals(&self.groups, &normals, self.points.len(), cancelled)?;
        let viewport = Viewport::new(geometry);
        self.triangles(cancelled, |group, corners| {
            let mut triangle = [[0.; 2]; 3];
            let mut points = [[0.; 3]; 3];
            let mut directions = [[0.; 3]; 3];
            for (index, corner) in corners.into_iter().enumerate() {
                let uv = *at(&group.uvs, corner).map_err(PointwiseError::Scene)?;
                let position = *at(&group.positions, corner).map_err(PointwiseError::Scene)?;
                triangle[index] =
                    viewport.vertex(at(&uvs, uv).map_err(PointwiseError::Scene)?.values());
                points[index] = self.position(group, corner)?.values();
                directions[index] = at(&normals, position)
                    .map_err(PointwiseError::Scene)?
                    .values();
            }
            raster_triangle(triangle, geometry, cancelled, |pixel, [_, y, z]| {
                let interpolate = |values: [[f32; 3]; 3]| {
                    let [x, y, z] = std::array::from_fn(|axis| {
                        let [a, b, c] = values.map(|row| row[axis]);
                        a + (b - a) * y + (c - a) * z
                    });
                    MeshVector::new(x, y, z)
                };
                emit(
                    pixel,
                    Surface {
                        position: interpolate(points),
                        normal: interpolate(directions),
                    },
                )
            })
        })
    }
}

pub(super) struct Faces {
    corners: Vec<i32>,
    pub(super) positions: Vec<i32>,
    pub(super) normals: Vec<i32>,
    pub(super) uvs: Vec<i32>,
    face_size: usize,
}

impl Faces {
    pub(super) fn read(group: &PolygonGroup) -> Result<Self, SceneError> {
        let face_size = match group.topology() {
            PolygonTopology::Triangles => 3,
            PolygonTopology::Quads => 4,
        };
        let corners = group.face_indices()?.try_values()?;
        if corners.len() % face_size != 0 {
            return Err(SceneError::IndexOutOfRange);
        }
        Ok(Self {
            corners,
            positions: group.try_indices(MeshAttribute::Position)?,
            normals: group.try_indices(MeshAttribute::Normal)?,
            uvs: group.try_indices(MeshAttribute::Uv)?,
            face_size,
        })
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = &[i32]> {
        self.corners.chunks_exact(self.face_size)
    }

    pub(super) fn triangles(&self) -> impl Iterator<Item = [i32; 3]> {
        self.iter().flat_map(|face| {
            let corners = match *face {
                [a, b, c, d] => [Some([a, b, d]), Some([b, c, d])],
                [a, b, c] => [Some([a, b, c]), None],
                _ => unreachable!("validated polygon topology"),
            };
            corners.into_iter().flatten()
        })
    }
}

pub(super) fn at<T>(values: &[T], index: i32) -> Result<&T, SceneError> {
    values
        .get(index as usize)
        .ok_or(SceneError::IndexOutOfRange)
}

pub(super) fn vertex_normals<E>(
    groups: &[Faces],
    normals: &[MeshVector],
    vertex_count: usize,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<MeshVector>, PointwiseError<E>> {
    let mut sums = allocated(vertex_count, MeshVector::default())?;
    for group in groups {
        for (index, &corner) in group.corners.iter().enumerate() {
            if index % 1024 == 0 {
                check_render_cancelled(cancelled)?;
            }
            let position = *at(&group.positions, corner).map_err(PointwiseError::Scene)?;
            let normal = *at(&group.normals, corner).map_err(PointwiseError::Scene)?;
            let normal = at(normals, normal).map_err(PointwiseError::Scene)?;
            let sum = sums
                .get_mut(position as usize)
                .ok_or(PointwiseError::Scene(SceneError::IndexOutOfRange))?;
            *sum = MeshVector::add(*sum, *normal);
        }
    }
    for sum in &mut sums {
        sum.normalize();
    }
    Ok(sums)
}

pub(super) fn finish_atlas<E>(
    geometry: ImageGeometry,
    mut coverage: Vec<usize>,
    mut red: Vec<f32>,
    cancelled: &dyn Fn() -> bool,
) -> Result<SharedImageMap, PointwiseError<E>> {
    pad_coverage(&mut coverage, geometry, cancelled)?;
    let mut transparency = allocated(coverage.len(), 1.)?;
    for (index, &source) in coverage.iter().enumerate() {
        if index % 1024 == 0 {
            check_render_cancelled(cancelled)?;
        }
        if source != BACKGROUND {
            red[index] = red[source];
            transparency[index] = 0.;
        }
    }
    let image = SharedImageMap::new(geometry);
    image.set_buffer(0, Some(&FloatBuffer::new(red)))?;
    image.set_buffer(3, Some(&FloatBuffer::new(transparency)))?;
    Ok(image)
}

struct VertexAtlas {
    coverage: Vec<usize>,
    red: Vec<f32>,
}

type VertexMeasure<E> = fn(
    &[Faces],
    &[MeshVector],
    &[MeshVector],
    &dyn Fn() -> bool,
) -> Result<Vec<f32>, PointwiseError<E>>;

fn draw_vertex_values<E>(
    model: &MeshModel,
    geometry: ImageGeometry,
    atlas: &mut VertexAtlas,
    measure: VertexMeasure<E>,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), PointwiseError<E>> {
    let points = model.try_positions().map_err(PointwiseError::Scene)?;
    let normals = model.try_normals().map_err(PointwiseError::Scene)?;
    let uvs = model.try_uvs().map_err(PointwiseError::Scene)?;
    let groups = model
        .groups()
        .iter()
        .map(Faces::read)
        .collect::<Result<Vec<_>, _>>()
        .map_err(PointwiseError::Scene)?;
    let normals = vertex_normals(&groups, &normals, points.len(), cancelled)?;
    let values = measure(&groups, &points, &normals, cancelled)?;
    let viewport = Viewport::new(geometry);
    for group in groups {
        for triangle in group.triangles() {
            check_render_cancelled(cancelled)?;
            let mut vertices = [[0.; 2]; 3];
            let mut triangle_values = [0.; 3];
            for (i, corner) in triangle.into_iter().enumerate() {
                let uv = *at(&group.uvs, corner).map_err(PointwiseError::Scene)?;
                let position = *at(&group.positions, corner).map_err(PointwiseError::Scene)?;
                vertices[i] =
                    viewport.vertex(at(&uvs, uv).map_err(PointwiseError::Scene)?.values());
                triangle_values[i] = *at(&values, position).map_err(PointwiseError::Scene)?;
            }
            raster_triangle(vertices, geometry, cancelled, |index, [_, y, z]| {
                if atlas.coverage[index] == BACKGROUND {
                    let [a, b, c] = triangle_values;
                    atlas.red[index] = unorm(a + (b - a) * y + (c - a) * z, 65535.);
                    atlas.coverage[index] = index;
                }
                Ok(())
            })?;
        }
    }
    Ok(())
}

pub(super) fn bake_vertex_values<E>(
    models: &[MeshModel],
    geometry: ImageGeometry,
    cancelled: &dyn Fn() -> bool,
    measure: VertexMeasure<E>,
) -> Result<SharedImageMap, PointwiseError<E>> {
    if models.is_empty() {
        return Ok(SharedImageMap::new(geometry));
    }
    let (width, height) = (geometry.width(), geometry.height());
    let area = width
        .checked_mul(height)
        .filter(|_| width > 0 && height > 0)
        .ok_or(PointwiseError::Argument)? as usize;
    let mut atlas = VertexAtlas {
        coverage: allocated(area, BACKGROUND)?,
        red: allocated(area, 0.)?,
    };
    for model in models {
        draw_vertex_values(model, geometry, &mut atlas, measure, cancelled)?;
    }
    finish_atlas(geometry, atlas.coverage, atlas.red, cancelled)
}
