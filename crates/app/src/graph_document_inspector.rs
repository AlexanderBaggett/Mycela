//! Drafts retain real parameter handles; no current value is stored separately.
use super::model::{LiveDocument, Node, failure, guard};
use super::view::custom::CustomDraft;
use super::view::environment::EnvironmentPanel;
use super::view::gradient::{EnvironmentGradientKind, GradientEditor};
use super::view::legacy_gradient::LegacyGradientEditor;
use super::view::levels::LevelsEditor;
use super::view::numeric_field::{NumericField, NumericRange};
use super::view::selector::SelectorColorEditor;
use crate::ForegroundError;
use gpui_kit::component::input::InputState;
use gpui_kit::{App, AppContext, Entity, Window};
use mycela_engine::{
    CanvasAreaEditor, CanvasAreaField, CanvasAreaSettings, CanvasShapeEditor, CanvasShapeField,
    CanvasShapeValues, CurveControlHandle, CurveControlKind, CurveLayerHandle,
    EnvironmentColorField, EnvironmentField, EnvironmentLayerKind, EnvironmentLight,
    EnvironmentMaskFill, EnvironmentMaskHemisphere, EnvironmentRecord, GradientAnimation,
    ImageGeneratorKind, ImageGeneratorParameter, ModernNodeAnimation, ModernNodeProperty,
    ModernNodeValue, PointwiseCustomObject, PointwiseInvertProperty, PointwiseMatrix,
    PointwiseParameter, PointwiseParameterValue, PointwiseRegion, SharedCurve, SharedGradient,
    SharedLimits, SharedSelectionOptions, SolidColorProperty,
};
type Parameter = PointwiseParameter<ForegroundError>;
type GeneratorParameter = ImageGeneratorParameter<ForegroundError>;
type ModernProperty = ModernNodeProperty<ForegroundError>;
pub(super) type CanvasArea = CanvasAreaEditor<ForegroundError>;
pub(super) type CanvasShape = CanvasShapeEditor<ForegroundError>;

fn validate_filter_control(property: &Parameter) -> Result<(), String> {
    if property.is_retired_wizard_control() {
        return Err("The Wizard control changed. Revert fields before continuing.".into());
    }
    Ok(())
}

#[derive(Clone)]
enum Binding {
    Number(Parameter),
    CanvasArea(CanvasArea, CanvasAreaField),
    CanvasShape(CanvasShape, CanvasShapeField),
    Environment(EnvironmentPanel, EnvironmentField),
    VariableIndex(Parameter),
    VariableMapping(Parameter),
    GeneratorVariableIndex(GeneratorParameter),
    GeneratorVariableMapping(GeneratorParameter),
    SolidVariableIndex(Node, SolidColorProperty),
    SolidVariableMapping(Node, SolidColorProperty),
    FilterColor(Parameter, usize),
    Axis(Parameter, usize),
    Text(Parameter),
    Generator(GeneratorParameter),
    GeneratorAxis(GeneratorParameter, usize),
    Color(Node, usize),
    Transparency(Node),
    Matrix(PointwiseMatrix<ForegroundError>, bool),
    LightPosition(Parameter, PointwiseCustomObject<ForegroundError>, usize),
    RegionCoordinate(Parameter, PointwiseRegion<ForegroundError>, usize),
    AnimationBound(
        AnimationParameter,
        AnimationOwner,
        SharedCurve<ForegroundError>,
        usize,
        usize,
    ),
    LayerScalar(LayerSelection, CurveControlHandle<ForegroundError>),
    GradientStop(GradientEditor, mycela_engine::GradientEntry, usize),
    Modern(ModernProperty),
    ModernAxis(ModernProperty, usize),
}
pub(crate) struct Draft {
    pub id: String,
    pub label: String,
    pub input: Entity<InputState>,
    loaded: String,
    binding: Binding,
    pub(super) live_error: Option<String>,
}
#[derive(Clone)]
pub(crate) enum Choice {
    Filter(Parameter),
    Generator(GeneratorParameter),
    Invert(PointwiseInvertProperty<ForegroundError>),
    Modern(ModernProperty),
    ModernBoolean(ModernProperty),
    Region(Parameter),
}
pub(crate) struct ChoiceField {
    pub id: String,
    pub label: String,
    pub binding: Choice,
}
#[derive(Clone)]
pub(crate) struct ChoiceOptions {
    pub entries: Vec<Option<Vec<u16>>>,
    root: Option<SharedSelectionOptions>,
}
impl ChoiceOptions {
    fn retained(root: SharedSelectionOptions) -> Self {
        Self {
            entries: root.entries(),
            root: Some(root),
        }
    }
    fn fixed(entries: Vec<Option<Vec<u16>>>) -> Self {
        Self {
            entries,
            root: None,
        }
    }
    pub fn same_snapshot(&self, other: &Self) -> bool {
        let same_root = match (&self.root, &other.root) {
            (Some(a), Some(b)) => a.same_reference(b),
            (None, None) => true,
            _ => false,
        };
        same_root && self.entries == other.entries
    }
}
impl Choice {
    pub(super) fn is_animated_channel(&self) -> bool {
        matches!(
            self,
            Self::Filter(property) if property.owner().kind() == mycela_engine::PointwiseKind::GetChannel
                && property.axis_is_animated(0).unwrap_or(false)
        )
    }

    pub(super) fn numeric_lab_property(
        &self,
    ) -> Option<mycela_engine::LabNumericProperty<ForegroundError>> {
        use mycela_engine::LabNumericProperty as Lab;
        match self {
            Self::Filter(property) => Lab::filter(property),
            Self::Generator(property) => Lab::generator(property),
            Self::Invert(property) => Lab::invert(property),
            _ => None,
        }
    }

    pub fn options(&self) -> Result<ChoiceOptions, String> {
        match self {
            Self::Filter(p) => match p.value() {
                PointwiseParameterValue::Choice { options, .. } => {
                    Ok(ChoiceOptions::retained(options))
                }
                _ => Err("The choice changed type.".into()),
            },
            Self::Generator(p) => p.options().map(ChoiceOptions::retained).map_err(failure),
            Self::Invert(p) => p.options().map(ChoiceOptions::retained).map_err(failure),
            Self::Modern(p) => p
                .selected_options()
                .map(|v| ChoiceOptions::fixed(v.into_iter().map(Some).collect()))
                .ok_or_else(|| "The choice changed type.".into()),
            Self::ModernBoolean(_) => Ok(ChoiceOptions::fixed(
                ["Off", "On"]
                    .into_iter()
                    .map(|s| Some(s.encode_utf16().collect()))
                    .collect(),
            )),
            Self::Region(_) => Ok(ChoiceOptions::fixed(
                crate::filter_stack::REGION_MODES
                    .into_iter()
                    .map(|mode| Some(mode.encode_utf16().collect()))
                    .collect(),
            )),
        }
    }
    pub fn index(&self) -> Result<i32, String> {
        match self {
            Self::Filter(p) => Ok(p.index()),
            Self::Generator(p) => p.index().map_err(failure),
            Self::Invert(p) => p.current().map_err(failure),
            Self::Modern(p) => match p.value() {
                ModernNodeValue::Int32(v) => Ok(v),
                _ => Err("The choice changed type.".into()),
            },
            Self::ModernBoolean(p) => match p.value() {
                ModernNodeValue::Boolean(v) => Ok(i32::from(v)),
                _ => Err("The choice changed type.".into()),
            },
            Self::Region(p) => Ok(p.region().map_err(failure)?.mode()),
        }
    }
    pub fn set(&self, index: i32) -> Result<(), String> {
        match self {
            Self::Filter(p) => {
                validate_filter_control(p)?;
                p.set_index(index).map_err(failure)
            }
            Self::Generator(p) => p.set_index(index).map_err(failure),
            Self::Invert(p) => p.set_index(index).map_err(failure),
            Self::Modern(p) => p.set_int32(index).map_err(failure),
            Self::ModernBoolean(p) => match index {
                0 => p.set_boolean(false).map_err(failure),
                1 => p.set_boolean(true).map_err(failure),
                _ => Err("The choice is invalid.".into()),
            },
            Self::Region(p) => {
                if !(0..crate::filter_stack::REGION_MODES.len() as i32).contains(&index) {
                    return Err("The region mode is invalid.".into());
                }
                let region = p.region().map_err(failure)?;
                let before = region.fields();
                let (_, mut x, y, mut height) = before;
                let aspect = PointwiseRegion::<ForegroundError>::aspect_for_mode(index);
                if aspect != region.aspect() {
                    let right = x + height * aspect;
                    if right > 1. {
                        x -= right - 1.;
                    }
                    if height * aspect > 1. {
                        height /= height * aspect;
                        x = 0.;
                    }
                }
                p.edit_region(&region, before, (index, x, y, height))
                    .map_err(failure)
            }
        }
    }
}
impl Binding {
    fn solid_component(&self) -> Option<f64> {
        match self {
            Self::Color(node, channel) => Some(node.solid_color_values().ok()?[*channel]),
            Self::Transparency(node) => node.solid_transparency().ok(),
            _ => None,
        }
    }
    fn check_light_position(
        parameter: &Parameter,
        object: &PointwiseCustomObject<ForegroundError>,
    ) -> Result<(), String> {
        let current = parameter.custom_object().map_err(failure)?;
        if current
            .as_ref()
            .is_some_and(|current| current.same_reference(object))
        {
            Ok(())
        } else {
            Err("The light position changed. Revert fields before continuing.".into())
        }
    }
    fn check_animation_curve(
        parameter: &AnimationParameter,
        animation: &AnimationOwner,
        curve: &SharedCurve<ForegroundError>,
        axis: usize,
    ) -> Result<(), String> {
        let current = parameter.retained()?;
        if !current
            .as_ref()
            .is_some_and(|current| current.same_reference(animation))
        {
            return Err("The animation changed. Revert fields before continuing.".into());
        }
        let (active, current) = parameter.axis(axis)?;
        if !active
            || !current
                .as_ref()
                .is_some_and(|current| current.same_reference(curve))
        {
            return Err("The animation curve changed. Revert fields before continuing.".into());
        }
        Ok(())
    }
    fn text(&self) -> Result<String, String> {
        Ok(match self {
            Self::Number(p) => p.number().to_string(),
            Self::CanvasArea(area, field) => {
                area.settings().map_err(failure)?.number(*field).to_string()
            }
            Self::CanvasShape(shape, field) => shape
                .settings()
                .map_err(failure)?
                .number(*field)
                .ok_or("The shape field is unavailable.")?
                .to_string(),
            Self::Environment(panel, field) => panel
                .value()
                .field(*field)
                .ok_or_else(|| "The environment field is unavailable.".to_owned())?
                .to_string(),
            Self::VariableIndex(p) => p.binding_index().to_string(),
            Self::GeneratorVariableIndex(p) => p.binding_index().to_string(),
            Self::SolidVariableIndex(node, property) => node
                .solid_binding_index(*property)
                .map_err(failure)?
                .to_string(),
            Self::VariableMapping(p) => p
                .binding_name()
                .as_deref()
                .map(String::from_utf16_lossy)
                .unwrap_or_default(),
            Self::GeneratorVariableMapping(p) => p
                .binding_name()
                .as_deref()
                .map(String::from_utf16_lossy)
                .unwrap_or_default(),
            Self::SolidVariableMapping(node, property) => node
                .solid_binding_mapping(*property)
                .map_err(failure)?
                .as_deref()
                .map(String::from_utf16_lossy)
                .unwrap_or_default(),
            Self::FilterColor(p, axis) => {
                p.color().map_err(failure)?.components()[*axis].to_string()
            }
            Self::Axis(p, axis) => p.pair().map_err(failure)?[*axis].to_string(),
            Self::Text(p) => p
                .text()
                .map_err(failure)?
                .as_deref()
                .map(String::from_utf16_lossy)
                .unwrap_or_default(),
            Self::Generator(p) => p.number().map_err(failure)?.to_string(),
            Self::GeneratorAxis(p, axis) => p.pair().map_err(failure)?[*axis].to_string(),
            Self::Color(node, channel) => {
                color_byte_text(node.solid_color_values().map_err(failure)?[*channel])
            }
            Self::Transparency(node) => {
                color_byte_text(1. - node.solid_transparency().map_err(failure)?)
            }
            Self::Matrix(matrix, radius) => if *radius {
                matrix.payload().radius()
            } else {
                matrix.payload().angle()
            }
            .to_string(),
            Self::LightPosition(parameter, object, axis) => {
                Self::check_light_position(parameter, object)?;
                object.light_position().map_err(failure)?[*axis].to_string()
            }
            Self::RegionCoordinate(_, region, axis) => {
                let (_, x, y, height) = region.fields();
                [x, y, height][*axis].to_string()
            }
            Self::AnimationBound(parameter, animation, curve, axis, side) => {
                Self::check_animation_curve(parameter, animation, curve, *axis)?;
                if *side == 0 {
                    curve.minimum()
                } else {
                    curve.maximum()
                }
                .to_string()
            }
            Self::LayerScalar(selection, control) => {
                selection.validate(Some(control))?;
                control.scalar().map_err(failure)?.to_string()
            }
            Self::GradientStop(editor, point, component) => editor.stop_value(point, *component)?,
            Self::Modern(p) => match p.value() {
                ModernNodeValue::Int32(v) => v.to_string(),
                ModernNodeValue::Double(v) => v.to_string(),
                _ => return Err("The property changed type.".into()),
            },
            Self::ModernAxis(p, axis) => match p.value() {
                ModernNodeValue::Vector3D(v) => v[*axis].to_string(),
                _ => return Err("The property changed type.".into()),
            },
        })
    }
    fn limits(&self) -> Option<SharedLimits> {
        match self {
            Self::Number(p) | Self::Axis(p, _) => p.retained_limits(),
            Self::Generator(p) | Self::GeneratorAxis(p, _) => p.limits().ok(),
            Self::Modern(p) => p.component_limits(0),
            Self::ModernAxis(p, axis) => p.component_limits(*axis),
            _ => None,
        }
    }
    fn parse_number(&self, text: &str) -> Result<f64, String> {
        let value = text.parse::<f64>().map_err(|_| "Enter a number.")?;
        if !value.is_finite() {
            return Err("Enter a finite number.".into());
        }
        if self.is_checkerboard_dimension() && value.fract() != 0.0 {
            return Err("Enter a whole number.".into());
        }
        if let Some(limits) = self.limits() {
            let bounds = limits.snapshot();
            let minimum = bounds.minimum.to_framework_f64();
            let maximum = bounds.maximum.to_framework_f64();
            if value < minimum || value > maximum {
                return Err(format!("Enter a value from {minimum} to {maximum}."));
            }
        }
        Ok(value)
    }
    fn is_checkerboard_dimension(&self) -> bool {
        matches!(
            self,
            Self::Number(property)
                if property.owner().kind() == mycela_engine::PointwiseKind::Checkerboard
        )
    }
    fn commit(&self, text: &str) -> Result<(), String> {
        let variable_index = || {
            text.parse::<i32>()
                .ok()
                .filter(|index| (-1..=19).contains(index))
                .ok_or_else(|| "Enter an integer from -1 to 19.".to_string())
        };
        if let Self::VariableIndex(p) = self {
            return p
                .set_binding_index_notifying(variable_index()?)
                .map_err(failure);
        }
        if let Self::VariableMapping(p) = self {
            let value: Vec<u16> = text.encode_utf16().collect();
            return p.set_binding_name_notifying(Some(&value)).map_err(failure);
        }
        if let Self::GeneratorVariableIndex(p) = self {
            return p
                .set_binding_index_notifying(variable_index()?)
                .map_err(failure);
        }
        if let Self::GeneratorVariableMapping(p) = self {
            let value: Vec<u16> = text.encode_utf16().collect();
            return p.set_binding_name_notifying(Some(&value)).map_err(failure);
        }
        if let Self::SolidVariableIndex(node, property) = self {
            return node
                .set_solid_binding_index(*property, variable_index()?)
                .map_err(failure);
        }
        if let Self::SolidVariableMapping(node, property) = self {
            let value: Vec<u16> = text.encode_utf16().collect();
            return node
                .set_solid_binding_mapping(*property, Some(&value))
                .map_err(failure);
        }
        if let Self::GradientStop(editor, point, component) = self {
            return editor.set_stop_value(point, *component, text);
        }
        if let Self::Text(p) = self {
            return p
                .set_text(Some(&text.encode_utf16().collect::<Vec<_>>()))
                .map_err(failure);
        }
        if let Self::Modern(p) = self
            && matches!(p.value(), ModernNodeValue::Int32(_))
        {
            let value = text
                .parse::<i32>()
                .map_err(|_| "Enter an integer.".to_string())?;
            if let Some(limits) = p.component_limits(0) {
                let bounds = limits.snapshot();
                let minimum = bounds.minimum.to_framework_f64();
                let maximum = bounds.maximum.to_framework_f64();
                if f64::from(value) < minimum || f64::from(value) > maximum {
                    return Err(format!("Enter a value from {minimum} to {maximum}."));
                }
            }
            return p.set_int32(value).map_err(failure);
        }
        let value = self.parse_number(text)?;
        match self {
            Self::Environment(..) => unreachable!("Environment fields are applied together"),
            Self::CanvasArea(..) => unreachable!("Canvas Area fields are applied together"),
            Self::CanvasShape(..) => unreachable!("Canvas shape fields are applied together"),
            Self::Number(p) => {
                validate_filter_control(p)?;
                p.set_number(value).map_err(failure)
            }
            Self::FilterColor(p, axis) => {
                validate_filter_control(p)?;
                let current = p.color().map_err(failure)?;
                let mut rgb = current.components();
                rgb[*axis] = value;
                p.set_color(current.with_components(rgb)).map_err(failure)
            }
            Self::Axis(p, axis) => p.set_axis(*axis, value).map_err(failure),
            Self::Text(_)
            | Self::GradientStop(..)
            | Self::VariableIndex(_)
            | Self::VariableMapping(_)
            | Self::GeneratorVariableIndex(_)
            | Self::GeneratorVariableMapping(_)
            | Self::SolidVariableIndex(..)
            | Self::SolidVariableMapping(..) => unreachable!(),
            Self::Generator(p) => p.set_number(value).map_err(failure),
            Self::GeneratorAxis(p, axis) => p.set_axis(*axis, value).map_err(failure),
            Self::Color(node, channel) => {
                if !(0.0..=255.0).contains(&value) {
                    return Err("Enter a color component from 0 to 255.".into());
                }
                node.set_solid_color_component(*channel as i32, value / 255.)
                    .map_err(failure)
            }
            Self::Transparency(node) => {
                if !(0.0..=255.0).contains(&value) {
                    return Err("Enter opacity from 0 to 255.".into());
                }
                node.set_solid_transparency(1. - value / 255.)
                    .map_err(failure)
            }
            Self::Matrix(matrix, radius) => {
                if *radius {
                    matrix.payload().set_radius(value);
                } else {
                    matrix.payload().set_angle(value);
                }
                matrix.notify().map_err(failure)
            }
            Self::LightPosition(..) => {
                unreachable!("Light position coordinates are applied together")
            }
            Self::RegionCoordinate(..) => unreachable!("Region coordinates are applied together"),
            Self::AnimationBound(parameter, animation, curve, axis, side) => {
                Self::check_animation_curve(parameter, animation, curve, *axis)?;
                let value = value as f32;
                if !value.is_finite() {
                    return Err("Enter a finite curve output.".into());
                }
                set_curve_bound(curve, *side, value);
                Ok(())
            }
            Self::LayerScalar(selection, control) => {
                selection.validate(Some(control))?;
                let value = value as f32;
                if !value.is_finite() {
                    return Err("Enter a finite number.".into());
                }
                set_layer_scalar(control, value)
            }
            Self::Modern(p) => p.set_double(value).map_err(failure),
            Self::ModernAxis(p, axis) => {
                let ModernNodeValue::Vector3D(mut current) = p.value() else {
                    return Err("The property changed type.".into());
                };
                current[*axis] = value;
                p.set_vector3d(current).map_err(failure)
            }
        }
    }
}

fn color_byte_text(normalized: f64) -> String {
    ((normalized * 255. * 1_000_000.).round() / 1_000_000.).to_string()
}

impl Draft {
    pub(super) fn filter_color_component(&self) -> Option<(&Parameter, usize)> {
        match &self.binding {
            Binding::FilterColor(property, axis) => Some((property, *axis)),
            _ => None,
        }
    }

    pub(super) fn control(&self, disabled: bool) -> NumericField {
        use Binding::*;
        // These legacy fields accept any finite value. Keep a usable thumb
        // range around the editor's ordinary units and include imported values.
        let flexible = |minimum: f64, maximum: f64, step| {
            NumericRange::new(minimum, maximum, step)
                .including(self.loaded.parse::<f64>().unwrap_or(0.0))
        };
        let percent = NumericRange::new(0.0, 100.0, 0.1);
        let unit = NumericRange::new(0.0, 1.0, 0.001);
        let color = NumericRange::new(0.0, 255.0, 1.0);
        let range = self
            .binding
            .limits()
            .as_ref()
            .and_then(NumericRange::from_limits)
            .or_else(|| {
                Some(match &self.binding {
                    CanvasArea(_, field) => NumericRange::bounds(field.bounds(), 0.1),
                    CanvasShape(_, field) => NumericRange::bounds(field.bounds(), 0.1),
                    Environment(_, field) => NumericRange::bounds(field.bounds(), 0.01),
                    VariableIndex(_) | GeneratorVariableIndex(_) | SolidVariableIndex(..) => {
                        NumericRange::new(-1.0, 19.0, 1.0)
                    }
                    FilterColor(..) => unit,
                    RegionCoordinate(_, region, axis) => {
                        let (_, x, y, height) = region.fields();
                        let aspect = region.aspect();
                        let maximum = match axis {
                            0 => (1.0 - height * aspect).max(0.0),
                            1 => (1.0 - height).max(0.0),
                            _ => ((1.0 - x) / aspect).min(1.0 - y).clamp(0.0, 1.0),
                        };
                        NumericRange::bounds([0.0, maximum], 0.001)
                    }
                    Color(..) | Transparency(_) => color,
                    Matrix(_, true) => flexible(0.0, 100.0, 0.1),
                    Matrix(_, false) => flexible(-360.0, 360.0, 1.0),
                    LightPosition(..) => flexible(-3.0, 3.0, 0.01),
                    AnimationBound(parameter, ..) if parameter.has_animation_palette() => percent,
                    AnimationBound(parameter, _, _, axis, _) => match parameter {
                        AnimationParameter::DirectCurve(p) | AnimationParameter::Filter(p) => {
                            p.retained_limits()
                        }
                        AnimationParameter::Generator(p) => p.limits().ok(),
                        AnimationParameter::Modern(p) => p.component_limits(*axis),
                        AnimationParameter::Solid(..) => return Some(unit),
                    }
                    .as_ref()
                    .and_then(NumericRange::from_limits)
                    .unwrap_or_else(|| flexible(0.0, 100.0, 0.1)),
                    LayerScalar(_, control) => control
                        .bounds()
                        .map(|bounds| NumericRange::bounds(bounds, 0.1))
                        .unwrap_or_else(|| flexible(-100.0, 100.0, 0.1)),
                    GradientStop(editor, _, 0) => NumericRange::new(
                        0.0,
                        f64::from(editor.width().saturating_sub(1).max(0)),
                        1.0,
                    ),
                    GradientStop(..) => unit,
                    Number(_) | Generator(_) | GeneratorAxis(..) | Axis(..) | Modern(_)
                    | ModernAxis(..) => flexible(-100.0, 100.0, 0.1),
                    Text(_)
                    | VariableMapping(_)
                    | GeneratorVariableMapping(_)
                    | SolidVariableMapping(..) => return None,
                })
            });
        let checkerboard_dimension = self.binding.is_checkerboard_dimension();
        NumericField::new(&self.input, self.id.clone(), self.label.clone(), range)
            .disabled(disabled)
            .live(self.is_live() && !self.changes_socket_count())
            .logarithmic(checkerboard_dimension)
            .whole_numbers(checkerboard_dimension)
    }

    fn new(
        id: String,
        label: String,
        binding: Binding,
        loaded: String,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).default_value(loaded.clone()));
        Self {
            id,
            label,
            input,
            loaded,
            binding,
            live_error: None,
        }
    }

    pub(super) fn is_live(&self) -> bool {
        matches!(
            self.binding,
            Binding::Number(_)
                | Binding::FilterColor(..)
                | Binding::Axis(..)
                | Binding::Text(_)
                | Binding::Generator(_)
                | Binding::GeneratorAxis(..)
                | Binding::Color(..)
                | Binding::Transparency(_)
                | Binding::Matrix(..)
                | Binding::Modern(_)
                | Binding::ModernAxis(..)
        )
    }

    pub(super) fn changes_socket_count(&self) -> bool {
        let Binding::Number(property) = &self.binding else {
            return false;
        };
        property.name().iter().copied().eq("Inputs".encode_utf16())
    }

    pub(super) fn commit_live(&mut self, cx: &App) -> bool {
        let text = self.input.read(cx).value();
        let current = match self.binding.text() {
            Ok(value) => value,
            Err(error) => {
                self.live_error = Some(error);
                return false;
            }
        };
        if text.as_ref() == current {
            self.loaded = current;
            self.live_error = None;
            return false;
        }
        let component = self.binding.solid_component();
        self.live_error = self.binding.commit(&text).err();
        if self.live_error.is_none() {
            self.loaded = text.to_string();
        }
        self.binding.text().is_ok_and(|value| value != current)
            || component != self.binding.solid_component()
    }

    pub(super) fn numeric_lab_property(
        &self,
    ) -> Option<(
        mycela_engine::LabNumericProperty<ForegroundError>,
        mycela_engine::LabNumericAxis,
    )> {
        use mycela_engine::{LabNumericAxis as Axis, LabNumericProperty as Lab};
        let axis = |index| if index == 0 { Axis::X } else { Axis::Y };
        Some(match &self.binding {
            Binding::Number(value) => (Lab::filter(value)?, Axis::Whole),
            Binding::Axis(value, component) => (Lab::filter(value)?, axis(*component)),
            Binding::Generator(value) => (Lab::generator(value)?, Axis::Whole),
            Binding::GeneratorAxis(value, component) => (Lab::generator(value)?, axis(*component)),
            Binding::Transparency(node) => (Lab::transparency(node)?, Axis::Whole),
            _ => return None,
        })
    }

    pub(super) fn environment_numeric_receiver(
        &self,
    ) -> Option<(&EnvironmentPanel, mycela_engine::EnvironmentNumericReceiver)> {
        let Binding::Environment(panel, field) = &self.binding else {
            return None;
        };
        let receiver = (*field).into();
        panel
            .value()
            .supports_numeric_adjuster(receiver)
            .then_some((panel, receiver))
    }

    pub fn is_environment(&self) -> bool {
        matches!(self.binding, Binding::Environment(..))
            || matches!(&self.binding, Binding::GradientStop(editor, ..) if editor.environment().is_some())
    }

    pub fn canvas_area_field(&self) -> Option<CanvasAreaField> {
        match self.binding {
            Binding::CanvasArea(_, field) => Some(field),
            _ => None,
        }
    }

    pub fn canvas_shape_field(&self) -> Option<CanvasShapeField> {
        match self.binding {
            Binding::CanvasShape(_, field) => Some(field),
            _ => None,
        }
    }

    fn stage_environment(
        &self,
        map: &mut EnvironmentRecord,
        gradients: &mut [Option<SharedGradient>; 2],
        cx: &App,
    ) -> Result<bool, String> {
        if !self.is_changed(cx) {
            return Ok(false);
        }
        if let Binding::GradientStop(editor, point, component) = &self.binding
            && let Some((_, _, kind)) = editor.environment()
        {
            if self.binding.text()? != self.loaded {
                return Err("The gradient stop changed. Revert fields before continuing.".into());
            }
            let draft = &mut gradients[kind as usize];
            if draft.is_none() {
                *draft = Some(editor.copy_for_edit()?);
            }
            let gradient = draft.as_ref().expect("gradient copy");
            let point = editor.copy_stop(point, gradient)?;
            GradientEditor::write_stop(&point, *component, &self.input.read(cx).value())?;
            return Ok(true);
        }
        let Binding::Environment(_, field) = self.binding else {
            return Ok(false);
        };
        let value = self
            .input
            .read(cx)
            .value()
            .parse::<f64>()
            .map_err(|_| format!("{} requires a number.", self.label))?;
        map.set_field(field, value).map_err(|error| {
            let [min, max] = field.bounds();
            match error {
                mycela_engine::EnvironmentEditError::Bounds => {
                    format!("{} must be between {min} and {max}.", self.label)
                }
                _ => error.to_string(),
            }
        })
    }

    pub fn is_layer_controlled(&self) -> bool {
        matches!(&self.binding, Binding::LayerScalar(_, control) if control.kind() == CurveControlKind::Modulated && control.is_active())
    }
    pub(super) fn is_controlled_selector_color(&self) -> bool {
        matches!(
            &self.binding,
            Binding::FilterColor(property, _)
                if property.owner().kind() == mycela_engine::PointwiseKind::Selector
                    && !SelectorColorEditor::can_edit_owned_color(property)
        )
    }
    pub fn is_binding(&self) -> bool {
        matches!(
            self.binding,
            Binding::VariableIndex(_)
                | Binding::VariableMapping(_)
                | Binding::GeneratorVariableIndex(_)
                | Binding::GeneratorVariableMapping(_)
                | Binding::SolidVariableIndex(..)
                | Binding::SolidVariableMapping(..)
        )
    }
    pub fn is_unrecorded(&self) -> bool {
        self.is_binding()
    }
    pub fn is_light_position(&self) -> bool {
        matches!(self.binding, Binding::LightPosition(..))
    }
    pub fn is_region_coordinate(&self) -> bool {
        matches!(self.binding, Binding::RegionCoordinate(..))
    }
    pub fn is_animation(&self) -> bool {
        matches!(self.binding, Binding::AnimationBound(..))
    }
    pub(super) fn edits_recorded_curve(&self) -> bool {
        match &self.binding {
            Binding::AnimationBound(parameter, ..) => parameter.records_curve_history(),
            Binding::LayerScalar(selection, _) => selection.parameter.records_curve_history(),
            _ => false,
        }
    }
    pub fn is_animation_palette(&self) -> bool {
        matches!(&self.binding, Binding::GradientStop(editor, ..) if editor.is_animation_palette())
    }
    pub(crate) fn is_colorize_stop(&self) -> bool {
        matches!(&self.binding, Binding::GradientStop(editor, ..) if editor.is_colorize())
    }

    pub(crate) fn stage_colorize_stop(
        &self,
        gradient: &mut Option<SharedGradient>,
        cx: &App,
    ) -> Result<(), String> {
        let Binding::GradientStop(editor, point, component) = &self.binding else {
            return Ok(());
        };
        if !(editor.is_colorize() && self.is_changed(cx)) {
            return Ok(());
        }
        if self.binding.text()? != self.loaded {
            return Err("The gradient stop changed. Revert fields before continuing.".into());
        }
        if gradient.is_none() {
            *gradient = Some(editor.copy_for_edit()?);
        }
        let copy = editor.copy_stop(point, gradient.as_ref().expect("gradient copy"))?;
        GradientEditor::write_stop(&copy, *component, &self.input.read(cx).value())
            .map_err(|error| format!("{}: {error}", self.label))
    }
    pub fn is_stored_solid_value(&self) -> bool {
        matches!(self.binding, Binding::Color(..) | Binding::Transparency(..))
    }
    pub fn is_changed(&self, cx: &App) -> bool {
        self.input.read(cx).value().as_ref() != self.loaded
    }
    pub(super) fn validate_pending_owner(&self, cx: &App) -> Result<(), String> {
        if self.is_changed(cx) && self.is_binding() && self.binding.text()? != self.loaded {
            return Err(format!(
                "{} changed. Reset field text before continuing.",
                self.label
            ));
        }
        Ok(())
    }
    pub fn commit(&self, cx: &App) -> Result<(), String> {
        if !self.is_changed(cx) {
            return Ok(());
        }
        self.validate_pending_owner(cx)?;
        self.binding
            .commit(self.input.read(cx).value().as_ref())
            .map_err(|e| format!("{}: {e}", self.label))
    }

    pub fn stage_animation(
        &self,
        edits: &mut Vec<AnimationEdit>,
        cx: &App,
    ) -> Result<bool, String> {
        if !self.is_changed(cx) {
            return Ok(false);
        }
        if let Binding::GradientStop(editor, point, component) = &self.binding {
            if let Some((parameter, animation)) = editor.animation() {
                let result = (|| {
                    if self.binding.text()? != self.loaded {
                        return Err(
                            "The palette stop changed. Revert fields before continuing.".into()
                        );
                    }
                    let expected = AnimationOwner::Legacy(animation.clone());
                    let edit = AnimationEdit::for_property(edits, parameter, &expected)?;
                    let gradient = edit.palette()?;
                    let copy = editor.copy_stop(point, &gradient)?;
                    GradientEditor::write_stop(
                        &copy,
                        *component,
                        self.input.read(cx).value().as_ref(),
                    )?;
                    edit.mark_palette_changed();
                    Ok(true)
                })();
                return result.map_err(|error: String| format!("{}: {error}", self.label));
            }
            return Ok(false);
        }
        let (parameter, animation) = match &self.binding {
            Binding::AnimationBound(parameter, animation, ..) => (parameter, animation),
            Binding::LayerScalar(selection, _) => (&selection.parameter, &selection.animation),
            _ => return Ok(false),
        };
        if !parameter.uses_animation_replacement() {
            return Ok(false);
        }
        let result = (|| {
            let edit = AnimationEdit::for_property(edits, parameter, animation)?;
            let value = self
                .input
                .read(cx)
                .value()
                .as_ref()
                .parse::<f64>()
                .map_err(|_| "Enter a number.")? as f32;
            if !value.is_finite() {
                return Err("Enter a finite number.".into());
            }
            match &self.binding {
                Binding::AnimationBound(parameter, animation, curve, axis, side) => {
                    Binding::check_animation_curve(parameter, animation, curve, *axis)?;
                    let curve = edit
                        .owner
                        .curve(*axis)?
                        .ok_or("The animation curve is unavailable.")?;
                    set_curve_bound(&curve, *side, value);
                }
                Binding::LayerScalar(selection, control) => {
                    selection.validate(Some(control))?;
                    let copy = selection.on_animation(&edit.owner)?;
                    set_layer_scalar(&selection.copy_control(control, &copy)?, value)?;
                }
                _ => unreachable!(),
            }
            edit.changed = true;
            Ok(true)
        })();
        result.map_err(|error: String| format!("{}: {error}", self.label))
    }
}

fn set_curve_bound(curve: &SharedCurve<ForegroundError>, side: usize, value: f32) {
    if side == 0 {
        curve.set_minimum(value);
    } else {
        curve.set_maximum(value);
    }
}

fn set_layer_scalar(
    control: &CurveControlHandle<ForegroundError>,
    value: f32,
) -> Result<(), String> {
    if control.kind() == CurveControlKind::Modulated && control.is_active() {
        return Err("This control is driven by another curve.".into());
    }
    if let Some([min, max]) = control.bounds()
        && !(min..=max).contains(&value)
    {
        return Err(format!("Enter a value from {min} to {max}."));
    }
    control.set_scalar(value).map_err(failure)
}
#[derive(Clone)]
pub(crate) enum AnimationParameter {
    DirectCurve(Parameter),
    Filter(Parameter),
    Generator(GeneratorParameter),
    Solid(Node, SolidColorProperty),
    Modern(ModernProperty),
}
#[derive(Clone)]
pub(crate) enum AnimationOwner {
    DirectCurve(SharedCurve<ForegroundError>),
    Legacy(GradientAnimation<ForegroundError>),
    Modern(ModernNodeAnimation<ForegroundError>),
}
impl AnimationOwner {
    pub(crate) fn same_reference(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::DirectCurve(a), Self::DirectCurve(b)) => a.same_reference(b),
            (Self::Legacy(a), Self::Legacy(b)) => a.same_reference(b),
            (Self::Modern(a), Self::Modern(b)) => a.same_reference(b),
            _ => false,
        }
    }
    fn flags(&self) -> Option<mycela_engine::AnimationFlags> {
        match self {
            Self::DirectCurve(_) => None,
            Self::Legacy(a) => a.flags(),
            Self::Modern(a) => a.flags(),
        }
    }
    fn curves(&self) -> Option<mycela_engine::AnimationCurves<ForegroundError>> {
        match self {
            Self::DirectCurve(_) => None,
            Self::Legacy(a) => a.curves(),
            Self::Modern(a) => a.curves(),
        }
    }
    pub(crate) fn curve(
        &self,
        axis: usize,
    ) -> Result<Option<SharedCurve<ForegroundError>>, String> {
        if let Self::DirectCurve(curve) = self {
            return if axis == 0 {
                Ok(Some(curve.clone()))
            } else {
                Err("Curve axis is unavailable.".into())
            };
        }
        self.curves()
            .ok_or("Animation curves are unavailable.")?
            .value(axis as i32)
            .map_err(failure)
    }
    pub(crate) fn ensure_curve(&self, axis: usize) -> Result<(), String> {
        match self {
            Self::DirectCurve(_) => Err("A direct curve cannot be replaced here.".into()),
            Self::Legacy(a) => a.ensure_curve(axis as i32).map_err(failure),
            Self::Modern(a) => a.ensure_curve(axis as i32).map_err(failure),
        }
    }
}
impl AnimationParameter {
    pub(super) fn animation_link_conflict(&self) -> Option<&'static str> {
        let Self::Filter(property) = self else {
            return None;
        };
        match property.owner().kind() {
            mycela_engine::PointwiseKind::GetChannel
                if property
                    .retained_adjustment()
                    .is_some_and(|link| link.is_active().unwrap_or(true)) =>
            {
                Some("Unlink the Channel adjuster to edit its animation.")
            }
            mycela_engine::PointwiseKind::Selector
                if matches!(property.value(), PointwiseParameterValue::Color(_))
                    && SelectorColorEditor::has_lab_link(property) =>
            {
                Some("Unlink the Color adjuster to edit its animation.")
            }
            _ => None,
        }
    }

    fn uses_animation_replacement(&self) -> bool {
        match self {
            Self::Generator(property) => property.owner().kind() == ImageGeneratorKind::Gradient,
            Self::Filter(property) => {
                property.wizard_variable_index().is_some() || self.tracks_filter_curve_history()
            }
            Self::Solid(..) => true,
            Self::DirectCurve(_) => true,
            Self::Modern(_) => false,
        }
    }

    fn same_replacement_property(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Generator(a), Self::Generator(b)) => a.same_reference(b),
            (Self::Filter(a), Self::Filter(b)) | (Self::DirectCurve(a), Self::DirectCurve(b)) => {
                a.same_reference(b)
            }
            (Self::Solid(a, ak), Self::Solid(b, bk)) => ak == bk && a.same_reference(b),
            _ => false,
        }
    }

    fn replace_legacy_animation(
        &self,
        animation: GradientAnimation<ForegroundError>,
    ) -> Result<(), String> {
        match self {
            Self::Generator(property) => property.set_animation(Some(animation)).map_err(failure),
            Self::Filter(property) => {
                validate_filter_control(property)?;
                property.set_animation(Some(animation)).map_err(failure)
            }
            Self::Solid(node, property) => node
                .set_solid_animation(*property, Some(animation))
                .map_err(failure),
            _ => Err("The animation changed type.".into()),
        }
    }

    pub(crate) fn apply_curve_preset(
        &self,
        document: &LiveDocument,
        axis: usize,
        animation: &AnimationOwner,
        curve: &SharedCurve<ForegroundError>,
        preset: &SharedCurve<ForegroundError>,
        insert: bool,
    ) -> Result<(), String> {
        match (self, animation) {
            (Self::DirectCurve(property), AnimationOwner::DirectCurve(_)) => guard(document)
                .apply_composite_curve_preset(property, curve, preset, insert)
                .map_err(failure),
            (_, AnimationOwner::Legacy(animation)) => {
                let next = animation
                    .with_curve_preset(axis as i32, preset, insert)
                    .map_err(failure)?;
                self.replace_legacy_animation(next)
            }
            (Self::Modern(property), AnimationOwner::Modern(animation)) => {
                let next = animation
                    .with_curve_preset(axis as i32, preset, insert)
                    .map_err(failure)?;
                property.set_animation(Some(&next)).map_err(failure)
            }
            _ => Err("The selected animation changed type.".into()),
        }
    }

    pub(crate) fn animation_edit(
        &self,
        expected: Option<&AnimationOwner>,
    ) -> Result<Option<AnimationEdit>, String> {
        self.uses_animation_replacement()
            .then(|| AnimationEdit::new(self, expected))
            .transpose()
    }

    fn tracks_filter_curve_history(&self) -> bool {
        matches!(
            self,
            Self::Filter(property) if matches!(
                property.owner().kind(),
                mycela_engine::PointwiseKind::ToneCurve
                    | mycela_engine::PointwiseKind::FreeTransform
                    | mycela_engine::PointwiseKind::Kaleidoscope
                    | mycela_engine::PointwiseKind::MoveRepeat
                    | mycela_engine::PointwiseKind::RotateTile
                    | mycela_engine::PointwiseKind::Distort
                    | mycela_engine::PointwiseKind::ChangeBci
                    | mycela_engine::PointwiseKind::ChangeHsb
                    | mycela_engine::PointwiseKind::Exposure
                    | mycela_engine::PointwiseKind::Selector
                    | mycela_engine::PointwiseKind::MatchColor
                    | mycela_engine::PointwiseKind::GetChannel
                    | mycela_engine::PointwiseKind::GaussianBlur
                    | mycela_engine::PointwiseKind::MotionBlur
                    | mycela_engine::PointwiseKind::ShapeBlur
                    | mycela_engine::PointwiseKind::Smear
                    | mycela_engine::PointwiseKind::VariableBlur
                    | mycela_engine::PointwiseKind::Sharpen
                    | mycela_engine::PointwiseKind::Atmosphere
                    | mycela_engine::PointwiseKind::ConvertEnvironment
                    | mycela_engine::PointwiseKind::DiffuseEnvironment
                    | mycela_engine::PointwiseKind::ScatterEnvironment
                    | mycela_engine::PointwiseKind::Stereogram
                    | mycela_engine::PointwiseKind::PointLight
                    | mycela_engine::PointwiseKind::FullLight
                    | mycela_engine::PointwiseKind::Shadow
                    | mycela_engine::PointwiseKind::ShadowG2
            )
        )
    }
    pub(crate) fn records_curve_history(&self) -> bool {
        self.is_direct_curve() || self.tracks_filter_curve_history()
    }
    pub(crate) fn is_direct_curve(&self) -> bool {
        matches!(self, Self::DirectCurve(_))
    }
    pub(crate) fn is_solid_color_animation(&self) -> bool {
        matches!(self, Self::Solid(_, SolidColorProperty::Color))
    }
    pub(super) fn has_animation_palette(&self) -> bool {
        self.is_solid_color_animation()
            || matches!(
                self,
                Self::Filter(property)
                    if property.owner().kind() == mycela_engine::PointwiseKind::Selector
                        && matches!(property.value(), PointwiseParameterValue::Color(_))
            )
    }
    pub(crate) fn is_solid_animation(&self) -> bool {
        matches!(self, Self::Solid(..))
    }
    pub(crate) fn retained(&self) -> Result<Option<AnimationOwner>, String> {
        match self {
            Self::DirectCurve(p) => match p.value() {
                PointwiseParameterValue::Curve(value) => Ok(value.map(AnimationOwner::DirectCurve)),
                _ => Err("The curve property changed type.".into()),
            },
            Self::Filter(p) => {
                validate_filter_control(p)?;
                p.retained_animation()
                    .map(|v| v.map(AnimationOwner::Legacy))
                    .map_err(failure)
            }
            Self::Generator(p) => p
                .retained_animation()
                .map(|v| v.map(AnimationOwner::Legacy))
                .map_err(failure),
            Self::Solid(node, property) => node
                .retained_solid_animation(*property)
                .map(|owner| owner.map(AnimationOwner::Legacy))
                .map_err(failure),
            Self::Modern(p) => Ok(p.retained_animation().map(AnimationOwner::Modern)),
        }
    }
    pub(crate) fn ensure(&self) -> Result<AnimationOwner, String> {
        match self {
            Self::DirectCurve(p) => p.curve().map(AnimationOwner::DirectCurve).map_err(failure),
            Self::Filter(p) => {
                validate_filter_control(p)?;
                p.animation().map(AnimationOwner::Legacy).map_err(failure)
            }
            Self::Generator(p) => p.animation().map(AnimationOwner::Legacy).map_err(failure),
            Self::Solid(node, property) => node
                .solid_animation(*property)
                .map(AnimationOwner::Legacy)
                .map_err(failure),
            Self::Modern(p) => p
                .animation()
                .map_err(failure)?
                .map(AnimationOwner::Modern)
                .ok_or_else(|| "This property cannot be animated.".into()),
        }
    }
    pub(crate) fn axis(
        &self,
        axis: usize,
    ) -> Result<(bool, Option<SharedCurve<ForegroundError>>), String> {
        let Some(owner) = self.retained()? else {
            return Ok((false, None));
        };
        if matches!(owner, AnimationOwner::DirectCurve(_)) {
            return Ok((axis == 0, owner.curve(axis)?));
        }
        let enabled = owner
            .flags()
            .ok_or("Animation flags are unavailable.")?
            .value(axis as i32)
            .map_err(failure)?;
        Ok((enabled, owner.curve(axis)?))
    }
    pub(crate) fn set_axis(
        &self,
        owner: &AnimationOwner,
        axis: usize,
        enabled: bool,
    ) -> Result<(), String> {
        match self {
            Self::DirectCurve(_) => Err("A direct curve cannot be disabled here.".into()),
            Self::Filter(p) => {
                validate_filter_control(p)?;
                p.set_axis_animated(axis, enabled).map_err(failure)
            }
            // Existing generator/scene editors mutate the actual retained flag array.
            Self::Generator(_) | Self::Modern(_) => owner
                .flags()
                .ok_or("Animation flags are unavailable.")?
                .set(axis as i32, enabled)
                .map_err(failure),
            Self::Solid(..) => Err("Solid Color animation edits require a replacement.".into()),
        }
    }
}

/// Work on a detached editable curve or animation. Publish through its owning
/// property so the previous owner remains available to history.
pub(crate) struct AnimationEdit {
    property: Box<AnimationParameter>,
    original: Option<AnimationOwner>,
    pub owner: AnimationOwner,
    changed: bool,
    palette_changed: bool,
}
impl AnimationEdit {
    fn new(
        property: &AnimationParameter,
        expected: Option<&AnimationOwner>,
    ) -> Result<Self, String> {
        let current = property.retained()?;
        let original = match (current, expected) {
            (Some(current), Some(expected)) if current.same_reference(expected) => Some(current),
            (None, None) if property.tracks_filter_curve_history() => None,
            (None, None) => Some(property.ensure()?),
            _ => {
                return Err(
                    "The selected animation changed. Revert fields before continuing.".into(),
                );
            }
        };
        let owner = match (&original, property) {
            (Some(AnimationOwner::Legacy(original)), _) => {
                AnimationOwner::Legacy(original.source_clone().map_err(failure)?)
            }
            (Some(AnimationOwner::DirectCurve(original)), _) => {
                AnimationOwner::DirectCurve(original.source_clone().map_err(failure)?)
            }
            (None, AnimationParameter::Filter(property)) => {
                AnimationOwner::Legacy(property.detached_animation().map_err(failure)?)
            }
            _ => return Err("The animation changed type.".into()),
        };
        Ok(Self {
            property: Box::new(property.clone()),
            owner,
            original,
            changed: false,
            palette_changed: false,
        })
    }

    pub(crate) fn for_property<'a>(
        edits: &'a mut Vec<Self>,
        property: &AnimationParameter,
        expected: &AnimationOwner,
    ) -> Result<&'a mut Self, String> {
        if let Some(index) = edits
            .iter()
            .position(|edit| edit.property.same_replacement_property(property))
        {
            if !edits[index]
                .original
                .as_ref()
                .is_some_and(|original| original.same_reference(expected))
            {
                return Err("The animation changed. Revert fields before continuing.".into());
            }
            return Ok(&mut edits[index]);
        }
        edits.push(Self::new(property, Some(expected))?);
        Ok(edits.last_mut().expect("appended animation edit"))
    }

    pub fn set_axis(&self, axis: usize, enabled: bool) -> Result<(), String> {
        self.owner
            .flags()
            .ok_or("Animation flags are unavailable.")?
            .set(axis as i32, enabled)
            .map_err(failure)
    }
    pub(crate) fn palette(&self) -> Result<SharedGradient, String> {
        let AnimationOwner::Legacy(owner) = &self.owner else {
            unreachable!();
        };
        owner.gradient().map_err(failure)
    }
    pub(crate) fn mark_palette_changed(&mut self) {
        self.changed = true;
        self.palette_changed = true;
    }

    pub fn apply_staged(
        self,
        selection: Option<&LayerSelection>,
        document: &LiveDocument,
    ) -> Result<Option<LayerSelection>, String> {
        if !self.changed {
            return Ok(None);
        }
        let next = self.copy_selection(selection)?;
        self.publish(Some(document))?;
        Ok(next)
    }

    pub fn copy_selection(
        &self,
        selection: Option<&LayerSelection>,
    ) -> Result<Option<LayerSelection>, String> {
        selection
            .filter(|selection| {
                selection
                    .parameter
                    .same_replacement_property(&self.property)
            })
            .filter(|selection| selection.validate(None).is_ok())
            .map(|selection| selection.on_animation(&self.owner))
            .transpose()
    }

    pub fn publish(self, document: Option<&LiveDocument>) -> Result<(), String> {
        let current = self.property.retained()?;
        let is_current = match (current.as_ref(), self.original.as_ref()) {
            (Some(current), Some(original)) => current.same_reference(original),
            (None, None) => true,
            _ => false,
        };
        if !is_current {
            return Err("The selected animation changed while editing.".into());
        }
        if self.palette_changed {
            self.palette()?.rebuild().map_err(failure)?;
        }
        match (&*self.property, &self.original, self.owner) {
            (
                AnimationParameter::DirectCurve(property),
                Some(AnimationOwner::DirectCurve(original)),
                AnimationOwner::DirectCurve(owner),
            ) => match document {
                Some(document) => guard(document)
                    .edit_composite_curve(property, original, owner, "Edit Curve")
                    .map_err(failure),
                None => property.edit_curve(original, owner).map_err(failure),
            },
            (_, None | Some(AnimationOwner::Legacy(_)), AnimationOwner::Legacy(owner)) => {
                self.property.replace_legacy_animation(owner)
            }
            _ => Err("The animation changed type.".into()),
        }
    }
}
#[derive(Clone)]
pub(crate) struct AnimationToggle {
    pub id: String,
    pub label: String,
    pub parameter: AnimationParameter,
    pub axis: usize,
    pub enabled: bool,
    pub animation: Option<AnimationOwner>,
    pub curve: Option<SharedCurve<ForegroundError>>,
}
#[derive(Clone)]
pub(crate) struct LayerSelection {
    pub parameter: AnimationParameter,
    pub animation: AnimationOwner,
    pub axis: usize,
    pub curve: SharedCurve<ForegroundError>,
    pub layer: CurveLayerHandle<ForegroundError>,
    pub ancestry: Vec<(
        CurveLayerHandle<ForegroundError>,
        CurveControlHandle<ForegroundError>,
    )>,
}
impl LayerSelection {
    pub(crate) fn on_animation(&self, animation: &AnimationOwner) -> Result<Self, String> {
        let curve = animation
            .curve(self.axis)?
            .ok_or("The animation curve is unavailable.")?;
        let root = self
            .ancestry
            .first()
            .map_or(&self.layer, |(parent, _)| parent);
        let index = self
            .curve
            .layers()
            .iter()
            .position(|layer| {
                layer
                    .as_ref()
                    .is_some_and(|layer| layer.same_reference(root))
            })
            .ok_or("The selected animation layer changed. Choose it again.")?;
        let mut layer = curve
            .layers()
            .get(index)
            .and_then(Clone::clone)
            .ok_or("The animation layer is unavailable.")?;
        let mut ancestry = Vec::with_capacity(self.ancestry.len());
        for (parent, link) in &self.ancestry {
            let link = copy_layer_control(parent, link, &layer)?;
            let child = link
                .retained_child()
                .ok_or("The modulation curve is unavailable.")?;
            ancestry.push((layer, link));
            layer = child;
        }
        Ok(Self {
            parameter: self.parameter.clone(),
            animation: animation.clone(),
            axis: self.axis,
            curve,
            layer,
            ancestry,
        })
    }

    pub(crate) fn copy_control(
        &self,
        control: &CurveControlHandle<ForegroundError>,
        copy: &Self,
    ) -> Result<CurveControlHandle<ForegroundError>, String> {
        copy_layer_control(&self.layer, control, &copy.layer)
    }

    pub fn same_path(&self, other: &Self) -> bool {
        self.animation.same_reference(&other.animation)
            && self.axis == other.axis
            && self.curve.same_reference(&other.curve)
            && self.layer.same_reference(&other.layer)
            && self.ancestry.len() == other.ancestry.len()
            && self
                .ancestry
                .iter()
                .zip(&other.ancestry)
                .all(|((a, x), (b, y))| a.same_reference(b) && x.same_reference(y))
    }
    fn layer_controls(
        layer: &CurveLayerHandle<ForegroundError>,
    ) -> Vec<CurveControlHandle<ForegroundError>> {
        layer
            .controls()
            .into_iter()
            .flatten()
            .chain(layer.shape_controls())
            .collect()
    }
    pub fn controls(&self) -> Vec<CurveControlHandle<ForegroundError>> {
        Self::layer_controls(&self.layer)
    }
    pub fn validate(
        &self,
        control: Option<&CurveControlHandle<ForegroundError>>,
    ) -> Result<(), String> {
        Binding::check_animation_curve(&self.parameter, &self.animation, &self.curve, self.axis)?;
        let root = self
            .ancestry
            .first()
            .map_or(&self.layer, |(parent, _)| parent);
        if !self
            .curve
            .layers()
            .iter()
            .flatten()
            .any(|layer| layer.same_reference(root))
        {
            return Err("The selected animation layer changed. Choose it again.".into());
        }
        for (index, (parent, link)) in self.ancestry.iter().enumerate() {
            let child = self
                .ancestry
                .get(index + 1)
                .map_or(&self.layer, |(parent, _)| parent);
            if !Self::layer_controls(parent)
                .iter()
                .any(|current| current.same_reference(link))
                || !link
                    .retained_child()
                    .is_some_and(|current| current.same_reference(child))
            {
                return Err("A modulation curve changed. Choose the layer again.".into());
            }
        }
        if control.is_some_and(|control| {
            !self
                .controls()
                .iter()
                .any(|current| current.same_reference(control))
        }) {
            return Err("A layer control changed. Choose the layer again.".into());
        }
        Ok(())
    }
}

fn copy_layer_control(
    source: &CurveLayerHandle<ForegroundError>,
    control: &CurveControlHandle<ForegroundError>,
    copy: &CurveLayerHandle<ForegroundError>,
) -> Result<CurveControlHandle<ForegroundError>, String> {
    LayerSelection::layer_controls(source)
        .into_iter()
        .zip(LayerSelection::layer_controls(copy))
        .find_map(|(current, copy)| current.same_reference(control).then_some(copy))
        .ok_or_else(|| "The layer control changed. Choose the layer again.".into())
}
#[derive(Default)]
pub(crate) struct Inspector {
    pub drafts: Vec<Draft>,
    pub choices: Vec<ChoiceField>,
    pub animations: Vec<AnimationToggle>,
    pub notes: Vec<String>,
    pub layer: Option<LayerSelection>,
    pub custom: Option<CustomDraft>,
    pub gradient: Option<GradientEditor>,
    pub mask_gradient: Option<GradientEditor>,
    pub legacy_gradient: Option<LegacyGradientEditor>,
    pub selector_color: Option<SelectorColorEditor>,
    pub(super) checkerboard_color_open: Option<usize>,
    pub levels: Option<Entity<LevelsEditor>>,
    pub environment: Option<EnvironmentPanel>,
    pub environment_panels: Vec<EnvironmentPanel>,
    pub canvas_areas: Vec<CanvasArea>,
    pub canvas_area: Option<CanvasAreaDraft>,
    pub canvas_shape: Option<CanvasShapeDraft>,
    pub owner: Option<Node>,
}

pub(crate) struct CanvasAreaDraft {
    pub editor: CanvasArea,
    pub outline: bool,
    loaded: CanvasAreaSettings,
}

pub(crate) struct CanvasShapeDraft {
    pub editor: CanvasShape,
    pub loaded: CanvasShapeValues,
}

pub(super) struct LightPositionDraft {
    property: Parameter,
    object: PointwiseCustomObject<ForegroundError>,
    before: [f64; 2],
    after: [f64; 2],
}

pub(super) struct RegionDraft {
    property: Parameter,
    region: PointwiseRegion<ForegroundError>,
    before: (i32, f32, f32, f32),
    coordinates: [f32; 3],
}

impl RegionDraft {
    pub(super) fn commit(self) -> Result<(), String> {
        let [x, y, height] = self.coordinates;
        self.property
            .edit_region(&self.region, self.before, (self.before.0, x, y, height))
            .map_err(failure)
    }
}

impl LightPositionDraft {
    pub(super) fn commit(self) -> Result<(), String> {
        self.property
            .edit_light_position(&self.object, self.before, self.after)
            .map_err(failure)
    }
}

impl Inspector {
    pub(super) fn stage_region(&self, cx: &App) -> Result<Option<RegionDraft>, String> {
        let mut edit = None;
        for draft in self.drafts.iter().filter(|draft| draft.is_changed(cx)) {
            let Binding::RegionCoordinate(property, region, axis) = &draft.binding else {
                continue;
            };
            let current = property.region().map_err(failure)?;
            if !current.same_reference(region) || draft.binding.text()? != draft.loaded {
                return Err("The region changed. Revert fields before continuing.".into());
            }
            let value = draft
                .input
                .read(cx)
                .value()
                .trim()
                .parse::<f32>()
                .ok()
                .filter(|value| (0. ..=1.).contains(value))
                .ok_or_else(|| format!("{} needs a value from 0 to 1.", draft.label))?;
            let before @ (_, x, y, height) = current.fields();
            let edit = edit.get_or_insert_with(|| RegionDraft {
                property: property.clone(),
                region: region.clone(),
                before,
                coordinates: [x, y, height],
            });
            if !edit.property.same_reference(property) {
                return Err("The region changed. Revert fields before continuing.".into());
            }
            edit.coordinates[*axis] = value;
        }
        if let Some(edit) = &edit {
            let [x, y, height] = edit.coordinates;
            if x + height * edit.region.aspect() > 1. || y + height > 1. {
                return Err("The region must fit inside the original image.".into());
            }
        }
        Ok(edit)
    }

    pub(super) fn stage_light_position(
        &self,
        cx: &App,
    ) -> Result<Option<LightPositionDraft>, String> {
        let mut edit = None;
        for draft in self.drafts.iter().filter(|draft| draft.is_changed(cx)) {
            let Binding::LightPosition(property, object, axis) = &draft.binding else {
                continue;
            };
            Binding::check_light_position(property, object)?;
            let before = object.light_position().map_err(failure)?;
            if before[*axis].to_string() != draft.loaded {
                return Err("The light position changed. Revert fields before continuing.".into());
            }
            let value = draft
                .input
                .read(cx)
                .value()
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|value| value.is_finite())
                .ok_or_else(|| format!("{}: Enter a finite number.", draft.label))?;
            let edit = edit.get_or_insert_with(|| LightPositionDraft {
                property: property.clone(),
                object: object.clone(),
                before,
                after: before,
            });
            if !edit.property.same_reference(property) {
                return Err("The light position changed. Revert fields before continuing.".into());
            }
            edit.after[*axis] = value;
        }
        Ok(edit)
    }

    pub fn select_canvas_shape(
        &mut self,
        editor: Option<CanvasShape>,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), String> {
        let selected = editor
            .map(|editor| {
                let loaded = editor.settings().map_err(failure)?;
                Ok::<_, String>(CanvasShapeDraft { editor, loaded })
            })
            .transpose()?;
        self.drafts
            .retain(|draft| draft.canvas_shape_field().is_none());
        if let Some(shape) = &selected {
            for (field, value) in shape.loaded.fields() {
                let label = field.label();
                let key = label.to_ascii_lowercase().replace(' ', "-");
                self.drafts.push(Draft::new(
                    format!("graph-canvas-shape-{key}"),
                    label.to_owned(),
                    Binding::CanvasShape(shape.editor.clone(), field),
                    value.to_string(),
                    window,
                    cx,
                ));
            }
        }
        self.canvas_shape = selected;
        Ok(())
    }

    pub fn stage_canvas_shape(&self, cx: &App) -> Result<Option<CanvasShapeValues>, String> {
        let Some(shape) = &self.canvas_shape else {
            return Ok(None);
        };
        let mut proposed = shape.loaded.clone();
        let mut changed = false;
        for draft in &self.drafts {
            let Some(field) = draft.canvas_shape_field().filter(|_| draft.is_changed(cx)) else {
                continue;
            };
            let value = draft
                .input
                .read(cx)
                .value()
                .parse::<f32>()
                .map_err(|_| format!("{} requires a number.", draft.label))?;
            if !proposed.set_number(field, value) {
                let [min, max] = field.bounds();
                let kind = if field.is_integer() {
                    "a whole number"
                } else {
                    "a number"
                };
                return Err(format!(
                    "{} requires {kind} from {min} to {max}.",
                    draft.label
                ));
            }
            changed = true;
        }
        Ok(changed.then_some(proposed))
    }

    pub fn apply_canvas_shape(&self, settings: &CanvasShapeValues) -> Result<(), String> {
        let shape = self
            .canvas_shape
            .as_ref()
            .ok_or("The Canvas shape is unavailable.")?;
        shape.editor.edit(&shape.loaded, settings).map_err(failure)
    }
    pub fn select_canvas_area(
        &mut self,
        editor: CanvasArea,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), String> {
        use CanvasAreaField::*;
        let loaded = editor.settings().map_err(failure)?;
        self.drafts
            .retain(|draft| draft.canvas_area_field().is_none());
        for (field, id, label) in [
            (OffsetX, "offset-x", "Shape offset X"),
            (OffsetY, "offset-y", "Shape offset Y"),
            (Thickness, "thickness", "Shape thickness"),
            (HardFeather, "hard-feather", "Hard feather"),
            (SoftFeather, "soft-feather", "Soft feather"),
            (OutlineThickness, "outline-thickness", "Outline thickness"),
        ] {
            self.drafts.push(Draft::new(
                format!("graph-canvas-area-{id}"),
                label.into(),
                Binding::CanvasArea(editor.clone(), field),
                loaded.number(field).to_string(),
                window,
                cx,
            ));
        }
        self.canvas_area = Some(CanvasAreaDraft {
            editor,
            outline: loaded.outline(),
            loaded,
        });
        Ok(())
    }

    pub fn stage_canvas_area(&self, cx: &App) -> Result<Option<CanvasAreaSettings>, String> {
        let Some(area) = &self.canvas_area else {
            return Ok(None);
        };
        let mut proposed = area.loaded;
        proposed.set_outline(area.outline);
        let mut changed = area.outline != area.loaded.outline();
        for draft in &self.drafts {
            let Some(field) = draft.canvas_area_field().filter(|_| draft.is_changed(cx)) else {
                continue;
            };
            let value = draft
                .input
                .read(cx)
                .value()
                .parse::<f64>()
                .map_err(|_| format!("{} requires a number.", draft.label))?;
            if !proposed.set_number(field, value) {
                let [min, max] = field.bounds();
                return Err(format!(
                    "Enter {} from {min} to {max}.",
                    draft.label.to_lowercase()
                ));
            }
            changed = true;
        }
        Ok(changed.then_some(proposed))
    }

    pub fn apply_canvas_area(&self, settings: CanvasAreaSettings) -> Result<(), String> {
        let area = self
            .canvas_area
            .as_ref()
            .ok_or("The Canvas layer is unavailable.")?;
        area.editor.edit(area.loaded, settings).map_err(failure)
    }

    pub fn select_environment_parameter(
        &mut self,
        index: usize,
        layer: usize,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), String> {
        self.environment = Some(
            self.environment_panels
                .get(index)
                .ok_or("The Environment variable is unavailable.")?
                .clone(),
        );
        self.select_environment_layer(layer, window, cx)
    }

    pub fn select_environment_layer(
        &mut self,
        selected: usize,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), String> {
        let Some(panel) = self.environment.as_mut() else {
            return Ok(());
        };
        panel.select(selected);
        let panel = panel.clone();
        self.clear_environment_fields();
        if !panel.is_editable() {
            return Ok(());
        }
        let mut fields = vec![EnvironmentField::Exposure, EnvironmentField::Gamma];
        if let Some(index) = panel.selected() {
            match panel.value().layer_kind(index) {
                Some(EnvironmentLayerKind::Image) => {
                    fields.extend([
                        EnvironmentField::ImageExposure(index),
                        EnvironmentField::ImageGamma(index),
                    ]);
                }
                Some(EnvironmentLayerKind::Light(light)) => {
                    fields.extend([
                        EnvironmentField::Rotation(index),
                        EnvironmentField::Elevation(index),
                    ]);
                    if matches!(
                        light,
                        EnvironmentLight::ClearSky | EnvironmentLight::CloudySky
                    ) {
                        fields.extend([
                            EnvironmentField::SkyHaze(index),
                            EnvironmentField::SkyDepth(index),
                        ]);
                    }
                }
                Some(EnvironmentLayerKind::Color)
                    if panel
                        .value()
                        .color_adjuster_key(EnvironmentColorField::LayerColor(index))
                        .is_none() =>
                {
                    fields.extend([
                        EnvironmentField::Red(index),
                        EnvironmentField::Green(index),
                        EnvironmentField::Blue(index),
                    ]);
                }
                _ => {}
            }
            fields.extend([
                EnvironmentField::Contrast(index),
                EnvironmentField::Hue(index),
                EnvironmentField::Saturation(index),
                EnvironmentField::Strength(index),
                EnvironmentField::BlendGamma(index),
            ]);
            if panel
                .value()
                .color_adjuster_key(EnvironmentColorField::Colorize(index))
                .is_none()
            {
                fields.extend((0..3).map(|channel| EnvironmentField::Tint(index, channel)));
            }
            if panel
                .value()
                .mask_hemisphere(index)
                .is_some_and(|mask| mask != EnvironmentMaskHemisphere::Off)
            {
                fields.push(EnvironmentField::MaskBlur(index));
                fields.extend((0..3).map(|channel| EnvironmentField::MaskTint(index, channel)));
                if panel.value().mask_fill(index) == Some(EnvironmentMaskFill::SolidColor) {
                    fields
                        .extend((0..3).map(|channel| EnvironmentField::MaskColor(index, channel)));
                }
            }
            if panel.value().mixer_enabled(index) {
                fields.extend([
                    EnvironmentField::MixerAmount(index),
                    EnvironmentField::MixerContrast(index),
                    EnvironmentField::MixerLevel(index),
                    EnvironmentField::MixerRotation(index),
                    EnvironmentField::MixerElevation(index),
                ]);
            }
        }
        for layer in std::iter::once(None).chain(panel.selected().map(Some)) {
            fields.extend(
                [
                    EnvironmentField::Heading(layer),
                    EnvironmentField::Pitch(layer),
                    EnvironmentField::Roll(layer),
                    EnvironmentField::FilterRadius(layer),
                    EnvironmentField::FilterStrength(layer),
                    EnvironmentField::FilterHorizontal(layer),
                    EnvironmentField::FilterVertical(layer),
                ]
                .into_iter()
                .filter(|field| panel.value().field(*field).is_some()),
            );
        }
        if let Some(index) = panel.selected() {
            for (kind, gradient) in [
                (
                    EnvironmentGradientKind::Recolor,
                    if panel
                        .value()
                        .color_adjuster_key(EnvironmentColorField::Recolor(index))
                        .is_some()
                    {
                        None
                    } else {
                        panel
                            .value()
                            .copy_recolor_gradient(index)
                            .map_err(failure)?
                    },
                ),
                (
                    EnvironmentGradientKind::Mask,
                    panel.value().copy_mask_gradient(index),
                ),
            ] {
                if let Some(gradient) = gradient {
                    self.add_gradient(
                        GradientEditor::new_environment(
                            panel.clone(),
                            index,
                            kind,
                            gradient,
                            window,
                            cx,
                        )?,
                        window,
                        cx,
                    )?;
                }
            }
        }
        for field in fields {
            self.add(
                format!("graph-environment:{}", field.label()),
                field.label().into(),
                Binding::Environment(panel.clone(), field),
                window,
                cx,
            )?;
        }
        Ok(())
    }

    pub fn clear_environment_fields(&mut self) {
        self.drafts.retain(|draft| !draft.is_environment());
        self.gradient = None;
        self.mask_gradient = None;
    }

    pub fn environment_fields_changed(&self, cx: &App) -> bool {
        self.gradient_position_changed(cx)
            || self
                .drafts
                .iter()
                .any(|draft| draft.is_environment() && draft.is_changed(cx))
    }

    pub fn stage_environment(&self, cx: &App) -> Result<Option<EnvironmentRecord>, String> {
        let Some(panel) = &self.environment else {
            return Ok(None);
        };
        if !self
            .drafts
            .iter()
            .any(|draft| draft.is_environment() && draft.is_changed(cx))
        {
            return Ok(None);
        }
        panel.validate()?;
        let mut proposed = panel.value().as_ref().clone();
        let mut gradients = [None, None];
        let mut changed = false;
        for draft in &self.drafts {
            changed |= draft.stage_environment(&mut proposed, &mut gradients, cx)?;
        }
        for (kind, gradient) in [
            EnvironmentGradientKind::Recolor,
            EnvironmentGradientKind::Mask,
        ]
        .into_iter()
        .zip(gradients)
        {
            if let Some(gradient) = gradient {
                let index = panel
                    .selected()
                    .ok_or("The gradient layer is unavailable.")?;
                gradient.rebuild().map_err(failure)?;
                kind.store(&mut proposed, index, &gradient)
                    .map_err(failure)?;
            }
        }
        Ok(changed.then_some(proposed))
    }

    fn add(
        &mut self,
        id: String,
        label: String,
        binding: Binding,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), String> {
        let loaded = binding.text()?;
        self.drafts
            .push(Draft::new(id, label, binding, loaded, window, cx));
        Ok(())
    }
    pub fn close_layer(&mut self) {
        self.layer = None;
        self.custom = None;
        self.drafts
            .retain(|draft| !matches!(draft.binding, Binding::LayerScalar(..)));
    }
    pub fn select_layer(
        &mut self,
        selection: LayerSelection,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), String> {
        selection.validate(None)?;
        self.drafts
            .retain(|draft| !matches!(draft.binding, Binding::LayerScalar(..)));
        for control in selection.controls() {
            if !matches!(
                control.kind(),
                CurveControlKind::Scalar | CurveControlKind::Modulated
            ) {
                continue;
            }
            let name = control
                .name()
                .as_deref()
                .map(String::from_utf16_lossy)
                .unwrap_or_else(|| format!("Control {}", control.id()));
            self.add(
                format!("graph-layer-control-{}", control.id()),
                format!("{:?} · {name}", selection.layer.kind()),
                Binding::LayerScalar(selection.clone(), control),
                window,
                cx,
            )?;
        }
        self.custom = CustomDraft::new(&selection.layer, window, cx)?;
        self.layer = Some(selection);
        Ok(())
    }
    fn add_animation(
        &mut self,
        id: String,
        label: String,
        parameter: AnimationParameter,
        axis: usize,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), String> {
        let animation = parameter.retained()?;
        let (enabled, curve) = parameter.axis(axis)?;
        if enabled {
            if let Some((animation, curve)) = animation.as_ref().zip(curve.as_ref()) {
                for (side, suffix) in ["Start", "End"].into_iter().enumerate() {
                    let output = if parameter.has_animation_palette() {
                        "palette position (%)"
                    } else {
                        "output"
                    };
                    self.add(
                        format!("{id}:{side}"),
                        format!("{label} {suffix} {output}"),
                        Binding::AnimationBound(
                            parameter.clone(),
                            animation.clone(),
                            curve.clone(),
                            axis,
                            side,
                        ),
                        window,
                        cx,
                    )?;
                }
            } else {
                self.notes.push(format!(
                    "{label}: the active animation curve is unavailable."
                ));
            }
        }
        self.animations.push(AnimationToggle {
            id,
            label,
            parameter,
            axis,
            enabled,
            animation,
            curve,
        });
        Ok(())
    }

    fn add_gradient(
        &mut self,
        editor: GradientEditor,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), String> {
        for (index, entry) in editor.entries.iter().enumerate() {
            let (kind, labels): (&str, &[&str]) = match entry {
                mycela_engine::GradientEntry::Color(_) => {
                    ("color", &["Position", "Red", "Green", "Blue"])
                }
                mycela_engine::GradientEntry::Alpha(_) => ("opacity", &["Position", "Opacity"]),
                _ => return Err("The gradient contains an unsupported stop.".into()),
            };
            for (component, label) in labels.iter().enumerate() {
                self.add(
                    editor.control_id(&format!("{kind}-{index}-{component}")),
                    format!(
                        "{}{kind} stop {} · {label}",
                        if editor.is_mask() { "Mask " } else { "" },
                        index + 1
                    ),
                    Binding::GradientStop(editor.clone(), entry.clone(), component),
                    window,
                    cx,
                )?;
            }
        }
        if editor.is_mask() {
            self.mask_gradient = Some(editor);
        } else {
            self.gradient = Some(editor);
        }
        Ok(())
    }
    pub fn fields_changed(&self, cx: &App) -> bool {
        self.drafts.iter().any(|d| !d.is_live() && d.is_changed(cx))
            || self
                .canvas_area
                .as_ref()
                .is_some_and(|area| area.outline != area.loaded.outline())
    }
    pub(crate) fn gradient_position_changed(&self, cx: &App) -> bool {
        self.gradient
            .iter()
            .chain(&self.mask_gradient)
            .any(|editor| editor.is_changed(cx))
    }
    pub fn is_changed(&self, cx: &App) -> bool {
        self.fields_changed(cx)
            || self.gradient_position_changed(cx)
            || self
                .legacy_gradient
                .as_ref()
                .is_some_and(|editor| editor.is_changed(cx))
            || self
                .custom
                .as_ref()
                .is_some_and(|draft| draft.is_changed(cx))
    }
    pub(crate) fn sync_unchanged(
        &mut self,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), String> {
        for draft in &mut self.drafts {
            if !draft.is_changed(cx) {
                let value = draft.binding.text()?;
                if value != draft.loaded {
                    draft
                        .input
                        .update(cx, |input, cx| input.set_value(value.clone(), window, cx));
                    draft.loaded = value;
                }
            }
        }
        Ok(())
    }
    pub fn load(node: &Node, key: &str, window: &mut Window, cx: &mut App) -> Result<Self, String> {
        Self::load_with_environment_adjusters(node, key, false, |_| None, window, cx)
    }

    pub(super) fn load_with_environment_adjusters(
        node: &Node,
        key: &str,
        lab_controls_only: bool,
        environment_adjuster: impl Fn(
            &PointwiseParameter<ForegroundError>,
        ) -> Option<mycela_engine::EnvironmentAdjuster>,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Self, String> {
        let mut view = Self {
            owner: Some(node.clone()),
            ..Self::default()
        };
        let mut ancestor = node.parent();
        let mut seen = Vec::<Node>::new();
        let mut has_variable_owner = false;
        while let Some(parent) = ancestor {
            if seen.iter().any(|old| old.same_reference(&parent)) {
                break;
            }
            seen.push(parent.clone());
            if matches!(
                parent.kind(),
                Some(
                    mycela_engine::PointwiseKind::MapVariable
                        | mycela_engine::PointwiseKind::WizardGroup
                )
            ) {
                has_variable_owner = true;
                break;
            }
            ancestor = parent.parent();
        }
        if let Some(filter) = node.filter() {
            if filter.kind() == mycela_engine::PointwiseKind::Canvas {
                view.canvas_areas = filter.canvas_area_editors();
            }
            if filter.kind() == mycela_engine::PointwiseKind::OklabColor {
                view.notes
                    .push(crate::filter_stack::OKLAB_INPUT_HELP.into());
            }
            if filter.kind() == mycela_engine::PointwiseKind::Checkerboard {
                view.notes.push(
                    "X and Y set the cell proportions. Scale repeats whole checker pairs to keep the texture seamless.".into(),
                );
            }
            let controls = if matches!(
                filter.kind(),
                mycela_engine::PointwiseKind::WizardGroup
                    | mycela_engine::PointwiseKind::WizardReference
            ) {
                match filter.wizard_dynamic_properties() {
                    Ok(controls) => controls,
                    Err(error) => {
                        view.notes.push(failure(error));
                        Vec::new()
                    }
                }
            } else {
                Vec::new()
            };
            let mut parameters = filter.parameters();
            if matches!(filter.kind(), mycela_engine::PointwiseKind::TemplateLab(_)) {
                parameters.drain(..3);
            }
            if filter.kind() == mycela_engine::PointwiseKind::WizardReference
                || lab_controls_only
                    && !matches!(filter.kind(), mycela_engine::PointwiseKind::TemplateLab(_))
            {
                parameters.clear();
            }
            for p in controls.iter().chain(&parameters).cloned() {
                let name = String::from_utf16_lossy(&p.name());
                let variable = p.wizard_variable_index();
                let id = match variable {
                    Some(index) => format!("{key}:wizard-variable:{index}"),
                    None => format!("{key}:{name}"),
                };
                if !lab_controls_only
                    && (has_variable_owner || p.binding_index() >= 0)
                    && variable.is_none()
                    && p.is_adjustable()
                    && matches!(
                        p.value(),
                        PointwiseParameterValue::Number { .. }
                            | PointwiseParameterValue::Pair { .. }
                            | PointwiseParameterValue::Choice { .. }
                            | PointwiseParameterValue::Color(_)
                            | PointwiseParameterValue::Environment(_)
                    )
                {
                    view.add(
                        format!("{id}:binding-index"),
                        format!("{name} variable index"),
                        Binding::VariableIndex(p.clone()),
                        window,
                        cx,
                    )?;
                    if !matches!(p.value(), PointwiseParameterValue::Environment(_)) {
                        view.add(
                            format!("{id}:binding-map"),
                            format!("{name} mapping"),
                            Binding::VariableMapping(p.clone()),
                            window,
                            cx,
                        )?;
                    }
                }
                match p.value() {
                    PointwiseParameterValue::Color(_) => {
                        if filter.kind() == mycela_engine::PointwiseKind::Selector {
                            view.selector_color =
                                Some(SelectorColorEditor::new(p.clone(), window, cx));
                        }
                        for (axis, label) in ["Red", "Green", "Blue"].into_iter().enumerate() {
                            view.add(
                                format!("{id}:{axis}"),
                                format!("{name} {label}"),
                                Binding::FilterColor(p.clone(), axis),
                                window,
                                cx,
                            )?;
                        }
                    }
                    PointwiseParameterValue::Number { .. } => {
                        view.add(id, name, Binding::Number(p), window, cx)?
                    }
                    PointwiseParameterValue::Pair { .. } => {
                        for axis in 0..2 {
                            view.add(
                                format!("{id}:{axis}"),
                                format!("{name} {}", if axis == 0 { "X" } else { "Y" }),
                                Binding::Axis(p.clone(), axis),
                                window,
                                cx,
                            )?;
                        }
                    }
                    PointwiseParameterValue::Choice { .. } => view.choices.push(ChoiceField {
                        id,
                        label: name,
                        binding: Choice::Filter(p),
                    }),
                    PointwiseParameterValue::Region { .. } => {
                        let region = p.region().map_err(failure)?;
                        for (axis, label) in ["X", "Y", "height"].into_iter().enumerate() {
                            view.add(
                                format!("{id}:{axis}"),
                                format!("{name} {label}"),
                                Binding::RegionCoordinate(p.clone(), region.clone(), axis),
                                window,
                                cx,
                            )?;
                        }
                        view.choices.push(ChoiceField {
                            id,
                            label: format!("{name} shape"),
                            binding: Choice::Region(p),
                        });
                    }
                    PointwiseParameterValue::Text(_) => {
                        view.add(id, name, Binding::Text(p), window, cx)?
                    }
                    PointwiseParameterValue::Matrix { .. } => {
                        if filter.kind() == mycela_engine::PointwiseKind::ColorizeG2 {
                            view.legacy_gradient = Some(LegacyGradientEditor::new(p, window, cx)?);
                            continue;
                        }
                        match p.custom_object().map_err(failure)? {
                            Some(object) if object.is_light_position() => {
                                for (axis, label) in ["X", "Y"].into_iter().enumerate() {
                                    view.add(
                                        format!("{id}:{axis}"),
                                        format!("{name} {label}"),
                                        Binding::LightPosition(p.clone(), object.clone(), axis),
                                        window,
                                        cx,
                                    )?;
                                }
                            }
                            Some(object) => {
                                let matrix = object.matrix().map_err(failure)?;
                                view.add(
                                    format!("{id}:angle"),
                                    format!("{name} angle"),
                                    Binding::Matrix(matrix.clone(), false),
                                    window,
                                    cx,
                                )?;
                                view.add(
                                    format!("{id}:radius"),
                                    format!("{name} radius"),
                                    Binding::Matrix(matrix, true),
                                    window,
                                    cx,
                                )?;
                            }
                            None => view.notes.push(format!(
                                "{name}: retained custom payload; advanced editing is unavailable."
                            )),
                        }
                    }
                    PointwiseParameterValue::Histogram(_) => {
                        view.levels = Some(LevelsEditor::create(p.clone(), window, cx)?);
                    }
                    PointwiseParameterValue::Curve(_) => {
                        p.curve().map_err(failure)?;
                        view.add_animation(
                            format!("{id}:curve"),
                            name,
                            AnimationParameter::DirectCurve(p),
                            0,
                            window,
                            cx,
                        )?;
                    }
                    PointwiseParameterValue::Gradient(_) => {
                        let gradient = p.gradient().map_err(failure)?;
                        let editor = GradientEditor::new(p, gradient, window, cx)?;
                        view.add_gradient(editor, window, cx)?;
                    }
                    PointwiseParameterValue::Environment(_) => {
                        let adjuster = environment_adjuster(&p);
                        view.environment_panels
                            .push(EnvironmentPanel::new(p, adjuster)?);
                    }
                    PointwiseParameterValue::Variable(_) => {}
                    _ => view.notes.push(format!(
                        "{name}: this retained parameter has no editor yet."
                    )),
                }
            }
            if !view.environment_panels.is_empty() {
                view.select_environment_parameter(0, 0, window, cx)?;
            }
            for (index, parameter) in parameters.into_iter().chain(controls).enumerate() {
                if !parameter.is_animatable() {
                    continue;
                }
                let animation_parameter = AnimationParameter::Filter(parameter.clone());
                let axes = match parameter.value() {
                    PointwiseParameterValue::Pair { .. } => 2,
                    PointwiseParameterValue::Number { .. }
                    | PointwiseParameterValue::Choice { .. } => 1,
                    PointwiseParameterValue::Color(_)
                        if animation_parameter.has_animation_palette() =>
                    {
                        1
                    }
                    _ => continue,
                };
                let variable = parameter.wizard_variable_index();
                if variable.is_some() {
                    // GTX serialization also materializes this default owner.
                    // Capture it now so saving cannot retire a toggle's snapshot.
                    parameter.animation().map_err(failure)?;
                }
                let name = String::from_utf16_lossy(&parameter.name());
                for axis in 0..axes {
                    let label = if axes == 2 {
                        format!("{name} {}", if axis == 0 { "X" } else { "Y" })
                    } else {
                        name.clone()
                    };
                    let id = match variable {
                        Some(index) => format!("{key}:wizard-variable:{index}:animation:{axis}"),
                        None => format!("{key}:animation:{index}:{axis}"),
                    };
                    view.add_animation(id, label, animation_parameter.clone(), axis, window, cx)?;
                }
                if animation_parameter.has_animation_palette()
                    && let Some(item) = view.animations.last()
                    && item.enabled
                    && let Some(AnimationOwner::Legacy(animation)) = &item.animation
                {
                    let editor = GradientEditor::new_animation_palette(
                        animation_parameter,
                        animation.clone(),
                        window,
                        cx,
                    )?;
                    view.add_gradient(editor, window, cx)?;
                }
            }
        } else if let Some(generator) = node.image_generator_handle() {
            for p in generator.parameters() {
                let name = p.name().to_string();
                let id = format!("{key}:{name}");
                if (has_variable_owner || p.binding_index() >= 0)
                    && p.is_adjustable()
                    && (p.is_paired() || p.is_choice() || p.is_scalar())
                {
                    view.add(
                        format!("{id}:binding-index"),
                        format!("{name} variable index"),
                        Binding::GeneratorVariableIndex(p.clone()),
                        window,
                        cx,
                    )?;
                    view.add(
                        format!("{id}:binding-map"),
                        format!("{name} mapping"),
                        Binding::GeneratorVariableMapping(p.clone()),
                        window,
                        cx,
                    )?;
                }
                if p.is_paired() {
                    for axis in 0..2 {
                        view.add(
                            format!("{id}:{axis}"),
                            format!("{name} {}", if axis == 0 { "X" } else { "Y" }),
                            Binding::GeneratorAxis(p.clone(), axis),
                            window,
                            cx,
                        )?;
                    }
                } else if p.is_choice() {
                    view.choices.push(ChoiceField {
                        id,
                        label: name,
                        binding: Choice::Generator(p),
                    });
                } else if p.is_scalar() {
                    view.add(id, name, Binding::Generator(p), window, cx)?;
                } else {
                    view.notes.push(format!(
                        "{name}: retained payload is preserved; replacement is unavailable here."
                    ));
                }
            }
            for (index, parameter) in generator.parameters().into_iter().enumerate() {
                if !parameter.is_animatable()
                    || !(parameter.is_paired() || parameter.is_scalar() || parameter.is_choice())
                {
                    continue;
                }
                let axes = if parameter.is_paired() { 2 } else { 1 };
                for axis in 0..axes {
                    let name = parameter.name();
                    let label = if axes == 2 {
                        format!("{name} {}", if axis == 0 { "X" } else { "Y" })
                    } else {
                        name.to_string()
                    };
                    view.add_animation(
                        format!("{key}:generator-animation:{index}:{axis}"),
                        label,
                        AnimationParameter::Generator(parameter.clone()),
                        axis,
                        window,
                        cx,
                    )?;
                }
            }
        } else if let Some(properties) = node
            .scene_transform_3d()
            .map(|transform| transform.properties())
            .or_else(|| {
                node.scene_primitive()
                    .map(|primitive| primitive.properties())
            })
            .or_else(|| node.uv_texture_properties())
            .or_else(|| node.import_3d_properties())
        {
            for (index, property) in properties.iter().cloned().enumerate() {
                let name = property
                    .name()
                    .as_deref()
                    .map(String::from_utf16_lossy)
                    .unwrap_or_else(|| "Property".into());
                let id = format!("{key}:modern:{index}");
                match property.value() {
                    ModernNodeValue::Vector3D(_) => {
                        let labels = property.component_labels();
                        for axis in 0..3 {
                            let suffix = labels
                                .as_ref()
                                .map(|v| String::from_utf16_lossy(&v[axis]))
                                .unwrap_or_else(|| ["X", "Y", "Z"][axis].into());
                            view.add(
                                format!("{id}:{axis}"),
                                format!("{name} {suffix}"),
                                Binding::ModernAxis(property.clone(), axis),
                                window,
                                cx,
                            )?;
                        }
                    }
                    ModernNodeValue::Int32(_) if property.selected_options().is_some() => {
                        view.choices.push(ChoiceField {
                            id,
                            label: name,
                            binding: Choice::Modern(property),
                        })
                    }
                    ModernNodeValue::Boolean(_) => view.choices.push(ChoiceField {
                        id,
                        label: name,
                        binding: Choice::ModernBoolean(property),
                    }),
                    ModernNodeValue::Int32(_) | ModernNodeValue::Double(_) => {
                        view.add(id, name, Binding::Modern(property), window, cx)?
                    }
                }
            }
            for (index, property) in properties.into_iter().enumerate() {
                if !property.backing_snapshot().animatable {
                    continue;
                }
                let axes = match property.value() {
                    ModernNodeValue::Vector3D(_) => 3,
                    ModernNodeValue::Int32(_) | ModernNodeValue::Double(_) => 1,
                    _ => continue,
                };
                let name = property
                    .name()
                    .as_deref()
                    .map(String::from_utf16_lossy)
                    .unwrap_or_else(|| "Property".into());
                let labels = property.component_labels();
                for axis in 0..axes {
                    let label = if axes == 3 {
                        let suffix = labels
                            .as_ref()
                            .map(|v| String::from_utf16_lossy(&v[axis]))
                            .unwrap_or_else(|| ["X", "Y", "Z"][axis].into());
                        format!("{name} {suffix}")
                    } else {
                        name.clone()
                    };
                    view.add_animation(
                        format!("{key}:modern-animation:{index}:{axis}"),
                        label,
                        AnimationParameter::Modern(property.clone()),
                        axis,
                        window,
                        cx,
                    )?;
                }
            }
            view.notes
                .push("Adjustment data and advanced curve layers are preserved.".into());
        } else if node.scene_clear_lights().is_some() {
            if node.slot(0).map_err(failure)?.node().is_none() {
                view.notes
                    .push("Connect a 3D model to the Model input.".into());
            }
        } else if node.scene_combine_3d().is_some() {
            if node.slot(0).map_err(failure)?.node().is_none()
                && node.slot(1).map_err(failure)?.node().is_none()
            {
                view.notes
                    .push("Connect a 3D model to Model A or Model B.".into());
            }
        } else if node.solid_color_values().is_ok() {
            for (channel, label) in ["Red", "Green", "Blue"].into_iter().enumerate() {
                view.add(
                    format!("{key}:color:{channel}"),
                    label.into(),
                    Binding::Color(node.clone(), channel),
                    window,
                    cx,
                )?;
            }
            view.add(
                format!("{key}:transparency"),
                "Opacity".into(),
                Binding::Transparency(node.clone()),
                window,
                cx,
            )?;
            for (property, name) in [
                (SolidColorProperty::Color, "Color"),
                (SolidColorProperty::Transparency, "Transparency"),
            ] {
                if has_variable_owner || node.solid_binding_index(property).map_err(failure)? >= 0 {
                    view.add(
                        format!("{key}:{name}:binding-index"),
                        format!("{name} variable index"),
                        Binding::SolidVariableIndex(node.clone(), property),
                        window,
                        cx,
                    )?;
                    view.add(
                        format!("{key}:{name}:binding-map"),
                        format!("{name} mapping"),
                        Binding::SolidVariableMapping(node.clone(), property),
                        window,
                        cx,
                    )?;
                }
                let parameter = AnimationParameter::Solid(node.clone(), property);
                view.add_animation(
                    format!("{key}:solid-animation:{}", name.to_ascii_lowercase()),
                    name.into(),
                    parameter.clone(),
                    0,
                    window,
                    cx,
                )?;
                if parameter.is_solid_color_animation()
                    && let Some(item) = view.animations.last()
                    && item.enabled
                    && let Some(AnimationOwner::Legacy(animation)) = &item.animation
                {
                    let editor = GradientEditor::new_animation_palette(
                        parameter,
                        animation.clone(),
                        window,
                        cx,
                    )?;
                    view.add_gradient(editor, window, cx)?;
                }
            }
        } else if let Ok(properties) = node.invert_properties() {
            for p in properties {
                view.choices.push(ChoiceField {
                    id: format!("{key}:invert:{}", p.channel()),
                    label: String::from_utf16_lossy(p.snapshot().map_err(failure)?.name()),
                    binding: Choice::Invert(p),
                });
            }
        }
        Ok(view)
    }
}
