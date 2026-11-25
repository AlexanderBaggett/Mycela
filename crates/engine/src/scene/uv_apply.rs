//! Single-layer UV coverage and the source's four directional padding passes.
use super::{MeshAttribute, Scene, SceneError};
use crate::pointwise::check_render_cancelled;
use crate::{FloatBuffer, ImageError, ImageGeometry, PointwiseError, SharedImageMap};

pub(super) const BACKGROUND: usize = usize::MAX;

pub(super) struct Viewport {
    side: f64,
    origin: [f64; 2],
}

impl Viewport {
    pub(super) fn new(geometry: ImageGeometry) -> Self {
        let width = f64::from(geometry.width());
        let height = f64::from(geometry.height());
        Self {
            side: height,
            origin: [(width - height) * 0.5, 0.],
        }
    }

    pub(super) fn vertex(&self, [u, v]: [f32; 2]) -> [f64; 2] {
        [
            self.origin[0] + f64::from(u) * self.side,
            self.origin[1] + f64::from(v) * self.side,
        ]
    }

    pub(super) fn uv(&self, pixel: usize, width: i32) -> [f64; 2] {
        let [x, y] = [pixel % width as usize, pixel / width as usize];
        [
            (x as f64 + 0.5 - self.origin[0]) / self.side,
            (y as f64 + 0.5 - self.origin[1]) / self.side,
        ]
    }
}

pub(super) struct TexturePlane {
    texels: Vec<f32>,
    dimensions: [i32; 2],
}

impl TexturePlane {
    pub(super) fn new(texture: &SharedImageMap, channel: i32) -> Result<Self, ImageError> {
        let dimensions = [texture.geometry().width(), texture.geometry().height()];
        let texels = texture.channel(channel, false)?.values();
        Ok(Self::uploaded(texels, dimensions, channel == 3))
    }

    pub(super) fn uploaded(mut texels: Vec<f32>, dimensions: [i32; 2], alpha: bool) -> Self {
        for texel in &mut texels {
            let value = if alpha { 1. - *texel } else { *texel };
            *texel = unorm(value, 255.);
        }
        Self { texels, dimensions }
    }

    pub(super) fn sample(&self, uv: [f64; 2], wraps: [bool; 2]) -> Result<f32, ImageError> {
        if self.texels.len() == 1 {
            return Ok(self.texels[0]);
        }
        let [width, height] = self.dimensions;
        let (x0, x1, x_ratio) = sample_axis(uv[0], width, wraps[0]);
        let (y0, y1, y_ratio) = sample_axis(uv[1], height, wraps[1]);
        let read = |x, y: usize| {
            self.texels
                .get(x + y * width as usize)
                .copied()
                .ok_or(ImageError::Index)
        };
        let horizontal =
            |y| -> Result<f32, ImageError> { Ok(mix(read(x0, y)?, read(x1, y)?, x_ratio)) };
        Ok(mix(horizontal(y0)?, horizontal(y1)?, y_ratio))
    }
}

fn sample_axis(normalized: f64, texels: i32, wraps: bool) -> (usize, usize, f32) {
    let coordinate = normalized * f64::from(texels) - 0.5;
    let floor = coordinate.floor();
    let boundary = |index: f64| {
        if wraps {
            index.rem_euclid(f64::from(texels)) as usize
        } else {
            (index as i32).clamp(0, texels - 1) as usize
        }
    };
    (
        boundary(floor),
        boundary(floor + 1.),
        (coordinate - floor) as f32,
    )
}

pub(crate) fn bake<E>(
    scene: Option<&Scene>,
    texture: &SharedImageMap,
    geometry: ImageGeometry,
    cancelled: &dyn Fn() -> bool,
) -> Result<SharedImageMap, PointwiseError<E>> {
    let image = SharedImageMap::new(geometry);
    image.set_constant(3, 1.)?;
    let Some(scene) = scene else {
        return Ok(image);
    };
    let (width, height) = (geometry.width(), geometry.height());
    let area = width
        .checked_mul(height)
        .filter(|_| width > 0 && height > 0)
        .ok_or(PointwiseError::Argument)? as usize;
    let (texture_width, texture_height) = (texture.geometry().width(), texture.geometry().height());
    if texture_width <= 0 || texture_height <= 0 {
        return Err(PointwiseError::Argument);
    }
    let viewport = Viewport::new(geometry);
    let mut coverage = allocated(area, BACKGROUND)?;
    for model in scene.try_models_snapshot().map_err(PointwiseError::Scene)? {
        check_render_cancelled(cancelled)?;
        let coords = model.try_uvs().map_err(PointwiseError::Scene)?;
        for group in model.groups() {
            let indices = group
                .try_indices(MeshAttribute::Uv)
                .map_err(PointwiseError::Scene)?;
            let corners = group.triangle_corners().map_err(PointwiseError::Scene)?;
            for triangle in corners.chunks_exact(3) {
                check_render_cancelled(cancelled)?;
                let mut vertices = [[0.; 2]; 3];
                for (&corner, position) in triangle.iter().zip(&mut vertices) {
                    let index = indices
                        .get(corner as usize)
                        .ok_or(SceneError::IndexOutOfRange)
                        .map_err(PointwiseError::Scene)?;
                    let uv = coords
                        .get(*index as usize)
                        .ok_or(SceneError::IndexOutOfRange)
                        .map_err(PointwiseError::Scene)?
                        .values();
                    *position = viewport.vertex(uv);
                }
                raster_triangle(vertices, geometry, cancelled, |index, _| {
                    coverage[index] = index;
                    Ok(())
                })?;
            }
        }
    }
    if coverage.iter().all(|&index| index == BACKGROUND) {
        return Ok(image);
    }
    pad_coverage(&mut coverage, geometry, cancelled)?;
    for channel in 0..4 {
        let source = TexturePlane::new(texture, channel)?;
        let border = if channel == 3 { 1. } else { 0. };
        let mut plane = allocated(area, border)?;
        for (row, indices) in plane
            .chunks_mut(width as usize)
            .zip(coverage.chunks(width as usize))
        {
            check_render_cancelled(cancelled)?;
            for (value, &index) in row.iter_mut().zip(indices) {
                if index != BACKGROUND {
                    let sampled = source.sample(viewport.uv(index, width), [true, false])?;
                    let sampled = unorm(sampled, 65535.);
                    *value = if channel == 3 { 1. - sampled } else { sampled };
                }
            }
        }
        image.set_buffer(channel, Some(&FloatBuffer::new(plane)))?;
    }
    Ok(image)
}

pub(super) fn unorm(value: f32, maximum: f32) -> f32 {
    (value.clamp(0., 1.) * maximum).round_ties_even() / maximum
}

fn mix(a: f32, b: f32, ratio: f32) -> f32 {
    a + (b - a) * ratio
}

pub(super) fn pad_coverage<E>(
    coverage: &mut [usize],
    geometry: ImageGeometry,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), PointwiseError<E>> {
    let (width, height) = (geometry.width(), geometry.height());
    let padding = (f64::from(width.max(height)) * 0.01).ceil() as i32;
    for (rows, reverse) in [(true, true), (true, false), (false, true), (false, false)] {
        let (lines, length, stride) = if rows {
            (height, width, 1)
        } else {
            (width, height, width)
        };
        for line in 0..lines {
            check_render_cancelled(cancelled)?;
            let start = if rows { line * width } else { line };
            let position = |offset| if reverse { length - 1 - offset } else { offset };
            let index = |x| (start + x * stride) as usize;
            let mut last = BACKGROUND;
            let mut distance = 0;
            for step in 0..padding.min(length) {
                let x = position(length - 1 - step);
                if coverage[index(x)] != BACKGROUND {
                    last = coverage[index(x)];
                    // The source uses the image width even during its column passes.
                    distance = width - 1 - x;
                    break;
                }
            }
            for step in 0..length {
                let target = &mut coverage[index(position(step))];
                if *target != BACKGROUND {
                    last = *target;
                    distance = 0;
                } else if last != BACKGROUND && distance < padding {
                    *target = last;
                    distance += 1;
                }
            }
        }
    }
    Ok(())
}

pub(super) fn allocated<T: Clone>(len: usize, value: T) -> Result<Vec<T>, ImageError> {
    let mut result = Vec::new();
    result
        .try_reserve_exact(len)
        .map_err(|_| ImageError::Allocation)?;
    result.resize(len, value);
    Ok(result)
}

pub(super) fn raster_triangle<E>(
    vertices: [[f64; 2]; 3],
    geometry: ImageGeometry,
    cancelled: &dyn Fn() -> bool,
    mut emit: impl FnMut(usize, [f32; 3]) -> Result<(), PointwiseError<E>>,
) -> Result<(), PointwiseError<E>> {
    let [a, b, c] = vertices;
    let edge = |start: [f64; 2], end: [f64; 2], point: [f64; 2]| {
        (end[0] - start[0]) * (point[1] - start[1]) - (end[1] - start[1]) * (point[0] - start[0])
    };
    let orientation = edge(a, b, c);
    if !orientation.is_finite() || orientation == 0. {
        return Ok(());
    }
    let bounds = |axis: usize, limit: i32| {
        let [a, b, c] = vertices.map(|v| v[axis]);
        let low = (a.min(b).min(c) - 0.5).ceil() as i32;
        let high = (a.max(b).max(c) - 0.5).floor() as i32;
        low.clamp(0, limit)..high.saturating_add(1).clamp(0, limit)
    };
    let columns = bounds(0, geometry.width());
    let sign = orientation.signum();
    for y in bounds(1, geometry.height()) {
        check_render_cancelled(cancelled)?;
        for x in columns.clone() {
            let point = [f64::from(x) + 0.5, f64::from(y) + 0.5];
            let edges = [edge(b, c, point), edge(c, a, point), edge(a, b, point)];
            if edges.into_iter().all(|edge| edge * sign >= 0.) {
                let index = (y * geometry.width() + x) as usize;
                emit(index, edges.map(|edge| (edge / orientation) as f32))?;
            }
        }
    }
    Ok(())
}
