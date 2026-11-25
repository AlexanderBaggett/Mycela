//! Retained generator controls. Numeric text and Select state are presentation;
//! the actual GradientGenerator owns every committed property.
#[path = "gradient_source_animation.rs"]
mod animation;
use crate::{ForegroundError, color_binding_effects::BindingRuntime};
use GradientGeneratorParameterKind::{BlockDensity, Location, Size};
use animation::GradientCurveEdit;
use gpui_kit::base::{Disableable, StyledExt, TestSupportExt};
use gpui_kit::component::{
    ActiveTheme, IndexPath,
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    collapsible::Collapsible,
    form::{Field, Form},
    input::{Input, InputState},
    select::{Select, SelectEvent, SelectItem, SelectState},
};
use gpui_kit::{
    App, AppContext, Context, Entity, Focusable, IntoElement, Render, SharedString, Subscription,
    Window, div, prelude::*,
};
use mycela_engine::{
    AnalyticCurveKind, CurveLayerHandle, GradientGenerator, GradientGeneratorChangeHandler,
    GradientGeneratorParameterKind, SharedCurve,
};
use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, Ordering},
};

gpui_kit::actions!(gradient_source, [ApplyGradient, RevertGradient]);
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum TextureSourceKind {
    SolidColor,
    Gradient,
    Noise,
    Cells,
    WoodRings,
    BricksBoards,
    Polygon,
    ImportedImage,
    Cone,
    Cube,
    Cylinder,
    Disc,
    Plane,
    Sphere,
    Torus,
}
impl TextureSourceKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::SolidColor => "Solid Color",
            Self::Gradient => "Gradient",
            Self::Noise => "Noise",
            Self::Cells => "Cells",
            Self::WoodRings => "Wood Rings",
            Self::BricksBoards => "Bricks & Boards",
            Self::Polygon => "Polygon",
            Self::ImportedImage => "Imported Image",
            Self::Cone => "Cone",
            Self::Cube => "Cube",
            Self::Cylinder => "Cylinder",
            Self::Disc => "Disc",
            Self::Plane => "Plane",
            Self::Sphere => "Sphere",
            Self::Torus => "Torus",
        }
    }
    pub(crate) fn generator_kind(self) -> Option<mycela_engine::ImageGeneratorKind> {
        use mycela_engine::ImageGeneratorKind as Kind;
        match self {
            Self::SolidColor => None,
            Self::Gradient => Some(Kind::Gradient),
            Self::Noise => Some(Kind::Noise),
            Self::Cells => Some(Kind::Cells),
            Self::WoodRings => Some(Kind::WoodRings),
            Self::BricksBoards => Some(Kind::BricksBoards),
            Self::ImportedImage => Some(Kind::ImportedImage),
            _ => None,
        }
    }
    pub(crate) fn scene_kind(self) -> Option<mycela_engine::ScenePrimitiveKind> {
        use mycela_engine::ScenePrimitiveKind as Kind;
        match self {
            Self::Cone => Some(Kind::Cone),
            Self::Cube => Some(Kind::Cube),
            Self::Cylinder => Some(Kind::Cylinder),
            Self::Disc => Some(Kind::Disc),
            Self::Plane => Some(Kind::Plane),
            Self::Sphere => Some(Kind::Sphere),
            Self::Torus => Some(Kind::Torus),
            _ => None,
        }
    }
    pub(crate) fn requires_owned_preview(self) -> bool {
        self.scene_kind().is_some()
            || matches!(
                self,
                Self::Polygon
                    | Self::Noise
                    | Self::Cells
                    | Self::WoodRings
                    | Self::BricksBoards
                    | Self::ImportedImage
            )
    }
}
#[derive(Clone)]
struct ChoiceItem {
    index: i32,
    title: SharedString,
}
impl SelectItem for ChoiceItem {
    type Value = i32;
    fn title(&self) -> SharedString {
        self.title.clone()
    }
    fn value(&self) -> &i32 {
        &self.index
    }
}
enum Edit {
    Values {
        pairs: Vec<(GradientGeneratorParameterKind, [f64; 2])>,
        scalars: Vec<(GradientGeneratorParameterKind, f64)>,
    },
    Choice(GradientGeneratorParameterKind, i32),
    Animation {
        axis: usize,
        enabled: bool,
    },
    Bounds(Vec<(usize, [f32; 2])>),
    GradientCurve(GradientCurveEdit),
    Time(f64),
}
struct Presentation {
    active: Arc<AtomicBool>,
    listener: GradientGeneratorChangeHandler<ForegroundError>,
}
struct ScalarDraft {
    kind: GradientGeneratorParameterKind,
    field: Entity<InputState>,
    synced: String,
}
struct CurveDraft {
    fields: [Entity<InputState>; 2],
    synced: [String; 2],
}
// Registration metadata selects presentation controls; committed values and
// animation roots remain exclusively in the actual generator parameters.
fn paired_kinds(model: &GradientGenerator<ForegroundError>) -> Vec<GradientGeneratorParameterKind> {
    model
        .parameters()
        .iter()
        .filter(|p| p.is_paired())
        .map(|p| p.kind())
        .collect()
}
fn scalar_kinds(model: &GradientGenerator<ForegroundError>) -> Vec<GradientGeneratorParameterKind> {
    model
        .parameters()
        .iter()
        .filter(|p| p.is_scalar())
        .map(|p| p.kind())
        .collect()
}
struct ChoiceDraft {
    kind: GradientGeneratorParameterKind,
    state: Entity<SelectState<Vec<ChoiceItem>>>,
    _subscription: Subscription,
}
fn choice_kinds(model: &GradientGenerator<ForegroundError>) -> Vec<GradientGeneratorParameterKind> {
    model
        .parameters()
        .iter()
        .filter(|p| p.is_choice())
        .map(|p| p.kind())
        .collect()
}
fn choice_id(kind: GradientGeneratorParameterKind) -> &'static str {
    use GradientGeneratorParameterKind::*;
    match kind {
        Orientation => "generator-orientation",
        ForceSquareRatio => "imported-force-square",
        UpsizeSmoothing => "imported-smoothing",
        _ => "gradient-mode",
    }
}
fn parameter_axis(
    model: &GradientGenerator<ForegroundError>,
    ix: usize,
) -> (GradientGeneratorParameterKind, i32) {
    let pairs = paired_kinds(model);
    if ix < pairs.len() * 2 {
        (pairs[ix / 2], (ix % 2) as i32)
    } else {
        let offset = ix - pairs.len() * 2;
        let scalars = scalar_kinds(model);
        if offset < scalars.len() {
            (scalars[offset], 0)
        } else {
            (
                choice_kinds(model)
                    .into_iter()
                    .filter(|kind| model.parameter(*kind).is_animatable())
                    .collect::<Vec<_>>()[offset - scalars.len()],
                0,
            )
        }
    }
}
fn axis_label(model: &GradientGenerator<ForegroundError>, ix: usize) -> String {
    let (kind, axis) = parameter_axis(model, ix);
    let parameter = model.parameter(kind);
    if parameter.is_paired() {
        format!("{} {}", parameter.name(), if axis == 0 { "X" } else { "Y" })
    } else {
        parameter.name().into()
    }
}
fn axis_key(model: &GradientGenerator<ForegroundError>, ix: usize) -> String {
    let (kind, axis) = parameter_axis(model, ix);
    let paired_key = match kind {
        Size => Some("size"),
        Location => Some("location"),
        BlockDensity => Some("block-density"),
        _ => None,
    };
    if let Some(key) = paired_key {
        format!("{key}-{}", if axis == 0 { "x" } else { "y" })
    } else {
        format!("scalar-{kind:?}").to_ascii_lowercase()
    }
}
fn paired_input_id(model: &GradientGenerator<ForegroundError>, ix: usize) -> String {
    // Stable IDs belong to the actual parameter and axis, independent of new pairs.
    match parameter_axis(model, ix) {
        (Size, axis) => format!("gradient-value-{axis}"),
        (Location, axis) => format!("gradient-value-{}", axis + 2),
        _ => format!("generator-pair-{}", axis_key(model, ix)),
    }
}
/// Read actual roots without invoking the lazy owner constructor in render/sync.
fn animation_axis(
    model: &GradientGenerator<ForegroundError>,
    ix: usize,
) -> Result<(bool, Option<SharedCurve<ForegroundError>>), String> {
    let (kind, axis) = parameter_axis(model, ix);
    let Some(owner) = model
        .parameter(kind)
        .retained_animation()
        .map_err(|e| e.to_string())?
    else {
        return Ok((false, None));
    };
    let flags = owner
        .flags()
        .ok_or_else(|| "The animation flags are unavailable.".to_owned())?;
    let enabled = flags.value(axis).map_err(|e| e.to_string())?;
    let curves = owner
        .curves()
        .ok_or_else(|| "The animation curves are unavailable.".to_owned())?;
    let curve = curves.value(axis).map_err(|e| e.to_string())?;
    Ok((enabled, curve))
}
/// These mutations run only in submit's engine phase, outside GPUI borrows.
fn set_animation_axis(
    model: &GradientGenerator<ForegroundError>,
    ix: usize,
    enabled: bool,
) -> Result<(), String> {
    let (kind, axis) = parameter_axis(model, ix);
    let property = model.parameter(kind);
    let owner = if enabled {
        property.animation().map_err(|e| e.to_string())?
    } else {
        let Some(owner) = property.retained_animation().map_err(|e| e.to_string())? else {
            return Ok(());
        };
        owner
    };
    if enabled {
        let curves = owner
            .curves()
            .ok_or_else(|| "The animation curves are unavailable.".to_owned())?;
        let is_new = curves.value(axis).map_err(|e| e.to_string())?.is_none();
        owner.ensure_curve(axis).map_err(|e| e.to_string())?;
        // Preserve any existing curve/layers, including an intentionally empty
        // inactive curve. Only a newly created UI curve gets a Linear layer.
        if is_new {
            let curve = curves
                .value(axis)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "The animation curve is unavailable.".to_owned())?;
            let linear = CurveLayerHandle::new(AnalyticCurveKind::Linear);
            curve.insert(0, Some(&linear)).map_err(|e| e.to_string())?;
        }
    }
    owner
        .flags()
        .ok_or_else(|| "The animation flags are unavailable.".to_owned())?
        .set(axis, enabled)
        .map_err(|e| e.to_string())
}

/// One retained generator; activation subscribes only a weak foreground route.
pub struct GradientSourceView {
    model: GradientGenerator<ForegroundError>,
    fields: Vec<Entity<InputState>>,
    synced: Vec<String>,
    scalars: Vec<ScalarDraft>,
    curves: Vec<CurveDraft>,
    time_input: Entity<InputState>,
    time_synced: String,
    preview_time: f64,
    show_animation: bool,
    curve_axis: Option<usize>,
    layer_inspector: crate::graph_document::inspector::Inspector,
    workspace_time: bool,
    input_owner: Option<Arc<crate::input_source::InputOwner>>,
    input_namespace: Option<String>,
    choices: Vec<ChoiceDraft>,
    presentation: Option<Presentation>,
    runtime: Weak<BindingRuntime>,
    error: Option<String>,
    pending: bool,
}
impl GradientSourceView {
    pub(crate) fn new(
        model: GradientGenerator<ForegroundError>,
        runtime: &Arc<BindingRuntime>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.bind_keys([
            gpui_kit::KeyBinding::new("secondary-enter", ApplyGradient, Some("GradientSource")),
            gpui_kit::KeyBinding::new("escape", RevertGradient, Some("GradientSource")),
        ]);
        let values = Self::values(&model);
        let synced: Vec<_> = values.into_iter().map(|v| v.to_string()).collect();
        let fields = synced
            .iter()
            .map(|value| cx.new(|cx| InputState::new(window, cx).default_value(value.clone())))
            .collect();
        let scalars: Vec<_> = scalar_kinds(&model)
            .into_iter()
            .map(|kind| {
                let value = model
                    .parameter(kind)
                    .number()
                    .expect("scalar property")
                    .to_string();
                ScalarDraft {
                    kind,
                    field: cx.new(|cx| InputState::new(window, cx).default_value(value.clone())),
                    synced: value,
                }
            })
            .collect();
        let curves = (0..synced.len()
            + scalars.len()
            + choice_kinds(&model)
                .into_iter()
                .filter(|kind| model.parameter(*kind).is_animatable())
                .count())
            .map(|_| CurveDraft {
                fields: std::array::from_fn(|_| cx.new(|cx| InputState::new(window, cx))),
                synced: std::array::from_fn(|_| String::new()),
            })
            .collect();
        let time_input = cx.new(|cx| InputState::new(window, cx).default_value("0"));
        let choices = choice_kinds(&model)
            .into_iter()
            .map(|kind| {
                let items = Self::choice_items(&model, kind);
                let selected = model
                    .parameter(kind)
                    .index()
                    .ok()
                    .and_then(|v| usize::try_from(v).ok())
                    .filter(|i| *i < items.len())
                    .map(IndexPath::new);
                let state = cx.new(|cx| SelectState::new(items, selected, window, cx));
                let subscription =
                    cx.subscribe_in(&state, window, move |view, _, event, window, cx| {
                        let SelectEvent::Confirm(Some(index)) = event else {
                            return;
                        };
                        if view.presentation.is_none() || view.pending {
                            view.sync(window, cx);
                            return;
                        }
                        view.submit(Edit::Choice(kind, *index), window, cx);
                    });
                ChoiceDraft {
                    kind,
                    state,
                    _subscription: subscription,
                }
            })
            .collect();
        Self {
            model,
            fields,
            synced,
            curves,
            scalars,
            time_input,
            time_synced: "0".into(),
            preview_time: 0.0,
            show_animation: false,
            curve_axis: None,
            layer_inspector: Default::default(),
            workspace_time: false,
            input_owner: None,
            input_namespace: None,
            choices,
            presentation: None,
            runtime: Arc::downgrade(runtime),
            error: None,
            pending: false,
        }
    }
    /// The owning workspace supplies one time for source and all filter inputs.
    /// Drafts, actual parameter handles and activation remain owned by this view.
    pub(crate) fn new_input(
        namespace: Option<String>,
        model: GradientGenerator<ForegroundError>,
        owner: Arc<crate::input_source::InputOwner>,
        runtime: &Arc<BindingRuntime>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self::new(model, runtime, window, cx);
        view.workspace_time = true;
        view.input_namespace = namespace;
        view.input_owner = Some(owner);
        view
    }
    fn control_id(&self, id: impl AsRef<str>) -> String {
        self.input_namespace.as_ref().map_or_else(
            || id.as_ref().to_owned(),
            |prefix| format!("{prefix}-{}", id.as_ref()),
        )
    }
    pub fn model(&self) -> GradientGenerator<ForegroundError> {
        self.model.clone()
    }
    /// Selected normalized preview time; it is presentation state, not a clock.
    pub fn preview_time(&self) -> f64 {
        self.preview_time
    }
    pub(crate) fn set_preview_time(
        &mut self,
        time: f64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.preview_time = time;
        self.sync(window, cx);
    }
    pub(crate) fn is_pending(&self) -> bool {
        self.pending
    }
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    pub(crate) fn preferred_focus(&self, cx: &App) -> gpui_kit::FocusHandle {
        self.fields[0].read(cx).focus_handle(cx)
    }
    fn values(model: &GradientGenerator<ForegroundError>) -> Vec<f64> {
        paired_kinds(model)
            .into_iter()
            .flat_map(|kind| {
                model
                    .parameter(kind)
                    .pair()
                    .expect("registered paired property")
            })
            .collect()
    }
    fn choice_items(
        model: &GradientGenerator<ForegroundError>,
        kind: GradientGeneratorParameterKind,
    ) -> Vec<ChoiceItem> {
        model
            .parameter(kind)
            .options()
            .map(|options| options.entries())
            .unwrap_or_default()
            .into_iter()
            .enumerate()
            .map(|(i, v)| ChoiceItem {
                index: i as i32,
                title: v
                    .as_deref()
                    .map(String::from_utf16_lossy)
                    .unwrap_or_else(|| {
                        if kind == GradientGeneratorParameterKind::Orientation {
                            "Unnamed orientation".into()
                        } else {
                            "Unnamed mode".into()
                        }
                    })
                    .into(),
            })
            .collect()
    }
    /// A validated imported candidate replaces only the actual model. Existing
    /// numeric InputState drafts survive; activation retires the former roots.
    pub(crate) fn replace_imported_model(
        &mut self,
        model: GradientGenerator<ForegroundError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close();
        self.model = model;
        self.sync(window, cx);
    }
    fn text(&self, cx: &App) -> Vec<String> {
        self.fields
            .iter()
            .map(|field| field.read(cx).value().to_string())
            .collect()
    }
    fn replace_drafts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.synced = Self::values(&self.model)
            .into_iter()
            .map(|v| v.to_string())
            .collect();
        for (field, value) in self.fields.iter().zip(&self.synced) {
            field.update(cx, |state, cx| state.set_value(value.clone(), window, cx));
        }
    }
    fn curve_text(&self, ix: usize, cx: &App) -> [String; 2] {
        std::array::from_fn(|axis| self.curves[ix].fields[axis].read(cx).value().to_string())
    }
    fn replace_curve_draft(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let values = animation_axis(&self.model, ix)
            .ok()
            .and_then(|(_, curve)| curve)
            .map(|curve| [curve.minimum().to_string(), curve.maximum().to_string()])
            .unwrap_or_else(|| [String::new(), String::new()]);
        self.curves[ix].synced = values.clone();
        for (field, value) in self.curves[ix].fields.iter().zip(values) {
            field.update(cx, |state, cx| state.set_value(value, window, cx));
        }
    }
    fn replace_time_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.time_synced = self.preview_time.to_string();
        self.time_input.update(cx, |state, cx| {
            state.set_value(self.time_synced.clone(), window, cx)
        });
    }
    /// Synchronize actual committed values only when no numeric draft is dirty.
    /// Programmatic Select synchronization emits no confirmation/write event.
    pub(crate) fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.text(cx) == self.synced {
            self.replace_drafts(window, cx);
        }
        for ix in 0..self.curves.len() {
            if self.curve_text(ix, cx) == self.curves[ix].synced {
                self.replace_curve_draft(ix, window, cx);
            }
        }
        if self.time_input.read(cx).value() == self.time_synced {
            self.replace_time_draft(window, cx);
        }
        for scalar in &mut self.scalars {
            if scalar.field.read(cx).value() == scalar.synced {
                scalar.synced = self
                    .model
                    .parameter(scalar.kind)
                    .number()
                    .expect("scalar property")
                    .to_string();
                scalar.field.update(cx, |state, cx| {
                    state.set_value(scalar.synced.clone(), window, cx)
                });
            }
        }
        for choice in &self.choices {
            let items = Self::choice_items(&self.model, choice.kind);
            let selected = self
                .model
                .parameter(choice.kind)
                .index()
                .ok()
                .and_then(|v| usize::try_from(v).ok())
                .filter(|i| *i < items.len())
                .map(IndexPath::new);
            choice.state.update(cx, |state, cx| {
                state.set_items(items, window, cx);
                state.set_selected_index(selected, window, cx);
            });
        }
        self.sync_layer_selection(window, cx);
        cx.notify();
    }
    pub(crate) fn rebind_input_owner(&mut self, owner: Arc<crate::input_source::InputOwner>) {
        self.close();
        self.input_owner = Some(owner);
    }
    pub(crate) fn set_active(&mut self, active: bool) {
        self.close();
        if !active {
            return;
        }
        let marker = Arc::new(AtomicBool::new(true));
        let weak_marker = Arc::downgrade(&marker);
        let runtime = self.runtime.clone();
        let input_owner = self.input_owner.as_ref().map(Arc::downgrade);
        let listener = GradientGeneratorChangeHandler::new(move |_, _| {
            if input_owner
                .as_ref()
                .is_some_and(|o| !o.upgrade().is_some_and(|o| o.admits(&weak_marker)))
            {
                return Ok(());
            }
            if !weak_marker
                .upgrade()
                .is_some_and(|m| m.load(Ordering::Acquire))
            {
                return Ok(());
            }
            runtime
                .upgrade()
                .ok_or(ForegroundError::OwnerClosed)?
                .request_effect_presentation(weak_marker.clone())
        });
        for parameter in self.model.parameters() {
            parameter.add_changed_handler(&listener);
        }
        self.presentation = Some(Presentation {
            active: marker,
            listener,
        });
    }
    pub(crate) fn close(&mut self) {
        self.pending = false;
        if let Some(old) = self.presentation.take() {
            old.active.store(false, Ordering::Release);
            for parameter in self.model.parameters() {
                parameter.remove_changed_handler(&old.listener);
            }
        }
    }
    fn submit(&mut self, edit: Edit, window: &mut Window, cx: &mut Context<Self>) {
        if self.input_owner.as_ref().is_some_and(|o| !o.is_current()) {
            if let Some(presentation) = &self.presentation {
                presentation.active.store(false, Ordering::Release);
            }
            return;
        }
        if self.pending {
            return;
        }
        let Some(presentation) = &self.presentation else {
            return;
        };
        let active = Arc::downgrade(&presentation.active);
        let runtime = self.runtime.clone();
        let model = self.model.clone();
        let input_owner = self.input_owner.clone();
        let handle = window.window_handle();
        let submitted = self.text(cx);
        let curve_submitted: Vec<[String; 2]> = (0..self.curves.len())
            .map(|ix| self.curve_text(ix, cx))
            .collect();
        let scalar_submitted: Vec<_> = self
            .scalars
            .iter()
            .map(|s| s.field.read(cx).value().to_string())
            .collect();
        let time_submitted = self.time_input.read(cx).value().to_string();
        let applies_pairs = matches!(&edit, Edit::Values { .. });
        let applied_bounds: Vec<usize> = match &edit {
            Edit::Bounds(values) => values.iter().map(|(ix, _)| *ix).collect(),
            Edit::GradientCurve(edit) => edit.applied_bounds().to_vec(),
            _ => Vec::new(),
        };
        let new_time = if let Edit::Time(time) = &edit {
            Some(*time)
        } else {
            None
        };
        // Curve/array writes intentionally emit no source notification.
        let silent_edit = matches!(
            &edit,
            Edit::Animation { .. } | Edit::Bounds(_) | Edit::Time(_)
        );
        let gradient_selection = self.layer_inspector.layer.clone();
        self.pending = true;
        cx.notify();
        cx.spawn(async move |owner, cx| {
            let alive = cx
                .update_window(handle, |_, _, cx| owner.read_with(cx, |_, _| ()))
                .is_ok_and(|r| r.is_ok());
            if !alive
                || !active.upgrade().is_some_and(|m| m.load(Ordering::Acquire))
                || input_owner.as_ref().is_some_and(|o| !o.admits(&active))
            {
                return;
            }
            // Real engine callbacks run without any Entity/App/Window borrow.
            let mut layer_selection = None;
            let result: Result<(), String> = match edit {
                Edit::Values {
                    pairs: values,
                    scalars,
                } => (|| {
                    for (kind, value) in values {
                        if input_owner.as_ref().is_some_and(|o| !o.admits(&active)) {
                            return Ok(());
                        }
                        model.parameter(kind).set_pair(value)?;
                    }
                    for (kind, value) in scalars {
                        if input_owner.as_ref().is_some_and(|o| !o.admits(&active)) {
                            return Ok(());
                        }
                        model.parameter(kind).set_number(value)?;
                    }
                    Ok::<_, mycela_engine::PointwiseError<ForegroundError>>(())
                })()
                .map_err(|e| e.to_string()),
                Edit::Choice(kind, index) => model
                    .parameter(kind)
                    .set_index(index)
                    .map_err(|e| e.to_string()),
                Edit::Animation { axis, enabled } => set_animation_axis(&model, axis, enabled),
                Edit::Bounds(values) => (|| {
                    for (ix, [lower, upper]) in values {
                        let curve = animation_axis(&model, ix)?.1.ok_or_else(|| {
                            "The curve changed before its bounds could be applied.".to_owned()
                        })?;
                        curve.set_minimum(lower);
                        curve.set_maximum(upper);
                    }
                    Ok(())
                })(),
                Edit::GradientCurve(edit) => edit
                    .publish(gradient_selection, || {
                        active.upgrade().is_some_and(|m| m.load(Ordering::Acquire))
                            && input_owner.as_ref().is_none_or(|o| o.admits(&active))
                    })
                    .map(|selection| layer_selection = Some(selection)),
                Edit::Time(_) => Ok(()),
            };
            if !active.upgrade().is_some_and(|m| m.load(Ordering::Acquire))
                || input_owner.as_ref().is_some_and(|o| !o.admits(&active))
            {
                return;
            }
            let delivered = cx.update_window(handle, |_, window, cx| {
                owner.update(cx, |view, cx| {
                    if input_owner.as_ref().is_some_and(|o| !o.admits(&active)) {
                        return;
                    }
                    view.pending = false;
                    view.error = result.err();
                    if let Some(selection) = layer_selection
                        && let Err(error) = view.set_layer_selection(selection, window, cx)
                    {
                        view.error = Some(error);
                    }
                    if applies_pairs && view.error.is_none() && view.text(cx) == submitted {
                        view.replace_drafts(window, cx);
                    }
                    if applies_pairs && view.error.is_none() {
                        for (scalar, submitted) in view.scalars.iter_mut().zip(&scalar_submitted) {
                            if scalar.field.read(cx).value() == *submitted {
                                scalar.synced = view
                                    .model
                                    .parameter(scalar.kind)
                                    .number()
                                    .expect("scalar property")
                                    .to_string();
                                scalar.field.update(cx, |state, cx| {
                                    state.set_value(scalar.synced.clone(), window, cx)
                                });
                            }
                        }
                    }
                    if view.error.is_none() {
                        for ix in applied_bounds {
                            if view.curve_text(ix, cx) == curve_submitted[ix] {
                                view.replace_curve_draft(ix, window, cx);
                            }
                        }
                    }
                    if let Some(time) = new_time {
                        view.preview_time = time;
                        if view.time_input.read(cx).value() == time_submitted {
                            view.replace_time_draft(window, cx);
                        }
                    }
                    view.sync(window, cx);
                })
            });
            // Time is published before the presentation request. Retirement of
            // this activation drops late jobs; partial engine errors still get
            // a fresh presentation without fabricating source callbacks.
            if silent_edit
                && delivered.is_ok_and(|result| result.is_ok())
                && active.upgrade().is_some_and(|m| m.load(Ordering::Acquire))
                && input_owner.as_ref().is_none_or(|o| o.admits(&active))
            {
                if let Some(runtime) = runtime.upgrade() {
                    if let Err(error) = runtime.request_effect_presentation(active.clone()) {
                        let _ = cx.update_window(handle, |_, _, cx| {
                            owner.update(cx, |view, cx| {
                                if input_owner.as_ref().is_some_and(|o| !o.admits(&active)) {
                                    return;
                                }
                                if view.error.is_none() {
                                    view.error = Some(error.to_string());
                                }
                                cx.notify();
                            })
                        });
                    }
                }
            }
        })
        .detach();
    }
    fn apply_time(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending || self.presentation.is_none() {
            return;
        }
        let parsed = self.time_input.read(cx).value().parse::<f64>();
        match parsed {
            Ok(time) if time.is_finite() && (0.0..=1.0).contains(&time) => {
                self.submit(Edit::Time(time), window, cx)
            }
            _ => {
                self.error = Some("Preview time needs a number from 0 to 1.".into());
                cx.notify();
            }
        }
    }
    fn apply_curves(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending || self.presentation.is_none() {
            return;
        }
        if self.model.kind() == mycela_engine::ImageGeneratorKind::Gradient {
            let edit = self.stage_curve_fields(cx);
            match edit {
                Ok(Some(edit)) => self.submit(Edit::GradientCurve(edit), window, cx),
                Ok(None) => (),
                Err(error) => {
                    self.error = Some(error);
                    cx.notify();
                }
            }
            return;
        }
        let mut values = Vec::new();
        for ix in 0..self.curves.len() {
            let label = axis_label(&self.model, ix);
            match animation_axis(&self.model, ix) {
                Ok((_, Some(_))) => {
                    let text = self.curve_text(ix, cx);
                    let mut bounds = [0.; 2];
                    for axis in 0..2 {
                        match text[axis].parse::<f32>() {
                            Ok(value) => bounds[axis] = value,
                            Err(_) => {
                                self.error = Some(format!(
                                    "{label} {} needs a number.",
                                    if axis == 0 { "lower" } else { "upper" }
                                ));
                                cx.notify();
                                return;
                            }
                        }
                    }
                    values.push((ix, bounds));
                }
                Ok((_, None)) => (),
                Err(error) => {
                    self.error = Some(error);
                    cx.notify();
                    return;
                }
            }
        }
        // Parse all drafts before the first write; actual setters are sequential.
        self.submit(Edit::Bounds(values), window, cx);
    }
    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending || self.presentation.is_none() {
            return;
        }
        let text = self.text(cx);
        let mut values = vec![0.; text.len()];
        for i in 0..text.len() {
            match text[i].parse::<f64>() {
                Ok(v) => values[i] = v,
                Err(_) => {
                    self.error = Some(format!(
                        "{} needs a number. Correct it and apply again.",
                        axis_label(&self.model, i)
                    ));
                    cx.notify();
                    return;
                }
            }
        }
        // Parsing is transactional; the real property setters remain sequential.
        let mut scalars = Vec::new();
        for scalar in &self.scalars {
            match scalar.field.read(cx).value().parse::<f64>() {
                Ok(value) => scalars.push((scalar.kind, value)),
                Err(_) => {
                    self.error = Some(format!(
                        "{} needs a number. Correct it and apply again.",
                        self.model.parameter(scalar.kind).name()
                    ));
                    cx.notify();
                    return;
                }
            }
        }
        self.submit(
            Edit::Values {
                pairs: paired_kinds(&self.model)
                    .into_iter()
                    .zip(values.chunks_exact(2).map(|v| [v[0], v[1]]))
                    .collect(),
                scalars,
            },
            window,
            cx,
        );
    }
    fn render_animation(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let theme = cx.theme().clone();
        let content = div()
            .v_flex()
            .gap_3()
            .when(!self.workspace_time, |this| {
                this.child(
                    div()
                        .v_flex()
                        .gap_2()
                        .child(
                            Field::new().label("Preview time").child(
                                div()
                                    .h_flex()
                                    .gap_2()
                                    .min_w_0()
                                    .child(
                                        Input::new(&self.time_input)
                                            .id(self.control_id("gradient-preview-time"))
                                            .aria_label("Preview time")
                                            .disabled(self.pending || self.presentation.is_none())
                                            .flex_1()
                                            .min_w_0(),
                                    )
                                    .child(
                                        Button::new(self.control_id("set-gradient-time"))
                                            .label("Set time")
                                            .disabled(self.pending || self.presentation.is_none())
                                            .on_click(cx.listener(|view, _, window, cx| {
                                                view.apply_time(window, cx)
                                            })),
                                    ),
                            ),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child("From 0 to 1"),
                        ),
                )
            })
            .child(
                div()
                    .v_flex()
                    .gap_3()
                    .border_t_1()
                    .border_color(theme.border)
                    .pt_3()
                    .child(div().text_sm().font_medium().child("Animation"))
                    .children((0..self.curves.len()).map(|ix| {
                        let status = animation_axis(&self.model, ix);
                        let enabled = status.as_ref().is_ok_and(|(enabled, _)| *enabled);
                        let has_curve = status.as_ref().is_ok_and(|(_, curve)| curve.is_some());
                        div()
                            .v_flex()
                            .gap_2()
                            .child(
                                Checkbox::new(
                                    self.control_id(format!(
                                        "animate-{}",
                                        axis_key(&self.model, ix)
                                    )),
                                )
                                .label(format!("Animate {}", axis_label(&self.model, ix)))
                                .checked(enabled)
                                .disabled(
                                    self.pending || self.presentation.is_none() || status.is_err(),
                                )
                                .on_change(cx.listener(
                                    move |view, enabled, window, cx| {
                                        view.toggle_source_animation(ix, *enabled, window, cx);
                                    },
                                )),
                            )
                            .child(Form::new().columns(2).children((0..2).map(|axis| {
                                let bound = if axis == 0 { "Lower" } else { "Upper" };
                                Field::new().label(bound).child(
                                    Input::new(&self.curves[ix].fields[axis])
                                        .id(self.control_id(format!(
                                            "curve-{}-{}",
                                            axis_key(&self.model, ix),
                                            bound.to_lowercase()
                                        )))
                                        .aria_label(format!(
                                            "{} curve {}",
                                            axis_label(&self.model, ix),
                                            bound.to_lowercase()
                                        ))
                                        .disabled(
                                            self.pending
                                                || self.presentation.is_none()
                                                || !has_curve,
                                        ),
                                )
                            })))
                            .when_some(status.err(), |this, error| {
                                this.child(div().text_sm().text_color(theme.danger).child(error))
                            })
                            .when(
                                self.model.kind() == mycela_engine::ImageGeneratorKind::Gradient
                                    && enabled
                                    && has_curve,
                                |this| this.child(self.curve_layer_section(ix, cx)),
                            )
                    }))
                    .child(
                        Button::new(self.control_id("apply-gradient-curves"))
                            .label("Apply curves")
                            .disabled(
                                self.pending
                                    || !(0..self.curves.len()).any(|ix| {
                                        animation_axis(&self.model, ix)
                                            .is_ok_and(|(_, curve)| curve.is_some())
                                    }),
                            )
                            .on_click(
                                cx.listener(|view, _, window, cx| view.apply_curves(window, cx)),
                            ),
                    ),
            );
        Collapsible::new()
            .open(self.show_animation)
            .child(
                Button::new(self.control_id("toggle-gradient-animation"))
                    .label(if self.show_animation {
                        "Hide animation"
                    } else {
                        "Show animation"
                    })
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.show_animation = !view.show_animation;
                        cx.notify();
                    })),
            )
            .content(content)
            .into_any_element()
    }
    fn revert(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending {
            return;
        }
        self.error = None;
        self.replace_drafts(window, cx);
        for scalar in &mut self.scalars {
            scalar.synced = self
                .model
                .parameter(scalar.kind)
                .number()
                .expect("scalar property")
                .to_string();
            scalar.field.update(cx, |state, cx| {
                state.set_value(scalar.synced.clone(), window, cx)
            });
        }
        for ix in 0..self.curves.len() {
            self.replace_curve_draft(ix, window, cx);
        }
        if let Some(layer) = self.layer_inspector.layer.clone() {
            self.layer_inspector.close_layer();
            if layer.validate(None).is_ok() {
                self.error = self.set_layer_selection(Some(layer), window, cx).err();
            }
        }
        self.replace_time_draft(window, cx);
        cx.notify();
    }
}
impl Drop for GradientSourceView {
    fn drop(&mut self) {
        self.close();
    }
}
impl Render for GradientSourceView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        div()
            .id(self.control_id("gradient-source"))
            .key_context("GradientSource")
            .v_flex()
            .gap_3()
            .on_action(cx.listener(|view, _: &ApplyGradient, window, cx| {
                view.apply(window, cx);
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|view, _: &RevertGradient, window, cx| {
                view.revert(window, cx);
                cx.stop_propagation();
            }))
            .children(self.choices.iter().map(|choice| {
                let parameter = self.model.parameter(choice.kind);
                Field::new().label(parameter.name()).child(
                    Select::new(&choice.state)
                        .id(self.control_id(choice_id(choice.kind)))
                        .accessibility_label(match choice.kind {
                            GradientGeneratorParameterKind::Orientation => {
                                "Bricks & Boards orientation".to_owned()
                            }
                            GradientGeneratorParameterKind::ForceSquareRatio
                            | GradientGeneratorParameterKind::UpsizeSmoothing => {
                                parameter.name().to_owned()
                            }
                            _ if self.model.kind()
                                == mycela_engine::ImageGeneratorKind::Gradient =>
                            {
                                "Gradient mode".to_owned()
                            }
                            _ => format!("{} mode", self.model.kind().name()),
                        })
                        .disabled(self.pending || self.presentation.is_none())
                        .placeholder("Unavailable option")
                        .w_full(),
                )
            }))
            .child(
                Form::new()
                    .columns(2)
                    .children(self.fields.iter().enumerate().map(|(i, field)| {
                        Field::new().label(axis_label(&self.model, i)).child(
                            Input::new(field)
                                .id(self.control_id(paired_input_id(&self.model, i)))
                                .aria_label(axis_label(&self.model, i))
                                .disabled(self.presentation.is_none()),
                        )
                    })),
            )
            .child(
                Form::new()
                    .columns(2)
                    .children(self.scalars.iter().map(|scalar| {
                        Field::new()
                            .label(self.model.parameter(scalar.kind).name())
                            .child(
                                Input::new(&scalar.field)
                                    .id(self.control_id(
                                        format!("generator-scalar-{:?}", scalar.kind)
                                            .to_ascii_lowercase(),
                                    ))
                                    .aria_label(self.model.parameter(scalar.kind).name())
                                    .disabled(self.pending || self.presentation.is_none()),
                            )
                    })),
            )
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(
                        Button::new(self.control_id("apply-gradient"))
                            .label("Apply")
                            .primary()
                            .disabled(self.pending || self.presentation.is_none())
                            .on_click(cx.listener(|view, _, window, cx| view.apply(window, cx))),
                    )
                    .child(
                        Button::new(self.control_id("revert-gradient"))
                            .label("Revert edits")
                            .disabled(self.pending || self.presentation.is_none())
                            .on_click(cx.listener(|view, _, window, cx| view.revert(window, cx))),
                    ),
            )
            .when(
                self.model.kind() != mycela_engine::ImageGeneratorKind::ImportedImage,
                |this| this.child(self.render_animation(cx)),
            )
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .id(self.control_id("gradient-error"))
                        .role(gpui_kit::Role::Alert)
                        .aria_label(error.clone())
                        .test_support()
                        .text_sm()
                        .text_color(theme.danger)
                        .child(error),
                )
            })
    }
}
