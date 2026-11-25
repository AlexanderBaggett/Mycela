//! Owned scene rendering. Source, effects, routes and time are detached before
//! background work; live Layer Maps controls remain with the actual editor.
use super::*;

#[derive(Default)]
pub(super) struct GeneratorPreview {
    serial: Arc<AtomicU64>,
    task: Option<Task<()>>,
    busy: bool,
    switching: bool,
}
impl GeneratorPreview {
    pub(super) fn retire(&mut self) {
        self.serial.fetch_add(1, Ordering::AcqRel);
        self.busy = false;
        self.switching = false;
        self.task.take();
    }
    pub(super) fn is_busy(&self) -> bool {
        self.busy
    }
    pub(super) fn is_switching(&self) -> bool {
        self.switching
    }
}
impl Drop for GeneratorPreview {
    fn drop(&mut self) {
        self.retire();
    }
}
impl SolidColorView {
    pub(super) fn generator_view(
        &self,
        kind: TextureSourceKind,
    ) -> Option<Entity<GradientSourceView>> {
        if kind == TextureSourceKind::Gradient {
            self.gradient.clone()
        } else {
            self.procedural.get(&kind).cloned()
        }
    }
    pub(super) fn active_generator(&self) -> Option<Entity<GradientSourceView>> {
        self.generator_view(self.source)
    }
    pub(super) fn active_scene(&self) -> Option<Entity<SceneSourceView>> {
        self.scenes.get(&self.source).cloned()
    }
    pub(super) fn active_polygon(&self) -> Option<Entity<PolygonSourceView>> {
        (self.source == TextureSourceKind::Polygon)
            .then(|| self.polygon.clone())
            .flatten()
    }
    fn prepare_generator(
        &mut self,
        source: TextureSourceKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), ForegroundError> {
        if source == TextureSourceKind::SolidColor || self.generator_view(source).is_some() {
            return Ok(());
        }
        if source == TextureSourceKind::Polygon {
            if self.polygon.is_none() {
                let name: Vec<u16> = "Polygon".encode_utf16().collect();
                let node = mycela_engine::create_node_by_identity(Some(&name))
                    .map_err(|error| ForegroundError::Operation(error.to_string()))?;
                let inspector =
                    crate::graph_document::inspector::Inspector::load(&node, "polygon", window, cx)
                        .map_err(ForegroundError::Operation)?;
                let view =
                    cx.new(|cx| PolygonSourceView::new(node, inspector, &self.runtime, window, cx));
                let handle = window.window_handle();
                self.polygon_time = Some(cx.subscribe(
                    &view,
                    move |view, _, event: &crate::spatial_filter::SampleTime, cx| {
                        let time = event.value;
                        let activation = event.activation.clone();
                        let epoch = view.generation.load(Ordering::Acquire);
                        cx.spawn(async move |owner, cx| {
                            let _ = cx.update_window(handle, |_, window, cx| {
                                let _ = owner.update(cx, |view, cx| {
                                    if view.source == TextureSourceKind::Polygon
                                        && view.generation.load(Ordering::Acquire) == epoch
                                        && activation
                                            .upgrade()
                                            .is_some_and(|v| v.load(Ordering::Acquire))
                                    {
                                        view.set_sample_time(time, window, cx);
                                    }
                                });
                            });
                        })
                        .detach();
                    },
                ));
                self.polygon = Some(view);
            }
            return Ok(());
        }
        if let Some(kind) = source.scene_kind() {
            if !self.scenes.contains_key(&source) {
                let model = mycela_engine::ScenePrimitiveNode::new(kind)
                    .map_err(|e| ForegroundError::Operation(e.to_string()))?;
                let view = cx.new(|cx| SceneSourceView::new(model, &self.runtime, window, cx));
                self.scenes.insert(source, view);
            }
            return Ok(());
        }
        let kind = source
            .generator_kind()
            .ok_or_else(|| ForegroundError::Operation("Select a generator first.".into()))?;
        let model = mycela_engine::ImageGenerator::with_kind(kind)
            .map_err(|e| ForegroundError::Operation(e.to_string()))?;
        let view = cx.new(|cx| GradientSourceView::new(model, &self.runtime, window, cx));
        if source == TextureSourceKind::Gradient {
            self.gradient = Some(view);
        } else {
            self.procedural.insert(source, view);
        }
        Ok(())
    }
    pub(super) fn begin_generator_switch(
        &mut self,
        source: TextureSourceKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = self
            .prepare_generator(source, window, cx)
            .and_then(|()| self.request_generator_preview(source, true, cx))
        {
            self.pipeline_error = Some(error.to_string());
        }
        cx.notify();
    }
    fn freeze_preview_scene(
        &self,
        source: TextureSourceKind,
        switching: bool,
        cx: &App,
    ) -> Result<crate::export_render::PendingExportRenderJob, ForegroundError> {
        use crate::export_render::{ExportRenderJob, ExportSource};
        let filters = self.filters.read(cx).pipeline();
        let invert = self.invert.as_ref().map(LiveInvert::model);
        let selection = if switching {
            PreviewSelection::BaseColor
        } else {
            self.preview_selection.clone()
        };
        let result = if source == TextureSourceKind::SolidColor {
            ExportRenderJob::freeze_at_time(
                ExportSource::Solid(&self.node),
                invert.as_ref(),
                &filters,
                selection,
                LIVE_PREVIEW_SIZE,
                LIVE_PREVIEW_SIZE,
                self.solid_switch_time(cx),
            )
        } else if source == TextureSourceKind::Polygon {
            let node = self
                .polygon
                .as_ref()
                .ok_or_else(|| {
                    ForegroundError::Operation("The Polygon source is unavailable.".into())
                })?
                .read(cx)
                .node();
            let time = self.sample_time(cx);
            ExportRenderJob::freeze_at_time(
                ExportSource::Graph {
                    node: &node,
                    camera: None,
                    time,
                },
                invert.as_ref(),
                &filters,
                selection,
                LIVE_PREVIEW_SIZE,
                LIVE_PREVIEW_SIZE,
                time,
            )
        } else if source.scene_kind().is_some() {
            let scene = self
                .scenes
                .get(&source)
                .ok_or_else(|| {
                    ForegroundError::Operation("The scene source is unavailable.".into())
                })?
                .read(cx);
            let node = scene.node();
            let camera = self.scene_camera.read(cx).camera();
            let time = self.sample_time(cx);
            ExportRenderJob::freeze_at_time(
                ExportSource::Graph {
                    node: &node,
                    camera: Some(&camera),
                    time,
                },
                invert.as_ref(),
                &filters,
                selection,
                LIVE_PREVIEW_SIZE,
                LIVE_PREVIEW_SIZE,
                time,
            )
        } else {
            let (model, time) = if source == TextureSourceKind::ImportedImage {
                if let Some(candidate) = self.import_candidate_model() {
                    candidate
                } else {
                    let generator = self.generator_view(source).ok_or_else(|| {
                        ForegroundError::Operation("The imported image is unavailable.".into())
                    })?;
                    let generator = generator.read(cx);
                    (generator.model(), generator.preview_time())
                }
            } else {
                let generator = self.generator_view(source).ok_or_else(|| {
                    ForegroundError::Operation("The generator is unavailable.".into())
                })?;
                let generator = generator.read(cx);
                (generator.model(), generator.preview_time())
            };
            ExportRenderJob::freeze_at_time(
                ExportSource::Gradient {
                    model: &model,
                    time,
                },
                invert.as_ref(),
                &filters,
                selection,
                LIVE_PREVIEW_SIZE,
                LIVE_PREVIEW_SIZE,
                time,
            )
        };
        result.map_err(|e| ForegroundError::Operation(e.to_string()))
    }
    pub(super) fn request_generator_preview(
        &mut self,
        source: TextureSourceKind,
        switching: bool,
        cx: &mut App,
    ) -> Result<(), ForegroundError> {
        self.generator_preview.retire();
        let pending = self.freeze_preview_scene(source, switching, cx)?;
        let epoch = self.generation.load(Ordering::Acquire);
        let serial = Arc::clone(&self.generator_preview.serial);
        let request = serial.load(Ordering::Acquire);
        let owner = self.dispatcher.owner.clone();
        let window_handle = self.dispatcher.window;
        let background = cx.background_executor().clone();
        self.generator_preview.busy = true;
        self.generator_preview.switching = switching;
        self.generator_preview.task = Some(cx.spawn(async move |cx| {
            let worker_serial = serial.clone();
            let work = background
                .spawn(async move {
                    let frame = pending.materialize()?;
                    let observed = frame.clone();
                    let rendered = frame
                        .render_preview_scene(|| worker_serial.load(Ordering::Acquire) != request);
                    Ok::<_, crate::export_render::ExportRenderError>((observed, rendered))
                })
                .await;
            let (observed, rendered) = match work {
                Ok(work) => work,
                Err(error) => {
                    let _ = cx.update_window(window_handle, |_, window, cx| {
                        let _ = owner.update(cx, |view, cx| {
                            if serial.load(Ordering::Acquire) == request
                                && view.generation.load(Ordering::Acquire) == epoch
                            {
                                view.fail_generator_capture(source, error.to_string(), window, cx);
                            }
                        });
                    });
                    return;
                }
            };
            let mut rendered = Some(rendered);
            loop {
                // Freeze P1 on the owner thread; only detached roots/leases cross
                // into the validation worker. No live model is sent later.
                let current = cx.update_window(window_handle, |_, window, cx| {
                    owner.update(cx, |view, cx| {
                        if serial.load(Ordering::Acquire) != request
                            || view.generation.load(Ordering::Acquire) != epoch
                        {
                            return None;
                        }
                        match view.freeze_preview_scene(source, switching, cx) {
                            Ok(pending) => Some(pending),
                            Err(error) => {
                                view.fail_generator_capture(source, error.to_string(), window, cx);
                                None
                            }
                        }
                    })
                });
                let Ok(Ok(Some(current))) = current else {
                    return;
                };
                let guard = current.frozen_values();
                let observed = observed.clone();
                let equal = background
                    .spawn(async move {
                        current
                            .materialize()
                            .map(|current| observed.same_render_values(&current))
                    })
                    .await;
                let retry_validation = cx.update_window(window_handle, |_, window, cx| {
                    owner.update(cx, |view, cx| {
                        if serial.load(Ordering::Acquire) != request
                            || view.generation.load(Ordering::Acquire) != epoch
                        {
                            return false;
                        }
                        match equal {
                            Err(error) => {
                                view.fail_generator_capture(source, error.to_string(), window, cx);
                                false
                            }
                            Ok(false) => {
                                if let Err(error) =
                                    view.request_generator_preview(source, switching, cx)
                                {
                                    view.fail_generator_capture(
                                        source,
                                        error.to_string(),
                                        window,
                                        cx,
                                    );
                                }
                                cx.notify();
                                false
                            }
                            Ok(true) => {
                                let current = match view.freeze_preview_scene(source, switching, cx)
                                {
                                    Ok(current) => current,
                                    Err(error) => {
                                        view.fail_generator_capture(
                                            source,
                                            error.to_string(),
                                            window,
                                            cx,
                                        );
                                        return false;
                                    }
                                };
                                // P2 closes silent shared-handle edits during P1's
                                // worker validation. This token never replaces the
                                // full materialized semantic comparison above.
                                if !guard.same_values(&current.frozen_values()) {
                                    return true;
                                }
                                if let Some(rendered) = rendered.take() {
                                    view.finish_generator_preview(
                                        source, switching, rendered, window, cx,
                                    );
                                }
                                false
                            }
                        }
                    })
                });
                if !matches!(retry_validation, Ok(Ok(true))) {
                    return;
                }
            }
        }));
        Ok(())
    }
    fn fail_generator_capture(
        &mut self,
        source: TextureSourceKind,
        error: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.generator_preview.busy = false;
        self.generator_preview.switching = false;
        if source == TextureSourceKind::ImportedImage && self.import_candidate_model().is_some() {
            self.fail_image_import(error.clone(), window, cx);
        }
        self.pipeline_error = Some(error);
        cx.notify();
    }
    fn finish_generator_preview(
        &mut self,
        source: TextureSourceKind,
        switching: bool,
        rendered: crate::export_render::PreviewSceneResult,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let result = rendered.result;
        let layer_inputs = rendered.layer_inputs;
        // Reconcile reached immediate inputs even when a downstream
        // stage failed. Otherwise its real map menus could stay stale
        // while the last successful pixels are correctly retained.
        let reached = result.as_ref().map_or(layer_inputs.as_slice(), |scene| {
            scene.layer_inputs.as_slice()
        });
        let controls = self.filters.update(cx, |filters, cx| {
            filters.reconcile_layer_inputs(reached, cx)
        });
        let result = controls.and_then(|changed| {
            if changed {
                return Ok(None);
            }
            let scene = result.map_err(|e| ForegroundError::Operation(e.to_string()))?;
            let raster = map_preview::raster(&scene.bitmap)?;
            Ok(Some(PreparedPreview {
                image: scene.image,
                selection: scene.selection,
                bitmap: scene.bitmap,
                raster,
            }))
        });
        match result {
            Ok(None) => {
                if let Err(e) = self.request_generator_preview(source, switching, cx) {
                    if source == TextureSourceKind::ImportedImage
                        && self.import_candidate_model().is_some()
                    {
                        self.fail_image_import(e.to_string(), window, cx);
                    }
                    self.pipeline_error = Some(e.to_string());
                }
                cx.notify();
                return;
            }
            Ok(Some(preview)) => {
                if switching {
                    let target = (source == TextureSourceKind::SolidColor)
                        .then(|| self.node.color_property());
                    if let Err(error) = self.binding.set_property(target.as_ref()) {
                        let previous = (self.source == TextureSourceKind::SolidColor)
                            .then(|| self.node.color_property());
                        if let Err(rollback) = self.binding.set_property(previous.as_ref()) {
                            self.runtime.retain_failure(rollback);
                        }
                        if source == TextureSourceKind::ImportedImage
                            && self.import_candidate_model().is_some()
                        {
                            self.fail_image_import(error.to_string(), window, cx);
                        }
                        self.pipeline_error = Some(error.to_string());
                        self.generator_preview.busy = false;
                        self.generator_preview.switching = false;
                        cx.notify();
                        return;
                    }
                    if source == TextureSourceKind::ImportedImage {
                        self.commit_image_candidate(window, cx);
                    }
                    self.runtime.retire_pending_input();
                    self.preview_export.retire();
                    self.generation.fetch_add(1, Ordering::AcqRel);
                    self.applied_text = None;
                    if source == TextureSourceKind::Polygon
                        || self.source == TextureSourceKind::Polygon
                        || source.scene_kind().is_some()
                        || (source == TextureSourceKind::SolidColor
                            && self.source.scene_kind().is_some())
                    {
                        self.sample_time = self.sample_time(cx);
                    }
                    self.source = source;
                    if source == TextureSourceKind::SolidColor {
                        self.sync_solid_animation_time(cx);
                    }
                    if let Some(scene) = self.active_scene() {
                        scene.update(cx, |view, cx| {
                            view.set_preview_time(self.sample_time, window, cx)
                        });
                    }
                    if let Some(polygon) = self.active_polygon() {
                        polygon.update(cx, |view, cx| {
                            view.set_preview_time(self.sample_time, window, cx)
                        });
                    }
                    self.activate_generators(window, cx);
                    if let Some(generator) = self.active_generator() {
                        window.focus(&generator.read(cx).preferred_focus(cx), cx);
                    } else if let Some(polygon) = self.active_polygon() {
                        window.focus(&polygon.read(cx).preferred_focus(cx), cx);
                    } else if let Some(scene) = self.active_scene() {
                        window.focus(&scene.read(cx).preferred_focus(cx), cx);
                    } else {
                        self.sync_unchanged_drafts(false, window, cx);
                        window.focus(&self.fields[0].read(cx).focus_handle(cx), cx);
                    }
                    window.set_window_title(&format!("Mycela · {} workspace", source.name()));
                } else if let Some(generator) = self.active_generator() {
                    generator.update(cx, |v, cx| v.sync(window, cx));
                } else if let Some(polygon) = self.active_polygon() {
                    polygon.update(cx, |v, cx| v.sync(window, cx));
                } else if let Some(scene) = self.active_scene() {
                    scene.update(cx, |v, cx| v.sync(window, cx));
                }
                self.sync_filter_time(window, cx);
                self.install_preview(preview, cx);
            }
            Err(e) => {
                if source == TextureSourceKind::ImportedImage
                    && self.import_candidate_model().is_some()
                {
                    self.fail_image_import(e.to_string(), window, cx);
                }
                self.pipeline_error = Some(e.to_string());
            }
        }
        self.generator_preview.busy = false;
        self.generator_preview.switching = false;
        cx.notify();
    }
    pub(super) fn activate_generators(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(gradient) = &self.gradient {
            gradient.update(cx, |view, cx| {
                view.set_active(self.source == TextureSourceKind::Gradient);
                view.sync(window, cx);
            });
        }
        for (kind, generator) in &self.procedural {
            generator.update(cx, |view, cx| {
                view.set_active(self.source == *kind);
                view.sync(window, cx);
            });
        }
        for (kind, scene) in &self.scenes {
            scene.update(cx, |view, cx| {
                view.set_active(self.source == *kind);
                view.sync(window, cx);
            });
        }
        if let Some(polygon) = &self.polygon {
            polygon.update(cx, |view, cx| {
                view.set_active(self.source == TextureSourceKind::Polygon);
                view.sync(window, cx);
            });
        }
        self.scene_camera.update(cx, |view, cx| {
            view.set_active(self.source.scene_kind().is_some());
            view.sync(window, cx);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::test::TestWindowExt;
    #[gpui_kit::test]
    async fn obsolete_source_job_and_failed_candidate_preserve_current_authority(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let (handle, root) = cx.update(crate::open_main_window).unwrap();
        let editor = cx.update(|cx| root.read(cx).color_view());
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| {
                view.switch_source(TextureSourceKind::Noise, window, cx);
                assert!(view.generator_preview.is_switching());
                // A queued old-source/timer/effect presentation keeps the switch.
                view.refresh_preview(cx).unwrap();
                assert!(view.generator_preview.is_switching());
                view.switch_source(TextureSourceKind::Gradient, window, cx);
            })
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(editor.read(cx).active_source(), TextureSourceKind::Gradient));
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| {
                view.prepare_generator(TextureSourceKind::Cells, window, cx)
                    .unwrap();
            })
        })
        .unwrap();
        let bad = cx.update(|cx| {
            editor
                .read(cx)
                .generator_source(TextureSourceKind::Cells)
                .unwrap()
                .read(cx)
                .model()
        });
        bad.parameter(mycela_engine::GradientGeneratorParameterKind::Mode)
            .set_index(99)
            .unwrap();
        let before = cx.update(|cx| editor.read(cx).applied_image.clone().unwrap());
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| {
                view.switch_source(TextureSourceKind::Cells, window, cx)
            })
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            let view = editor.read(cx);
            assert_eq!(view.source, TextureSourceKind::Gradient);
            assert!(view.applied_image.as_ref().unwrap().same_reference(&before));
            assert!(view.pipeline_error.is_some());
        });
        // Keep the window active during the whole asynchronous fixture.
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .unwrap();
    }
    #[gpui_kit::test]
    async fn clicking_current_source_does_not_cancel_its_latest_refresh(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let (handle, root) = cx.update(crate::open_main_window).unwrap();
        let editor = cx.update(|cx| root.read(cx).color_view());
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| {
                view.switch_source(TextureSourceKind::Noise, window, cx)
            })
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| {
                view.request_generator_preview(TextureSourceKind::Noise, false, cx)
                    .unwrap();
                let request = view.generator_preview.serial.load(Ordering::Acquire);
                view.switch_source(TextureSourceKind::Noise, window, cx);
                assert!(view.generator_preview.is_busy());
                assert_eq!(
                    view.generator_preview.serial.load(Ordering::Acquire),
                    request
                );
            })
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            let view = editor.read(cx);
            assert!(!view.generator_preview.is_busy());
            assert_eq!(view.source, TextureSourceKind::Noise);
        });
    }
    #[gpui_kit::test]
    async fn candidate_external_edit_restarts_owned_frame_before_source_commit(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let (handle, root) = cx.update(crate::open_main_window).unwrap();
        let editor = cx.update(|cx| root.read(cx).color_view());
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| {
                view.switch_source(TextureSourceKind::Noise, window, cx)
            })
        })
        .unwrap();
        let model = cx.update(|cx| {
            editor
                .read(cx)
                .generator_source(TextureSourceKind::Noise)
                .unwrap()
                .read(cx)
                .model()
        });
        // The candidate's UI is not activated yet, so this emits no active
        // presentation. Completion must detect the actual effective change.
        model
            .parameter(mycela_engine::GradientGeneratorParameterKind::Seed)
            .set_number(17.)
            .unwrap();
        cx.run_until_parked();
        let expected = model
            .capture_render(0.)
            .unwrap()
            .render_with_preflight::<ForegroundError>(
                LIVE_PREVIEW_SIZE,
                LIVE_PREVIEW_SIZE,
                crate::export_render::generator_execution(),
                &mut crate::memory_capacity::noise_preflight,
            )
            .unwrap();
        let expected =
            map_preview::project_bitmap(&expected, &PreviewSelection::BaseColor).unwrap();
        cx.update(|cx| {
            let view = editor.read(cx);
            assert_eq!(view.source, TextureSourceKind::Noise);
            assert!(!view.generator_preview.is_busy());
            for (x, y) in [
                (0, 0),
                (17, 29),
                (LIVE_PREVIEW_SIZE - 1, LIVE_PREVIEW_SIZE - 1),
            ] {
                assert_eq!(
                    view.bitmap().unwrap().argb_pixel(x, y).unwrap(),
                    expected.argb_pixel(x, y).unwrap()
                );
            }
        });
    }
    #[gpui_kit::test]
    async fn silent_matrix_cell_edit_requeues_whole_scene_and_obsolete_scene_cannot_publish(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let (handle, root) = cx.update(crate::open_main_window).unwrap();
        let editor = cx.update(|cx| root.read(cx).color_view());
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| {
                view.switch_source(TextureSourceKind::Gradient, window, cx)
            });
            window.render_frame(cx);
            window.click("filters-tab", cx);
            window.render_frame(cx);
        })
        .unwrap();
        crate::filter_catalog_test::add(cx, handle, mycela_engine::PointwiseKind::Matrix);
        cx.run_until_parked();
        let (matrix, source, before) = cx.update(|cx| {
            let view = editor.read(cx);
            (
                view.filters.read(cx).filters()[0].clone(),
                view.active_generator().unwrap().read(cx).model(),
                view.bitmap().unwrap().rows().unwrap(),
            )
        });
        let object = matrix.parameter(0).unwrap().matrix().unwrap().unwrap();
        let coefficients = object.payload().coefficients().unwrap();
        let input = source
            .render_unbound(LIVE_PREVIEW_SIZE, LIVE_PREVIEW_SIZE, 0.)
            .unwrap();
        assert!(
            input.channel_observation().flags().unwrap()[0],
            "Matrix changes dense input; scalar channels are source-correct copies"
        );
        let original = input.channel(0, false).unwrap().values();
        assert_eq!(
            original.len(),
            (LIVE_PREVIEW_SIZE * LIVE_PREVIEW_SIZE) as usize
        );
        // Actual aip dense center-only convolution with one iteration and a
        // binary half coefficient is the independent f32 half of each input.
        let half: Vec<f32> = original.iter().map(|value| *value * 0.5f32).collect();
        assert_ne!(original, half);
        cx.update(|cx| {
            editor.update(cx, |view, cx| {
                view.refresh_preview(cx).unwrap();
                assert!(view.generator_preview.is_busy());
            })
        });
        // No ordinary/speed notification or foreground refresh accompanies this
        // real public array edit. Publication must compare effective scene data.
        coefficients.write(0, 0.5).unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            let view = editor.read(cx);
            assert!(!view.generator_preview.is_busy());
            assert!(view.pipeline_error.is_none(), "{:?}", view.pipeline_error);
            let image = view.applied_image.as_ref().unwrap();
            assert!(image.channel_observation().flags().unwrap()[0]);
            assert_eq!(image.channel(0, false).unwrap().values(), half);
            assert_ne!(view.bitmap().unwrap().rows().unwrap(), before);
        });
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| {
                view.request_generator_preview(TextureSourceKind::Gradient, false, cx)
                    .unwrap();
                assert!(view.generator_preview.is_busy());
            });
            window.remove_window();
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert!(editor.read(cx).applied_image.is_none()));
        assert_eq!(
            coefficients.read(0).unwrap(),
            0.5,
            "outward real array handle remains valid after window retirement"
        );
    }

    #[gpui_kit::test]
    async fn queued_sample_time_obeys_actual_selected_child_retirement_and_valid_facade(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let (handle, root) = cx.update(crate::open_main_window).unwrap();
        cx.simulate_window_resize(
            handle,
            gpui_kit::size(gpui_kit::px(1000.), gpui_kit::px(2400.)),
        );
        let editor = cx.update(|cx| root.read(cx).color_view());
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("filters-tab", cx);
            window.render_frame(cx);
        })
        .unwrap();
        crate::filter_catalog_test::add(cx, handle, mycela_engine::PointwiseKind::Pixelate);
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
        })
        .unwrap();
        crate::filter_catalog_test::add(cx, handle, mycela_engine::PointwiseKind::Matrix);
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("select-filter-1", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let forwarded = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = Arc::clone(&forwarded);
        let _observer = cx.update(|cx| {
            let filters = editor.read(cx).filter_stack();
            editor.update(cx, |_, cx| {
                cx.subscribe(
                    &filters,
                    move |_, _, event: &crate::spatial_filter::SampleTime, _| {
                        assert!(
                            event
                                .activation
                                .upgrade()
                                .is_some_and(|active| active.load(Ordering::Acquire))
                        );
                        observed.fetch_add(1, Ordering::AcqRel);
                    },
                )
            })
        });
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("filter-1-sample-time", cx);
            window.press("secondary-a", cx);
            window.input("0.5", cx);
            window.click("apply-filter-1-time", cx);
        })
        .unwrap();
        assert_eq!(
            forwarded.load(Ordering::Acquire),
            1,
            "the actual child event reached the parent and queued its deferred time task"
        );
        cx.update(|cx| {
            assert_eq!(
                editor.read(cx).sample_time(cx),
                0.,
                "the queued task has not run"
            )
        });
        // A separate actual user command retires this selected child before
        // that task runs. Stable source/epoch alone would incorrectly accept it.
        cx.update_window(handle, |_, window, cx| window.click("remove-filter", cx))
            .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            let view = editor.read(cx);
            assert_eq!(view.sample_time(cx), 0.);
            assert_eq!(view.filter_stack().read(cx).identities(), vec![2]);
        });
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("filter-2-sample-time", cx);
            window.press("secondary-a", cx);
            window.input("0.25", cx);
            window.click("apply-filter-2-time", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(editor.read(cx).sample_time(cx), 0.25));
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
        })
        .unwrap();
        crate::filter_catalog_test::add(cx, handle, mycela_engine::PointwiseKind::Pixelate);
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("filter-3-sample-time", cx);
            window.press("secondary-a", cx);
            window.input("0.75", cx);
            window.click("apply-filter-3-time", cx);
        })
        .unwrap();
        assert_eq!(forwarded.load(Ordering::Acquire), 3);
        cx.update_window(handle, |_, window, cx| window.click("select-filter-2", cx))
            .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            let view = editor.read(cx);
            assert_eq!(view.sample_time(cx), 0.25);
            assert_eq!(view.filter_stack().read(cx).selected_identity(), Some(2));
            assert_eq!(
                view.filter_stack().read(cx).filters()[1]
                    .parameter(0)
                    .unwrap()
                    .pair()
                    .unwrap(),
                [10., 10.]
            );
        });
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("filter-2-sample-time", cx);
            window.press("secondary-a", cx);
            window.input("0.5", cx);
            window.click("apply-filter-2-time", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(editor.read(cx).sample_time(cx), 0.5));
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| {
                view.switch_source(TextureSourceKind::Gradient, window, cx)
            })
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(editor.read(cx).sample_time(cx), 0.));
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| {
                view.switch_source(TextureSourceKind::SolidColor, window, cx)
            })
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            assert_eq!(
                editor.read(cx).sample_time(cx),
                0.5,
                "the valid global facade preserves each source's remembered time"
            )
        });
    }
}

#[cfg(test)]
mod owned_import_laws {
    use super::*;
    fn bmp() -> Vec<u8> {
        let mut bytes = vec![0; 62];
        bytes[..2].copy_from_slice(b"BM");
        bytes[2..6].copy_from_slice(&62u32.to_le_bytes());
        bytes[10..14].copy_from_slice(&54u32.to_le_bytes());
        bytes[14..18].copy_from_slice(&40u32.to_le_bytes());
        bytes[18..22].copy_from_slice(&2i32.to_le_bytes());
        bytes[22..26].copy_from_slice(&1i32.to_le_bytes());
        bytes[26..28].copy_from_slice(&1u16.to_le_bytes());
        bytes[28..30].copy_from_slice(&24u16.to_le_bytes());
        bytes[54..60].copy_from_slice(&[30, 20, 10, 60, 50, 40]);
        bytes
    }
    #[gpui_kit::test]
    async fn silent_imported_warm_cell_edit_revalidates_before_publish_and_close_keeps_caller_roots(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let (handle, root) = cx.update(crate::open_main_window).unwrap();
        let editor = cx.update(|cx| root.read(cx).color_view());
        let (model, bytes, cells) = cx
            .update_window(handle, |_, window, cx| {
                editor.update(cx, |view, cx| {
                    view.prepare_generator(TextureSourceKind::ImportedImage, window, cx)
                        .unwrap();
                    let model = view
                        .generator_view(TextureSourceKind::ImportedImage)
                        .unwrap()
                        .read(cx)
                        .model();
                    let payload = model
                        .parameter(mycela_engine::ImageGeneratorParameterKind::Image)
                        .custom_value()
                        .unwrap()
                        .unwrap()
                        .imported_image()
                        .unwrap();
                    let bytes = mycela_engine::ByteBuffer::new(bmp());
                    payload.set_bytes(Some(bytes.clone())).unwrap();
                    let cells = payload.image().unwrap().unwrap().channel(0, true).unwrap();
                    view.request_generator_preview(TextureSourceKind::ImportedImage, true, cx)
                        .unwrap();
                    (model, bytes, cells)
                })
            })
            .unwrap();
        // No property event accompanies either public backing-array edit.
        cells.write(0, 0.875).unwrap();
        bytes.write(54, 199).unwrap();
        cx.run_until_parked();
        let expected = model
            .capture_render(0.)
            .unwrap()
            .render_with_preflight::<ForegroundError>(
                LIVE_PREVIEW_SIZE,
                LIVE_PREVIEW_SIZE,
                crate::export_render::generator_execution(),
                &mut crate::memory_capacity::noise_preflight,
            )
            .unwrap();
        let expected =
            map_preview::project_bitmap(&expected, &PreviewSelection::BaseColor).unwrap();
        cx.update(|cx| {
            let view = editor.read(cx);
            assert_eq!(view.source, TextureSourceKind::ImportedImage);
            assert!(!view.generator_preview.is_busy());
            assert!(view.pipeline_error.is_none(), "{:?}", view.pipeline_error);
            assert_eq!(
                view.bitmap().unwrap().rows().unwrap(),
                expected.rows().unwrap()
            );
        });
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| {
                view.request_generator_preview(TextureSourceKind::ImportedImage, false, cx)
                    .unwrap()
            });
            window.remove_window();
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert!(editor.read(cx).applied_image.is_none()));
        bytes.write(54, 17).unwrap();
        cells.write(0, 0.625).unwrap();
        assert_eq!(bytes.read(54).unwrap(), 17);
        assert_eq!(cells.read(0).unwrap(), 0.625);
    }

    #[gpui_kit::test]
    #[ignore = "requires a real compatible wgpu device; run explicitly on the host"]
    async fn scene_time_changes_during_owned_switch_revalidate_the_actual_candidate(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::test::TestWindowExt;
        cx.update(gpui_kit::init);
        let (handle, root) = cx.update(crate::open_main_window).unwrap();
        cx.simulate_window_resize(
            handle,
            gpui_kit::size(gpui_kit::px(1200.), gpui_kit::px(2200.)),
        );
        let editor = cx.update(|cx| root.read(cx).color_view());
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| {
                view.set_sample_time(0.25, window, cx);
                view.switch_source(TextureSourceKind::Cone, window, cx);
            });
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            let view = editor.read(cx);
            assert_eq!(
                view.active_source(),
                TextureSourceKind::Cone,
                "the real first render must succeed"
            );
            assert_eq!(view.sample_time(cx), 0.25);
            assert!(view.bitmap().is_some());
        });
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("toggle-scene-animation", cx);
            window.render_frame(cx);
            window.click("scene-preview-time", cx);
            window.press("secondary-a", cx);
            window.input("0.5", cx);
            window.click("set-scene-time", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(editor.read(cx).sample_time(cx), 0.5));
        // Give the real Cube candidate a time-varying Size X before its
        // worker snapshot, so stale time changes alter its rendered pixels.
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| {
                view.prepare_generator(TextureSourceKind::Cube, window, cx)
                    .unwrap();
                let node = view
                    .scenes
                    .get(&TextureSourceKind::Cube)
                    .unwrap()
                    .read(cx)
                    .node();
                let property = node.scene_primitive().unwrap().property(0).unwrap();
                let animation = property.animation().unwrap().unwrap();
                animation.ensure_curve(0).unwrap();
                let curve = animation.curves().unwrap().value(0).unwrap().unwrap();
                curve
                    .insert(
                        0,
                        Some(&mycela_engine::CurveLayerHandle::new(
                            mycela_engine::AnalyticCurveKind::Linear,
                        )),
                    )
                    .unwrap();
                curve.set_minimum(0.4);
                curve.set_maximum(2.0);
                animation.flags().unwrap().set(0, true).unwrap();
            });
        })
        .unwrap();
        let before = cx.update(|cx| {
            let view = editor.read(cx);
            view.freeze_preview_scene(TextureSourceKind::Cube, true, cx)
                .unwrap()
                .frozen_values()
        });
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| {
                view.switch_source(TextureSourceKind::Cube, window, cx);
                assert!(view.generator_preview.is_switching());
                // This is the actual workspace time entry used by filter
                // controls, invoked while the owned Cube job is in flight.
                view.set_sample_time(0.75, window, cx);
            });
        })
        .unwrap();
        cx.run_until_parked();
        let (current, installed) = cx.update(|cx| {
            let view = editor.read(cx);
            assert_eq!(
                view.active_source(),
                TextureSourceKind::Cube,
                "the actual device job must complete"
            );
            assert_eq!(view.sample_time(cx), 0.75);
            assert_eq!(view.active_scene().unwrap().read(cx).preview_time(), 0.75);
            let current = view
                .freeze_preview_scene(TextureSourceKind::Cube, false, cx)
                .unwrap();
            assert!(!before.same_values(&current.frozen_values()));
            (current, view.bitmap().unwrap())
        });
        let expected = current.materialize().unwrap().render(|| false).unwrap();
        assert_eq!(
            installed.rows().unwrap(),
            expected.rows().unwrap(),
            "the installed scene must match the actual committed time, not the stale candidate"
        );

        // Add a real spatial filter through the UI, then switch to Solid Color.
        // The shared time must remain available to the filter after this owned
        // source switch, including its visible clean input draft.
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("filters-tab", cx);
            window.render_frame(cx);
        })
        .unwrap();
        crate::filter_catalog_test::add(cx, handle, mycela_engine::PointwiseKind::Pixelate);
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("filter-1-sample-time").value(), Some("0.75"));
            editor.update(cx, |view, cx| {
                view.switch_source(TextureSourceKind::SolidColor, window, cx)
            });
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            let view = editor.read(cx);
            assert_eq!(view.active_source(), TextureSourceKind::SolidColor);
            assert_eq!(view.sample_time(cx), 0.75);
        });
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("filter-1-sample-time").value(), Some("0.75"));
        })
        .unwrap();
    }
}
