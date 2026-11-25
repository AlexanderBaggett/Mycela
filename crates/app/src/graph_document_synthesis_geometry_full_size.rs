//! Detached full-size editing over the shared Synthesis source guide.
use super::*;
use gpui_kit::EventEmitter;
use mycela_engine::SynthesisDraftPreviewTarget;

#[derive(Clone, Copy)]
enum SourceKind {
    Blend,
    Patches,
    Repeating,
}

pub(super) fn button(target: &Target, disabled: bool, cx: &Context<GraphDocumentView>) -> Button {
    let target = target.clone();
    Button::new("synthesis-full-size")
        .small()
        .label("Edit full size…")
        .tab_index(0)
        .disabled(disabled)
        .on_click(cx.listener(move |view, _, window, cx| {
            view.open_full_size_synthesis_source(&target, window, cx);
        }))
}

struct FullSizeSource {
    owner: WeakEntity<GraphDocumentView>,
    target: Target,
    kind: SourceKind,
    layer: usize,
    edit: SynthesisEdit,
    geometry: Option<SynthesisGeometry>,
    drag: Option<Drag>,
    hover: Option<GuideHover>,
    grid_selection: GridSelection,
    patch_list: PatchList,
    patch_transform_mode: bool,
    patch_numbers: Option<TransformNumbers>,
    source_focus: FocusHandle,
    navigation: SourceNavigation,
    fields: Fields,
    field_observers: Vec<Subscription>,
    changed: bool,
    error: Option<String>,
    source: Option<source::State>,
    texture_preview: SourceTexturePreview,
    _texture_preview_observer: Subscription,
}

pub(super) struct SourceTexturePreview {
    scope: String,
    raster: Option<std::sync::Arc<gpui_kit::RenderImage>>,
    pending: bool,
    error: Option<String>,
}

impl SourceTexturePreview {
    pub(super) fn read(view: &GraphDocumentView) -> Self {
        Self {
            scope: view.displayed_preview_scope().to_owned(),
            raster: view.visible_preview().map(|preview| preview.raster.clone()),
            pending: view.draft_preview_pending(),
            error: view.preview_error().map(str::to_owned),
        }
    }

    pub(super) fn changed(&self, view: &GraphDocumentView) -> bool {
        self.scope != view.displayed_preview_scope()
            || self.pending != view.draft_preview_pending()
            || self.error.as_deref() != view.preview_error()
            || match (&self.raster, view.visible_preview()) {
                (Some(old), Some(new)) => !std::sync::Arc::ptr_eq(old, &new.raster),
                (None, None) => false,
                _ => true,
            }
    }
    pub(super) fn render(
        &self,
        owner: &WeakEntity<GraphDocumentView>,
        cx: &App,
    ) -> impl IntoElement {
        let raster = self.raster.as_ref().and_then(|image| {
            owner
                .upgrade()
                .map(|owner| owner.read(cx).render_preview_raster(image.clone(), 1.))
        });
        let theme = cx.theme();
        div()
            .id("synthesis-full-texture-preview")
            .test_support()
            .role(gpui_kit::Role::Group)
            .aria_label(format!("Texture preview · {}", self.scope))
            .v_flex()
            .w(rems(10.))
            .flex_none()
            .gap_1()
            .child(div().font_semibold().child("Preview"))
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(self.scope.clone()),
            )
            .child(
                div()
                    .relative()
                    .size(rems(10.))
                    .overflow_hidden()
                    .child(crate::preview_background::checkerboard_for(1.))
                    .children(raster),
            )
            .child(
                div()
                    .h(rems(1.))
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .when(self.pending, |status| status.child("Rendering…")),
            )
            .when_some(self.error.clone(), |panel, error| {
                panel.child(div().text_sm().text_color(theme.danger).child(error))
            })
    }
}

struct SourceDraftChanged;

impl EventEmitter<SourceDraftChanged> for FullSizeSource {}

impl FullSizeSource {
    fn toggle_patch_tool(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.drag.is_some() || !matches!(self.geometry, Some(Patch { .. })) {
            return;
        }
        let Some(bounds) = patch_values(&self.edit, self.layer, self.geometry, None) else {
            return;
        };
        self.patch_transform_mode = !self.patch_transform_mode;
        self.patch_numbers = self.patch_transform_mode.then(|| {
            TransformNumbers::new(
                bounds,
                |dialog: &mut Self, field, value, window, cx| {
                    dialog.commit_patch_number(field, value, window, cx);
                },
                window,
                cx,
            )
        });
        self.hover = None;
        cx.notify();
    }

    fn commit_patch_number(
        &mut self,
        field: SynthesisPatchTransformField,
        value: f64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(selection) = PatchList::selected(self.geometry) else {
            return;
        };
        let Some(fitted) = self.navigation.fitted.get() else {
            return;
        };
        self.command(
            GeometryCommand::SetPatchTransformField(selection, field, value, fitted),
            window,
            cx,
        );
    }

    fn sync_patch_numbers(&self, window: &mut Window, cx: &mut App) {
        if let Some(numbers) = &self.patch_numbers
            && let Some(bounds) = patch_values(&self.edit, self.layer, self.geometry, self.drag)
        {
            numbers.sync(bounds, window, cx);
        }
    }

    fn refresh_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.fields = self
            .geometry
            .map_or_else(Vec::new, |geometry| {
                self.edit.geometry_fields(self.layer, geometry)
            })
            .into_iter()
            .map(|field| {
                let [min, max] = field.range();
                let input = cx.new(|cx| {
                    InputState::new(window, cx)
                        .default_value(field_text(field))
                        .min(min)
                        .max(max)
                        .step(if field.is_whole() { 1. } else { 0.01 })
                });
                (field, input)
            })
            .collect();
        self.field_observers = self
            .fields
            .iter()
            .map(|(_, input)| {
                cx.subscribe_in(
                    input,
                    window,
                    |dialog, _, event: &InputEvent, window, cx| {
                        match event {
                            InputEvent::Change => dialog.error = dialog.apply_live_fields(cx).err(),
                            InputEvent::Blur | InputEvent::PressEnter { .. }
                                if matches!(dialog.kind, SourceKind::Blend) =>
                            {
                                dialog.error = dialog.apply_live_fields(cx).err();
                                if dialog.error.is_none() {
                                    dialog.sync_crop_fields(window, cx);
                                }
                            }
                            _ => return,
                        }
                        cx.notify();
                    },
                )
            })
            .collect();
    }

    fn sync_crop_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let fields = self.edit.geometry_fields(self.layer, Crop);
        for ((field, input), effective) in self.fields.iter_mut().zip(fields) {
            *field = effective;
            let text = field_text(effective);
            if input.read(cx).value().as_ref() != text {
                input.update(cx, |input, cx| input.set_value(text, window, cx));
            }
        }
    }

    fn apply_live_fields(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        if self.apply_fields(cx)? {
            cx.emit(SourceDraftChanged);
        }
        Ok(())
    }

    fn apply_fields(&mut self, cx: &App) -> Result<bool, String> {
        let Some(geometry) = self.geometry else {
            return Ok(false);
        };
        let before = self.edit.geometry_fields(self.layer, geometry);
        let crop_before = self.edit.crop_rectangle(self.layer);
        let grid_before = self
            .edit
            .repeat_grid(self.layer)
            .map(|(dimensions, _)| dimensions);
        if crop_before.is_none()
            && !self
                .fields
                .iter()
                .zip(&before)
                .any(|((field, input), current)| {
                    field_value(*field, input, cx) != Some(current.value())
                })
        {
            return Ok(false);
        }
        let input_geometry = match &self.source {
            Some(source::State::Ready(raster)) => Some(raster.geometry),
            _ => None,
        };
        apply_fields(&self.fields, cx, |values| {
            self.edit.set_geometry_fields_with_input_geometry(
                self.layer,
                geometry,
                values,
                input_geometry,
            )
        })?;
        if grid_before
            != self
                .edit
                .repeat_grid(self.layer)
                .map(|(dimensions, _)| dimensions)
        {
            self.grid_selection.select(self.geometry);
        }
        let changed = if crop_before.is_some() {
            crop_before != self.edit.crop_rectangle(self.layer)
        } else {
            let after = self.edit.geometry_fields(self.layer, geometry);
            !before
                .iter()
                .map(|field| field.value().to_bits())
                .eq(after.iter().map(|field| field.value().to_bits()))
        };
        self.changed |= changed;
        Ok(changed)
    }

    fn command(&mut self, command: GeometryCommand, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(error) = self.apply_live_fields(cx) {
            self.error = Some(error);
            cx.notify();
            return;
        }
        if matches!(command, GeometryCommand::TogglePatchTool) {
            self.toggle_patch_tool(window, cx);
            return;
        }
        let source_geometry = match &self.source {
            Some(source::State::Ready(raster)) => Some(raster.geometry),
            _ => None,
        };
        let Some((label, selection)) = edit_geometry(
            &mut self.edit,
            self.layer,
            self.geometry,
            command,
            &self.grid_selection,
            source_geometry,
        ) else {
            if matches!(command, GeometryCommand::CreatePatch(_)) {
                self.error = Some("Couldn’t fit the patch inside this source.".into());
                cx.notify();
            }
            return;
        };
        if label.is_some() {
            self.changed = true;
            cx.emit(SourceDraftChanged);
        }
        let patch = PatchList::selected(selection);
        let list_changes = matches!(
            command,
            GeometryCommand::AddPatch
                | GeometryCommand::CreatePatch(_)
                | GeometryCommand::RemovePatch(_)
        );
        if list_changes || PatchList::selected(self.geometry) != patch {
            self.patch_list.reveal(patch);
        }
        if list_changes {
            self.patch_list.focus(window, cx);
        }
        self.geometry = selection;
        if matches!(command, GeometryCommand::Select(_)) {
            self.grid_selection.select(selection);
        }
        self.hover = None;
        self.error = None;
        self.refresh_fields(window, cx);
        self.sync_patch_numbers(window, cx);
        cx.notify();
    }

    fn guide_event(&mut self, event: GuideEvent, window: &mut Window, cx: &mut Context<Self>) {
        if let GuideEvent::Navigate(event) = event {
            self.navigate(event, cx);
            return;
        }
        if self.navigation.pan.is_some() {
            return;
        }
        let points = self
            .edit
            .repeat_grid(self.layer)
            .map_or(&[][..], |(_, points)| points);
        let transforming = self
            .drag
            .is_some_and(|drag| matches!(drag.handle, Handle::PatchTransform { .. }));
        match guide_update(
            &mut self.drag,
            &mut self.hover,
            &mut self.grid_selection,
            points,
            event,
        ) {
            GuideUpdate::None => {}
            GuideUpdate::Repaint => {
                if transforming
                    || self
                        .drag
                        .is_some_and(|drag| matches!(drag.handle, Handle::PatchTransform { .. }))
                {
                    self.sync_patch_numbers(window, cx);
                }
                cx.notify();
            }
            GuideUpdate::Edit(command) => self.command(command, window, cx),
        }
    }

    fn navigate(&mut self, event: SourceViewEvent, cx: &mut Context<Self>) {
        let editing = self.drag.is_some() || self.grid_selection.gesture.is_some();
        if self.navigation.update(event, editing) {
            self.hover = None;
            cx.notify();
        }
    }

    fn source(&self) -> (Option<source::Raster>, Option<String>) {
        match &self.source {
            Some(source::State::Ready(raster)) => (Some(raster.clone()), None),
            Some(source::State::Loading(_)) => (None, Some("Loading source image…".into())),
            Some(source::State::Failed(error)) => {
                (None, Some(format!("Couldn’t show source image. {error}")))
            }
            None => (
                None,
                Some("Connect an input or choose a source image.".into()),
            ),
        }
    }

    fn controls(&self, cx: &Context<Self>) -> impl IntoElement {
        let action = |id, label, command, disabled| {
            Button::new(id)
                .small()
                .label(label)
                .disabled(disabled)
                .tab_index(0)
                .on_click(
                    cx.listener(move |dialog, _, window, cx| dialog.command(command, window, cx)),
                )
        };
        let select = |next| GeometryCommand::Select(Some(next));
        let row = div().h_flex().items_center().gap_2().flex_wrap();
        match self.kind {
            SourceKind::Blend => row,
            SourceKind::Repeating => {
                let point = match self.geometry {
                    Some(GridPoint(index)) => Some(index),
                    _ => None,
                };
                let count = self
                    .edit
                    .repeat_grid(self.layer)
                    .map_or(0, |(_, points)| points.len());
                row.child(action(
                    "synthesis-full-grid-size",
                    "Grid size",
                    select(Grid),
                    point.is_none(),
                ))
                .child(action(
                    "synthesis-full-grid-previous",
                    "Previous point",
                    select(GridPoint(point.unwrap_or(0).saturating_sub(1))),
                    point == Some(0) || count == 0,
                ))
                .child(action(
                    "synthesis-full-grid-next",
                    "Next point",
                    select(GridPoint(point.map_or(0, |index| index + 1))),
                    count == 0 || point == count.checked_sub(1),
                ))
            }
            SourceKind::Patches => {
                let mut row = row;
                if let Some(Patch { selection, knot }) = self.geometry {
                    let points = self
                        .edit
                        .patch_knots(self.layer, selection)
                        .map_or(0, |points| points.len());
                    let count = self
                        .edit
                        .patch_counts(self.layer)
                        .map_or(0, |(count, _)| count);
                    row = row
                        .child(
                            div()
                                .text_sm()
                                .child(format!("Patch {} of {count}", selection + 1)),
                        )
                        .child(action(
                            "synthesis-full-prev-patch",
                            "Previous patch",
                            select(Patch {
                                selection: selection.saturating_sub(1),
                                knot: 0,
                            }),
                            selection == 0,
                        ))
                        .child(action(
                            "synthesis-full-next-patch",
                            "Next patch",
                            select(Patch {
                                selection: selection + 1,
                                knot: 0,
                            }),
                            selection + 1 >= count,
                        ))
                        .child(action(
                            "synthesis-full-remove-patch",
                            "Remove patch",
                            GeometryCommand::RemovePatch(selection),
                            false,
                        ))
                        .when(!self.patch_transform_mode, |row| {
                            row.child(action(
                                "synthesis-full-prev-knot",
                                "Previous point",
                                select(Patch {
                                    selection,
                                    knot: knot.saturating_sub(1),
                                }),
                                knot == 0,
                            ))
                            .child(action(
                                "synthesis-full-next-knot",
                                "Next point",
                                select(Patch {
                                    selection,
                                    knot: knot + 1,
                                }),
                                knot + 1 >= points,
                            ))
                            .child(action(
                                "synthesis-full-remove-knot",
                                "Remove point",
                                GeometryCommand::RemoveKnot(selection, knot),
                                points <= 3,
                            ))
                        });
                }
                row
            }
        }
    }
}

impl Render for FullSizeSource {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (raster, source_status) = self.source();
        let loading = matches!(self.source, Some(source::State::Loading(_)));
        let disabled = loading || (matches!(self.kind, SourceKind::Blend) && raster.is_none());
        let rem = window.rem_size();
        let viewport = window.viewport_size();
        let columns = if viewport.width < rem * 40. { 2 } else { 4 };
        let patches = matches!(self.kind, SourceKind::Patches);
        let field_count = if patches {
            self.fields.len().max(12)
        } else {
            self.fields.len()
        };
        let fields_height = (rem * 4. * field_count.div_ceil(columns) as f32)
            .min((viewport.height * 0.30).max(rem * 5.));
        let chrome = if patches { 17. } else { 15. };
        let height = (viewport.height - fields_height - rem * chrome)
            .max(rem * 9.)
            .min(rem * 42.);
        let view_disabled = disabled || (raster.is_none() && self.geometry.is_none());
        let has_raster = raster.is_some();
        let guide = Guide::from_edit(
            &self.edit,
            self.layer,
            self.geometry,
            self.drag,
            &self.grid_selection,
            raster.as_ref().map(|raster| raster.geometry),
            self.patch_transform_mode,
        )
        .unwrap_or(Guide {
            handles: Vec::new(),
            lines: Lines::None,
            label: "Source image",
        });
        let guide = guide.render(
            GuideView {
                selected: self.geometry,
                drag: self.drag,
                hover: self.hover,
                raster,
                full_height: Some(height),
                source_view: (!view_disabled).then_some(self.navigation.view),
                panning: self.navigation.pan.is_some(),
                disabled,
                focus: self.source_focus.clone(),
                fitted_size: self.navigation.fitted.clone(),
            },
            Self::guide_event,
            cx,
        );
        let workspace = if patches {
            let owner = cx.weak_entity();
            div()
                .h_flex()
                .h(height)
                .items_stretch()
                .gap_2()
                .child(div().flex_1().min_w_0().child(guide))
                .child(
                    div()
                        .w(rems(10.5))
                        .flex_none()
                        .child(self.patch_list.render(
                            PatchListView {
                                inline: false,
                                edit: &self.edit,
                                layer: self.layer,
                                selection: self.geometry,
                                disabled,
                            },
                            move |selection, window, cx| {
                                let _ = owner.update(cx, |dialog, cx| {
                                    dialog.command(
                                        GeometryCommand::SelectPatch(selection),
                                        window,
                                        cx,
                                    );
                                });
                            },
                            cx,
                        )),
                )
                .into_any_element()
        } else {
            guide.into_any_element()
        };
        let fields = Form::new().columns(columns).children(
            self.fields
                .iter()
                .enumerate()
                .filter(|(_, (field, _))| {
                    !self.patch_transform_mode || field.number_field().is_some()
                })
                .map(|(index, (field, input))| {
                    let linked = field.number_field().and_then(|number| {
                        let link = self.edit.number_adjustment(Some(self.layer), number)?;
                        let control = link
                            .components()
                            .first()
                            .filter(|component| component.active && !component.name.is_empty())?;
                        let value = self.edit.number_value(Some(self.layer), number)?;
                        Some((number_text(value), String::from_utf16_lossy(&control.name)))
                    });
                    let item = match linked {
                        Some((value, name)) => div()
                            .text_sm()
                            .child(value)
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(name),
                            )
                            .into_any_element(),
                        None => Input::new(input)
                            .id(("synthesis-full-size-field", index))
                            .small()
                            .disabled(disabled)
                            .into_any_element(),
                    };
                    Field::new().label(field.label()).child(item)
                }),
        );
        div()
            .v_flex()
            .gap_2()
            .when(matches!(self.kind, SourceKind::Repeating), |body| {
                body.child(self.controls(cx))
            })
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .flex_wrap()
                    .justify_between()
                    .when(patches, |bar| {
                        let owner = cx.weak_entity();
                        bar.child(patch_creation_controls(
                            false,
                            !self.edit.layer_has_source(self.layer),
                            has_raster,
                            None,
                            move |command, window, cx| {
                                let _ = owner
                                    .update(cx, |dialog, cx| dialog.command(command, window, cx));
                            },
                        ))
                    })
                    .child(self.navigation.controls(
                        false,
                        view_disabled
                            || self.drag.is_some()
                            || self.grid_selection.gesture.is_some(),
                        matches!(self.geometry, Some(Patch { .. })).then(|| {
                            Button::new("synthesis-full-patch-tool")
                                .small()
                                .label(if self.patch_transform_mode {
                                    "Edit points"
                                } else {
                                    "Transform patch"
                                })
                                .tab_index(0)
                                .disabled(!has_raster || self.drag.is_some())
                                .on_click(cx.listener(|dialog, _, window, cx| {
                                    dialog.command(GeometryCommand::TogglePatchTool, window, cx)
                                }))
                        }),
                        Self::navigate,
                        cx,
                    )),
            )
            .child(
                div()
                    .h_flex()
                    .w_full()
                    .items_start()
                    .gap_3()
                    .child(div().flex_1().min_w_0().child(workspace))
                    .when(viewport.width < rem * 96., |workspace| {
                        workspace.child(self.texture_preview.render(&self.owner, cx))
                    }),
            )
            .when(patches && self.geometry.is_some(), |body| {
                body.child(self.controls(cx))
            })
            .children(
                self.patch_numbers
                    .as_ref()
                    .filter(|_| matches!(self.geometry, Some(Patch { .. })))
                    .map(|numbers| {
                        numbers.render(false, disabled || !has_raster || self.drag.is_some())
                    }),
            )
            .when(!self.fields.is_empty(), |body| {
                body.child(
                    div()
                        .id("synthesis-full-size-controls")
                        .max_h(fields_height)
                        .overflow_y_scroll()
                        .child(fields),
                )
            })
            .when_some(self.error.clone().or(source_status), |body, status| {
                body.child(
                    div()
                        .id("synthesis-full-size-status")
                        .role(gpui_kit::Role::Status)
                        .aria_label(status.clone())
                        .test_support()
                        .text_sm()
                        .child(status),
                )
            })
    }
}

impl GraphDocumentView {
    pub(super) fn freeze_source_draft(
        &self,
        target: &Target,
        draft: SynthesisEdit,
    ) -> Result<Option<draft_preview::DraftPreviewJob>, String> {
        if !target.current(self) || target.material_locked() {
            return Ok(None);
        }
        let editing_material = target.editing_material.as_ref();
        let owner = if target.lab_material {
            let Some(origin) = self
                .synthesis
                .as_ref()
                .and_then(|panel| panel.lab_material.as_ref())
            else {
                return Ok(None);
            };
            let Some(material) = editing_material else {
                return Ok(None);
            };
            SynthesisDraftPreviewTarget::LabMaterial {
                selected: origin.selected.as_ref(),
                material,
            }
        } else if let Some(style) = &target.style {
            let Some((owner, canvas, pass)) = style.synthesis_number_receiver() else {
                return Ok(None);
            };
            let Some((expected, _)) = self
                .style
                .as_ref()
                .and_then(style::Panel::synthesis_style_draft)
            else {
                return Ok(None);
            };
            SynthesisDraftPreviewTarget::Style {
                owner,
                canvas,
                expected,
                pass,
                material: editing_material,
            }
        } else {
            SynthesisDraftPreviewTarget::Standalone {
                owner: &target.node,
                material: editing_material,
            }
        };
        let material = target.lab_material.then(|| draft.record());
        let session = guard(&self.document)
            .preview_synthesis_draft(owner, draft)
            .map_err(model::failure)?;
        session
            .map(|session| self.freeze_draft_preview(session, material, true))
            .transpose()
    }

    fn open_full_size_synthesis_source(
        &mut self,
        target: &Target,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_busy()
            || !target.current(self)
            || target.material_locked()
            || self.has_field_drafts(cx)
        {
            return;
        }
        let Some((panel, layer)) = self.synthesis.as_ref().zip(target.layer) else {
            return;
        };
        let kind = match panel.edit.layer(layer).map(|(_, kind)| kind) {
            Some(SynthesisLayerKind::Blend) => SourceKind::Blend,
            Some(SynthesisLayerKind::Patches) => SourceKind::Patches,
            Some(SynthesisLayerKind::Repeating) => SourceKind::Repeating,
            _ => return,
        };
        let edit = panel.edit.clone();
        let geometry = match (kind, panel.geometry) {
            (SourceKind::Blend, _) => Some(Crop),
            (SourceKind::Patches, Some(selection @ Patch { .. })) => Some(selection),
            (SourceKind::Patches, _) => edit
                .patch_counts(layer)
                .filter(|(patches, _)| *patches != 0)
                .map(|_| Patch {
                    selection: 0,
                    knot: 0,
                }),
            (SourceKind::Repeating, Some(selection @ GridPoint(_))) => Some(selection),
            (SourceKind::Repeating, _) => Some(Grid),
        };
        let (source, pending) = match self.prepare_full_size_synthesis_source(panel, layer) {
            source::GuidePreparation::Available(source) => (source, None),
            source::GuidePreparation::Pending(job) => {
                let ticket = Ticket::new();
                (
                    Some(source::State::loading(ticket.clone())),
                    Some((ticket, job)),
                )
            }
        };
        let library = self.environment_library.clone();
        let mut grid_selection = GridSelection::new(geometry);
        if geometry == Some(Grid) {
            grid_selection.copy_points_from(&panel.grid_selection);
        }
        let owner = cx.entity();
        let target = target.clone();
        let patch_transform_mode = panel.patch_transform_mode;
        let texture_preview = SourceTexturePreview::read(self);
        let draft = cx.new(|cx| {
            let patch_list = PatchList::new(cx);
            patch_list.reveal(PatchList::selected(geometry));
            let texture_preview_observer =
                cx.observe(&owner, |editor: &mut FullSizeSource, owner, cx| {
                    let view = owner.read(cx);
                    if editor.texture_preview.changed(view) {
                        editor.texture_preview = SourceTexturePreview::read(view);
                        cx.notify();
                    }
                });
            let mut draft = FullSizeSource {
                owner: owner.downgrade(),
                target: target.clone(),
                kind,
                layer,
                edit,
                geometry,
                drag: None,
                hover: None,
                grid_selection,
                patch_list,
                patch_transform_mode: false,
                patch_numbers: None,
                source_focus: cx.focus_handle(),
                navigation: SourceNavigation::default(),
                fields: Vec::new(),
                field_observers: Vec::new(),
                changed: false,
                error: None,
                source,
                texture_preview,
                _texture_preview_observer: texture_preview_observer,
            };
            draft.refresh_fields(window, cx);
            if patch_transform_mode {
                draft.toggle_patch_tool(window, cx);
            }
            if let Some((ticket, job)) = pending {
                let worker = ticket.clone();
                cx.spawn(async move |dialog, cx| {
                    let raster = cx
                        .background_spawn(async move { job.render(&library, &worker) })
                        .await;
                    if ticket.is_current() {
                        let _ = dialog.update(cx, |draft, cx| {
                            draft.source = Some(match raster {
                                Ok(raster) => source::State::Ready(raster),
                                Err(error) => source::State::Failed(error),
                            });
                            cx.notify();
                        });
                    }
                })
                .detach();
            }
            draft
        });
        let owner = owner.downgrade();
        self.pause_preview();
        self.modal = true;
        let editor_id = draft.entity_id();
        let subscription = cx.subscribe(&draft, move |view, draft, _: &SourceDraftChanged, cx| {
            let draft = draft.read(cx);
            let target = draft.target.clone();
            let edit = draft.edit.clone();
            view.queue_draft_preview(
                editor_id,
                move |view| view.freeze_source_draft(&target, edit),
                cx,
            );
        });
        self.draft_preview = Some(draft_preview::DraftPreview::new(editor_id, subscription));
        window.open_dialog(cx, move |dialog, window, _| {
            let close = owner.clone();
            let apply_draft = draft.clone();
            let apply_owner = owner.clone();
            let apply = move |window: &mut Window, cx: &mut App| {
                let staged = apply_draft.update(cx, |draft, cx| {
                    draft.apply_fields(cx)?;
                    Ok::<_, String>((
                        draft.owner.clone(),
                        draft.target.clone(),
                        draft.edit.clone(),
                        draft.geometry,
                        draft.changed,
                    ))
                });
                let result = staged.and_then(|(owner, target, texture, geometry, changed)| {
                    owner
                        .update(cx, |view, cx| {
                            if !target.current(view) || view.work.is_some() {
                                return Err(
                                    "The Synthesis layer changed. Open its source editor again."
                                        .into(),
                                );
                            }
                            if changed {
                                view.error = None;
                                view.commit_synthesis(
                                    &target,
                                    texture,
                                    (target.layer, geometry),
                                    "Edit Synthesis source guide",
                                    window,
                                    cx,
                                );
                                if let Some(error) = view.error.clone() {
                                    return Err(error);
                                }
                            } else if geometry != target.geometry {
                                view.synthesis_geometry_command(
                                    &target,
                                    GeometryCommand::Select(geometry),
                                    window,
                                    cx,
                                );
                            }
                            Ok(())
                        })
                        .unwrap_or_else(|_| Err("The graph window closed.".into()))
                });
                if let Err(error) = result {
                    apply_draft.update(cx, |draft, cx| {
                        draft.error = Some(error);
                        cx.notify();
                    });
                    return;
                }
                // Restore graph shortcuts as soon as Apply returns.
                window.close_dialog(cx);
                let _ = apply_owner.update(cx, |view, cx| {
                    view.close_draft_preview(editor_id);
                    view.modal = false;
                    view.focus.focus(window, cx);
                    cx.notify();
                });
            };
            let keyboard_apply = apply.clone();
            let editor = draft.clone();
            let cancel_editor = draft.clone();
            let action_width = gpui_kit::rems(5.);
            let width = (window.viewport_size().width - window.rem_size() * 3.)
                .min(window.rem_size() * 66.);
            dialog
                .title(match kind {
                    SourceKind::Blend => "Blend crop",
                    SourceKind::Patches => "Patches source",
                    SourceKind::Repeating => "Repeating pattern source",
                })
                .width(width)
                .overlay_closable(false)
                .child(draft.clone())
                .footer(
                    DialogFooter::new()
                        .child(
                            div().w(action_width).h_8().child(
                                DialogClose::new()
                                    .child(Button::new("cancel").w_full().label("Cancel")),
                            ),
                        )
                        .child(
                            Button::new("ok")
                                .w(action_width)
                                .primary()
                                .label("Apply")
                                .on_click(move |_, window, cx| apply(window, cx)),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    if let Some((field, value)) = editor
                        .read(cx)
                        .patch_numbers
                        .as_ref()
                        .and_then(|numbers| numbers.focused(window, cx))
                    {
                        editor.update(cx, |draft, cx| {
                            if let Some(value) = value {
                                draft.commit_patch_number(field, value, window, cx);
                            } else {
                                draft.sync_patch_numbers(window, cx);
                            }
                        });
                        return false;
                    }
                    let field_has_focus = editor.read(cx).fields.iter().any(|(_, input)| {
                        gpui_kit::Focusable::focus_handle(input.read(cx), cx).is_focused(window)
                    });
                    if field_has_focus {
                        editor.update(cx, |draft, cx| {
                            draft.error = draft.apply_live_fields(cx).err();
                            if draft.error.is_none() && matches!(draft.kind, SourceKind::Blend) {
                                draft.sync_crop_fields(window, cx);
                            }
                            cx.notify();
                        });
                    } else {
                        keyboard_apply(window, cx);
                    }
                    false
                })
                .on_cancel(move |_, window, cx| {
                    cancel_editor.update(cx, |draft, cx| {
                        if draft.drag.is_some_and(|drag| {
                            matches!(drag.handle, Handle::PatchTransform { .. })
                        }) {
                            draft.guide_event(GuideEvent::CancelTransform, window, cx);
                            false
                        } else {
                            true
                        }
                    })
                })
                .on_close(move |_, _, cx| {
                    let _ = close.update(cx, |view, cx| {
                        if view.close_draft_preview(editor_id) {
                            view.modal = false;
                            cx.notify();
                        }
                    });
                })
        });
        cx.notify();
    }
}
