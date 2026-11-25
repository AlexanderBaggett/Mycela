//! Projection shaders evaluated over the mesh's existing UV triangles.
use super::uv_apply::{
    BACKGROUND, TexturePlane, Viewport, allocated, pad_coverage, raster_triangle, unorm,
};
use super::uv_multi_project::MultiProjection;
use super::{MeshAttribute, MeshModel, SceneError, SceneTransform, SceneVector};
use crate::pointwise::check_render_cancelled;
use crate::{FloatBuffer, ImageError, ImageGeometry, PointwiseError, SharedImageMap};

pub(crate) enum ProjectionMode {
    Cylindrical,
    Planar { side: i32 },
    ShrinkWrap,
    Spherical,
    Multi(MultiProjection),
}

pub(crate) struct Projection {
    matrix: [f32; 16],
    direction: [f32; 3],
    mode: ProjectionMode,
    tile: bool,
}

impl Projection {
    pub(crate) fn new(
        size: [f64; 3],
        location: [f64; 3],
        [heading, pitch, roll]: [f64; 3],
        mode: ProjectionMode,
        tile: bool,
    ) -> Self {
        let transform = SceneTransform::from_components(
            vector(size.map(|value| 1. / value)),
            vector(location.map(|value| -value)),
            vector([heading, -pitch, roll]),
        );
        let [x, y, z, _] = transform.transform_homogeneous([0., 0., -1., 0.]);
        let tile = tile && !matches!(mode, ProjectionMode::ShrinkWrap);
        Self {
            matrix: transform.values().map(|value| value as f32),
            direction: normalize([-x as f32, -y as f32, z as f32]),
            mode,
            tile,
        }
    }

    fn transform(&self, point: [f32; 3], w: f32) -> [f32; 3] {
        let m = self.matrix;
        std::array::from_fn(|j| {
            m[j] * point[0] + m[j + 4] * point[1] + m[j + 8] * point[2] + m[j + 12] * w
        })
    }

    fn color(
        &self,
        point: [f32; 3],
        normal: [f32; 3],
        texture: &Texture,
        background: [f32; 4],
    ) -> Result<[f32; 4], ImageError> {
        if let ProjectionMode::Multi(multi) = &self.mode {
            return multi.sample(
                self.transform(point, 1.),
                normalize(self.transform(normal, 0.)),
                self.tile,
                background,
                |uv| texture.sample(uv),
            );
        }
        match self.coordinates(point, normal) {
            Some(uv) => Ok(over(texture.sample(uv)?, background)),
            None => Ok(background),
        }
    }

    fn coordinates(&self, mut point: [f32; 3], normal: [f32; 3]) -> Option<[f64; 2]> {
        use ProjectionMode::*;
        if let Planar { side } = self.mode {
            let facing = dot(normal, self.direction);
            let front_limit = if self.tile { 0. } else { -0.001 };
            if (side == 1 && facing < front_limit) || (side == 2 && facing > 0.) {
                return None;
            }
        }
        if matches!(self.mode, ShrinkWrap | Spherical) {
            point = normalize(point);
        }
        let position = self.transform(point, 1.);
        let [x, y, z] = position;
        let longitude = || {
            let angle = z.atan2(x) * 0.5 * std::f32::consts::FRAC_1_PI;
            if angle <= 0. { angle + 1. } else { angle }
        };
        let polar_angle =
            || normalize(position)[1].clamp(-1., 1.).acos() * std::f32::consts::FRAC_1_PI;
        let uv = match self.mode {
            Cylindrical => [longitude(), 0.5 - y],
            Planar { .. } => [x + 0.5, 0.5 - y],
            ShrinkWrap => {
                let radius = polar_angle() * 0.5;
                let [u, v, _] = normalize([x, z, 0.]);
                [radius * u + 0.5, radius * v + 0.5]
            }
            Spherical => [longitude(), polar_angle()],
            Multi(_) => unreachable!("handled by the multi-projection sampler"),
        };
        if uv
            .iter()
            .any(|v| !v.is_finite() || (!self.tile && !(0. ..=1.).contains(v)))
        {
            return None;
        }
        Some(uv.map(f64::from))
    }
}

fn vector([x, y, z]: [f64; 3]) -> SceneVector {
    SceneVector::new(x, y, z)
}

pub(super) fn dot([x, y, z]: [f32; 3], [a, b, c]: [f32; 3]) -> f32 {
    x * a + y * b + z * c
}

pub(super) fn normalize(v: [f32; 3]) -> [f32; 3] {
    let length = dot(v, v).sqrt();
    if length == 0. {
        v
    } else {
        v.map(|component| component / length)
    }
}

fn index<T>(values: &[T], at: i32) -> Result<&T, SceneError> {
    values.get(at as usize).ok_or(SceneError::IndexOutOfRange)
}

fn bounding_center(points: &[super::MeshVector]) -> [f32; 3] {
    let mut minimum = [f32::MAX; 3];
    let mut maximum = [f32::MIN; 3];
    for point in points {
        for (axis, value) in point.values().into_iter().enumerate() {
            minimum[axis] = minimum[axis].min(value);
            maximum[axis] = maximum[axis].max(value);
        }
    }
    std::array::from_fn(|axis| (minimum[axis] + maximum[axis]) * 0.5)
}

// Genetica's projection mesh averages normals by position before the side test.
fn average_normals<E>(
    model: &MeshModel,
    positions: usize,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<[f32; 3]>, PointwiseError<E>> {
    let normals = model.try_normals().map_err(PointwiseError::Scene)?;
    let mut smooth = allocated(positions, [0.; 3])?;
    for group in model.groups() {
        let points = group
            .try_indices(MeshAttribute::Position)
            .map_err(PointwiseError::Scene)?;
        let indices = group
            .try_indices(MeshAttribute::Normal)
            .map_err(PointwiseError::Scene)?;
        if points.len() != indices.len() {
            return Err(PointwiseError::Scene(SceneError::IndexOutOfRange));
        }
        for (corner, (&point, &normal)) in points.iter().zip(&indices).enumerate() {
            if corner % 1024 == 0 {
                check_render_cancelled(cancelled)?;
            }
            let normal = index(&normals, normal)
                .map_err(PointwiseError::Scene)?
                .values();
            let average = smooth
                .get_mut(point as usize)
                .ok_or(PointwiseError::Scene(SceneError::IndexOutOfRange))?;
            for (target, value) in average.iter_mut().zip(normal) {
                *target += value;
            }
        }
    }
    for normal in &mut smooth {
        *normal = normalize(*normal);
    }
    Ok(smooth)
}

struct Texture([TexturePlane; 4]);

impl Texture {
    fn new(image: &SharedImageMap) -> Result<Self, ImageError> {
        Ok(Self([
            TexturePlane::new(image, 0)?,
            TexturePlane::new(image, 1)?,
            TexturePlane::new(image, 2)?,
            TexturePlane::new(image, 3)?,
        ]))
    }

    fn sample(&self, uv: [f64; 2]) -> Result<[f32; 4], ImageError> {
        let mut result = [0.; 4];
        for (value, plane) in result.iter_mut().zip(&self.0) {
            *value = plane.sample(uv, [true, true])?;
        }
        Ok(result)
    }
}

fn over(front: [f32; 4], back: [f32; 4]) -> [f32; 4] {
    let background = back[3] * (1. - front[3]);
    let alpha = front[3] + background;
    if alpha <= 0. {
        return [0.; 4];
    }
    let reciprocal = 1. / alpha;
    [
        (front[0] * front[3] + background * back[0]) * reciprocal,
        (front[1] * front[3] + background * back[1]) * reciprocal,
        (front[2] * front[3] + background * back[2]) * reciprocal,
        alpha,
    ]
}

pub(crate) fn bake<E>(
    model: &MeshModel,
    texture: &SharedImageMap,
    geometry: ImageGeometry,
    projection: &Projection,
    cancelled: &dyn Fn() -> bool,
) -> Result<SharedImageMap, PointwiseError<E>> {
    let width = geometry.width();
    let height = geometry.height();
    let area = width
        .checked_mul(height)
        .filter(|_| width > 0 && height > 0)
        .ok_or(PointwiseError::Argument)? as usize;
    let viewport = Viewport::new(geometry);
    let source = Texture::new(texture)?;
    let background = model
        .material()
        .map()
        .as_ref()
        .map(Texture::new)
        .transpose()?;
    let mut coverage = allocated(area, BACKGROUND)?;
    let mut rgba = allocated(area, [0.; 4])?;
    let positions = model.try_positions().map_err(PointwiseError::Scene)?;
    let center = bounding_center(&positions);
    let texcoords = model.try_uvs().map_err(PointwiseError::Scene)?;
    let normals = match projection.mode {
        ProjectionMode::Planar { side: 1 | 2 } | ProjectionMode::Multi(_) => {
            Some(average_normals(model, positions.len(), cancelled)?)
        }
        _ => None,
    };
    for group in model.groups() {
        check_render_cancelled(cancelled)?;
        let mut corners = group.triangle_corners().map_err(PointwiseError::Scene)?;
        if group.topology() == super::PolygonTopology::Quads {
            for quad in corners.chunks_exact_mut(6) {
                // The original GL_QUADS capture interpolates across the 1–3 diagonal.
                quad[2] = quad[5];
                quad[3] = quad[1];
            }
        }
        let uv_indices = group
            .try_indices(MeshAttribute::Uv)
            .map_err(PointwiseError::Scene)?;
        let point_indices = group
            .try_indices(MeshAttribute::Position)
            .map_err(PointwiseError::Scene)?;
        for corners in corners.chunks_exact(3) {
            let mut atlas = [[0.; 2]; 3];
            let mut xyz = [[0.; 3]; 3];
            let mut normal = [[0.; 3]; 3];
            for (vertex, &corner) in corners.iter().enumerate() {
                let uv = *index(&uv_indices, corner).map_err(PointwiseError::Scene)?;
                let point = *index(&point_indices, corner).map_err(PointwiseError::Scene)?;
                atlas[vertex] = viewport.vertex(
                    index(&texcoords, uv)
                        .map_err(PointwiseError::Scene)?
                        .values(),
                );
                xyz[vertex] = index(&positions, point)
                    .map_err(PointwiseError::Scene)?
                    .values();
                if let Some(normals) = &normals {
                    normal[vertex] = *index(normals, point).map_err(PointwiseError::Scene)?;
                }
            }
            raster_triangle(atlas, geometry, cancelled, |pixel, weights| {
                let point = std::array::from_fn(|axis| {
                    dot([xyz[0][axis], xyz[1][axis], xyz[2][axis]], weights) - center[axis]
                });
                let normal = std::array::from_fn(|axis| {
                    dot([normal[0][axis], normal[1][axis], normal[2][axis]], weights)
                });
                let back = match &background {
                    Some(background) => background.sample(viewport.uv(pixel, width))?,
                    None => [1., 1., 1., 0.],
                };
                let color = projection.color(point, normal, &source, back)?;
                rgba[pixel] = color.map(|value| unorm(value, 65535.));
                coverage[pixel] = pixel;
                Ok(())
            })?;
        }
    }
    pad_coverage(&mut coverage, geometry, cancelled)?;
    let image = SharedImageMap::new(geometry);
    for (channel, background) in [0., 0., 0., 1.].into_iter().enumerate() {
        let mut plane = allocated(area, background)?;
        for (pixel, &index) in coverage.iter().enumerate() {
            if pixel % 1024 == 0 {
                check_render_cancelled(cancelled)?;
            }
            if index != BACKGROUND {
                let value = rgba[index][channel];
                plane[pixel] = if channel == 3 { 1. - value } else { value };
            }
        }
        image.set_buffer(channel as i32, Some(&FloatBuffer::new(plane)))?;
    }
    Ok(image)
}
