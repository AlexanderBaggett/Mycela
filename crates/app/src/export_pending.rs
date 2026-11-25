//! Exact click-time scene operands, without spill reads or live owner aliases.
use super::*;
use crate::filter_pipeline::{FrozenFilterValues, PendingFilterStage};
use mycela_engine::{
    FrozenRenderBatch, FrozenRenderValues, PendingGradientRender, PendingNodeRender,
    RenderFreezeSession,
};

enum PendingSource {
    Solid(SolidColorNodeSnapshot),
    Graph(
        PendingNodeRender<ForegroundError>,
        Option<crate::scene_backend::CameraFrame>,
    ),
    Gradient(PendingGradientRender<ForegroundError>),
}
#[derive(Clone)]
enum FrozenSource {
    Solid(SolidColorNodeSnapshot),
    Graph(
        FrozenRenderValues,
        Option<crate::scene_backend::CameraFrame>,
    ),
    Gradient(FrozenRenderValues),
}
#[derive(Clone)]
pub(crate) struct FrozenSceneValues {
    source: FrozenSource,
    invert: Option<InvertRenderSnapshot>,
    filters: Vec<FrozenFilterValues>,
    selection: PreviewSelection,
    geometry: RenderGeometry,
    time: f64,
}
impl FrozenSceneValues {
    pub(crate) fn same_values(&self, other: &Self) -> bool {
        let source = match (&self.source, &other.source) {
            (FrozenSource::Solid(a), FrozenSource::Solid(b)) => {
                a.mode == b.mode
                    && a.color
                        .iter()
                        .zip(b.color)
                        .all(|(a, b)| a.to_bits() == b.to_bits())
                    && a.transparency.to_bits() == b.transparency.to_bits()
            }
            (FrozenSource::Gradient(a), FrozenSource::Gradient(b)) => a == b,
            (FrozenSource::Graph(a, ac), FrozenSource::Graph(b, bc)) => {
                a == b && same_optional_camera(ac, bc)
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
            && self.filters == other.filters
            && self.selection == other.selection
            && self.geometry == other.geometry
            && self.time.to_bits() == other.time.to_bits()
    }
}
pub(crate) struct PendingExportRenderJob {
    source: PendingSource,
    graph_cache: Option<PointwiseFinalExportCache<ForegroundError>>,
    environment_library: Option<mycela_engine::EnvironmentResourceLibrary>,
    invert: Option<InvertRenderSnapshot>,
    filters: Vec<PendingFilterStage>,
    selection: PreviewSelection,
    geometry: RenderGeometry,
    time: f64,
    batch: FrozenRenderBatch,
}
impl PendingExportRenderJob {
    pub(crate) fn with_graph_cache(
        mut self,
        cache: PointwiseFinalExportCache<ForegroundError>,
    ) -> Self {
        self.graph_cache = Some(cache);
        self
    }

    pub(crate) fn with_environment_library(
        mut self,
        library: mycela_engine::EnvironmentResourceLibrary,
    ) -> Self {
        self.environment_library = Some(library);
        self
    }

    pub(crate) fn frozen_values(&self) -> FrozenSceneValues {
        FrozenSceneValues {
            source: match &self.source {
                PendingSource::Solid(v) => FrozenSource::Solid(v.clone()),
                PendingSource::Graph(v, camera) => {
                    FrozenSource::Graph(v.frozen_values().clone(), camera.clone())
                }
                PendingSource::Gradient(v) => FrozenSource::Gradient(v.frozen_values().clone()),
            },
            invert: self.invert.clone(),
            filters: self
                .filters
                .iter()
                .map(PendingFilterStage::frozen_values)
                .collect(),
            selection: self.selection.clone(),
            geometry: self.geometry,
            time: self.time,
        }
    }
    pub(crate) fn materialize(self) -> Result<ExportRenderJob, ExportRenderError> {
        let mut batch = self.batch.materialize().map_err(failure)?;
        let source = match self.source {
            PendingSource::Solid(v) => SourceFrame::Solid(v),
            PendingSource::Graph(v, camera) => {
                SourceFrame::Graph(v.complete(&mut batch).map_err(failure)?, camera)
            }
            PendingSource::Gradient(v) => {
                SourceFrame::Gradient(v.complete(&mut batch).map_err(failure)?)
            }
        };
        let filters = self
            .filters
            .into_iter()
            .map(|v| v.complete(&mut batch))
            .collect::<Result<Vec<_>, _>>()?;
        // Complete batch/session contains only detached references. Retire it
        // before rendering so mutable worker caches cannot change comparisons.
        drop(batch);
        Ok(ExportRenderJob {
            source,
            graph_cache: self.graph_cache,
            environment_library: self.environment_library,
            invert: self.invert,
            filters,
            selection: self.selection,
            geometry: self.geometry,
            time: self.time,
        })
    }
}
impl ExportRenderJob {
    pub(crate) fn freeze_at_time(
        source: ExportSource<'_>,
        invert: Option<&SharedInvertNode<ForegroundError>>,
        filters: &[FilterStage],
        selection: PreviewSelection,
        width: i32,
        height: i32,
        time: f64,
    ) -> Result<PendingExportRenderJob, ExportRenderError> {
        let geometry = RenderGeometry::new(width, height)?;
        let mut session = RenderFreezeSession::new();
        let source = match source {
            ExportSource::Solid(v) => {
                PendingSource::Solid(v.capture_preview_at(time).map_err(failure)?)
            }
            ExportSource::FinalSolid(v) => {
                PendingSource::Solid(v.capture_final_at(time).map_err(failure)?)
            }
            ExportSource::Graph {
                node,
                camera,
                time: source_time,
            } => {
                if source_time.to_bits() != time.to_bits() {
                    return Err(failure("The source and filter sample times differ."));
                }
                PendingSource::Graph(
                    session.freeze_node(node, time).map_err(failure)?,
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
                PendingSource::Gradient(session.freeze_gradient(model, time).map_err(failure)?)
            }
        };
        let invert = invert
            .map(|v| v.capture_render().map_err(|e| failure(format!("{e:?}"))))
            .transpose()?;
        let filters = filters
            .iter()
            .map(|v| v.freeze_render(&mut session, time))
            .collect::<Result<Vec<_>, _>>()?;
        let batch = session.finish().map_err(failure)?;
        Ok(PendingExportRenderJob {
            source,
            graph_cache: None,
            environment_library: None,
            invert,
            filters,
            selection,
            geometry,
            time,
            batch,
        })
    }
}

#[cfg(test)]
mod laws {
    use super::*;
    use mycela_engine::SolidColorCloneMode;
    #[test]
    fn consecutive_graph_png_jobs_share_eligible_final_images() {
        let gradient = GradientGenerator::<ForegroundError>::new().unwrap();
        let mut nodes = vec![gradient.node()];
        for _ in 0..9 {
            let invert = PointwiseNode::invert();
            invert.slot(0).unwrap().set_node(nodes.last()).unwrap();
            nodes.push(invert);
        }
        let cache = PointwiseFinalExportCache::default();
        let render = || {
            ExportRenderJob::freeze_at_time(
                ExportSource::Graph {
                    node: &nodes[6],
                    camera: None,
                    time: 0.,
                },
                None,
                &[],
                PreviewSelection::BaseColor,
                8,
                4,
                0.,
            )
            .unwrap()
            .with_graph_cache(cache.clone())
            .materialize()
            .unwrap()
            .render_scene(|| false, false)
            .unwrap()
        };
        let first = render();
        let second = render();
        assert!(first.image.same_reference(&second.image));
        assert_eq!(first.bitmap.rows().unwrap(), second.bitmap.rows().unwrap());
        nodes[9].invert_property(0).unwrap().set_index(1).unwrap();
        let downstream_edit = render();
        assert!(first.image.same_reference(&downstream_edit.image));
        gradient
            .parameter(mycela_engine::GradientGeneratorParameterKind::Location)
            .set_axis(0, 0.5)
            .unwrap();
        let edited = render();
        assert!(!first.image.same_reference(&edited.image));
        assert_ne!(first.bitmap.rows().unwrap(), edited.bitmap.rows().unwrap());
    }

    #[test]
    fn polygon_graph_freeze_keeps_click_time_values_for_image_export() {
        let identity: Vec<u16> = "Polygon".encode_utf16().collect();
        let node =
            mycela_engine::create_node_by_identity::<ForegroundError>(Some(&identity)).unwrap();
        let edges = node.filter().unwrap().parameter(2).unwrap();
        let freeze = || {
            ExportRenderJob::freeze_at_time(
                ExportSource::Graph {
                    node: &node,
                    camera: None,
                    time: 0.,
                },
                None,
                &[],
                PreviewSelection::BaseColor,
                48,
                48,
                0.,
            )
            .unwrap()
        };
        let first = freeze();
        let first_values = first.frozen_values();
        edges.set_number(6.).unwrap();
        let second = freeze();
        assert!(!first_values.same_values(&second.frozen_values()));
        let first = first.materialize().unwrap();
        let second = second.materialize().unwrap();
        assert!(!first.same_render_values(&second));
        let old_rows = first.render(|| false).unwrap().rows().unwrap();
        let new_rows = second.render(|| false).unwrap().rows().unwrap();
        assert_ne!(old_rows, new_rows);
        assert_eq!(edges.number(), 6.);
    }
    #[test]
    fn scene_camera_is_captured_and_compared_without_mutating_live_camera() {
        let primitive = mycela_engine::ScenePrimitiveNode::<ForegroundError>::new(
            mycela_engine::ScenePrimitiveKind::Cube,
        )
        .unwrap();
        let node = primitive.node();
        let camera = mycela_engine::SceneCamera::source_preview();
        let freeze = || {
            ExportRenderJob::freeze_at_time(
                ExportSource::Graph {
                    node: &node,
                    camera: Some(&camera),
                    time: 0.,
                },
                None,
                &[],
                PreviewSelection::BaseColor,
                8,
                6,
                0.,
            )
            .unwrap()
        };
        let first = freeze();
        let values = first.frozen_values();
        camera
            .set_orbit_component(mycela_engine::CameraOrbitComponent::Azimuth, 90.)
            .unwrap();
        let second = freeze();
        assert!(!values.same_values(&second.frozen_values()));
        let first = first.materialize().unwrap();
        let second = second.materialize().unwrap();
        assert!(!first.same_render_values(&second));
        assert!(matches!(
            first.render(|| true),
            Err(ExportRenderError::Cancelled)
        ));
        assert_eq!(camera.orbit_values().unwrap()[1], 90.);
    }
    #[test]
    fn scene_freeze_retains_committed_node_and_cancel_avoids_device_initialization() {
        let primitive = mycela_engine::ScenePrimitiveNode::<ForegroundError>::new(
            mycela_engine::ScenePrimitiveKind::Cube,
        )
        .unwrap();
        let node = primitive.node();
        let camera = mycela_engine::SceneCamera::source_preview();
        let pending = ExportRenderJob::freeze_at_time(
            ExportSource::Graph {
                node: &node,
                camera: Some(&camera),
                time: 0.25,
            },
            None,
            &[],
            PreviewSelection::BaseColor,
            7,
            5,
            0.25,
        )
        .unwrap();
        let before = pending.frozen_values();
        primitive
            .property(0)
            .unwrap()
            .set_vector3d([2.; 3])
            .unwrap();
        let current = ExportRenderJob::freeze_at_time(
            ExportSource::Graph {
                node: &node,
                camera: Some(&camera),
                time: 0.25,
            },
            None,
            &[],
            PreviewSelection::BaseColor,
            7,
            5,
            0.25,
        )
        .unwrap();
        assert!(!before.same_values(&current.frozen_values()));
        let first = pending.materialize().unwrap();
        let second = current.materialize().unwrap();
        assert!(!first.same_render_values(&second));
        assert!(matches!(
            first.render(|| true),
            Err(ExportRenderError::Cancelled)
        ));
    }
    #[test]
    fn materialization_uses_click_time_commits_after_live_edits_and_cancel_drops_no_live_owner() {
        let node =
            SharedSolidColorNode::<ForegroundError>::new_unattached(SolidColorCloneMode::Original)
                .unwrap();
        node.color_property().set_color([0.2, 0.4, 0.8]).unwrap();
        let expected = ExportRenderJob::capture_at_time(
            ExportSource::Solid(&node),
            None,
            &[],
            PreviewSelection::BaseColor,
            7,
            5,
            0.,
        )
        .unwrap()
        .render(|| false)
        .unwrap()
        .rows()
        .unwrap();
        let pending = ExportRenderJob::freeze_at_time(
            ExportSource::Solid(&node),
            None,
            &[],
            PreviewSelection::BaseColor,
            7,
            5,
            0.,
        )
        .unwrap();
        node.color_property().set_color([0.8, 0.4, 0.2]).unwrap();
        let current = ExportRenderJob::freeze_at_time(
            ExportSource::Solid(&node),
            None,
            &[],
            PreviewSelection::BaseColor,
            7,
            5,
            0.,
        )
        .unwrap();
        assert!(
            !pending
                .frozen_values()
                .same_values(&current.frozen_values())
        );
        drop(current);
        let actual = std::thread::spawn(move || {
            pending
                .materialize()
                .unwrap()
                .render(|| false)
                .unwrap()
                .rows()
                .unwrap()
        })
        .join()
        .unwrap();
        assert_eq!(actual, expected);
        assert_eq!(node.snapshot().color, [0.8, 0.4, 0.2]);
    }
}
