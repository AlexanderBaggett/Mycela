//! Execution-only boundary between the retained CPU graph and a real image
//! renderer. The engine owns geometry and evaluated properties; the application
//! supplies its device, camera, cancellation and actual allocation services.
use super::{PointwiseError, SharedImageMap};
use crate::{ImageGeometry, Scene, SceneAllocationRequest, SceneError, UvLightSettings};

/// A real scene backend borrowed by one graph evaluation. Implementations own
/// their execution resources rather than adding another graph or scene cache.
///
/// Allocation callbacks may reenter the retained graph. They must return after
/// releasing policy/resource locks. The engine evaluates the source transform
/// properties after geometry construction, preserving those callbacks' effects.
/// Rendering receives the actual generated scene and runs outside model guards.
pub trait SceneImageRenderer<E> {
    /// Admit the actual element count, byte count and element type at each source
    /// array allocation. Failure is returned before the corresponding allocation.
    fn preflight_array(&mut self, request: SceneAllocationRequest) -> Result<(), SceneError>;

    /// Query the active device's real maximum 2D texture size. A backend that
    /// cannot supply this source material-assignment dependency must fail.
    fn material_texture_limit(&mut self) -> Result<u32, PointwiseError<E>> {
        Err(PointwiseError::UnsupportedCapability(
            "material texture size service",
        ))
    }

    /// Render the actual scene into normal engine image channels. The caller's
    /// retained camera and selected device belong to this backend. Device or
    /// missing-capability errors must remain errors rather than successful empty
    /// frames. Graph finalization retains its normal ownership and seal behavior.
    fn render_scene_image(
        &mut self,
        scene: &Scene,
        width: i32,
        height: i32,
    ) -> Result<SharedImageMap, PointwiseError<E>>;

    /// Execute one validated Mycela fragment shader over the requested image.
    fn render_shader_image(
        &mut self,
        _program: &super::ShaderProgram,
        _inputs: &[Option<SharedImageMap>],
        _geometry: ImageGeometry,
        _time: f64,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        Err(PointwiseError::UnsupportedCapability("Shader renderer"))
    }

    /// Shade the source's UV vertex-map materials in a single camera layer.
    fn render_uv_vertex_map_image(
        &mut self,
        scene: &Scene,
        geometry: ImageGeometry,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        self.render_scene_image(scene, geometry.width(), geometry.height())
    }

    /// Shade the source's UV Environment Light materials with the scene's
    /// usual layers and the atlas-compatible camera readback.
    fn render_uv_material_image(
        &mut self,
        scene: &Scene,
        geometry: ImageGeometry,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        self.render_scene_image(scene, geometry.width(), geometry.height())
    }

    /// Render the camera view using the UV Light node's temporary shader.
    fn render_uv_light_image(
        &mut self,
        _scene: &Scene,
        _geometry: ImageGeometry,
        _light: UvLightSettings,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        Err(PointwiseError::UnsupportedCapability(
            "UV Light camera shading",
        ))
    }
}
