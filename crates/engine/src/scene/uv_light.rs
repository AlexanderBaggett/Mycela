//! UV Light's red-channel atlas and its light-space depth pass.
use super::camera::look_at;
use super::uv_apply::{BACKGROUND, allocated, unorm};
use super::uv_depth::raster_depth;
use super::uv_mesh::{AtlasMesh, area, finish_atlas, read_meshes};
use super::{MeshModel, MeshVector, Scene, SceneTransform, SceneVector};
use crate::{FloatBuffer, ImageGeometry, PointwiseError, SharedImageMap};

#[derive(Clone, Copy, Debug)]
pub struct UvLightSettings {
    position: [f32; 3],
    shadow: f32,
    illumination: f32,
}

impl UvLightSettings {
    pub(crate) fn new(position: [f64; 3], shadow: f64, illumination: f64) -> Self {
        Self {
            position: position.map(|value| value as f32),
            shadow: (shadow * 0.01) as f32,
            illumination: (illumination * 0.01) as f32,
        }
    }

    pub fn position(self) -> [f32; 3] {
        self.position
    }

    pub fn shadow_strength(self) -> f32 {
        self.shadow
    }

    pub fn global_illumination(self) -> f32 {
        self.illumination
    }

    pub fn shadow_map<E>(
        self,
        scene: &Scene,
        geometry: ImageGeometry,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Option<UvLightShadow>, PointwiseError<E>> {
        if self.shadow <= 0. {
            return Ok(None);
        }
        let models = scene.try_models_snapshot().map_err(PointwiseError::Scene)?;
        let meshes = read_meshes(&models, cancelled)?;
        UvLightShadow::prepare(self, &meshes, geometry, cancelled)
    }

    fn shade(self, position: [f32; 3], normal: [f32; 3], shadow: Option<&UvLightShadow>) -> f32 {
        let mut to_light = MeshVector::sub(vector(self.position), vector(position));
        to_light.normalize();
        let angle = to_light.dot(vector(normal));
        let shadow = shadow.map_or(1., |shadow| shadow.factor(position, self.shadow));
        (self.illumination * (angle + 1.) * 0.5 + (1. - self.illumination) * angle * shadow)
            .clamp(0., 1.)
    }
}

pub struct UvLightShadow {
    world_to_shadow: SceneTransform,
    geometry: ImageGeometry,
    depth: Vec<f32>,
}

impl UvLightShadow {
    pub fn transform(&self) -> SceneTransform {
        self.world_to_shadow
    }

    pub fn image(&self) -> Result<SharedImageMap, crate::ImageError> {
        let image = SharedImageMap::new(self.geometry);
        let mut depth = allocated(self.depth.len(), 0.)?;
        depth.copy_from_slice(&self.depth);
        image.set_buffer(0, Some(&FloatBuffer::new(depth)))?;
        Ok(image)
    }

    fn prepare<E>(
        light: UvLightSettings,
        models: &[AtlasMesh],
        target: ImageGeometry,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Option<Self>, PointwiseError<E>> {
        let Some(world_to_clip) = shadow_camera(light.position, models, target) else {
            return Ok(None);
        };
        let double = |v: i32| v.checked_mul(2).ok_or(PointwiseError::Argument);
        let geometry = ImageGeometry::new(double(target.width())?, double(target.height())?);
        let mut depth = allocated(area(geometry)?, 1.)?;
        raster_depth(
            models,
            geometry,
            world_to_clip,
            16777215.,
            &mut depth,
            cancelled,
        )?;
        let world_to_shadow = world_to_clip
            .then(SceneTransform::scale(SceneVector::new(0.5, 0.5, 0.5)))
            .then(SceneTransform::translation(SceneVector::new(0.5, 0.5, 0.5)));
        Ok(Some(Self {
            world_to_shadow,
            geometry,
            depth,
        }))
    }

    fn factor(&self, position: [f32; 3], strength: f32) -> f32 {
        let [x, y, z] = position;
        let [u, v, distance, w] = self
            .world_to_shadow
            .transform_homogeneous([f64::from(x), f64::from(y), f64::from(z), 1.])
            .map(|value| value as f32);
        if w <= 0. {
            return 1.;
        }
        let threshold = (distance - 0.01) / w;
        let [width, height] = [self.geometry.width(), self.geometry.height()];
        let axis = |p: f32, len: i32| (p * len as f32).floor().clamp(0., (len - 1) as f32) as usize;
        let mut hits = 0;
        for [dx, dy] in [
            [-0.0005, -0.0005],
            [0.0005, -0.0005],
            [-0.0005, 0.0005],
            [0.0005, 0.0005],
        ] {
            let x = axis(u / w + dx, width);
            let y = axis(v / w + dy, height);
            hits += i32::from(self.depth[y * width as usize + x] < threshold);
        }
        1. - hits as f32 * 0.25 * strength
    }
}

fn vector([x, y, z]: [f32; 3]) -> MeshVector {
    MeshVector::new(x, y, z)
}

fn shadow_camera(
    position: [f32; 3],
    meshes: &[AtlasMesh],
    geometry: ImageGeometry,
) -> Option<SceneTransform> {
    let points = || {
        meshes
            .iter()
            .flat_map(|mesh| mesh.points.iter().map(|point| point.values()))
    };
    let mut minimum = [f32::MAX; 3];
    let mut maximum = [f32::MIN; 3];
    for point in points() {
        for (axis, value) in point.into_iter().enumerate() {
            minimum[axis] = minimum[axis].min(value);
            maximum[axis] = maximum[axis].max(value);
        }
    }
    let [x, y, z] = std::array::from_fn(|i| f64::from((minimum[i] + maximum[i]) * 0.5));
    let [px, py, pz] = position.map(f64::from);
    if (px == x && py == y && pz == z) || meshes.is_empty() {
        return None;
    }
    let up = if px.abs() < 0.0001 && pz.abs() < 0.0001 {
        SceneVector::new(0., 0., py.abs())
    } else {
        SceneVector::new(0., 1., 0.)
    };
    let view = look_at(SceneVector::new(px, py, pz), SceneVector::new(x, y, z), up);
    let aspect = f64::from(geometry.width()) / f64::from(geometry.height());
    let (near, far) = (0.5, 1024.);
    let perspective = |extent: f64| {
        SceneTransform::from_values([
            near / (0.4 * aspect * extent),
            0.,
            0.,
            0.,
            0.,
            near / (0.4 * extent),
            0.,
            0.,
            0.,
            0.,
            -(far + near) / (far - near),
            -1.,
            0.,
            0.,
            -2. * far * near / (far - near),
            0.,
        ])
    };
    let initial = view.then(perspective(1.)).values();
    let mut left = f64::MAX;
    let mut right = f64::MIN;
    let mut bottom = f64::MAX;
    let mut top = f64::MIN;
    for p in points() {
        let [x, y, z] = p.map(f64::from);
        // The original fitting pass multiplies by the stored matrix rows.
        // The depth shader subsequently uses the OpenGL upload orientation.
        let row =
            |i: usize| initial[i] * x + initial[i + 1] * y + initial[i + 2] * z + initial[i + 3];
        let x = row(0) / row(12);
        let y = row(4) / row(12);
        left = left.min(x);
        right = right.max(x);
        bottom = bottom.min(y);
        top = top.max(y);
    }
    let extent = (left.max(-aspect) / aspect)
        .abs()
        .max((right.min(aspect) / aspect).abs())
        .max(bottom.max(-1.).abs())
        .max(top.min(1.).abs());
    (extent.is_finite() && extent > 0.).then(|| view.then(perspective(extent)))
}

pub(crate) fn bake<E>(
    models: &[MeshModel],
    geometry: ImageGeometry,
    light: UvLightSettings,
    cancelled: &dyn Fn() -> bool,
) -> Result<SharedImageMap, PointwiseError<E>> {
    if models.is_empty() {
        return Ok(SharedImageMap::new(geometry));
    }
    let area = area(geometry)?;
    let meshes = read_meshes(models, cancelled)?;
    let shadow = if light.shadow > 0. {
        UvLightShadow::prepare(light, &meshes, geometry, cancelled)?
    } else {
        None
    };
    let mut coverage = allocated(area, BACKGROUND)?;
    let mut red = allocated(area, 0.)?;
    for mesh in &meshes {
        mesh.raster_surfaces(geometry, cancelled, |index, surface| {
            if coverage[index] == BACKGROUND {
                red[index] = unorm(
                    light.shade(
                        surface.position.values(),
                        surface.normal.values(),
                        shadow.as_ref(),
                    ),
                    65535.,
                );
                coverage[index] = index;
            }
            Ok(())
        })?;
    }
    finish_atlas(geometry, coverage, red, cancelled)
}
