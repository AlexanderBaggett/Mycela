//! Selected spatial property editor. Text is a draft; the pair, custom object,
//! coefficient array and animation roots remain the engine's actual authority.
use crate::{ForegroundError, color_binding_effects::BindingRuntime};
use gpui_kit::base::{Disableable, StyledExt, TestSupportExt};
use gpui_kit::component::{
    ActiveTheme, Sizable,
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    form::{Field, Form},
    input::{Input, InputState},
    menu::{DropdownMenu, PopupMenuItem},
};
use gpui_kit::{
    AnyWindowHandle, App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable,
    IntoElement, Render, Window, div, prelude::*,
};
use mycela_engine::{
    AnalyticCurveKind, CurveLayerHandle, MatrixCoefficients, PointwiseFilter, PointwiseKind,
    PointwiseMatrix, PointwiseParameter, PointwiseParameterValue, SharedCurve,
};
use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, Ordering},
};

gpui_kit::actions!(spatial_filter, [ApplySpatial, RevertSpatial]);
const OKLAB_COMPONENTS: [(i32, &str); 4] = [(0, "lightness"), (1, "a"), (2, "b"), (3, "opacity")];
#[derive(Clone)]
pub(crate) struct SampleTime {
    pub(crate) value: f64,
    pub(crate) activation: Weak<AtomicBool>,
}

#[derive(Clone, Copy)]
enum FieldKey {
    X,
    Y,
    Angle,
    Radius,
    Iterations,
    Coefficient(usize),
    Parameter(i32, Option<usize>),
}
impl FieldKey {
    fn custom(self) -> bool {
        matches!(self, Self::Angle | Self::Radius | Self::Coefficient(_))
    }
}
struct Draft {
    key: FieldKey,
    label: String,
    id: String,
    input: Entity<InputState>,
    synced: String,
}
struct CurveDraft {
    parameter: i32,
    axis: i32,
    key: &'static str,
    fields: [Entity<InputState>; 2],
    synced: [String; 2],
    curve: Option<SharedCurve<ForegroundError>>,
}
#[derive(Clone)]
enum Authority {
    Unavailable(String),
    Pixels(PointwiseParameter<ForegroundError>),
    Parameters,
    Matrix {
        parameter: PointwiseParameter<ForegroundError>,
        object: Option<PointwiseMatrix<ForegroundError>>,
        coefficients: Option<MatrixCoefficients>,
    },
}
impl Authority {
    fn current(&self) -> bool {
        match self {
            Self::Unavailable(_) => false,
            Self::Pixels(_) | Self::Parameters => true,
            Self::Matrix {
                parameter,
                object,
                coefficients,
            } => {
                let Ok(current) = parameter.matrix() else {
                    return false;
                };
                let Some((current, object)) = current.as_ref().zip(object.as_ref()) else {
                    return false;
                };
                if !current.same_reference(object) {
                    return false;
                }
                match (current.payload().coefficients(), coefficients) {
                    (Some(a), Some(b)) => a.same_reference(b),
                    (None, None) => true,
                    _ => false,
                }
            }
        }
    }
    fn editable(&self) -> bool {
        self.current()
            && match self {
                Self::Unavailable(_) => false,
                Self::Pixels(_) | Self::Parameters => true,
                Self::Matrix { coefficients, .. } => {
                    coefficients.as_ref().is_some_and(|a| a.len() >= 9)
                }
            }
    }
    fn value(
        &self,
        key: FieldKey,
        model: &PointwiseFilter<ForegroundError>,
    ) -> Result<f64, String> {
        match (self, key) {
            (Self::Parameters, FieldKey::Parameter(index, axis)) => {
                let p = model.parameter(index).map_err(|e| e.to_string())?;
                match axis {
                    Some(axis) => p.pair().map(|v| v[axis]).map_err(|e| e.to_string()),
                    None => Ok(p.number()),
                }
            }
            (Self::Pixels(p), FieldKey::X) => p.pair().map(|v| v[0]).map_err(|e| e.to_string()),
            (Self::Pixels(p), FieldKey::Y) => p.pair().map(|v| v[1]).map_err(|e| e.to_string()),
            (
                Self::Matrix {
                    object: Some(m), ..
                },
                FieldKey::Angle,
            ) => Ok(m.payload().angle()),
            (
                Self::Matrix {
                    object: Some(m), ..
                },
                FieldKey::Radius,
            ) => Ok(m.payload().radius()),
            (
                Self::Matrix {
                    coefficients: Some(a),
                    ..
                },
                FieldKey::Coefficient(i),
            ) => a.read(i).map_err(|e| e.to_string()),
            (_, FieldKey::Iterations) => model
                .parameter(1)
                .map(|p| p.number())
                .map_err(|e| e.to_string()),
            _ => Err(
                "These filter settings are unavailable. Reload to inspect the current values."
                    .into(),
            ),
        }
    }
}
type CurveBoundsEdit = (i32, i32, SharedCurve<ForegroundError>, [Option<f32>; 2]);
enum Edit {
    Values(Vec<(FieldKey, f64, bool)>),
    Choice(i32, i32),
    Animation(i32, i32, bool),
    Bounds(Vec<CurveBoundsEdit>),
}

pub(crate) struct SpatialFilterView {
    id: u64,
    model: PointwiseFilter<ForegroundError>,
    authority: Authority,
    drafts: Vec<Draft>,
    curves: Vec<CurveDraft>,
    time: Entity<InputState>,
    time_synced: String,
    focus: FocusHandle,
    window: AnyWindowHandle,
    active: Arc<AtomicBool>,
    runtime: Weak<BindingRuntime>,
    pending: bool,
    error: Option<String>,
}
impl EventEmitter<SampleTime> for SpatialFilterView {}
impl SpatialFilterView {
    pub(crate) fn new(
        id: u64,
        model: PointwiseFilter<ForegroundError>,
        sample_time: f64,
        runtime: &Arc<BindingRuntime>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.bind_keys([
            gpui_kit::KeyBinding::new("secondary-enter", ApplySpatial, Some("SpatialFilter")),
            gpui_kit::KeyBinding::new("escape", RevertSpatial, Some("SpatialFilter")),
        ]);
        let authority = Self::authority(&model).unwrap_or_else(Authority::Unavailable);
        let mut view = Self {
            id,
            model,
            authority,
            drafts: Vec::new(),
            curves: Vec::new(),
            time: cx.new(|cx| InputState::new(window, cx).default_value(sample_time.to_string())),
            time_synced: sample_time.to_string(),
            focus: cx.focus_handle().tab_stop(true),
            window: window.window_handle(),
            active: Arc::new(AtomicBool::new(true)),
            runtime: Arc::downgrade(runtime),
            pending: false,
            error: None,
        };
        view.load(window, cx);
        view
    }
    fn authority(model: &PointwiseFilter<ForegroundError>) -> Result<Authority, String> {
        let p = model.parameter(0).map_err(|e| e.to_string())?;
        if uses_parameter_fields(model.kind()) {
            Ok(Authority::Parameters)
        } else if model.kind() == PointwiseKind::Pixelate {
            Ok(Authority::Pixels(p))
        } else {
            let object = p.matrix().map_err(|e| e.to_string())?;
            let coefficients = object.as_ref().and_then(|m| m.payload().coefficients());
            Ok(Authority::Matrix {
                parameter: p,
                object,
                coefficients,
            })
        }
    }
    fn animation_axis(
        &self,
        parameter: i32,
        axis: i32,
    ) -> Result<(bool, Option<SharedCurve<ForegroundError>>), String> {
        animation_axis(
            &self.model.parameter(parameter).map_err(|e| e.to_string())?,
            axis,
        )
    }
    pub(crate) fn close(&mut self) {
        self.active.store(false, Ordering::Release);
    }
    pub(crate) fn preferred_focus(&self, cx: &App) -> FocusHandle {
        self.drafts
            .iter()
            .find(|d| !d.key.custom() || self.authority.editable())
            .map_or_else(|| self.focus.clone(), |d| d.input.read(cx).focus_handle(cx))
    }
    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.drafts.clear();
        self.curves.clear();
        let prefix = match self.model.kind() {
            PointwiseKind::Pixelate => "pixelate",
            PointwiseKind::SmartBlur => "smart",
            PointwiseKind::Smear => "smear",
            PointwiseKind::VariableBlur => "variable",
            PointwiseKind::OklabColor => "oklab",
            _ => "matrix",
        };
        let mut fields = if uses_parameter_fields(self.model.kind()) {
            let specs = match self.model.kind() {
                PointwiseKind::SmartBlur => vec![(0, None, "radius"), (1, None, "threshold")],
                PointwiseKind::Smear => vec![
                    (0, Some(0), "size-x"),
                    (0, Some(1), "size-y"),
                    (1, None, "samples"),
                ],
                PointwiseKind::VariableBlur => vec![(0, None, "radius")],
                PointwiseKind::OklabColor => OKLAB_COMPONENTS
                    .map(|(index, key)| (index, None, key))
                    .to_vec(),
                _ => Vec::new(),
            };
            specs
                .iter()
                .map(|(parameter, axis, key)| {
                    let label = self
                        .model
                        .parameter(*parameter)
                        .map(|p| String::from_utf16_lossy(&p.name()))
                        .unwrap_or_else(|_| "Unavailable parameter".into());
                    let label = axis.map_or(label.clone(), |axis| {
                        format!("{label} {}", if axis == 0 { "X" } else { "Y" })
                    });
                    (
                        FieldKey::Parameter(*parameter, *axis),
                        label,
                        (*key).to_owned(),
                    )
                })
                .collect()
        } else if prefix == "pixelate" {
            vec![
                (FieldKey::X, "Pixels X".to_owned(), "pixels-x".to_owned()),
                (FieldKey::Y, "Pixels Y".to_owned(), "pixels-y".to_owned()),
            ]
        } else {
            vec![
                (FieldKey::Angle, "Angle (degrees)".into(), "angle".into()),
                (FieldKey::Radius, "Radius".into(), "radius".into()),
                (
                    FieldKey::Iterations,
                    "Iterations".into(),
                    "iterations".into(),
                ),
            ]
        };
        if prefix == "matrix" {
            fields.extend((0..9).map(|i| {
                (
                    FieldKey::Coefficient(i),
                    if i == 0 {
                        "Center · tap 0".into()
                    } else {
                        format!("Tap {i}")
                    },
                    format!("coefficient-{i}"),
                )
            }));
        }
        for (key, label, suffix) in fields {
            let value = self
                .authority
                .value(key, &self.model)
                .map(|v| v.to_string())
                .unwrap_or_default();
            self.drafts.push(Draft {
                key,
                label,
                id: format!("{prefix}-{}-{suffix}", self.id),
                input: cx.new(|cx| InputState::new(window, cx).default_value(value.clone())),
                synced: value,
            });
        }
        let keys: Vec<(i32, i32, &'static str)> = match self.model.kind() {
            PointwiseKind::Pixelate => vec![(0, 0, "pixels-x"), (0, 1, "pixels-y")],
            PointwiseKind::SmartBlur => vec![(0, 0, "radius"), (1, 0, "threshold")],
            PointwiseKind::Smear => vec![(0, 0, "size-x"), (0, 1, "size-y"), (1, 0, "samples")],
            PointwiseKind::VariableBlur => vec![(0, 0, "radius")],
            PointwiseKind::OklabColor => OKLAB_COMPONENTS
                .map(|(index, key)| (index, 0, key))
                .to_vec(),
            _ => vec![(1, 0, "iterations")],
        };
        for (parameter, axis, key) in keys {
            let curve = self.animation_axis(parameter, axis).ok().and_then(|v| v.1);
            let synced = curve.as_ref().map_or([String::new(), String::new()], |c| {
                [c.minimum().to_string(), c.maximum().to_string()]
            });
            let fields = std::array::from_fn(|i| {
                cx.new(|cx| InputState::new(window, cx).default_value(synced[i].clone()))
            });
            self.curves.push(CurveDraft {
                parameter,
                axis,
                key,
                fields,
                synced,
                curve,
            });
        }
        cx.notify();
    }
    pub(crate) fn set_sample_time(
        &mut self,
        time: f64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.time.read(cx).value() == self.time_synced {
            self.time_synced = time.to_string();
            self.time.update(cx, |s, cx| {
                s.set_value(self.time_synced.clone(), window, cx)
            });
        }
        cx.notify();
    }
    pub(crate) fn refresh_controls(&mut self, cx: &mut Context<Self>) {
        let handle = self.window;
        let active = Arc::downgrade(&self.active);
        cx.spawn(async move |owner, cx| {
            let _ = cx.update_window(handle, |_, window, cx| {
                let _ = owner.update(cx, |view, cx| {
                    if active.upgrade().is_some_and(|a| a.load(Ordering::Acquire)) && !view.pending
                    {
                        view.sync(window, cx);
                    }
                });
            });
        })
        .detach();
        cx.notify();
    }
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let custom_available = self.authority.editable();
        for draft in &mut self.drafts {
            if (draft.key.custom() && !custom_available)
                || draft.input.read(cx).value() != draft.synced
            {
                continue;
            }
            let Ok(value) = self.authority.value(draft.key, &self.model) else {
                continue;
            };
            draft.synced = value.to_string();
            draft
                .input
                .update(cx, |s, cx| s.set_value(draft.synced.clone(), window, cx));
        }
        for ix in 0..self.curves.len() {
            let actual = self
                .animation_axis(self.curves[ix].parameter, self.curves[ix].axis)
                .ok()
                .and_then(|v| v.1);
            let old = &mut self.curves[ix];
            let same = match (&old.curve, &actual) {
                (None, None) => true,
                (Some(a), Some(b)) => a.same_reference(b),
                _ => false,
            };
            let clean: [bool; 2] =
                std::array::from_fn(|i| old.fields[i].read(cx).value() == old.synced[i]);
            if same || clean.iter().all(|v| *v) {
                old.curve = actual;
                let values = old
                    .curve
                    .as_ref()
                    .map_or([String::new(), String::new()], |c| {
                        [c.minimum().to_string(), c.maximum().to_string()]
                    });
                for i in 0..2 {
                    if clean[i] {
                        old.synced[i] = values[i].clone();
                        old.fields[i]
                            .update(cx, |s, cx| s.set_value(old.synced[i].clone(), window, cx));
                    }
                }
            } else {
                self.error =
                    Some("An animation curve changed. Reload before applying its bounds.".into());
            }
        }
        cx.notify();
    }
    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending {
            return;
        }
        match Self::authority(&self.model) {
            Ok(a) => {
                self.authority = a;
                self.error = None;
                self.load(window, cx);
                window.focus(&self.preferred_focus(cx), cx);
            }
            Err(e) => self.error = Some(e),
        }
        cx.notify();
    }
    fn revert(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending {
            return;
        }
        self.error = None;
        let custom_available = self.authority.editable();
        for draft in &mut self.drafts {
            if draft.key.custom() && !custom_available {
                continue;
            }
            let Ok(value) = self.authority.value(draft.key, &self.model) else {
                continue;
            };
            draft.synced = value.to_string();
            draft
                .input
                .update(cx, |s, cx| s.set_value(draft.synced.clone(), window, cx));
        }
        for ix in 0..self.curves.len() {
            let curve = self
                .animation_axis(self.curves[ix].parameter, self.curves[ix].axis)
                .ok()
                .and_then(|v| v.1);
            let draft = &mut self.curves[ix];
            draft.curve = curve;
            draft.synced = draft
                .curve
                .as_ref()
                .map_or([String::new(), String::new()], |c| {
                    [c.minimum().to_string(), c.maximum().to_string()]
                });
            for i in 0..2 {
                draft.fields[i]
                    .update(cx, |s, cx| s.set_value(draft.synced[i].clone(), window, cx));
            }
        }
        cx.notify();
    }
    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync(window, cx);
        let custom_available = self.authority.editable();
        let parsed = self
            .drafts
            .iter()
            .filter(|d| !d.key.custom() || custom_available)
            .map(|d| {
                d.input
                    .read(cx)
                    .value()
                    .parse::<f64>()
                    .map(|v| (d.key, v, d.input.read(cx).value() != d.synced))
                    .map_err(|_| format!("{} needs a number. Correct it and apply again.", d.label))
            })
            .collect::<Result<Vec<_>, _>>();
        match parsed {
            Ok(values) => self.submit(Edit::Values(values), window, cx),
            Err(e) => {
                self.error = Some(e);
                cx.notify();
            }
        }
    }
    fn apply_bounds(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync(window, cx);
        let parsed = (|| {
            let mut values = Vec::new();
            for draft in &self.curves {
                let Some(curve) = &draft.curve else { continue };
                let a = draft.fields[0]
                    .read(cx)
                    .value()
                    .parse::<f32>()
                    .map_err(|_| "Curve lower output needs a number.")?;
                let b = draft.fields[1]
                    .read(cx)
                    .value()
                    .parse::<f32>()
                    .map_err(|_| "Curve upper output needs a number.")?;
                values.push((
                    draft.parameter,
                    draft.axis,
                    curve.clone(),
                    [
                        (draft.fields[0].read(cx).value() != draft.synced[0]).then_some(a),
                        (draft.fields[1].read(cx).value() != draft.synced[1]).then_some(b),
                    ],
                ));
            }
            Ok::<_, String>(values)
        })();
        match parsed {
            Ok(v) => self.submit(Edit::Bounds(v), window, cx),
            Err(e) => {
                self.error = Some(e);
                cx.notify();
            }
        }
    }
    fn apply_time(&mut self, cx: &mut Context<Self>) {
        match self.time.read(cx).value().parse::<f64>() {
            Ok(t) if t.is_finite() && (0. ..=1.).contains(&t) => {
                self.time_synced = self.time.read(cx).value().to_string();
                self.error = None;
                cx.emit(SampleTime {
                    value: t,
                    activation: Arc::downgrade(&self.active),
                });
            }
            _ => self.error = Some("Sample time must be a number from 0 to 1.".into()),
        }
        cx.notify();
    }
    fn submit(&mut self, edit: Edit, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending || !self.active.load(Ordering::Acquire) {
            return;
        }
        let unchanged = match &edit {
            Edit::Values(values) => values.iter().all(|(_, _, dirty)| !dirty),
            Edit::Bounds(values) => values
                .iter()
                .all(|(_, _, _, bounds)| bounds.iter().all(Option::is_none)),
            _ => false,
        };
        if unchanged {
            // Refresh silent committed edits without queuing an edit or blocking Reload.
            if let Some(runtime) = self.runtime.upgrade() {
                let _ = runtime.request_effect_presentation(Arc::downgrade(&self.active));
            }
            return;
        }
        let edits_custom = matches!(&edit,Edit::Values(values) if values.iter().any(|(key,_,dirty)|key.custom()&&*dirty));
        let edits_pair =
            matches!(&edit,Edit::Values(_) if self.model.kind()==PointwiseKind::Pixelate);
        if (edits_custom || edits_pair) && !self.authority.editable() {
            self.error =
                Some("These filter settings changed. Reload before editing these fields.".into());
            cx.notify();
            return;
        }
        let active = Arc::downgrade(&self.active);
        let runtime = self.runtime.clone();
        let authority = self.authority.clone();
        let model = self.model.clone();
        let handle = window.window_handle();
        let submitted: Vec<_> = self
            .drafts
            .iter()
            .map(|d| d.input.read(cx).value().to_string())
            .collect();
        let curve_submitted: Vec<[String; 2]> = self
            .curves
            .iter()
            .map(|d| std::array::from_fn(|i| d.fields[i].read(cx).value().to_string()))
            .collect();
        let applies_values = matches!(&edit, Edit::Values(_));
        let applies_bounds = matches!(&edit, Edit::Bounds(_));
        self.pending = true;
        self.error = None;
        cx.notify();
        cx.spawn(async move |owner,cx| {
            let alive=cx.update_window(handle,|_,_,cx|owner.read_with(cx,|_,_|())).is_ok_and(|r|r.is_ok());
            if !alive || !active.upgrade().is_some_and(|a|a.load(Ordering::Acquire)) {return;}
            // Revalidate the retained roots, then release every UI/engine guard
            // before actual setters and ordinary object notification callbacks.
            let result=(|| {
                if (edits_custom||edits_pair) && !authority.editable() {return Err("These filter settings changed before Apply. Reload and try again.".into());}
                match edit {
                    Edit::Values(values)=> {
                        if uses_parameter_fields(model.kind()) {
                            // Parse occurred before queuing. Merge only dirty Size axes
                            // into the current actual pair; engine setters own normalization.
                            let mut pairs = Vec::new();
                            for (key,value,dirty) in &values {
                                if !dirty {continue;}
                                if !active.upgrade().is_some_and(|a|a.load(Ordering::Acquire)) {return Ok(());}
                                let FieldKey::Parameter(index,axis)=*key else {return Err("The filter field type changed.".into());};
                                let p=model.parameter(index).map_err(|e|e.to_string())?;
                                if axis.is_some() {
                                    if pairs.contains(&index) {continue;}
                                    let mut pair=p.pair().map_err(|e|e.to_string())?;
                                    for (key,value,dirty) in &values {if let FieldKey::Parameter(other,Some(axis))=key {if *other==index&&*dirty {pair[*axis] = *value;}}}
                                    p.set_pair(pair).map_err(|e|e.to_string())?;
                                    pairs.push(index);
                                } else {p.set_number(*value).map_err(|e|e.to_string())?;}
                            }
                        } else if model.kind()==PointwiseKind::Pixelate {
                            let Authority::Pixels(p)=&authority else {return Err("Pixel sizes are unavailable.".into());};
                            let mut pair=p.pair().map_err(|e|e.to_string())?;let mut changed=false;
                            for (key,value,dirty) in values {if !dirty {continue;}match key {FieldKey::X=>{pair[0]=value;changed=true;},FieldKey::Y=>{pair[1]=value;changed=true;},_=>()}}
                            if changed {p.set_pair(pair).map_err(|e|e.to_string())?;}
                        } else {
                            let iterations=values.iter().find_map(|(key,value,dirty)|matches!(key,FieldKey::Iterations).then_some((*value,*dirty))).filter(|(_,dirty)|*dirty).map(|(v,_)|v);
                            if edits_custom {
                                let Authority::Matrix {object:Some(object),coefficients:Some(coefficients),..}=&authority else {return Err("Matrix settings are unavailable.".into());};
                                let payload=object.payload();
                                for (key,value,dirty) in values {if !dirty {continue;}match key {FieldKey::Angle=>payload.set_angle(value),FieldKey::Radius=>payload.set_radius(value),FieldKey::Coefficient(i)=>coefficients.write(i,value).map_err(|e|e.to_string())?,_=>()}}
                                // Only a genuine custom-object edit invokes its
                                // ordinary aoi command. Iterations alone does not
                                // clone previous Matrix or call Matrix observers.
                                object.notify().map_err(|e|e.to_string())?;
                            }
                            if let Some(value)=iterations {model.parameter(1).map_err(|e|e.to_string())?.set_number(value).map_err(|e|e.to_string())?;}
                        }
                    },
                    Edit::Choice(parameter,index)=>model.parameter(parameter).map_err(|e|e.to_string())?.set_index(index).map_err(|e|e.to_string())?,
                    Edit::Animation(parameter,axis,enabled)=>set_animation(&model.parameter(parameter).map_err(|e|e.to_string())?,axis,enabled)?,
                    Edit::Bounds(values)=> {for (parameter,axis,curve,[lower,upper]) in values {let p=model.parameter(parameter).map_err(|e|e.to_string())?;let actual=animation_axis(&p,axis)?.1.ok_or("The animation curve is unavailable.")?;if !actual.same_reference(&curve) {return Err("The animation curve changed. Reload before applying its bounds.".into());}
if let Some(lower)=lower {curve.set_minimum(lower);}
if let Some(upper)=upper {curve.set_maximum(upper);}}}
                }
                Ok::<_,String>(())
            })();
            if !active.upgrade().is_some_and(|a|a.load(Ordering::Acquire)) {return;}
            let delivered=cx.update_window(handle,|_,window,cx|owner.update(cx,|view,cx| {
                view.pending=false;view.error=result.err();
                if view.error.is_none() && applies_values {
                    for (draft,text) in view.drafts.iter_mut().zip(&submitted) {
                        if (draft.key.custom()&&!view.authority.editable()) || draft.input.read(cx).value()!=*text {continue;}
                        let Ok(value)=view.authority.value(draft.key,&view.model) else {continue;};
                        draft.synced=value.to_string();draft.input.update(cx,|s,cx|s.set_value(draft.synced.clone(),window,cx));
                    }
                }
                if view.error.is_none() && applies_bounds {
                    for (draft,text) in view.curves.iter_mut().zip(&curve_submitted) {
                        if !draft.fields.iter().zip(text).all(|(f,t)|f.read(cx).value()==*t) {continue;}
                        let Some(curve)=&draft.curve else {continue;};
                        draft.synced=[curve.minimum().to_string(),curve.maximum().to_string()];
                        for i in 0..2 {draft.fields[i].update(cx,|s,cx|s.set_value(draft.synced[i].clone(),window,cx));}
                    }
                }
                view.sync(window,cx);
            }));
            // Partial source commits also need presentation. This is the app's
            // existing refresh request, not a fabricated parameter notification.
            if !delivered.is_ok_and(|r|r.is_ok()) {return;}
            if let Some(runtime)=runtime.upgrade() {let _=runtime.request_effect_presentation(active);}
        }).detach();
    }
}
fn uses_parameter_fields(kind: PointwiseKind) -> bool {
    matches!(
        kind,
        PointwiseKind::SmartBlur
            | PointwiseKind::Smear
            | PointwiseKind::VariableBlur
            | PointwiseKind::OklabColor
    )
}
pub(crate) fn animation_axis(
    p: &PointwiseParameter<ForegroundError>,
    axis: i32,
) -> Result<(bool, Option<SharedCurve<ForegroundError>>), String> {
    let Some(owner) = p.retained_animation().map_err(|e| e.to_string())? else {
        return Ok((false, None));
    };
    let flag = owner
        .flags()
        .ok_or("Animation flags are unavailable.")?
        .value(axis)
        .map_err(|e| e.to_string())?;
    let curve = owner
        .curves()
        .ok_or("Animation curves are unavailable.")?
        .value(axis)
        .map_err(|e| e.to_string())?;
    Ok((flag, curve))
}
pub(crate) fn set_animation(
    p: &PointwiseParameter<ForegroundError>,
    axis: i32,
    enabled: bool,
) -> Result<(), String> {
    let owner = if enabled {
        p.animation().map_err(|e| e.to_string())?
    } else {
        let Some(owner) = p.retained_animation().map_err(|e| e.to_string())? else {
            return Ok(());
        };
        owner
    };
    if enabled {
        let curves = owner.curves().ok_or("Animation curves are unavailable.")?;
        let new = curves.value(axis).map_err(|e| e.to_string())?.is_none();
        owner.ensure_curve(axis).map_err(|e| e.to_string())?;
        if new {
            let curve = curves
                .value(axis)
                .map_err(|e| e.to_string())?
                .ok_or("Animation curve is unavailable.")?;
            curve
                .insert(0, Some(&CurveLayerHandle::new(AnalyticCurveKind::Linear)))
                .map_err(|e| e.to_string())?;
        }
    }
    p.set_axis_animated(axis as usize, enabled)
        .map_err(|e| e.to_string())
}
impl Drop for SpatialFilterView {
    fn drop(&mut self) {
        self.close();
    }
}
impl Render for SpatialFilterView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let id = self.id;
        let available = self.authority.editable();
        let disabled = self.pending;
        let mut content = div()
            .id(format!("spatial-filter-{id}"))
            .role(gpui_kit::Role::Group)
            .aria_label(format!("{} properties", self.model.kind().name()))
            .test_support()
            .track_focus(&self.focus)
            .key_context("SpatialFilter")
            .v_flex()
            .gap_3()
            .on_action(cx.listener(|view, _: &ApplySpatial, window, cx| {
                view.apply(window, cx);
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|view, _: &RevertSpatial, window, cx| {
                view.revert(window, cx);
                cx.stop_propagation();
            }));
        if !available {
            let message=match &self.authority {Authority::Unavailable(e)=>e.clone(),_=>"Matrix settings changed or are unavailable. Reload to inspect the current values.".into()};
            content = content.child(div().text_sm().text_color(theme.danger).child(message));
        }
        let smoothing = if self.model.kind() == PointwiseKind::Pixelate {
            self.model
                .parameter(1)
                .and_then(|p| p.options().map(|options| (p, options)))
                .ok()
        } else {
            None
        };
        if let Some((p, options)) = smoothing {
            let index = p.index();
            let shown = p
                .text()
                .ok()
                .flatten()
                .map(|v| String::from_utf16_lossy(&v))
                .unwrap_or_else(|| "Unavailable smoothing".into());
            let owner = cx.entity().downgrade();
            content = content.child(
                Field::new().label("Smoothing").child(
                    Button::new(format!("pixelate-{id}-smoothing"))
                        .label(shown)
                        .small()
                        .w_full()
                        .disabled(self.pending)
                        .dropdown_caret(true)
                        .dropdown_menu(move |mut menu, _, _| {
                            for (i, value) in options.entries().into_iter().enumerate() {
                                let owner = owner.clone();
                                menu = menu.item(
                                    PopupMenuItem::new(
                                        value
                                            .as_deref()
                                            .map(String::from_utf16_lossy)
                                            .unwrap_or_else(|| "Unnamed".into()),
                                    )
                                    .checked(index == i as i32)
                                    .disabled(value.is_none())
                                    .on_click(
                                        move |_, window, cx| {
                                            let _ = owner.update(cx, |view, cx| {
                                                view.submit(Edit::Choice(1, i as i32), window, cx)
                                            });
                                        },
                                    ),
                                );
                            }
                            menu
                        }),
                ),
            );
        }
        if uses_parameter_fields(self.model.kind()) {
            for (parameter, p) in self.model.parameters().into_iter().enumerate() {
                let PointwiseParameterValue::Choice {
                    current, options, ..
                } = p.value()
                else {
                    continue;
                };
                let shown = p
                    .text()
                    .ok()
                    .flatten()
                    .map(|v| String::from_utf16_lossy(&v))
                    .unwrap_or_else(|| "Unavailable choice".into());
                let owner = cx.entity().downgrade();
                content = content.child(
                    Field::new()
                        .label(String::from_utf16_lossy(&p.name()))
                        .child(
                            Button::new(format!("filter-choice-{id}-{parameter}"))
                                .label(shown.clone())
                                .tooltip(shown)
                                .small()
                                .w_full()
                                .disabled(disabled)
                                .dropdown_caret(true)
                                .dropdown_menu(move |mut menu, _, _| {
                                    for (index, value) in options.entries().into_iter().enumerate()
                                    {
                                        let owner = owner.clone();
                                        menu =
                                            menu.item(
                                                PopupMenuItem::new(
                                                    value
                                                        .as_deref()
                                                        .map(String::from_utf16_lossy)
                                                        .unwrap_or_else(|| "Unnamed".into()),
                                                )
                                                .checked(current == index as i32)
                                                .disabled(value.is_none())
                                                .on_click(move |_, window, cx| {
                                                    let _ = owner.update(cx, |view, cx| {
                                                        view.submit(
                                                            Edit::Choice(
                                                                parameter as i32,
                                                                index as i32,
                                                            ),
                                                            window,
                                                            cx,
                                                        )
                                                    });
                                                }),
                                            );
                                    }
                                    menu.scrollable(true)
                                }),
                        ),
                );
            }
        }
        content = content.child(
            Form::new()
                .columns(2)
                .children(self.drafts.iter().map(|draft| {
                    Field::new().label(draft.label.clone()).child(
                        Input::new(&draft.input)
                            .id(draft.id.clone())
                            .aria_label(draft.label.clone())
                            .disabled(disabled || (draft.key.custom() && !available)),
                    )
                })),
        );
        content = content.child(
            div()
                .h_flex()
                .flex_wrap()
                .gap_2()
                .child(
                    Button::new(format!("apply-spatial-filter-{id}"))
                        .label("Apply filter")
                        .primary()
                        .disabled(disabled)
                        .on_click(cx.listener(|v, _, w, cx| v.apply(w, cx))),
                )
                .child(
                    Button::new(format!("revert-spatial-filter-{id}"))
                        .label("Revert edits")
                        .disabled(self.pending)
                        .on_click(cx.listener(|v, _, w, cx| v.revert(w, cx))),
                )
                .child(
                    Button::new(format!("reload-spatial-filter-{id}"))
                        .label("Reload settings")
                        .disabled(self.pending)
                        .on_click(cx.listener(|v, _, w, cx| v.reload(w, cx))),
                ),
        );
        content = content.child(div().font_semibold().child("Animation"));
        for draft in &self.curves {
            let parameter = draft.parameter;
            let axis = draft.axis;
            let key = draft.key;
            let actual = self.animation_axis(parameter, axis);
            let enabled = actual.as_ref().map(|v| v.0).unwrap_or(false);
            content = content.child(
                Checkbox::new(format!("animate-filter-{id}-{key}"))
                    .label(format!("Animate {}", key.replace('-', " ")))
                    .checked(enabled)
                    .disabled(disabled)
                    .on_click(cx.listener(move |v, enabled, w, cx| {
                        v.submit(Edit::Animation(parameter, axis, *enabled), w, cx)
                    })),
            );
            if enabled {
                content = content.child(
                    Form::new().columns(2).children(
                        ["lower", "upper"]
                            .into_iter()
                            .enumerate()
                            .map(|(i, bound)| {
                                Field::new()
                                    .label(format!(
                                        "{} output",
                                        if i == 0 { "Start" } else { "End" }
                                    ))
                                    .child(
                                        Input::new(&draft.fields[i])
                                            .id(format!("filter-{id}-curve-{key}-{bound}"))
                                            .aria_label(format!("{key} {bound} output"))
                                            .disabled(disabled),
                                    )
                            }),
                    ),
                );
            }
        }
        content = content.child(
            Button::new(format!("apply-filter-{id}-curves"))
                .label("Apply curve outputs")
                .disabled(disabled)
                .on_click(cx.listener(|v, _, w, cx| v.apply_bounds(w, cx))),
        );
        content = content
            .child(
                Field::new().label("Workspace sample time (0–1)").child(
                    Input::new(&self.time)
                        .id(format!("filter-{id}-sample-time"))
                        .aria_label("Workspace sample time"),
                ),
            )
            .child(
                Button::new(format!("apply-filter-{id}-time"))
                    .label("Set sample time")
                    .disabled(self.pending)
                    .on_click(cx.listener(|v, _, _, cx| v.apply_time(cx))),
            );
        content.when_some(self.error.clone(), |this, e| {
            this.child(
                div()
                    .id(format!("spatial-filter-{id}-error"))
                    .role(gpui_kit::Role::Alert)
                    .aria_label(e.clone())
                    .test_support()
                    .text_sm()
                    .text_color(theme.danger)
                    .child(e),
            )
        })
    }
}
