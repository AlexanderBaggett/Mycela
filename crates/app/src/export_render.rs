//! Click-time owned render data. No GPUI entity, callback, live parameter or
//! preview bitmap is retained by the background job.
use crate::{
    ForegroundError,
    filter_pipeline::{
        CapturedFilterStage, FilterStage, RenderGeometry, RenderInvert, RenderStage, checkpoint,
        failure, render_chain_observed,
    },
    solid_color::map_preview::{PreviewSelection, project_bitmap},
};
use mycela_engine::{
    GradientGenerator, GradientRenderSnapshot, InvertRenderSnapshot, PointwiseFinalExportCache,
    PointwiseNode, PointwiseNodeRenderSnapshot, PreviewBitmap, SharedInvertNode,
    SharedSolidColorNode, SolidColorNodeSnapshot,
};

pub(crate) use crate::filter_pipeline::RenderError as ExportRenderError;
#[cfg(test)]
use mycela_engine::RenderCaptureSession;
#[path = "export_pending.rs"]
mod pending;
pub(crate) use pending::PendingExportRenderJob;

pub(crate) fn generator_execution() -> mycela_engine::ProceduralExecution {
    let count = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    mycela_engine::ProceduralExecution::WorkerPool {
        processor_count: i32::try_from(count).unwrap_or(i32::MAX),
    }
}

pub(crate) enum ExportSource<'a> {
    Solid(&'a SharedSolidColorNode<ForegroundError>),
    FinalSolid(&'a SharedSolidColorNode<ForegroundError>),
    Graph {
        node: &'a PointwiseNode<ForegroundError>,
        camera: Option<&'a mycela_engine::SceneCamera<ForegroundError>>,
        time: f64,
    },
    Gradient {
        model: &'a GradientGenerator<ForegroundError>,
        time: f64,
    },
}
#[derive(Clone)]
enum SourceFrame {
    Solid(SolidColorNodeSnapshot),
    Graph(
        PointwiseNodeRenderSnapshot<ForegroundError>,
        Option<crate::scene_backend::CameraFrame>,
    ),
    Gradient(GradientRenderSnapshot),
}
fn same_optional_camera(
    left: &Option<crate::scene_backend::CameraFrame>,
    right: &Option<crate::scene_backend::CameraFrame>,
) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left.same_values(right),
        (None, None) => true,
        _ => false,
    }
}

#[derive(Clone)]
pub(crate) struct ExportRenderJob {
    source: SourceFrame,
    graph_cache: Option<PointwiseFinalExportCache<ForegroundError>>,
    environment_library: Option<mycela_engine::EnvironmentResourceLibrary>,
    invert: Option<InvertRenderSnapshot>,
    filters: Vec<CapturedFilterStage>,
    selection: PreviewSelection,
    geometry: RenderGeometry,
    time: f64,
}
pub(crate) struct RenderedScene {
    pub(crate) image: mycela_engine::SharedImageMap,
    pub(crate) bitmap: PreviewBitmap,
    pub(crate) selection: PreviewSelection,
    pub(crate) layer_inputs: Vec<(u64, mycela_engine::SharedImageMap)>,
}
pub(crate) struct PreviewSceneResult {
    pub(crate) result: Result<RenderedScene, ExportRenderError>,
    pub(crate) layer_inputs: Vec<(u64, mycela_engine::SharedImageMap)>,
}
impl ExportRenderJob {
    /// Capture reads committed values only. The engine freezes evaluated Gradient
    /// axes and deep filter children without source setters, events or rebuilds
    /// on the live scene. Capability/shape errors remain explicit failures.
    #[cfg(test)]
    pub(crate) fn capture(
        source: ExportSource<'_>,
        invert: Option<&SharedInvertNode<ForegroundError>>,
        filters: &[FilterStage],
        selection: PreviewSelection,
        width: i32,
        height: i32,
    ) -> Result<Self, ExportRenderError> {
        let time = match &source {
            ExportSource::Solid(_) | ExportSource::FinalSolid(_) => 0.,
            ExportSource::Graph { time, .. } => *time,
            ExportSource::Gradient { time, .. } => *time,
        };
        Self::capture_at_time(source, invert, filters, selection, width, height, time)
    }
    #[cfg(test)]
    pub(crate) fn capture_at_time(
        source: ExportSource<'_>,
        invert: Option<&SharedInvertNode<ForegroundError>>,
        filters: &[FilterStage],
        selection: PreviewSelection,
        width: i32,
        height: i32,
        time: f64,
    ) -> Result<Self, ExportRenderError> {
        let geometry = RenderGeometry::new(width, height)?;
        let mut session = RenderCaptureSession::new();
        let source = match source {
            ExportSource::Solid(model) => {
                SourceFrame::Solid(model.capture_preview_at(time).map_err(failure)?)
            }
            ExportSource::FinalSolid(model) => {
                SourceFrame::Solid(model.capture_final_at(time).map_err(failure)?)
            }
            ExportSource::Graph {
                node,
                camera,
                time: source_time,
            } => {
                if source_time.to_bits() != time.to_bits() {
                    return Err(failure("The source and filter sample times differ."));
                }
                SourceFrame::Graph(
                    session.capture_node(node, time).map_err(failure)?,
                    camera
                        .map(crate::scene_backend::CameraFrame::capture)
                        .transpose()?,
                )
            }
            ExportSource::Gradient {
                model,
                time: source_time,
            } => {
                if source_time.to_bits() != time.to_bits() {
                    return Err(failure("The source and filter sample times differ."));
                }
                SourceFrame::Gradient(session.capture_gradient(model, time).map_err(failure)?)
            }
        };
        let invert = invert
            .map(|model| {
                model
                    .capture_render()
                    .map_err(|e| failure(format!("{e:?}")))
            })
            .transpose()?;
        let filters = filters
            .iter()
            .map(|stage| stage.capture_render(&mut session, time))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            source,
            graph_cache: None,
            environment_library: None,
            invert,
            filters,
            selection,
            geometry,
            time,
        })
    }
    /// Compare owned effective operands captured before work, never a live
    /// notification counter or the worker's mutated Layer Maps options.
    pub(crate) fn same_render_values(&self, other: &Self) -> bool {
        let source = match (&self.source, &other.source) {
            (SourceFrame::Solid(a), SourceFrame::Solid(b)) => {
                a.mode == b.mode
                    && a.color
                        .iter()
                        .zip(b.color)
                        .all(|(a, b)| a.to_bits() == b.to_bits())
                    && a.transparency.to_bits() == b.transparency.to_bits()
            }
            (SourceFrame::Gradient(a), SourceFrame::Gradient(b)) => a.same_render_values(b),
            (SourceFrame::Graph(a, ac), SourceFrame::Graph(b, bc)) => {
                a.same_render_values(b) && same_optional_camera(ac, bc)
            }
            _ => false,
        };
        let invert = match (&self.invert, &other.invert) {
            (None, None) => true,
            (Some(a), Some(b)) => a.same_render_values(b),
            _ => false,
        };
        source
            && invert
            && self.time.to_bits() == other.time.to_bits()
            && self.geometry == other.geometry
            && self.selection == other.selection
            && self.filters.len() == other.filters.len()
            && self
                .filters
                .iter()
                .zip(&other.filters)
                .all(|(a, b)| a.same_render_values(b))
    }
    /// Consume once: Layer Maps may refresh only this owned frame. Cancellation
    /// is cooperative at stage and conversion boundaries and within Gradient.
    pub(crate) fn render(
        self,
        cancelled: impl Fn() -> bool,
    ) -> Result<PreviewBitmap, ExportRenderError> {
        self.render_scene(cancelled, false)
            .map(|scene| scene.bitmap)
    }
    pub(crate) fn render_scene(
        self,
        cancelled: impl Fn() -> bool,
        preview_fallback: bool,
    ) -> Result<RenderedScene, ExportRenderError> {
        self.render_scene_with_inputs(cancelled, preview_fallback, &mut Vec::new())
    }
    // Reached options remain usable even if a later stage or projection fails.
    pub(crate) fn render_preview_scene(self, cancelled: impl Fn() -> bool) -> PreviewSceneResult {
        let mut inputs = Vec::new();
        let result = self.render_scene_with_inputs(cancelled, true, &mut inputs);
        PreviewSceneResult {
            result,
            layer_inputs: inputs,
        }
    }
    fn render_scene_with_inputs(
        self,
        cancelled: impl Fn() -> bool,
        preview_fallback: bool,
        layer_inputs: &mut Vec<(u64, mycela_engine::SharedImageMap)>,
    ) -> Result<RenderedScene, ExportRenderError> {
        checkpoint(&cancelled)?;
        let image = match self.source {
            SourceFrame::Solid(snapshot) => mycela_engine::SharedImageMap::from_image(
                snapshot
                    .render(self.geometry.width(), self.geometry.height())
                    .map_err(failure)?,
            ),
            SourceFrame::Graph(snapshot, camera) => {
                let options = crate::scene_backend::GRAPH_PNG_OPTIONS;
                if !preview_fallback {
                    crate::scene_backend::render_export(
                        &snapshot,
                        mycela_engine::ImageGeometry::new(
                            self.geometry.width(),
                            self.geometry.height(),
                        ),
                        &cancelled,
                        options,
                        camera.as_ref(),
                        self.graph_cache.as_ref(),
                        self.environment_library.as_ref(),
                    )?
                } else {
                    match camera {
                        Some(camera) => crate::scene_backend::render_with_camera(
                            &snapshot,
                            self.geometry.width(),
                            self.geometry.height(),
                            &cancelled,
                            options,
                            &camera,
                            self.environment_library.as_ref(),
                        )?,
                        None => crate::scene_backend::render(
                            &snapshot,
                            self.geometry.width(),
                            self.geometry.height(),
                            &cancelled,
                            options,
                            self.environment_library.as_ref(),
                        )?,
                    }
                }
            }
            SourceFrame::Gradient(snapshot) => snapshot
                .render_with_preflight_and_cancel::<ForegroundError>(
                    self.geometry.width(),
                    self.geometry.height(),
                    generator_execution(),
                    &mut crate::memory_capacity::noise_preflight,
                    &cancelled,
                )
                .map_err(|error| match error {
                    mycela_engine::PointwiseError::Cancelled => ExportRenderError::Cancelled,
                    error => failure(error),
                })?,
        };
        checkpoint(&cancelled)?;
        let image = render_chain_observed(
            image,
            self.invert.as_ref().map(RenderInvert::Captured),
            self.filters.iter().map(RenderStage::Captured),
            self.geometry,
            &cancelled,
            layer_inputs,
        )?;
        checkpoint(&cancelled)?;
        // Unlike preview presentation, a missing selected occurrence is an error.
        // project_bitmap reads channels/stores without mutating upstream maps.
        let selection = if preview_fallback
            && !crate::solid_color::map_preview::choices(&image).contains(&self.selection)
        {
            PreviewSelection::BaseColor
        } else {
            self.selection
        };
        let bitmap = project_bitmap(&image, &selection).map_err(ExportRenderError::Render)?;
        checkpoint(&cancelled)?;
        Ok(RenderedScene {
            image,
            bitmap,
            selection,
            layer_inputs: layer_inputs.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter_pipeline::FilterInput;
    use crate::filter_pipeline::render_chain;
    use mycela_engine::{
        GradientGeneratorParameterKind, PointwiseFilter, PointwiseKind, SharedImageMap,
        SolidColorCloneMode,
    };
    use std::cell::Cell;

    fn solid() -> SharedSolidColorNode<ForegroundError> {
        let node = SharedSolidColorNode::new_unattached(SolidColorCloneMode::Original).unwrap();
        node.color_property().set_color([0.2, 0.4, 0.8]).unwrap();
        node.transparency_property().set_transparency(0.25).unwrap();
        node
    }
    fn stage(id: u64, kind: PointwiseKind, routes: [FilterInput; 3]) -> FilterStage {
        FilterStage::new(id, PointwiseFilter::new(kind).unwrap(), routes)
    }
    fn capture(
        node: &SharedSolidColorNode<ForegroundError>,
        filters: &[FilterStage],
        width: i32,
        height: i32,
    ) -> ExportRenderJob {
        ExportRenderJob::capture(
            ExportSource::Solid(node),
            None,
            filters,
            PreviewSelection::BaseColor,
            width,
            height,
        )
        .unwrap()
    }
    #[test]
    fn environment_filter_exports_use_owned_values_and_real_input_routes() {
        for kind in [
            PointwiseKind::RotateEnvironment,
            PointwiseKind::DiffuseEnvironment,
            PointwiseKind::ScatterEnvironment,
            PointwiseKind::ReflectEnvironment,
            PointwiseKind::RefractEnvironment,
            PointwiseKind::Reflect,
            PointwiseKind::Refract,
            PointwiseKind::Stereogram,
            PointwiseKind::Caustics,
            PointwiseKind::Transmit,
        ] {
            let generator = GradientGenerator::<ForegroundError>::new().unwrap();
            let filter = stage(
                1,
                kind,
                [
                    FilterInput::Previous,
                    FilterInput::Source,
                    FilterInput::None,
                ],
            );
            let radius = if kind == PointwiseKind::DiffuseEnvironment {
                1
            } else {
                0
            };
            filter
                .model()
                .parameter(radius)
                .unwrap()
                .set_number(if kind == PointwiseKind::RotateEnvironment {
                    90.0
                } else if matches!(
                    kind,
                    PointwiseKind::ReflectEnvironment
                        | PointwiseKind::RefractEnvironment
                        | PointwiseKind::Reflect
                        | PointwiseKind::Refract
                        | PointwiseKind::Stereogram
                        | PointwiseKind::Caustics
                        | PointwiseKind::Transmit
                ) {
                    0.2
                } else {
                    60.0
                })
                .unwrap();
            let input = generator.render_unbound(8, 8, 0.0).unwrap();
            let result = render_chain(
                input,
                None,
                [RenderStage::Live(&filter, 0.0)],
                RenderGeometry::new(8, 8).unwrap(),
                &|| false,
            )
            .unwrap();
            let expected = project_bitmap(&result, &PreviewSelection::BaseColor).unwrap();
            let job = ExportRenderJob::capture(
                ExportSource::Gradient {
                    model: &generator,
                    time: 0.0,
                },
                None,
                std::slice::from_ref(&filter),
                PreviewSelection::BaseColor,
                8,
                8,
            )
            .unwrap();
            filter
                .model()
                .parameter(radius)
                .unwrap()
                .set_number(0.0)
                .unwrap();
            generator
                .parameter(GradientGeneratorParameterKind::Mode)
                .set_index(1)
                .unwrap();
            let actual = job.render(|| false).unwrap();
            assert_eq!(
                actual.rows().unwrap(),
                expected.rows().unwrap(),
                "{}",
                kind.name()
            );
        }
    }
    #[test]
    fn atmosphere_export_freezes_seed_and_volume_controls() {
        let node = solid();
        let filter = stage(1, PointwiseKind::Atmosphere, [FilterInput::Previous; 3]);
        filter
            .model()
            .parameter(2)
            .unwrap()
            .set_number(0.0)
            .unwrap();
        filter
            .model()
            .parameter(3)
            .unwrap()
            .set_number(1.0)
            .unwrap();
        let input = SharedImageMap::from_image(node.render(4, 4, 0.0).unwrap());
        let out = render_chain(
            input,
            None,
            [RenderStage::Live(&filter, 0.0)],
            RenderGeometry::new(4, 4).unwrap(),
            &|| false,
        )
        .unwrap();
        let expected = project_bitmap(&out, &PreviewSelection::BaseColor).unwrap();
        let job = capture(&node, std::slice::from_ref(&filter), 4, 4);
        filter
            .model()
            .parameter(5)
            .unwrap()
            .set_number(20.0)
            .unwrap();
        assert_eq!(
            job.render(|| false).unwrap().rows().unwrap(),
            expected.rows().unwrap()
        );
    }
    #[test]
    fn convert_environment_export_freezes_projection_choice() {
        let generator = GradientGenerator::<ForegroundError>::new().unwrap();
        let filter = stage(
            1,
            PointwiseKind::ConvertEnvironment,
            [FilterInput::Previous; 3],
        );
        filter.model().parameter(1).unwrap().set_index(3).unwrap();
        let source = generator.render_unbound(16, 16, 0.0).unwrap();
        let image = render_chain(
            source,
            None,
            [RenderStage::Live(&filter, 0.0)],
            RenderGeometry::new(16, 16).unwrap(),
            &|| false,
        )
        .unwrap();
        let expected = project_bitmap(&image, &PreviewSelection::BaseColor).unwrap();
        let job = ExportRenderJob::capture(
            ExportSource::Gradient {
                model: &generator,
                time: 0.0,
            },
            None,
            std::slice::from_ref(&filter),
            PreviewSelection::BaseColor,
            16,
            16,
        )
        .unwrap();
        filter.model().parameter(1).unwrap().set_index(4).unwrap();
        assert_eq!(
            job.render(|| false).unwrap().rows().unwrap(),
            expected.rows().unwrap()
        );
    }
    #[test]
    fn full_light_fourth_route_survives_export_capture() {
        let node = solid();
        let model = PointwiseFilter::new(PointwiseKind::FullLight).unwrap();
        model.parameter(2).unwrap().set_number(0.0).unwrap();
        model.parameter(8).unwrap().set_number(0.0).unwrap();
        let absent = FilterStage::new(
            1,
            model.clone(),
            [
                FilterInput::Previous,
                FilterInput::Source,
                FilterInput::Source,
                FilterInput::None,
            ],
        );
        let present = FilterStage::new(
            1,
            model.clone(),
            [
                FilterInput::Previous,
                FilterInput::Source,
                FilterInput::Source,
                FilterInput::Source,
            ],
        );
        let source = SharedImageMap::from_image(node.render(4, 4, 0.0).unwrap());
        let draw = |stage| {
            render_chain(
                source.clone(),
                None,
                [RenderStage::Live(stage, 0.0)],
                RenderGeometry::new(4, 4).unwrap(),
                &|| false,
            )
            .unwrap()
        };
        let a = draw(&absent);
        let b = draw(&present);
        assert!((a.channel(0, true).unwrap().read(0).unwrap() - 0.1).abs() < 0.00001);
        assert!((b.channel(0, true).unwrap().read(0).unwrap() - 0.02).abs() < 0.00001);
        let a_job = capture(&node, &[absent], 4, 4);
        let b_job = capture(&node, &[present], 4, 4);
        model.parameter(7).unwrap().set_number(0.0).unwrap();
        assert_eq!(
            a_job.render(|| false).unwrap().rows().unwrap(),
            project_bitmap(&a, &PreviewSelection::BaseColor)
                .unwrap()
                .rows()
                .unwrap()
        );
        assert_eq!(
            b_job.render(|| false).unwrap().rows().unwrap(),
            project_bitmap(&b, &PreviewSelection::BaseColor)
                .unwrap()
                .rows()
                .unwrap()
        );
        assert!(model.node().slot(3).unwrap().node().is_none());
    }
    #[test]
    fn variable_geometry_is_rendered_directly_and_solid_frame_owns_raw_values() {
        let node = solid();
        let job = capture(&node, &[], 7, 3);
        node.color_property().set_color([1., 0., 0.]).unwrap();
        node.transparency_property().set_transparency(1.).unwrap();
        let bitmap = job.render(|| false).unwrap();
        assert_eq!((bitmap.width(), bitmap.height()), (7, 3));
        assert_eq!(bitmap.rows().unwrap().len(), 3);
        for y in 0..3 {
            for x in 0..7 {
                assert_eq!(bitmap.argb_pixel(x, y).unwrap(), 0xbf3366cc);
            }
        }
        for (w, h) in [(0, 3), (3, 0), (-1, 1), (i32::MAX, 2)] {
            assert!(matches!(
                ExportRenderJob::capture(
                    ExportSource::Solid(&node),
                    None,
                    &[],
                    PreviewSelection::BaseColor,
                    w,
                    h
                ),
                Err(ExportRenderError::InvalidGeometry)
            ));
        }
    }
    #[test]
    fn frozen_graph_export_uses_final_gradient_construction_and_group_reference() {
        use mycela_engine::{
            GradientGeneratorParameterKind as Parameter, PointwiseFilter, PointwiseKind,
        };
        let gradient = GradientGenerator::<ForegroundError>::new().unwrap();
        gradient
            .parameter(Parameter::Location)
            .set_axis(0, 0.25)
            .unwrap();
        let expected = project_bitmap(
            &gradient.render_unbound(7, 3, 0.).unwrap(),
            &PreviewSelection::BaseColor,
        )
        .unwrap();
        // The final constructor retains its literal mode options, even if the
        // current original's selected option object has been edited.
        gradient
            .parameter(Parameter::Mode)
            .options()
            .unwrap()
            .set_entry(0, None)
            .unwrap();
        let group = PointwiseFilter::new(PointwiseKind::Group).unwrap().node();
        group
            .slot(0)
            .unwrap()
            .set_node(Some(&gradient.node()))
            .unwrap();
        let reference = PointwiseFilter::new(PointwiseKind::Reference)
            .unwrap()
            .node();
        group
            .content(0)
            .unwrap()
            .set_node(Some(&reference))
            .unwrap();
        let job = ExportRenderJob::freeze_at_time(
            ExportSource::Graph {
                node: &reference,
                camera: None,
                time: 0.,
            },
            None,
            &[],
            PreviewSelection::BaseColor,
            7,
            3,
            0.,
        )
        .unwrap();
        gradient
            .parameter(Parameter::Location)
            .set_axis(0, 0.75)
            .unwrap();
        let exported = job.materialize().unwrap().render(|| false).unwrap();
        assert_eq!(exported.rows().unwrap(), expected.rows().unwrap());
        assert!(gradient.cached_final_image().is_none());
    }

    #[test]
    fn connected_refraction_preview_matches_final_at_both_render_scales() {
        use mycela_engine::{
            AnalyticCurveKind, CurveLayerHandle, ImageGeometry, PointwiseRenderOptions,
            RenderedImage,
        };

        for kind in [PointwiseKind::Refract, PointwiseKind::RefractEnvironment] {
            let base = GradientGenerator::new().unwrap();
            // Keep the ramp discontinuity away from the spherical lookup seam
            // so this comparison measures displacement rather than resampling.
            base.parameter(GradientGeneratorParameterKind::Location)
                .set_axis(0, 0.25)
                .unwrap();
            let heightmap = GradientGenerator::new().unwrap();
            let filter = PointwiseFilter::new(kind).unwrap();
            let node = filter.node();
            let (inputs, height_index) = if kind == PointwiseKind::Refract {
                filter.parameter(0).unwrap().set_number(1.5).unwrap();
                filter.parameter(1).unwrap().set_number(0.5).unwrap();
                filter.parameter(2).unwrap().set_number(0.25).unwrap();
                ([base.node(), heightmap.node()], 2)
            } else {
                filter.parameter(0).unwrap().set_number(0.25).unwrap();
                filter.parameter(1).unwrap().set_number(1.5).unwrap();
                ([heightmap.node(), base.node()], 0)
            };
            for (slot, input) in node.slots().iter().zip(inputs) {
                slot.set_node(Some(&input)).unwrap();
            }
            let height = filter.parameter(height_index).unwrap();
            height.set_axis_animated(0, true).unwrap();
            let curve = height.animation_curve(0).unwrap();
            curve.set_minimum(0.125);
            curve.set_maximum(0.375);
            curve.append(&[Some(CurveLayerHandle::new(AnalyticCurveKind::Linear))]);
            // A successful export through the Original fallback would hide
            // regressions in the source-shaped final scheduling path.
            assert!(node.final_graph_clone().is_ok());

            let time = 0.375;
            let preview_options = PointwiseRenderOptions::new(false, false);
            let snapshot = RenderCaptureSession::new()
                .capture_node(&node, time)
                .unwrap();
            let geometries = [(96, 96), (192, 192), (96, 64), (192, 128), (64, 96)]
                .map(|(width, height)| ImageGeometry::new(width, height));
            let frames = geometries.map(|geometry| {
                let animated = crate::scene_backend::render_graph_playback(
                    &snapshot,
                    geometry,
                    &|| false,
                    None,
                )
                .unwrap();
                assert!(matches!(animated, RenderedImage::Animated(_)));
                let frame = animated.frame_at_time(time).unwrap();
                assert_eq!(frame.geometry(), geometry);
                frame
            });
            let samples: Vec<_> = geometries
                .into_iter()
                .zip(frames)
                .map(|(geometry, animated)| {
                    let (width, height) = (geometry.width(), geometry.height());
                    let size = format!("{width}×{height}");
                    let preview = crate::scene_backend::render(
                        &snapshot,
                        width,
                        height,
                        &|| false,
                        preview_options,
                        None,
                    )
                    .unwrap();
                    let export = ExportRenderJob::freeze_at_time(
                        ExportSource::Graph {
                            node: &node,
                            camera: None,
                            time,
                        },
                        None,
                        &[],
                        PreviewSelection::BaseColor,
                        width,
                        height,
                        time,
                    )
                    .unwrap()
                    .materialize()
                    .unwrap()
                    .render_scene(|| false, false)
                    .unwrap();
                    assert_eq!(preview.geometry(), geometry);
                    assert_eq!(export.image.geometry(), geometry);
                    assert_eq!(
                        (export.bitmap.width(), export.bitmap.height()),
                        (width, height)
                    );
                    for channel in 0..4 {
                        let actual = preview.channel(channel, false).unwrap();
                        let animated = animated.channel(channel, false).unwrap();
                        let expected = export.image.channel(channel, false).unwrap();
                        assert_eq!(actual.len(), expected.len(), "{kind:?} at {size}");
                        assert_eq!(animated.len(), expected.len(), "{kind:?} at {size}");
                        for i in 0..expected.len() {
                            let expected = expected.read(i).unwrap();
                            assert_eq!(
                                actual.read(i).unwrap(),
                                expected,
                                "{kind:?} at {size}, channel {channel}, pixel {i}"
                            );
                            assert_eq!(
                                animated.read(i).unwrap(),
                                expected,
                                "{kind:?} animated preview at {size}, channel {channel}, pixel {i}"
                            );
                        }
                    }
                    let i = ((height / 2) * width + width / 4) as usize;
                    let refracted = preview.channel(0, false).unwrap().read(i).unwrap();
                    let original = base
                        .render_unbound(width, height, time)
                        .unwrap()
                        .channel(0, false)
                        .unwrap()
                        .read(i)
                        .unwrap();
                    assert!(
                        (refracted - original).abs() > 0.01,
                        "{kind:?} had no refraction at {size}"
                    );
                    refracted
                })
                .collect();
            assert!(
                samples
                    .iter()
                    .all(|sample| (samples[0] - sample).abs() < 0.005),
                "{kind:?} normalized samples {samples:?}"
            );
        }
    }

    #[test]
    fn bundled_privacy_glass_preview_matches_final_at_two_resolutions() {
        let bytes = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../genetica/Presets/Filter/Transform/Privacy Glass.gtx"
        ));
        check_bundled_preview_and_export(bytes, &[(96, 96), (192, 192)]);
    }

    #[test]
    fn bundled_emboss_previews_and_exports_at_square_and_rectangular_sizes() {
        let bytes = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../genetica/Presets/Filter/Texture Effect/Print & Emboss/Emboss.gtx"
        ));
        check_bundled_preview_and_export(bytes, &[(96, 96), (192, 192), (192, 128)]);
    }

    #[test]
    fn bundled_wood_rings_preview_matches_final_with_regions_gaps_rotation_and_radial() {
        let bytes = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../genetica/Wgroups/Wood/Rings.wtx"
        ));
        let root = bundled_root(bytes);
        let preview = crate::LIVE_PREVIEW_SIZE;
        check_graph_preview_and_export(&root, &[(preview, preview), (128, 80)]);

        let mut pending = vec![root.clone()];
        let regions = loop {
            let node = pending.pop().expect("Wood Rings Regions node");
            if let Some(filter) = node
                .filter()
                .filter(|filter| filter.kind() == PointwiseKind::Regions)
            {
                break filter;
            }
            pending.extend(
                node.slots()
                    .into_iter()
                    .chain(node.contents())
                    .filter_map(|slot| slot.node()),
            );
        };
        regions.parameter(3).unwrap().set_index(0).unwrap();
        check_graph_preview_and_export(&root, &[(96, 96), (128, 80)]);
        regions.parameter(2).unwrap().set_index(1).unwrap();
        check_graph_preview_and_export(&root, &[(96, 96), (128, 80)]);
        regions.parameter(1).unwrap().set_index(1).unwrap();
        check_graph_preview_and_export(&root, &[(96, 96)]);

        regions.parameter(1).unwrap().set_index(0).unwrap();
        regions.parameter(2).unwrap().set_index(5).unwrap();
        regions.parameter(3).unwrap().set_index(1).unwrap();
        root.set_graph_scale(2.).unwrap();
        check_graph_preview_and_export(&root, &[(96, 96), (128, 80)]);
        root.set_graph_scale(1.).unwrap();
        root.apply_graph_size_scale(2.).unwrap();
        check_graph_preview_and_export(&root, &[(96, 96)]);
    }

    #[test]
    fn embedded_environment_previews_and_exports_with_the_same_projection() {
        use mycela_engine::{
            BinaryWriter, EnvironmentBlendMode, EnvironmentField, EnvironmentImage,
            EnvironmentImageProjection, EnvironmentLayerKind, EnvironmentMaskFill,
            EnvironmentMaskHemisphere, EnvironmentRecord, EnvironmentTransformFilter,
            Gtx375GraphDocument, PointwiseKind, RgbColor,
        };
        let bytes = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../genetica/Edit Style Preview/Background/Cloth.jpg"
        ));
        let mut map = EnvironmentRecord::default();
        map.add_layer(EnvironmentLayerKind::Color).unwrap();
        map.set_field(EnvironmentField::Red(0), 0.4).unwrap();
        map.set_field(EnvironmentField::Green(0), 0.2).unwrap();
        map.add_layer(EnvironmentLayerKind::Image).unwrap();
        map.set_layer_image(
            0,
            EnvironmentImage::from_file_bytes(bytes.to_vec(), "jpg").unwrap(),
        )
        .unwrap();
        map.set_image_projection(0, EnvironmentImageProjection::LightProbe)
            .unwrap();
        map.set_mask_hemisphere(0, EnvironmentMaskHemisphere::Lower)
            .unwrap();
        map.set_mask_fill(0, EnvironmentMaskFill::Transparent)
            .unwrap();
        map.set_field(EnvironmentField::MaskBlur(0), 10.).unwrap();
        map.set_transform_filter(Some(0), EnvironmentTransformFilter::Anisotropic)
            .unwrap();
        map.set_field(EnvironmentField::Roll(Some(0)), 15.).unwrap();
        map.set_transform_filter(None, EnvironmentTransformFilter::Diffuse)
            .unwrap();
        map.set_field(EnvironmentField::Heading(None), -20.)
            .unwrap();
        map.set_field(EnvironmentField::Gamma, 1.25).unwrap();
        map.set_recolor_enabled(0, true).unwrap();
        let gradient = map.copy_recolor_gradient(0).unwrap().unwrap();
        gradient
            .color_point(0)
            .unwrap()
            .unwrap()
            .set_color(RgbColor::new(0.15, 0.45, 0.8, 1.));
        gradient.add_alpha(0, 0.55).unwrap();
        map.set_recolor_gradient(0, &gradient).unwrap();
        map.set_field(EnvironmentField::Hue(0), -20.).unwrap();
        map.set_blend_mode(0, EnvironmentBlendMode::Overlay)
            .unwrap();
        map.set_mixer_enabled(0, true).unwrap();
        map.set_field(EnvironmentField::MixerContrast(0), 10.)
            .unwrap();
        let filter =
            mycela_engine::PointwiseFilter::<ForegroundError>::new(PointwiseKind::MakeEnvironment)
                .unwrap();
        filter
            .parameter(0)
            .unwrap()
            .set_environment(std::sync::Arc::new(map))
            .unwrap();
        filter.parameter(1).unwrap().set_index(6).unwrap();
        let mut document = Gtx375GraphDocument::new(vec![Some(filter.node())]).unwrap();
        let mut saved = BinaryWriter::default();
        document.save(&mut saved).unwrap();
        check_bundled_preview_and_export(saved.bytes(), &[(96, 96), (192, 192), (192, 128)]);
    }

    fn check_bundled_preview_and_export(bytes: &[u8], dimensions: &[(i32, i32)]) {
        check_graph_preview_and_export(&bundled_root(bytes), dimensions);
    }

    fn bundled_root(bytes: &[u8]) -> PointwiseNode<ForegroundError> {
        use mycela_engine::{Gtx375GraphDocument, NodeWireReadPolicy};

        let document = Gtx375GraphDocument::<ForegroundError>::load_bounded(
            bytes,
            NodeWireReadPolicy::new(128, 4096),
        )
        .unwrap();
        assert_eq!(document.consumed() as usize, bytes.len());
        document.branch(0).unwrap().unwrap()
    }

    fn check_graph_preview_and_export(
        node: &PointwiseNode<ForegroundError>,
        dimensions: &[(i32, i32)],
    ) {
        let time = 0.;
        let selection = PreviewSelection::BaseColor;
        let snapshot = RenderCaptureSession::new()
            .capture_node(node, time)
            .unwrap();

        for &(width, height) in dimensions {
            let preview = crate::scene_backend::render_export(
                &snapshot,
                mycela_engine::ImageGeometry::new(width, height),
                &|| false,
                crate::scene_backend::GRAPH_PNG_OPTIONS,
                None,
                None,
                None,
            )
            .unwrap();
            let red = preview.materialize(0).unwrap().values();
            assert!(
                red.iter().any(|&sample| sample != red[0]),
                "Preview had a uniform red channel at {width}×{height}"
            );
            let preview = project_bitmap(&preview, &selection).unwrap();
            let final_image = ExportRenderJob::freeze_at_time(
                ExportSource::Graph {
                    node,
                    camera: None,
                    time,
                },
                None,
                &[],
                selection.clone(),
                width,
                height,
                time,
            )
            .unwrap()
            .materialize()
            .unwrap()
            .render(|| false)
            .unwrap();
            let actual = preview.rows().unwrap();
            let expected = final_image.rows().unwrap();
            assert_eq!((final_image.width(), final_image.height()), (width, height));
            assert_eq!(actual.len(), expected.len());
            let differences = actual.iter().zip(&expected).filter(|(a, b)| a != b).count();
            assert_eq!(
                differences, 0,
                "Preview differed from export at {width}×{height}"
            );
        }
    }

    #[test]
    fn still_export_finalizes_solid_and_invert_without_a_gradient() {
        let solid = PointwiseNode::<ForegroundError>::solid_color().unwrap();
        solid.set_solid_color([0.25, 0.0, 0.0]).unwrap();
        let invert = PointwiseNode::invert();
        invert.set_invert_red_only().unwrap();
        invert.slot(0).unwrap().set_node(Some(&solid)).unwrap();
        invert
            .invert_property(0)
            .unwrap()
            .options()
            .unwrap()
            .set_entry(0, Some("No".encode_utf16().collect()))
            .unwrap();
        let pending = ExportRenderJob::freeze_at_time(
            ExportSource::Graph {
                node: &invert,
                camera: None,
                time: 0.0,
            },
            None,
            &[],
            PreviewSelection::BaseColor,
            5,
            3,
            0.0,
        )
        .unwrap();
        let bitmap = pending.materialize().unwrap().render(|| false).unwrap();
        assert_eq!(bitmap.argb_pixel(2, 1).unwrap(), 0xffbf0000);
    }

    #[test]
    fn frozen_graph_export_uses_final_arithmetic_and_hsb_workers() {
        use mycela_engine::{PointwiseRenderEffects, PointwiseRenderOptions, ProceduralExecution};

        let hue = GradientGenerator::<ForegroundError>::new().unwrap();
        let saturation = GradientGenerator::new().unwrap();
        saturation
            .parameter(GradientGeneratorParameterKind::Mode)
            .set_index(1)
            .unwrap();
        let red = |value| {
            let node = PointwiseNode::solid_color().unwrap();
            node.set_solid_color([value, 0., 0.]).unwrap();
            node
        };
        let filter = |kind, inputs: &[PointwiseNode<ForegroundError>]| {
            let node = PointwiseFilter::new(kind).unwrap().node();
            for (index, input) in inputs.iter().enumerate() {
                node.slot(index as i32)
                    .unwrap()
                    .set_node(Some(input))
                    .unwrap();
            }
            node
        };
        let add = filter(PointwiseKind::Add, &[hue.node(), red(0.125)]);
        let multiply = filter(PointwiseKind::Multiply, &[saturation.node(), red(0.5)]);
        let root = filter(PointwiseKind::FromHsb, &[add, multiply, red(0.75)]);

        let mut capacity = |_| unreachable!();
        let mut required = |_, _| unreachable!();
        let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut required);
        let time = 0.375;
        let original = root
            .render_original_with_effects(
                7,
                3,
                PointwiseRenderOptions::new(false, true),
                time,
                ProceduralExecution::CallingThread,
                &mut effects,
            )
            .unwrap();
        assert!(original.caught_errors().is_empty());
        let expected =
            project_bitmap(&original.image().unwrap(), &PreviewSelection::BaseColor).unwrap();

        // The final constructor restores this literal choice. An Original
        // fallback would fail on the captured Hue graph.
        hue.parameter(GradientGeneratorParameterKind::Mode)
            .options()
            .unwrap()
            .set_entry(0, None)
            .unwrap();
        let job = ExportRenderJob::freeze_at_time(
            ExportSource::Graph {
                node: &root,
                camera: None,
                time,
            },
            None,
            &[],
            PreviewSelection::BaseColor,
            7,
            3,
            time,
        )
        .unwrap();
        saturation
            .parameter(GradientGeneratorParameterKind::Location)
            .set_axis(1, 0.5)
            .unwrap();
        let image = job.materialize().unwrap().render(|| false).unwrap();
        assert_eq!(image.rows().unwrap(), expected.rows().unwrap());
    }

    #[test]
    fn dense_gradient_frame_owns_evaluated_time_and_geometry() {
        let model = GradientGenerator::<ForegroundError>::new().unwrap();
        let before = model.render_unbound(7, 3, 0.25).unwrap();
        let expected = project_bitmap(&before, &PreviewSelection::BaseColor).unwrap();
        let job = ExportRenderJob::capture(
            ExportSource::Gradient {
                model: &model,
                time: 0.25,
            },
            None,
            &[],
            PreviewSelection::BaseColor,
            7,
            3,
        )
        .unwrap();
        model
            .parameter(GradientGeneratorParameterKind::Mode)
            .set_index(1)
            .unwrap();
        model
            .parameter(GradientGeneratorParameterKind::Location)
            .set_pair([0.5, 0.5])
            .unwrap();
        let bitmap = job.render(|| false).unwrap();
        assert_eq!(bitmap.rows().unwrap(), expected.rows().unwrap());
        assert_ne!(
            bitmap.argb_pixel(0, 0).unwrap(),
            bitmap.argb_pixel(6, 0).unwrap(),
            "real horizontal samples differ across requested width"
        );
    }
    #[test]
    fn selected_named_occurrence_is_captured_and_missing_selection_errors() {
        let node = solid();
        let name: Vec<u16> = "Height".encode_utf16().collect();
        let a = stage(1, PointwiseKind::StoreMap, [FilterInput::Previous; 3]);
        let b = stage(2, PointwiseKind::StoreMap, [FilterInput::Previous; 3]);
        for s in [&a, &b] {
            s.model()
                .parameter(0)
                .unwrap()
                .set_text(Some(&name))
                .unwrap();
        }
        let green = stage(3, PointwiseKind::GetChannel, [FilterInput::Previous; 3]);
        green.model().parameter(0).unwrap().set_index(1).unwrap();
        let selected = PreviewSelection::Named {
            name: name.clone(),
            occurrence: 1,
        };
        let job = ExportRenderJob::capture(
            ExportSource::Solid(&node),
            None,
            &[a.clone(), green, b.clone()],
            selected,
            5,
            2,
        )
        .unwrap();
        b.model()
            .parameter(0)
            .unwrap()
            .set_text(Some(&"Changed".encode_utf16().collect::<Vec<_>>()))
            .unwrap();
        let bitmap = job.render(|| false).unwrap();
        assert_eq!((bitmap.width(), bitmap.height()), (5, 2));
        assert_eq!(bitmap.argb_pixel(4, 1).unwrap(), 0xff666666);
        let absent = ExportRenderJob::capture(
            ExportSource::Solid(&node),
            None,
            &[],
            PreviewSelection::Named {
                name,
                occurrence: 0,
            },
            5,
            2,
        )
        .unwrap();
        assert!(
            absent
                .render(|| false)
                .unwrap_err()
                .to_string()
                .contains("selected map is no longer available")
        );
    }
    #[test]
    fn frozen_gradient_export_projects_the_named_map_from_final_render() {
        use mycela_engine::PointwiseRenderOptions;
        let gradient = GradientGenerator::<ForegroundError>::new().unwrap();
        gradient
            .parameter(GradientGeneratorParameterKind::Location)
            .set_axis(0, 0.25)
            .unwrap();
        let name: Vec<u16> = "Height".encode_utf16().collect();
        let store = PointwiseFilter::new(PointwiseKind::StoreMap).unwrap();
        store.parameter(0).unwrap().set_text(Some(&name)).unwrap();
        let node = store.node();
        node.slot(0)
            .unwrap()
            .set_node(Some(&gradient.node()))
            .unwrap();
        let selection = PreviewSelection::Named {
            name,
            occurrence: 0,
        };
        let source = gradient.render_unbound(7, 3, 0.375).unwrap();
        let expected = store
            .render_supplied_with_options(
                &[Some(source)],
                7,
                3,
                PointwiseRenderOptions::new(false, true),
            )
            .unwrap()
            .unwrap();
        let expected = project_bitmap(&expected, &selection).unwrap();

        gradient
            .parameter(GradientGeneratorParameterKind::Mode)
            .options()
            .unwrap()
            .set_entry(0, None)
            .unwrap();
        let pending = ExportRenderJob::freeze_at_time(
            ExportSource::Graph {
                node: &node,
                camera: None,
                time: 0.375,
            },
            None,
            &[],
            selection,
            7,
            3,
            0.375,
        )
        .unwrap();
        store
            .parameter(0)
            .unwrap()
            .set_text(Some(&"Replaced".encode_utf16().collect::<Vec<_>>()))
            .unwrap();
        let exported = pending.materialize().unwrap().render(|| false).unwrap();
        assert_eq!(exported.rows().unwrap(), expected.rows().unwrap());
    }

    #[test]
    fn routes_filter_options_and_invert_are_owned_and_failures_do_not_fall_back() {
        let node = solid();
        let get = stage(7, PointwiseKind::GetChannel, [FilterInput::Previous; 3]);
        get.model().parameter(0).unwrap().set_index(0).unwrap();
        let add = stage(
            9,
            PointwiseKind::Add,
            [
                FilterInput::Filter(7),
                FilterInput::Source,
                FilterInput::None,
            ],
        );
        let invert =
            SharedInvertNode::<ForegroundError>::new(mycela_engine::SelectionCloneMode::Original);
        let live = mycela_engine::SharedImageMap::from_image(node.render(4, 2, 0.).unwrap());
        let stages = [get.clone(), add.clone()];
        let expected = render_chain(
            live,
            Some(RenderInvert::Live(&invert)),
            stages.iter().map(|stage| RenderStage::Live(stage, 0.)),
            RenderGeometry::new(4, 2).unwrap(),
            &|| false,
        )
        .unwrap();
        let expected = project_bitmap(&expected, &PreviewSelection::BaseColor).unwrap();
        let job = ExportRenderJob::capture(
            ExportSource::Solid(&node),
            Some(&invert),
            &stages,
            PreviewSelection::BaseColor,
            4,
            2,
        )
        .unwrap();
        get.model()
            .parameter(0)
            .unwrap()
            .options()
            .unwrap()
            .set_entry(0, Some("Blue".encode_utf16().collect()))
            .unwrap();
        get.model().parameter(0).unwrap().set_index(2).unwrap();
        invert
            .property(0)
            .unwrap()
            .options()
            .set_entry(0, Some("No".encode_utf16().collect()))
            .unwrap();
        assert_eq!(
            job.render(|| false).unwrap().rows().unwrap(),
            expected.rows().unwrap()
        );
        let bad = stage(
            10,
            PointwiseKind::Multiply,
            [
                FilterInput::Filter(999),
                FilterInput::Source,
                FilterInput::None,
            ],
        );
        assert!(
            capture(&node, &[bad], 4, 2)
                .render(|| false)
                .unwrap_err()
                .to_string()
                .contains("#999")
        );
    }
    #[test]
    fn layer_map_choice_refresh_never_changes_live_choices() {
        let node = solid();
        let store = stage(1, PointwiseKind::StoreMap, [FilterInput::Previous; 3]);
        store
            .model()
            .parameter(0)
            .unwrap()
            .set_text(Some(&"Height".encode_utf16().collect::<Vec<_>>()))
            .unwrap();
        let layer = stage(2, PointwiseKind::LayerMaps, [FilterInput::Previous; 3]);
        let first = layer.model().parameter(0).unwrap();
        let options = first.options().unwrap();
        let entries = options.entries();
        let index = first.index();
        let second = layer.model().parameter(1).unwrap();
        let second_entries = second.options().unwrap().entries();
        let job = capture(&node, &[store, layer.clone()], 5, 2);
        // A refresh (including a subsequent source render error) belongs only
        // to the detached job, never to these retained live option arrays.
        let _result = job.render(|| false);
        assert_eq!(options.entries(), entries);
        assert_eq!(first.index(), index);
        assert_eq!(second.options().unwrap().entries(), second_entries);
        assert!(first.options().unwrap().same_reference(&options));
    }
    #[test]
    fn cancellation_has_stage_and_conversion_checkpoints() {
        let node = solid();
        assert!(
            capture(&node, &[], 4, 2)
                .render(|| true)
                .unwrap_err()
                .is_cancelled()
        );
        let filters = [
            stage(
                1,
                PointwiseKind::Add,
                [FilterInput::Source, FilterInput::None, FilterInput::None],
            ),
            stage(
                2,
                PointwiseKind::Multiply,
                [
                    FilterInput::Previous,
                    FilterInput::Source,
                    FilterInput::None,
                ],
            ),
        ];
        for stop in [2, 4, 6, 9] {
            let calls = Cell::new(0);
            let result = capture(&node, &filters, 4, 2).render(|| {
                calls.set(calls.get() + 1);
                calls.get() >= stop
            });
            assert!(result.unwrap_err().is_cancelled(), "checkpoint {stop}");
        }
    }
    #[test]
    fn gradient_jobs_stop_in_the_pixel_worker_before_projection() {
        let model = GradientGenerator::<ForegroundError>::new().unwrap();
        let node = model.node();
        for source in [
            ExportSource::Gradient {
                model: &model,
                time: 0.,
            },
            ExportSource::Graph {
                node: &node,
                camera: None,
                time: 0.,
            },
        ] {
            let job =
                ExportRenderJob::capture(source, None, &[], PreviewSelection::BaseColor, 257, 257)
                    .unwrap();
            let calls = Cell::new(0);
            let result = job.render(|| {
                calls.set(calls.get() + 1);
                calls.get() == 20
            });
            assert!(result.unwrap_err().is_cancelled());
        }
    }

    #[test]
    fn captured_dense_combine_uses_actual_system_policy_at_non_preview_width() {
        use mycela_engine::PointwiseRenderOptions;
        let source = GradientGenerator::<ForegroundError>::new().unwrap();
        let invert =
            SharedInvertNode::<ForegroundError>::new(mycela_engine::SelectionCloneMode::Original);
        let combine = stage(
            1,
            PointwiseKind::Combine,
            [
                FilterInput::Previous,
                FilterInput::Source,
                FilterInput::None,
            ],
        );
        let (width, height) = (501, 3);
        let image = source.render_unbound(width, height, 0.25).unwrap();
        let top = invert.render_shared(Some(&image), width, height).unwrap();
        let empty =
            mycela_engine::SharedImageMap::new(mycela_engine::ImageGeometry::new(width, height));
        // At this valid geometry both policy outcomes choose one pass; the
        // existing explicit helper is an independent numeric reference path.
        let expected = combine
            .model()
            .render_combine_with_capacity(
                &[Some(top), Some(image), Some(empty)],
                width,
                height,
                PointwiseRenderOptions::new(false, true),
                true,
            )
            .unwrap()
            .unwrap();
        let expected = project_bitmap(&expected, &PreviewSelection::BaseColor).unwrap();
        let job = ExportRenderJob::capture(
            ExportSource::Gradient {
                model: &source,
                time: 0.25,
            },
            Some(&invert),
            &[combine],
            PreviewSelection::BaseColor,
            width,
            height,
        )
        .unwrap();
        let bitmap = job.render(|| false).unwrap();
        assert_eq!((bitmap.width(), bitmap.height()), (width, height));
        assert_eq!(bitmap.rows().unwrap(), expected.rows().unwrap());
    }
    #[test]
    fn procedural_export_owns_committed_values_and_renders_chosen_geometry() {
        use mycela_engine::{
            ImageGenerator, ImageGeneratorKind, ImageGeneratorParameterKind as Parameter,
        };
        for kind in [
            ImageGeneratorKind::Noise,
            ImageGeneratorKind::Cells,
            ImageGeneratorKind::WoodRings,
        ] {
            let model = ImageGenerator::<ForegroundError>::with_kind(kind).unwrap();
            model.parameter(Parameter::Seed).set_number(-11.).unwrap();
            if kind == ImageGeneratorKind::Noise {
                model
                    .parameter(Parameter::Iterations)
                    .set_number(3.)
                    .unwrap();
            }
            let frame = model.capture_render(0.).unwrap();
            let expected = frame
                .render_with_preflight::<ForegroundError>(
                    7,
                    5,
                    generator_execution(),
                    &mut crate::memory_capacity::noise_preflight,
                )
                .unwrap();
            let expected = project_bitmap(&expected, &PreviewSelection::BaseColor).unwrap();
            let job = ExportRenderJob::capture(
                ExportSource::Gradient {
                    model: &model,
                    time: 0.,
                },
                None,
                &[],
                PreviewSelection::BaseColor,
                7,
                5,
            )
            .unwrap();
            model.parameter(Parameter::Seed).set_number(19.).unwrap();
            let actual = job.render(|| false).unwrap();
            assert_eq!((actual.width(), actual.height()), (7, 5));
            for y in 0..5 {
                for x in 0..7 {
                    assert_eq!(
                        actual.argb_pixel(x, y).unwrap(),
                        expected.argb_pixel(x, y).unwrap()
                    );
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "scene_export_device.rs"]
mod scene_export_device;
