//! Worker-owned adapter from the canonical graph to the offscreen renderer.
//! No live view, graph owner or GPUI context is retained by this backend.
use crate::{
    ForegroundError,
    filter_pipeline::{RenderError, failure},
};
use mycela_engine::{
    CameraError, ImageError, ImageGeometry, PointwiseError, PointwiseFinalExportCache,
    PointwiseNodeRenderSnapshot, PointwiseRenderEffects, PointwiseRenderOptions, RenderedImage,
    Scene, SceneAllocationRequest, SceneCamera, SceneError, SceneImageRenderer, SharedImageMap,
    UvLightSettings,
};
use mycela_renderer::{
    SceneDepthPrecision, SceneLayerColor, SceneRenderError, SceneRenderRequest, SceneRenderer,
    SceneTargetProfile,
};
use std::{
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

pub(crate) const GRAPH_PNG_OPTIONS: PointwiseRenderOptions =
    PointwiseRenderOptions::new(false, true);

/// A detached camera owner and its captured scalar values. Rendering uses a
/// further source copy, so projection setup cannot mutate the saved frame.
#[derive(Clone)]
pub(crate) struct CameraFrame {
    camera: SceneCamera<ForegroundError>,
    values: CameraValues,
}

#[derive(Clone, PartialEq)]
struct CameraValues {
    orbit: Option<[u64; 3]>,
    target: [u64; 3],
    near: u64,
    far: u64,
    frustum: [u64; 4],
    auto_view: bool,
    auto_projection: bool,
}

impl CameraFrame {
    pub(crate) fn capture(camera: &SceneCamera<ForegroundError>) -> Result<Self, RenderError> {
        let camera = camera.source_copy().map_err(failure)?;
        let values = CameraValues {
            orbit: camera.orbit_values().map(|v| v.map(f64::to_bits)),
            target: camera.target().values().map(f64::to_bits),
            near: camera.near().to_bits(),
            far: camera.far().to_bits(),
            frustum: camera.frustum().map(f64::to_bits),
            auto_view: camera.is_view_automatic(),
            auto_projection: camera.is_projection_automatic(),
        };
        Ok(Self { camera, values })
    }

    pub(crate) fn same_values(&self, other: &Self) -> bool {
        self.values == other.values
    }
}

#[derive(Default)]
pub(crate) struct PreviewDevices {
    renderer: Option<SceneRenderer>,
    shader: Option<mycela_renderer::ShaderRenderer>,
}

struct Backend<'a> {
    devices: &'a mut PreviewDevices,
    camera: SceneCamera<ForegroundError>,
    cancelled: &'a dyn Fn() -> bool,
    // The required atomic is inactive; the borrowed predicate above is the
    // actual generation/closed-state authority at renderer checkpoints.
    inactive: AtomicBool,
    deadline: Instant,
    failure: Option<RenderError>,
}
enum CameraShading {
    Standard,
    UvLight(UvLightSettings),
    UvVertexMap,
    UvMaterial,
}

impl Backend<'_> {
    fn ensure_renderer(&mut self) -> Result<(), PointwiseError<ForegroundError>> {
        if (self.cancelled)() {
            self.failure = Some(RenderError::Cancelled);
            return Err(PointwiseError::UnsupportedCapability(
                "scene renderer cancelled",
            ));
        }
        if self.devices.renderer.is_none() {
            let profile = SceneTargetProfile::new(
                SceneLayerColor::GeneticaOpenGl8,
                SceneDepthPrecision::Float32,
            );
            match pollster::block_on(SceneRenderer::new(profile)) {
                Ok(renderer) => self.devices.renderer = Some(renderer),
                Err(error) => {
                    self.failure = Some(failure(format!(
                        "Couldn’t initialize the scene renderer: {error}"
                    )));
                    return Err(PointwiseError::UnsupportedCapability(
                        "scene renderer unavailable",
                    ));
                }
            }
        }
        Ok(())
    }
    fn render_camera(
        &mut self,
        scene: &Scene,
        geometry: ImageGeometry,
        shading: CameraShading,
    ) -> Result<SharedImageMap, PointwiseError<ForegroundError>> {
        self.ensure_renderer()?;
        let width = u32::try_from(geometry.width()).map_err(|_| PointwiseError::Argument)?;
        let height = u32::try_from(geometry.height()).map_err(|_| PointwiseError::Argument)?;
        let aspect = f64::from(width) / f64::from(height);
        if let Err(error) = self.camera.set_frustum([-aspect, aspect, 1., -1.]) {
            let projected = project(&SceneRenderError::Camera(error.clone()));
            self.failure = Some(failure(format!("Couldn’t set the scene camera: {error}")));
            return Err(projected);
        }
        let request = SceneRenderRequest::source_node(width, height, &self.inactive, self.deadline)
            .with_cancel_check(self.cancelled);
        let shadow = match shading {
            CameraShading::UvLight(light) => light.shadow_map(scene, geometry, self.cancelled)?,
            _ => None,
        };
        let request = match shading {
            CameraShading::Standard => request,
            CameraShading::UvVertexMap => request.with_uv_vertex_map(),
            CameraShading::UvLight(light) => request.with_uv_light(light, shadow.as_ref()),
            CameraShading::UvMaterial => request.with_uv_material_camera(),
        };
        match self
            .devices
            .renderer
            .as_ref()
            .expect("renderer was initialized")
            .render(scene, &self.camera, &request)
        {
            Ok(frame) => frame.into_image().map_err(PointwiseError::Image),
            Err(error) => {
                let projected = project(&error);
                self.failure = Some(match error {
                    SceneRenderError::Cancelled => RenderError::Cancelled,
                    other => failure(format!("Couldn’t render the scene: {other}")),
                });
                Err(projected)
            }
        }
    }
}
impl SceneImageRenderer<ForegroundError> for Backend<'_> {
    fn render_shader_image(
        &mut self,
        program: &mycela_engine::ShaderProgram,
        inputs: &[Option<SharedImageMap>],
        geometry: ImageGeometry,
        time: f64,
    ) -> Result<SharedImageMap, PointwiseError<ForegroundError>> {
        let result = pollster::block_on(async {
            if self.devices.shader.is_none() {
                self.devices.shader = Some(mycela_renderer::ShaderRenderer::new().await?);
            }
            self.devices
                .shader
                .as_ref()
                .expect("initialized renderer")
                .render(
                    program,
                    inputs,
                    geometry,
                    time,
                    self.cancelled,
                    self.deadline,
                )
                .await
        });
        result.map_err(|message| {
            self.failure = Some(if (self.cancelled)() {
                RenderError::Cancelled
            } else {
                failure(&message)
            });
            PointwiseError::Shader(message)
        })
    }

    fn material_texture_limit(&mut self) -> Result<u32, PointwiseError<ForegroundError>> {
        self.ensure_renderer()?;
        Ok(self
            .devices
            .renderer
            .as_ref()
            .ok_or(PointwiseError::UnsupportedCapability(
                "scene renderer unavailable",
            ))?
            .max_material_texture_dimension())
    }

    fn preflight_array(&mut self, request: SceneAllocationRequest) -> Result<(), SceneError> {
        if (self.cancelled)() {
            self.failure = Some(RenderError::Cancelled);
            return Err(SceneError::MissingAllocationServices);
        }
        let available = mycela_system_memory::available_physical_bytes()
            .map_err(|_| SceneError::MissingAllocationServices)?;
        if request.byte_count() as u64 > available {
            Err(SceneError::Allocation)
        } else {
            Ok(())
        }
    }

    fn render_scene_image(
        &mut self,
        scene: &Scene,
        width: i32,
        height: i32,
    ) -> Result<SharedImageMap, PointwiseError<ForegroundError>> {
        self.render_camera(
            scene,
            ImageGeometry::new(width, height),
            CameraShading::Standard,
        )
    }

    fn render_uv_vertex_map_image(
        &mut self,
        scene: &Scene,
        geometry: ImageGeometry,
    ) -> Result<SharedImageMap, PointwiseError<ForegroundError>> {
        self.render_camera(scene, geometry, CameraShading::UvVertexMap)
    }

    fn render_uv_material_image(
        &mut self,
        scene: &Scene,
        geometry: ImageGeometry,
    ) -> Result<SharedImageMap, PointwiseError<ForegroundError>> {
        self.render_camera(scene, geometry, CameraShading::UvMaterial)
    }

    fn render_uv_light_image(
        &mut self,
        scene: &Scene,
        geometry: ImageGeometry,
        light: UvLightSettings,
    ) -> Result<SharedImageMap, PointwiseError<ForegroundError>> {
        self.render_camera(scene, geometry, CameraShading::UvLight(light))
    }
}
fn project(error: &SceneRenderError<ForegroundError>) -> PointwiseError<ForegroundError> {
    match error {
        SceneRenderError::Camera(CameraError::Callback(error)) => {
            PointwiseError::Observer(error.clone())
        }
        SceneRenderError::Camera(CameraError::NullMatrixFactory)
        | SceneRenderError::MissingCameraMatrix => PointwiseError::NullReference,
        SceneRenderError::Scene(error) => PointwiseError::Scene(*error),
        SceneRenderError::Image(error) => PointwiseError::Image(*error),
        SceneRenderError::Allocation | SceneRenderError::InsufficientMemory => {
            PointwiseError::Image(ImageError::Allocation)
        }
        SceneRenderError::InvalidGeometry => PointwiseError::Argument,
        SceneRenderError::InvalidMesh => PointwiseError::Scene(SceneError::IndexOutOfRange),
        // The canonical graph can catch source-shaped errors and return an empty
        // image. Preserve actual backend errors separately and reject that result.
        SceneRenderError::MemoryQuery(_)
        | SceneRenderError::UnsupportedFeature(_)
        | SceneRenderError::Device(_)
        | SceneRenderError::Cancelled
        | SceneRenderError::TimedOut => {
            PointwiseError::UnsupportedCapability("scene renderer failed")
        }
    }
}

/// Evaluate one detached graph using a lazy device backend. Non-scene graphs do
/// not request a device; renderer failures cannot become caught blank images.
pub(crate) fn render(
    snapshot: &PointwiseNodeRenderSnapshot<ForegroundError>,
    width: i32,
    height: i32,
    cancelled: &impl Fn() -> bool,
    options: PointwiseRenderOptions,
    library: Option<&mycela_engine::EnvironmentResourceLibrary>,
) -> Result<SharedImageMap, RenderError> {
    let camera = CameraFrame::capture(&SceneCamera::source_preview())?;
    render_with_camera(
        snapshot, width, height, cancelled, options, &camera, library,
    )
}

pub(crate) fn render_with_camera(
    snapshot: &PointwiseNodeRenderSnapshot<ForegroundError>,
    width: i32,
    height: i32,
    cancelled: &impl Fn() -> bool,
    options: PointwiseRenderOptions,
    camera: &CameraFrame,
    library: Option<&mycela_engine::EnvironmentResourceLibrary>,
) -> Result<SharedImageMap, RenderError> {
    evaluate(cancelled, Some(camera), library, |effects| {
        snapshot.render_with_execution_and_effects(
            width,
            height,
            crate::export_render::generator_execution(),
            options,
            effects,
        )
    })?
    .still_image()
    .ok_or_else(|| failure("The graph returned no still image."))
}

pub(crate) fn render_graph_playback(
    snapshot: &PointwiseNodeRenderSnapshot<ForegroundError>,
    geometry: mycela_engine::ImageGeometry,
    cancelled: &impl Fn() -> bool,
    library: Option<&mycela_engine::EnvironmentResourceLibrary>,
) -> Result<RenderedImage, RenderError> {
    evaluate(cancelled, None, library, |effects| {
        snapshot.render_export_playback_with_effects(
            geometry,
            crate::export_render::generator_execution(),
            GRAPH_PNG_OPTIONS,
            effects,
        )
    })
}

pub(crate) fn render_thumbnail_sample(
    renderer: &mycela_engine::PointwiseThumbnailRender<ForegroundError>,
    time: f64,
    animated: bool,
    cache: &mycela_engine::PointwiseThumbnailCache,
    devices: &mut PreviewDevices,
    cancelled: &impl Fn() -> bool,
    library: &mycela_engine::EnvironmentResourceLibrary,
) -> Result<RenderedImage, RenderError> {
    evaluate_with_devices(devices, cancelled, None, Some(library), None, |effects| {
        renderer.render_sample(
            time,
            animated,
            crate::export_render::generator_execution(),
            cache,
            effects,
        )
    })
}

pub(crate) fn render_sequence_frame(
    renderer: &mut mycela_engine::PointwiseSequenceRender<ForegroundError>,
    time: f64,
    geometry: mycela_engine::ImageGeometry,
    devices: &mut PreviewDevices,
    cancelled: &impl Fn() -> bool,
    library: &mycela_engine::EnvironmentResourceLibrary,
) -> Result<SharedImageMap, RenderError> {
    let execution = crate::export_render::generator_execution();
    let image = evaluate_with_devices(devices, cancelled, None, Some(library), None, |effects| {
        renderer.render_frame(time, execution, effects)
    })?
    .still_image()
    .ok_or_else(|| failure("The graph returned no frame at the requested time."))?;
    let image = mycela_engine::downsample_render::<ForegroundError>(&image, geometry, execution)
        .map_err(failure)?;
    if cancelled() {
        return Err(RenderError::Cancelled);
    }
    Ok(image)
}

pub(crate) fn render_export(
    snapshot: &PointwiseNodeRenderSnapshot<ForegroundError>,
    geometry: mycela_engine::ImageGeometry,
    cancelled: &impl Fn() -> bool,
    options: PointwiseRenderOptions,
    camera: Option<&CameraFrame>,
    cache: Option<&PointwiseFinalExportCache<ForegroundError>>,
    library: Option<&mycela_engine::EnvironmentResourceLibrary>,
) -> Result<SharedImageMap, RenderError> {
    render_export_observed(
        snapshot, geometry, cancelled, options, camera, cache, library, None,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn render_export_observed(
    snapshot: &PointwiseNodeRenderSnapshot<ForegroundError>,
    geometry: mycela_engine::ImageGeometry,
    cancelled: &impl Fn() -> bool,
    options: PointwiseRenderOptions,
    camera: Option<&CameraFrame>,
    cache: Option<&PointwiseFinalExportCache<ForegroundError>>,
    library: Option<&mycela_engine::EnvironmentResourceLibrary>,
    progress: Option<mycela_engine::GraphProgressObserver<'_>>,
) -> Result<SharedImageMap, RenderError> {
    evaluate_with_devices(
        &mut PreviewDevices::default(),
        cancelled,
        camera,
        library,
        progress,
        |effects| {
            let execution = crate::export_render::generator_execution();
            if let (Some(cache), None) = (cache, camera) {
                cache.render_snapshot_with_effects(snapshot, geometry, execution, options, effects)
            } else {
                snapshot.render_export_with_effects(geometry, execution, options, effects)
            }
        },
    )?
    .still_image()
    .ok_or_else(|| failure("The graph returned no still image."))
}

pub(crate) fn render_live(
    node: &mycela_engine::PointwiseNode<ForegroundError>,
    width: i32,
    height: i32,
    time: f64,
    cancelled: &impl Fn() -> bool,
    options: PointwiseRenderOptions,
) -> Result<SharedImageMap, RenderError> {
    evaluate(cancelled, None, None, |effects| {
        node.render_original_with_effects(
            width,
            height,
            options,
            time,
            crate::export_render::generator_execution(),
            effects,
        )
    })?
    .still_image()
    .ok_or_else(|| failure("The graph returned no still image."))
}

pub(super) fn render_canvas_tile(
    snapshot: &PointwiseNodeRenderSnapshot<ForegroundError>,
    geometry: mycela_engine::ImageGeometry,
    cancelled: &impl Fn() -> bool,
    library: Option<&mycela_engine::EnvironmentResourceLibrary>,
) -> Result<SharedImageMap, RenderError> {
    evaluate(cancelled, None, library, |effects| {
        snapshot.render_canvas_tile_with_effects(
            geometry,
            crate::export_render::generator_execution(),
            GRAPH_PNG_OPTIONS,
            effects,
        )
    })?
    .still_image()
    .ok_or_else(|| failure("Canvas returned no tile."))
}

pub(crate) fn render_synthesis_preview(
    snapshot: Option<&PointwiseNodeRenderSnapshot<ForegroundError>>,
    texture: Option<&mycela_engine::SynthesisRecord>,
    layer: Option<usize>,
    geometry: mycela_engine::ImageGeometry,
    cancelled: &impl Fn() -> bool,
    library: &mycela_engine::EnvironmentResourceLibrary,
) -> Result<SharedImageMap, RenderError> {
    evaluate(cancelled, None, Some(library), |effects| {
        let execution = crate::export_render::generator_execution();
        if let Some(texture) = texture {
            return texture.render_editor_preview_with_effects(
                snapshot,
                layer,
                geometry,
                execution,
                GRAPH_PNG_OPTIONS,
                effects,
            );
        }
        let snapshot = snapshot.ok_or(PointwiseError::InvalidCast)?;
        if let Some(layer) = layer {
            snapshot.render_synthesis_layer_with_effects(
                layer,
                geometry,
                execution,
                GRAPH_PNG_OPTIONS,
                effects,
            )
        } else {
            snapshot.render_export_with_effects(geometry, execution, GRAPH_PNG_OPTIONS, effects)
        }
    })?
    .still_image()
    .ok_or_else(|| failure("Texture Synthesis returned no image."))
}

fn evaluate(
    cancelled: &impl Fn() -> bool,
    camera: Option<&CameraFrame>,
    library: Option<&mycela_engine::EnvironmentResourceLibrary>,
    execute: impl FnOnce(
        &mut PointwiseRenderEffects<'_, ForegroundError>,
    ) -> Result<
        mycela_engine::PointwiseRenderOutcome<ForegroundError>,
        PointwiseError<ForegroundError>,
    >,
) -> Result<RenderedImage, RenderError> {
    evaluate_with_devices(
        &mut PreviewDevices::default(),
        cancelled,
        camera,
        library,
        None,
        execute,
    )
}

fn evaluate_with_devices(
    devices: &mut PreviewDevices,
    cancelled: &impl Fn() -> bool,
    camera: Option<&CameraFrame>,
    library: Option<&mycela_engine::EnvironmentResourceLibrary>,
    progress: Option<mycela_engine::GraphProgressObserver<'_>>,
    execute: impl FnOnce(
        &mut PointwiseRenderEffects<'_, ForegroundError>,
    ) -> Result<
        mycela_engine::PointwiseRenderOutcome<ForegroundError>,
        PointwiseError<ForegroundError>,
    >,
) -> Result<RenderedImage, RenderError> {
    if cancelled() {
        return Err(RenderError::Cancelled);
    }
    let mut backend = Backend {
        devices,
        camera: match camera {
            Some(frame) => frame.camera.source_copy().map_err(failure)?,
            None => SceneCamera::source_preview(),
        },
        cancelled,
        inactive: AtomicBool::new(false),
        deadline: Instant::now() + Duration::from_secs(120),
        failure: None,
    };
    let mut capacity = crate::memory_capacity::query;
    let mut preflight = crate::memory_capacity::preflight;
    let outcome = {
        let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight)
            .with_cancel_check(cancelled)
            .with_scene_renderer(&mut backend)
            .with_environment_library(library)
            .with_graph_progress(progress);
        execute(&mut effects)
    };
    if let Some(error) = backend.failure {
        return Err(error);
    }
    if cancelled() {
        return Err(RenderError::Cancelled);
    }
    let outcome = outcome.map_err(|error| match error {
        PointwiseError::Cancelled => RenderError::Cancelled,
        PointwiseError::UnsupportedCapability("Group contents and variable evaluation") => {
            failure("Rendering groups is not available yet.")
        }
        PointwiseError::UnsupportedCapability("Reference group scheduling") => {
            failure("Rendering group references is not available yet.")
        }
        error => failure(error),
    })?;
    if let Some(error) = outcome.caught_errors().first() {
        return Err(failure(format!("Couldn’t render the graph: {error}")));
    }
    outcome
        .output()
        .ok_or_else(|| failure("The graph returned no image."))
}

#[cfg(test)]
mod image_scene_tests {
    use super::*;
    use mycela_engine::{
        ImageGeometry, PointwiseFilter, PointwiseKind, PointwiseNode, ProceduralExecution,
        SceneClearLightsNode, SceneCombineNode, SceneTransformNode,
    };

    fn graph(
        color: [f64; 3],
        transparency: f64,
        transform: bool,
    ) -> PointwiseNode<ForegroundError> {
        let image = PointwiseNode::solid_color().unwrap();
        image.set_solid_color(color).unwrap();
        image.set_solid_transparency(transparency).unwrap();
        let scene = if transform {
            SceneTransformNode::new().unwrap().node()
        } else {
            SceneClearLightsNode::new().node()
        };
        scene.slot(0).unwrap().set_node(Some(&image)).unwrap();
        scene
    }

    #[test]
    fn direct_image_path_requires_a_backend_for_all_render_options() {
        let root = graph([1., 0., 0.], 0.25, false);
        let image = root.slot(0).unwrap().node().unwrap();
        let cache = SharedImageMap::new(ImageGeometry::new(4, 4));
        let mut capacity = crate::memory_capacity::query;
        let mut preflight = crate::memory_capacity::preflight;
        let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight);
        for retained in [None, Some(cache)] {
            image.set_cached_image(retained).unwrap();
            for color_only in [false, true] {
                for channels in [false, true] {
                    assert!(matches!(
                        root.render_original_with_effects(
                            4,
                            4,
                            PointwiseRenderOptions::new(color_only, channels),
                            0.,
                            ProceduralExecution::CallingThread,
                            &mut effects
                        ),
                        Err(PointwiseError::Scene(SceneError::MissingRenderer))
                    ));
                }
            }
        }
        let combined = SceneCombineNode::new().node();
        let image = PointwiseNode::solid_color().unwrap();
        combined.slot(0).unwrap().set_node(Some(&image)).unwrap();
        assert!(matches!(
            combined.render_original_with_effects(
                4,
                4,
                PointwiseRenderOptions::new(false, false),
                0.,
                ProceduralExecution::CallingThread,
                &mut effects
            ),
            Err(PointwiseError::Scene(SceneError::MissingRenderer))
        ));
    }

    #[test]
    fn nested_cached_image_material_requires_a_backend() {
        let nested = graph([1., 0., 0.], 0.25, true);
        let image = nested.slot(0).unwrap().node().unwrap();
        image
            .set_cached_image(Some(SharedImageMap::new(ImageGeometry::new(4, 4))))
            .unwrap();
        let combine = SceneCombineNode::new().node();
        combine.slot(0).unwrap().set_node(Some(&nested)).unwrap();
        let clear = SceneClearLightsNode::new().node();
        clear.slot(0).unwrap().set_node(Some(&combine)).unwrap();
        let mut capacity = crate::memory_capacity::query;
        let mut preflight = crate::memory_capacity::preflight;
        let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight);
        assert!(matches!(
            clear.render_original_with_effects(
                4,
                4,
                PointwiseRenderOptions::new(false, false),
                0.,
                ProceduralExecution::CallingThread,
                &mut effects
            ),
            Err(PointwiseError::Scene(SceneError::MissingRenderer))
        ));
    }

    #[test]
    fn shared_image_material_requires_a_backend() {
        let image = PointwiseNode::solid_color().unwrap();
        let combine = SceneCombineNode::new().node();
        combine
            .slot(0)
            .unwrap()
            .set_node_quiet(Some(&image))
            .unwrap();
        combine
            .slot(1)
            .unwrap()
            .set_node_quiet(Some(&image))
            .unwrap();
        assert!(
            combine
                .slot(0)
                .unwrap()
                .node()
                .unwrap()
                .same_reference(&image)
        );
        assert!(
            combine
                .slot(1)
                .unwrap()
                .node()
                .unwrap()
                .same_reference(&image)
        );
        let mut capacity = crate::memory_capacity::query;
        let mut preflight = crate::memory_capacity::preflight;
        let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight);
        assert!(matches!(
            combine.render_original_with_effects(
                4,
                4,
                PointwiseRenderOptions::new(false, false),
                0.,
                ProceduralExecution::CallingThread,
                &mut effects
            ),
            Err(PointwiseError::Scene(SceneError::MissingRenderer))
        ));
    }

    #[test]
    fn shared_dag_inside_one_image_subtree_reaches_the_existing_renderer() {
        let image = PointwiseNode::solid_color().unwrap();
        let add = PointwiseFilter::new(PointwiseKind::Add).unwrap().node();
        add.slot(0).unwrap().set_node_quiet(Some(&image)).unwrap();
        add.slot(1).unwrap().set_node_quiet(Some(&image)).unwrap();
        let clear = SceneClearLightsNode::new().node();
        clear.slot(0).unwrap().set_node(Some(&add)).unwrap();
        let mut capacity = crate::memory_capacity::query;
        let mut preflight = crate::memory_capacity::preflight;
        let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight);
        assert!(matches!(
            clear.render_original_with_effects(
                4,
                4,
                PointwiseRenderOptions::new(false, false),
                0.,
                ProceduralExecution::CallingThread,
                &mut effects
            ),
            Err(PointwiseError::Scene(SceneError::MissingRenderer))
        ));
    }

    // This adapter delegates every operation to the application's real backend;
    // it only observes the actual retained material map at the rendering boundary.
    struct Observe<'a> {
        inner: Backend<'a>,
        images: Vec<SharedImageMap>,
    }
    impl SceneImageRenderer<ForegroundError> for Observe<'_> {
        fn material_texture_limit(&mut self) -> Result<u32, PointwiseError<ForegroundError>> {
            self.inner.material_texture_limit()
        }
        fn preflight_array(&mut self, request: SceneAllocationRequest) -> Result<(), SceneError> {
            self.inner.preflight_array(request)
        }
        fn render_scene_image(
            &mut self,
            scene: &Scene,
            width: i32,
            height: i32,
        ) -> Result<SharedImageMap, PointwiseError<ForegroundError>> {
            let materials: Vec<_> = scene
                .models()
                .iter()
                .map(|model| model.material())
                .collect();
            let images: Vec<_> = materials
                .iter()
                .filter_map(|material| material.map())
                .collect();
            self.images = images.clone();
            let output = self.inner.render_scene_image(scene, width, height)?;
            for (material, actual) in materials
                .iter()
                .filter(|material| material.map().is_some())
                .zip(images.iter())
            {
                assert!(material.map().unwrap().same_reference(actual));
            }
            Ok(output)
        }
    }

    #[test]
    #[ignore = "requires a real supported GPU adapter and the renderer's texture features"]
    fn uv_texture_connected_scenes_reach_the_offscreen_renderer() {
        for bytes in [
            include_bytes!("../../../../port/evidence/uv-apply-model.node").as_slice(),
            include_bytes!("../../../../port/evidence/uv-curvature-textured-model.node").as_slice(),
            include_bytes!("../../../../port/evidence/uv-multi-project-textured-model.node")
                .as_slice(),
            include_bytes!("../../../../port/evidence/uv-project-textured-model.node").as_slice(),
        ] {
            let mut reader = mycela_engine::BinaryCursor::new(bytes).unwrap();
            let identity = reader.read_string().unwrap();
            let model = mycela_engine::load_node_binary::<ForegroundError>(
                &mut reader,
                3.75,
                Some(&identity),
            )
            .unwrap();
            let snapshot = mycela_engine::RenderCaptureSession::new()
                .capture_node(&model, 0.)
                .unwrap();
            let output = render_export(
                &snapshot,
                mycela_engine::ImageGeometry::new(128, 80),
                &|| false,
                GRAPH_PNG_OPTIONS,
                None,
                None,
                None,
            )
            .unwrap();
            let red = output.materialize(0).unwrap().values();
            let transparency = output.materialize(3).unwrap().values();
            assert!(
                red.iter()
                    .zip(transparency)
                    .any(|(&red, transparency)| red > 0.05 && transparency < 0.99)
            );
        }
    }

    #[test]
    #[ignore = "requires the portable offscreen GPU backend"]
    fn uv_environment_light_camera_matches_layered_original() {
        const ORIGINAL: &[u8] =
            include_bytes!("../../../../port/evidence/uv-environment-light-original-camera.u16le");
        let size = ImageGeometry::new(128, 80);
        let count = size.area() as usize;
        let cases = [
            (
                "alpha",
                include_bytes!("../../../../port/evidence/uv-environment-light-alpha-model.node")
                    .as_slice(),
                0.,
            ),
            (
                "animated",
                include_bytes!("../../../../port/evidence/uv-environment-light-animated.node")
                    .as_slice(),
                0.75,
            ),
            (
                "combined",
                include_bytes!(
                    "../../../../port/evidence/uv-environment-light-combined-model.node"
                )
                .as_slice(),
                0.,
            ),
            (
                "empty",
                include_bytes!("../../../../port/evidence/uv-environment-light-empty-model.node")
                    .as_slice(),
                0.,
            ),
            (
                "sphere",
                include_bytes!("../../../../port/evidence/uv-environment-light-sphere-model.node")
                    .as_slice(),
                0.,
            ),
        ];
        assert_eq!(ORIGINAL.len(), cases.len() * count * 8);
        for ((name, data, time), expected) in
            cases.into_iter().zip(ORIGINAL.chunks_exact(count * 8))
        {
            let mut cursor = mycela_engine::BinaryCursor::new(data).unwrap();
            let identity = cursor.read_string().unwrap();
            let node = mycela_engine::load_node_binary::<ForegroundError>(
                &mut cursor,
                3.75,
                Some(&identity),
            )
            .unwrap();
            let snapshot = mycela_engine::RenderCaptureSession::new()
                .capture_node(&node, time)
                .unwrap();
            let output = render_export(
                &snapshot,
                size,
                &|| false,
                GRAPH_PNG_OPTIONS,
                None,
                None,
                None,
            )
            .unwrap();
            for (channel, native) in expected.chunks_exact(count * 2).enumerate() {
                let values = output.materialize(channel as i32).unwrap().values();
                let mut sum = 0.;
                for (&value, sample) in values.iter().zip(native.chunks_exact(2)) {
                    let expected =
                        f32::from(u16::from_le_bytes(sample.try_into().unwrap())) / 65535.;
                    let error = (value - expected).abs();
                    assert!(
                        error <= 2.6 / 255.,
                        "{name} channel {channel} difference {error}"
                    );
                    sum += error;
                }
                assert!(sum / count as f32 <= 0.00009, "{name} channel {channel}");
            }
        }
    }

    #[test]
    #[ignore = "requires the portable offscreen GPU backend"]
    fn uv_vertex_map_camera_matches_original_single_layer() {
        const NATIVE: &[u8] =
            include_bytes!("../../../../port/evidence/uv-vertex-map-original-camera.u16le");
        const OCCLUSION: &[u8] =
            include_bytes!("../../../../port/evidence/uv-occlusion-original-camera.u16le");
        let plain = ImageGeometry::new(128, 80);
        let occlusion = ImageGeometry::new(96, 64);
        let (thickness, curvature) = NATIVE.split_at(plain.area() as usize * 4);
        let (ambient, environment) = OCCLUSION.split_at(occlusion.area() as usize * 4);
        for (size, bytes, expected) in [
            (
                plain,
                include_bytes!("../../../../port/evidence/uv-thickness-textured-model.node")
                    .as_slice(),
                thickness,
            ),
            (
                plain,
                include_bytes!("../../../../port/evidence/uv-curvature-textured-model.node")
                    .as_slice(),
                curvature,
            ),
            (
                occlusion,
                include_bytes!("../../../../port/evidence/uv-ambient-occlusion-sphere-model.node")
                    .as_slice(),
                ambient,
            ),
            (
                occlusion,
                include_bytes!(
                    "../../../../port/evidence/uv-environment-occlusion-sphere-model.node"
                )
                .as_slice(),
                environment,
            ),
        ] {
            let plane_size = size.area() as usize * 2;
            assert_eq!(expected.len(), plane_size * 2);
            let mut cursor = mycela_engine::BinaryCursor::new(bytes).unwrap();
            let identity = cursor.read_string().unwrap();
            let node = mycela_engine::load_node_binary::<ForegroundError>(
                &mut cursor,
                3.75,
                Some(&identity),
            )
            .unwrap();
            let snapshot = mycela_engine::RenderCaptureSession::new()
                .capture_node(&node, 0.)
                .unwrap();
            let image = render_export(
                &snapshot,
                size,
                &|| false,
                GRAPH_PNG_OPTIONS,
                None,
                None,
                None,
            )
            .unwrap();
            for (channel, expected) in [0, 3].into_iter().zip(expected.chunks_exact(plane_size)) {
                let values = image.materialize(channel).unwrap().values();
                assert_eq!(values.len() * 2, expected.len());
                for (index, (value, expected)) in
                    values.into_iter().zip(expected.chunks_exact(2)).enumerate()
                {
                    let expected =
                        f32::from(u16::from_le_bytes(expected.try_into().unwrap())) / 65535.;
                    let limit = if size == occlusion { 0.55 } else { 0.5 } / 255.;
                    assert!(
                        (value - expected).abs() <= limit,
                        "{identity:?} channel {channel}, pixel {index}: {value} vs {expected}"
                    );
                }
            }
            for channel in [1, 2] {
                assert!(
                    image
                        .materialize(channel)
                        .unwrap()
                        .values()
                        .iter()
                        .all(|value| *value == 0.)
                );
            }
        }
    }

    #[test]
    #[ignore = "requires the portable offscreen GPU backend"]
    fn uv_light_camera_matches_original_and_forwards_geometry() {
        const NATIVE: &[u8] =
            include_bytes!("../../../../port/evidence/uv-light-original-camera.u8");
        let size = ImageGeometry::new(96, 96);
        let area = size.area() as usize;
        assert_eq!(NATIVE.len(), area * 4);
        let render_node = |node: &PointwiseNode<ForegroundError>| {
            let snapshot = mycela_engine::RenderCaptureSession::new()
                .capture_node(node, 0.)
                .unwrap();
            render_export(
                &snapshot,
                size,
                &|| false,
                GRAPH_PNG_OPTIONS,
                None,
                None,
                None,
            )
            .unwrap()
        };
        let fixtures = [
            include_bytes!("../../../../port/evidence/uv-light-model.node").as_slice(),
            include_bytes!("../../../../port/evidence/uv-light-model-shadow.node").as_slice(),
        ];
        for (bytes, expected) in fixtures.into_iter().zip(NATIVE.chunks_exact(area * 2)) {
            let mut cursor = mycela_engine::BinaryCursor::new(bytes).unwrap();
            let identity = cursor.read_string().unwrap();
            let node = mycela_engine::load_node_binary(&mut cursor, 3.75, Some(&identity)).unwrap();
            let image = render_node(&node);
            for channel in 0..4 {
                let values = image.materialize(channel).unwrap().values();
                assert_eq!(values.len(), area);
                match channel {
                    0 => {
                        for (index, value) in values.into_iter().enumerate() {
                            assert!(
                                (value - f32::from(expected[index]) / 255.).abs()
                                    <= 1. / 255. + f32::EPSILON
                            );
                        }
                    }
                    3 => {
                        for (index, value) in values.into_iter().enumerate() {
                            assert_eq!(value, f32::from(expected[area + index]) / 255.);
                        }
                    }
                    _ => assert!(values.into_iter().all(|value| value == 0.)),
                }
            }
        }

        let parent = SceneTransformNode::new().unwrap().node();
        let sphere =
            mycela_engine::ScenePrimitiveNode::new(mycela_engine::ScenePrimitiveKind::Sphere)
                .unwrap()
                .node();
        parent.slot(0).unwrap().set_node(Some(&sphere)).unwrap();
        let plain = render_node(&parent);
        let light = PointwiseNode::uv_light().unwrap();
        light.uv_texture_output().unwrap().set_int32(1).unwrap();
        parent.slot(0).unwrap().set_node(None).unwrap();
        light.slot(0).unwrap().set_node(Some(&sphere)).unwrap();
        parent.slot(0).unwrap().set_node(Some(&light)).unwrap();
        let linked = render_node(&parent);
        for channel in 0..4 {
            assert_eq!(
                plain.materialize(channel).unwrap().values(),
                linked.materialize(channel).unwrap().values()
            );
        }
    }

    #[test]
    #[ignore = "requires a real supported GPU adapter and the renderer's texture features"]
    fn real_gpu_scene_uses_actual_child_color_and_opacity() {
        let cancelled = || false;
        let mut devices = PreviewDevices::default();
        let mut backend = Observe {
            inner: Backend {
                devices: &mut devices,
                camera: SceneCamera::source_preview(),
                cancelled: &cancelled,
                inactive: AtomicBool::new(false),
                deadline: Instant::now() + Duration::from_secs(120),
                failure: None,
            },
            images: Vec::new(),
        };
        for (rgb, dominant, transform) in
            [([1., 0., 0.], 0usize, false), ([0., 0., 1.], 2usize, true)]
        {
            let root = graph(rgb, 0.25, transform);
            let mut capacity = crate::memory_capacity::query;
            let mut preflight = crate::memory_capacity::preflight;
            let outcome = {
                let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight)
                    .with_scene_renderer(&mut backend);
                root.render_original_with_effects(
                    32,
                    32,
                    PointwiseRenderOptions::new(false, false),
                    0.,
                    ProceduralExecution::CallingThread,
                    &mut effects,
                )
                .unwrap()
            };
            assert!(outcome.caught_errors().is_empty());
            assert_eq!(backend.images.len(), 1);
            let source = backend
                .images
                .pop()
                .expect("renderer observed actual scene map");
            assert_eq!(
                source
                    .channel(dominant as i32, true)
                    .unwrap()
                    .read(0)
                    .unwrap(),
                1.
            );
            assert_eq!(source.channel(3, true).unwrap().read(0).unwrap(), 0.25);
            let output = outcome.image().unwrap();
            let maximum = |index| {
                output
                    .channel(index, true)
                    .unwrap()
                    .values()
                    .into_iter()
                    .fold(0_f32, f32::max)
            };
            assert!(
                maximum(dominant as i32) > 0.01,
                "map color must reach sampled framebuffer"
            );
            assert!(
                maximum((2 - dominant) as i32) < 0.01,
                "other source color channel must remain dark"
            );
            let transparency = output.channel(3, true).unwrap().values();
            assert!(
                transparency
                    .iter()
                    .any(|value| *value > 0.05 && *value < 0.95),
                "sampled alpha must reach framebuffer"
            );
        }
    }
    #[test]
    #[ignore = "requires a real supported GPU adapter and the renderer's texture features"]
    fn nested_and_combined_scenes_keep_both_actual_material_images() {
        let red = graph([1., 0., 0.], 0.25, true);
        red.scene_transform_3d()
            .unwrap()
            .property(1)
            .unwrap()
            .set_vector3d([-0.7, 0., 0.])
            .unwrap();
        let blue = graph([0., 0., 1.], 0.5, true);
        blue.scene_transform_3d()
            .unwrap()
            .property(1)
            .unwrap()
            .set_vector3d([0.7, 0., 0.])
            .unwrap();
        let combine = SceneCombineNode::new().node();
        combine.slot(0).unwrap().set_node(Some(&red)).unwrap();
        combine.slot(1).unwrap().set_node(Some(&blue)).unwrap();
        let clear = SceneClearLightsNode::new().node();
        clear.slot(0).unwrap().set_node(Some(&combine)).unwrap();
        let cancelled = || false;
        let mut devices = PreviewDevices::default();
        let mut backend = Observe {
            inner: Backend {
                devices: &mut devices,
                camera: SceneCamera::source_preview(),
                cancelled: &cancelled,
                inactive: AtomicBool::new(false),
                deadline: Instant::now() + Duration::from_secs(120),
                failure: None,
            },
            images: Vec::new(),
        };
        let mut capacity = crate::memory_capacity::query;
        let mut preflight = crate::memory_capacity::preflight;
        let outcome = {
            let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight)
                .with_scene_renderer(&mut backend);
            clear
                .render_original_with_effects(
                    32,
                    32,
                    PointwiseRenderOptions::new(false, false),
                    0.,
                    ProceduralExecution::CallingThread,
                    &mut effects,
                )
                .unwrap()
        };
        assert!(outcome.caught_errors().is_empty());
        assert_eq!(backend.images.len(), 2);
        assert!(!backend.images[0].same_reference(&backend.images[1]));
        assert_eq!(
            backend.images[0].channel(0, true).unwrap().read(0).unwrap(),
            1.
        );
        assert_eq!(
            backend.images[0].channel(3, true).unwrap().read(0).unwrap(),
            0.25
        );
        assert_eq!(
            backend.images[1].channel(2, true).unwrap().read(0).unwrap(),
            1.
        );
        assert_eq!(
            backend.images[1].channel(3, true).unwrap().read(0).unwrap(),
            0.5
        );
        let output = outcome.image().unwrap();
        for channel in [0, 2] {
            let maximum = output
                .channel(channel, true)
                .unwrap()
                .values()
                .into_iter()
                .fold(0_f32, f32::max);
            assert!(
                maximum > 0.01,
                "each actual material must reach the framebuffer"
            );
        }
        assert!(
            output
                .channel(3, true)
                .unwrap()
                .values()
                .iter()
                .any(|value| *value > 0.05 && *value < 0.95)
        );
    }
}

#[test]
#[ignore = "requires a working wgpu adapter"]
fn shader_graph_matches_preview_and_export() {
    use mycela_engine::{PointwiseFilter, PointwiseKind, RenderCaptureSession, ShaderProgram};
    use std::sync::Arc;

    let shader = PointwiseFilter::<ForegroundError>::new(PointwiseKind::Shader)
        .unwrap()
        .node();
    let color = mycela_engine::PointwiseNode::solid_color().unwrap();
    color.set_solid_color([0.25, 0.75, 0.5]).unwrap();
    let source = r#"
requires swizzle_assignment;
@group(1) @binding(4) var paint: texture_2d<f32>;
@group(1) @binding(7) var disconnected: texture_2d<f32>;
fn shade(uv: vec2f) -> vec4f {
    var c = sample_input(paint, uv);
    c.rg = c.gr;
    c.b = time;
    return c + sample_input(disconnected, uv);
}
"#;
    shader
        .edit_shader_program(Arc::new(ShaderProgram::compile(source).unwrap()))
        .unwrap();
    shader.slot(0).unwrap().set_node(Some(&color)).unwrap();
    let mut capture = RenderCaptureSession::new();
    for (width, height, time) in [(8, 4, 0.125), (512, 512, 0.125), (512, 512, 0.625)] {
        let snapshot = capture.capture_node(&shader, time).unwrap();
        let preview = render(&snapshot, width, height, &|| false, GRAPH_PNG_OPTIONS, None).unwrap();
        let exported = render_export(
            &snapshot,
            ImageGeometry::new(width, height),
            &|| false,
            GRAPH_PNG_OPTIONS,
            None,
            None,
            None,
        )
        .unwrap();
        for (channel, expected) in [0.75, 0.25, time as f32, 0.0].into_iter().enumerate() {
            let channel = channel as i32;
            let output = preview.materialize(channel).unwrap();
            assert!(
                output.values().iter().all(|value| *value == expected),
                "{width}×{height} at {time}, channel {channel}: {:?}, expected {expected}",
                output.read(0)
            );
            assert_eq!(
                output.values(),
                exported.materialize(channel).unwrap().values()
            );
        }
    }

    let coordinates = r#"
fn shade(uv: vec2f) -> vec4f {
    return vec4f(uv.x, pixel.y * texel_size.y, select(0.0, 1.0, resolution.x == 1024.0), 1.0);
}
"#;
    shader
        .edit_shader_program(Arc::new(ShaderProgram::compile(coordinates).unwrap()))
        .unwrap();
    let snapshot = RenderCaptureSession::new()
        .capture_node(&shader, 0.)
        .unwrap();
    let exported = render_export(
        &snapshot,
        ImageGeometry::new(1024, 256),
        &|| false,
        GRAPH_PNG_OPTIONS,
        None,
        None,
        None,
    )
    .unwrap();
    for (channel, first, last) in [
        (0, 0.5 / 1024., 1023.5 / 1024.),
        (1, 0.5 / 256., 255.5 / 256.),
        (2, 1., 1.),
        (3, 0., 0.),
    ] {
        let output = exported.materialize(channel).unwrap();
        assert_eq!(output.read(0).unwrap(), first);
        assert_eq!(output.read(1024 * 256 - 1).unwrap(), last);
    }
}
