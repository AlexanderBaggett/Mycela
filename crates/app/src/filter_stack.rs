//! Real retained filters and ordered editor wiring. Engine parameters remain
//! authoritative; entries own only routes, selection/order, and draft text.
use crate::{
    ForegroundError,
    color_binding_effects::BindingRuntime,
    colorize_gradient::ColorizeGradientView,
    filter_pipeline::{FilterInput, FilterStage},
    input_source::{InputSourceSlot, InputSourceView},
    spatial_filter::{SampleTime, SpatialFilterView},
    tone_curve::ToneCurveView,
};
use gpui_kit::base::{Disableable, Selectable, StyledExt, TestSupportExt};
use gpui_kit::component::{
    ActiveTheme, Sizable,
    button::{Button, ButtonVariants},
    form::{Field, Form},
    input::{Input, InputState},
    menu::{DropdownMenu, PopupMenu, PopupMenuItem},
};
use gpui_kit::{
    AppContext, Context, Entity, Focusable, IntoElement, Render, Window, div, prelude::*,
};
use mycela_engine::{
    Histogram, PointwiseCustomObject, PointwiseFilter, PointwiseKind, PointwiseParameter,
    PointwiseParameterValue,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

gpui_kit::actions!(filter_stack, [ApplyFilter, RevertFilter]);
const MAX_FILTERS: usize = 32;
pub(crate) const REGION_MODES: [&str; 4] =
    ["4 squares", "8 triangles", "24 triangles", "48 triangles"];
pub(crate) const OKLAB_INPUT_HELP: &str = "Connected maps use their red channel in place of the value or curve. For a and b, black is −0.4, mid-gray is 0, and white is +0.4.";
const KINDS: [PointwiseKind; 58] = [
    PointwiseKind::ChangeBci,
    PointwiseKind::ChangeHsb,
    PointwiseKind::ChangeOklab,
    PointwiseKind::Colorize,
    PointwiseKind::OklabColor,
    PointwiseKind::Levels,
    PointwiseKind::GetChannel,
    PointwiseKind::StoreMap,
    PointwiseKind::ClearMaps,
    PointwiseKind::MergeMaps,
    PointwiseKind::FixMapContrast,
    PointwiseKind::LayerMaps,
    PointwiseKind::ToneCurve,
    PointwiseKind::Combine,
    PointwiseKind::Add,
    PointwiseKind::Multiply,
    PointwiseKind::FromHsb,
    PointwiseKind::Regions,
    PointwiseKind::Hair,
    PointwiseKind::Pixelate,
    PointwiseKind::Matrix,
    PointwiseKind::Rotate,
    PointwiseKind::GaussianBlur,
    PointwiseKind::Sharpen,
    PointwiseKind::MedianBlur,
    PointwiseKind::MotionBlur,
    PointwiseKind::ShapeBlur,
    PointwiseKind::SmartBlur,
    PointwiseKind::Smear,
    PointwiseKind::VariableBlur,
    PointwiseKind::Equalize,
    PointwiseKind::Atmosphere,
    PointwiseKind::ConvertEnvironment,
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
    PointwiseKind::PointLight,
    PointwiseKind::Shadow,
    PointwiseKind::FullLight,
    PointwiseKind::Frequency,
    PointwiseKind::Fractalize,
    PointwiseKind::Exposure,
    PointwiseKind::Selector,
    PointwiseKind::MatchColor,
    PointwiseKind::ArtBlur,
    PointwiseKind::Distort,
    PointwiseKind::FreeTransform,
    PointwiseKind::Kaleidoscope,
    PointwiseKind::MoveRepeat,
    PointwiseKind::Remap,
    PointwiseKind::RotateTile,
];

// Presentation metadata only. KINDS remains the sole catalog; commands retain
// their actual PointwiseKind, never the menu's transient positional row index.
const CATALOG_GROUPS: [&str; 6] = [
    "Color and tone",
    "Maps",
    "Combine inputs",
    "Transform",
    "Blur and detail",
    "Environment",
];
fn catalog_group(kind: PointwiseKind) -> &'static str {
    match kind {
        PointwiseKind::Shader => "Custom",
        PointwiseKind::WizardGroup
        | PointwiseKind::Group
        | PointwiseKind::Reference
        | PointwiseKind::MapVariable
        | PointwiseKind::WizardReference
        | PointwiseKind::TemplateLab(_) => "Groups",
        PointwiseKind::Canvas | PointwiseKind::Checkerboard => "Generate",
        PointwiseKind::Style | PointwiseKind::Synthesis => "Advanced",
        PointwiseKind::ChangeBci
        | PointwiseKind::ChangeHsb
        | PointwiseKind::ChangeOklab
        | PointwiseKind::Colorize
        | PointwiseKind::ColorizeG2
        | PointwiseKind::OklabColor
        | PointwiseKind::WizardColor
        | PointwiseKind::WizardFiveColor
        | PointwiseKind::WizardSlideColor
        | PointwiseKind::WizardToneCurve
        | PointwiseKind::Levels
        | PointwiseKind::GetChannel
        | PointwiseKind::ToneCurve
        | PointwiseKind::ToneCurveG2
        | PointwiseKind::Equalize
        | PointwiseKind::Exposure
        | PointwiseKind::Selector
        | PointwiseKind::MatchColor
        | PointwiseKind::PointLight
        | PointwiseKind::IlluminateG2
        | PointwiseKind::Shadow
        | PointwiseKind::ShadowG2
        | PointwiseKind::FullLight => "Color and tone",
        PointwiseKind::StoreMap
        | PointwiseKind::ClearMaps
        | PointwiseKind::MergeMaps
        | PointwiseKind::FixMapContrast
        | PointwiseKind::LayerMaps => "Maps",
        PointwiseKind::Combine
        | PointwiseKind::Add
        | PointwiseKind::Multiply
        | PointwiseKind::FromHsb
        | PointwiseKind::Hair
        | PointwiseKind::Regions => "Combine inputs",
        PointwiseKind::Pixelate
        | PointwiseKind::Matrix
        | PointwiseKind::Rotate
        | PointwiseKind::Frequency
        | PointwiseKind::Fractalize
        | PointwiseKind::Distort
        | PointwiseKind::FreeTransform
        | PointwiseKind::Kaleidoscope
        | PointwiseKind::MoveRepeat
        | PointwiseKind::Remap
        | PointwiseKind::RotateTile
        | PointwiseKind::WizardMatrix
        | PointwiseKind::WizardKaleidoscope
        | PointwiseKind::Polygon
        | PointwiseKind::LegacyBricksBoards => "Transform",
        PointwiseKind::Atmosphere
        | PointwiseKind::ConvertEnvironment
        | PointwiseKind::MakeEnvironment
        | PointwiseKind::RotateEnvironment
        | PointwiseKind::DiffuseEnvironment
        | PointwiseKind::ScatterEnvironment
        | PointwiseKind::ReflectEnvironment
        | PointwiseKind::RefractEnvironment
        | PointwiseKind::Reflect
        | PointwiseKind::Refract
        | PointwiseKind::Stereogram
        | PointwiseKind::Caustics
        | PointwiseKind::Transmit => "Environment",
        PointwiseKind::GaussianBlur
        | PointwiseKind::Sharpen
        | PointwiseKind::MedianBlur
        | PointwiseKind::MotionBlur
        | PointwiseKind::ShapeBlur
        | PointwiseKind::SmartBlur
        | PointwiseKind::Smear
        | PointwiseKind::VariableBlur
        | PointwiseKind::ArtBlur => "Blur and detail",
    }
}

struct Entry {
    id: u64,
    model: PointwiseFilter<ForegroundError>,
    inputs: [FilterInput; 4],
    input_sources: Vec<Entity<InputSourceView>>,
}
enum AuxiliaryEdit {
    Values(Vec<(Binding, RequestedValue)>),
    Choice(i32, i32),
    RegionShape {
        entry: u64,
        parameter: PointwiseParameter<ForegroundError>,
        value: i32,
    },
}
#[derive(Clone)]
enum Binding {
    Number(PointwiseParameter<ForegroundError>),
    Pair(PointwiseParameter<ForegroundError>, usize),
    LightPosition(
        PointwiseParameter<ForegroundError>,
        PointwiseCustomObject<ForegroundError>,
        usize,
    ),
    Region(PointwiseParameter<ForegroundError>, usize),
    Color(PointwiseParameter<ForegroundError>, usize),
    Text(PointwiseParameter<ForegroundError>),
    Histogram(Histogram, usize),
}
struct Draft {
    label: String,
    input: Entity<InputState>,
    binding: Binding,
    loaded_text: String,
}
enum RequestedValue {
    Unchanged,
    Number(f64),
    Text(Vec<u16>),
}
impl Draft {
    fn check_light_position(
        parameter: &PointwiseParameter<ForegroundError>,
        object: &PointwiseCustomObject<ForegroundError>,
    ) -> Result<(), ForegroundError> {
        if parameter
            .custom_object()
            .map_err(error)?
            .as_ref()
            .is_some_and(|current| current.same_reference(object))
        {
            Ok(())
        } else {
            Err(ForegroundError::Operation(
                "The light position changed. Revert fields before continuing.".into(),
            ))
        }
    }
    fn display_value(&self) -> Result<String, ForegroundError> {
        let number = match &self.binding {
            Binding::Number(p) => p.number(),
            Binding::Pair(p, axis) => p.pair().map_err(error)?[*axis],
            Binding::LightPosition(p, object, axis) => {
                Self::check_light_position(p, object)?;
                object.light_position().map_err(error)?[*axis]
            }
            Binding::Region(p, field) => {
                let r = p.region().map_err(error)?;
                match field {
                    0 => f64::from(r.mode()),
                    1 => f64::from(r.x()),
                    2 => f64::from(r.y()),
                    _ => f64::from(r.size()),
                }
            }
            Binding::Color(p, axis) => p.color().map_err(error)?.components()[*axis],
            Binding::Text(p) => {
                return Ok(p
                    .text()
                    .map_err(error)?
                    .as_deref()
                    .map(String::from_utf16_lossy)
                    .unwrap_or_default());
            }
            Binding::Histogram(h, component) => {
                let at = usize::try_from(h.selected_channel()).map_err(|_| {
                    ForegroundError::Operation("The Levels channel is invalid.".into())
                })?;
                f64::from(h.arrays()[*component].read(at).map_err(error)?)
            }
        };
        Ok(number.to_string())
    }
    fn requested_value(&self, cx: &gpui_kit::App) -> Result<RequestedValue, String> {
        let text = self.input.read(cx).value();
        if matches!(self.binding, Binding::Text(_)) {
            // A lossy display cannot round-trip null or unpaired UTF-16. Only
            // a genuinely changed text draft requests a stored-text mutation.
            if text.as_ref() == self.loaded_text.as_str() {
                Ok(RequestedValue::Unchanged)
            } else {
                Ok(RequestedValue::Text(text.encode_utf16().collect()))
            }
        } else {
            let value = text.parse::<f64>().map_err(|_| {
                format!("{} needs a number. Correct it and apply again.", self.label)
            })?;
            if matches!(self.binding, Binding::Region(_, 0))
                && (!value.is_finite() || value.fract() != 0. || !(0.0..=3.0).contains(&value))
            {
                return Err("Region mode needs an integer from 0 to 3.".into());
            }
            Ok(RequestedValue::Number(value))
        }
    }
    fn commit(&self, value: RequestedValue) -> Result<(), ForegroundError> {
        Self::commit_binding(&self.binding, value)
    }
    fn commit_binding(binding: &Binding, value: RequestedValue) -> Result<(), ForegroundError> {
        match (binding, value) {
            (Binding::Text(_), RequestedValue::Unchanged) => Ok(()),
            (Binding::Color(p, axis), RequestedValue::Number(value)) => {
                let current = p.color().map_err(error)?;
                let mut rgb = current.components();
                rgb[*axis] = value;
                p.set_color(current.with_components(rgb)).map_err(error)
            }
            (Binding::LightPosition(p, object, axis), RequestedValue::Number(value)) => {
                Self::check_light_position(p, object)?;
                if *axis == 0 {
                    object.set_light_position_x(value).map_err(error)?;
                } else {
                    object.set_light_position_y(value).map_err(error)?;
                }
                object.notify().map_err(error)
            }
            (Binding::Pair(p, axis), RequestedValue::Number(value)) => {
                p.set_axis(*axis, value).map_err(error)
            }
            (Binding::Region(p, field), RequestedValue::Number(value)) => {
                let r = p.region().map_err(error)?;
                match field {
                    0 => r.set_mode(value as i32),
                    1 => r.set_x(value as f32),
                    2 => r.set_y(value as f32),
                    _ => r.set_size(value as f32),
                };
                r.notify().map_err(error)
            }
            (Binding::Number(p), RequestedValue::Number(value)) => {
                p.set_number(value).map_err(error)
            }
            (Binding::Text(p), RequestedValue::Text(value)) => {
                p.set_text(Some(&value)).map_err(error)
            }
            (Binding::Histogram(h, component), RequestedValue::Number(value)) => {
                h.set_component(*component, value as f32).map_err(error)
            }
            _ => Err(ForegroundError::Operation(
                "The filter field type changed.".into(),
            )),
        }
    }
}

fn owns_input_sources(kind: PointwiseKind) -> bool {
    matches!(
        kind,
        PointwiseKind::MedianBlur
            | PointwiseKind::MotionBlur
            | PointwiseKind::ShapeBlur
            | PointwiseKind::SmartBlur
            | PointwiseKind::Smear
            | PointwiseKind::VariableBlur
            | PointwiseKind::MatchColor
            | PointwiseKind::ArtBlur
            | PointwiseKind::Distort
            | PointwiseKind::FreeTransform
            | PointwiseKind::Kaleidoscope
            | PointwiseKind::MoveRepeat
            | PointwiseKind::Remap
            | PointwiseKind::RotateTile
    )
}

fn error(e: impl std::fmt::Display) -> ForegroundError {
    ForegroundError::Operation(e.to_string())
}

/// The actual models are retained here; their parameter handles own all committed
/// numeric/text/choice/Histogram/Gradient state. No wire DTO or second parameter map.
pub struct FilterStackView {
    entries: Vec<Entry>,
    next_id: u64,
    selected: Option<u64>,
    focus: gpui_kit::FocusHandle,
    drafts: Vec<Draft>,
    tone_curve: Option<Entity<ToneCurveView>>,
    colorize_gradient: Option<Entity<ColorizeGradientView>>,
    spatial: Option<Entity<SpatialFilterView>>,
    spatial_time: Option<gpui_kit::Subscription>,
    sample_time: f64,
    error: Option<String>,
    runtime: std::sync::Weak<BindingRuntime>,
    active: Arc<AtomicBool>,
    auxiliary_epoch: Arc<AtomicU64>,
    auxiliary_pending: bool,
}
impl gpui_kit::EventEmitter<SampleTime> for FilterStackView {}
impl FilterStackView {
    pub(crate) fn new(runtime: &Arc<BindingRuntime>, cx: &mut Context<Self>) -> Self {
        crate::colorize_gradient::bind_keys(cx);
        cx.bind_keys([
            gpui_kit::KeyBinding::new("secondary-enter", ApplyFilter, Some("FilterStack")),
            gpui_kit::KeyBinding::new("escape", RevertFilter, Some("FilterStack")),
        ]);
        Self {
            entries: Vec::new(),
            next_id: 1,
            selected: None,
            focus: cx.focus_handle().tab_stop(true),
            drafts: Vec::new(),
            tone_curve: None,
            colorize_gradient: None,
            spatial: None,
            spatial_time: None,
            sample_time: 0.,
            error: None,
            runtime: Arc::downgrade(runtime),
            active: Arc::new(AtomicBool::new(true)),
            auxiliary_epoch: Arc::new(AtomicU64::new(0)),
            auxiliary_pending: false,
        }
    }
    pub fn filters(&self) -> Vec<PointwiseFilter<ForegroundError>> {
        self.entries.iter().map(|e| e.model.clone()).collect()
    }
    pub(crate) fn pipeline(&self) -> Vec<FilterStage> {
        self.entries
            .iter()
            .map(|entry| FilterStage::new(entry.id, entry.model.clone(), entry.inputs))
            .collect()
    }
    pub fn identities(&self) -> Vec<u64> {
        self.entries.iter().map(|e| e.id).collect()
    }
    pub(crate) fn preferred_focus(&self, cx: &gpui_kit::App) -> gpui_kit::FocusHandle {
        self.drafts
            .first()
            .map(|draft| draft.input.read(cx).focus_handle(cx))
            .unwrap_or_else(|| {
                self.colorize_gradient
                    .as_ref()
                    .map(|gradient| gradient.read(cx).preferred_focus(cx))
                    .or_else(|| {
                        self.tone_curve
                            .as_ref()
                            .map(|curve| curve.read(cx).preferred_focus(cx))
                    })
                    .or_else(|| {
                        self.spatial
                            .as_ref()
                            .map(|view| view.read(cx).preferred_focus(cx))
                    })
                    .unwrap_or_else(|| self.focus.clone())
            })
    }
    pub fn selected_identity(&self) -> Option<u64> {
        self.selected
    }
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    pub fn pipeline_names(&self) -> Vec<&'static str> {
        self.entries.iter().map(|e| e.model.kind().name()).collect()
    }
    fn selected_model(&self) -> Option<PointwiseFilter<ForegroundError>> {
        self.entries
            .iter()
            .find(|e| Some(e.id) == self.selected)
            .map(|e| e.model.clone())
    }
    // A popup can outlive the row/editor that opened it. The captured entry and
    // actual property owner must still be the selected retained occurrence.
    fn accepts_region_shape(
        &self,
        id: u64,
        parameter: &PointwiseParameter<ForegroundError>,
    ) -> bool {
        self.selected == Some(id)
            && self
                .entries
                .iter()
                .find(|entry| entry.id == id)
                .is_some_and(|entry| {
                    entry.model.same_reference(&parameter.owner())
                        && entry.model.parameters().into_iter().any(|current| {
                            current.same_reference(parameter)
                                && matches!(current.value(), PointwiseParameterValue::Region { .. })
                        })
                })
    }
    fn region_shape_item(
        owner: gpui_kit::WeakEntity<Self>,
        entry: u64,
        parameter: PointwiseParameter<ForegroundError>,
        value: i32,
        label: &'static str,
        current: i32,
    ) -> PopupMenuItem {
        PopupMenuItem::new(label)
            .checked(current == value)
            .on_click(move |_, window, cx| {
                let _ = owner.update(cx, |view, cx| {
                    view.submit_auxiliary(
                        AuxiliaryEdit::RegionShape {
                            entry,
                            parameter: parameter.clone(),
                            value,
                        },
                        window.window_handle(),
                        cx,
                    );
                });
            })
    }
    fn present(&mut self, cx: &mut Context<Self>) {
        if self.active.load(Ordering::Acquire) {
            if let Some(runtime) = self.runtime.upgrade() {
                if let Err(e) = runtime.request_effect_presentation(Arc::downgrade(&self.active)) {
                    self.error = Some(e.to_string());
                }
            }
        }
        cx.notify();
    }
    pub(crate) fn uses_spatial(&self) -> bool {
        self.entries.iter().any(|e| {
            matches!(
                e.model.kind(),
                PointwiseKind::Distort
                    | PointwiseKind::FreeTransform
                    | PointwiseKind::Kaleidoscope
                    | PointwiseKind::MoveRepeat
                    | PointwiseKind::Remap
                    | PointwiseKind::RotateTile
                    | PointwiseKind::MatchColor
                    | PointwiseKind::ArtBlur
                    | PointwiseKind::Exposure
                    | PointwiseKind::Selector
                    | PointwiseKind::Frequency
                    | PointwiseKind::Fractalize
                    | PointwiseKind::Pixelate
                    | PointwiseKind::Matrix
                    | PointwiseKind::Rotate
                    | PointwiseKind::Equalize
                    | PointwiseKind::Atmosphere
                    | PointwiseKind::ConvertEnvironment
                    | PointwiseKind::RotateEnvironment
                    | PointwiseKind::DiffuseEnvironment
                    | PointwiseKind::ScatterEnvironment
                    | PointwiseKind::ReflectEnvironment
                    | PointwiseKind::RefractEnvironment
                    | PointwiseKind::Reflect
                    | PointwiseKind::Refract
                    | PointwiseKind::Stereogram
                    | PointwiseKind::Regions
                    | PointwiseKind::Hair
                    | PointwiseKind::OklabColor
                    | PointwiseKind::Caustics
                    | PointwiseKind::Transmit
                    | PointwiseKind::PointLight
                    | PointwiseKind::Shadow
                    | PointwiseKind::FullLight
                    | PointwiseKind::GaussianBlur
                    | PointwiseKind::Sharpen
                    | PointwiseKind::MedianBlur
                    | PointwiseKind::MotionBlur
                    | PointwiseKind::ShapeBlur
                    | PointwiseKind::SmartBlur
                    | PointwiseKind::Smear
                    | PointwiseKind::VariableBlur
            )
        })
    }
    pub(crate) fn set_sample_time(
        &mut self,
        time: f64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sample_time = time;
        for entry in &self.entries {
            for view in &entry.input_sources {
                view.update(cx, |v, cx| v.set_sample_time(time, window, cx));
            }
        }
        if let Some(spatial) = &self.spatial {
            spatial.update(cx, |v, cx| v.set_sample_time(time, window, cx));
        }
    }
    pub(crate) fn reconcile_layer_inputs(
        &mut self,
        inputs: &[(u64, mycela_engine::SharedImageMap)],
        cx: &mut Context<Self>,
    ) -> Result<bool, ForegroundError> {
        let mut changed = false;
        for (id, image) in inputs {
            let Some(entry) = self.entries.iter().find(|e| e.id == *id) else {
                return Err(ForegroundError::TextureReplaced);
            };
            let selection = || -> Result<_, ForegroundError> {
                (0..2)
                    .map(|i| {
                        let p = entry.model.parameter(i).map_err(error)?;
                        // An out-of-range retained choice is valid state to compare.
                        // Its getter error must not prevent refreshing real options.
                        Ok((p.index(), p.text().map_err(|e| e.source_class())))
                    })
                    .collect::<Result<Vec<_>, _>>()
            };
            let before = selection()?;
            entry
                .model
                .refresh_layer_map_options(Some(image))
                .map_err(error)?;
            changed |= before != selection()?;
        }
        self.refresh_controls(cx);
        Ok(changed)
    }
    pub(crate) fn refresh_controls(&mut self, cx: &mut Context<Self>) {
        for entry in &self.entries {
            for view in &entry.input_sources {
                view.update(cx, |v, cx| v.refresh_controls(cx));
            }
        }
        if let Some(spatial) = &self.spatial {
            spatial.update(cx, |view, cx| view.refresh_controls(cx));
        }
        if let Some(gradient) = &self.colorize_gradient {
            gradient.update(cx, |view, cx| view.refresh_controls(cx));
        }
        if let Some(curve) = &self.tone_curve {
            curve.update(cx, |view, cx| view.refresh_controls(cx));
        }
        cx.notify();
    }
    fn retire_inputs(&self, cx: &mut gpui_kit::App) {
        for entry in &self.entries {
            for view in &entry.input_sources {
                view.update(cx, |v, cx| v.set_active(false, cx));
            }
        }
    }
    /// Source replacement retires queued input edits while retaining each draft
    /// and real source owner. New work gets a fresh activation, not a reused flag.
    pub(crate) fn retire_input_work(&mut self, cx: &mut gpui_kit::App) {
        self.auxiliary_epoch.fetch_add(1, Ordering::AcqRel);
        self.auxiliary_pending = false;
        self.retire_inputs(cx);
        for entry in &self.entries {
            for view in &entry.input_sources {
                view.update(cx, |v, cx| {
                    v.set_active(self.selected == Some(entry.id), cx)
                });
            }
        }
    }
    fn retire_curve(&mut self, cx: &mut gpui_kit::App) {
        self.spatial_time.take();
        if let Some(view) = self.spatial.take() {
            view.update(cx, |v, _| v.close());
        }
        if let Some(gradient) = self.colorize_gradient.take() {
            gradient.update(cx, |view, _| view.close());
        }
        if let Some(curve) = self.tone_curve.take() {
            curve.update(cx, |view, _| view.close());
        }
    }
    pub(crate) fn clear(&mut self, cx: &mut Context<Self>) {
        self.retire_curve(cx);
        self.auxiliary_epoch.fetch_add(1, Ordering::AcqRel);
        self.auxiliary_pending = false;
        self.retire_inputs(cx);
        self.entries.clear();
        self.selected = None;
        self.drafts.clear();
        self.error = None;
        self.present(cx);
    }
    pub(crate) fn close(&mut self, cx: &mut gpui_kit::App) {
        self.retire_curve(cx);
        self.retire_inputs(cx);
        self.active.store(false, Ordering::Release);
        self.entries.clear();
        self.drafts.clear();
        self.selected = None;
    }
    fn catalog_menu(
        owner: gpui_kit::WeakEntity<Self>,
        available: bool,
        mut menu: PopupMenu,
    ) -> PopupMenu {
        for group in CATALOG_GROUPS {
            menu = menu.label(group);
            for kind in KINDS
                .into_iter()
                .filter(|kind| catalog_group(*kind) == group)
            {
                let owner = owner.clone();
                menu = menu.item(
                    PopupMenuItem::new(kind.name())
                        .disabled(!available)
                        .on_click(move |_, window, cx| {
                            let _ = owner.update(cx, |view, cx| view.add(kind, window, cx));
                        }),
                );
            }
        }
        // The standard menu owns scrolling, keyboard selection, collision,
        // dismissal and focus restoration. Its default cap follows the window.
        menu.scrollable(true)
    }
    fn add(&mut self, kind: PointwiseKind, window: &mut Window, cx: &mut Context<Self>) {
        if !self.active.load(Ordering::Acquire) || self.entries.len() >= MAX_FILTERS {
            return;
        }
        match PointwiseFilter::new(kind) {
            Ok(model) => {
                let id = self.next_id;
                self.next_id += 1;
                let slots: &[InputSourceSlot] = match kind {
                    PointwiseKind::Distort => &[
                        InputSourceSlot::DistortDirection,
                        InputSourceSlot::DistortDistance,
                    ],
                    PointwiseKind::Remap => &[InputSourceSlot::RemapX, InputSourceSlot::RemapY],
                    PointwiseKind::MatchColor => &[InputSourceSlot::MatchReference],
                    PointwiseKind::ArtBlur => {
                        &[InputSourceSlot::ArtRadius, InputSourceSlot::ArtDriver]
                    }
                    PointwiseKind::MedianBlur => &[InputSourceSlot::MedianRadius],
                    PointwiseKind::SmartBlur => &[InputSourceSlot::SmartRadius],
                    PointwiseKind::VariableBlur => &[InputSourceSlot::VariableRadius],
                    PointwiseKind::Smear => &[
                        InputSourceSlot::SmearDirection,
                        InputSourceSlot::SmearDistance,
                    ],
                    PointwiseKind::MotionBlur => {
                        &[InputSourceSlot::MotionRadius, InputSourceSlot::MotionAngle]
                    }
                    PointwiseKind::ShapeBlur => {
                        &[InputSourceSlot::ShapeRadius, InputSourceSlot::ShapeMap]
                    }
                    _ => &[],
                };
                let input_sources = self
                    .runtime
                    .upgrade()
                    .map(|runtime| {
                        slots
                            .iter()
                            .map(|slot| {
                                cx.new(|_| {
                                    InputSourceView::new(
                                        id,
                                        *slot,
                                        model.clone(),
                                        self.sample_time,
                                        &runtime,
                                        window,
                                    )
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                self.entries.push(Entry {
                    id,
                    model,
                    input_sources,
                    inputs: if kind == PointwiseKind::FullLight {
                        [
                            FilterInput::Previous,
                            FilterInput::Source,
                            FilterInput::Source,
                            FilterInput::None,
                        ]
                    } else if matches!(
                        kind,
                        PointwiseKind::Combine
                            | PointwiseKind::Reflect
                            | PointwiseKind::Refract
                            | PointwiseKind::Stereogram
                            | PointwiseKind::Transmit
                            | PointwiseKind::ReflectEnvironment
                            | PointwiseKind::RefractEnvironment
                    ) {
                        [
                            FilterInput::Previous,
                            FilterInput::Source,
                            FilterInput::None,
                            FilterInput::None,
                        ]
                    } else if owns_input_sources(kind)
                        || matches!(kind, PointwiseKind::Hair | PointwiseKind::Regions)
                    {
                        // Use the current chain and leave auxiliary inputs empty.
                        [
                            FilterInput::Previous,
                            FilterInput::None,
                            FilterInput::None,
                            FilterInput::None,
                        ]
                    } else {
                        // New source nodes have unconnected slots. Each No
                        // input is resolved to its own actual empty image.
                        [FilterInput::None; 4]
                    },
                });
                self.select(id, window, cx);
                self.present(cx);
            }
            Err(e) => {
                self.error = Some(e.to_string());
                cx.notify();
            }
        }
    }
    fn select(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        if self.entries.iter().all(|e| e.id != id) {
            return;
        }
        if self.selected == Some(id) {
            return;
        }
        self.selected = Some(id);
        self.error = None;
        self.load_editor(window, cx);
    }
    fn load_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.retire_curve(cx);
        self.retire_input_work(cx);
        self.drafts.clear();
        if let Some(model) = self.selected_model() {
            if matches!(
                model.kind(),
                PointwiseKind::Pixelate
                    | PointwiseKind::Matrix
                    | PointwiseKind::OklabColor
                    | PointwiseKind::SmartBlur
                    | PointwiseKind::Smear
                    | PointwiseKind::VariableBlur
            ) {
                if let (Some(id), Some(runtime)) = (self.selected, self.runtime.upgrade()) {
                    let editor = cx.new(|cx| {
                        SpatialFilterView::new(id, model, self.sample_time, &runtime, window, cx)
                    });
                    self.spatial_time =
                        Some(cx.subscribe(&editor, |_, _, event: &SampleTime, cx| {
                            cx.emit(event.clone())
                        }));
                    window.focus(&editor.read(cx).preferred_focus(cx), cx);
                    self.spatial = Some(editor);
                }
                cx.notify();
                return;
            }
            if model.kind() == PointwiseKind::ToneCurve {
                if let (Ok(parameter), Some(runtime)) = (model.parameter(0), self.runtime.upgrade())
                {
                    match parameter.curve() {
                        Ok(curve) => {
                            self.tone_curve = Some(cx.new(|cx| {
                                ToneCurveView::new(parameter, curve, &runtime, window, cx)
                            }))
                        }
                        Err(error) => self.error = Some(error.to_string()),
                    }
                } else {
                    self.error = Some("The tone curve is unavailable.".into());
                }
            }
            let mut bindings = Vec::new();
            for p in model.parameters() {
                let name = String::from_utf16_lossy(&p.name());
                match p.value() {
                    PointwiseParameterValue::Pair { .. } => {
                        for (axis, label) in ["X", "Y"].into_iter().enumerate() {
                            bindings
                                .push((format!("{name} {label}"), Binding::Pair(p.clone(), axis)));
                        }
                    }
                    PointwiseParameterValue::Matrix { .. } => match p.custom_object() {
                        Ok(Some(object)) if object.is_light_position() => {
                            for (axis, label) in ["X", "Y"].into_iter().enumerate() {
                                bindings.push((
                                    format!("{name} {label}"),
                                    Binding::LightPosition(p.clone(), object.clone(), axis),
                                ));
                            }
                        }
                        Err(e) => self.error = Some(e.to_string()),
                        _ => {}
                    },
                    PointwiseParameterValue::Region { .. } => {
                        for (field, label) in ["Region X", "Region Y", "Region size"]
                            .into_iter()
                            .enumerate()
                        {
                            bindings.push((label.into(), Binding::Region(p.clone(), field + 1)));
                        }
                    }
                    PointwiseParameterValue::Number { .. } => {
                        bindings.push((name, Binding::Number(p)))
                    }
                    PointwiseParameterValue::Color(_) => {
                        for (axis, label) in ["Red", "Green", "Blue"].into_iter().enumerate() {
                            bindings
                                .push((format!("{name} {label}"), Binding::Color(p.clone(), axis)));
                        }
                    }
                    PointwiseParameterValue::Text(_) => bindings.push((name, Binding::Text(p))),
                    PointwiseParameterValue::Histogram(h) => {
                        for (component, label) in ["Black point", "Midpoint", "White point"]
                            .into_iter()
                            .enumerate()
                        {
                            bindings.push((label.into(), Binding::Histogram(h.clone(), component)));
                        }
                    }
                    PointwiseParameterValue::Gradient(_) => {
                        if let Some(runtime) = self.runtime.upgrade() {
                            let editor =
                                cx.new(|cx| ColorizeGradientView::new(p, &runtime, window, cx));
                            editor.update(cx, |view, cx| view.initialize(window, cx));
                            self.colorize_gradient = Some(editor);
                        }
                    }
                    _ => (),
                }
            }
            for (label, binding) in bindings {
                let input = cx.new(|cx| InputState::new(window, cx));
                let mut draft = Draft {
                    label,
                    input,
                    binding,
                    loaded_text: String::new(),
                };
                match draft.display_value() {
                    Ok(v) => {
                        draft.loaded_text = v.clone();
                        draft.input.update(cx, |s, cx| s.set_value(v, window, cx));
                    }
                    Err(e) => self.error = Some(e.to_string()),
                }
                self.drafts.push(draft);
            }
        }
        if let Some(draft) = self.drafts.first() {
            window.focus(&draft.input.read(cx).focus_handle(cx), cx);
        } else {
            window.focus(&self.preferred_focus(cx), cx);
        }
        cx.notify();
    }
    fn move_selected(&mut self, offset: isize, cx: &mut Context<Self>) {
        let Some(from) = self
            .entries
            .iter()
            .position(|e| Some(e.id) == self.selected)
        else {
            return;
        };
        let Some(to) = from
            .checked_add_signed(offset)
            .filter(|to| *to < self.entries.len())
        else {
            return;
        };
        self.entries.swap(from, to);
        self.present(cx);
    }
    fn remove_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self
            .entries
            .iter()
            .position(|e| Some(e.id) == self.selected)
        else {
            return;
        };
        for view in &self.entries[index].input_sources {
            view.update(cx, |v, cx| v.set_active(false, cx));
        }
        self.entries.remove(index);
        self.selected = self
            .entries
            .get(index.min(self.entries.len().saturating_sub(1)))
            .map(|e| e.id);
        self.load_editor(window, cx);
        self.present(cx);
    }
    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .selected_model()
            .is_some_and(|m| owns_input_sources(m.kind()))
        {
            if self.auxiliary_pending {
                return;
            }
            let parsed: Result<Vec<_>, _> =
                self.drafts.iter().map(|d| d.requested_value(cx)).collect();
            match parsed {
                Ok(values) => {
                    let values = self
                        .drafts
                        .iter()
                        .zip(values)
                        .map(|(draft, value)| (draft.binding.clone(), value))
                        .collect();
                    self.submit_auxiliary(
                        AuxiliaryEdit::Values(values),
                        window.window_handle(),
                        cx,
                    );
                }
                Err(error) => {
                    self.error = Some(error);
                    cx.notify();
                }
            }
            return;
        }
        // Validate every numeric draft before any live parameter changes. Text
        // stays untrimmed here; each original map node owns its name policy.
        let values: Result<Vec<_>, _> = self.drafts.iter().map(|d| d.requested_value(cx)).collect();
        let result = values.and_then(|values| {
            for (draft, value) in self.drafts.iter().zip(values) {
                draft.commit(value).map_err(|e| e.to_string())?;
            }
            Ok(())
        });
        self.error = result.err();
        if self.error.is_none() {
            for draft in &mut self.drafts {
                if let Ok(value) = draft.display_value() {
                    draft.loaded_text = value.clone();
                    draft
                        .input
                        .update(cx, |s, cx| s.set_value(value, window, cx));
                }
            }
        }
        self.present(cx);
    }
    fn set_choice(
        &mut self,
        id: u64,
        parameter: usize,
        index: i32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected != Some(id) {
            return;
        }
        if let Some(model) = self.selected_model() {
            if owns_input_sources(model.kind()) {
                self.submit_auxiliary(
                    AuxiliaryEdit::Choice(parameter as i32, index),
                    window.window_handle(),
                    cx,
                );
                return;
            }
            self.error = model
                .parameter(parameter as i32)
                .and_then(|p| p.set_index(index))
                .err()
                .map(|e| e.to_string());
            self.present(cx);
        }
    }
    fn submit_auxiliary(
        &mut self,
        edit: AuxiliaryEdit,
        handle: gpui_kit::AnyWindowHandle,
        cx: &mut Context<Self>,
    ) {
        if self.auxiliary_pending || !self.active.load(Ordering::Acquire) {
            return;
        }
        let Some(model) = self.selected_model() else {
            return;
        };
        if !owns_input_sources(model.kind()) {
            return;
        }
        if let AuxiliaryEdit::RegionShape {
            entry, parameter, ..
        } = &edit
            && !self.accepts_region_shape(*entry, parameter)
        {
            return;
        }
        let submitted: Vec<_> = self
            .drafts
            .iter()
            .map(|d| d.input.read(cx).value().to_string())
            .collect();
        let applies_values = matches!(&edit, AuxiliaryEdit::Values(_));
        let epoch = self.auxiliary_epoch.clone();
        let request = epoch.load(Ordering::Acquire);
        let active = Arc::downgrade(&self.active);
        let runtime = self.runtime.clone();
        self.auxiliary_pending = true;
        cx.notify();
        cx.spawn(async move |owner, cx| {
            let alive = cx
                .update_window(handle, |_, _, cx| {
                    owner.read_with(cx, |view, _| match &edit {
                        AuxiliaryEdit::RegionShape {
                            entry, parameter, ..
                        } => view.accepts_region_shape(*entry, parameter),
                        _ => true,
                    })
                })
                .is_ok_and(|r| r.unwrap_or(false));
            if !alive
                || epoch.load(Ordering::Acquire) != request
                || !active.upgrade().is_some_and(|m| m.load(Ordering::Acquire))
            {
                return;
            }
            // The actual setter can invoke caller callbacks. No App/Entity guard survives.
            let result = match edit {
                AuxiliaryEdit::Values(values) => (|| {
                    for (binding, value) in values {
                        if epoch.load(Ordering::Acquire) != request
                            || !active.upgrade().is_some_and(|m| m.load(Ordering::Acquire))
                        {
                            return Ok(());
                        }
                        Draft::commit_binding(&binding, value)?;
                    }
                    Ok(())
                })(),
                AuxiliaryEdit::RegionShape {
                    parameter, value, ..
                } => Draft::commit_binding(
                    &Binding::Region(parameter, 0),
                    RequestedValue::Number(f64::from(value)),
                ),
                AuxiliaryEdit::Choice(parameter, value) => model
                    .parameter(parameter)
                    .and_then(|p| p.set_index(value))
                    .map_err(error),
            }
            .map_err(|e| e.to_string());
            let delivered = cx.update_window(handle, |_, window, cx| {
                owner.update(cx, |v, cx| {
                    if epoch.load(Ordering::Acquire) != request {
                        return;
                    }
                    v.auxiliary_pending = false;
                    v.error = result.err();
                    if applies_values && v.error.is_none() {
                        for (draft, submitted) in v.drafts.iter_mut().zip(&submitted) {
                            if draft.input.read(cx).value().as_ref() == submitted.as_str() {
                                if let Ok(value) = draft.display_value() {
                                    draft.loaded_text = value.clone();
                                    draft
                                        .input
                                        .update(cx, |v, cx| v.set_value(value, window, cx));
                                }
                            }
                        }
                    }
                    cx.notify();
                })
            });
            if delivered.is_ok_and(|r| r.is_ok())
                && epoch.load(Ordering::Acquire) == request
                && active.upgrade().is_some_and(|m| m.load(Ordering::Acquire))
            {
                if let Some(runtime) = runtime.upgrade() {
                    if let Err(e) = runtime.request_effect_presentation(active) {
                        runtime.retain_failure(e);
                    }
                }
            }
        })
        .detach();
    }
    fn input_choices(&self, id: u64) -> Vec<(FilterInput, String)> {
        let mut choices = vec![
            (FilterInput::Previous, "Previous output".into()),
            (FilterInput::Source, "Source before Invert".into()),
            (FilterInput::None, "No input".into()),
        ];
        for entry in self.entries.iter().take_while(|entry| entry.id != id) {
            choices.push((
                FilterInput::Filter(entry.id),
                format!("Filter #{} · {}", entry.id, entry.model.kind().name()),
            ));
        }
        choices
    }
    fn set_input(&mut self, id: u64, slot: usize, route: FilterInput, cx: &mut Context<Self>) {
        // A menu can outlive selection, reorder, removal or window closure.
        // Revalidate the actual owner and upstream availability at activation.
        if !self.active.load(Ordering::Acquire) || self.selected != Some(id) {
            return;
        }
        if !self
            .input_choices(id)
            .iter()
            .any(|(candidate, _)| *candidate == route)
        {
            return;
        }
        if let Some(entry) = self.entries.iter_mut().find(|entry| {
            entry.id == id
                && entry.model.node().slots().len() > 1
                && (!owns_input_sources(entry.model.kind()) || slot == 0)
                && slot < entry.model.node().slots().len()
        }) {
            entry.inputs[slot] = route;
            self.error = None;
            self.present(cx);
        }
    }
    fn set_histogram_channel(
        &mut self,
        id: u64,
        channel: i32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected != Some(id) {
            return;
        }
        if let Some(model) = self.selected_model() {
            match model.parameter(0).and_then(|p| p.histogram()) {
                Ok(h) => {
                    h.set_channel(channel);
                    self.load_editor(window, cx);
                }
                Err(e) => self.error = Some(e.to_string()),
            }
            cx.notify();
        }
    }
}
impl Drop for FilterStackView {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
    }
}
impl Render for FilterStackView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let available = self.entries.len() < MAX_FILTERS;
        let add_button = Button::new("add-filter")
            .label("Add filter…")
            .accessibility_label("Add filter")
            .small()
            .disabled(!available)
            .dropdown_caret(true);
        // DropdownMenu's Popover owns its trigger handler independently of the
        // Button. At capacity render only the actual disabled Button, so neither
        // pointer nor keyboard can open an otherwise enabled popover wrapper.
        let catalog = if available {
            let owner = cx.entity().downgrade();
            add_button
                .dropdown_menu(move |menu, _, _| Self::catalog_menu(owner.clone(), available, menu))
                .into_any_element()
        } else {
            add_button.into_any_element()
        };
        let selected = self.selected;
        let index = self.entries.iter().position(|e| Some(e.id) == selected);
        let mut editor = div().v_flex().gap_3();
        if let Some(model) = self.selected_model() {
            let id = selected.unwrap();
            editor = editor.child(div().font_semibold().child(model.kind().name()));
            let slots = model.node().slots();
            if slots.len() > 1 {
                let entry = self
                    .entries
                    .iter()
                    .find(|entry| entry.id == id)
                    .expect("selected entry");
                let choices = self.input_choices(id);
                let input_help = match model.kind() {
                    PointwiseKind::OklabColor => OKLAB_INPUT_HELP,
                    PointwiseKind::Distort | PointwiseKind::Smear => {
                        "Choose the Original input here. Direction and Distance maps have their own sources below."
                    }
                    PointwiseKind::Remap => {
                        "Choose the Original input here. X and Y maps have their own sources below."
                    }
                    PointwiseKind::MatchColor => {
                        "Choose the Original input here. The Match To image has its own source below."
                    }
                    PointwiseKind::ArtBlur => {
                        "Choose the Original input here. Radius and Driver maps have their own sources below."
                    }
                    PointwiseKind::SmartBlur
                    | PointwiseKind::VariableBlur
                    | PointwiseKind::MedianBlur => {
                        "Choose the Original input here. The radius map has its own source below."
                    }
                    PointwiseKind::ShapeBlur => {
                        "Choose the Original input here. Radius and Shape maps have their own sources below."
                    }
                    PointwiseKind::MotionBlur => {
                        "Choose the Original input here. Radius and Angle maps have their own sources below."
                    }
                    _ => {
                        "Choose the source before Invert, the previous output, or an earlier filter. No input uses an empty image."
                    }
                };
                editor = editor.child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(input_help),
                );
                editor = editor.child(
                    Form::new().children(
                        slots
                            .into_iter()
                            .enumerate()
                            .filter(|(slot, _)| !owns_input_sources(model.kind()) || *slot == 0)
                            .map(|(slot, input)| {
                                let label = String::from_utf16_lossy(&input.name());
                                let route = entry.inputs[slot];
                                let shown = choices
                                    .iter()
                                    .find(|(candidate, _)| *candidate == route)
                                    .map(|(_, label)| label.clone())
                                    .unwrap_or_else(|| match route {
                                        FilterInput::Filter(id) => {
                                            format!("Unavailable filter #{id}")
                                        }
                                        _ => "Unavailable input".into(),
                                    });
                                let owner = cx.entity().downgrade();
                                let choices = choices.clone();
                                Field::new().label(label).child(
                                    Button::new(if model.kind() == PointwiseKind::Combine {
                                        format!("combine-input-{id}-{slot}")
                                    } else {
                                        format!("filter-input-{id}-{slot}")
                                    })
                                    .label(shown.clone())
                                    .tooltip(shown)
                                    .w_full()
                                    .small()
                                    .dropdown_caret(true)
                                    .dropdown_menu(
                                        move |mut menu, _, _| {
                                            for (candidate, label) in &choices {
                                                let owner = owner.clone();
                                                let candidate = *candidate;
                                                menu = menu.item(
                                                    PopupMenuItem::new(label.clone())
                                                        .checked(candidate == route)
                                                        .on_click(move |_, _, cx| {
                                                            let _ = owner.update(cx, |view, cx| {
                                                                view.set_input(
                                                                    id, slot, candidate, cx,
                                                                )
                                                            });
                                                        }),
                                                );
                                            }
                                            menu.scrollable(true)
                                        },
                                    ),
                                )
                            }),
                    ),
                );
            }
            if let Some(gradient) = &self.colorize_gradient {
                editor = editor.child(gradient.clone());
            }
            if model.kind() == PointwiseKind::Levels {
                if let Ok(h) = model.parameter(0).and_then(|p| p.histogram()) {
                    editor = editor.child(div().h_flex().flex_wrap().gap_1().children(
                        ["All", "Red", "Green", "Blue"].into_iter().enumerate().map(
                            |(i, label)| {
                                Button::new(format!("histogram-{id}-{i}"))
                                    .label(label)
                                    .small()
                                    .selected(h.selected_channel() == i as i32)
                                    .on_click(cx.listener(move |view, _, window, cx| {
                                        view.set_histogram_channel(id, i as i32, window, cx)
                                    }))
                            },
                        ),
                    ));
                }
            }
            for (parameter, p) in model.parameters().into_iter().enumerate().filter(|_| {
                !matches!(
                    model.kind(),
                    PointwiseKind::Pixelate
                        | PointwiseKind::Matrix
                        | PointwiseKind::SmartBlur
                        | PointwiseKind::Smear
                        | PointwiseKind::VariableBlur
                )
            }) {
                if let PointwiseParameterValue::Region { .. } = p.value() {
                    let region = p.region();
                    if let Ok(region) = region {
                        let current = region.mode();
                        let owner = cx.entity().downgrade();
                        let parameter = p.clone();
                        let labels = REGION_MODES;
                        let shown = usize::try_from(current)
                            .ok()
                            .and_then(|ix| labels.get(ix))
                            .copied()
                            .unwrap_or("Custom region mode");
                        editor = editor.child(
                            Field::new().label("Region shape").child(
                                Button::new(format!("region-mode-{id}"))
                                    .label(shown)
                                    .small()
                                    .w_full()
                                    .disabled(self.auxiliary_pending)
                                    .dropdown_caret(true)
                                    .dropdown_menu(move |mut menu, _, _| {
                                        for (ix, label) in labels.into_iter().enumerate() {
                                            menu = menu.item(Self::region_shape_item(
                                                owner.clone(),
                                                id,
                                                parameter.clone(),
                                                ix as i32,
                                                label,
                                                current,
                                            ));
                                        }
                                        menu
                                    }),
                            ),
                        );
                    }
                    continue;
                }
                if let PointwiseParameterValue::Choice {
                    current, options, ..
                } = p.value()
                {
                    if (model.kind() == PointwiseKind::Combine && matches!(parameter, 0 | 2))
                        || (model.kind() == PointwiseKind::Fractalize && parameter == 3)
                        || matches!(
                            model.kind(),
                            PointwiseKind::ConvertEnvironment
                                | PointwiseKind::Rotate
                                | PointwiseKind::GaussianBlur
                                | PointwiseKind::MedianBlur
                                | PointwiseKind::MotionBlur
                                | PointwiseKind::ShapeBlur
                                | PointwiseKind::ArtBlur
                                | PointwiseKind::FreeTransform
                                | PointwiseKind::Distort
                        )
                    {
                        let shown = p
                            .text()
                            .ok()
                            .flatten()
                            .as_deref()
                            .map(String::from_utf16_lossy)
                            .unwrap_or_else(|| "Unavailable choice".into());
                        let owner = cx.entity().downgrade();
                        let entries = options.entries();
                        editor = editor.child(
                            Field::new()
                                .label(String::from_utf16_lossy(&p.name()))
                                .child(
                                    Button::new(if model.kind() == PointwiseKind::Combine {
                                        format!("combine-choice-{id}-{parameter}")
                                    } else {
                                        format!("filter-choice-{id}-{parameter}")
                                    })
                                    .label(shown.clone())
                                    .tooltip(shown)
                                    .disabled(self.auxiliary_pending)
                                    .small()
                                    .w_full()
                                    .dropdown_caret(true)
                                    .dropdown_menu(
                                        move |mut menu, _, _| {
                                            for (ix, value) in entries.iter().enumerate() {
                                                let owner = owner.clone();
                                                menu = menu.item(
                                                    PopupMenuItem::new(
                                                        value
                                                            .as_deref()
                                                            .map(String::from_utf16_lossy)
                                                            .unwrap_or_else(|| "Unnamed".into()),
                                                    )
                                                    .disabled(value.is_none())
                                                    .checked(current == ix as i32)
                                                    .on_click(move |_, window, cx| {
                                                        let _ = owner.update(cx, |view, cx| {
                                                            view.set_choice(
                                                                id, parameter, ix as i32, window,
                                                                cx,
                                                            )
                                                        });
                                                    }),
                                                );
                                            }
                                            menu.scrollable(true)
                                        },
                                    ),
                                ),
                        );
                        continue;
                    }
                    editor = editor.child(
                        div()
                            .v_flex()
                            .gap_2()
                            .child(String::from_utf16_lossy(&p.name()))
                            .child(div().h_flex().flex_wrap().gap_1().children(
                                options.entries().into_iter().enumerate().map(|(i, value)| {
                                    Button::new(format!("choice-{id}-{parameter}-{i}"))
                                        .label(
                                            value
                                                .as_deref()
                                                .map(String::from_utf16_lossy)
                                                .unwrap_or_else(|| "Unnamed".into()),
                                        )
                                        .small()
                                        .disabled(value.is_none())
                                        .selected(current == i as i32)
                                        .toggled(current == i as i32)
                                        .on_click(cx.listener(move |view, _, window, cx| {
                                            view.set_choice(id, parameter, i as i32, window, cx)
                                        }))
                                }),
                            )),
                    );
                }
            }
            editor = editor.child(Form::new().children(self.drafts.iter().enumerate().map(
                |(i, draft)| {
                    Field::new().label(draft.label.clone()).child(
                        Input::new(&draft.input)
                            .id(format!("filter-{id}-value-{i}"))
                            .aria_label(draft.label.clone()),
                    )
                },
            )));
            if !self.drafts.is_empty() {
                editor = editor.child(
                    div()
                        .h_flex()
                        .gap_2()
                        .child(
                            Button::new("apply-filter")
                                .label("Apply filter")
                                .disabled(self.auxiliary_pending)
                                .primary()
                                .on_click(
                                    cx.listener(|view, _, window, cx| view.apply(window, cx)),
                                ),
                        )
                        .child(
                            Button::new("revert-filter")
                                .label("Revert edits")
                                .disabled(self.auxiliary_pending)
                                .on_click(cx.listener(|view, _, window, cx| {
                                    view.error = None;
                                    view.load_editor(window, cx);
                                })),
                        ),
                );
            }
            if matches!(
                model.kind(),
                PointwiseKind::SmartBlur | PointwiseKind::Smear | PointwiseKind::VariableBlur
            ) {
                editor = editor.children(self.spatial.clone());
            }
            if let Some(entry) = self.entries.iter().find(|e| e.id == id) {
                for input in &entry.input_sources {
                    editor = if matches!(
                        model.kind(),
                        PointwiseKind::MotionBlur
                            | PointwiseKind::ShapeBlur
                            | PointwiseKind::Smear
                            | PointwiseKind::ArtBlur
                            | PointwiseKind::Distort
                            | PointwiseKind::Remap
                    ) {
                        editor.child(
                            div()
                                .border_t_1()
                                .border_color(theme.border)
                                .pt_3()
                                .child(input.clone()),
                        )
                    } else {
                        editor.child(input.clone())
                    };
                }
            }
        }
        div()
            .id("filter-stack")
            .role(gpui_kit::Role::Group)
            .aria_label("Filters")
            .test_support()
            .track_focus(&self.focus)
            .key_context("FilterStack")
            .v_flex()
            .p_4()
            .gap_4()
            .on_action(cx.listener(|view, _: &ApplyFilter, window, cx| {
                view.apply(window, cx);
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|view, _: &RevertFilter, window, cx| {
                view.error = None;
                view.load_editor(window, cx);
                cx.stop_propagation();
            }))
            .child(
                div()
                    .h_flex()
                    .justify_between()
                    .gap_2()
                    .child(catalog)
                    .child(
                        div()
                            .id("filter-count")
                            .role(gpui_kit::Role::Status)
                            .aria_label(format!(
                                "{} of {MAX_FILTERS} filters{}",
                                self.entries.len(),
                                if available { "" } else { "; limit reached" }
                            ))
                            .test_support()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(format!("{} / {MAX_FILTERS}", self.entries.len())),
                    ),
            )
            .when(self.entries.is_empty(), |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("Add a filter to shape the texture."),
                )
            })
            .child(
                div()
                    .v_flex()
                    .gap_1()
                    .children(self.entries.iter().enumerate().map(|(position, entry)| {
                        let id = entry.id;
                        Button::new(format!("select-filter-{id}"))
                            .label(format!("{} · {}", position + 1, entry.model.kind().name()))
                            .w_full()
                            .selected(Some(id) == selected)
                            .on_click(
                                cx.listener(move |view, _, window, cx| view.select(id, window, cx)),
                            )
                    })),
            )
            .when(selected.is_some(), |this| {
                this.child(
                    div()
                        .h_flex()
                        .gap_2()
                        .child(
                            Button::new("filter-up")
                                .label("Up")
                                .small()
                                .disabled(index == Some(0))
                                .on_click(cx.listener(|view, _, _, cx| view.move_selected(-1, cx))),
                        )
                        .child(
                            Button::new("filter-down")
                                .label("Down")
                                .small()
                                .disabled(index.is_some_and(|i| i + 1 == self.entries.len()))
                                .on_click(cx.listener(|view, _, _, cx| view.move_selected(1, cx))),
                        )
                        .child(
                            Button::new("remove-filter")
                                .label("Remove")
                                .small()
                                .on_click(cx.listener(|view, _, window, cx| {
                                    view.remove_selected(window, cx)
                                })),
                        ),
                )
            })
            .child(editor)
            .children(self.tone_curve.clone())
            .children(self.spatial.clone().filter(|_| {
                !self.selected_model().is_some_and(|model| {
                    matches!(
                        model.kind(),
                        PointwiseKind::SmartBlur
                            | PointwiseKind::Smear
                            | PointwiseKind::VariableBlur
                    )
                })
            }))
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .id("filter-error")
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

#[cfg(test)]
mod region_popup_tests {
    use super::*;
    use gpui_kit::test::TestWindowExt;

    #[gpui_kit::test]
    async fn retained_region_menu_callbacks_reject_selection_removal_and_queued_retirement(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(true);
        });
        let (handle, root) = cx.update(crate::open_main_window).unwrap();
        let editor = cx.update(|cx| root.read(cx).color_view());
        let stack = cx.update(|cx| editor.read(cx).filter_stack());
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("source-gradient", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.click("filters-tab", cx);
        })
        .unwrap();
        crate::filter_catalog_test::add(cx, handle, PointwiseKind::Kaleidoscope);
        crate::filter_catalog_test::add(cx, handle, PointwiseKind::Kaleidoscope);
        cx.run_until_parked();
        let (ids, models) = cx.update(|cx| {
            let view = stack.read(cx);
            (view.identities(), view.filters())
        });
        assert_eq!(ids.len(), 2);
        let before: Vec<_> = models
            .iter()
            .map(|model| {
                let parameter = model.parameter(1).unwrap();
                let current = parameter.region().unwrap();
                let previous = parameter.previous_region().unwrap();
                let fields = (current.fields(), previous.fields());
                (parameter, current, previous, fields)
            })
            .collect();
        let unchanged = || {
            for (parameter, current, previous, fields) in &before {
                assert!(parameter.region().unwrap().same_reference(current));
                assert!(
                    parameter
                        .previous_region()
                        .unwrap()
                        .same_reference(previous)
                );
                assert_eq!((current.fields(), previous.fields()), *fields);
                assert!(
                    current
                        .constructor_owner()
                        .unwrap()
                        .same_reference(parameter)
                );
            }
        };
        // Retain the actual GPUI PopupMenuItem handlers built by the production
        // menu factory. Normal selection may dismiss the visual popup; a caller
        // holding its action must still be rejected against the new selection.
        cx.update_window(handle, |_, window, cx| {
            stack.update(cx, |view, cx| view.select(ids[0], window, cx));
        })
        .unwrap();
        let mut actions = [1, 2, 3].map(|value| {
            let item = FilterStackView::region_shape_item(
                stack.downgrade(),
                ids[0],
                before[0].0.clone(),
                value,
                "Retained region shape",
                0,
            );
            let PopupMenuItem::Item {
                handler: Some(handler),
                ..
            } = item
            else {
                panic!("Region mode must use the real semantic item callback");
            };
            Some(handler)
        });
        cx.update_window(handle, |_, window, cx| {
            stack.update(cx, |view, cx| view.select(ids[1], window, cx));
            actions[0].take().unwrap()(&gpui_kit::ClickEvent::default(), window, cx);
            let view = stack.read(cx);
            assert_eq!(view.selected_identity(), Some(ids[1]));
            assert!(
                !view.auxiliary_pending,
                "stale action must not publish pending work"
            );
        })
        .unwrap();
        cx.run_until_parked();
        unchanged();
        cx.update_window(handle, |_, window, cx| {
            stack.update(cx, |view, cx| view.select(ids[0], window, cx));
            actions[1].take().unwrap()(&gpui_kit::ClickEvent::default(), window, cx);
            assert!(
                stack.read(cx).auxiliary_pending,
                "current owner's action must be admitted"
            );
            // Retire a valid queued callback before its async setter can run.
            stack.update(cx, |view, cx| view.select(ids[1], window, cx));
            assert!(!stack.read(cx).auxiliary_pending);
        })
        .unwrap();
        cx.run_until_parked();
        unchanged();
        cx.update_window(handle, |_, window, cx| {
            stack.update(cx, |view, cx| {
                view.select(ids[0], window, cx);
                view.remove_selected(window, cx);
            });
            actions[2].take().unwrap()(&gpui_kit::ClickEvent::default(), window, cx);
            let view = stack.read(cx);
            assert_eq!(view.identities(), vec![ids[1]]);
            assert_eq!(view.selected_identity(), Some(ids[1]));
            assert!(view.filters()[0].same_reference(&models[1]));
            assert!(
                !view.auxiliary_pending,
                "removed popup owner must not enqueue an edit"
            );
        })
        .unwrap();
        cx.run_until_parked();
        unchanged();
    }
}

#[cfg(test)]
mod catalog_tests {
    use super::*;
    use gpui_kit::component::{Theme, ThemeMode};
    use gpui_kit::{TestAppContext, test::TestWindowExt};

    #[gpui_kit::test]
    async fn compact_catalog_exposes_all_kinds_and_restores_keyboard_focus_at_minimum_zoom(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(true);
        });
        let (handle, root) = cx.update(crate::open_main_window).unwrap();
        cx.simulate_window_resize(
            handle,
            gpui_kit::size(gpui_kit::px(760.), gpui_kit::px(480.)),
        );
        let editor = cx.update(|cx| root.read(cx).color_view());
        let stack = cx.update(|cx| editor.read(cx).filter_stack());
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("filters-tab", cx);
        })
        .unwrap();
        for (mode, font) in [(ThemeMode::Light, 16.), (ThemeMode::Dark, 20.)] {
            cx.update(|cx| {
                Theme::change(mode, None, cx);
                Theme::update(cx, |theme| theme.font_size = gpui_kit::px(font));
            });
            cx.update_window(handle, |_, window, cx| {
                window.render_frame(cx);
                let trigger = window.find("add-filter");
                assert!(trigger.visible());
                assert!(trigger.bounds().size.height <= window.rem_size() * 3.);
                for _ in 0..64 {
                    if window.find("add-filter").focused() == Some(true) {
                        break;
                    }
                    window.press("tab", cx);
                }
                assert_eq!(window.find("add-filter").focused(), Some(true));
                window.press("enter", cx);
                assert_eq!(window.find("popup-menu").focused(), Some(true));
                let mut menu = window.within("popup-menu");
                for kind in KINDS {
                    let matches = (0usize..64)
                        .filter(|ix| {
                            menu.try_find(*ix)
                                .is_some_and(|row| row.label() == Some(kind.name()))
                        })
                        .count();
                    assert_eq!(matches, 1, "one real command for {}", kind.name());
                }
                menu.press("escape", cx);
            })
            .unwrap();
            cx.run_until_parked();
            cx.update_window(handle, |_, window, cx| {
                window.render_frame(cx);
                assert!(window.try_find("popup-menu").is_none());
                assert_eq!(window.find("add-filter").focused(), Some(true));
                assert!(stack.read(cx).filters().is_empty());
                for ix in 0..KINDS.len() {
                    assert!(window.try_find(format!("add-filter-{ix}")).is_none());
                }
            })
            .unwrap();
        }
        crate::filter_catalog_test::add(cx, handle, PointwiseKind::ChangeBci);
        cx.run_until_parked();
        let held = cx.update(|cx| stack.read(cx).filters()[0].clone());
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            let field = window.find("filter-1-value-0");
            let trigger = window.find("add-filter");
            let viewport = window.find("color-inspector").bounds();
            assert!(
                field.bounds().top() - trigger.bounds().bottom() < viewport.size.height,
                "the selected editor follows the compact catalog rather than39 wrapped buttons"
            );
            crate::filter_catalog_test::reveal(window, "filter-1-value-0", cx);
            window.click("filter-1-value-0", cx);
            window.press("secondary-a", cx);
            window.input("12", cx);
            crate::filter_catalog_test::reveal(window, "apply-filter", cx);
            window.click("apply-filter", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(held.parameter(1).unwrap().number(), 12.);
        crate::filter_catalog_test::add(cx, handle, PointwiseKind::RotateTile);
        cx.run_until_parked();
        cx.update(|cx| {
            let models = stack.read(cx).filters();
            assert_eq!(models.len(), 2);
            assert!(models[0].same_reference(&held));
            assert_eq!(models[1].kind(), PointwiseKind::RotateTile);
        });
    }

    #[gpui_kit::test]
    async fn catalog_capacity_disables_add_then_removal_reenables_the_real_command(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(true);
        });
        let (handle, root) = cx.update(crate::open_main_window).unwrap();
        cx.simulate_window_resize(
            handle,
            gpui_kit::size(gpui_kit::px(1000.), gpui_kit::px(1000.)),
        );
        let editor = cx.update(|cx| root.read(cx).color_view());
        let stack = cx.update(|cx| editor.read(cx).filter_stack());
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("filters-tab", cx);
        })
        .unwrap();
        for _ in 0..MAX_FILTERS {
            crate::filter_catalog_test::add(cx, handle, PointwiseKind::ChangeBci);
            cx.run_until_parked();
        }
        let held = cx.update(|cx| stack.read(cx).filters());
        assert_eq!(held.len(), MAX_FILTERS);
        cx.update_window(handle, |_, window, cx| {
            crate::filter_catalog_test::reveal(window, "add-filter", cx);
            assert_eq!(
                window.find("filter-count").label(),
                Some("32 of 32 filters; limit reached")
            );
            window.click("add-filter", cx);
            assert!(window.try_find("popup-menu").is_none());
            assert_eq!(stack.read(cx).filters().len(), MAX_FILTERS);
            crate::filter_catalog_test::reveal(window, "remove-filter", cx);
            window.click("remove-filter", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            crate::filter_catalog_test::reveal(window, "add-filter", cx);
            assert_eq!(
                window.find("filter-count").label(),
                Some("31 of 32 filters")
            );
        })
        .unwrap();
        crate::filter_catalog_test::add(cx, handle, PointwiseKind::GetChannel);
        cx.run_until_parked();
        cx.update(|cx| {
            let view = stack.read(cx);
            let actual = view.filters();
            assert_eq!(actual.len(), MAX_FILTERS);
            for (before, after) in held.iter().take(MAX_FILTERS - 1).zip(&actual) {
                assert!(before.same_reference(after));
            }
            assert_eq!(actual.last().unwrap().kind(), PointwiseKind::GetChannel);
            assert_eq!(view.selected_identity(), Some(33));
        });
    }
}
