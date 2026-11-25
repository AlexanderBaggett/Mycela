//! UV Ambient Occlusion and UV Environment Occlusion share sample directions,
//! geometry and depth visibility. Each light reuses a single depth buffer.
use super::camera::look_at;
use super::uv_apply::{BACKGROUND, TexturePlane, Viewport, allocated, pad_coverage, unorm};
use super::uv_depth::raster_depth;
use super::uv_mesh::{AtlasMesh, Surface, area, read_meshes};
use super::{MeshModel, MeshVector, SceneTransform, SceneVector};
use crate::pointwise::check_render_cancelled;
use crate::{FloatBuffer, ImageGeometry, PointwiseError, SharedImageMap};
use std::f64::consts::PI;

// Quantize each batch like a 32-sampler OpenGL device with one slot reserved
// for the environment texture. The CPU path reuses a single depth buffer.
const LIGHTS_PER_BATCH: usize = 31;

#[derive(Clone, Copy)]
pub(crate) struct Occlusion {
    pub(crate) density: i32,
    pub(crate) environment: bool,
}

#[derive(Clone, Copy, Default)]
struct Accumulation {
    facing: f32,
    visible: [f32; 4],
}

struct Environment {
    planes: [TexturePlane; 4],
    ranges: [[f32; 2]; 4],
    transparent: bool,
}

impl Environment {
    fn new<E>(
        image: Option<&SharedImageMap>,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Self, PointwiseError<E>> {
        let mut ranges = [[0., 1.]; 4];
        let mut transparent = false;
        let geometry = image.map_or(ImageGeometry::new(1, 1), SharedImageMap::geometry);
        let mut planes = Vec::with_capacity(4);
        for (channel, range) in ranges.iter_mut().enumerate() {
            check_render_cancelled(cancelled)?;
            let alpha = channel == 3;
            let mut values = match image {
                Some(image) => image.channel(channel as i32, false)?.values(),
                None => vec![0.],
            };
            if alpha {
                transparent = values.len() != 1 || values[0].abs() > 1e-7;
            }
            let (min, max) = values
                .iter()
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(min, max), &v| {
                    (min.min(v), max.max(v))
                });
            if min < 0. || max > 1. {
                *range = [min, max];
                if max != min {
                    let reciprocal = 1. / (max - min);
                    for value in &mut values {
                        *value = (*value - min) * reciprocal;
                    }
                } else {
                    values.fill(0.);
                }
            }
            planes.push(TexturePlane::uploaded(
                values,
                [geometry.width(), geometry.height()],
                alpha,
            ));
        }
        Ok(Self {
            planes: planes
                .try_into()
                .map_err(|_| PointwiseError::InvalidOperation)?,
            ranges,
            transparent,
        })
    }

    fn sample<E>(&self, direction: MeshVector) -> Result<[f32; 4], PointwiseError<E>> {
        let [x, y, z] = direction.values();
        let mut u = (x / z).atan() * (0.5 / std::f32::consts::PI) + 0.5;
        if z < 0. {
            u += 0.5;
        }
        if u.is_nan() {
            u = 0.5;
        }
        let v = 0.5 - y.clamp(-1., 1.).asin() / std::f32::consts::PI;
        let mut result = [0.; 4];
        let channels = if self.transparent { 4 } else { 3 };
        for (output, plane) in result[..channels].iter_mut().zip(&self.planes) {
            *output = plane.sample([f64::from(u), f64::from(v)], [true, false])?;
        }
        Ok(result)
    }
}

fn sample_lights(radius: f64, density: i32) -> Vec<MeshVector> {
    let step = 180. / f64::from(3 + density * 2);
    let mut lights = vec![
        MeshVector::new(0., radius as f32, 0.),
        MeshVector::new(0., -radius as f32, 0.),
    ];
    let mut polar = step;
    while polar < 179.9999 {
        let angle = polar / 180. * PI;
        let (sine, cosine) = angle.sin_cos();
        let azimuth_step = 360. / (f64::from(4 + density * 2) * sine).ceil();
        let mut azimuth = 0.;
        while azimuth < 359.9999 {
            let (x, z) = (azimuth / 180. * PI).sin_cos();
            lights.push(MeshVector::new(
                (-radius * x * sine) as f32,
                (radius * cosine) as f32,
                (radius * z * sine) as f32,
            ));
            azimuth += azimuth_step;
        }
        polar += step;
    }
    lights
}

fn light_camera(light: MeshVector, geometry: ImageGeometry) -> SceneTransform {
    let [x, y, z] = light.values().map(f64::from);
    let up = if x == 0. && z == 0. {
        SceneVector::new(0., 0., 1.)
    } else {
        SceneVector::new(0., 1., 0.)
    };
    let (near, far) = (1., 1024.);
    let aspect = f64::from(geometry.width()) / f64::from(geometry.height());
    look_at(SceneVector::new(x, y, z), SceneVector::default(), up).then(
        SceneTransform::from_values([
            2.5 / aspect,
            0.,
            0.,
            0.,
            0.,
            2.5,
            0.,
            0.,
            0.,
            0.,
            -(far + near) / (far - near),
            -1.,
            0.,
            0.,
            -2. * near * far / (far - near),
            0.,
        ]),
    )
}

fn center_meshes(meshes: &mut [AtlasMesh]) -> f64 {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for point in meshes.iter().flat_map(|mesh| &mesh.points) {
        for (axis, value) in point.values().into_iter().enumerate() {
            min[axis] = min[axis].min(value);
            max[axis] = max[axis].max(value);
        }
    }
    let [x, y, z] = std::array::from_fn(|axis| (min[axis] + max[axis]) * 0.5);
    let center = MeshVector::new(x, y, z);
    for point in meshes.iter_mut().flat_map(|mesh| &mut mesh.points) {
        *point = MeshVector::sub(*point, center);
    }
    f64::from((0..3).map(|axis| max[axis] - min[axis]).fold(0., f32::max)) * 2.
}

fn atlas<E>(
    meshes: &[AtlasMesh],
    geometry: ImageGeometry,
    cancelled: &dyn Fn() -> bool,
) -> Result<(Vec<usize>, Vec<Surface>), PointwiseError<E>> {
    let count = area(geometry)?;
    let mut coverage = allocated(count, BACKGROUND)?;
    let mut surfaces = allocated(count, Surface::default())?;
    for mesh in meshes {
        mesh.raster_surfaces(geometry, cancelled, |pixel, surface| {
            coverage[pixel] = pixel;
            surfaces[pixel] = surface;
            Ok(())
        })?;
    }
    Ok((coverage, surfaces))
}

struct DepthView<'a> {
    rows: &'a [f32],
    transform: SceneTransform,
    dimensions: [i32; 2],
}

impl DepthView<'_> {
    fn visibility(&self, point: MeshVector) -> Option<f32> {
        let [x, y, z] = point.values().map(f64::from);
        let [u, v, distance, w] = self
            .transform
            .transform_homogeneous([x, y, z, 1.])
            .map(|v| v as f32);
        if w <= 0. {
            return None;
        }
        let threshold = (distance - 0.05) / w;
        let [width, height] = self.dimensions;
        let [u, v] = [u / w * width as f32, v / w * height as f32];
        let axis = |value: f32, length: i32| value.floor().clamp(0., (length - 1) as f32) as usize;
        let taps = [
            [u - 1., v - 1.],
            [u + 1., v - 1.],
            [u - 1., v + 1.],
            [u + 1., v + 1.],
        ];
        let hits = taps
            .into_iter()
            .filter(|&[x, y]| {
                self.rows[axis(y, height) * width as usize + axis(x, width)] >= threshold
            })
            .count();
        Some(hits as f32 * 0.25)
    }
}

pub(crate) fn bake<E>(
    models: &[MeshModel],
    geometry: ImageGeometry,
    settings: Occlusion,
    texture: Option<&SharedImageMap>,
    cancelled: &dyn Fn() -> bool,
) -> Result<SharedImageMap, PointwiseError<E>> {
    let output = SharedImageMap::new(geometry);
    output.set_constant(3, 1.)?;
    if models.is_empty() {
        return Ok(output);
    }
    let mut meshes = read_meshes(models, cancelled)?;
    let radius = center_meshes(&mut meshes);
    let (coverage, surfaces) = atlas(&meshes, geometry, cancelled)?;
    let mut accumulation = allocated(surfaces.len(), Accumulation::default())?;
    let mut depth = allocated(surfaces.len(), 1.)?;
    let environment = settings
        .environment
        .then(|| Environment::new(texture, cancelled))
        .transpose()?;
    if radius > 0. && radius.is_finite() {
        let lights = sample_lights(radius, settings.density.clamp(0, 100));
        let mut sums = allocated(surfaces.len(), Accumulation::default())?;
        for lights_in_batch in lights.chunks(LIGHTS_PER_BATCH) {
            sums.fill(Accumulation::default());
            let weight = 1. / lights_in_batch.len() as f32;
            for &light in lights_in_batch {
                check_render_cancelled(cancelled)?;
                let camera = light_camera(light, geometry);
                depth.fill(1.);
                raster_depth(&meshes, geometry, camera, 65535., &mut depth, cancelled)?;
                let depth = DepthView {
                    rows: &depth,
                    transform: camera
                        .then(SceneTransform::scale(SceneVector::new(0.5, 0.5, 0.5)))
                        .then(SceneTransform::translation(SceneVector::new(0.5, 0.5, 0.5))),
                    dimensions: [geometry.width(), geometry.height()],
                };
                for (pixel, (surface, sum)) in surfaces.iter().zip(&mut sums).enumerate() {
                    if pixel % 1024 == 0 {
                        check_render_cancelled(cancelled)?;
                    }
                    if coverage[pixel] == BACKGROUND {
                        continue;
                    }
                    let mut direction = MeshVector::sub(light, surface.position);
                    if direction.dot(surface.normal) <= 0. {
                        continue;
                    }
                    let Some(visibility) = depth.visibility(surface.position) else {
                        continue;
                    };
                    sum.facing += weight;
                    if visibility == 0. {
                        continue;
                    }
                    if let Some(environment) = &environment {
                        direction.normalize();
                        let sample = environment.sample(direction)?;
                        for (total, value) in sum.visible.iter_mut().zip(sample) {
                            *total += value * (visibility * weight);
                        }
                    } else {
                        sum.visible[0] += visibility * weight;
                    }
                }
            }
            let fraction = lights_in_batch.len() as f32 / lights.len() as f32;
            for (total, sum) in accumulation.iter_mut().zip(&sums) {
                total.facing += unorm(sum.facing, 65535.) * fraction;
                for (total, value) in total.visible.iter_mut().zip(sum.visible) {
                    *total += unorm(value, 65535.) * fraction;
                }
            }
        }
    }
    complete(
        output,
        geometry,
        coverage,
        &accumulation,
        environment.as_ref(),
        cancelled,
    )
}

fn complete<E>(
    output: SharedImageMap,
    geometry: ImageGeometry,
    coverage: Vec<usize>,
    sums: &[Accumulation],
    environment: Option<&Environment>,
    cancelled: &dyn Fn() -> bool,
) -> Result<SharedImageMap, PointwiseError<E>> {
    let dimensions = [geometry.width(), geometry.height()];
    let viewport = Viewport::new(geometry);
    let transparent = environment.is_some_and(|e| e.transparent);
    let mut alpha_plane = coverage
        .iter()
        .zip(sums)
        .map(|(&index, sum)| {
            if index == BACKGROUND {
                0.
            } else if transparent && sum.facing > 0.0001 {
                sum.visible[3] / sum.facing
            } else if transparent {
                0.
            } else {
                1.
            }
        })
        .collect::<Vec<_>>();
    let opaque = coverage
        .iter()
        .map(|&index| if index == BACKGROUND { 0. } else { 1. })
        .collect();
    let coverage_plane = TexturePlane::uploaded(opaque, dimensions, false);
    let mut indices = allocated(coverage.len(), BACKGROUND)?;
    for (pixel, index) in indices.iter_mut().enumerate() {
        if pixel % 1024 == 0 {
            check_render_cancelled(cancelled)?;
        }
        let uv = viewport.uv(pixel, geometry.width());
        if (0. ..1.).contains(&uv[0]) && coverage_plane.sample(uv, [true, true])? > 0.0001 {
            *index = pixel;
        }
    }
    pad_coverage(&mut indices, geometry, cancelled)?;
    for channel in 0..4 {
        if environment.is_none() && matches!(channel, 1 | 2) {
            continue;
        }
        let alpha = channel == 3;
        let source = if alpha {
            std::mem::take(&mut alpha_plane)
        } else {
            sums.iter()
                .map(|sum| {
                    if sum.facing > 0.0001 {
                        sum.visible[channel] / sum.facing
                    } else {
                        0.
                    }
                })
                .collect()
        };
        let plane = TexturePlane::uploaded(source, dimensions, false);
        let [min, max] = environment.map_or([0., 1.], |e| e.ranges[channel]);
        let mut values = allocated(indices.len(), if alpha { 1. } else { 0. })?;
        for (index, (&pixel, value)) in indices.iter().zip(&mut values).enumerate() {
            if index % 1024 == 0 {
                check_render_cancelled(cancelled)?;
            }
            if pixel != BACKGROUND {
                let sampled = unorm(
                    plane.sample(viewport.uv(pixel, geometry.width()), [true, true])?,
                    65535.,
                );
                *value = (if alpha { 1. - sampled } else { sampled }) * (max - min) + min;
            } else if environment.is_some() {
                *value = *value * (max - min) + min;
            }
        }
        output.set_buffer(channel as i32, Some(&FloatBuffer::new(values)))?;
    }
    Ok(output)
}
