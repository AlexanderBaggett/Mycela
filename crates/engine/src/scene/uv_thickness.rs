//! Measure the first surface reached along the inward normal of each vertex.
use super::uv_apply::allocated;
use super::uv_mesh::{Faces, at, bake_vertex_values};
use super::{MeshModel, MeshVector};
use crate::pointwise::check_render_cancelled;
use crate::{ImageError, ImageGeometry, PointwiseError, SharedImageMap};

type Point = [f64; 3];

const PARALLEL_EPSILON: f64 = 0.0001;
const SURFACE_EPSILON: f64 = 0.000001;

fn difference(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot([x, y, z]: Point, [u, v, w]: Point) -> f64 {
    x * u + y * v + z * w
}

fn unit(value: Point) -> Point {
    let length = dot(value, value).sqrt();
    if length == 0. {
        value
    } else {
        let scale = 1. / length;
        value.map(|coordinate| coordinate * scale)
    }
}

struct Polygon {
    plane_normal: Point,
    plane_offset: f64,
    axes: [Point; 2],
    outline: [[f64; 2]; 4],
    corners: usize,
}

impl Polygon {
    fn new(points: [Point; 4], corners: usize) -> Self {
        let [[ax, ay, az], [bx, by, bz], [cx, cy, cz], _] = points;
        // Quads use the plane through their first three vertices. The source
        // parallel test uses the original, unnormalized coefficients.
        let plane_normal = [
            ay * (bz - cz) + by * (cz - az) + cy * (az - bz),
            az * (bx - cx) + bz * (cx - ax) + cz * (ax - bx),
            ax * (by - cy) + bx * (cy - ay) + cx * (ay - by),
        ];
        let plane_offset =
            -(ax * (by * cz - cy * bz) + bx * (cy * az - ay * cz) + cx * (ay * bz - by * az));
        let axes = [
            unit(difference(points[2], points[1])),
            unit(difference(points[0], points[1])),
        ];
        let outline = points.map(|point| [dot(point, axes[0]), dot(point, axes[1])]);
        Self {
            plane_normal,
            plane_offset,
            axes,
            outline,
            corners,
        }
    }

    fn nearer_hit(&self, point: Point, inward: Point, closest: f64) -> Option<f64> {
        let denominator = dot(self.plane_normal, inward);
        if denominator.abs() < PARALLEL_EPSILON {
            return None;
        }
        let intersection = -(dot(self.plane_normal, point) + self.plane_offset) / denominator;
        let hit = std::array::from_fn(|axis| point[axis] + inward[axis] * intersection);
        let delta = difference(hit, point);
        let forward = dot(delta, inward);
        let squared_distance = dot(delta, delta);
        if !forward.is_finite() || forward <= SURFACE_EPSILON || squared_distance >= closest {
            return None;
        }

        // Significant edge determinants must agree in sign. A hit on the
        // polygon boundary is accepted.
        let [x, y] = self.axes.map(|axis| dot(hit, axis));
        let mut side = 0.0;
        let points = &self.outline[..self.corners];
        let edges = points.iter().zip(points.iter().cycle().skip(1));
        for (&[ax, ay], &[bx, by]) in edges {
            let determinant = x * ay - x * by - y * ax + y * bx + ax * by - ay * bx;
            if determinant.abs() > SURFACE_EPSILON {
                let next = determinant.signum();
                if next * side < -SURFACE_EPSILON {
                    return None;
                }
                side = next;
            }
        }
        Some(squared_distance)
    }
}

fn measure<E>(
    groups: &[Faces],
    points: &[MeshVector],
    normals: &[MeshVector],
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<f32>, PointwiseError<E>> {
    let mut polygons = Vec::new();
    for group in groups {
        polygons
            .try_reserve(group.iter().count())
            .map_err(|_| ImageError::Allocation)?;
        for face in group.iter() {
            check_render_cancelled(cancelled)?;
            let mut vertices = [[0.; 3]; 4];
            for (vertex, &corner) in vertices.iter_mut().zip(face) {
                let position = *at(&group.positions, corner).map_err(PointwiseError::Scene)?;
                *vertex = at(points, position)
                    .map_err(PointwiseError::Scene)?
                    .values()
                    .map(f64::from);
            }
            polygons.push(Polygon::new(vertices, face.len()));
        }
    }

    let mut distances = allocated(points.len(), 0.)?;
    // Normal preparation makes normal indices equal to position indices. All
    // incident corners of a position therefore share one ray and one result.
    for ((point, normal), output) in points.iter().zip(normals).zip(&mut distances) {
        let origin = point.values().map(f64::from);
        let direction = normal.values().map(|coordinate| -f64::from(coordinate));
        let mut closest = f64::MAX;
        for (index, polygon) in polygons.iter().enumerate() {
            if index % 256 == 0 {
                check_render_cancelled(cancelled)?;
            }
            if let Some(hit) = polygon.nearer_hit(origin, direction, closest) {
                closest = hit;
            }
        }
        if closest < f64::MAX {
            *output = closest.sqrt() as f32;
        }
    }
    Ok(distances)
}

pub(crate) fn bake<E>(
    models: &[MeshModel],
    geometry: ImageGeometry,
    cancelled: &dyn Fn() -> bool,
) -> Result<SharedImageMap, PointwiseError<E>> {
    bake_vertex_values(models, geometry, cancelled, measure)
}
