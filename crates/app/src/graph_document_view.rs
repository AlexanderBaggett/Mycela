#[path = "graph_document_adjuster_definition.rs"]
mod adjuster_definition;
#[path = "graph_document_adjuster_metadata.rs"]
mod adjuster_metadata;
#[path = "graph_document_adjuster_name.rs"]
mod adjuster_name;
#[path = "graph_document_animation.rs"]
mod animation;
#[path = "graph_document_canvas.rs"]
mod canvas;
#[path = "graph_document_canvas_drawing.rs"]
mod canvas_drawing;
#[path = "graph_document_command_menu.rs"]
mod command_menu;
#[path = "graph_document_curve_adjuster.rs"]
mod curve_adjuster;
#[path = "graph_document_curve_plot.rs"]
mod curve_plot;
#[path = "graph_document_curve_presets.rs"]
mod curve_presets;
#[path = "graph_document_custom.rs"]
pub(super) mod custom;
#[path = "graph_document_workspace.rs"]
mod document_workspace;
#[path = "graph_document_draft_preview.rs"]
mod draft_preview;
#[path = "graph_document_environment.rs"]
pub(super) mod environment;
#[path = "graph_document_environment_export.rs"]
mod environment_export;
#[path = "graph_document_environment_presets.rs"]
mod environment_presets;
#[path = "graph_document_export.rs"]
mod export;
#[path = "graph_document_gradient.rs"]
pub(super) mod gradient;
#[path = "graph_document_group_scope.rs"]
mod group_scope;
#[path = "graph_document_history.rs"]
mod history;
#[path = "graph_document_import_mesh.rs"]
mod import_mesh;
#[path = "graph_document_lab_color_adjusters.rs"]
mod lab_color_adjusters;
#[path = "graph_document_lab_presets.rs"]
mod lab_presets;
#[path = "graph_document_lab_sections.rs"]
mod lab_sections;
#[path = "graph_document_legacy_gradient.rs"]
pub(super) mod legacy_gradient;
#[path = "graph_document_levels.rs"]
pub(super) mod levels;
#[path = "graph_document_modulation.rs"]
pub(super) mod modulation;
#[path = "graph_document_node_library.rs"]
mod node_library;
#[path = "graph_document_node_previews.rs"]
mod node_previews;
#[path = "graph_document_numeric_adjusters.rs"]
mod numeric_adjusters;
#[path = "graph_document_numeric_field.rs"]
pub(super) mod numeric_field;
#[path = "graph_document_numeric_links.rs"]
mod numeric_links;
#[path = "graph_document_overview.rs"]
mod overview;
#[path = "graph_document_overview_presets.rs"]
mod overview_presets;
#[path = "graph_document_preferences.rs"]
mod preferences;
#[path = "graph_document_preset_files.rs"]
mod preset_files;
#[path = "graph_document_preview_display.rs"]
mod preview_display;
#[path = "graph_document_preview_maps.rs"]
mod preview_maps;
#[path = "graph_document_recent_paths.rs"]
mod recent_paths;
#[path = "graph_document_region.rs"]
mod region;
#[path = "graph_document_rename.rs"]
mod rename;
#[path = "graph_document_results.rs"]
mod results;
#[path = "graph_document_selector.rs"]
pub(super) mod selector;
#[path = "graph_document_shader.rs"]
mod shader;
#[path = "graph_document_style.rs"]
mod style;
#[path = "graph_document_synthesis.rs"]
mod synthesis;
use super::{
    inspector::Inspector,
    model::{
        self, DocumentFormat, LiveDocument, Node, Prepared, Preview, PreviewOutput, Ticket, guard,
    },
    navigation::{Navigation, Target},
};
use crate::{ForegroundError, LIVE_PREVIEW_SIZE};
use gpui_kit::base::{
    Disableable, ElementExt, InteractiveElementExt, Selectable, StyledExt, TestSupportExt,
};
use gpui_kit::component::{
    ActiveTheme, Sizable, WindowExt,
    button::{Button, ButtonVariant, ButtonVariants},
    checkbox::Checkbox,
    collapsible::Collapsible,
    dialog::{DialogAction, DialogClose, DialogFooter},
    form::{Field, Form},
    input::{Input, InputEvent, InputState, NumberInput},
    list::ListItem,
    menu::{DropdownMenu, PopupMenu, PopupMenuItem},
    progress::Progress,
    scroll::ScrollableElement,
    tab::{Tab, TabBar},
    tree::{Tree, TreeState},
};
use gpui_kit::{
    AnyWindowHandle, App, AppContext, Context, Entity, EventEmitter, FocusHandle, IntoElement,
    Render, Rgba, Subscription, Task, Window, WindowOptions, div, img, prelude::*, rems,
};
use mycela_engine::{
    Gtx375GraphDocument, HistoryDirection, PointwiseError, PointwiseFilter,
    PointwiseFinalExportCache, PointwiseNode, SolidColorProperty, create_node_by_identity,
};
use std::{
    cell::Cell,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, Mutex},
    time::Duration,
};
const DEFAULT_PREVIEW_RATE: f64 = 10.0;
const MIN_PREVIEW_RATE: f64 = 1.0;
const MAX_PREVIEW_RATE: f64 = 100.0;
gpui_kit::actions!(
    graph_document,
    [
        OpenGraphDocument,
        OpenNodeEditor,
        OpenDocument,
        SaveDocument,
        SaveDocumentAs,
        RenameDocument,
        RevertGraphFields,
        FocusGraph,
        GroupSelectedBranch,
        RenderSelectedBranch,
        ExportGraphImage,
        CopyNodeTree,
        PasteNodeTree,
        PasteNodeIntoSelection,
        UndoGraphEdit,
        RedoGraphEdit,
        RemoveCustomCurvePoint,
        RemoveLegacyStop,
        NudgeLegacyStopLeft,
        NudgeLegacyStopRight,
        CancelCanvasDrawing,
        FinishCanvasPath,
        DeleteCanvasShape
    ]
);
pub(crate) fn bind_keys(cx: &mut App) {
    canvas::bind_keys(cx);
    synthesis::bind_patch_list_keys(cx);
    cx.bind_keys([
        gpui_kit::KeyBinding::new("secondary-alt-o", OpenGraphDocument, None),
        gpui_kit::KeyBinding::new("secondary-o", OpenDocument, Some("GraphDocument")),
        gpui_kit::KeyBinding::new("secondary-s", SaveDocument, Some("GraphDocument")),
        gpui_kit::KeyBinding::new("secondary-shift-s", SaveDocumentAs, Some("GraphDocument")),
        gpui_kit::KeyBinding::new("f2", RenameDocument, Some("GraphDocument")),
        gpui_kit::KeyBinding::new("escape", RevertGraphFields, Some("GraphDocument")),
        gpui_kit::KeyBinding::new("secondary-1", FocusGraph, Some("GraphDocument")),
        gpui_kit::KeyBinding::new("secondary-g", GroupSelectedBranch, Some("GraphDocument")),
        gpui_kit::KeyBinding::new("secondary-r", RenderSelectedBranch, Some("GraphDocument")),
        gpui_kit::KeyBinding::new("secondary-shift-e", ExportGraphImage, Some("GraphDocument")),
        gpui_kit::KeyBinding::new("secondary-c", CopyNodeTree, Some("GraphDocument")),
        gpui_kit::KeyBinding::new("secondary-v", PasteNodeTree, Some("GraphDocument")),
        gpui_kit::KeyBinding::new("secondary-z", UndoGraphEdit, Some("GraphDocument")),
        gpui_kit::KeyBinding::new("secondary-shift-z", RedoGraphEdit, Some("GraphDocument")),
        gpui_kit::KeyBinding::new("delete", RemoveCustomCurvePoint, Some("CustomCurvePlot")),
        gpui_kit::KeyBinding::new("backspace", RemoveCustomCurvePoint, Some("CustomCurvePlot")),
        gpui_kit::KeyBinding::new("delete", RemoveLegacyStop, Some("LegacyColorGradient")),
        gpui_kit::KeyBinding::new("backspace", RemoveLegacyStop, Some("LegacyColorGradient")),
        gpui_kit::KeyBinding::new("left", NudgeLegacyStopLeft, Some("LegacyColorGradient")),
        gpui_kit::KeyBinding::new("right", NudgeLegacyStopRight, Some("LegacyColorGradient")),
        gpui_kit::KeyBinding::new("escape", CancelCanvasDrawing, Some("TextureCanvasDrawing")),
        gpui_kit::KeyBinding::new("enter", FinishCanvasPath, Some("TextureCanvasDrawing")),
        gpui_kit::KeyBinding::new("delete", DeleteCanvasShape, Some("TextureCanvasDrawing")),
        gpui_kit::KeyBinding::new("backspace", DeleteCanvasShape, Some("TextureCanvasDrawing")),
        #[cfg(not(target_os = "macos"))]
        gpui_kit::KeyBinding::new("secondary-y", RedoGraphEdit, Some("GraphDocument")),
    ]);
}
#[derive(Clone)]
enum Intent {
    Open(DocumentFormat),
    Select(String),
    CanvasLevel {
        path: Vec<Node>,
        select_group: Option<Node>,
    },
    Close,
}
#[derive(Clone, Copy, PartialEq)]
enum Work {
    Opening,
    Duplicating,
    ApplyingOverviewPreset,
    ChoosingOverviewPresetFolder,
    ImportingMesh,
    LoadingEnvironment,
    LoadingSynthesis,
    LoadingStyle,
    LoadingNoiseLab,
    OpeningCurvePreset,
    SavingCurvePreset,
    Saving,
    ChoosingSave,
    Renaming,
    ChoosingExport,
    Exporting,
    RenderingResult,
    ProjectingResult,
    ChoosingEnvironmentOutput,
    WritingEnvironmentOutput,
    ChoosingSynthesisPreset,
    WritingSynthesisPreset,
}
impl Work {
    fn can_cancel_export(self) -> bool {
        matches!(
            self,
            Self::Exporting
                | Self::WritingEnvironmentOutput
                | Self::WritingSynthesisPreset
                | Self::SavingCurvePreset
        )
    }
}
struct PreviewPlayback {
    frames: Arc<[Preview]>,
    _timer: Task<()>,
}

struct SavePathDraft {
    input: Entity<InputState>,
    replacement: Option<PathBuf>,
    error: Option<String>,
}

impl SavePathDraft {
    fn confirm(&mut self, cx: &mut Context<Self>) -> Option<PathBuf> {
        let raw = self.input.read(cx).value();
        let mut path = PathBuf::from(raw.trim());
        let result = (|| {
            if !path.is_absolute() || path.file_name().is_none() {
                return Err("Enter the full path, including the filename.".into());
            }
            if path.extension().is_none() {
                path.set_extension(DocumentFormat::Mycela.extension());
            }
            DocumentFormat::Mycela.validate_save_path(&path)?;
            if path.try_exists().map_err(model::failure)?
                && self.replacement.as_ref() != Some(&path)
            {
                self.replacement = Some(path);
                return Ok(None);
            }
            Ok(Some(path))
        })();
        self.error = result.as_ref().err().cloned();
        if self.error.is_some() {
            self.replacement = None;
        }
        cx.notify();
        result.ok().flatten()
    }
}

pub(crate) struct SourceAnimationFrame(pub(crate) f64);

/// Owns a live graph document, navigation and drafts. Preview is an owned
/// render frame; serialization always reads the current document's real nodes.
pub struct GraphDocumentView {
    document: LiveDocument,
    workspace: Option<gpui_kit::WeakEntity<document_workspace::DocumentWorkspace>>,
    overview_open: bool,
    overview: overview::Overview,
    source_animation: Option<SolidColorProperty>,
    export_cache: PointwiseFinalExportCache<ForegroundError>,
    environment_library: mycela_engine::EnvironmentResourceLibrary,
    history_router: Option<Arc<history::Router>>,
    path: Option<PathBuf>,
    imported_from: Option<PathBuf>,
    draft_name: String,
    mesh_import_folder: Option<PathBuf>,
    dirty: bool,
    revision: u64,
    navigation: Navigation,
    canvas: canvas::CanvasState,
    drawing: Option<canvas_drawing::DrawingTool>,
    drawing_focus: FocusHandle,
    curve_plot_drag: Option<curve_plot::CurvePlotDrag>,
    curve_plot_focus: FocusHandle,
    tree: Entity<TreeState>,
    selection: Option<String>,
    inspector: Inspector,
    synthesis: Option<synthesis::Panel>,
    synthesis_preview: synthesis::PreviewState,
    style: Option<style::Panel>,
    embedded_noise: Option<style::noise_lab::Host>,
    embedded_environment: Option<environment::editor::Host>,
    environment_preview: Option<model::EnvironmentPreview>,
    draft_preview: Option<draft_preview::DraftPreview>,
    child_editor_close: Option<Subscription>,
    lab_sections: lab_sections::LabSections,
    lab_thumbnail: Option<lab_sections::LabThumbnail>,
    animation_open: bool,
    bindings_open: bool,
    sample_time: f64,
    sample_time_input: Entity<InputState>,
    sample_time_error: Option<String>,
    focus: FocusHandle,
    window: AnyWindowHandle,
    modal: bool,
    closed: bool,
    work: Option<Work>,
    ticket: Option<Ticket>,
    save_fallback_path: Option<PathBuf>,
    preview_ticket: Option<Ticket>,
    node_previews: node_previews::NodePreviews,
    node_library: node_library::NodeLibrary,
    preview_pending: bool,
    preview_lab_section: bool,
    preview: Option<Preview>,
    preview_maps: preview_maps::PreviewMaps,
    preview_display: preview_display::PreviewDisplay,
    results: results::Results,
    preview_error: Option<String>,
    animated_preview_available: bool,
    animated_preview_pending: bool,
    preview_playback: Option<PreviewPlayback>,
    preview_rate: f64,
    preview_rate_input: Entity<InputState>,
    preview_frame: Option<(usize, usize)>,
    loop_preview: bool,
    error: Option<String>,
    status: Option<String>,
    selection_serial: u64,
    inspector_scroll: gpui_kit::ScrollHandle,
    synthesis_source_anchor: gpui_kit::ScrollAnchor,
    synthesis_guide_top: Rc<Cell<Option<gpui_kit::Pixels>>>,
    draft_observers: Vec<Subscription>,
    levels_subscription: Option<Subscription>,
    _tree_observer: Subscription,
    _window_close: Subscription,
    _preview_rate_subscription: Subscription,
}
impl EventEmitter<SourceAnimationFrame> for GraphDocumentView {}

impl GraphDocumentView {
    fn compact_synthesis_inspector(&self, window: &Window) -> bool {
        window.viewport_size().height < window.rem_size() * 56. && self.synthesis.is_some()
    }

    fn show_nodes_workspace(&mut self, cx: &mut Context<Self>) {
        self.overview_open = false;
        self.results.set_visible(false);
        cx.notify();
    }

    fn render_workspace_tabs(&self, busy: bool, cx: &Context<Self>) -> impl IntoElement {
        let selected = if self.results.is_visible() {
            2
        } else {
            usize::from(!self.overview_open)
        };
        TabBar::new("graph-workspace-tabs")
            .small()
            .selected_index(selected)
            .child(Tab::new().label("Overview").disabled(busy))
            .child(Tab::new().label("Nodes").disabled(busy))
            .child(Tab::new().label("Results").disabled(busy))
            .on_click(cx.listener(|view, index, _, cx| {
                if view.is_busy() {
                    return;
                }
                view.show_nodes_workspace(cx);
                match *index {
                    0 => {
                        view.overview_open = true;
                        view.pause_preview();
                        view.load_overview_catalog(cx);
                    }
                    2 => {
                        view.results.set_visible(true);
                        view.pause_preview();
                    }
                    _ => {}
                }
            }))
    }

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_in_workspace(None, window, cx)
    }

    fn new_in_workspace(
        workspace: Option<gpui_kit::WeakEntity<document_workspace::DocumentWorkspace>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let node = PointwiseNode::<ForegroundError>::solid_color().expect("real Solid constructor");
        let document = Gtx375GraphDocument::new(vec![Some(node)]).expect("one real branch");
        Self::with_document_in_workspace(
            Arc::new(Mutex::new(document)),
            None,
            0.0,
            false,
            workspace,
            window,
            cx,
        )
    }

    fn with_document(
        document: LiveDocument,
        path: Option<PathBuf>,
        time: f64,
        source_animation: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_document_in_workspace(document, path, time, source_animation, None, window, cx)
    }

    fn with_document_in_workspace(
        document: LiveDocument,
        path: Option<PathBuf>,
        time: f64,
        source_animation: bool,
        workspace: Option<gpui_kit::WeakEntity<document_workspace::DocumentWorkspace>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let tree = cx.new(|cx| TreeState::new(cx));
        let handle = window.window_handle();
        let observer = cx.observe(&tree, |view, tree, cx| {
            let requested = tree
                .read(cx)
                .selected_item()
                .map(|item| item.id.to_string());
            if requested == view.selection {
                return;
            }
            let Some(key) = requested else {
                return;
            };
            if view.work.is_some() || view.modal {
                view.restore_tree_selection(cx);
                return;
            }
            view.selection_serial += 1;
            let serial = view.selection_serial;
            let revision = view.revision;
            let handle = view.window;
            cx.spawn(async move |owner, cx| {
                let _ = cx.update_window(handle, |_, window, cx| {
                    owner.update(cx, |view, cx| {
                        if serial == view.selection_serial && revision == view.revision {
                            view.request(Intent::Select(key), window, cx);
                        }
                    })
                });
            })
            .detach();
        });
        let weak = cx.weak_entity();
        let window_id = handle.window_id();
        let close = cx.on_window_closed(move |cx, id| {
            if id == window_id {
                let _ = weak.update(cx, |view, _| view.retire());
            }
        });
        if workspace.is_none() {
            let weak = cx.weak_entity();
            window.on_window_should_close(cx, move |window, cx| {
                weak.update(cx, |view, cx| {
                    if view.embedded_noise.is_some() || view.embedded_environment.is_some() {
                        return !view.is_busy();
                    }
                    if view.child_editor_close.is_some() {
                        return false;
                    }
                    if view.work == Some(Work::Saving) {
                        view.error =
                            Some("Wait for the document save to finish before closing.".into());
                        cx.notify();
                        return false;
                    }
                    if (view.source_animation.is_none() && view.is_dirty())
                        || view.has_field_drafts(cx)
                    {
                        view.request(Intent::Close, window, cx);
                        false
                    } else {
                        view.retire();
                        true
                    }
                })
                .unwrap_or(true)
            });
        }
        let preview_rate_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(DEFAULT_PREVIEW_RATE.to_string())
                .step(0.1)
                .min(MIN_PREVIEW_RATE)
                .max(MAX_PREVIEW_RATE)
        });
        let preview_rate_subscription =
            cx.subscribe_in(&preview_rate_input, window, |view, _, event, window, cx| {
                view.update_preview_rate(event, window, cx);
            });
        let inspector_scroll = gpui_kit::ScrollHandle::new();
        let synthesis_source_anchor = gpui_kit::ScrollAnchor::for_handle(inspector_scroll.clone());
        let mut view = Self {
            document,
            workspace,
            overview_open: false,
            overview: overview::Overview::new(window, cx),
            source_animation: source_animation.then_some(SolidColorProperty::Color),
            export_cache: PointwiseFinalExportCache::default(),
            environment_library: environment::preferred_library(cx),
            history_router: None,
            path,
            imported_from: None,
            draft_name: "Untitled".into(),
            mesh_import_folder: None,
            dirty: false,
            revision: 0,
            navigation: Navigation::default(),
            canvas: canvas::CanvasState::new(window, cx),
            drawing: None,
            drawing_focus: cx.focus_handle().tab_stop(true),
            curve_plot_drag: None,
            curve_plot_focus: cx.focus_handle().tab_stop(true),
            tree,
            selection: None,
            inspector: Inspector::default(),
            synthesis: None,
            synthesis_preview: synthesis::PreviewState::default(),
            style: None,
            embedded_noise: None,
            embedded_environment: None,
            environment_preview: None,
            draft_preview: None,
            child_editor_close: None,
            lab_sections: lab_sections::LabSections::default(),
            lab_thumbnail: None,
            animation_open: source_animation,
            bindings_open: false,
            sample_time: time,
            sample_time_input: cx
                .new(|cx| InputState::new(window, cx).default_value(time.to_string())),
            sample_time_error: None,
            focus: cx.focus_handle().tab_stop(true),
            window: handle,
            modal: false,
            closed: false,
            work: None,
            ticket: None,
            save_fallback_path: None,
            preview_ticket: None,
            node_previews: node_previews::NodePreviews::default(),
            node_library: node_library::NodeLibrary::new(window, cx),
            preview_pending: false,
            preview_lab_section: false,
            preview: None,
            preview_maps: preview_maps::PreviewMaps::default(),
            preview_display: preview_display::PreviewDisplay::default(),
            results: results::Results::default(),
            preview_error: None,
            animated_preview_available: false,
            animated_preview_pending: false,
            preview_playback: None,
            preview_rate: DEFAULT_PREVIEW_RATE,
            preview_rate_input,
            preview_frame: None,
            loop_preview: true,
            error: None,
            status: None,
            selection_serial: 0,
            inspector_scroll,
            synthesis_source_anchor,
            synthesis_guide_top: Rc::default(),
            draft_observers: Vec::new(),
            levels_subscription: None,
            _tree_observer: observer,
            _window_close: close,
            _preview_rate_subscription: preview_rate_subscription,
        };
        view.rebuild(window, cx);
        view.focus.focus(window, cx);
        view.start_preview(cx);
        view
    }

    fn watch_child_editor(&mut self, child: gpui_kit::WindowId, cx: &mut Context<Self>) {
        let owner = cx.weak_entity();
        self.child_editor_close = Some(cx.on_window_closed(move |cx, id| {
            if id == child {
                let _ = owner.update(cx, |parent, cx| {
                    parent.modal = false;
                    parent.child_editor_close = None;
                    if parent.environment_preview.take().is_some() {
                        parent.start_preview(cx);
                    }
                    cx.notify();
                });
            }
        }));
        self.pause_preview();
        self.modal = true;
    }

    pub fn selected_node(&self) -> Option<Node> {
        self.selection
            .as_deref()
            .and_then(|key| self.navigation.target(key))
            .and_then(|target| target.node())
    }
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }
    pub fn is_dirty(&self) -> bool {
        self.dirty || self.history().is_some_and(|history| history.is_dirty())
    }
    pub fn is_busy(&self) -> bool {
        self.work.is_some() || self.modal
    }
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref().or_else(|| {
            self.inspector
                .drafts
                .iter()
                .find_map(|draft| draft.live_error.as_deref())
        })
    }
    pub fn preview_error(&self) -> Option<&str> {
        self.draft_preview_error().or_else(|| {
            if self.synthesis_preview_mode().is_some() {
                self.synthesis_preview_error()
            } else {
                self.preview_error.as_deref()
            }
        })
    }
    pub fn raster_pixels(&self) -> Option<&[u8]> {
        self.visible_preview().and_then(|p| p.raster.as_bytes(0))
    }
    pub fn preview_bitmap(&self) -> Option<&mycela_engine::PreviewBitmap> {
        self.visible_preview().map(|p| &p.bitmap)
    }

    fn visible_preview(&self) -> Option<&model::Preview> {
        self.draft_preview_frame().or_else(|| {
            if self.synthesis_preview_mode().is_some() {
                self.synthesis_preview_frame()
            } else {
                self.preview.as_ref()
            }
        })
    }
    pub fn branch_count(&self) -> usize {
        self.navigation
            .rows
            .iter()
            .filter(|r| matches!(r.target, Target::Branch(..)))
            .count()
    }
    fn retire(&mut self) {
        self.closed = true;
        self.overview.retire();
        self.drawing = None;
        self.synthesis = None;
        self.synthesis_preview = synthesis::PreviewState::default();
        self.embedded_environment = None;
        self.draft_preview = None;
        self.pause_preview();
        self.deactivate_history();
        if self.source_animation.is_some() {
            guard(&self.document).unbind_property_history();
        }
        if let Some(ticket) = self.ticket.take() {
            ticket.retire();
        }
        if let Some(ticket) = self.preview_ticket.take() {
            ticket.retire();
        }
        self.export_cache = PointwiseFinalExportCache::default();
        self.lab_thumbnail = None;
        self.node_previews.clear();
        self.results = results::Results::default();
    }
    fn restore_tree_selection(&self, cx: &mut Context<Self>) {
        let selected = self.selection.clone();
        self.tree.update(cx, |tree, cx| {
            let item = selected
                .as_ref()
                .and_then(|key| tree.index_of(&key.clone().into()))
                .and_then(|ix| tree.entry(ix))
                .map(|e| e.item().clone());
            tree.set_selected_item(item.as_ref(), cx);
        });
    }
    fn rebuild(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.rebuild_navigation(cx);
        let key = self
            .selection
            .clone()
            .filter(|k| {
                self.navigation
                    .target(k)
                    .is_some_and(|target| self.canvas.contains(&target))
            })
            .or_else(|| self.canvas.first_content_key(&self.navigation))
            .or_else(|| self.navigation.first());
        self.select(key, window, cx);
    }
    fn rebuild_navigation(&mut self, cx: &mut Context<Self>) {
        let items = self.navigation.rebuild(&self.document);
        self.bind_history();
        self.canvas.rebuild(&self.navigation);
        self.tree.update(cx, |tree, cx| tree.set_items(items, cx));
    }
    fn select(&mut self, key: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.select_with_lab_section(key, None, window, cx);
    }
    fn select_with_lab_section(
        &mut self,
        key: Option<String>,
        section: Option<&Node>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.curve_plot_drag = None;
        let same_selection = self.selection == key;
        let old_lab_section = same_selection
            .then(|| self.lab_sections.selected().cloned())
            .flatten();
        let old_layer = (self.selection == key)
            .then(|| self.inspector.layer.clone())
            .flatten();
        let old_node = self.inspector.owner.clone();
        let old_checkerboard_color = self.inspector.checkerboard_color_open;
        let old_gradient_selection = same_selection
            .then(|| self.inspector.legacy_gradient.as_ref()?.selection(cx))
            .flatten();
        let old_style = self.style.take();
        let old_canvas_area = same_selection
            .then(|| {
                self.inspector
                    .canvas_area
                    .as_ref()
                    .map(|area| area.editor.clone())
            })
            .flatten();
        let old_environment = if self.selection == key {
            self.inspector.environment.clone()
        } else {
            None
        };
        self.selection = key;
        self.restore_tree_selection(cx);
        let node = self.selected_node();
        self.lab_sections = node
            .as_ref()
            .map_or_else(lab_sections::LabSections::default, |node| {
                lab_sections::LabSections::load(node, section.or(old_lab_section.as_ref()))
                    .unwrap_or_else(|error| {
                        self.error = Some(error);
                        lab_sections::LabSections::default()
                    })
            });
        if self.preview_lab_section
            && (!same_selection
                || !self.lab_sections.editing_nested()
                || !node
                    .as_ref()
                    .zip(old_node.as_ref())
                    .is_some_and(|(current, old)| current.same_reference(old)))
        {
            self.preview_lab_section = false;
            self.preview = None;
        }
        self.animated_preview_available = self
            .preview_target()
            .is_some_and(|node| node.has_animated_preview().unwrap_or(false));
        self.inspector = match node {
            Some(node) => {
                let root_key = self.selection.as_deref().unwrap_or("document");
                let nested = self.lab_sections.editing_nested();
                let controls = self.lab_sections.selected().unwrap_or(&node);
                let section_key =
                    nested.then(|| format!("{root_key}:lab:{}", controls.occurrence_id()));
                let mut fields = Inspector::load_with_environment_adjusters(
                    controls,
                    section_key.as_deref().unwrap_or(root_key),
                    nested,
                    |property| guard(&self.document).environment_adjuster(property),
                    window,
                    cx,
                )
                .unwrap_or_else(|error| {
                    self.error = Some(error);
                    Inspector::default()
                });
                if old_node
                    .as_ref()
                    .is_some_and(|old| old.same_reference(&node))
                {
                    fields.checkerboard_color_open = old_checkerboard_color;
                }
                fields.owner = Some(node);
                fields
            }
            None => Inspector::default(),
        };
        self.style = self
            .inspector
            .owner
            .as_ref()
            .and_then(|node| style::Panel::load(node, old_style.as_ref(), window, cx));
        self.sync_style_environment(old_environment.as_ref(), window, cx);
        self.reload_synthesis(None, window, cx);
        let same_section = match (old_lab_section.as_ref(), self.lab_sections.selected()) {
            (Some(old), Some(new)) => old.same_reference(new),
            (None, None) => true,
            _ => false,
        };
        if same_section
            && let (Some(old), Some(new)) = (old_node.as_ref(), self.inspector.owner.as_ref())
            && old.same_reference(new)
            && let Some(gradient) = self.inspector.legacy_gradient.as_mut()
            && let Some((position, focus)) = old_gradient_selection
        {
            gradient.restore_selection(position, focus, window, cx);
        }
        if same_section
            && let (Some(layer), Some(old_node), Some(new_node)) =
                (old_layer, old_node.as_ref(), self.inspector.owner.as_ref())
            && old_node.same_reference(new_node)
            && layer.validate(None).is_ok()
        {
            let _ = self.inspector.select_layer(layer, window, cx);
        }
        if same_section
            && let Some(previous) = old_environment
            && let (Some(old), Some(new)) = (old_node.as_ref(), self.inspector.owner.as_ref())
            && old.same_reference(new)
            && let Some(index) = self
                .inspector
                .environment_panels
                .iter()
                .position(|panel| panel.same_source(&previous))
            && (index != 0 || previous.selected().is_some_and(|layer| layer != 0))
            && let Err(error) = self.inspector.select_environment_parameter(
                index,
                previous.selected().unwrap_or(0),
                window,
                cx,
            )
        {
            self.error = Some(error);
        }
        let canvas_areas = &self.inspector.canvas_areas;
        let area = canvas_areas
            .iter()
            .find(|area| {
                old_canvas_area
                    .as_ref()
                    .is_some_and(|old| old.same_reference(area))
            })
            .or_else(|| canvas_areas.first());
        if let Some(area) = area.cloned()
            && let Err(error) = self.inspector.select_canvas_area(area, window, cx)
        {
            self.error = Some(error);
        }
        if let Some(drawing) = self.drawing.as_mut() {
            if !same_selection
                || self.lab_sections.editing_nested()
                || !self
                    .inspector
                    .owner
                    .as_ref()
                    .is_some_and(|node| drawing.is_target(node))
            {
                self.drawing = None;
            } else {
                drawing.refresh();
                if drawing.is_selecting()
                    && let Some(current) = drawing.single_selection()
                    && let Err(error) =
                        self.inspector
                            .select_canvas_shape(Some(current), window, cx)
                {
                    self.error = Some(error);
                }
            }
        }
        self.observe_drafts(cx);
        self.attach_levels_editor(window, cx);
        self.refresh_canvas_design_plan(cx);
        cx.notify();
    }
    fn has_field_drafts(&self, cx: &App) -> bool {
        self.inspector.is_changed(cx)
            || self.style.as_ref().is_some_and(|panel| panel.pending(cx))
            || self
                .synthesis
                .as_ref()
                .is_some_and(|panel| panel.pending(cx))
    }
    fn observe_drafts(&mut self, cx: &mut Context<Self>) {
        let mut inputs: Vec<_> = self
            .inspector
            .drafts
            .iter()
            .map(|draft| draft.input.clone())
            .collect();
        if let Some(custom) = &self.inspector.custom {
            inputs.extend(custom.fields.iter().cloned());
        }
        if let Some(panel) = &self.synthesis {
            inputs.extend(panel.inputs());
        }
        if let Some(panel) = &self.style {
            inputs.extend(panel.inputs());
        }
        for gradient in self
            .inspector
            .gradient
            .iter()
            .chain(&self.inspector.mask_gradient)
        {
            inputs.push(gradient.position.clone());
        }
        if let Some(gradient) = &self.inspector.legacy_gradient {
            inputs.extend(gradient.inputs());
        }
        self.draft_observers = inputs
            .iter()
            .map(|input| cx.observe(input, |_, _, cx| cx.notify()))
            .collect();
        self.draft_observers.extend(
            self.inspector
                .drafts
                .iter()
                .filter(|draft| draft.is_live())
                .map(|draft| {
                    cx.subscribe(&draft.input, |view, input, event: &InputEvent, cx| {
                        if matches!(event, InputEvent::Change) {
                            view.commit_live_field(&input, cx);
                        }
                    })
                }),
        );
        self.draft_observers
            .extend(self.staged_field_inputs().into_iter().map(|input| {
                cx.subscribe(&input, |view, input, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        view.queue_staged_field(input, cx);
                    }
                })
            }));
        if let Some(gradient) = &self.inspector.legacy_gradient {
            self.draft_observers
                .push(cx.observe(gradient.picker(), |view, _, cx| {
                    if let Some(gradient) = &view.inspector.legacy_gradient
                        && gradient.is_changed(cx)
                        && let Some(input) = gradient.inputs().next()
                    {
                        view.queue_staged_field(input, cx);
                    }
                    cx.notify();
                }));
        }
        if let Some(editor) = &self.inspector.selector_color {
            self.draft_observers
                .push(cx.observe(editor.picker(), |_, _, cx| cx.notify()));
        }
    }

    fn staged_field_inputs(&self) -> Vec<Entity<InputState>> {
        let mut inputs = self
            .inspector
            .drafts
            .iter()
            .filter(|draft| !draft.is_live())
            .map(|draft| draft.input.clone())
            .collect::<Vec<_>>();
        if let Some(panel) = &self.style {
            inputs.extend(panel.inputs());
        }
        if let Some(panel) = &self.synthesis {
            inputs.extend(panel.inputs());
        }
        if let Some(gradient) = &self.inspector.legacy_gradient {
            inputs.extend(gradient.inputs());
        }
        inputs
    }

    fn queue_staged_field(&self, input: Entity<InputState>, cx: &mut Context<Self>) {
        let owner = cx.weak_entity();
        let handle = self.window;
        cx.defer(move |cx| {
            let _ = handle.update(cx, |_, window, cx| {
                let _ = owner.update(cx, |view, cx| {
                    view.commit_staged_field(input, window, cx);
                });
            });
        });
    }

    fn commit_staged_field(
        &mut self,
        input: Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self
            .staged_field_inputs()
            .iter()
            .position(|field| *field == input)
        else {
            return;
        };
        let focused = gpui_kit::Focusable::focus_handle(input.read(cx), cx).is_focused(window);
        let range = input.read(cx).selected_range();
        let selection = self.selection.clone();
        let field_id = self
            .inspector
            .drafts
            .iter()
            .find(|draft| draft.input == input)
            .map(|draft| draft.id.clone());
        self.commit_staged_fields(window, cx);
        if focused && self.selection == selection {
            let field = if let Some(id) = field_id {
                self.inspector
                    .drafts
                    .iter()
                    .find(|draft| draft.id == id)
                    .map(|draft| draft.input.clone())
            } else {
                self.staged_field_inputs().get(index).cloned()
            };
            if let Some(field) = field.filter(|field| *field != input) {
                field.update(cx, |input, cx| {
                    input.set_selected_range(range, cx);
                    input.focus(window, cx);
                });
            }
        }
    }

    fn commit_live_field(&mut self, input: &Entity<InputState>, cx: &mut Context<Self>) {
        if self.closed || self.is_busy() {
            return;
        }
        let Some(index) = self
            .inspector
            .drafts
            .iter()
            .position(|draft| draft.input == *input)
        else {
            return;
        };
        if !self.is_original_selection(&self.selection, &self.inspector.owner, self.revision) {
            self.inspector.drafts[index].live_error =
                Some("The selected node changed. Select the node again.".into());
            self.retire_stale_preview();
            cx.notify();
            return;
        }
        self.ensure_history_binding();
        let was_dirty = self.dirty;
        let stamp = self.begin_history_edit();
        let draft = &mut self.inspector.drafts[index];
        if draft.commit_live(cx) {
            let structural = draft.changes_socket_count();
            if draft.edits_direct_gradient() {
                self.node_previews.report_local_gradient_edit(self.revision);
            }
            self.finish_history_edit(stamp, true);
            self.revision += 1;
            self.status = None;
            self.error = None;
            if structural {
                self.rebuild_navigation(cx);
                self.restore_tree_selection(cx);
            }
            self.start_preview(cx);
        } else {
            self.dirty = was_dirty;
        }
        cx.notify();
    }
    fn request(&mut self, intent: Intent, window: &mut Window, cx: &mut Context<Self>) {
        if self.work.is_some()
            || self.modal
            || ((self.source_animation.is_some()
                || self.embedded_noise.is_some()
                || self.embedded_environment.is_some())
                && matches!(intent, Intent::Open(_)))
        {
            return;
        }
        if let Intent::Open(format) = &intent
            && self.workspace.is_some()
        {
            self.choose_open(*format, cx);
            return;
        }
        self.pause_preview();
        let draft = self.has_field_drafts(cx);
        let confirm = match &intent {
            Intent::Select(_) | Intent::CanvasLevel { .. } => draft,
            Intent::Open(_) | Intent::Close => {
                draft || (self.source_animation.is_none() && self.is_dirty())
            }
        };
        if !confirm {
            self.perform(intent, window, cx);
            return;
        }
        self.restore_tree_selection(cx);
        self.modal = true;
        let owner = cx.weak_entity();
        let close = owner.clone();
        let target = intent.clone();
        let (title, label, body) = match intent {
            Intent::Select(_) | Intent::CanvasLevel { .. } => (
                "Discard unfinished field text?",
                "Discard text",
                "The graph keeps the last valid values.",
            ),
            Intent::Open(DocumentFormat::Mycela) => (
                "Open another document?",
                "Discard and open…",
                "Save first to keep current changes. Your current document stays open if the picker is cancelled or the new file cannot open.",
            ),
            Intent::Open(DocumentFormat::Gtx) => (
                "Import a Genetica GTX?",
                "Discard and import…",
                "Save first to keep current changes. The current project stays open if the import is cancelled or fails.",
            ),
            Intent::Close if self.source_animation.is_some() => (
                "Discard unfinished field text?",
                "Discard text",
                "Solid Color keeps the last valid values.",
            ),
            Intent::Close => (
                "Close unsaved document?",
                "Discard changes",
                "Closing discards current unsaved changes.",
            ),
        };
        window.open_dialog(cx, move |dialog, _, _| {
            let owner = owner.clone();
            let close = close.clone();
            let target = target.clone();
            dialog
                .title(title)
                .child(body)
                .footer(
                    DialogFooter::new()
                        .child(DialogClose::new().child(Button::new("cancel").label("Cancel")))
                        .child(DialogAction::new().child(Button::new("ok").danger().label(label))),
                )
                .on_ok(move |_, window, cx| {
                    let _ = owner.update(cx, |view, cx| {
                        view.modal = false;
                        view.perform(target.clone(), window, cx);
                    });
                    true
                })
                .on_close(move |_, _, cx| {
                    let _ = close.update(cx, |view, cx| {
                        view.modal = false;
                        cx.notify();
                    });
                })
        });
        cx.notify();
    }
    fn perform(&mut self, intent: Intent, window: &mut Window, cx: &mut Context<Self>) {
        match intent {
            Intent::Select(key) => {
                if let Some(target) = self.navigation.target(&key) {
                    self.canvas
                        .follow_selection(&self.navigation, &key, &target);
                }
                if self.overview_open {
                    self.update_overview_stage_mode(&key, cx);
                }
                self.select(Some(key), window, cx);
                self.start_preview(cx);
            }
            Intent::CanvasLevel { path, select_group } => {
                self.canvas_apply_level(path, select_group, window, cx);
            }
            Intent::Open(format) => self.choose_open(format, cx),
            Intent::Close => {
                self.retire();
                window.remove_window();
            }
        }
    }
    fn owns(&self, ticket: &Ticket) -> bool {
        !self.closed
            && ticket.is_current()
            && self
                .ticket
                .as_ref()
                .is_some_and(|old| Arc::ptr_eq(&old.active, &ticket.active))
    }
    fn choose_open(&mut self, format: DocumentFormat, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        if let Some(folder) = recent_paths::RecentPaths::folders(cx).first().cloned() {
            self.choose_open_in_folder(format, folder, false, cx);
            return;
        }
        let prompt = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(
                match format {
                    DocumentFormat::Mycela => "Open Mycela project",
                    DocumentFormat::Gtx => "Import Genetica GTX",
                }
                .into(),
            ),
        });
        self.load_document_after(
            format,
            async move {
                match prompt.await {
                    Ok(Ok(Some(mut paths))) if paths.len() == 1 => Ok(paths.pop()),
                    Ok(Ok(None)) => Ok(None),
                    Ok(Ok(Some(_))) => Err("Choose one document.".into()),
                    Ok(Err(error)) => Err(format!("Couldn’t open the file picker: {error}")),
                    Err(_) => Err("The file picker closed before returning a result.".into()),
                }
            },
            cx,
        );
    }

    fn choose_open_in_folder(
        &mut self,
        format: DocumentFormat,
        folder: PathBuf,
        explicit: bool,
        cx: &mut Context<Self>,
    ) {
        let mut app = cx.to_async();
        let owner = cx.weak_entity();
        let window = self.window;
        self.load_document_after(
            format,
            async move {
                let folder = if recent_paths::missing(&folder, true, &app).await {
                    app.update(|cx| {
                        let _ = recent_paths::RecentPaths::forget_folder(&folder, cx);
                        if explicit {
                            let _ = owner.update(cx, |view, cx| {
                                view.error = Some(format!(
                                    "This folder no longer exists: {}",
                                    folder.display()
                                ));
                                cx.notify();
                            });
                        }
                    });
                    dirs::document_dir()
                        .unwrap_or_else(|| folder.parent().unwrap_or(&folder).to_owned())
                } else {
                    folder
                };
                recent_paths::open_in_folder(window, folder, format, &mut app).await
            },
            cx,
        );
    }

    fn open_recent_file(&mut self, format: DocumentFormat, path: PathBuf, cx: &mut Context<Self>) {
        let app = cx.to_async();
        self.load_document_after(
            format,
            async move {
                if recent_paths::missing(&path, false, &app).await {
                    app.update(|cx| {
                        let _ = recent_paths::RecentPaths::forget_file(&path, cx);
                    });
                    Err(format!("This file no longer exists: {}", path.display()))
                } else {
                    Ok(Some(path))
                }
            },
            cx,
        );
    }

    fn load_document_after(
        &mut self,
        format: DocumentFormat,
        selected: impl std::future::Future<Output = Result<Option<PathBuf>, String>> + 'static,
        cx: &mut Context<Self>,
    ) {
        if self.is_busy() {
            return;
        }
        let ticket = Ticket::new();
        self.ticket = Some(ticket.clone());
        self.work = Some(Work::Opening);
        cx.notify();
        let background = cx.background_executor().clone();
        let handle = self.window;
        let library = self.environment_library.clone();
        cx.spawn(async move |owner, cx| {
            let selected = selected.await;
            if !cx
                .update_window(handle, |_, _, cx| {
                    owner.read_with(cx, |view, _| view.owns(&ticket))
                })
                .is_ok_and(|v| v.is_ok_and(|v| v))
            {
                return;
            }
            let result = match selected {
                Ok(Some(path)) => {
                    let work = ticket.clone();
                    background
                        .spawn(async move { Prepared::read(path, format, &work, Some(&library)) })
                        .await
                }
                Ok(None) => Ok(None),
                Err(error) => Err(error),
            };
            let _ = cx.update_window(handle, |_, window, cx| {
                let pending =
                    owner
                        .update(cx, |view, cx| {
                            if !view.owns(&ticket) {
                                return None;
                            }
                            view.work = None;
                            view.ticket = None;
                            cx.notify();
                            match result {
                                Ok(Some(prepared)) => {
                                    if let Some(workspace) = view.workspace.clone() {
                                        if let Err(error) =
                                            recent_paths::RecentPaths::remember_document(
                                                &prepared.path,
                                                prepared.format,
                                                cx,
                                            )
                                        {
                                            eprintln!(
                                                "Could not save recent Mycela paths: {error}"
                                            );
                                        }
                                        return Some((workspace, prepared));
                                    }
                                    view.install(prepared, window, cx);
                                }
                                Ok(None) => {}
                                Err(error) => view.error = Some(error),
                            }
                            None
                        })
                        .ok()
                        .flatten();
                if let Some((workspace, prepared)) = pending {
                    let _ = workspace.update(cx, |workspace, cx| {
                        workspace.open_prepared(prepared, window, cx);
                    });
                }
            });
        })
        .detach();
    }
    fn install(&mut self, prepared: Prepared, window: &mut Window, cx: &mut Context<Self>) {
        self.pause_preview();
        self.lab_thumbnail = None;
        if let Some(ticket) = self.preview_ticket.take() {
            ticket.retire();
        }
        self.preview_pending = false;
        self.preview_frame = None;
        self.deactivate_history();
        self.document = prepared.document;
        self.mesh_import_folder = None;
        self.export_cache = PointwiseFinalExportCache::default();
        (self.path, self.imported_from) = match prepared.format {
            DocumentFormat::Mycela => (Some(prepared.path), None),
            DocumentFormat::Gtx => (None, Some(prepared.path)),
        };
        self.draft_name = self
            .imported_from
            .as_deref()
            .and_then(Path::file_stem)
            .map_or_else(
                || "Untitled".into(),
                |name| name.to_string_lossy().into_owned(),
            );
        self.preview = prepared.preview;
        self.preview_maps = preview_maps::PreviewMaps::default();
        self.preview_display = preview_display::PreviewDisplay::default();
        self.preview_error = prepared.preview_error;
        self.error = None;
        self.status = prepared.import_warning;
        self.dirty = false;
        self.revision += 1;
        self.navigation = Navigation::default();
        self.canvas.clear_level();
        self.selection = None;
        self.rebuild(window, cx);
        self.refresh_lab_thumbnail(cx);
        if let (Some(preview), Some(target)) = (&self.preview, self.preview_target()) {
            self.preview_maps
                .publish_still(target.occurrence_id(), preview);
        }
        self.tree.update(cx, |tree, cx| tree.focus(window, cx));
        if self.sample_time != 0.0 {
            self.start_preview(cx);
        }
    }
    fn set_sample_time(&mut self, cx: &mut Context<Self>) {
        if self.closed || self.is_busy() || self.preview_animation_active() {
            return;
        }
        let value = self.sample_time_input.read(cx).value();
        let parsed = value.trim().parse::<f64>();
        let time = match parsed {
            Ok(time) if time.is_finite() && (0.0..=1.0).contains(&time) => time,
            _ => {
                self.sample_time_error = Some("Enter a sample time from 0 to 1.".into());
                cx.notify();
                return;
            }
        };
        self.sample_time_error = None;
        if self.sample_time != time || self.preview_frame.is_some() {
            self.sample_time = time;
            self.start_preview(cx);
        }
        cx.notify();
    }
    fn preview_animation_active(&self) -> bool {
        self.animated_preview_pending || self.preview_playback.is_some()
    }

    pub(crate) fn sync_source_time(
        &mut self,
        time: f64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.closed || self.sample_time == time {
            return;
        }
        self.sample_time = time;
        self.sample_time_error = None;
        self.sample_time_input.update(cx, |input, cx| {
            input.set_value(time.to_string(), window, cx);
        });
        self.start_preview_job(false, cx);
    }

    pub(crate) fn sync_source_values(&mut self, cx: &mut Context<Self>) {
        if !self.closed {
            self.start_preview_job(false, cx);
        }
    }
    pub(crate) fn source_time(&self) -> f64 {
        self.sample_time
    }

    fn pause_preview(&mut self) {
        self.preview_playback.take();
        if self.animated_preview_pending {
            if let Some(ticket) = self.preview_ticket.take() {
                ticket.retire();
            }
            self.animated_preview_pending = false;
            self.preview_pending = false;
        }
    }

    fn toggle_preview_animation(&mut self, cx: &mut Context<Self>) {
        if self.closed || self.is_busy() || !self.animated_preview_available {
            return;
        }
        if self.preview_animation_active() {
            self.pause_preview();
            cx.notify();
        } else {
            self.start_preview_job(true, cx);
        }
    }

    fn update_preview_rate(
        &mut self,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let commit = matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur);
        if self.closed || self.is_busy() || (!commit && !matches!(event, InputEvent::Change)) {
            return;
        }
        let text = self.preview_rate_input.read(cx).unmask_value();
        let parsed = text.parse::<f64>().ok().filter(|rate| rate.is_finite());
        let rate = if commit {
            parsed
                .unwrap_or(self.preview_rate)
                .clamp(MIN_PREVIEW_RATE, MAX_PREVIEW_RATE)
        } else if let Some(rate) =
            parsed.filter(|rate| (MIN_PREVIEW_RATE..=MAX_PREVIEW_RATE).contains(rate))
        {
            rate
        } else {
            return;
        };
        let rate = (rate * 10.0).round_ties_even() / 10.0;
        if commit && rate.to_string() != text.as_ref() {
            self.preview_rate_input.update(cx, |input, cx| {
                input.set_value(rate.to_string(), window, cx);
            });
        }
        if self.preview_rate == rate {
            return;
        }
        self.preview_rate = rate;
        if let (Some(playback), Some((index, _))) = (&self.preview_playback, self.preview_frame) {
            self.schedule_preview_ticks(playback.frames.clone(), index, window, cx);
        }
        cx.notify();
    }

    fn show_preview_frame(
        &mut self,
        frames: &[Preview],
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.preview = Some(frames[index].clone());
        self.preview_frame = Some((index, frames.len()));
        self.sample_time = index as f64 / frames.len() as f64;
        self.sample_time_error = None;
        self.sample_time_input.update(cx, |input, cx| {
            input.set_value(self.sample_time.to_string(), window, cx);
        });
        if self.source_animation.is_some() {
            cx.emit(SourceAnimationFrame(self.sample_time));
        }
        cx.notify();
    }

    fn play_preview_frames(
        &mut self,
        frames: Vec<Preview>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let last = frames.len() - 1;
        let mut index =
            ((self.sample_time * frames.len() as f64).round_ties_even() as usize).min(last);
        if index == last && !self.loop_preview {
            index = 0;
        }
        self.show_preview_frame(&frames, index, window, cx);
        self.schedule_preview_ticks(frames.into(), index, window, cx);
    }

    fn schedule_preview_ticks(
        &mut self,
        frames: Arc<[Preview]>,
        mut index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let last = frames.len() - 1;
        // The source's playback timer truncates 1000 / fps to milliseconds.
        let interval = Duration::from_millis((1000.0 / self.preview_rate) as u64);
        let selected = self.selection.clone();
        let expected_node = self.inspector.owner.clone();
        let revision = self.revision;
        let playing_frames = frames.clone();
        let timer = cx.spawn_in(window, async move |owner, cx| {
            let frames = playing_frames;
            loop {
                cx.background_executor().timer(interval).await;
                let playing = owner.update_in(cx, |view, window, cx| {
                    if !view.is_original_selection(&selected, &expected_node, revision) {
                        view.retire_stale_preview();
                        cx.notify();
                        return false;
                    }
                    if view.is_busy() {
                        view.pause_preview();
                        cx.notify();
                        return false;
                    }
                    if index < last || view.loop_preview {
                        index = (index + 1) % frames.len();
                        view.show_preview_frame(&frames, index, window, cx);
                    }
                    if index == last && !view.loop_preview {
                        view.pause_preview();
                        cx.notify();
                        return false;
                    }
                    true
                });
                if !matches!(playing, Ok(true)) {
                    break;
                }
            }
        });
        self.preview_playback = Some(PreviewPlayback {
            frames,
            _timer: timer,
        });
    }

    fn render_playback_controls(&self, busy: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let label = if self.animated_preview_pending {
            "Cancel playback"
        } else if self.preview_playback.is_some() {
            "Pause"
        } else {
            "Play"
        };
        div()
            .h_flex()
            .flex_wrap()
            .gap_2()
            .child(
                div()
                    .h_flex()
                    .flex_none()
                    .gap_2()
                    .child(
                        Button::new("graph-play-preview")
                            .small()
                            .tab_index(0)
                            .label(label)
                            .disabled(busy)
                            .on_click(
                                cx.listener(|view, _, _, cx| view.toggle_preview_animation(cx)),
                            ),
                    )
                    .child(
                        Checkbox::new("graph-loop-preview")
                            .label("Loop")
                            .checked(self.loop_preview)
                            .disabled(busy)
                            .on_change(cx.listener(|view, value, _, cx| {
                                view.loop_preview = *value;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .h_flex()
                    .flex_none()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("FPS"),
                    )
                    .child(
                        div()
                            .id("graph-preview-rate")
                            .test_support()
                            .role(gpui_kit::Role::Group)
                            .aria_label("Preview frame rate in frames per second")
                            .w(gpui_kit::rems(7.0))
                            .child(
                                NumberInput::new(&self.preview_rate_input)
                                    .small()
                                    .disabled(busy),
                            ),
                    ),
            )
    }

    fn start_preview(&mut self, cx: &mut Context<Self>) {
        self.refresh_levels_input(cx);
        self.publish_working_environment(cx);
        self.refresh_lab_thumbnail(cx);
        self.refresh_environment_layer_preview(cx);
        self.refresh_synthesis_guide_source(cx);
        if self.source_animation.is_some() && !self.closed {
            cx.emit(SourceAnimationFrame(self.sample_time));
        }
        self.start_preview_job(false, cx);
    }

    fn freeze_preview(&self, node: &Node, time: f64) -> Result<model::PendingPreview, String> {
        model::freeze_with_environment(node, time, self.environment_preview.as_ref())
    }
    fn start_preview_job(&mut self, animate: bool, cx: &mut Context<Self>) {
        self.refresh_synthesis_preview(cx);
        let canvas_tile = self.drawing.is_some();
        let animate = animate && !canvas_tile;
        self.pause_preview();
        if !animate {
            self.preview_frame = None;
        }
        if let Some(ticket) = self.preview_ticket.take() {
            ticket.retire();
        }
        let revision = self.revision;
        let time = self.sample_time;
        let selected = self.selection.clone();
        let expected_node = self.inspector.owner.clone();
        if expected_node.is_some()
            && !self.is_original_selection(&selected, &expected_node, revision)
        {
            self.retire_stale_preview();
            cx.notify();
            return;
        }
        let Some(node) = self.preview_target() else {
            self.preview = None;
            self.preview_maps = preview_maps::PreviewMaps::default();
            self.preview_error = None;
            self.preview_pending = false;
            cx.notify();
            return;
        };
        let target_id = node.occurrence_id();
        let map_selection = if self.supports_preview_maps() {
            self.preview_maps.requested_for(target_id)
        } else {
            Default::default()
        };
        self.animated_preview_available = node.has_animated_preview().unwrap_or(false);
        let pending = match self.freeze_preview(&node, time) {
            Ok(pending) => pending,
            Err(error) => {
                self.preview_error = Some(error);
                self.preview_pending = false;
                cx.notify();
                return;
            }
        };
        drop(node);
        let ticket = Ticket::new();
        self.preview_ticket = Some(ticket.clone());
        self.preview_error = None;
        self.preview_pending = true;
        self.animated_preview_pending = animate;
        let handle = self.window;
        let background = cx.background_executor().clone();
        let library = self.environment_library.clone();
        cx.spawn(async move |owner, cx| {
            let work = ticket.clone();
            let result = background
                .spawn(async move {
                    let frame = pending.materialize()?;
                    let observed = frame.clone();
                    let rendered = if animate {
                        model::render_animation(frame, &work, Some(&library), &map_selection)
                    } else if canvas_tile {
                        model::render_canvas_tile(frame, &work, Some(&library))
                            .map(|p| p.map(PreviewOutput::Still))
                    } else {
                        model::render_map(frame, &work, Some(&library), &map_selection)
                            .map(|p| p.map(PreviewOutput::Still))
                    };
                    Ok::<_, String>((observed, rendered))
                })
                .await;
            let (observed, result) = match result {
                Ok(result) => result,
                Err(error) => {
                    let _ = cx.update_window(handle, |_, _, cx| {
                        owner.update(cx, |view, cx| {
                            if ticket.is_current()
                                && !view.closed
                                && view.revision == revision
                                && view.selection == selected
                            {
                                if !view.is_original_selection(&selected, &expected_node, revision)
                                {
                                    view.retire_stale_preview();
                                    cx.notify();
                                    return;
                                }
                                view.preview_error = Some(error);
                                view.preview_pending = false;
                                view.animated_preview_pending = false;
                                cx.notify();
                            }
                        })
                    });
                    return;
                }
            };
            let mut result = Some(result);
            loop {
                let current = cx.update_window(handle, |_, _, cx| {
                    owner.update(cx, |view, cx| {
                        if !ticket.is_current()
                            || view.closed
                            || view.revision != revision
                            || view.selection != selected
                        {
                            return None;
                        }
                        if !view.is_original_selection(&selected, &expected_node, revision) {
                            view.retire_stale_preview();
                            cx.notify();
                            return None;
                        }
                        let pending = view
                            .preview_target()
                            .filter(|node| node.occurrence_id() == target_id)
                            .ok_or_else(|| {
                                "The render target changed. Render the branch again.".to_string()
                            })
                            .and_then(|node| view.freeze_preview(&node, time));
                        match pending {
                            Ok(pending) => Some(pending),
                            Err(error) => {
                                view.preview_error = Some(error);
                                view.preview_pending = false;
                                view.animated_preview_pending = false;
                                cx.notify();
                                None
                            }
                        }
                    })
                });
                let Ok(Ok(Some(current))) = current else {
                    return;
                };
                let guard = current.values();
                let observed = observed.clone();
                let equal = background
                    .spawn(async move {
                        current
                            .materialize()
                            .map(|current| observed.same_render_values(&current))
                    })
                    .await;
                let retry = cx.update_window(handle, |_, window, cx| {
                    owner.update(cx, |view, cx| {
                        if !ticket.is_current()
                            || view.closed
                            || view.revision != revision
                            || view.selection != selected
                        {
                            return false;
                        }
                        if !view.is_original_selection(&selected, &expected_node, revision) {
                            view.retire_stale_preview();
                            cx.notify();
                            return false;
                        }
                        match equal {
                            Err(error) => {
                                view.preview_error = Some(error);
                                view.preview_pending = false;
                                view.animated_preview_pending = false;
                                cx.notify();
                                false
                            }
                            Ok(false) => {
                                view.start_preview_job(animate, cx);
                                false
                            }
                            Ok(true) => {
                                let pending = view
                                    .preview_target()
                                    .filter(|node| node.occurrence_id() == target_id)
                                    .ok_or_else(|| {
                                        "The render target changed. Render the branch again."
                                            .to_string()
                                    })
                                    .and_then(|node| view.freeze_preview(&node, time));
                                let current = match pending {
                                    Ok(current) => current,
                                    Err(error) => {
                                        view.preview_error = Some(error);
                                        view.preview_pending = false;
                                        view.animated_preview_pending = false;
                                        cx.notify();
                                        return false;
                                    }
                                };
                                if guard != current.values() {
                                    return true;
                                }
                                view.preview_pending = false;
                                view.animated_preview_pending = false;
                                if let Some(result) = result.take() {
                                    match result {
                                        Ok(Some(PreviewOutput::Still(preview))) => {
                                            view.preview_maps.publish_still(target_id, &preview);
                                            view.preview = Some(preview);
                                            view.preview_error = None;
                                        }
                                        Ok(Some(PreviewOutput::Animation(frames))) => {
                                            view.preview_maps.publish_target(target_id);
                                            view.preview_error = None;
                                            view.play_preview_frames(frames, window, cx);
                                        }
                                        Ok(None) => {}
                                        Err(error) => view.preview_error = Some(error),
                                    }
                                }
                                cx.notify();
                                false
                            }
                        }
                    })
                });
                if !matches!(retry, Ok(Ok(true))) {
                    return;
                }
            }
        })
        .detach();
        cx.notify();
    }
    fn cancel_preview(&mut self) {
        self.pause_preview();
        self.lab_thumbnail = None;
        if let Some(ticket) = self.preview_ticket.take() {
            ticket.retire();
        }
        self.preview_pending = false;
    }

    fn retire_stale_preview(&mut self) {
        self.cancel_preview();
        self.preview_error =
            Some("The selected node changed. Revert fields to preview the current graph.".into());
    }
    fn commit_staged_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.closed || self.is_busy() {
            return;
        }
        if self.style.as_ref().is_some_and(|panel| panel.pending(cx)) {
            self.apply_style_fields(window, cx);
            if self.style.as_ref().is_some_and(|panel| panel.pending(cx)) {
                return;
            }
        }
        if self
            .synthesis
            .as_ref()
            .is_some_and(|panel| panel.pending(cx))
        {
            self.apply_synthesis_fields(window, cx);
            if self
                .synthesis
                .as_ref()
                .is_some_and(|panel| panel.pending(cx))
            {
                return;
            }
        }
        if !self.inspector.fields_changed(cx)
            && !self
                .inspector
                .legacy_gradient
                .as_ref()
                .is_some_and(|editor| editor.is_changed(cx))
        {
            return;
        }
        let selection = self.selection.clone();
        let node = self.inspector.owner.clone();
        if !self.is_original_selection(&selection, &node, self.revision) {
            self.error = Some("The selected node changed. Revert edits before continuing.".into());
            self.retire_stale_preview();
            cx.notify();
            return;
        }
        for draft in self
            .inspector
            .drafts
            .iter()
            .filter(|draft| !draft.is_live())
        {
            if let Err(error) = draft.validate_pending_owner(cx) {
                self.error = Some(error);
                cx.notify();
                return;
            }
        }
        let environment_map = match self.inspector.stage_environment(cx) {
            Ok(value) => value,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        let canvas_area = match self.inspector.stage_canvas_area(cx) {
            Ok(settings) => settings,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        let canvas_shape = match self.inspector.stage_canvas_shape(cx) {
            Ok(settings) => settings,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        let light_position = match self.inspector.stage_light_position(cx) {
            Ok(edit) => edit,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        let region = match self.inspector.stage_region(cx) {
            Ok(edit) => edit,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        let legacy_gradient = match self
            .inspector
            .legacy_gradient
            .as_ref()
            .map(|editor| editor.stage(cx))
            .transpose()
        {
            Ok(edit) => edit.flatten(),
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        if self.inspector.legacy_gradient.is_some()
            && legacy_gradient.is_none()
            && !self.inspector.fields_changed(cx)
        {
            self.error = None;
            self.rebuild(window, cx);
            return;
        }
        let mut colorize_gradient = None;
        for draft in &self.inspector.drafts {
            if let Err(error) = draft.stage_colorize_stop(&mut colorize_gradient, cx) {
                self.error = Some(error);
                cx.notify();
                return;
            }
        }
        let mut animation_edits = Vec::new();
        for draft in self
            .inspector
            .drafts
            .iter()
            .filter(|draft| draft.edits_recorded_curve())
        {
            if let Err(error) = draft.stage_animation(&mut animation_edits, cx) {
                self.error = Some(error);
                cx.notify();
                return;
            }
        }
        if canvas_area.is_some()
            || canvas_shape.is_some()
            || environment_map.is_some()
            || light_position.is_some()
            || region.is_some()
            || legacy_gradient.is_some()
            || colorize_gradient.is_some()
            || !animation_edits.is_empty()
            || self
                .inspector
                .drafts
                .iter()
                .any(|draft| draft.is_stored_solid_value() && draft.is_changed(cx))
        {
            self.ensure_history_binding();
        }
        if (environment_map.is_some() || !animation_edits.is_empty())
            && let Some(history) = self.history()
        {
            history.separate_next_edit();
        }
        let stamp = self.begin_history_edit();
        self.revision += 1;
        let revision = self.revision;
        self.status = None;
        self.error = None;
        if let Some(map) = environment_map
            && let Some(panel) = &self.inspector.environment
        {
            self.error = panel.commit(map, &self.document).err();
        }
        if let Some(edit) = legacy_gradient {
            self.error = edit.commit().err();
        }
        if self.error.is_none()
            && let (Some(gradient), Some(editor)) = (colorize_gradient, &self.inspector.gradient)
        {
            self.error = editor.commit_colorize(gradient, &self.document).err();
        }
        if self.error.is_none()
            && let Some(edit) = light_position
        {
            self.error = edit.commit().err();
        }
        if self.error.is_none()
            && let Some(edit) = region
        {
            self.error = edit.commit().err();
        }
        let mut includes_unrecorded_field = self
            .inspector
            .drafts
            .iter()
            .any(|draft| draft.is_unrecorded() && draft.is_changed(cx));
        for draft in &self.inspector.drafts {
            if self.error.is_some() {
                break;
            }
            if draft.is_live()
                || draft.is_environment()
                || draft.is_colorize_stop()
                || draft.is_light_position()
                || draft.is_region_coordinate()
                || draft.edits_recorded_curve()
                || draft.canvas_area_field().is_some()
                || draft.canvas_shape_field().is_some()
            {
                continue;
            }
            if !self.is_original_selection(&selection, &node, revision) {
                self.error = Some(
                    "The selected node changed while applying edits. Earlier changes were kept."
                        .into(),
                );
                self.retire_stale_preview();
                cx.notify();
                return;
            }
            let solid_history = if draft.is_stored_solid_value() && draft.is_changed(cx) {
                self.history().map(|history| {
                    let sequence = history.edit_sequence();
                    (history, sequence)
                })
            } else {
                None
            };
            let result = draft
                .stage_animation(&mut animation_edits, cx)
                .and_then(|staged| if staged { Ok(()) } else { draft.commit(cx) });
            if let Err(error) = result {
                self.error = Some(error);
                break;
            }
            if let Some((history, sequence)) = solid_history {
                includes_unrecorded_field |= history.edit_sequence() == sequence;
            }
        }
        for edit in animation_edits {
            if !self.is_original_selection(&selection, &node, revision) {
                break;
            }
            match edit.apply_staged(self.inspector.layer.as_ref(), &self.document) {
                Ok(Some(layer)) => self.inspector.layer = Some(layer),
                Ok(None) => {}
                Err(error) => {
                    self.error = Some(error);
                    break;
                }
            }
        }
        if self.error.is_none()
            && let Some(settings) = canvas_area
            && self.is_original_selection(&selection, &node, revision)
        {
            self.error = self.inspector.apply_canvas_area(settings).err();
        }
        if self.error.is_none()
            && let Some(settings) = &canvas_shape
            && self.is_original_selection(&selection, &node, revision)
        {
            self.error = self.inspector.apply_canvas_shape(settings).err();
        }
        if !self.is_original_selection(&selection, &node, revision) {
            self.error = Some(
                "The selected node changed while applying edits. Earlier changes were kept.".into(),
            );
            self.retire_stale_preview();
            cx.notify();
            return;
        }
        if self.error.is_none() {
            self.finish_history_edit(stamp, !includes_unrecorded_field);
            self.rebuild(window, cx);
            if self.embedded_environment.is_some() {
                self.focus.focus(window, cx);
            }
        }
        self.start_preview(cx);
        cx.notify();
    }
    fn choose(&mut self, key: &str, index: i32, window: &mut Window, cx: &mut Context<Self>) {
        if self.closed || self.is_busy() {
            return;
        }
        if self.has_field_drafts(cx) {
            self.error = Some("Correct or reset field text before changing a choice.".into());
            cx.notify();
            return;
        }
        let Some(choice) = self
            .inspector
            .choices
            .iter()
            .find(|c| c.id == key)
            .map(|c| c.binding.clone())
        else {
            return;
        };
        if matches!(choice, super::inspector::Choice::Region(_)) && choice.index() == Ok(index) {
            return;
        }
        let selection = self.selection.clone();
        let node = self.inspector.owner.clone();
        if !self.is_original_selection(&selection, &node, self.revision) {
            self.error = Some("The selected node changed. Revert edits before continuing.".into());
            self.retire_stale_preview();
            cx.notify();
            return;
        }
        if let super::inspector::Choice::Filter(property) = &choice
            && property.owner().kind() == mycela_engine::PointwiseKind::Group
            && property
                .owner()
                .parameter(4)
                .is_ok_and(|toggle| toggle.same_reference(property))
        {
            self.choose_group_scope(&property.owner(), index == 0, window, cx);
            return;
        }
        let stamp = self.begin_history_edit();
        self.revision += 1;
        let revision = self.revision;
        self.status = None;
        let result = choice.set(index);
        if !self.is_original_selection(&selection, &node, revision) {
            self.error = Some(result.err().unwrap_or_else(|| "The selected node changed while applying the choice. The earlier change was kept.".into()));
            self.retire_stale_preview();
            cx.notify();
            return;
        }
        self.error = result.err();
        self.finish_history_edit(stamp, self.error.is_none());
        self.rebuild(window, cx);
        self.start_preview(cx);
    }
    fn revert_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.closed || self.is_busy() {
            return;
        }
        let stale_preview = self
            .preview_error
            .as_deref()
            .is_some_and(|error| error.starts_with("The selected node changed"));
        if self.inspector.legacy_gradient.is_some()
            || self
                .error
                .as_deref()
                .is_some_and(|error| error.starts_with("The selected node changed"))
        {
            self.error = None;
        }
        self.rebuild(window, cx);
        if stale_preview {
            self.start_preview(cx);
        }
    }
    fn is_original_selection(
        &self,
        key: &Option<String>,
        node: &Option<Node>,
        revision: u64,
    ) -> bool {
        if self.closed || self.selection != *key || self.revision != revision {
            return false;
        }
        let Some(target) = key.as_deref().and_then(|key| self.navigation.target(key)) else {
            return false;
        };
        if !self.canvas_target_current(&target) {
            return false;
        }
        match (self.selected_node(), node) {
            (None, None) => true,
            (Some(current), Some(expected)) => {
                current.same_reference(expected) && self.lab_sections.is_current(&current)
            }
            _ => false,
        }
    }
    fn add_group_contents(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        if self.has_field_drafts(cx) {
            self.error =
                Some("Correct or reset field text before adding a contents branch.".into());
            cx.notify();
            return;
        }
        let Some(filter) = self.selected_node().and_then(|node| node.filter()) else {
            return;
        };
        if !matches!(
            filter.kind(),
            mycela_engine::PointwiseKind::Group
                | mycela_engine::PointwiseKind::WizardGroup
                | mycela_engine::PointwiseKind::MapVariable
        ) {
            return;
        }
        let stamp = self.begin_history_edit();
        match filter.edit_add_group_content([0.0, 0.0]) {
            Ok(slot) => {
                self.finish_history_edit(stamp, true);
                self.revision += 1;
                self.status = None;
                self.error = None;
                self.rebuild(window, cx);
                let key = self
                    .navigation
                    .rows
                    .iter()
                    .find_map(|row| match &row.target {
                        Target::Slot(current) if current.same_reference(&slot) => {
                            Some(row.key.clone())
                        }
                        _ => None,
                    });
                self.select(key, window, cx);
                self.start_preview(cx);
            }
            Err(error) => {
                self.error = Some(model::failure(error));
                cx.notify();
            }
        }
    }
    fn remove_group_contents(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        if self.has_field_drafts(cx) {
            self.error =
                Some("Correct or reset field text before removing a contents branch.".into());
            cx.notify();
            return;
        }
        let Some(Target::Slot(slot)) = self
            .selection
            .as_deref()
            .and_then(|key| self.navigation.target(key))
        else {
            return;
        };
        let parent = slot.parent();
        let Some(filter) = parent.filter() else {
            return;
        };
        if !matches!(
            filter.kind(),
            mycela_engine::PointwiseKind::Group
                | mycela_engine::PointwiseKind::WizardGroup
                | mycela_engine::PointwiseKind::MapVariable
        ) || !parent
            .contents()
            .iter()
            .skip(1)
            .any(|item| item.same_reference(&slot))
        {
            return;
        }
        if slot.node().is_none() {
            self.finish_remove_group_contents(slot, window, cx);
            return;
        }
        let selection = self.selection.clone();
        let node = slot.node();
        let revision = self.revision;
        let undoable = self.history().is_some();
        self.modal = true;
        let owner = cx.weak_entity();
        let close = owner.clone();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let owner = owner.clone();
            let close = close.clone();
            let slot = slot.clone();
            let selection = selection.clone();
            let node = node.clone();
            alert
                .title("Remove contents branch?")
                .description(if undoable {
                    "This removes the connected nodes from the group."
                } else {
                    "This removes the connected nodes from the group. This change cannot be undone."
                })
                .confirm()
                .ok_text("Remove branch")
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    let _ = owner.update(cx, |view, cx| {
                        view.modal = false;
                        if view.is_original_selection(&selection, &node, revision)
                            && !view.has_field_drafts(cx)
                        {
                            view.finish_remove_group_contents(slot.clone(), window, cx);
                        } else {
                            view.error = Some(
                                "The selected branch changed. Review it before removing it.".into(),
                            );
                            cx.notify();
                        }
                    });
                    true
                })
                .on_close(move |_, _, cx| {
                    let _ = close.update(cx, |view, cx| {
                        view.modal = false;
                        cx.notify();
                    });
                })
        });
        cx.notify();
    }
    fn finish_remove_group_contents(
        &mut self,
        slot: mycela_engine::PointwiseSlot<ForegroundError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let parent = slot.parent();
        let Some(filter) = parent.filter() else {
            return;
        };
        let selection = self.selection.clone();
        let stamp = self.begin_history_edit();
        self.revision += 1;
        let revision = self.revision;
        self.status = None;
        let result = if self.history().is_some() {
            guard(&self.document)
                .remove_group_content(&slot)
                .map_err(model::failure)
        } else {
            let clear = guard(&self.document)
                .edit_slot(&slot, None)
                .map_err(model::failure);
            clear.and_then(|()| {
                if !self.is_original_selection(&selection, &None, revision) {
                    return Err("The selected branch changed while clearing it. Review the graph before removing it.".into());
                }
                filter.remove_empty_group_content(&slot).map_err(model::failure)
            })
        };
        match result {
            Ok(()) => {
                self.error = None;
                self.finish_history_edit(stamp, true);
                self.rebuild(window, cx);
                let key = self.navigation.rows.iter().find_map(|row| {
                    row.target
                        .node()
                        .filter(|node| node.same_reference(&parent))
                        .map(|_| row.key.clone())
                });
                self.select(key, window, cx);
                self.start_preview(cx);
            }
            Err(error) => {
                self.error = Some(error);
                self.rebuild(window, cx);
                self.start_preview(cx);
            }
        }
    }
    fn delete_current(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        if self.has_field_drafts(cx) {
            self.error = Some("Correct or reset field text before deleting a node.".into());
            cx.notify();
            return;
        }
        let Some(target) = self
            .selection
            .as_deref()
            .and_then(|key| self.navigation.target(key))
            .filter(|target| target.node().is_some_and(|node| !node.is_default_input()))
        else {
            return;
        };
        self.ensure_history_binding();
        let stamp = self.begin_history_edit();
        self.revision += 1;
        self.status = None;
        self.error = match target {
            Target::Branch(index, _) => guard(&self.document)
                .delete_current_branch(index)
                .map_err(|error| error.to_string()),
            Target::Slot(slot) => guard(&self.document)
                .delete_current_slot(&slot)
                .map_err(model::failure),
        }
        .err();
        self.finish_history_edit(stamp, self.error.is_none());
        self.rebuild(window, cx);
        self.start_preview(cx);
    }

    fn group_selected_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_busy() || self.source_animation.is_some() {
            return;
        }
        if self.has_field_drafts(cx) {
            self.error = Some("Correct or reset field text before grouping a branch.".into());
            cx.notify();
            return;
        }
        let selection = self.selection.clone();
        let owner = self.inspector.owner.clone();
        let Some(target) = selection
            .as_deref()
            .and_then(|key| self.navigation.target(key))
            .filter(|target| target.node().is_none_or(|node| !node.is_default_input()))
        else {
            return;
        };
        if !self.is_original_selection(&selection, &owner, self.revision) {
            self.error =
                Some("The selected branch changed. Select it again before grouping.".into());
            self.retire_stale_preview();
            cx.notify();
            return;
        }
        self.ensure_history_binding();
        let stamp = self.begin_history_edit();
        self.revision += 1;
        self.status = None;
        let result = {
            let mut document = guard(&self.document);
            match target {
                Target::Branch(index, _) => document.group_branch(index),
                Target::Slot(slot) => document.group_slot(&slot),
            }
        };
        self.error = result.err().map(model::failure);
        self.finish_history_edit(stamp, self.error.is_none());
        self.rebuild(window, cx);
        self.start_preview(cx);
    }

    fn convert_selected_lab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.closed || self.is_busy() || self.source_animation.is_some() {
            return;
        }
        if self.has_field_drafts(cx) {
            self.error = Some("Correct or reset field text before converting the Lab.".into());
            cx.notify();
            return;
        }
        let owner = self.inspector.owner.clone();
        let selection = self.selection.clone();
        let Some(target) = selection
            .as_deref()
            .and_then(|key| self.navigation.target(key))
        else {
            return;
        };
        if !self.is_original_selection(&selection, &owner, self.revision)
            || !matches!(
                target.node().and_then(|node| node.kind()),
                Some(mycela_engine::PointwiseKind::TemplateLab(_))
            )
        {
            return;
        }
        self.ensure_history_binding();
        let was_dirty = self.dirty;
        let stamp = self.begin_history_edit();
        self.revision += 1;
        self.status = None;
        let result = {
            let mut document = guard(&self.document);
            match target {
                Target::Branch(index, _) => document.convert_lab_branch(index),
                Target::Slot(slot) => document.convert_lab_slot(&slot),
            }
        };
        self.error = result.err().map(model::failure);
        if self.error.is_some()
            && self
                .selected_node()
                .zip(owner)
                .is_some_and(|(current, old)| current.same_reference(&old))
        {
            self.dirty = was_dirty;
        }
        self.finish_history_edit(stamp, self.error.is_none());
        self.rebuild(window, cx);
        self.start_preview(cx);
    }

    fn add_canvas_shape(
        &mut self,
        primitive: mycela_engine::CanvasPrimitive,
        input: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_canvas_shape_edit(
            true,
            |filter| filter.edit_add_canvas_shape(primitive, input),
            window,
            cx,
        );
    }

    fn apply_canvas_shape_edit(
        &mut self,
        new_layer: bool,
        edit: impl FnOnce(
            &PointwiseFilter<ForegroundError>,
        ) -> Result<(), PointwiseError<ForegroundError>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.apply_canvas_shape_change(new_layer, |filter| edit(filter).map(|()| true), window, cx)
    }

    fn apply_canvas_shape_change(
        &mut self,
        new_layer: bool,
        edit: impl FnOnce(
            &PointwiseFilter<ForegroundError>,
        ) -> Result<bool, PointwiseError<ForegroundError>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.is_busy() {
            return false;
        }
        if self.has_field_drafts(cx) {
            self.error = Some("Correct or reset field text before editing Canvas shapes.".into());
            cx.notify();
            return false;
        }
        let Some(filter) = self.selected_node().and_then(|node| node.filter()) else {
            return false;
        };
        self.ensure_history_binding();
        let prior_dirty = self.dirty;
        let stamp = self.begin_history_edit();
        let result = edit(&filter);
        if matches!(result, Ok(false)) {
            self.dirty = prior_dirty;
            return false;
        }
        let applied = result.is_ok();
        if applied && new_layer {
            self.inspector.canvas_area = None;
        }
        self.revision += 1;
        self.status = None;
        self.error = result.err().map(model::failure);
        self.finish_history_edit(stamp, self.error.is_none());
        self.rebuild(window, cx);
        self.start_preview(cx);
        applied
    }

    fn canvas_shape_picker(
        &self,
        from_input: bool,
        busy: bool,
        cx: &Context<Self>,
    ) -> Option<gpui_kit::AnyElement> {
        use mycela_engine::CanvasPrimitive;
        let node = self.selected_node()?;
        if node.kind() != Some(mycela_engine::PointwiseKind::Canvas) {
            return None;
        }
        let count = node.reference_inputs().ok()?.len();
        if from_input && count == 0 {
            return None;
        }
        let owner = cx.weak_entity();
        let selection = self.selection.clone();
        let revision = self.revision;
        let add_shape = move |primitive, input, window: &mut Window, cx: &mut App| {
            let _ = owner.update(cx, |view, cx| {
                if view.is_original_selection(&selection, &Some(node.clone()), revision) {
                    view.add_canvas_shape(primitive, input, window, cx);
                }
            });
        };
        let add_items = move |mut menu: PopupMenu, input: Option<usize>| {
            for primitive in CanvasPrimitive::ALL {
                let label = match input {
                    Some(0) if count == 1 => format!("{} from Input 1", primitive.name()),
                    _ => primitive.name().to_owned(),
                };
                let add_shape = add_shape.clone();
                menu = menu.item(PopupMenuItem::new(label).on_click(move |_, window, cx| {
                    add_shape(primitive, input, window, cx);
                }));
            }
            menu.scrollable(true)
        };
        let (id, label) = if from_input {
            ("graph-canvas-input-shape", "Add shape from input…")
        } else {
            ("graph-canvas-shape", "Add shape…")
        };
        Some(
            Button::new(id)
                .small()
                .label(label)
                .disabled(busy || self.has_field_drafts(cx))
                .dropdown_menu(move |mut menu, window, cx| {
                    if from_input && count > 1 {
                        for index in 0..count {
                            let add_items = add_items.clone();
                            menu = menu.submenu(
                                format!("Input {}", index + 1),
                                window,
                                cx,
                                move |menu, _, _| add_items(menu, Some(index)),
                            );
                        }
                        menu.scrollable(true)
                    } else {
                        add_items(menu, from_input.then_some(0))
                    }
                })
                .into_any_element(),
        )
    }

    fn canvas_area_panel(&self, busy: bool, cx: &Context<Self>) -> Option<impl IntoElement> {
        let area = self.inspector.canvas_area.as_ref()?;
        let owner = cx.weak_entity();
        let selection = self.selection.clone();
        let node = self.inspector.owner.clone();
        let revision = self.revision;
        let selected = area.editor.clone();
        let options = self.inspector.canvas_areas.clone();
        let label = selected.label().to_owned();
        let fields = self.inspector.drafts.iter().filter_map(|draft| {
            let field = draft.canvas_area_field()?;
            let inactive =
                field == mycela_engine::CanvasAreaField::OutlineThickness && !area.outline;
            Some(
                Field::new()
                    .label(draft.label.clone())
                    .child(draft.control(busy || inactive)),
            )
        });
        Some(
            div()
                .v_flex()
                .gap_2()
                .child(div().font_semibold().child("Area"))
                .child(
                    Button::new("graph-canvas-area-layer")
                        .small()
                        .label(label)
                        .disabled(busy || self.has_field_drafts(cx) || options.len() < 2)
                        .dropdown_menu(move |mut menu, _, _| {
                            for area in &options {
                                let owner = owner.clone();
                                let selection = selection.clone();
                                let node = node.clone();
                                let area = area.clone();
                                menu = menu.item(
                                    PopupMenuItem::new(area.label().to_owned())
                                        .checked(selected.same_reference(&area))
                                        .on_click(move |_, window, cx| {
                                            let _ = owner.update(cx, |view, cx| {
                                                if view.is_busy()
                                                    || view.has_field_drafts(cx)
                                                    || !view.is_original_selection(
                                                        &selection, &node, revision,
                                                    )
                                                {
                                                    return;
                                                }
                                                view.error = view
                                                    .inspector
                                                    .select_canvas_area(area.clone(), window, cx)
                                                    .err();
                                                view.observe_drafts(cx);
                                                cx.notify();
                                            });
                                        }),
                                );
                            }
                            menu.scrollable(true)
                        }),
                )
                .child(
                    Checkbox::new("graph-canvas-area-outline")
                        .label("Make outline")
                        .checked(area.outline)
                        .disabled(busy)
                        .on_change(cx.listener(|view, enabled, window, cx| {
                            if let Some(area) = view.inspector.canvas_area.as_mut() {
                                area.outline = *enabled;
                                view.commit_staged_fields(window, cx);
                            }
                        })),
                )
                .child(Form::new().columns(2).children(fields)),
        )
    }

    fn replace(&mut self, identity: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        if self.has_field_drafts(cx) {
            self.error = Some("Correct or reset field text before replacing a node.".into());
            cx.notify();
            return;
        }
        let Some(target) = self
            .selection
            .as_deref()
            .and_then(|key| self.navigation.target(key))
        else {
            return;
        };
        if identity.is_some()
            && self
                .selection
                .as_ref()
                .is_some_and(|key| key.matches("/slot-").count() >= 128)
        {
            self.error = Some("This editor supports at most 128 nested nodes.".into());
            cx.notify();
            return;
        }
        let node = match identity.map(new_graph_node).transpose() {
            Ok(node) => node,
            Err(error) => {
                self.error = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        let stamp = self.begin_history_edit();
        self.revision += 1;
        self.status = None;
        self.error = self.edit_target(&target, node.as_ref()).err();
        self.finish_history_edit(stamp, self.error.is_none());
        self.rebuild(window, cx);
        self.start_preview(cx);
    }
    fn save(&mut self, save_as: bool, cx: &mut Context<Self>) {
        if self.embedded_noise.is_some() || self.embedded_environment.is_some() {
            return;
        }
        let format = if self.source_animation.is_some() {
            DocumentFormat::Gtx
        } else {
            DocumentFormat::Mycela
        };
        self.save_as_format(save_as, format, cx);
    }

    fn save_gtx_copy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        if self.has_field_drafts(cx) {
            self.error = Some("Correct or reset field text before saving.".into());
            cx.notify();
            return;
        }
        let checked = guard(&self.document).gtx_incompatibilities();
        let issues = match checked {
            Ok(issues) if issues.is_empty() => {
                self.save_as_format(true, DocumentFormat::Gtx, cx);
                return;
            }
            Ok(issues) => issues
                .into_iter()
                .map(|issue| issue.to_string())
                .collect::<Vec<_>>(),
            Err(error) => {
                self.error = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        self.modal = true;
        let owner = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let owner = owner.clone();
            dialog
                .title("Can’t save as Genetica GTX")
                .child("Save a Mycela project to keep these nodes and settings.")
                .child(
                    div()
                        .id("gtx-incompatibilities")
                        .v_flex()
                        .gap_2()
                        .max_h_64()
                        .overflow_y_scrollbar()
                        .children(issues.iter().cloned().map(|issue| div().child(issue))),
                )
                .footer(
                    DialogFooter::new()
                        .child(DialogClose::new().trigger(|button| button.label("Close"))),
                )
                .on_close(move |_, _, cx| {
                    let _ = owner.update(cx, |view, cx| {
                        view.modal = false;
                        cx.notify();
                    });
                })
        });
        cx.notify();
    }

    fn save_as_format(&mut self, save_as: bool, format: DocumentFormat, cx: &mut Context<Self>) {
        self.save_as_in_folder(save_as, format, None, cx);
    }

    fn other_open_document_paths(&self, cx: &Context<Self>) -> Vec<PathBuf> {
        self.workspace
            .as_ref()
            .and_then(|owner| owner.upgrade())
            .map(|workspace| workspace.read(cx).other_document_paths(cx.entity_id(), cx))
            .unwrap_or_default()
    }

    fn validate_save_destination(
        path: &Path,
        format: DocumentFormat,
        open_paths: &[PathBuf],
        source: Option<&Path>,
    ) -> Result<(), String> {
        format.validate_save_path(path)?;
        let canonical_target = std::fs::canonicalize(path).ok();
        if open_paths.iter().any(|open| {
            open == path
                || canonical_target.as_ref().is_some_and(|target| {
                    std::fs::canonicalize(open).is_ok_and(|open| open == *target)
                })
        }) {
            return Err("That file is already open in another document.".into());
        }
        if let Some(source) = source
            && std::fs::canonicalize(source)
                .ok()
                .zip(canonical_target)
                .is_some_and(|(source, target)| source == target)
        {
            return Err("Choose a different filename to preserve the imported GTX.".into());
        }
        Ok(())
    }

    fn open_save_path_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(suggested) = self.save_fallback_path.take() else {
            return;
        };
        if let Some(ticket) = self.ticket.take() {
            ticket.retire();
        }
        self.work = None;
        self.modal = true;
        let input =
            cx.new(|cx| InputState::new(window, cx).default_value(suggested.display().to_string()));
        let focus = gpui_kit::Focusable::focus_handle(input.read(cx), cx);
        let focused = Rc::new(Cell::new(false));
        let draft = cx.new(|_| SavePathDraft {
            input,
            replacement: None,
            error: None,
        });
        let owner = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            let close = owner.clone();
            let save = owner.clone();
            let editing = draft.clone();
            let draft = draft.read(cx);
            let focus = focus.clone();
            let focused = focused.clone();
            dialog
                .title("Save As")
                .child(
                    Field::new().label("File path").child(
                        div()
                            .relative()
                            .on_prepaint(move |_, window, cx| {
                                if !focused.replace(true) {
                                    window.defer(cx, move |window, cx| focus.focus(window, cx));
                                }
                            })
                            .child(
                                Input::new(&draft.input)
                                    .id("graph-save-manual-path")
                                    .aria_label("File path"),
                            ),
                    ),
                )
                .when_some(draft.error.clone(), |dialog, error| {
                    dialog.child(
                        div()
                            .id("graph-save-manual-error")
                            .role(gpui_kit::Role::Alert)
                            .text_color(cx.theme().danger)
                            .child(error),
                    )
                })
                .when(draft.replacement.is_some(), |dialog| {
                    dialog.child("That file already exists. Choose Replace file to confirm.")
                })
                .footer(
                    DialogFooter::new()
                        .child(DialogClose::new().trigger(|button| button.label("Cancel")))
                        .child(DialogAction::new().child(
                            Button::new("graph-save-manual-confirm").primary().label(
                                if draft.replacement.is_some() {
                                    "Replace file"
                                } else {
                                    "Save"
                                },
                            ),
                        )),
                )
                .on_ok(move |_, _, cx| {
                    let Some(path) = editing.update(cx, |draft, cx| draft.confirm(cx)) else {
                        return false;
                    };
                    let _ = save.update(cx, |view, cx| {
                        view.modal = false;
                        view.save_to_entered_path(path, cx);
                    });
                    true
                })
                .on_close(move |_, _, cx| {
                    let _ = close.update(cx, |view, cx| {
                        view.modal = false;
                        cx.notify();
                    });
                })
        });
        cx.notify();
    }

    fn save_to_entered_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let open_paths = self.other_open_document_paths(cx);
        let source = self.imported_from.clone();
        let document = self.document.clone();
        let revision = self.revision;
        let handle = self.window;
        let ticket = Ticket::new();
        let work = ticket.clone();
        self.ticket = Some(ticket.clone());
        self.work = Some(Work::Saving);
        self.error = None;
        let format = DocumentFormat::Mycela;
        let pending = cx.background_spawn(async move {
            Self::validate_save_destination(&path, format, &open_paths, source.as_deref())?;
            model::save(&document, path, format, &work)
        });
        cx.spawn(async move |owner, cx| {
            let result = pending.await;
            let _ = cx.update_window(handle, |_, _, cx| {
                owner.update(cx, |view, cx| {
                    view.saved(result, &ticket, revision, format, cx)
                })
            });
        })
        .detach();
        cx.notify();
    }

    fn save_in_recent_folder(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let window = self.window;
        cx.spawn(async move |owner, cx| {
            let missing = recent_paths::missing(&path, true, cx).await;
            let _ = cx.update_window(window, |_, _, cx| {
                owner.update(cx, |view, cx| {
                    if view.closed || view.is_busy() {
                        return;
                    }
                    let folder = if missing {
                        let _ = recent_paths::RecentPaths::forget_folder(&path, cx);
                        view.error =
                            Some(format!("This folder no longer exists: {}", path.display()));
                        dirs::document_dir()
                            .unwrap_or_else(|| path.parent().unwrap_or(&path).to_owned())
                    } else {
                        path
                    };
                    view.save_as_in_folder(true, DocumentFormat::Mycela, Some(folder), cx);
                })
            });
        })
        .detach();
    }

    fn save_as_in_folder(
        &mut self,
        save_as: bool,
        format: DocumentFormat,
        folder: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        if self.is_busy() {
            return;
        }
        if self.has_field_drafts(cx) {
            self.error = Some("Correct or reset field text before saving.".into());
            cx.notify();
            return;
        }
        // The main workspace can still edit this source while the utility's
        // file picker is open. Serialize the committed revision at activation.
        let source_bytes = if self.source_animation.is_some() {
            match model::serialize(&self.document, format) {
                Ok(writer) => {
                    self.error = None;
                    self.status = None;
                    Some(writer)
                }
                Err(error) => {
                    self.error = Some(error);
                    cx.notify();
                    return;
                }
            }
        } else {
            None
        };
        let ticket = Ticket::new();
        self.ticket = Some(ticket.clone());
        let document = self.document.clone();
        let revision = self.revision;
        let handle = self.window;
        let background = cx.background_executor().clone();
        let path = self.path.clone();
        let source = self.imported_from.clone();
        if !save_as
            && format == DocumentFormat::Mycela
            && source_bytes.is_none()
            && let Some(save_path) = path.as_ref()
        {
            self.work = Some(Work::Saving);
            let path = save_path.clone();
            cx.spawn(async move |owner, cx| {
                let work = ticket.clone();
                let result = background
                    .spawn(async move { model::save(&document, path, format, &work) })
                    .await;
                let _ = cx.update_window(handle, |_, _, cx| {
                    owner.update(cx, |view, cx| {
                        view.saved(result, &ticket, revision, format, cx)
                    })
                });
            })
            .detach();
        } else {
            self.work = Some(Work::ChoosingSave);
            let open_paths = self.other_open_document_paths(cx);
            let suggested = path.as_ref().or(source.as_ref());
            let directory = folder
                .or_else(|| {
                    suggested
                        .and_then(|path| path.parent())
                        .map(Path::to_path_buf)
                })
                .or_else(|| recent_paths::RecentPaths::folders(cx).first().cloned())
                .or_else(dirs::document_dir)
                .unwrap_or_else(|| PathBuf::from("."));
            let name = self.suggested_document_stem();
            let suffix = if format == DocumentFormat::Gtx && source.is_some() {
                "-copy"
            } else {
                ""
            };
            let name = format!("{name}{suffix}.{}", format.extension());
            self.save_fallback_path =
                (format == DocumentFormat::Mycela).then(|| directory.join(&name));
            let prompt = cx.prompt_for_new_path(&directory, Some(&name));
            cx.spawn(async move |owner, cx| {
                let selected = prompt.await;
                if !cx
                    .update_window(handle, |_, _, cx| {
                        owner.read_with(cx, |view, _| view.owns(&ticket))
                    })
                    .is_ok_and(|v| v.is_ok_and(|v| v))
                {
                    return;
                }
                if format == DocumentFormat::Mycela && matches!(selected, Ok(Err(_)) | Err(_)) {
                    let _ = cx.update_window(handle, |_, window, cx| {
                        owner.update(cx, |view, cx| view.open_save_path_dialog(window, cx))
                    });
                    return;
                }
                let result = match selected {
                    Ok(Ok(Some(mut path))) => {
                        let added_extension =
                            path.extension().is_none() && path.file_name().is_some();
                        if added_extension {
                            path.set_extension(format.extension());
                        }
                        let _ = owner.update(cx, |view, cx| {
                            view.work = Some(Work::Saving);
                            cx.notify();
                        });
                        let work = ticket.clone();
                        background
                            .spawn(async move {
                                Self::validate_save_destination(
                                    &path,
                                    format,
                                    &open_paths,
                                    source.as_deref(),
                                )?;
                                if added_extension && path.try_exists().map_err(model::failure)? {
                                    return Err(format!(
                                        "Choose the existing .{} file to confirm replacing it.",
                                        format.extension()
                                    ));
                                }
                                if let Some(writer) = source_bytes {
                                    model::save_bytes(writer.bytes(), path, &work)
                                } else {
                                    model::save(&document, path, format, &work)
                                }
                            })
                            .await
                    }
                    Ok(Ok(None)) => Ok(None),
                    Ok(Err(e)) => Err(format!("Couldn’t open the save dialog: {e}")),
                    Err(_) => Err("The save dialog closed before returning a result.".into()),
                };
                let _ = cx.update_window(handle, |_, _, cx| {
                    owner.update(cx, |view, cx| {
                        view.saved(result, &ticket, revision, format, cx)
                    })
                });
            })
            .detach();
        }
        cx.notify();
    }
    fn saved(
        &mut self,
        result: Result<Option<PathBuf>, String>,
        ticket: &Ticket,
        revision: u64,
        format: DocumentFormat,
        cx: &mut Context<Self>,
    ) {
        if !self.owns(ticket) {
            return;
        }
        self.work = None;
        self.ticket = None;
        self.save_fallback_path = None;
        match result {
            Ok(Some(path)) => {
                if self.workspace.is_some()
                    && let Err(error) =
                        recent_paths::RecentPaths::remember_document(&path, format, cx)
                {
                    eprintln!("Could not save recent Mycela paths: {error}");
                }
                if self.source_animation.is_none() && format == DocumentFormat::Gtx {
                    self.error = None;
                    self.status = Some("Genetica GTX copy saved".into());
                    cx.notify();
                    return;
                }
                self.path = Some(path);
                self.error = None;
                if self.source_animation.is_none() && self.revision == revision {
                    if let Some(history) = self.history()
                        && let Err(error) = history.preserve_savepoint()
                    {
                        self.error = Some(error.to_string());
                    } else {
                        self.dirty = false;
                    }
                }
                self.status = Some(if self.source_animation.is_some() {
                    "Solid Color source saved".into()
                } else {
                    "Saved".into()
                });
            }
            Ok(None) => {}
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }

    fn remember_export_folder(&self, path: &Path, cx: &mut Context<Self>) {
        if self.workspace.is_some()
            && let Some(folder) = path.parent()
            && let Err(error) = recent_paths::RecentPaths::remember_folder(folder, cx)
        {
            eprintln!("Could not save recent Mycela paths: {error}");
        }
    }
}
impl Drop for GraphDocumentView {
    fn drop(&mut self) {
        self.retire();
    }
}

fn new_graph_node(identity: &str) -> Result<Node, mycela_engine::NodeWireError<ForegroundError>> {
    create_node_by_identity(Some(&identity.encode_utf16().collect::<Vec<_>>()))
}

fn node_catalog(
    menu: PopupMenu,
    choose: impl Fn(&'static str, &mut Window, &mut App) + Clone + 'static,
) -> PopupMenu {
    node_catalog_choices(
        menu,
        STANDALONE_NODES.iter().chain(NODES_WITH_INPUTS),
        choose,
    )
}

fn node_catalog_choices(
    mut menu: PopupMenu,
    identities: impl IntoIterator<Item = &'static &'static str>,
    choose: impl Fn(&'static str, &mut Window, &mut App) + Clone + 'static,
) -> PopupMenu {
    for &identity in identities {
        let choose = choose.clone();
        menu = menu.item(PopupMenuItem::new(identity).on_click(move |_, window, cx| {
            choose(identity, window, cx);
        }));
    }
    menu.scrollable(true)
}

const STANDALONE_NODES: &[&str] = &[
    "Solid Color",
    "Oklab Color",
    "Checkerboard",
    "Gradient",
    "Noise",
    "Cells",
    "Wood Rings",
    "Bricks & Boards",
    "Imported Image",
    "Import 3D",
    "Cone",
    "Cube",
    "Cylinder",
    "Disc",
    "Plane",
    "Sphere",
    "Torus",
    "Reference",
    "Polygon",
    "Make Environment",
    "Wood Lab",
];

const NODES_WITH_INPUTS: &[&str] = &[
    "Shader",
    "Canvas",
    "Style",
    "Clear Lights",
    "Combine 3D",
    "Transform 3D",
    "UV Apply",
    "UV Ambient Occlusion",
    "UV Curvature",
    "UV Environment Light",
    "UV Environment Occlusion",
    "UV Light",
    "UV Multi Project",
    "UV Project",
    "Group",
    "Cut & Tile Lab",
    "Fiber Lab",
    "Layer Lab",
    "Noise Lab",
    "Select Noise",
    "Select Pattern",
    "Substance Lab",
    "Weather Lab",
    "Point Light",
    "Full Light",
    "Shadow",
    "Atmosphere",
    "Hair",
    "Convert Environment",
    "Rotate Environment",
    "Diffuse Environment",
    "Scatter Environment",
    "Reflect Environment",
    "Refract Environment",
    "Reflect",
    "Refract",
    "Stereogram",
    "Regions",
    "Caustics",
    "Transmit",
    "Invert",
    "Change BCI",
    "Change HSB",
    "Colorize",
    "Levels",
    "Get Channel",
    "Store Map",
    "Clear Maps",
    "Merge Maps",
    "Fix Map Contrast",
    "Layer Maps",
    "Tone Curve",
    "Combine",
    "A + B",
    "A * B",
    "From HSB",
    "Pixelate",
    "Matrix",
    "Rotate",
    "Gaussian Blur",
    "Sharpen",
    "Motion Blur",
    "Shape Blur",
    "Smart Blur",
    "Smear Blur",
    "Variable Blur",
    "Equalize",
    "Frequency",
    "Fractalize",
    "Exposure",
    "Selector",
    "Match Color",
    "Art Blur",
    "Distort",
    "Free Transform",
    "Kaleidoscope",
    "Move & Repeat",
    "Remap",
    "Rotate & Tile",
];
impl GraphDocumentView {
    fn render_checkerboard_colors(
        &self,
        colors: [mycela_engine::PointwiseParameter<ForegroundError>; 2],
        mut channels: [Vec<Field>; 2],
        busy: bool,
        cx: &Context<Self>,
    ) -> Field {
        let open = self.inspector.checkerboard_color_open;
        let theme = cx.theme();
        let buttons = colors.into_iter().enumerate().map(|(index, property)| {
            let (id, label) = [
                ("checkerboard-color-1", "Color 1"),
                ("checkerboard-color-2", "Color 2"),
            ][index];
            let swatch = property
                .effective_color_at(self.sample_time)
                .or_else(|_| property.color())
                .ok()
                .map(|color| {
                    let [r, g, b] = color.components().map(|value| value.clamp(0., 1.) as f32);
                    Rgba { r, g, b, a: 1. }.into()
                })
                .unwrap_or(theme.background);
            Button::new(id)
                .label(label)
                .icon(if open == Some(index) {
                    gpui_kit::component::IconName::ChevronUp
                } else {
                    gpui_kit::component::IconName::ChevronDown
                })
                .small()
                .flex_1()
                .selected(open == Some(index))
                .disabled(busy)
                .on_click(cx.listener(move |view, _, _, cx| {
                    let open = &mut view.inspector.checkerboard_color_open;
                    *open = (*open != Some(index)).then_some(index);
                    cx.notify();
                }))
                .child(
                    div()
                        .h_4()
                        .w_5()
                        .flex_shrink_0()
                        .rounded_sm()
                        .border_1()
                        .border_color(theme.border)
                        .bg(swatch),
                )
        });
        Field::new().child(
            div()
                .v_flex()
                .gap_2()
                .child(div().h_flex().gap_2().children(buttons))
                .when_some(open, |body, color| {
                    body.child(Form::new().children(std::mem::take(&mut channels[color])))
                }),
        )
    }
}

impl Render for GraphDocumentView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.is_busy();
        let owner = cx.weak_entity();
        let theme = cx.theme().clone();
        let name = self.document_name();
        let imported = self.path.is_none() && self.imported_from.is_some();
        let source_property = self.source_animation;
        let source_animation = source_property.is_some();
        let embedded_noise = self.embedded_noise.is_some();
        let embedded_environment = self.embedded_environment.is_some();
        let property_editor = embedded_noise || embedded_environment;
        let graph_tools = !source_animation && !property_editor;
        let save_location = self
            .path
            .as_ref()
            .filter(|_| graph_tools)
            .map(|path| path.display().to_string());
        let results_visible = graph_tools && self.results.is_visible();
        let title = if embedded_environment {
            "Edit environment map".into()
        } else if embedded_noise {
            "Noise Lab".into()
        } else if source_animation {
            "Solid Color animation".into()
        } else {
            let source = if imported { " · Imported GTX" } else { "" };
            let changed = if self.is_dirty() { " · Modified" } else { "" };
            format!("{name}{source}{changed}")
        };
        let history_draft = self.has_field_drafts(cx);
        let can_undo = !busy && !history_draft && self.history_available(HistoryDirection::Undo);
        let can_redo = !busy && !history_draft && self.history_available(HistoryDirection::Redo);
        let menus = self.document_menus(busy, results_visible, can_undo, can_redo, cx);
        let toolbar = div()
            .h_flex()
            .min_w_0()
            .flex_wrap()
            .gap_2()
            .when_else(graph_tools, |row| row.px_3(), |row| row.p_3())
            .when(graph_tools && self.workspace.is_some(), |toolbar| {
                toolbar.child(self.render_workspace_tabs(busy, cx))
            })
            .child(
                div()
                    .v_flex()
                    .flex_1()
                    .min_w_0()
                    .when_else(
                        graph_tools && self.workspace.is_some(),
                        |heading| {
                            heading.child(
                                div().h_flex().min_w_0().child(
                                    Button::new("graph-document-name")
                                        .ghost()
                                        .xsmall()
                                        .px_0()
                                        .max_w_full()
                                        .tab_index(0)
                                        .disabled(busy)
                                        .accessibility_label(title.clone())
                                        .tooltip("Double-click to rename document")
                                        .child(
                                            div().truncate().font_semibold().child(title.clone()),
                                        )
                                        .on_click(cx.listener(
                                            |view, event: &gpui_kit::ClickEvent, window, cx| {
                                                if event.click_count() == 2 || event.is_keyboard() {
                                                    view.open_rename_document(window, cx);
                                                }
                                            },
                                        )),
                                ),
                            )
                        },
                        |heading| {
                            heading.child(div().truncate().font_semibold().child(title.clone()))
                        },
                    )
                    .when_some(save_location, |header, path| {
                        let tooltip = path.clone();
                        header.child(
                            div()
                                .id("graph-save-location")
                                .test_support()
                                .truncate()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .tooltip(move |window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(tooltip.clone())
                                        .build(window, cx)
                                })
                                .child(path),
                        )
                    }),
            )
            .when(
                graph_tools && !results_visible && !self.overview_open,
                |toolbar| {
                    toolbar
                        .child(
                            Button::new("graph-view-canvas")
                                .small()
                                .label("Graph")
                                .selected(self.canvas.visible)
                                .disabled(busy)
                                .on_click(cx.listener(|view, _, window, cx| {
                                    view.canvas.visible = true;
                                    if let Some(key) = view.selection.as_deref()
                                        && let Some(target) = view.navigation.target(key)
                                    {
                                        view.canvas.follow_selection(
                                            &view.navigation,
                                            key,
                                            &target,
                                        );
                                    }
                                    view.canvas_center(window);
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("graph-view-list")
                                .small()
                                .label("List")
                                .selected(!self.canvas.visible)
                                .disabled(busy)
                                .on_click(cx.listener(|view, _, _, cx| {
                                    view.canvas.visible = false;
                                    cx.notify();
                                })),
                        )
                },
            )
            .when(graph_tools, |toolbar| {
                toolbar
                    .child(
                        Button::new("graph-save")
                            .small()
                            .ghost()
                            .icon(gpui_kit::assets::IconName::Save)
                            .tooltip("Save project")
                            .accessibility_label("Save project")
                            .disabled(busy)
                            .on_click(cx.listener(|view, _, _, cx| view.save(false, cx))),
                    )
                    .child(
                        Button::new("graph-save-as")
                            .small()
                            .ghost()
                            .label("Save As…")
                            .disabled(busy)
                            .on_click(cx.listener(|view, _, _, cx| view.save(true, cx))),
                    )
            })
            .when(
                self.work == Some(Work::ChoosingSave) && self.save_fallback_path.is_some(),
                |toolbar| {
                    toolbar.child(
                        Button::new("graph-save-enter-path")
                            .small()
                            .label("Enter path…")
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.open_save_path_dialog(window, cx);
                            })),
                    )
                },
            )
            .when(self.history().is_some(), |toolbar| {
                toolbar
                    .child(
                        Button::new("graph-undo")
                            .small()
                            .ghost()
                            .icon(gpui_kit::assets::IconName::Undo2)
                            .tooltip("Undo")
                            .accessibility_label("Undo")
                            .disabled(!can_undo)
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.travel_history(HistoryDirection::Undo, window, cx)
                            })),
                    )
                    .child(
                        Button::new("graph-redo")
                            .small()
                            .ghost()
                            .icon(gpui_kit::assets::IconName::Redo2)
                            .tooltip("Redo")
                            .accessibility_label("Redo")
                            .disabled(!can_redo)
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.travel_history(HistoryDirection::Redo, window, cx)
                            })),
                    )
            })
            .when(graph_tools && !results_visible, |toolbar| {
                toolbar.child(
                    Button::new("graph-export-image")
                        .small()
                        .ghost()
                        .icon(gpui_kit::assets::IconName::Download)
                        .tooltip("Export preview image")
                        .accessibility_label("Export preview image")
                        .disabled(busy || self.preview_target().is_none())
                        .on_click(
                            cx.listener(|view, _, window, cx| view.open_export_image(window, cx)),
                        ),
                )
            })
            .when(graph_tools, |toolbar| {
                toolbar.child(self.render_results_navigation(busy, cx))
            })
            .when(embedded_noise, |toolbar| {
                toolbar
                    .child(
                        Button::new("noise-lab-cancel")
                            .small()
                            .label("Cancel")
                            .disabled(busy)
                            .on_click(|_, window, _| window.remove_window()),
                    )
                    .child(
                        Button::new("noise-lab-apply")
                            .small()
                            .primary()
                            .label("Apply to Style")
                            .disabled(busy)
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.apply_embedded_noise(window, cx)
                            })),
                    )
            })
            .when(embedded_environment, |toolbar| {
                toolbar
                    .child(
                        Button::new("environment-editor-cancel")
                            .small()
                            .label("Cancel")
                            .disabled(busy)
                            .on_click(|_, window, _| window.remove_window()),
                    )
                    .child(
                        Button::new("environment-editor-apply")
                            .small()
                            .primary()
                            .label("Apply map")
                            .disabled(busy)
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.apply_embedded_environment(window, cx)
                            })),
                    )
            })
            .when(source_animation, |toolbar| {
                toolbar
                    .child(
                        Button::new("solid-animation-save-as")
                            .small()
                            .label("Save source as GTX…")
                            .disabled(busy)
                            .on_click(cx.listener(|view, _, _, cx| view.save(true, cx))),
                    )
                    .child(
                        Button::new("solid-animation-close")
                            .small()
                            .label("Close")
                            .disabled(busy)
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.request(Intent::Close, window, cx)
                            })),
                    )
            })
            .when(self.work.is_some_and(Work::can_cancel_export), |toolbar| {
                toolbar.child(
                    Button::new("graph-cancel-export")
                        .small()
                        .label("Cancel export")
                        .on_click(cx.listener(|view, _, _, cx| view.cancel_export(cx))),
                )
            });
        let navigation = div()
            .v_flex()
            .size_full()
            .min_h_0()
            .child(
                div()
                    .h_flex()
                    .p_3()
                    .gap_2()
                    .child(div().flex_1().font_semibold().child("Graph"))
                    .child(
                        Button::new("graph-focus-tree")
                            .small()
                            .ghost()
                            .label("Select node")
                            .disabled(busy)
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.canvas.visible = false;
                                view.tree.update(cx, |tree, cx| tree.focus(window, cx));
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div().flex_1().min_h_0().child(
                    Tree::new(&self.tree, |_, entry, _, _, _| {
                        ListItem::new(entry.item().id.clone())
                            .accessibility_label(entry.item().label.clone())
                            .child(div().truncate().child(entry.item().label.clone()))
                    })
                    .context_menu({
                        let owner = owner.clone();
                        move |_, entry, menu, window, cx| {
                            Self::build_node_commands(
                                menu,
                                &owner,
                                entry.item().id.as_ref(),
                                window,
                                cx,
                            )
                        }
                    }),
                ),
            );
        let mut form = Form::new();
        if let Some(levels) = &self.inspector.levels {
            form = form.child(Field::new().child(levels.clone()));
        }
        if let Some(gradient) = &self.inspector.legacy_gradient {
            form = form.child(Field::new().child(self.render_legacy_gradient(gradient, busy, cx)));
        }
        if let Some(editor) = &self.inspector.selector_color {
            form = form.child(
                Field::new().label("Color").child(
                    div()
                        .h_flex()
                        .flex_wrap()
                        .gap_2()
                        .child(self.render_selector_color(editor, busy, cx))
                        .child(self.render_selector_color_adjuster(editor, busy, cx)),
                ),
            );
        }
        if let Some(gradient) = &self.inspector.gradient
            && gradient.environment().is_some()
        {
            form = form.child(Field::new().child(self.render_graph_gradient(gradient, busy, cx)));
        }
        if let Some(gradient) = &self.inspector.mask_gradient {
            form = form.child(Field::new().child(self.render_graph_gradient(gradient, busy, cx)));
        }
        let checkerboard_colors = self
            .inspector
            .owner
            .as_ref()
            .and_then(Node::filter)
            .filter(|filter| filter.kind() == mycela_engine::PointwiseKind::Checkerboard)
            .and_then(|filter| Some([filter.parameter(0).ok()?, filter.parameter(1).ok()?]));
        let mut checkerboard_channels = [Vec::new(), Vec::new()];
        let history = self.history();
        for draft in self.inspector.drafts.iter().filter(|draft| {
            (!source_animation || !draft.is_stored_solid_value())
                && !draft.is_animation()
                && !draft.is_binding()
                && !draft.is_animation_palette()
                && draft.canvas_area_field().is_none()
                && draft.canvas_shape_field().is_none()
        }) {
            let controls = (!source_animation)
                .then(|| draft.numeric_lab_property())
                .flatten()
                .map(|(property, axis)| {
                    self.numeric_link_controls(property, axis, &draft.id, busy, cx)
                });
            let environment = draft.environment_numeric_receiver();
            let environment_menu = environment.and_then(|(panel, receiver)| {
                self.render_environment_numeric_adjuster(
                    panel,
                    receiver,
                    format!("{}:lab-link", draft.id),
                    busy,
                    cx,
                )
            });
            let linked = controls.as_ref().is_some_and(|controls| controls.linked)
                || environment.is_some_and(|(panel, receiver)| {
                    panel.value().numeric_adjuster_key(receiver).is_some()
                });
            let input = draft
                .control(
                    busy || linked
                        || draft.is_layer_controlled()
                        || draft.is_controlled_selector_color(),
                )
                .history(history.clone());
            let checkerboard_channel =
                draft.filter_color_component().and_then(|(property, axis)| {
                    let index = checkerboard_colors
                        .as_ref()?
                        .iter()
                        .position(|color| color.same_reference(property))?;
                    Some((index, axis))
                });
            let field = Field::new()
                .label(if let Some((_, axis)) = checkerboard_channel {
                    ["Red", "Green", "Blue"][axis].to_owned()
                } else if draft.is_stored_solid_value() {
                    format!("{} (0–255)", draft.label)
                } else {
                    draft.label.clone()
                })
                .child(
                    div()
                        .v_flex()
                        .gap_1()
                        .child(input)
                        .when_some(controls, |row, controls| row.child(controls.buttons))
                        .when_some(environment_menu, |row, menu| row.child(menu)),
                )
                .when_some(draft.live_error.clone(), |field, error| {
                    field.description_fn(move |_, cx| {
                        div()
                            .text_sm()
                            .text_color(cx.theme().danger)
                            .child(error.clone())
                    })
                });
            if let Some((color, _)) = checkerboard_channel {
                checkerboard_channels[color].push(field);
            } else {
                form = form.child(field);
            }
        }
        if let Some(colors) = checkerboard_colors {
            form = form.child(self.render_checkerboard_colors(
                colors,
                checkerboard_channels,
                busy,
                cx,
            ));
        }
        if let Some(gradient) = &self.inspector.gradient
            && !gradient.is_animation_palette()
            && gradient.environment().is_none()
        {
            form = form.child(Field::new().child(self.render_graph_gradient(gradient, busy, cx)));
        }
        if let Some(layer) = &self.inspector.layer {
            form = form.child(
                Field::new().child(
                    Button::new("graph-close-layer")
                        .label(format!("Close {:?} layer", layer.layer.kind()))
                        .disabled(busy || self.has_field_drafts(cx))
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.curve_plot_drag = None;
                            view.inspector.close_layer();
                            view.observe_drafts(cx);
                            cx.notify();
                        })),
                ),
            );
            let selection = layer.clone();
            let target_item = self.inspector.animations.iter().find(|item| {
                item.axis == layer.axis
                    && item
                        .curve
                        .as_ref()
                        .is_some_and(|curve| curve.same_reference(&layer.curve))
            });
            if let Some(item) = target_item {
                let target = animation::AnimationTarget {
                    selection: self.selection.clone(),
                    node: self.inspector.owner.clone(),
                    revision: self.revision,
                    item: item.clone(),
                };
                form = form.child(
                    Field::new().child(self.render_graph_modulation(layer, &target, busy, cx)),
                );
                if let Some(custom) = &self.inspector.custom {
                    form = form.child(
                        Field::new()
                            .child(self.render_graph_custom(layer, custom, &target, busy, cx)),
                    );
                }
                for control in layer.controls() {
                    let (label, expected) = match control.kind() {
                        mycela_engine::CurveControlKind::Toggle => (
                            control
                                .name()
                                .as_deref()
                                .map(String::from_utf16_lossy)
                                .unwrap_or_else(|| "Option".into()),
                            control.toggle().ok().map(i32::from),
                        ),
                        mycela_engine::CurveControlKind::Direction => {
                            ("Reverse direction".into(), control.enumeration().ok())
                        }
                        _ => continue,
                    };
                    let Some(expected) = expected else {
                        continue;
                    };
                    let id = control.id();
                    let selection = selection.clone();
                    let target = target.clone();
                    form = form.child(
                        Field::new().child(
                            Checkbox::new(format!("graph-layer-toggle-{id}"))
                                .label(label)
                                .checked(expected != 0)
                                .disabled(busy)
                                .on_change(cx.listener(move |view, value, window, cx| {
                                    view.toggle_graph_layer_control(
                                        &selection,
                                        &control,
                                        (expected, i32::from(*value)),
                                        &target,
                                        window,
                                        cx,
                                    )
                                })),
                        ),
                    );
                }
            }
        }
        for choice in self
            .inspector
            .choices
            .iter()
            .filter(|_| !embedded_environment)
        {
            let channel_animated = choice.binding.is_animated_channel();
            let controls = (!source_animation)
                .then(|| choice.binding.numeric_lab_property())
                .flatten()
                .map(|property| {
                    let axis = mycela_engine::LabNumericAxis::Whole;
                    let link_blocked = channel_animated
                        && guard(&self.document)
                            .numeric_property_link_name(&property, axis)
                            .is_none();
                    self.numeric_link_controls(property, axis, &choice.id, busy || link_blocked, cx)
                });
            let linked = controls.as_ref().is_some_and(|controls| controls.linked);
            let options = choice.binding.options();
            let index = choice.binding.index();
            let label = match (&options, &index) {
                (Ok(options), Ok(index)) => options
                    .entries
                    .get(*index as usize)
                    .and_then(|s| s.as_deref())
                    .map(String::from_utf16_lossy)
                    .unwrap_or_else(|| "Invalid choice".into()),
                _ => "Unavailable".into(),
            };
            let owner = owner.clone();
            let key = choice.id.clone();
            let expected_options = options.ok();
            let entries = expected_options
                .as_ref()
                .map(|v| v.entries.clone())
                .unwrap_or_default();
            let selected = index.ok();
            let selection = self.selection.clone();
            let node = self.inspector.owner.clone();
            let revision = self.revision;
            form = form.child(
                Field::new().label(choice.label.clone()).child(
                    div()
                        .v_flex()
                        .gap_1()
                        .child(
                            Button::new(choice.id.clone())
                                .label(label)
                                .disabled(busy || linked || channel_animated)
                                .dropdown_menu(move |mut menu, _, _| {
                                    for (ix, text) in entries.iter().enumerate() {
                                        let owner = owner.clone();
                                        let key = key.clone();
                                        let selection = selection.clone();
                                        let node = node.clone();
                                        let expected_options = expected_options.clone();
                                        let label = text
                                            .as_deref()
                                            .map(String::from_utf16_lossy)
                                            .unwrap_or_else(|| "Unnamed option".into());
                                        menu = menu.item(
                                            PopupMenuItem::new(label)
                                                .checked(selected == Some(ix as i32))
                                                .on_click(move |_, window, cx| {
                                                    let _ = owner.update(cx, |view, cx| {
                                                        let options_unchanged = view
                                                            .inspector
                                                            .choices
                                                            .iter()
                                                            .find(|choice| choice.id == key)
                                                            .and_then(|choice| {
                                                                choice.binding.options().ok()
                                                            })
                                                            .is_some_and(|current| {
                                                                expected_options
                                                                    .as_ref()
                                                                    .is_some_and(|expected| {
                                                                        current
                                                                            .same_snapshot(expected)
                                                                    })
                                                            });
                                                        if options_unchanged
                                                            && view.is_original_selection(
                                                                &selection, &node, revision,
                                                            )
                                                        {
                                                            view.choose(&key, ix as i32, window, cx)
                                                        }
                                                    });
                                                }),
                                        );
                                    }
                                    menu.scrollable(true)
                                }),
                        )
                        .when_some(controls, |row, controls| row.child(controls.buttons)),
                ),
            );
        }
        let mut animation_content = div().v_flex().gap_3().child(
            div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child("New animations start with a linear layer."),
        );
        for animation in self.inspector.animations.iter().filter(|item| {
            source_property.is_none_or(|property| {
                item.parameter.is_solid_color_animation() == (property == SolidColorProperty::Color)
            })
        }) {
            let item = animation.clone();
            let target = animation::AnimationTarget {
                selection: self.selection.clone(),
                node: self.inspector.owner.clone(),
                revision: self.revision,
                item: item.clone(),
            };
            let layer_target = target.clone();
            let id = item.id.clone();
            let link_conflict = item.parameter.animation_link_conflict();
            let mut group = div().v_flex().gap_2();
            if item.parameter.is_direct_curve() {
                group = group.child(div().font_semibold().child(item.label.clone()));
            } else {
                group = group.child(
                    Checkbox::new(id.clone())
                        .label(format!("Animate {}", item.label))
                        .checked(item.enabled)
                        .disabled(busy || link_conflict.is_some())
                        .on_change(cx.listener(move |view, enabled, window, cx| {
                            view.toggle_graph_animation(&target, *enabled, window, cx)
                        })),
                );
                if let Some(reason) = link_conflict {
                    group = group.child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(reason),
                    );
                }
            }
            if animation.enabled && link_conflict.is_none() {
                let mut fields = Form::new().columns(2);
                for side in 0..2 {
                    let id = format!("{}:{side}", animation.id);
                    if let Some(draft) = self.inspector.drafts.iter().find(|draft| draft.id == id) {
                        fields = fields.child(
                            Field::new()
                                .label(match (item.parameter.has_animation_palette(), side) {
                                    (true, 0) => "Start position (%)",
                                    (true, _) => "End position (%)",
                                    (false, 0) => "Start output",
                                    (false, _) => "End output",
                                })
                                .child(draft.control(busy)),
                        );
                    }
                }
                group = group.child(fields);
                if let Some(curve) = animation.curve.as_ref() {
                    if self.animation_open && item.parameter.is_direct_curve() {
                        group = group.children(self.render_curve_adjuster(&layer_target, busy, cx));
                        match crate::tone_curve::samples(curve) {
                            Ok(values) => {
                                group = group.child(
                                    div()
                                        .id("graph-tone-curve-preview")
                                        .test_support()
                                        .role(gpui_kit::Role::Image)
                                        .aria_label("Tone Curve input and output")
                                        .h(rems(6.))
                                        .border_1()
                                        .border_color(theme.border)
                                        .p_1()
                                        .child(crate::tone_curve::graph(values, cx)),
                                );
                            }
                            Err(error) => {
                                group = group.child(
                                    div()
                                        .text_sm()
                                        .text_color(theme.muted_foreground)
                                        .child(error),
                                );
                            }
                        }
                    }
                    group = group.child(self.render_curve_presets(
                        layer_target.clone(),
                        curve,
                        busy || self.has_field_drafts(cx),
                        cx,
                    ));
                    group = group.child(self.render_graph_layers(layer_target, curve, busy, cx));
                }
                if item.parameter.has_animation_palette()
                    && let Some(palette) = &self.inspector.gradient
                {
                    let mut stops = Form::new().columns(2);
                    for draft in self
                        .inspector
                        .drafts
                        .iter()
                        .filter(|draft| draft.is_animation_palette())
                    {
                        stops = stops.child(
                            Field::new()
                                .label(draft.label.clone())
                                .child(draft.control(busy)),
                        );
                    }
                    group = group
                        .child(self.render_graph_gradient(palette, busy, cx))
                        .child(stops);
                }
            }
            animation_content = animation_content.child(group);
        }
        let animation_section = if self.inspector.animations.is_empty() {
            None
        } else if source_animation {
            Some(animation_content.into_any_element())
        } else {
            Some(
                Collapsible::new()
                    .open(self.animation_open)
                    .child(
                        Button::new("graph-toggle-animation")
                            .label(if self.animation_open {
                                "Hide curves and animation"
                            } else {
                                "Show curves and animation"
                            })
                            .disabled(busy)
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.animation_open = !view.animation_open;
                                cx.notify();
                            })),
                    )
                    .content(animation_content)
                    .into_any_element(),
            )
        };
        let binding_drafts: Vec<_> = self
            .inspector
            .drafts
            .iter()
            .filter(|draft| draft.is_binding())
            .collect();
        let binding_section = if binding_drafts.is_empty() {
            None
        } else {
            let mut fields = Form::new();
            for draft in binding_drafts {
                fields = fields.child(
                    Field::new()
                        .label(draft.label.clone())
                        .child(draft.control(busy)),
                );
            }
            Some(Collapsible::new().open(self.bindings_open)
                .child(Button::new("graph-toggle-bindings")
                    .label(if self.bindings_open { "Hide variable bindings" } else { "Show variable bindings" })
                    .disabled(busy).on_click(cx.listener(|view, _, _, cx| { view.bindings_open = !view.bindings_open; cx.notify(); })))
                .content(div().v_flex().gap_2()
                    .child(div().text_sm().text_color(theme.muted_foreground).child("Use -1 for no binding. Animated variables accept mappings such as 0>0;100>1."))
                    .child(fields)))
        };
        let label = self
            .selection
            .as_deref()
            .and_then(|key| self.navigation.label(key))
            .unwrap_or("Select a branch or input")
            .to_string();
        let lab_material_editor = self.lab_material_editor_open();
        let inspector = div()
            .v_flex()
            .size_full()
            .min_h_0()
            .child(
                div()
                    .v_flex()
                    .gap_2()
                    .p_3()
                    .border_b_1()
                    .border_color(theme.border)
                    .children(self.render_lab_material_heading())
                    .map(|heading| {
                        if lab_material_editor {
                            return heading;
                        }
                        heading
                            .when_some(source_property, |heading, property| {
                                heading.child(
                                    TabBar::new("solid-animation-properties")
                                        .selected_index(usize::from(
                                            property == SolidColorProperty::Transparency,
                                        ))
                                        .child(
                                            Tab::new()
                                                .label("Color")
                                                .disabled(busy || history_draft),
                                        )
                                        .child(
                                            Tab::new()
                                                .label("Transparency")
                                                .disabled(busy || history_draft),
                                        )
                                        .on_click(cx.listener(|view, index, _, cx| {
                                            if view.is_busy() || view.has_field_drafts(cx) {
                                                return;
                                            }
                                            let next = Some(if *index == 0 {
                                                SolidColorProperty::Color
                                            } else {
                                                SolidColorProperty::Transparency
                                            });
                                            if view.source_animation == next {
                                                return;
                                            }
                                            view.source_animation = next;
                                            view.curve_plot_drag = None;
                                            view.inspector.close_layer();
                                            view.observe_drafts(cx);
                                            cx.notify();
                                        })),
                                )
                            })
                            .when(embedded_noise, |heading| {
                                heading.child(div().font_semibold().child("Lab controls"))
                            })
                            .when(graph_tools, |heading| {
                                heading
                                    .child(div().font_semibold().child("Properties"))
                                    .child(div().text_sm().child(label))
                            })
                    }),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id(if lab_material_editor {
                                "graph-lab-material-scroll"
                            } else {
                                "graph-inspector-scroll"
                            })
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.inspector_scroll)
                            .lock_scroll_axis()
                            .test_support()
                            .child(div().p_4().v_flex().gap_3().map(|content| {
                                if lab_material_editor {
                                    return content
                                        .children(self.render_synthesis_panel(busy, window, cx));
                                }
                                content
                                    .when_some(
                                        self.render_mesh_import(busy, cx),
                                        |content, picker| content.child(picker),
                                    )
                                    .when_some(
                                        self.canvas_shape_picker(false, busy, cx),
                                        |content, shape| {
                                            content.child(
                                                div()
                                                    .h_flex()
                                                    .flex_wrap()
                                                    .gap_2()
                                                    .child(shape)
                                                    .children(
                                                        self.canvas_shape_picker(true, busy, cx),
                                                    ),
                                            )
                                        },
                                    )
                                    .when(
                                        self.selected_node().is_some_and(|node| {
                                            node.kind()
                                                == Some(mycela_engine::PointwiseKind::WizardGroup)
                                        }),
                                        |content| {
                                            content.child(
                                                Button::new("graph-add-environment-variable")
                                                    .label("Add Environment variable")
                                                    .small()
                                                    .disabled(busy)
                                                    .on_click(cx.listener(
                                                        |view, _, window, cx| {
                                                            view.add_wizard_environment_variable(
                                                                window, cx,
                                                            )
                                                        },
                                                    )),
                                            )
                                        },
                                    )
                                    .when(!source_animation && !embedded_environment, |content| {
                                        content.child(
                                            div()
                                                .h_flex()
                                                .flex_wrap()
                                                .gap_2()
                                                .children(self.lab_adjuster_menu(busy, cx))
                                                .child(
                                                    Button::new("graph-lab-presets")
                                                        .small()
                                                        .ghost()
                                                        .label("Lab presets…")
                                                        .disabled(busy)
                                                        .on_click(cx.listener(
                                                            |view, _, window, cx| {
                                                                view.open_lab_presets(window, cx);
                                                            },
                                                        )),
                                                ),
                                        )
                                    })
                                    .when_some(
                                        self.inspector
                                            .environment
                                            .as_ref()
                                            .filter(|panel| !panel.is_style()),
                                        |content, panel| {
                                            content.child(
                                                self.render_environment_controls(panel, busy, cx),
                                            )
                                        },
                                    )
                                    .children(self.render_shader_controls(busy, cx))
                                    .children(self.render_region_picker(busy, cx))
                                    .children(self.render_canvas_layer_controls(busy, cx))
                                    .children(self.render_style_panel(busy, cx))
                                    .when_some(
                                        self.inspector
                                            .environment
                                            .as_ref()
                                            .filter(|panel| panel.is_style()),
                                        |content, panel| {
                                            let style_pending = self
                                                .style
                                                .as_ref()
                                                .is_some_and(|panel| panel.pending(cx));
                                            content.child(self.render_environment_controls(
                                                panel,
                                                busy || style_pending,
                                                cx,
                                            ))
                                        },
                                    )
                                    .children(self.render_synthesis_panel(busy, window, cx))
                                    .child(form)
                                    .children(self.render_canvas_shape_panel(busy, cx))
                                    .when(
                                        !self
                                            .style
                                            .as_ref()
                                            .is_some_and(style::Panel::canvas_expanded),
                                        |content| {
                                            content.children(self.canvas_area_panel(busy, cx))
                                        },
                                    )
                                    .when_some(binding_section, |this, section| this.child(section))
                                    .when_some(animation_section, |this, section| {
                                        this.child(section)
                                    })
                                    .children(self.inspector.notes.iter().map(|note| {
                                        div()
                                            .text_sm()
                                            .text_color(theme.muted_foreground)
                                            .child(note.clone())
                                    }))
                            })),
                    )
                    .vertical_scrollbar(&self.inspector_scroll),
            )
            .when(
                (self.error.is_some() && self.has_field_drafts(cx))
                    || self
                        .inspector
                        .drafts
                        .iter()
                        .any(|draft| draft.live_error.is_some()),
                |panel| {
                    panel.child(
                        div()
                            .h_flex()
                            .gap_2()
                            .p_3()
                            .border_t_1()
                            .border_color(theme.border)
                            .child(
                                Button::new("graph-revert")
                                    .label("Reset field text")
                                    .disabled(busy)
                                    .on_click(cx.listener(|view, _, window, cx| {
                                        view.revert_fields(window, cx);
                                    })),
                            ),
                    )
                },
            )
            .children(self.render_shared_material_footer(busy, cx));
        let preview_status = if self.draft_preview_pending() {
            "Rendering…".to_owned()
        } else if let Some(status) = self.environment_converter_status(cx) {
            status
        } else if let Some(status) = self.synthesis_preview_status() {
            status
        } else if self.animated_preview_pending {
            "Rendering frames…".to_owned()
        } else if self.preview_pending {
            "Rendering…".to_owned()
        } else if self.preview_error.is_some() && self.preview.is_some() {
            "Last successful preview".to_owned()
        } else if let Some((index, count)) = self.preview_frame {
            format!("Frame {} of {count}", index + 1)
        } else if self.preview_lab_section {
            format!(
                "{LIVE_PREVIEW_SIZE} × {LIVE_PREVIEW_SIZE} · {}",
                self.lab_sections.selected_label().unwrap_or("Lab section")
            )
        } else if embedded_environment {
            format!("{LIVE_PREVIEW_SIZE} × {LIVE_PREVIEW_SIZE} · working map")
        } else if self.environment_preview.is_some() {
            format!("{LIVE_PREVIEW_SIZE} × {LIVE_PREVIEW_SIZE} · Environment draft")
        } else if self.preview_uses_texture_output() {
            format!("{LIVE_PREVIEW_SIZE} × {LIVE_PREVIEW_SIZE} · Texture output")
        } else if self.synthesis_whole_is_selection() {
            format!("{LIVE_PREVIEW_SIZE} × {LIVE_PREVIEW_SIZE} · Whole texture")
        } else {
            format!("{LIVE_PREVIEW_SIZE} × {LIVE_PREVIEW_SIZE} · selected node")
        };
        let time_disabled = busy || self.preview_animation_active();

        let (displayed, aspect) = self
            .environment_converter_raster(cx)
            .map(|(raster, aspect)| (Some(raster), aspect))
            .unwrap_or_else(|| {
                (
                    self.levels_draft_preview(cx)
                        .or_else(|| self.visible_preview())
                        .map(|preview| preview.raster.clone()),
                    1.,
                )
            });
        let synthesis_controls = self.synthesis_preview_context().is_some();
        let standalone_texture =
            self.synthesis_preview_context() == Some(synthesis::TextureContext::Standalone);
        let preview_status_line = || {
            div()
                .id("graph-preview-status")
                .test_support()
                .aria_label(preview_status.clone())
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(preview_status.clone())
        };
        let preview = div()
            .id("graph-preview-panel")
            .test_support()
            .v_flex()
            .size_full()
            .min_h_0()
            .p_3()
            .gap_2()
            .child(
                div()
                    .h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(div().font_semibold().child(if self.drawing.is_some() {
                        "Canvas tile"
                    } else {
                        "Preview"
                    }))
                    .when(!synthesis_controls, |row| row.child(preview_status_line()))
                    .children(self.environment_converter_picker(cx))
                    .children(self.render_preview_display_controls(busy, cx))
                    .children(self.render_preview_map_selector(busy, cx))
                    .when(self.synthesis_preview_mode().is_none(), |row| {
                        row.children(self.render_drawing_tools(busy, cx))
                    })
                    .when(
                        !source_animation && self.lab_sections.editing_nested() && !lab_material_editor,
                        |row| {
                            row.child(
                                Button::new("graph-preview-lab-section")
                                    .small()
                                    .label(if self.preview_lab_section {
                                        if self.preview_display.pins_texture_output() {
                                            "Show texture output"
                                        } else {
                                            "Show whole branch"
                                        }
                                    } else {
                                        "Render Lab section"
                                    })
                                    .disabled(busy)
                                    .on_click(cx.listener(|view, _, _, cx| {
                                        view.set_section_preview(!view.preview_lab_section, cx);
                                    })),
                            )
                        },
                    ),
            )
            .when(synthesis_controls, |preview| preview.child(preview_status_line()))
            .when_some(self.drawing.as_ref(), |this, tool| {
                this.child(div().text_sm().text_color(theme.muted_foreground)
                    .child(if tool.is_selecting() {
                        "Click to select or drag to move. Alt snaps to grid."
                    } else {
                        "Drag to draw. Shift locks proportions · Ctrl draws from center · Alt snaps to grid."
                    }))
            })
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(crate::preview_background::checkerboard_for(aspect))
                    .children(self.render_design_plan_image(false))
                    .when_some(displayed, |this, raster| {
                        this.child(self.render_preview_raster(raster, aspect))
                    })
                    .children(self.render_design_plan_image(true))
                    .children(self.render_drawing_surface(cx)),
            )
            .children(self.render_design_plan_status(cx))
            .children(self.render_environment_layer_preview())
            .when_some(self.environment_converter_error(cx), |this, error| {
                this.child(
                    div()
                        .id("environment-preview-converter-error")
                        .role(gpui_kit::Role::Alert)
                        .test_support()
                        .aria_label(error.clone())
                        .text_sm()
                        .text_color(theme.danger)
                        .child(error),
                )
            })
            .when_some(self.preview_error(), |this, error| {
                this.child(
                    div()
                        .id("graph-preview-error")
                        .test_support()
                        .text_sm()
                        .text_color(theme.danger)
                        .child(error.to_owned()),
                )
            })
            .when(self.animated_preview_available && self.drawing.is_none() && self.synthesis_preview_mode().is_none(), |this| {
                this.child(self.render_playback_controls(busy, cx))
            })
            .when(!property_editor, |preview| preview.child(
                div()
                    .h_flex()
                    .gap_2()
                    .items_end()
                    .child(
                        Field::new().label("Sample time (0–1)").child(
                            div().w_24().child(
                                Input::new(&self.sample_time_input)
                                    .id("graph-sample-time")
                                    .aria_label(if source_animation {
                                        "Source sample time"
                                    } else {
                                        "Graph sample time"
                                    })
                                    .disabled(time_disabled),
                            ),
                        ),
                    )
                    .child(
                        Button::new("graph-set-sample-time")
                            .small()
                            .label("Set time")
                            .disabled(time_disabled)
                            .on_click(cx.listener(|view, _, _, cx| view.set_sample_time(cx))),
                    ),
            ))
            .when_some(self.sample_time_error.as_ref(), |this, error| {
                this.child(
                    div()
                        .id("graph-sample-time-error")
                        .role(gpui_kit::Role::Alert)
                        .aria_label(error.clone())
                        .test_support()
                        .text_sm()
                        .text_color(theme.danger)
                        .child(error.clone()),
                )
            });
        let rem = window.rem_size();
        if results_visible
            || self.overview_open
            || source_animation
            || property_editor
            || self.drawing.is_some()
            || !self.canvas.visible
        {
            self.node_previews.suspend();
        }
        let lab_section_panel = self.render_lab_section_panel(busy, rem, cx);
        let panels = if results_visible {
            self.render_results(busy, cx).into_any_element()
        } else if graph_tools && self.overview_open {
            self.render_overview(
                busy,
                inspector.into_any_element(),
                preview.into_any_element(),
                window,
                cx,
            )
            .into_any_element()
        } else if source_animation || property_editor {
            gpui_kit::component::resizable::h_resizable(if lab_section_panel.is_some() {
                "property-lab-panels"
            } else {
                "solid-animation-panels"
            })
            .when_some(lab_section_panel, |panels, panel| panels.child(panel))
            .child(
                gpui_kit::component::resizable::resizable_panel()
                    .size_range(rem * 14. ..gpui_kit::Pixels::MAX)
                    .child(preview),
            )
            .child(
                gpui_kit::component::resizable::resizable_panel()
                    .size(rem * 22.)
                    .size_range(rem * 16. ..rem * 32.)
                    .child(inspector),
            )
            .into_any_element()
        } else if let Some(hierarchy) = self.render_canvas_hierarchy(busy, cx) {
            gpui_kit::component::resizable::h_resizable("canvas-authoring-panels")
                .child(
                    gpui_kit::component::resizable::resizable_panel()
                        .size_range(rem * 16. ..gpui_kit::Pixels::MAX)
                        .child(preview),
                )
                .child(
                    gpui_kit::component::resizable::resizable_panel()
                        .size(rem * 22.)
                        .size_range(rem * 17. ..rem * 36.)
                        .child(
                            gpui_kit::component::resizable::v_resizable("canvas-authoring-detail")
                                .child(
                                    gpui_kit::component::resizable::resizable_panel()
                                        .size(rem * 19.)
                                        .size_range(rem * 12. ..gpui_kit::Pixels::MAX)
                                        .child(hierarchy),
                                )
                                .child(
                                    gpui_kit::component::resizable::resizable_panel()
                                        .size_range(rem * 9. ..gpui_kit::Pixels::MAX)
                                        .child(inspector),
                                ),
                        ),
                )
                .into_any_element()
        } else if self.canvas.visible {
            gpui_kit::component::resizable::h_resizable(if lab_section_panel.is_some() {
                "graph-canvas-lab-panels"
            } else {
                "graph-canvas-panels"
            })
            .child(
                gpui_kit::component::resizable::resizable_panel()
                    .size_range(rem * 20. ..gpui_kit::Pixels::MAX)
                    .child(self.render_canvas(window, cx)),
            )
            .when_some(lab_section_panel, |panels, panel| panels.child(panel))
            .child(
                gpui_kit::component::resizable::resizable_panel()
                    .size(rem * 24.)
                    .size_range(rem * 14. ..rem * 36.)
                    .child(
                        gpui_kit::component::resizable::v_resizable(if standalone_texture {
                            "graph-canvas-synthesis-detail"
                        } else {
                            "graph-canvas-detail"
                        })
                        .child(
                            gpui_kit::component::resizable::resizable_panel()
                                .size_range(rem * 12. ..gpui_kit::Pixels::MAX)
                                .child(inspector),
                        )
                        .child(
                            gpui_kit::component::resizable::resizable_panel()
                                .size(rem * if standalone_texture { 24. } else { 14. })
                                .size_range(
                                    rem * if standalone_texture { 18. } else { 10. }
                                        ..rem * if standalone_texture { 32. } else { 28. },
                                )
                                .child(preview),
                        ),
                    ),
            )
            .into_any_element()
        } else {
            gpui_kit::component::resizable::h_resizable(if lab_section_panel.is_some() {
                "graph-document-lab-panels"
            } else {
                "graph-document-panels"
            })
            .child(
                gpui_kit::component::resizable::resizable_panel()
                    .size(rem * 16.)
                    .size_range(rem * 10. ..rem * 28.)
                    .child(navigation),
            )
            .when_some(lab_section_panel, |panels, panel| panels.child(panel))
            .child(
                gpui_kit::component::resizable::resizable_panel()
                    .size_range(rem * 14. ..gpui_kit::Pixels::MAX)
                    .child(preview),
            )
            .child(
                gpui_kit::component::resizable::resizable_panel()
                    .size(rem * 20.)
                    .size_range(rem * 14. ..rem * 32.)
                    .child(inspector),
            )
            .into_any_element()
        };
        let rendering = matches!(
            self.work,
            Some(
                Work::RenderingResult
                    | Work::WritingEnvironmentOutput
                    | Work::WritingSynthesisPreset
            )
        ) || self.work == Some(Work::Exporting) && !results_visible;
        let feedback = self
            .error
            .clone()
            .or_else(|| self.status.clone())
            .unwrap_or_else(|| match self.work {
                Some(Work::Opening) => "Opening document…".into(),
                Some(Work::Duplicating) => "Duplicating document…".into(),
                Some(Work::ApplyingOverviewPreset) => "Applying preset…".into(),
                Some(Work::ChoosingOverviewPresetFolder) => "Choosing a preset folder…".into(),
                Some(Work::ImportingMesh) => "Importing 3D model…".into(),
                Some(Work::LoadingEnvironment) => "Loading environment file…".into(),
                Some(Work::LoadingSynthesis) => "Loading Synthesis file…".into(),
                Some(Work::LoadingStyle) => "Loading Style preset…".into(),
                Some(Work::LoadingNoiseLab) => "Preparing Noise Lab input…".into(),
                Some(Work::OpeningCurvePreset) => "Opening curve preset…".into(),
                Some(Work::SavingCurvePreset) => "Saving curve preset…".into(),
                Some(Work::ChoosingSave) if source_animation => {
                    "Choose where to save the GTX".into()
                }
                Some(Work::ChoosingSave) => "Choose where to save the document".into(),
                Some(Work::Saving) if source_animation => "Saving Solid Color source…".into(),
                Some(Work::Saving) => "Saving current graph…".into(),
                Some(Work::Renaming) => "Renaming document…".into(),
                Some(Work::ChoosingExport) => "Choose where to save the image".into(),
                Some(Work::Exporting) if results_visible => "Saving image…".into(),
                Some(Work::Exporting) => "Rendering and saving image…".into(),
                Some(Work::RenderingResult) => "Rendering result…".into(),
                Some(Work::ProjectingResult) => "Preparing result map…".into(),
                Some(Work::ChoosingEnvironmentOutput) => "Choose an environment output file".into(),
                Some(Work::WritingEnvironmentOutput) => {
                    "Rendering and saving environment map…".into()
                }
                Some(Work::ChoosingSynthesisPreset) => {
                    "Choose where to save the Synthesis preset".into()
                }
                Some(Work::WritingSynthesisPreset) => {
                    "Rendering and saving Synthesis preset…".into()
                }
                None => {
                    if self
                        .inspector
                        .gradient
                        .as_ref()
                        .is_some_and(|draft| draft.is_changed(cx))
                    {
                        "New gradient stop position · Add a stop or revert before saving".into()
                    } else if self
                        .inspector
                        .custom
                        .as_ref()
                        .is_some_and(|draft| draft.is_changed(cx))
                    {
                        "Unapplied point edits · Add, move, or revert before saving".into()
                    } else if self.has_field_drafts(cx) {
                        "Unapplied field edits".into()
                    } else if !source_animation && self.is_dirty() {
                        "Unsaved changes".into()
                    } else {
                        "Ready".into()
                    }
                }
            });
        div()
            .id("graph-document-workspace")
            .key_context("GraphDocument")
            .track_focus(&self.focus)
            .size_full()
            .v_flex()
            .bg(theme.background)
            .text_color(theme.foreground)
            .on_action(cx.listener(|view, _: &OpenDocument, window, cx| {
                view.request(Intent::Open(DocumentFormat::Mycela), window, cx)
            }))
            .on_action(cx.listener(|view, _: &OpenGraphDocument, window, cx| {
                view.request(Intent::Open(DocumentFormat::Mycela), window, cx)
            }))
            .on_action(cx.listener(|view, _: &SaveDocument, _, cx| view.save(false, cx)))
            .on_action(cx.listener(|view, _: &SaveDocumentAs, _, cx| view.save(true, cx)))
            .when(graph_tools, |workspace| {
                workspace.on_action(cx.listener(|view, _: &RenameDocument, window, cx| {
                    view.open_rename_document(window, cx);
                }))
            })
            .on_action(cx.listener(|view, _: &UndoGraphEdit, window, cx| {
                view.travel_history(HistoryDirection::Undo, window, cx)
            }))
            .on_action(cx.listener(|view, _: &RedoGraphEdit, window, cx| {
                view.travel_history(HistoryDirection::Redo, window, cx)
            }))
            .when(graph_tools && !results_visible, |this| {
                this.on_action(cx.listener(|view, _: &CopyNodeTree, _, cx| {
                    view.copy_node_tree(cx);
                }))
                .on_action(cx.listener(|view, _: &PasteNodeTree, window, cx| {
                    view.paste_node_tree(window, cx);
                }))
                .on_action(cx.listener(|view, _: &PasteNodeIntoSelection, window, cx| {
                    view.paste_node_into_selection(window, cx);
                }))
                .on_action(cx.listener(|view, _: &ExportGraphImage, window, cx| {
                    view.open_export_image(window, cx)
                }))
                .on_action(cx.listener(
                    |view, _: &RenderSelectedBranch, _, cx| {
                        view.render_selected_branch(cx);
                    },
                ))
            })
            .when(!results_visible, |this| {
                this.on_action(cx.listener(|view, _: &RevertGraphFields, window, cx| {
                    if view.embedded_environment.is_some() && !view.is_busy() {
                        window.remove_window();
                    } else {
                        view.revert_fields(window, cx);
                    }
                }))
            })
            .when(graph_tools && !results_visible, |this| {
                this.on_action(cx.listener(|view, _: &GroupSelectedBranch, window, cx| {
                    view.group_selected_branch(window, cx)
                }))
                .on_action(cx.listener(|view, _: &FocusGraph, window, cx| {
                    view.canvas.visible = false;
                    view.tree.update(cx, |tree, cx| tree.focus(window, cx));
                    cx.notify();
                }))
            })
            .child(
                div()
                    .v_flex()
                    .border_b_1()
                    .border_color(theme.border)
                    .when(graph_tools, |header| header.child(menus))
                    .child(toolbar),
            )
            .child(
                div()
                    .h_flex()
                    .items_start()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .children(self.workspace.as_ref().and_then(|owner| {
                        let workspace = owner.upgrade()?;
                        Some(workspace.read(cx).render_strip(owner.clone(), busy, cx))
                    }))
                    .child(div().flex_1().h_full().min_w_0().min_h_0().child(panels)),
            )
            .child(
                div()
                    .id("graph-document-status")
                    .test_support()
                    .role(gpui_kit::Role::Status)
                    .aria_label(feedback.clone())
                    .h_flex()
                    .gap_3()
                    .items_center()
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(theme.border)
                    .text_sm()
                    .when(self.error.is_some(), |this| this.text_color(theme.danger))
                    .child(feedback)
                    .when(rendering, |row| {
                        row.child(
                            div()
                                .id("graph-render-progress")
                                .test_support()
                                .w(rems(9.))
                                .child(
                                    Progress::new("render-progress")
                                        .small()
                                        .loading(true)
                                        .accessibility_label("Render progress"),
                                ),
                        )
                    }),
            )
    }
}
pub fn open_graph_document_window(
    cx: &mut App,
) -> Result<(AnyWindowHandle, Entity<GraphDocumentView>), String> {
    open_graph_window(true, cx)
}

pub(crate) fn open_node_editor_window(
    cx: &mut App,
) -> Result<(AnyWindowHandle, Entity<GraphDocumentView>), String> {
    open_graph_window(false, cx)
}

fn open_graph_window(
    choose_file: bool,
    cx: &mut App,
) -> Result<(AnyWindowHandle, Entity<GraphDocumentView>), String> {
    bind_keys(cx);
    recent_paths::RecentPaths::initialize(cx);
    let (window, workspace) = gpui_kit::open_window(
        WindowOptions {
            // Physical platform window bounds, not product control geometry.
            window_bounds: Some(gpui_kit::WindowBounds::Windowed(
                gpui_kit::Bounds::centered(
                    None,
                    gpui_kit::size(gpui_kit::px(1120.), gpui_kit::px(720.)),
                    cx,
                ),
            )),
            window_min_size: Some(gpui_kit::size(gpui_kit::px(880.), gpui_kit::px(520.))),
            app_id: Some("org.mycela.MyCela".into()),
            ..Default::default()
        },
        cx,
        move |window, cx| {
            window.set_window_title("Mycela · Node editor");
            cx.new(|cx| document_workspace::DocumentWorkspace::new(choose_file, window, cx))
        },
    )
    .map_err(|e| e.to_string())?;
    Ok((window, workspace.read(cx).active()))
}

pub(crate) fn open_solid_animation_window(
    document: LiveDocument,
    path: Option<PathBuf>,
    time: f64,
    cx: &mut App,
) -> Result<(AnyWindowHandle, Entity<GraphDocumentView>), String> {
    gpui_kit::open_window(
        WindowOptions {
            window_bounds: Some(gpui_kit::WindowBounds::Windowed(
                gpui_kit::Bounds::centered(
                    None,
                    gpui_kit::size(gpui_kit::px(880.), gpui_kit::px(640.)),
                    cx,
                ),
            )),
            window_min_size: Some(gpui_kit::size(gpui_kit::px(640.), gpui_kit::px(480.))),
            ..Default::default()
        },
        cx,
        move |window, cx| {
            window.set_window_title("Mycela · Solid Color animation");
            cx.new(|cx| GraphDocumentView::with_document(document, path, time, true, window, cx))
        },
    )
    .map_err(|e| e.to_string())
}

#[cfg(test)]
#[path = "graph_document_scene_tests.rs"]
mod scene_tests;
