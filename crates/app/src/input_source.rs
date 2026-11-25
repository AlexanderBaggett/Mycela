//! An input owns a real source occurrence. The selected editor is presentation;
//! The actual retained slot is the authority for attachment and rendering.
use crate::{
    ForegroundError,
    color_binding_effects::BindingRuntime,
    gradient_source::{GradientSourceView, TextureSourceKind},
};
use gpui_kit::base::{Disableable, StyledExt, TestSupportExt};
use gpui_kit::component::{
    ActiveTheme, Sizable,
    button::{Button, ButtonVariants},
    form::{Field, Form},
    input::{Input, InputState},
    menu::{DropdownMenu, PopupMenuItem},
};
use gpui_kit::{App, AppContext, Context, Entity, IntoElement, Render, Window, div, prelude::*};
use mycela_engine::{PointwiseFilter, PointwiseNode, PointwiseSlot, PointwiseSolidChangeHandler};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

const SOURCES: [TextureSourceKind; 6] = [
    TextureSourceKind::SolidColor,
    TextureSourceKind::Gradient,
    TextureSourceKind::Noise,
    TextureSourceKind::Cells,
    TextureSourceKind::WoodRings,
    TextureSourceKind::BricksBoards,
];
gpui_kit::actions!(input_source, [ApplyInputColor, RevertInputColor]);

/// Actual input-slot identities, with presentation names independent from map pixels.
#[derive(Clone, Copy)]
pub(crate) enum InputSourceSlot {
    MedianRadius,
    MotionRadius,
    MotionAngle,
    ShapeRadius,
    ShapeMap,
    SmartRadius,
    VariableRadius,
    SmearDirection,
    SmearDistance,
    MatchReference,
    ArtRadius,
    ArtDriver,
    DistortDirection,
    DistortDistance,
    RemapX,
    RemapY,
}
impl InputSourceSlot {
    fn slot(self) -> i32 {
        match self {
            Self::MedianRadius
            | Self::MotionRadius
            | Self::ShapeRadius
            | Self::SmartRadius
            | Self::VariableRadius
            | Self::SmearDirection
            | Self::MatchReference
            | Self::ArtRadius
            | Self::DistortDirection
            | Self::RemapX => 1,
            Self::MotionAngle
            | Self::ShapeMap
            | Self::SmearDistance
            | Self::ArtDriver
            | Self::DistortDistance
            | Self::RemapY => 2,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::DistortDirection => "Direction map source",
            Self::DistortDistance => "Distance map source",
            Self::RemapX => "X map source",
            Self::RemapY => "Y map source",

            Self::MedianRadius
            | Self::MotionRadius
            | Self::ShapeRadius
            | Self::SmartRadius
            | Self::VariableRadius
            | Self::ArtRadius => "Radius map source",
            Self::MatchReference => "Match to source",
            Self::ArtDriver => "Driver map source",
            Self::SmearDirection => "Direction map source",
            Self::SmearDistance => "Distance map source",
            Self::MotionAngle => "Angle map source",
            Self::ShapeMap => "Shape map source",
        }
    }
    fn empty(self) -> &'static str {
        match self {
            Self::DistortDirection => "No direction map",
            Self::DistortDistance => "No distance map",
            Self::RemapX => "No X map",
            Self::RemapY => "No Y map",

            Self::MedianRadius
            | Self::MotionRadius
            | Self::ShapeRadius
            | Self::SmartRadius
            | Self::VariableRadius
            | Self::ArtRadius => "No radius map",
            Self::MatchReference => "No match image",
            Self::ArtDriver => "No driver map",
            Self::SmearDirection => "No direction map",
            Self::SmearDistance => "No distance map",
            Self::MotionAngle => "No angle map",
            Self::ShapeMap => "No shape map",
        }
    }
    fn namespace(self, id: u64) -> Option<String> {
        match self {
            Self::DistortDirection => Some(format!("distort-direction-{id}")),
            Self::DistortDistance => Some(format!("distort-distance-{id}")),
            Self::RemapX => Some(format!("remap-x-{id}")),
            Self::RemapY => Some(format!("remap-y-{id}")),

            Self::MatchReference => Some(format!("match-reference-{id}")),
            Self::ArtRadius => Some(format!("art-radius-{id}")),
            Self::ArtDriver => Some(format!("art-driver-{id}")),
            Self::MedianRadius => None,
            Self::MotionRadius => Some(format!("motion-radius-{id}")),
            Self::MotionAngle => Some(format!("motion-angle-{id}")),
            Self::ShapeRadius => Some(format!("shape-radius-{id}")),
            Self::ShapeMap => Some(format!("shape-map-{id}")),
            Self::SmartRadius => Some(format!("smart-radius-{id}")),
            Self::VariableRadius => Some(format!("variable-radius-{id}")),
            Self::SmearDirection => Some(format!("smear-direction-{id}")),
            Self::SmearDistance => Some(format!("smear-distance-{id}")),
        }
    }
    fn selector(self, id: u64) -> String {
        match self {
            Self::DistortDirection => format!("distort-direction-source-{id}"),
            Self::DistortDistance => format!("distort-distance-source-{id}"),
            Self::RemapX => format!("remap-x-source-{id}"),
            Self::RemapY => format!("remap-y-source-{id}"),

            Self::MatchReference => format!("match-reference-source-{id}"),
            Self::ArtRadius => format!("art-radius-source-{id}"),
            Self::ArtDriver => format!("art-driver-source-{id}"),
            Self::MedianRadius => format!("median-radius-source-{id}"),
            Self::MotionRadius => format!("motion-radius-source-{id}"),
            Self::MotionAngle => format!("motion-angle-source-{id}"),
            Self::ShapeRadius => format!("shape-radius-source-{id}"),
            Self::ShapeMap => format!("shape-map-source-{id}"),
            Self::SmartRadius => format!("smart-radius-source-{id}"),
            Self::VariableRadius => format!("variable-radius-source-{id}"),
            Self::SmearDirection => format!("smear-direction-source-{id}"),
            Self::SmearDistance => format!("smear-distance-source-{id}"),
        }
    }
    fn section(self, id: u64) -> gpui_kit::ElementId {
        match self {
            Self::DistortDirection => format!("distort-direction-{id}-source").into(),
            Self::DistortDistance => format!("distort-distance-{id}-source").into(),
            Self::RemapX => format!("remap-x-{id}-source").into(),
            Self::RemapY => format!("remap-y-{id}-source").into(),

            Self::MatchReference => format!("match-reference-{id}-source").into(),
            Self::ArtRadius => format!("art-radius-{id}-source").into(),
            Self::ArtDriver => format!("art-driver-{id}-source").into(),
            Self::MedianRadius => ("median-radius-map", id).into(),
            Self::MotionRadius => format!("motion-radius-{id}-source").into(),
            Self::MotionAngle => format!("motion-angle-{id}-source").into(),
            Self::ShapeRadius => format!("shape-radius-{id}-source").into(),
            Self::ShapeMap => format!("shape-map-{id}-source").into(),
            Self::SmartRadius => format!("smart-radius-{id}-source").into(),
            Self::VariableRadius => format!("variable-radius-{id}-source").into(),
            Self::SmearDirection => format!("smear-direction-{id}-source").into(),
            Self::SmearDistance => format!("smear-distance-{id}-source").into(),
        }
    }
}
fn scoped_id(namespace: Option<&str>, id: impl AsRef<str>) -> String {
    namespace.map_or_else(
        || id.as_ref().to_owned(),
        |prefix| format!("{prefix}-{}", id.as_ref()),
    )
}

/// Shared by a source editor and its queued jobs; callbacks retain only Weak.
/// Attachment identity, not a UI Boolean, authorizes edits to an input owner.
pub(crate) struct InputOwner {
    slot: PointwiseSlot<ForegroundError>,
    index: i32,
    node: PointwiseNode<ForegroundError>,
}
impl InputOwner {
    fn new(
        slot: PointwiseSlot<ForegroundError>,
        index: i32,
        node: PointwiseNode<ForegroundError>,
    ) -> Self {
        Self { slot, index, node }
    }
    pub(crate) fn is_current(&self) -> bool {
        self.slot
            .parent()
            .slot(self.index)
            .is_ok_and(|slot| slot.same_reference(&self.slot))
            && self
                .slot
                .node()
                .is_some_and(|node| node.same_reference(&self.node))
    }
    /// Synchronous callbacks may replace the slot after a setter commits.
    /// Retire the old marker so its already queued presentation cannot publish.
    pub(crate) fn admits(&self, activation: &Weak<AtomicBool>) -> bool {
        let Some(marker) = activation.upgrade() else {
            return false;
        };
        if !self.is_current() {
            marker.store(false, Ordering::Release);
            return false;
        }
        marker.load(Ordering::Acquire)
    }
}

enum SourceEditor {
    Solid(Entity<InputColorView>),
    Generator(Entity<GradientSourceView>),
}
struct SourceOccurrence {
    node: PointwiseNode<ForegroundError>,
    owner: Arc<InputOwner>,
    editor: SourceEditor,
}

/// Per-filter source occurrences survive selection and retain their dirty drafts.
/// Source/Previous/Filter DAG lowering remains a separate graph-editing feature.
pub(crate) struct InputSourceView {
    id: u64,
    input: InputSourceSlot,
    filter: PointwiseFilter<ForegroundError>,
    sources: BTreeMap<TextureSourceKind, SourceOccurrence>,
    runtime: Weak<BindingRuntime>,
    active: Arc<AtomicBool>,
    pending: bool,
    time: f64,
    window: gpui_kit::AnyWindowHandle,
    error: Option<String>,
}
impl InputSourceView {
    pub(crate) fn new(
        id: u64,
        input: InputSourceSlot,
        filter: PointwiseFilter<ForegroundError>,
        time: f64,
        runtime: &Arc<BindingRuntime>,
        window: &Window,
    ) -> Self {
        Self {
            id,
            input,
            filter,
            time,
            sources: BTreeMap::new(),
            runtime: Arc::downgrade(runtime),
            active: Arc::new(AtomicBool::new(false)),
            pending: false,
            window: window.window_handle(),
            error: None,
        }
    }
    fn attached(&self) -> Option<PointwiseNode<ForegroundError>> {
        self.filter
            .node()
            .slot(self.input.slot())
            .ok()
            .and_then(|s| s.node())
    }
    fn selected(&self) -> Option<TextureSourceKind> {
        let node = self.attached()?;
        self.sources
            .iter()
            .find(|(_, s)| s.node.same_reference(&node))
            .map(|(kind, _)| *kind)
    }
    fn activate_children(&self, cx: &mut App) {
        let selected = self.selected();
        for (kind, source) in &self.sources {
            let enabled = self.active.load(Ordering::Acquire) && selected == Some(*kind);
            match &source.editor {
                SourceEditor::Solid(view) => view.update(cx, |v, _| v.set_active(enabled)),
                SourceEditor::Generator(view) => view.update(cx, |v, _| v.set_active(enabled)),
            }
        }
    }
    pub(crate) fn set_active(&mut self, active: bool, cx: &mut App) {
        self.active.store(false, Ordering::Release);
        self.pending = false;
        self.active = Arc::new(AtomicBool::new(active));
        self.activate_children(cx);
    }
    pub(crate) fn set_sample_time(
        &mut self,
        time: f64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.time = time;
        for source in self.sources.values() {
            if let SourceEditor::Generator(view) = &source.editor {
                view.update(cx, |v, cx| v.set_preview_time(time, window, cx));
            }
        }
    }
    pub(crate) fn refresh_controls(&mut self, cx: &mut Context<Self>) {
        let active = Arc::downgrade(&self.active);
        let handle = self.window;
        cx.spawn(async move |owner, cx| {
            if !active.upgrade().is_some_and(|m| m.load(Ordering::Acquire)) {
                return;
            }
            let _ = cx.update_window(handle, |_, window, cx| {
                owner.update(cx, |v, cx| {
                    if !active.upgrade().is_some_and(|m| m.load(Ordering::Acquire)) {
                        return;
                    }
                    for source in v.sources.values() {
                        match &source.editor {
                            SourceEditor::Solid(view) => {
                                view.update(cx, |v, cx| v.sync(window, cx))
                            }
                            SourceEditor::Generator(view) => {
                                view.update(cx, |v, cx| v.sync(window, cx))
                            }
                        }
                    }
                    cx.notify();
                })
            });
        })
        .detach();
    }
    fn choose(
        &mut self,
        requested: Option<TextureSourceKind>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.pending || !self.active.load(Ordering::Acquire) {
            return;
        }
        if requested == self.selected()
            && ((requested.is_none() && self.attached().is_none())
                || requested
                    .and_then(|kind| self.sources.get(&kind))
                    .is_some_and(|source| source.owner.is_current()))
        {
            return;
        }
        let existing = requested.and_then(|kind| self.sources.get(&kind).map(|s| s.node.clone()));
        let expected = self.attached();
        let filter = self.filter.clone();
        let input = self.input;
        let marker = Arc::downgrade(&self.active);
        let runtime = self.runtime.clone();
        let handle = window.window_handle();
        self.pending = true;
        cx.notify();
        cx.spawn(async move |owner, cx| {
            let alive = cx
                .update_window(handle, |_, _, cx| owner.read_with(cx, |_, _| ()))
                .is_ok_and(|v| v.is_ok());
            if !alive || !marker.upgrade().is_some_and(|m| m.load(Ordering::Acquire)) {
                return;
            }
            // Actual slot commit/reparent/notifications run outside App/Entity borrows.
            let result = (|| -> Result<_, String> {
                let slot = filter
                    .node()
                    .slot(input.slot())
                    .map_err(|e| e.to_string())?;
                let current = slot.node();
                if !same_node(&current, &expected) {
                    return Err("The input source changed. Choose it again.".into());
                }
                let node = match (requested, existing) {
                    (None, _) => None,
                    (Some(_), Some(node)) => Some(node),
                    (Some(TextureSourceKind::SolidColor), None) => {
                        Some(PointwiseNode::solid_color().map_err(|e| e.to_string())?)
                    }
                    (Some(kind), None) => Some(
                        PointwiseNode::image_generator(
                            kind.generator_kind()
                                .ok_or_else(|| "The source kind is unavailable.".to_owned())?,
                        )
                        .map_err(|e| e.to_string())?,
                    ),
                };
                if node.is_none() {
                    // This linear editor supplies Original through its render
                    // route. Use qi.c(null) to clear only the chosen auxiliary:
                    // ordinary qi.Node(null) converts an emptied base graph to
                    // filter mode and installs fresh default-image inputs.
                    // The common completion path requests app presentation.
                    slot.set_node_quiet(None).map_err(|e| e.to_string())?;
                } else {
                    slot.set_node(node.as_ref()).map_err(|e| e.to_string())?;
                }
                Ok(node)
            })();
            if !marker.upgrade().is_some_and(|m| m.load(Ordering::Acquire)) {
                return;
            }
            let delivered = cx.update_window(handle, |_, window, cx| {
                owner.update(cx, |v, cx| {
                    v.pending = false;
                    match result {
                        Err(e) => v.error = Some(e),
                        Ok(node) => {
                            v.error = None;
                            if let (Some(kind), Some(node)) = (requested, node) {
                                let time = v.time;
                                let Ok(slot) = v.filter.node().slot(input.slot()) else {
                                    v.error = Some("The input source slot is unavailable.".into());
                                    return;
                                };
                                let input_owner =
                                    Arc::new(InputOwner::new(slot, input.slot(), node.clone()));
                                let namespace = v.input.namespace(v.id);
                                if let Some(source) = v.sources.get_mut(&kind) {
                                    // Reattach the retained editor to the new actual qi.
                                    // Old queued jobs keep their retired owner/marker.
                                    match &source.editor {
                                        SourceEditor::Solid(view) => view.update(cx, |view, _| {
                                            view.rebind_owner(input_owner.clone())
                                        }),
                                        SourceEditor::Generator(view) => view
                                            .update(cx, |view, _| {
                                                view.rebind_input_owner(input_owner.clone())
                                            }),
                                    }
                                    source.owner = input_owner.clone();
                                }
                                if let std::collections::btree_map::Entry::Vacant(entry) =
                                    v.sources.entry(kind)
                                {
                                    if let Some(runtime) = v.runtime.upgrade() {
                                        let editor = if kind == TextureSourceKind::SolidColor {
                                            Some(SourceEditor::Solid(cx.new(|cx| {
                                                InputColorView::new(
                                                    namespace.clone(),
                                                    node.clone(),
                                                    input_owner.clone(),
                                                    &runtime,
                                                    window,
                                                    cx,
                                                )
                                            })))
                                        } else {
                                            node.image_generator_handle().map(|model| {
                                                SourceEditor::Generator(cx.new(|cx| {
                                                    let mut editor = GradientSourceView::new_input(
                                                        namespace.clone(),
                                                        model,
                                                        input_owner.clone(),
                                                        &runtime,
                                                        window,
                                                        cx,
                                                    );
                                                    editor.set_preview_time(time, window, cx);
                                                    editor
                                                }))
                                            })
                                        };
                                        if let Some(editor) = editor {
                                            entry.insert(SourceOccurrence {
                                                node,
                                                owner: input_owner.clone(),
                                                editor,
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                    v.activate_children(cx);
                    cx.notify();
                })
            });
            // Present partial engine state too. A failed setter is not rolled back.
            if delivered.is_ok_and(|v| v.is_ok())
                && marker.upgrade().is_some_and(|m| m.load(Ordering::Acquire))
            {
                if let Some(runtime) = runtime.upgrade() {
                    if let Err(error) = runtime.request_effect_presentation(marker) {
                        runtime.retain_failure(error);
                    }
                }
            }
        })
        .detach();
    }
}
fn same_node(
    a: &Option<PointwiseNode<ForegroundError>>,
    b: &Option<PointwiseNode<ForegroundError>>,
) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.same_reference(b),
        _ => false,
    }
}
impl Drop for InputSourceView {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
    }
}
impl Render for InputSourceView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.selected();
        let attached = self.attached().is_some();
        let label = current.map(TextureSourceKind::name).unwrap_or(if attached {
            "External source"
        } else {
            self.input.empty()
        });
        let owner = cx.entity().downgrade();
        let empty = self.input.empty();
        let mut view = div()
            .id(self.input.section(self.id))
            .v_flex()
            .gap_3()
            .child(
                Field::new().label(self.input.label()).child(
                    Button::new(self.input.selector(self.id))
                        .label(label)
                        .small()
                        .w_full()
                        .dropdown_caret(true)
                        .disabled(self.pending)
                        .dropdown_menu(move |mut menu, _, _| {
                            for kind in std::iter::once(None).chain(SOURCES.into_iter().map(Some)) {
                                let owner = owner.clone();
                                menu = menu.item(
                                    PopupMenuItem::new(
                                        kind.map(TextureSourceKind::name).unwrap_or(empty),
                                    )
                                    .checked(current == kind && (kind.is_some() || !attached))
                                    .on_click(
                                        move |_, window, cx| {
                                            let _ = owner
                                                .update(cx, |v, cx| v.choose(kind, window, cx));
                                        },
                                    ),
                                );
                            }
                            menu.scrollable(true)
                        }),
                ),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        "Each source is an independent input. Animation uses the workspace time.",
                    ),
            );
        if let Some(source) = current.and_then(|kind| self.sources.get(&kind)) {
            view = match &source.editor {
                SourceEditor::Solid(editor) => view.child(editor.clone()),
                SourceEditor::Generator(editor) => view.child(editor.clone()),
            };
        } else if attached {
            view =
                view.child(div().text_sm().child(
                    "This input is owned by the graph. Its controls are not available here.",
                ));
        }
        view.when_some(self.error.clone(), |v, e| {
            v.child(
                div()
                    .id(scoped_id(
                        self.input.namespace(self.id).as_deref(),
                        format!("input-source-error-{}", self.id),
                    ))
                    .role(gpui_kit::Role::Alert)
                    .aria_label(e.clone())
                    .test_support()
                    .text_color(cx.theme().danger)
                    .child(e),
            )
        })
    }
}

/// Solid inputs use the existing sponsored Solid Color setters; these fields are drafts only.
struct InputColorView {
    namespace: Option<String>,
    owner: Arc<InputOwner>,
    node: PointwiseNode<ForegroundError>,
    fields: [Entity<InputState>; 4],
    synced: [String; 4],
    active: Arc<AtomicBool>,
    listener: Option<PointwiseSolidChangeHandler<ForegroundError>>,
    runtime: Weak<BindingRuntime>,
    pending: bool,
    error: Option<String>,
}
impl InputColorView {
    fn control_id(&self, id: impl AsRef<str>) -> String {
        scoped_id(self.namespace.as_deref(), id)
    }
    fn values(node: &PointwiseNode<ForegroundError>) -> Result<[String; 4], String> {
        let color = node.solid_color_values().map_err(|e| e.to_string())?;
        let alpha = node.solid_transparency().map_err(|e| e.to_string())?;
        Ok([
            color[0].to_string(),
            color[1].to_string(),
            color[2].to_string(),
            alpha.to_string(),
        ])
    }
    fn new(
        namespace: Option<String>,
        node: PointwiseNode<ForegroundError>,
        owner: Arc<InputOwner>,
        runtime: &Arc<BindingRuntime>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.bind_keys([
            gpui_kit::KeyBinding::new("secondary-enter", ApplyInputColor, Some("InputColor")),
            gpui_kit::KeyBinding::new("escape", RevertInputColor, Some("InputColor")),
        ]);
        let values = Self::values(&node);
        let error = values.as_ref().err().cloned();
        let synced = values.unwrap_or_else(|_| std::array::from_fn(|_| String::new()));
        let fields = std::array::from_fn(|i| {
            cx.new(|cx| InputState::new(window, cx).default_value(synced[i].clone()))
        });
        Self {
            namespace,
            node,
            owner,
            fields,
            synced,
            runtime: Arc::downgrade(runtime),
            active: Arc::new(AtomicBool::new(false)),
            listener: None,
            pending: false,
            error,
        }
    }
    fn rebind_owner(&mut self, owner: Arc<InputOwner>) {
        self.set_active(false);
        self.owner = owner;
    }
    fn set_active(&mut self, active: bool) {
        self.active.store(false, Ordering::Release);
        if let Some(listener) = self.listener.take() {
            let _ = self.node.remove_solid_change_handler(&listener);
        }
        self.pending = false;
        self.active = Arc::new(AtomicBool::new(active));
        if !active {
            return;
        }
        let marker = Arc::downgrade(&self.active);
        let runtime = self.runtime.clone();
        let owner = Arc::downgrade(&self.owner);
        match self.node.solid_change_handler(move |_, _| {
            if !marker.upgrade().is_some_and(|m| m.load(Ordering::Acquire))
                || !owner.upgrade().is_some_and(|o| o.admits(&marker))
            {
                return Ok(());
            }
            runtime
                .upgrade()
                .ok_or(ForegroundError::OwnerClosed)?
                .request_effect_presentation(marker.clone())
        }) {
            Ok(listener) => match self.node.add_solid_change_handler(&listener) {
                Ok(()) => self.listener = Some(listener),
                Err(e) => self.error = Some(e.to_string()),
            },
            Err(e) => self.error = Some(e.to_string()),
        }
    }
    fn text(&self, cx: &App) -> [String; 4] {
        std::array::from_fn(|i| self.fields[i].read(cx).value().to_string())
    }
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match Self::values(&self.node) {
            Ok(values) => {
                let text = self.text(cx);
                for i in 0..4 {
                    if text[i] == self.synced[i] {
                        self.fields[i]
                            .update(cx, |v, cx| v.set_value(values[i].clone(), window, cx));
                    }
                    self.synced[i] = values[i].clone();
                }
            }
            Err(e) => self.error = Some(e),
        }
        cx.notify();
    }
    fn revert(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending {
            return;
        }
        if let Ok(values) = Self::values(&self.node) {
            for (field, value) in self.fields.iter().zip(&values) {
                field.update(cx, |v, cx| v.set_value(value.clone(), window, cx));
            }
            self.synced = values;
            self.error = None;
        }
        cx.notify();
    }
    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending || !self.owner.admits(&Arc::downgrade(&self.active)) {
            return;
        }
        let submitted = self.text(cx);
        let mut values = [0.; 4];
        for i in 0..4 {
            match submitted[i].parse::<f64>() {
                Ok(value) => values[i] = value,
                Err(_) => {
                    self.error = Some(format!(
                        "{} needs a number.",
                        ["Red", "Green", "Blue", "Transparency"][i]
                    ));
                    cx.notify();
                    return;
                }
            }
        }
        let node = self.node.clone();
        let active = Arc::downgrade(&self.active);
        let input_owner = self.owner.clone();
        let runtime = self.runtime.clone();
        let handle = window.window_handle();
        self.pending = true;
        cx.notify();
        cx.spawn(async move |owner, cx| {
            let alive = cx
                .update_window(handle, |_, _, cx| owner.read_with(cx, |_, _| ()))
                .is_ok_and(|v| v.is_ok());
            if !alive || !input_owner.admits(&active) {
                return;
            }
            let result = (|| {
                node.set_solid_color([values[0], values[1], values[2]])
                    .map_err(|e| e.to_string())?;
                if !input_owner.admits(&active) {
                    return Ok(());
                }
                node.set_solid_transparency(values[3])
                    .map_err(|e| e.to_string())
            })();
            if !input_owner.admits(&active) {
                return;
            }
            let _ = cx.update_window(handle, |_, window, cx| {
                owner.update(cx, |v, cx| {
                    if !input_owner.admits(&active) {
                        return;
                    }
                    v.pending = false;
                    v.error = result.err();
                    if v.error.is_none() && v.text(cx) == submitted {
                        if let Ok(values) = Self::values(&v.node) {
                            for (field, value) in v.fields.iter().zip(&values) {
                                field.update(cx, |v, cx| v.set_value(value.clone(), window, cx));
                            }
                            v.synced = values;
                        }
                    }
                    v.sync(window, cx);
                })
            });
            if input_owner.admits(&active) {
                if let Some(runtime) = runtime.upgrade() {
                    if let Err(e) = runtime.request_effect_presentation(active) {
                        runtime.retain_failure(e);
                    }
                }
            }
        })
        .detach();
    }
}
impl Drop for InputColorView {
    fn drop(&mut self) {
        self.set_active(false);
    }
}
impl Render for InputColorView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .v_flex()
            .gap_3()
            .key_context("InputColor")
            .on_action(cx.listener(|v, _: &ApplyInputColor, window, cx| {
                v.apply(window, cx);
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|v, _: &RevertInputColor, window, cx| {
                v.revert(window, cx);
                cx.stop_propagation();
            }))
            .child(
                Form::new()
                    .columns(2)
                    .children(self.fields.iter().enumerate().map(|(i, field)| {
                        Field::new()
                            .label(["Red", "Green", "Blue", "Transparency"][i])
                            .child(
                                Input::new(field)
                                    .id(self.control_id(format!("input-color-{i}")))
                                    .aria_label(["Red", "Green", "Blue", "Transparency"][i])
                                    .disabled(self.pending),
                            )
                    })),
            )
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(
                        Button::new(self.control_id("apply-input-color"))
                            .label("Apply")
                            .primary()
                            .disabled(self.pending)
                            .on_click(cx.listener(|v, _, window, cx| v.apply(window, cx))),
                    )
                    .child(
                        Button::new(self.control_id("revert-input-color"))
                            .label("Revert edits")
                            .disabled(self.pending)
                            .on_click(cx.listener(|v, _, window, cx| v.revert(window, cx))),
                    ),
            )
            .when_some(self.error.clone(), |v, error| {
                v.child(
                    div()
                        .id(self.control_id("input-color-error"))
                        .role(gpui_kit::Role::Alert)
                        .aria_label(error.clone())
                        .test_support()
                        .text_color(cx.theme().danger)
                        .child(error),
                )
            })
    }
}
