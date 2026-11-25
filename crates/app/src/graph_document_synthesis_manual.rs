//! Manual placement authoring for a Patches layer.
use super::*;
use full_size::SourceTexturePreview;
use gpui_kit::{EventEmitter, Global};
use mycela_engine::{
    SynthesisPlacementClipboard, SynthesisPlacementField, SynthesisPlacementGeometry,
};
use std::hash::{BuildHasher, RandomState};

#[path = "graph_document_synthesis_manual_controls.rs"]
mod controls;
#[path = "graph_document_synthesis_manual_tile.rs"]
mod tile;
#[cfg(test)]
#[path = "graph_document_synthesis_manual_ui_tests.rs"]
mod ui_tests;
use controls::{ManualCommand, ManualCopyList, ManualCopyView, ManualDispatch, ManualNumbers};
use tile::{Tile, TileDrag, TileEvent};

type EditorContext<'a> = Context<'a, Editor>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Inline,
    FullSize,
}

enum EditorEvent {
    Selected,
    Edited(&'static str),
    FullSize,
    Source,
}

struct ManualClipboard(SynthesisPlacementClipboard);
impl Global for ManualClipboard {}

pub(in crate::graph_document::view::synthesis) struct Inline {
    editor: Entity<Editor>,
    _events: Subscription,
}

impl Inline {
    pub(in crate::graph_document::view::synthesis) fn render(&self) -> AnyElement {
        self.editor.clone().into_any_element()
    }

    pub(in crate::graph_document::view::synthesis) fn sync(
        &self,
        edit: &SynthesisEdit,
        layer: usize,
        revision: u64,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<SynthesisGeometry> {
        self.editor.update(cx, |editor, cx| {
            let selected = editor.selection_identity();
            let previous = editor.value();
            if !editor.edit.same_layer_source(editor.layer, edit, layer) {
                editor.raster = None;
                editor.raw = None;
            }
            editor.layer = layer;
            editor.edit = edit.clone();
            editor.revision = revision;
            editor.drag = None;
            editor.rebuild_rows();
            let index = editor.index_of(selected);
            if previous != index.and_then(|index| edit.placement_geometry(layer, index)) {
                editor.raw_centers = [false; 2];
            }
            editor.set_selected(index, window, cx);
            editor.rebuild_tile();
            cx.notify();
            editor.selected.map(Placement)
        })
    }
}

pub(in crate::graph_document::view::synthesis) fn bind_keys(cx: &mut App) {
    ManualCopyList::bind_keys(cx);
}

pub(super) fn open_button(
    target: &Target,
    disabled: bool,
    cx: &Context<GraphDocumentView>,
) -> Button {
    let target = target.clone();
    Button::new("synthesis-edit-placements")
        .small()
        .tab_index(0)
        .label("Manual")
        .disabled(disabled)
        .on_click(cx.listener(move |view, _, window, cx| {
            if !view.is_busy() && target.current(view) && !target.material_locked() {
                view.install_inline_synthesis_manual(None, window, cx);
                view.refresh_inline_synthesis_manual(cx);
            }
        }))
}

struct Editor {
    owner: WeakEntity<GraphDocumentView>,
    mode: Mode,
    edit: SynthesisEdit,
    layer: usize,
    revision: u64,
    selected: Option<usize>,
    source_patch: Option<usize>,
    raw_centers: [bool; 2],
    rows: Vec<(usize, SynthesisPlacementGeometry)>,
    list: ManualCopyList,
    numbers: ManualNumbers,
    focus: FocusHandle,
    tile: Tile,
    navigation: SourceNavigation,
    drag: Option<TileDrag>,
    hover: Option<CursorStyle>,
    raster: Option<source::Raster>,
    raw: Option<ImageGeometry>,
    request: Option<PreviewRequest>,
    ticket: Option<Ticket>,
    error: Option<String>,
    changed: bool,
    blocked: bool,
    output: Option<SourceTexturePreview>,
    _owner: Subscription,
}

impl EventEmitter<EditorEvent> for Editor {}

#[derive(PartialEq, Eq)]
struct PreviewRequest {
    revision: u64,
    time: u64,
    library: Option<PathBuf>,
}

impl Editor {
    fn new(init: EditorInit, window: &mut Window, cx: &mut EditorContext) -> Self {
        let EditorInit {
            owner,
            mode,
            edit,
            layer,
            revision,
            selected,
            source_patch,
            blocked,
            output,
        } = init;
        let selected = selected.filter(|&index| edit.placement_geometry(layer, index).is_some());
        let value = selected.and_then(|index| edit.placement_geometry(layer, index));
        let list = ManualCopyList::new(cx);
        list.reveal(selected);
        let observe = cx.observe(&owner, |editor: &mut Editor, owner, cx| {
            let view = owner.read(cx);
            let blocked = editor.mode == Mode::Inline && view.is_busy();
            let output_changed = editor
                .output
                .as_ref()
                .is_some_and(|output| output.changed(view));
            if output_changed {
                editor.output = Some(SourceTexturePreview::read(view));
            }
            if editor.blocked != blocked || output_changed {
                editor.blocked = blocked;
                cx.notify();
            }
        });
        let mut editor = Self {
            owner: owner.downgrade(),
            mode,
            edit,
            layer,
            revision,
            selected,
            source_patch: value
                .map(SynthesisPlacementGeometry::patch)
                .or(source_patch),
            raw_centers: [false; 2],
            rows: Vec::new(),
            list,
            numbers: ManualNumbers::new(value, Self::apply_number, window, cx),
            focus: cx.focus_handle(),
            tile: Tile::default(),
            navigation: SourceNavigation::default(),
            drag: None,
            hover: None,
            raster: None,
            raw: None,
            request: None,
            ticket: None,
            error: None,
            changed: false,
            blocked,
            output,
            _owner: observe,
        };
        editor.rebuild_rows();
        editor
    }

    fn value(&self) -> Option<SynthesisPlacementGeometry> {
        self.selected
            .and_then(|index| self.edit.placement_geometry(self.layer, index))
    }

    fn selection_identity(&self) -> Option<usize> {
        self.value().map(SynthesisPlacementGeometry::identity)
    }

    fn index_of(&self, identity: Option<usize>) -> Option<usize> {
        let identity = identity?;
        self.rows
            .iter()
            .find(|(_, value)| value.identity() == identity)
            .map(|(index, _)| *index)
    }

    fn rebuild_rows(&mut self) {
        let (patches, copies) = self.edit.patch_counts(self.layer).unwrap_or_default();
        self.rows = (0..copies)
            .filter_map(|index| {
                self.edit
                    .placement_geometry(self.layer, index)
                    .map(|value| (index, value))
            })
            .collect();
        self.source_patch = self
            .source_patch
            .filter(|patch| *patch < patches)
            .or((patches > 0).then_some(0));
    }

    fn rebuild_tile(&mut self) {
        self.tile = Tile::new(&self.edit, self.layer, self.raw, &self.rows);
    }

    fn set_selected(&mut self, index: Option<usize>, window: &mut Window, cx: &mut EditorContext) {
        if self.selected != index {
            self.raw_centers = [false; 2];
        }
        let value = index.and_then(|index| self.edit.placement_geometry(self.layer, index));
        self.selected = index.filter(|_| value.is_some());
        if let Some(value) = value {
            self.source_patch = Some(value.patch());
        }
        self.numbers
            .sync_with_center_fields(value, self.raw_centers, window, cx);
        self.list.reveal(
            self.selected
                .and_then(|selected| self.rows.iter().position(|(index, _)| *index == selected)),
        );
    }

    fn publish(
        &mut self,
        selection: Option<usize>,
        label: &'static str,
        window: &mut Window,
        cx: &mut EditorContext,
    ) {
        self.changed = true;
        self.rebuild_rows();
        self.rebuild_tile();
        self.set_selected(selection, window, cx);
        cx.emit(EditorEvent::Edited(label));
        cx.notify();
    }

    fn apply_number(
        &mut self,
        field: SynthesisPlacementField,
        value: f64,
        window: &mut Window,
        cx: &mut EditorContext,
    ) {
        if !self.blocked
            && self.drag.is_none()
            && let Some(index) = self.selected
            && self
                .edit
                .set_manual_placement_field(self.layer, index, field, value)
        {
            match field {
                SynthesisPlacementField::CenterX => self.raw_centers[0] = true,
                SynthesisPlacementField::CenterY => self.raw_centers[1] = true,
                _ => {}
            }
            self.publish(Some(index), "Edit patch copy transform", window, cx);
        }
    }

    fn command(&mut self, command: ManualCommand, window: &mut Window, cx: &mut EditorContext) {
        if self.blocked || self.drag.is_some() {
            return;
        }
        let layer = self.layer;
        let (next, label) = match command {
            ManualCommand::Select(index) => {
                self.raw_centers = [false; 2];
                self.set_selected(Some(index), window, cx);
                cx.emit(EditorEvent::Selected);
                cx.notify();
                return;
            }
            ManualCommand::ChoosePatch(patch) => {
                self.source_patch = Some(patch);
                cx.notify();
                let Some(index) = self.selected else {
                    return;
                };
                if !self.edit.set_manual_placement_patch(layer, index, patch) {
                    return;
                }
                (Some(index), "Change patch copy source")
            }
            ManualCommand::NewCopy(patch) => {
                let bits = RandomState::new().hash_one(0_u8);
                let center =
                    [bits as u32, (bits >> 32) as u32].map(|axis| (axis >> 8) as f32 / 16_777_216.);
                let Some(index) = self.edit.create_manual_placement(layer, patch, center) else {
                    return;
                };
                (Some(index), "Add patch copy")
            }
            ManualCommand::Delete(index) | ManualCommand::Cut(index) => {
                let cutting = matches!(command, ManualCommand::Cut(_));
                let clipboard = cutting
                    .then(|| self.edit.copy_manual_placement(layer, index))
                    .flatten();
                let Some(index) = self.edit.delete_manual_placement(layer, index) else {
                    return;
                };
                if let Some(copy) = clipboard {
                    cx.set_global(ManualClipboard(copy));
                }
                (
                    index,
                    if cutting {
                        "Cut patch copy"
                    } else {
                        "Delete patch copy"
                    },
                )
            }
            ManualCommand::MoveForward(index) | ManualCommand::MoveBack(index) => {
                let to = if matches!(command, ManualCommand::MoveForward(_)) {
                    index.saturating_sub(1)
                } else {
                    index.saturating_add(1)
                };
                if !self.edit.move_manual_placement(layer, index, to) {
                    return;
                }
                (Some(to), "Reorder patch copy")
            }
            ManualCommand::Copy(index) => {
                if let Some(copy) = self.edit.copy_manual_placement(layer, index) {
                    cx.set_global(ManualClipboard(copy));
                    cx.notify();
                }
                return;
            }
            ManualCommand::Paste(index) => {
                let Some(copy) = cx.try_global::<ManualClipboard>() else {
                    return;
                };
                let Some(index) = self.edit.paste_manual_placement(layer, index, &copy.0) else {
                    return;
                };
                (Some(index), "Paste patch copy")
            }
        };
        self.raw_centers = [false; 2];
        self.publish(next, label, window, cx);
        self.list.focus(window, cx);
    }

    fn navigate(&mut self, event: SourceViewEvent, cx: &mut EditorContext) {
        if self.navigation.update(event, self.drag.is_some()) {
            self.hover = None;
            cx.notify();
        }
    }

    fn tile_event(&mut self, event: TileEvent, window: &mut Window, cx: &mut EditorContext) {
        match event {
            TileEvent::Begin(drag) if !self.blocked => {
                self.focus.focus(window, cx);
                self.raw_centers = [false; 2];
                self.set_selected(drag.map(|drag| drag.index()), window, cx);
                self.drag = drag;
                cx.emit(EditorEvent::Selected);
                cx.notify();
            }
            TileEvent::Track(position, modifiers, hover) => {
                if let Some(drag) = &mut self.drag {
                    drag.track(position, modifiers);
                    let projected = self
                        .raw
                        .and_then(|source| drag.project(&self.edit, self.layer, source));
                    self.numbers.sync(projected.or(self.value()), window, cx);
                    cx.notify();
                } else if self.hover != hover {
                    self.hover = hover;
                    cx.notify();
                }
            }
            TileEvent::Release(position, modifiers, outside) => {
                let Some(mut drag) = self.drag.take() else {
                    return;
                };
                drag.track(position, modifiers);
                self.hover = None;
                if drag.moved()
                    && let Some(raw) = self.raw
                    && self.edit.transform_placement(
                        self.layer,
                        drag.index(),
                        drag.transform(outside),
                        raw,
                    )
                {
                    self.publish(Some(drag.index()), "Transform patch copy", window, cx);
                } else {
                    self.numbers.sync(self.value(), window, cx);
                    cx.notify();
                }
            }
            TileEvent::Cancel => {
                if self.drag.take().is_some() {
                    self.hover = None;
                    self.numbers.sync(self.value(), window, cx);
                    cx.notify();
                }
            }
            TileEvent::Begin(_) => {}
        }
    }

    fn queue_tile(
        &mut self,
        job: Result<Option<source::ManualPreviewJob>, String>,
        library: mycela_engine::EnvironmentResourceLibrary,
        cx: &mut EditorContext,
    ) {
        if let Some(old) = self.ticket.take() {
            old.retire();
        }
        self.error = None;
        let job = match job {
            Ok(Some(job)) => job,
            Ok(None) => return,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        let ticket = Ticket::new();
        self.ticket = Some(ticket.clone());
        let worker = ticket.clone();
        cx.spawn(async move |editor, cx| {
            let result = cx
                .background_spawn(async move { job.render(&library, &worker) })
                .await;
            if ticket.is_current() {
                let _ = editor.update(cx, |editor, cx| {
                    editor.ticket = None;
                    match result {
                        Ok((raster, raw)) => {
                            editor.raster = Some(raster);
                            if raw != editor.raw {
                                editor.raw = raw;
                                editor.rebuild_tile();
                            }
                        }
                        Err(error) => editor.error = Some(error),
                    }
                    cx.notify();
                });
            }
        })
        .detach();
        cx.notify();
    }
}

impl Drop for Editor {
    fn drop(&mut self) {
        if let Some(ticket) = self.ticket.take() {
            ticket.retire();
        }
    }
}

struct EditorInit {
    owner: Entity<GraphDocumentView>,
    mode: Mode,
    edit: SynthesisEdit,
    layer: usize,
    revision: u64,
    selected: Option<usize>,
    source_patch: Option<usize>,
    blocked: bool,
    output: Option<SourceTexturePreview>,
}

impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut EditorContext) -> impl IntoElement {
        let inline = self.mode == Mode::Inline;
        let rem = window.rem_size();
        let viewport = window.viewport_size();
        let height = (viewport.height - rem * 18.).clamp(rem * 12., rem * 36.);
        let show_output = !inline && viewport.width < rem * 96.;
        let use_output_column = show_output && viewport.width >= rem * 55.;
        let owner = cx.weak_entity();
        let dispatch: ManualDispatch = Rc::new(move |command, window, cx| {
            let _ = owner.update(cx, |editor, cx| editor.command(command, window, cx));
        });
        let moving = self.drag.is_some();
        let patch_count = self
            .edit
            .patch_counts(self.layer)
            .map_or(0, |(patches, _)| patches);
        let projection = self.drag.and_then(|drag| {
            let raw = self.raw?;
            let value = drag.project(&self.edit, self.layer, raw)?;
            let outline = self.edit.placement_outline(self.layer, value, raw)?;
            Some((value, outline))
        });
        let selected = projection
            .as_ref()
            .map(|(value, _)| *value)
            .or_else(|| self.value());
        let tile = self.tile.render(
            tile::TileView {
                inline,
                height,
                selected: self.selected.zip(selected),
                projection: projection.map(|(_, outline)| outline),
                raster: self.raster.clone(),
                source: self.raw,
                navigation: &self.navigation,
                drag: self.drag,
                hover: self.hover,
                focus: self.focus.clone(),
                disabled: self.blocked || self.raw.is_none(),
            },
            cx,
        );
        let list = self.list.render(
            inline,
            ManualCopyView::new(
                &self.rows,
                self.selected,
                cx.try_global::<ManualClipboard>().is_some(),
                self.blocked || moving,
            ),
            dispatch.clone(),
            cx,
        );
        let theme = cx.theme();
        let status = self.error.clone().or_else(|| {
            if self.ticket.is_some() {
                Some("Rendering placement tile…".into())
            } else if self.raw.is_none() {
                Some("Choose or connect a source image.".into())
            } else if self.rows.is_empty() {
                Some("Create a copy to place a patch.".into())
            } else {
                None
            }
        });
        let tile =
            div()
                .v_flex()
                .gap_1()
                .w_full()
                .child(tile)
                .when_some(status, |panel, status| {
                    panel.child(
                        div()
                            .id("synthesis-manual-tile-status")
                            .role(gpui_kit::Role::Status)
                            .text_sm()
                            .text_color(if self.error.is_some() {
                                theme.danger
                            } else {
                                theme.muted_foreground
                            })
                            .child(status),
                    )
                });
        let output = self
            .output
            .as_ref()
            .filter(|_| show_output)
            .map(|output| output.render(&self.owner, cx).into_any_element());
        let (column_output, rail_output) = if use_output_column {
            (output, None)
        } else {
            (None, output)
        };
        let workspace = if inline {
            div()
                .h_flex()
                .items_start()
                .gap_2()
                .child(div().min_w_0().flex_1().child(tile))
                .child(div().w(rems(8.5)).flex_none().child(list))
                .into_any_element()
        } else {
            let shorter_list = rail_output.is_some();
            div()
                .h_flex()
                .items_start()
                .gap_3()
                .child(div().min_w_0().flex_1().child(tile))
                .child(
                    div()
                        .v_flex()
                        .gap_3()
                        .w(rems(13.))
                        .flex_none()
                        .child(
                            div()
                                .h(if shorter_list { rem * 10. } else { height })
                                .child(list),
                        )
                        .children(rail_output),
                )
                .children(column_output)
                .into_any_element()
        };
        div()
            .v_flex()
            .gap_2()
            .w_full()
            .id(if inline {
                "synthesis-manual-editor"
            } else {
                "synthesis-full-manual-editor"
            })
            .test_support()
            .when(inline, |panel| {
                panel.child(
                    div()
                        .h_flex()
                        .gap_2()
                        .items_center()
                        .flex_wrap()
                        .child(div().font_semibold().child("Manual placement"))
                        .child(
                            Button::new("synthesis-edit-patches")
                                .small()
                                .ghost()
                                .label("Source")
                                .disabled(self.blocked || moving)
                                .on_click(cx.listener(|_, _, _, cx| cx.emit(EditorEvent::Source))),
                        )
                        .child(
                            Button::new("synthesis-manual-full-size")
                                .small()
                                .label("Edit full size…")
                                .disabled(self.blocked || moving)
                                .on_click(
                                    cx.listener(|_, _, _, cx| cx.emit(EditorEvent::FullSize)),
                                ),
                        ),
                )
            })
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .items_center()
                    .flex_wrap()
                    .justify_between()
                    .child(controls::toolbar(
                        inline,
                        patch_count,
                        self.source_patch,
                        self.blocked || moving || self.raw.is_none(),
                        dispatch,
                    ))
                    .child(self.navigation.controls(
                        inline,
                        self.blocked || self.raster.is_none() || moving,
                        None,
                        Self::navigate,
                        cx,
                    )),
            )
            .when(!inline, |body| {
                body.child(
                    div()
                        .min_h(rems(3.5))
                        .children(self.numbers.render(false, self.blocked || moving)),
                )
            })
            .child(workspace)
            .when(inline, |body| {
                body.children(self.numbers.render(true, self.blocked || moving))
            })
    }
}

impl GraphDocumentView {
    pub(in crate::graph_document::view::synthesis) fn install_inline_synthesis_manual(
        &mut self,
        selected: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((panel, layer)) = self
            .synthesis
            .as_ref()
            .and_then(|panel| Some((panel, panel.layer?)))
        else {
            return;
        };
        if panel.manual.is_some() || panel.edit.patch_counts(layer).is_none() {
            return;
        }
        let source_patch = PatchList::selected(panel.geometry);
        let init = EditorInit {
            owner: cx.entity(),
            mode: Mode::Inline,
            edit: panel.edit.clone(),
            layer,
            revision: self.revision,
            selected,
            source_patch,
            blocked: self.is_busy(),
            output: None,
        };
        let editor = cx.new(|cx| Editor::new(init, window, cx));
        let events = cx.subscribe_in(&editor, window, |view, editor, event, window, cx| {
            view.inline_manual_event(editor, event, window, cx);
        });
        if let Some(panel) = &mut self.synthesis {
            panel.geometry = editor.read(cx).selected.map(Placement);
            panel.geometry_fields.clear();
            panel.drag = None;
            panel.manual = Some(Inline {
                editor,
                _events: events,
            });
        }
        self.observe_drafts(cx);
        cx.notify();
    }

    pub(in crate::graph_document::view::synthesis) fn refresh_inline_synthesis_manual(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let Some(panel) = &self.synthesis else {
            return;
        };
        let Some(manual) = &panel.manual else {
            return;
        };
        let editor = manual.editor.clone();
        let request = PreviewRequest {
            revision: self.revision,
            time: self.sample_time.to_bits(),
            library: self.environment_library.root().map(Path::to_path_buf),
        };
        if editor.read(cx).request.as_ref() == Some(&request) {
            return;
        }
        let target = Target::new(self, panel);
        let job = self.prepare_synthesis_manual_preview(&target, &panel.edit);
        editor.update(cx, |editor, cx| {
            editor.request = Some(request);
            editor.queue_tile(job, self.environment_library.clone(), cx);
        });
    }

    fn inline_manual_event(
        &mut self,
        editor: &Entity<Editor>,
        event: &EditorEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(panel) = &self.synthesis else {
            return;
        };
        let draft = editor.read(cx);
        if self.is_busy()
            || draft.revision != self.revision
            || panel
                .manual
                .as_ref()
                .is_none_or(|manual| &manual.editor != editor)
        {
            return;
        }
        let target = Target::new(self, panel);
        if target.material_locked() {
            return;
        }
        let geometry = draft.selected.map(Placement);
        match *event {
            EditorEvent::Selected => {
                self.synthesis
                    .as_mut()
                    .expect("active Manual panel")
                    .geometry = geometry;
            }
            EditorEvent::Edited(label) => {
                let edit = draft.edit.clone();
                let revision = self.revision;
                self.commit_synthesis(&target, edit, (target.layer, geometry), label, window, cx);
                if self.revision == revision
                    && let Some(panel) = self.synthesis.as_mut()
                    && let Some(manual) = &panel.manual
                    && let Some(layer) = panel.layer
                {
                    panel.geometry = manual.sync(&panel.edit, layer, revision, window, cx);
                }
                self.refresh_inline_synthesis_manual(cx);
            }
            EditorEvent::Source => {
                let geometry = draft
                    .source_patch
                    .map(|selection| Patch { selection, knot: 0 });
                self.synthesis.as_mut().expect("active Manual panel").manual = None;
                self.synthesis_geometry_command(
                    &target,
                    GeometryCommand::Select(geometry),
                    window,
                    cx,
                );
            }
            EditorEvent::FullSize => {
                self.open_full_size_synthesis_manual(target, editor, window, cx);
            }
        }
    }

    fn open_full_size_synthesis_manual(
        &mut self,
        target: Target,
        inline: &Entity<Editor>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.has_field_drafts(cx) {
            return;
        }
        let editor = inline.read(cx);
        let init = EditorInit {
            owner: cx.entity(),
            mode: Mode::FullSize,
            edit: editor.edit.clone(),
            layer: editor.layer,
            revision: self.revision,
            selected: editor.selected,
            source_patch: editor.source_patch,
            blocked: false,
            output: Some(SourceTexturePreview::read(self)),
        };
        let ready = (editor.raster.clone(), editor.raw);
        let needs_tile = editor.ticket.is_some() || ready.0.is_none();
        let job = needs_tile.then(|| self.prepare_synthesis_manual_preview(&target, &init.edit));
        let library = self.environment_library.clone();
        let draft = cx.new(|cx| {
            let mut draft = Editor::new(init, window, cx);
            (draft.raster, draft.raw) = ready;
            draft.rebuild_tile();
            if let Some(job) = job {
                draft.queue_tile(job, library, cx);
            }
            draft
        });
        let owner = cx.weak_entity();
        let inline = inline.clone();
        let editor_id = draft.entity_id();
        self.pause_preview();
        self.modal = true;
        let preview_target = target.clone();
        let event = cx.subscribe(&draft, move |view, draft, event: &EditorEvent, cx| {
            if !matches!(event, EditorEvent::Edited(_)) || !preview_target.current(view) {
                return;
            }
            let edit = draft.read(cx).edit.clone();
            let tile = view.prepare_synthesis_manual_preview(&preview_target, &edit);
            let library = view.environment_library.clone();
            draft.update(cx, |editor, cx| editor.queue_tile(tile, library, cx));
            let target = preview_target.clone();
            view.queue_draft_preview(
                editor_id,
                move |view| view.freeze_source_draft(&target, edit),
                cx,
            );
        });
        self.draft_preview = Some(draft_preview::DraftPreview::new(editor_id, event));
        window.open_dialog(cx, move |dialog, window, _| {
            let dismiss = owner.clone();
            let apply_editor = draft.clone();
            let apply_owner = owner.clone();
            let target = target.clone();
            let inline = inline.clone();
            let apply = move |window: &mut Window, cx: &mut App| {
                let draft = apply_editor.read(cx);
                let edit = draft.edit.clone();
                let changed = draft.changed;
                let selection = draft.selected;
                let source_patch = draft.source_patch;
                let result = apply_owner
                    .update(cx, |view, cx| {
                        if !target.current(view) || view.work.is_some() {
                            return Err(
                                "The Patches layer changed. Reopen Manual placement.".to_owned()
                            );
                        }
                        if changed {
                            view.error = None;
                            view.commit_synthesis(
                                &target,
                                edit,
                                (target.layer, selection.map(Placement)),
                                "Edit manual patch placements",
                                window,
                                cx,
                            );
                            if let Some(error) = view.error.clone() {
                                return Err(error);
                            }
                            view.refresh_inline_synthesis_manual(cx);
                        }
                        inline.update(cx, |editor, cx| {
                            editor.source_patch = source_patch;
                            editor.raw_centers = [false; 2];
                            editor.set_selected(selection, window, cx);
                            cx.notify();
                        });
                        if let Some(panel) = &mut view.synthesis {
                            panel.geometry = selection.map(Placement);
                        }
                        Ok(())
                    })
                    .unwrap_or_else(|_| Err("The graph window closed.".into()));
                if let Err(error) = result {
                    apply_editor.update(cx, |draft, cx| {
                        draft.error = Some(error);
                        cx.notify();
                    });
                    return;
                }
                window.close_dialog(cx);
                let _ = apply_owner.update(cx, |view, cx| {
                    view.close_draft_preview(editor_id);
                    view.modal = false;
                    view.focus.focus(window, cx);
                    cx.notify();
                });
            };
            let keyboard_apply = apply.clone();
            let keys = draft.clone();
            let cancel = draft.clone();
            let width = (window.viewport_size().width - window.rem_size() * 3.)
                .min(window.rem_size() * 66.);
            dialog
                .title("Manual patch placement")
                .width(width)
                .overlay_closable(false)
                .child(draft.clone())
                .footer(
                    DialogFooter::new()
                        .child(DialogClose::new().child(Button::new("cancel").label("Cancel")))
                        .child(
                            Button::new("ok")
                                .primary()
                                .label("Apply")
                                .on_click(move |_, window, cx| apply(window, cx)),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    if let Some((field, value)) = keys.read(cx).numbers.focused(window, cx) {
                        keys.update(cx, |editor, cx| {
                            if let Some(value) = value {
                                editor.apply_number(field, value, window, cx);
                            }
                            editor.numbers.sync_with_center_fields(
                                editor.value(),
                                editor.raw_centers,
                                window,
                                cx,
                            );
                        });
                    } else if keys.read(cx).drag.is_none() {
                        keyboard_apply(window, cx);
                    }
                    false
                })
                .on_cancel(move |_, window, cx| {
                    if cancel.read(cx).drag.is_none() {
                        return true;
                    }
                    cancel.update(cx, |editor, cx| {
                        editor.tile_event(TileEvent::Cancel, window, cx)
                    });
                    false
                })
                .on_close(move |_, _, cx| {
                    let _ = dismiss.update(cx, |view, cx| {
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
