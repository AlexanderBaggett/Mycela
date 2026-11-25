use super::{PointwiseError, SharedImageMap, guard};
use crate::{
    AnalyticCurveKind, CurveLayerHandle, CurveRangeRecord, DecimalBits, FloatBuffer,
    HistoryDirection, HistoryError, ImageError, LimitsDefinition, LimitsStep, RangeOptionsRecord,
    SharedCurve, SharedCurveRangeRecord, SharedGradient, SharedLimits, SharedRangeOptionsRecord,
    SharedSelectionOptions, history::HistoryGraphEdit,
};
#[path = "color_parameter.rs"]
mod color_parameter;
#[path = "levels_editor.rs"]
pub(super) mod levels_editor;
#[path = "nonlinear_parameters.rs"]
mod nonlinear_parameters;
#[path = "point_light_parameters.rs"]
mod point_light;
#[path = "property_wire.rs"]
pub(crate) mod wire;
use std::{
    fmt,
    sync::{Arc, Mutex},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PointwiseKind {
    ChangeBci,
    ChangeHsb,
    Colorize,
    ColorizeG2,
    Levels,
    GetChannel,
    Add,
    Multiply,
    FromHsb,
    OklabColor,
    Shader,
    ClearMaps,
    StoreMap,
    MergeMaps,
    FixMapContrast,
    LayerMaps,
    ToneCurve,
    ToneCurveG2,
    Combine,
    Pixelate,
    Matrix,
    WizardMatrix,
    WizardColor,
    WizardFiveColor,
    WizardSlideColor,
    WizardKaleidoscope,
    WizardToneCurve,
    Rotate,
    GaussianBlur,
    Sharpen,
    MedianBlur,
    MotionBlur,
    ShapeBlur,
    SmartBlur,
    Smear,
    VariableBlur,
    Equalize,
    PointLight,
    IlluminateG2,
    Shadow,
    ShadowG2,
    Polygon,
    Atmosphere,
    ConvertEnvironment,
    MakeEnvironment,
    RotateEnvironment,
    DiffuseEnvironment,
    ScatterEnvironment,
    ReflectEnvironment,
    RefractEnvironment,
    Reflect,
    Refract,
    Stereogram,
    Regions,
    Caustics,
    Transmit,
    FullLight,
    Frequency,
    Fractalize,
    Exposure,
    Selector,
    MatchColor,
    ArtBlur,
    Distort,
    FreeTransform,
    Kaleidoscope,
    MoveRepeat,
    Remap,
    RotateTile,
    WizardGroup,
    Group,
    Reference,
    MapVariable,
    TemplateLab(super::TemplateLabKind),
    WizardReference,
    Hair,
    LegacyBricksBoards,
    Canvas,
    Style,
    Synthesis,
    Checkerboard,
}
impl PointwiseKind {
    pub(super) fn has_compact_inputs(self) -> bool {
        self.is_group() || self == Self::Canvas
    }

    pub(super) fn is_group(self) -> bool {
        matches!(
            self,
            Self::Group
                | Self::WizardGroup
                | Self::MapVariable
                | Self::TemplateLab(_)
                | Self::WizardReference
        )
    }

    pub fn input_count(self) -> usize {
        match self {
            Self::WizardGroup
            | Self::Group
            | Self::MapVariable
            | Self::TemplateLab(_)
            | Self::Canvas
            | Self::WizardReference => 20,
            Self::Polygon | Self::Reference | Self::Shader | Self::Checkerboard => 0,
            Self::FullLight | Self::OklabColor => 4,
            Self::Add
            | Self::Multiply
            | Self::ReflectEnvironment
            | Self::RefractEnvironment
            | Self::Reflect
            | Self::Refract
            | Self::Stereogram
            | Self::Regions
            | Self::Transmit
            | Self::MedianBlur
            | Self::SmartBlur
            | Self::VariableBlur
            | Self::MatchColor => 2,
            Self::FromHsb
            | Self::Combine
            | Self::MotionBlur
            | Self::ShapeBlur
            | Self::Smear
            | Self::ArtBlur
            | Self::Distort
            | Self::Remap
            | Self::Hair
            | Self::Style => 3,
            _ => 1,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::WizardGroup => "Wizard Group",
            Self::Group => "Group",
            Self::Reference => "Reference",
            Self::MapVariable => "Map Variable",
            Self::TemplateLab(kind) => kind.template().name,
            Self::WizardReference => "Wizard Reference",
            Self::ChangeBci => "Change BCI",
            Self::ChangeHsb => "Change HSB",
            Self::Colorize => "Colorize",
            Self::ColorizeG2 => "Colorize G2",
            Self::Levels => "Levels",
            Self::Equalize => "Equalize",
            Self::PointLight => "Point Light",
            Self::IlluminateG2 => "Illuminate G2",
            Self::Shadow => "Shadow",
            Self::ShadowG2 => "Shadow G2",
            Self::Polygon => "Polygon",
            Self::Atmosphere => "Atmosphere",
            Self::Hair => "Hair",
            Self::LegacyBricksBoards => "Bricks 'n Boards",
            Self::Canvas => "Canvas",
            Self::Style => "Style",
            Self::Synthesis => "Synthesis",
            Self::ConvertEnvironment => "Convert Environment",
            Self::MakeEnvironment => "Make Environment",
            Self::RotateEnvironment => "Rotate Environment",
            Self::DiffuseEnvironment => "Diffuse Environment",
            Self::ScatterEnvironment => "Scatter Environment",
            Self::ReflectEnvironment => "Reflect Environment",
            Self::RefractEnvironment => "Refract Environment",
            Self::Reflect => "Reflect",
            Self::Refract => "Refract",
            Self::Stereogram => "Stereogram",
            Self::Regions => "Regions",
            Self::Caustics => "Caustics",
            Self::Transmit => "Transmit",
            Self::FullLight => "Full Light",
            Self::Frequency => "Frequency",
            Self::Fractalize => "Fractalize",
            Self::Exposure => "Exposure",
            Self::Selector => "Selector",
            Self::MatchColor => "Match Color",
            Self::ArtBlur => "Art Blur",
            Self::Distort => "Distort",
            Self::FreeTransform => "Free Transform",
            Self::Kaleidoscope => "Kaleidoscope",
            Self::MoveRepeat => "Move & Repeat",
            Self::Remap => "Remap",
            Self::RotateTile => "Rotate & Tile",

            Self::GetChannel => "Get Channel",
            Self::Add => "A+B",
            Self::Multiply => "A*B",
            Self::FromHsb => "From HSB",
            Self::OklabColor => "Oklab Color",
            Self::Checkerboard => "Checkerboard",
            Self::Shader => "Shader",
            Self::ClearMaps => "Clear Maps",
            Self::StoreMap => "Store Map",
            Self::MergeMaps => "Merge Maps",
            Self::FixMapContrast => "Fix Map Contrast",
            Self::LayerMaps => "Layer Maps",
            Self::ToneCurve => "Tone Curve",
            Self::ToneCurveG2 => "Tone Curve G2",
            Self::Combine => "Combine",
            Self::Pixelate => "Pixelate",
            Self::Matrix => "Matrix",
            Self::WizardMatrix => "Wizard Matrix",
            Self::WizardColor => "Wizard Color",
            Self::WizardFiveColor => "Wizard 5 Color",
            Self::WizardSlideColor => "Wizard Slide Color",
            Self::WizardKaleidoscope => "Wizard Kaleidoscope",
            Self::WizardToneCurve => "Wizard Tone Curve",
            Self::Rotate => "Rotate",
            Self::GaussianBlur => "Gaussian Blur",
            Self::Sharpen => "Sharpen",
            Self::MedianBlur => "Median Blur",
            Self::MotionBlur => "Motion Blur",
            Self::ShapeBlur => "Shape Blur",
            Self::SmartBlur => "Smart Blur",
            Self::Smear => "Smear Blur",
            Self::VariableBlur => "Variable Blur",
        }
    }
}
#[derive(Clone, Debug)]
pub struct Histogram(Arc<Mutex<HistogramState>>);
#[derive(Debug)]
struct HistogramState {
    arrays: [FloatBuffer; 3],
    selected: i32,
    disabled: bool,
}
impl Default for Histogram {
    fn default() -> Self {
        Self::new()
    }
}
impl Histogram {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(HistogramState {
            arrays: [
                FloatBuffer::new(vec![0.0; 4]),
                FloatBuffer::new(vec![0.5; 4]),
                FloatBuffer::new(vec![1.0; 4]),
            ],
            selected: 0,
            disabled: false,
        })))
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub fn selected_channel(&self) -> i32 {
        guard(&self.0).selected
    }
    /// Source validates the OLD index, allowing an invalid new index to stick.
    pub fn set_channel(&self, value: i32) {
        let mut s = guard(&self.0);
        if (0..4).contains(&s.selected) {
            s.selected = value;
        }
    }
    pub fn is_disabled(&self) -> bool {
        guard(&self.0).disabled
    }
    pub fn set_disabled(&self, value: bool) {
        guard(&self.0).disabled = value;
    }
    pub fn arrays(&self) -> [FloatBuffer; 3] {
        guard(&self.0).arrays.clone()
    }
    pub fn projection(&self, channel: i32) -> Result<Option<FloatBuffer>, ImageError> {
        if !(0..3).contains(&channel) {
            return Ok(None);
        }
        let arrays = self.arrays();
        let mut values = Vec::with_capacity(3);
        for a in arrays {
            values.push(a.read(channel as usize + 1)?);
        }
        Ok(Some(FloatBuffer::new(values)))
    }
    pub fn set_component(&self, component: usize, value: f32) -> Result<(), ImageError> {
        let (selected, disabled, arrays) = {
            let s = guard(&self.0);
            (s.selected, s.disabled, s.arrays.clone())
        };
        let a = arrays.get(component).ok_or(ImageError::Index)?;
        let at = usize::try_from(selected).map_err(|_| ImageError::Index)?;
        // The read/index error precedes the disabled short-circuit.
        if a.read(at)? != value && !disabled {
            if selected != 0 {
                a.write(at, value)?;
                a.write(0, (a.read(1)? + a.read(2)? + a.read(3)?) / 3.0)?;
            } else if a.read(0)? != value {
                for i in 0..4 {
                    a.write(i, value)?;
                }
            }
        }
        Ok(())
    }
    pub fn set_values(&self, values: Option<&FloatBuffer>) -> Result<(), ImageError> {
        if let Some(values) = values
            && values.len() == 3
        {
            for i in 0..3 {
                self.set_component(i, values.read(i)?)?;
            }
        }
        Ok(())
    }
    pub fn source_clone(&self) -> Self {
        let s = guard(&self.0);
        Self(Arc::new(Mutex::new(HistogramState {
            arrays: std::array::from_fn(|i| FloatBuffer::new(s.arrays[i].values())),
            selected: s.selected,
            disabled: false,
        })))
    }
}
#[derive(Debug)]
#[non_exhaustive]
pub enum PointwiseParameterValue<E = ()> {
    Number {
        current: f64,
        previous: f64,
        limits: SharedLimits,
    },
    Choice {
        current: i32,
        previous: i32,
        options: SharedSelectionOptions,
    },
    /// Stored o1.c UTF16, including null. No host string normalization.
    Text(Option<Vec<u16>>),
    /// Retained os variable specification used by Wizard Groups.
    Variable(crate::VariableSpec),
    Gradient(Option<SharedGradient>),
    Environment(Arc<crate::EnvironmentRecord>),
    /// Real o1.i curve authority; None is the source lazy state.
    Curve(Option<SharedCurve<E>>),
    /// Actual copied rr value, including its retained optional ColorInput reference.
    Color(crate::ContextColorValue<E>),
    Histogram(Histogram),
    Pair {
        current: [f64; 2],
        previous: [f64; 2],
        limits: SharedLimits,
    },
    Region {
        current: Option<PointwiseCustomObject<E>>,
        previous: Option<PointwiseCustomObject<E>>,
    },
    Matrix {
        current: Option<PointwiseCustomObject<E>>,
        previous: Option<PointwiseCustomObject<E>>,
    },
}
impl<E> Clone for PointwiseParameterValue<E> {
    fn clone(&self) -> Self {
        match self {
            Self::Number {
                current,
                previous,
                limits,
            } => Self::Number {
                current: *current,
                previous: *previous,
                limits: limits.clone(),
            },
            Self::Choice {
                current,
                previous,
                options,
            } => Self::Choice {
                current: *current,
                previous: *previous,
                options: options.clone(),
            },
            Self::Text(v) => Self::Text(v.clone()),
            Self::Variable(v) => Self::Variable(v.clone()),
            Self::Gradient(v) => Self::Gradient(v.clone()),
            Self::Environment(v) => Self::Environment(v.clone()),
            Self::Curve(v) => Self::Curve(v.clone()),
            Self::Color(v) => Self::Color(v.clone()),
            Self::Histogram(v) => Self::Histogram(v.clone()),
            Self::Pair {
                current,
                previous,
                limits,
            } => Self::Pair {
                current: *current,
                previous: *previous,
                limits: limits.clone(),
            },
            Self::Region { current, previous } => Self::Region {
                current: current.clone(),
                previous: previous.clone(),
            },
            Self::Matrix { current, previous } => Self::Matrix {
                current: current.clone(),
                previous: previous.clone(),
            },
        }
    }
}
#[derive(Clone, PartialEq)]
pub(crate) struct ChoiceMemory {
    // Source m_m selects ordinary Int32 versus ae7 undo production. The
    // m_aa/m_ab anchor fields also exist when this flag is false.
    enabled: bool,
    index: i32,
    text: Option<Vec<u16>>,
}
#[derive(Clone)]
struct ParameterState<E> {
    extras: ParameterExtras<E>,
    // os.aa/name-preserving choice state is distinct from current/previous.
    choice_memory: Option<ChoiceMemory>,
    name: Vec<u16>,
    value: PointwiseParameterValue<E>,
}

fn wizard_environment_parameter<E>(name: Vec<u16>) -> Result<ParameterState<E>, PointwiseError<E>> {
    let mut map = crate::EnvironmentRecord::default();
    map.add_layer(crate::EnvironmentLayerKind::Light(
        crate::EnvironmentLight::Soft,
    ))
    .map_err(|_| ImageError::Allocation)?;
    let extras = ParameterExtras::default();
    extras.wire.set_capabilities(false, true);
    Ok(ParameterState {
        extras,
        choice_memory: None,
        name,
        value: PointwiseParameterValue::Environment(Arc::new(map)),
    })
}
struct FilterState<E> {
    factor: f64,
    canvas: Option<Arc<super::canvas::CanvasDocument>>,
    style: Option<Arc<super::canvas::CanvasStyle>>,
    synthesis: Option<Arc<super::canvas::SynthesisTexture>>,
    synthesis_wire: Option<wire::LegacyWireMetadata<E>>,
    synthesis_catalog: Option<crate::SynthesisCatalogValue>,
    lab_inheritance: (f64, f64),
    wizard_template: Option<&'static str>,
    previous_templates: std::collections::BTreeMap<&'static str, super::region::NodeId>,
    clone_mode: PointwiseCloneMode,
    kind: PointwiseKind,
    parameters: Vec<ParameterState<E>>,
    wizard_dynamic: Option<Vec<usize>>,
    wizard_label: Option<Vec<u16>>,
    wizard_positions: Vec<[f32; 2]>,
    group_guid: Option<[u8; 16]>,
    group_presets: Option<Arc<Mutex<crate::GtxPresetCollection>>>,
    cache: Option<super::RenderedImage>,
    handlers: Vec<PointwiseCacheHandler<E>>,
}
pub(super) struct Cohort<E> {
    filters: Mutex<Vec<FilterState<E>>>,
}
/// One actual cache notification binding; Clone retains that binding identity.
pub struct PointwiseCacheHandler<E>(Arc<CacheCallback<E>>);
type CacheCallback<E> = dyn Fn(&PointwiseFilter<E>) -> Result<(), E> + Send + Sync;
impl<E> Clone for PointwiseCacheHandler<E> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}
impl<E> fmt::Debug for PointwiseCacheHandler<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PointwiseCacheHandler({:p})", Arc::as_ptr(&self.0))
    }
}
impl<E> PointwiseCacheHandler<E> {
    pub fn new(
        callback: impl Fn(&PointwiseFilter<E>) -> Result<(), E> + Send + Sync + 'static,
    ) -> Self {
        Self(Arc::new(callback))
    }
    pub fn same_binding(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
/// Supplied-map filter state and retained parameter roots. Internal parent edges
/// are cohort coordinates; outward property handles sponsor the same cohort.
/// This deliberately exposes no fake graph evaluator or upstream child cache.
pub struct PointwiseFilter<E = ()> {
    core: Arc<Cohort<E>>,
    ix: usize,
    pub(super) graph_id: super::region::NodeId,
    pub(super) sponsor: Arc<super::region::Sponsor<E>>,
}
impl<E> Clone for PointwiseFilter<E> {
    fn clone(&self) -> Self {
        Self {
            core: Arc::clone(&self.core),
            ix: self.ix,
            graph_id: self.graph_id,
            sponsor: Arc::clone(&self.sponsor),
        }
    }
}
impl<E> fmt::Debug for PointwiseFilter<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "PointwiseFilter({:p}/{})",
            Arc::as_ptr(&self.core),
            self.ix
        )
    }
}
pub struct PointwiseParameter<E = ()> {
    owner: PointwiseFilter<E>,
    ix: usize,
}
pub(crate) struct WeakPointwiseFilter<E>(std::sync::Weak<super::graph::NodeCore<E>>);

pub(crate) struct SynthesisLinkState<E> {
    pub(crate) adjustment: Option<super::SharedPropertyAdjustment<E>>,
    pub(crate) catalog: Option<crate::SynthesisCatalogValue>,
    pub(crate) local: crate::SynthesisRecord,
}

pub(crate) struct WeakPointwiseParameter<E> {
    owner: WeakPointwiseFilter<E>,
    index: usize,
}

pub(crate) enum NumericAdjustment {
    Number(f64),
    Pair([f64; 2]),
    Choice(i32),
}

#[derive(PartialEq)]
pub(crate) enum NumericPropertyState {
    Number {
        current: f64,
        previous: f64,
    },
    Pair {
        current: [f64; 2],
        previous: [f64; 2],
    },
    Choice {
        current: i32,
        previous: i32,
        memory: Option<ChoiceMemory>,
    },
}

impl<E: 'static> PointwiseParameter<E> {
    pub(crate) fn prepare_numeric_adjustment(
        &self,
        adjustment: NumericAdjustment,
    ) -> Result<(NumericPropertyState, NumericPropertyState), PointwiseError<E>> {
        use NumericPropertyState as State;
        let (raw, memory) = {
            let entries = guard(&self.owner.core.filters);
            let property = &entries[self.owner.ix].parameters[self.ix];
            (property.value.clone(), property.choice_memory.clone())
        };
        match (raw, adjustment) {
            (
                PointwiseParameterValue::Number {
                    current,
                    previous,
                    limits,
                },
                NumericAdjustment::Number(value),
            ) => {
                let value = normalize_parameter(&limits, value)?;
                Ok((
                    State::Number { current, previous },
                    State::Number {
                        current: value,
                        previous: value,
                    },
                ))
            }
            (
                PointwiseParameterValue::Pair {
                    current,
                    previous,
                    limits,
                },
                NumericAdjustment::Pair(value),
            ) => {
                let value = [
                    normalize_parameter(&limits, value[0])?,
                    normalize_parameter(&limits, value[1])?,
                ];
                Ok((
                    State::Pair { current, previous },
                    State::Pair {
                        current: value,
                        previous: value,
                    },
                ))
            }
            (
                PointwiseParameterValue::Choice {
                    current,
                    previous,
                    options,
                },
                NumericAdjustment::Choice(index),
            ) => {
                let mut new_memory = memory.clone();
                if let Some(anchor) = new_memory.as_mut().filter(|anchor| anchor.enabled) {
                    anchor.index = index;
                    if let Some(text) = options.entries().get(index as usize) {
                        anchor.text = text.clone();
                    }
                }
                Ok((
                    State::Choice {
                        current,
                        previous,
                        memory,
                    },
                    State::Choice {
                        current: index,
                        previous: index,
                        memory: new_memory,
                    },
                ))
            }
            _ => Err(PointwiseError::UnsupportedCapability(
                "numeric Lab property",
            )),
        }
    }

    pub(crate) fn write_numeric_adjustment(&self, state: &NumericPropertyState) {
        use NumericPropertyState as State;
        let mut entries = guard(&self.owner.core.filters);
        let property = &mut entries[self.owner.ix].parameters[self.ix];
        match (&mut property.value, state) {
            (
                PointwiseParameterValue::Number {
                    current, previous, ..
                },
                State::Number {
                    current: next,
                    previous: last,
                },
            ) => (*current, *previous) = (*next, *last),
            (
                PointwiseParameterValue::Pair {
                    current, previous, ..
                },
                State::Pair {
                    current: next,
                    previous: last,
                },
            ) => (*current, *previous) = (*next, *last),
            (
                PointwiseParameterValue::Choice {
                    current, previous, ..
                },
                State::Choice {
                    current: next,
                    previous: last,
                    memory,
                },
            ) => {
                (*current, *previous) = (*next, *last);
                property.choice_memory.clone_from(memory);
            }
            _ => unreachable!("retained numeric adjustment property changed type"),
        }
    }

    pub(crate) fn normalize_numeric_adjustment(&self) -> Result<(), PointwiseError<E>> {
        self.normalize_size()
    }

    pub(crate) fn notify_numeric_adjustment(&self) -> Result<(), PointwiseError<E>> {
        self.notify_change(crate::ContextChangeArgument::shared_empty())
    }
}

impl<E: 'static> WeakPointwiseFilter<E> {
    pub(crate) fn upgrade(&self) -> Option<PointwiseFilter<E>> {
        let node = self.0.upgrade()?;
        let sponsor = super::region::Sponsor::for_node(&node)?;
        super::PointwiseNode {
            id: node.id,
            sponsor,
        }
        .filter()
    }
}

impl<E: 'static> WeakPointwiseParameter<E> {
    pub(crate) fn upgrade(&self) -> Option<PointwiseParameter<E>> {
        let parameter = PointwiseParameter {
            owner: self.owner.upgrade()?,
            ix: self.index,
        };
        (!parameter.is_retired_wizard_control()).then_some(parameter)
    }
}
impl<E> Clone for PointwiseParameter<E> {
    fn clone(&self) -> Self {
        Self {
            owner: self.owner.clone(),
            ix: self.ix,
        }
    }
}
impl<E> fmt::Debug for PointwiseParameter<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PointwiseParameter({:?}/{})", self.owner, self.ix)
    }
}
fn text(value: &str) -> Vec<u16> {
    value.encode_utf16().collect()
}
fn decimal(value: i32) -> Result<DecimalBits, ImageError> {
    DecimalBits::new([
        value.unsigned_abs(),
        0,
        0,
        if value < 0 { 0x8000_0000 } else { 0 },
    ])
    .map_err(|_| ImageError::Argument)
}
fn limits(
    places: i32,
    min: i32,
    max: i32,
    wrap: bool,
    extended: bool,
) -> Result<SharedLimits, ImageError> {
    let minimum = decimal(min)?;
    let maximum = decimal(max)?;
    let range = if extended {
        let mut r = RangeOptionsRecord::new(minimum, maximum);
        r.flags = [false, true, true];
        Some(SharedRangeOptionsRecord::new(r))
    } else {
        None
    };
    SharedLimits::from_definition(LimitsDefinition {
        decimal_places: places,
        step: LimitsStep::Derived(decimal(1)?),
        minimum,
        maximum,
        used: true,
        use_range: range,
        curve_range: if wrap {
            Some(SharedCurveRangeRecord::new(CurveRangeRecord {
                minimum: decimal(0)?,
                maximum: decimal(360)?,
                wrap_y: true,
            }))
        } else {
            None
        },
    })
    .map_err(|_| ImageError::Overflow)
}
fn number<E>(
    name: &str,
    places: i32,
    min: i32,
    max: i32,
    wrap: bool,
    extended: bool,
) -> Result<ParameterState<E>, ImageError> {
    Ok(ParameterState {
        extras: ParameterExtras::default(),
        choice_memory: None,
        name: text(name),
        value: PointwiseParameterValue::Number {
            current: 0.0,
            previous: 0.0,
            limits: limits(places, min, max, wrap, extended)?,
        },
    })
}
fn checkerboard_parameters<E>() -> Result<Vec<ParameterState<E>>, ImageError> {
    let color = |name, value| ParameterState {
        extras: ParameterExtras::with_ordinary_limits(),
        choice_memory: None,
        name: text(name),
        value: PointwiseParameterValue::Color(crate::ContextColorValue::unlinked([value; 3])),
    };
    let unsigned = |name, default, maximum| -> Result<_, ImageError> {
        Ok(ParameterState {
            extras: ParameterExtras::default(),
            choice_memory: None,
            name: text(name),
            value: PointwiseParameterValue::Number {
                current: default,
                previous: default,
                limits: limits(0, 1, maximum, false, false)?,
            },
        })
    };
    Ok(vec![
        color("Color 1", 0.0),
        color("Color 2", 1.0),
        unsigned("X", 1.0, super::checkerboard::MAX_CELL_SIZE)?,
        unsigned("Y", 1.0, super::checkerboard::MAX_CELL_SIZE)?,
        unsigned("Scale", 2.0, super::checkerboard::MAX_SCALE)?,
    ])
}
fn lab_scale_and_seed<E>() -> Result<[ParameterState<E>; 2], ImageError> {
    let make = |name: &str, definition| -> Result<ParameterState<E>, ImageError> {
        Ok(ParameterState {
            extras: ParameterExtras::default(),
            choice_memory: None,
            name: text(name),
            value: PointwiseParameterValue::Number {
                current: 0.,
                previous: 0.,
                limits: SharedLimits::from_definition_with_profile(
                    definition,
                    crate::LimitsArithmeticProfile::Framework64OleAut32TenFirst,
                )
                .map_err(|_| ImageError::Overflow)?,
            },
        })
    };
    let mut scale_range = RangeOptionsRecord::new(decimal(-20)?, decimal(20)?);
    scale_range.multiplier = 0.1;
    scale_range.flags = [false, true, false];
    let mut seed_range = RangeOptionsRecord::new(decimal(0)?, decimal(1_000)?);
    seed_range.flags = [true, true, true];
    Ok([
        make(
            "Scale",
            LimitsDefinition {
                decimal_places: 3,
                step: LimitsStep::Derived(
                    DecimalBits::new([1, 0, 0, 1 << 16]).map_err(|_| ImageError::Argument)?,
                ),
                minimum: decimal(-100)?,
                maximum: decimal(100)?,
                used: true,
                use_range: Some(SharedRangeOptionsRecord::new(scale_range)),
                curve_range: Some(SharedCurveRangeRecord::new(CurveRangeRecord {
                    minimum: decimal(-4)?,
                    maximum: decimal(4)?,
                    wrap_y: false,
                })),
            },
        )?,
        make(
            "Seed",
            LimitsDefinition {
                decimal_places: 0,
                step: LimitsStep::Derived(decimal(1)?),
                minimum: decimal(-1_000_001)?,
                maximum: decimal(1_000_000)?,
                used: true,
                use_range: Some(SharedRangeOptionsRecord::new(seed_range)),
                curve_range: None,
            },
        )?,
    ])
}
fn choice<E>(name: &str, options: &[&str], current: i32) -> ParameterState<E> {
    ParameterState {
        extras: ParameterExtras::default(),
        choice_memory: None,
        name: text(name),
        value: PointwiseParameterValue::Choice {
            current,
            previous: current,
            options: SharedSelectionOptions::new(options.iter().map(|s| Some(text(s))).collect()),
        },
    }
}
fn oklab_parameters<E>() -> Result<Vec<ParameterState<E>>, ImageError> {
    let chroma = DecimalBits::new([4, 0, 0, 1 << 16]).map_err(|_| ImageError::Argument)?;
    let negative_chroma =
        DecimalBits::new([4, 0, 0, 0x8001_0000]).map_err(|_| ImageError::Argument)?;
    let unit = [decimal(0)?, decimal(1)?];
    let signed = [negative_chroma, chroma];
    let step = DecimalBits::new([1, 0, 0, 4 << 16]).map_err(|_| ImageError::Argument)?;
    [
        ("Lightness", 0.5, unit),
        ("a (green–red)", 0.0, signed),
        ("b (blue–yellow)", 0.0, signed),
        ("Opacity", 1.0, unit),
    ]
    .into_iter()
    .map(|(name, initial, [minimum, maximum])| {
        let limits = SharedLimits::from_definition(LimitsDefinition {
            decimal_places: 4,
            step: LimitsStep::Derived(step),
            minimum,
            maximum,
            used: true,
            use_range: None,
            curve_range: None,
        })
        .map_err(|_| ImageError::Overflow)?;
        Ok(ParameterState {
            extras: ParameterExtras::default(),
            choice_memory: None,
            name: text(name),
            value: PointwiseParameterValue::Number {
                current: initial,
                previous: initial,
                limits,
            },
        })
    })
    .collect()
}
fn blur_number<E>(
    name: &str,
    places: i32,
    initial: f64,
    multiplier: f64,
) -> Result<ParameterState<E>, ImageError> {
    let min = decimal(0)?;
    let max = decimal(1)?;
    let mut range = RangeOptionsRecord::new(min, max);
    range.multiplier = multiplier;
    let step = DecimalBits::new([1, 0, 0, 2 << 16]).map_err(|_| ImageError::Argument)?;
    let limits = SharedLimits::from_definition_with_profile(
        LimitsDefinition {
            decimal_places: places,
            step: LimitsStep::Derived(step),
            minimum: min,
            maximum: max,
            used: true,
            use_range: Some(SharedRangeOptionsRecord::new(range)),
            curve_range: None,
        },
        crate::LimitsArithmeticProfile::Framework64OleAut32TenFirst,
    )
    .map_err(|_| ImageError::Overflow)?;
    Ok(ParameterState {
        extras: ParameterExtras::default(),
        choice_memory: None,
        name: text(name),
        value: PointwiseParameterValue::Number {
            current: initial,
            previous: initial,
            limits,
        },
    })
}
fn environment_number<E>(
    name: &str,
    initial: f64,
    radius: bool,
) -> Result<ParameterState<E>, ImageError> {
    let range = if radius {
        let mut r = RangeOptionsRecord::new(decimal(0)?, decimal(100)?);
        r.multiplier = 0.2;
        Some(SharedRangeOptionsRecord::new(r))
    } else {
        None
    };
    let limits = SharedLimits::from_definition(LimitsDefinition {
        decimal_places: 1,
        step: LimitsStep::Derived(decimal(1)?),
        minimum: decimal(0)?,
        maximum: decimal(100)?,
        used: true,
        use_range: range,
        curve_range: None,
    })
    .map_err(|_| ImageError::Overflow)?;
    Ok(ParameterState {
        extras: ParameterExtras::default(),
        choice_memory: None,
        name: text(name),
        value: PointwiseParameterValue::Number {
            current: initial,
            previous: initial,
            limits,
        },
    })
}
fn optic_number<E>(
    name: &str,
    initial: f64,
    minimum: i32,
    maximum: i32,
    multiplier: f64,
    curve: (i32, i32),
    places: i32,
) -> Result<ParameterState<E>, PointwiseError<E>> {
    let step = DecimalBits::new([1, 0, 0, 2 << 16]).map_err(|_| ImageError::Argument)?;
    let mut p = point_light::numeric(
        name,
        initial,
        places,
        step,
        -1_000_000,
        1_000_000,
        Some((minimum, maximum, multiplier, [false, true, true])),
    )?;
    if let PointwiseParameterValue::Number { limits, .. } = &mut p.value {
        let mut range = RangeOptionsRecord::new(decimal(minimum)?, decimal(maximum)?);
        range.multiplier = multiplier;
        range.flags = [false, true, true];
        let definition = LimitsDefinition {
            decimal_places: places,
            step: LimitsStep::Derived(step),
            minimum: decimal(-1_000_000)?,
            maximum: decimal(1_000_000)?,
            used: true,
            use_range: Some(SharedRangeOptionsRecord::new(range)),
            curve_range: Some(SharedCurveRangeRecord::new(CurveRangeRecord {
                minimum: decimal(curve.0)?,
                maximum: decimal(curve.1)?,
                wrap_y: false,
            })),
        };
        *limits = SharedLimits::from_definition_with_profile(
            definition,
            crate::LimitsArithmeticProfile::Framework64OleAut32TenFirst,
        )
        .map_err(|_| ImageError::Overflow)?;
    }
    Ok(p)
}
fn motion_parameters<E>() -> Result<Vec<ParameterState<E>>, ImageError> {
    let make = |name: &str,
                current: f64,
                definition: LimitsDefinition|
     -> Result<ParameterState<E>, ImageError> {
        let limits = SharedLimits::from_definition_with_profile(
            definition,
            crate::LimitsArithmeticProfile::Framework64OleAut32TenFirst,
        )
        .map_err(|_| ImageError::Overflow)?;
        Ok(ParameterState {
            extras: ParameterExtras::default(),
            choice_memory: None,
            name: text(name),
            value: PointwiseParameterValue::Number {
                current,
                previous: current,
                limits,
            },
        })
    };
    let shape = choice(
        "Shape",
        &[
            "Triangle",
            "Square",
            "Circle",
            "Line",
            "Triple Line",
            "Cross",
        ],
        3,
    );
    let radius = blur_number("Radius", 4, 0.1, 0.2)?;
    let mut range = RangeOptionsRecord::new(decimal(-360)?, decimal(360)?);
    range.flags = [false, true, true];
    let angle = make(
        "Angle",
        0.,
        LimitsDefinition {
            use_range: Some(SharedRangeOptionsRecord::new(range)),
            curve_range: Some(SharedCurveRangeRecord::new(CurveRangeRecord {
                minimum: decimal(0)?,
                maximum: decimal(360)?,
                wrap_y: true,
            })),
            ..LimitsDefinition::default()
        },
    )?;
    let samples = make(
        "Samples",
        20.,
        LimitsDefinition {
            decimal_places: 0,
            step: LimitsStep::Derived(decimal(1)?),
            minimum: decimal(2)?,
            maximum: decimal(100)?,
            used: true,
            use_range: Some(SharedRangeOptionsRecord::new(RangeOptionsRecord::new(
                decimal(2)?,
                decimal(100)?,
            ))),
            curve_range: None,
        },
    )?;
    Ok(vec![
        shape,
        radius,
        angle,
        samples,
        choice("Face Center", &["Yes", "No"], 1),
        choice("Seamless", &["Yes", "No"], 0),
    ])
}
fn stored_text<E>(name: &str) -> ParameterState<E> {
    ParameterState {
        extras: ParameterExtras::with_ordinary_limits(),
        choice_memory: None,
        name: text(name),
        value: PointwiseParameterValue::Text(Some(Vec::new())),
    }
}
fn map_choice<E>(name: &str, current: i32) -> ParameterState<E> {
    let mut p = choice(name, &["None Found"], current);
    p.choice_memory = Some(ChoiceMemory {
        enabled: true,
        index: current,
        text: None,
    });
    p
}
fn map_opacity<E>() -> Result<ParameterState<E>, ImageError> {
    let increment = DecimalBits::new([1, 0, 0, 2 << 16]).map_err(|_| ImageError::Argument)?;
    let limits = SharedLimits::from_definition(LimitsDefinition {
        decimal_places: 3,
        step: LimitsStep::Derived(increment),
        minimum: decimal(0)?,
        maximum: decimal(1)?,
        used: true,
        use_range: None,
        curve_range: None,
    })
    .map_err(|_| ImageError::Overflow)?;
    Ok(ParameterState {
        extras: ParameterExtras::default(),
        choice_memory: None,
        name: text("Opacity"),
        value: PointwiseParameterValue::Number {
            current: 1.0,
            previous: 1.0,
            limits,
        },
    })
}
impl<E> PointwiseFilter<E> {
    pub fn new(kind: PointwiseKind) -> Result<Self, PointwiseError<E>> {
        Self::new_in_mode(kind, PointwiseCloneMode::Original)
    }
    pub(super) fn new_in_mode(
        kind: PointwiseKind,
        mode: PointwiseCloneMode,
    ) -> Result<Self, PointwiseError<E>> {
        let mut parameters = match kind {
            PointwiseKind::Synthesis => vec![
                number("Scale", 3, -10, 10, false, false)?,
                number("Seed", 0, 0, 1_000_000, false, true)?,
            ],
            PointwiseKind::Style => vec![
                number("Scale", 1, -20, 20, false, false)?,
                number("Seed", 0, -1_000_001, 1_000_000, false, true)?,
            ],
            PointwiseKind::Reference => {
                let mut p = number("Slot Number", 0, 1, 20, false, false)?;
                if let PointwiseParameterValue::Number {
                    current, previous, ..
                } = &mut p.value
                {
                    *current = 1.0;
                    *previous = 1.0;
                }
                vec![p]
            }
            PointwiseKind::WizardGroup
            | PointwiseKind::Group
            | PointwiseKind::MapVariable
            | PointwiseKind::WizardReference
            | PointwiseKind::Canvas
            | PointwiseKind::TemplateLab(_) => {
                let inputs = number("Inputs", 0, 0, 20, false, false)?;
                let mut iteration_range = RangeOptionsRecord::new(decimal(1)?, decimal(4)?);
                iteration_range.decimal_places = 0;
                iteration_range.flags = [false, false, false];
                let iteration_limits = SharedLimits::from_definition(LimitsDefinition {
                    decimal_places: 0,
                    step: LimitsStep::Derived(decimal(1)?),
                    minimum: decimal(1)?,
                    maximum: decimal(1_000_000)?,
                    used: false,
                    use_range: Some(SharedRangeOptionsRecord::new(iteration_range)),
                    curve_range: None,
                })
                .map_err(|_| ImageError::Overflow)?;
                let iterations = ParameterState {
                    extras: ParameterExtras::default(),
                    choice_memory: None,
                    name: text("Iterations"),
                    value: PointwiseParameterValue::Number {
                        current: 1.0,
                        previous: 1.0,
                        limits: iteration_limits,
                    },
                };
                let mut values = vec![inputs, iterations, stored_text("Name")];
                if kind == PointwiseKind::Canvas {
                    values.push(transform_parameters::size_parameter(mode)?);
                    values.push(number("Seed", 0, -1_000_001, 1_000_000, false, false)?);
                }
                if let PointwiseKind::TemplateLab(lab) = kind {
                    if let PointwiseParameterValue::Number {
                        current, previous, ..
                    } = &mut values[0].value
                    {
                        *current = lab.template().inputs.len() as f64;
                        *previous = *current;
                    }
                    values.extend(lab_scale_and_seed()?);
                } else if kind == PointwiseKind::WizardReference {
                    values.push(ParameterState {
                        extras: ParameterExtras::default(),
                        choice_memory: None,
                        name: text("Options"),
                        value: PointwiseParameterValue::Variable(crate::VariableSpec::new(
                            crate::VARIABLE_SELECTION,
                        )),
                    });
                } else if kind == PointwiseKind::Group {
                    if let PointwiseParameterValue::Number {
                        current, previous, ..
                    } = &mut values[0].value
                    {
                        *current = 1.0;
                        *previous = 1.0;
                    }
                } else if kind == PointwiseKind::WizardGroup {
                    for index in 0..20 {
                        values.push(ParameterState {
                            extras: ParameterExtras::default(),
                            choice_memory: None,
                            name: text(&format!("Variable {index}")),
                            value: PointwiseParameterValue::Variable(crate::VariableSpec::default()),
                        });
                    }
                }
                if matches!(kind, PointwiseKind::Group | PointwiseKind::WizardGroup) {
                    values.push(choice("Skip for Color Only", &["Yes", "No"], 1));
                }
                if kind == PointwiseKind::MapVariable {
                    let scalar = |name: &str, current: f64| ParameterState {
                        extras: ParameterExtras::default(),
                        choice_memory: None,
                        name: text(name),
                        value: PointwiseParameterValue::Number {
                            current,
                            previous: current,
                            limits: SharedLimits::default(),
                        },
                    };
                    values.push(scalar("Original", 0.));
                    values.push(ParameterState {
                        extras: ParameterExtras::with_ordinary_limits(),
                        choice_memory: None,
                        name: text("Original Color"),
                        value: PointwiseParameterValue::Color(crate::ContextColorValue::unlinked(
                            [0.; 3],
                        )),
                    });
                    values.extend([
                        scalar("Multiply 1", 1.),
                        scalar("Add 1", 0.),
                        scalar("Multiply 2", 1.),
                        scalar("Add 2", 0.),
                    ]);
                    values.push(number("Map to Variable", 0, 0, 19, false, false)?);
                    values.push(choice("Round", &["Yes", "No"], 1));
                }
                if kind == PointwiseKind::Group {
                    values.push(choice("Catch Adjusters", &["Yes", "No"], 1));
                }
                values
            }
            PointwiseKind::SmartBlur => vec![
                blur_number("Radius", 4, 0.1, 0.2)?,
                blur_number("Threshold", 4, 0.1, 0.2)?,
                choice("Mode", &["Brightness", "Multi-Pass"], 0),
                choice("Seamless", &["Yes", "No"], 0),
            ],
            PointwiseKind::Smear => retained_spatial::smear_parameters(mode)?,
            PointwiseKind::VariableBlur => vec![
                blur_number("Radius", 4, 0.1, 0.2)?,
                choice("Seamless", &["Yes", "No"], 0),
            ],
            PointwiseKind::Pixelate => pixelate_parameters()?,
            PointwiseKind::Matrix => matrix_parameters()?,
            PointwiseKind::WizardMatrix => retained_spatial::wizard_matrix_parameters()?,
            PointwiseKind::WizardKaleidoscope => {
                let mut values =
                    transform_parameters::parameters(PointwiseKind::Kaleidoscope, mode)?;
                values.push(number("Mode", 0, 0, 3, false, false)?);
                values
            }
            PointwiseKind::WizardToneCurve => {
                let tenth =
                    DecimalBits::new([1, 0, 0, 1 << 16]).map_err(|_| ImageError::Argument)?;
                let mut range = RangeOptionsRecord::new(decimal(-100)?, decimal(100)?);
                range.multiplier = 0.05;
                let curve_limits = SharedLimits::from_definition_with_profile(
                    LimitsDefinition {
                        decimal_places: 3,
                        step: LimitsStep::Derived(tenth),
                        minimum: decimal(-100)?,
                        maximum: decimal(100)?,
                        used: true,
                        use_range: Some(SharedRangeOptionsRecord::new(range)),
                        curve_range: None,
                    },
                    crate::LimitsArithmeticProfile::Framework64OleAut32TenFirst,
                )
                .map_err(|_| ImageError::Overflow)?;
                let make_unit_limits = || {
                    SharedLimits::from_definition_with_profile(
                        LimitsDefinition {
                            decimal_places: 3,
                            step: LimitsStep::Derived(tenth),
                            minimum: decimal(0)?,
                            maximum: decimal(1)?,
                            used: true,
                            use_range: None,
                            curve_range: None,
                        },
                        crate::LimitsArithmeticProfile::Framework64OleAut32TenFirst,
                    )
                    .map_err(|_| ImageError::Overflow)
                };
                let scalar = |name: &str, current: f64, limits: SharedLimits| ParameterState {
                    extras: ParameterExtras::default(),
                    choice_memory: None,
                    name: text(name),
                    value: PointwiseParameterValue::Number {
                        current,
                        previous: current,
                        limits,
                    },
                };
                vec![
                    scalar("Curve", 1., curve_limits),
                    scalar("Start", 0., make_unit_limits()?),
                    scalar("End", 1., make_unit_limits()?),
                    choice("Affect Red", &["Yes", "No"], 0),
                    choice("Affect Green", &["Yes", "No"], 0),
                    choice("Affect Blue", &["Yes", "No"], 0),
                    choice("Affect Transparency", &["Yes", "No"], 1),
                ]
            }

            PointwiseKind::WizardColor
            | PointwiseKind::WizardFiveColor
            | PointwiseKind::WizardSlideColor => {
                let names: &[&str] = match kind {
                    PointwiseKind::WizardColor => &["Color 1", "Color 2"],
                    PointwiseKind::WizardFiveColor => {
                        &["Color 1", "Color 2", "Color 3", "Color 4", "Color 5"]
                    }
                    PointwiseKind::WizardSlideColor => &["Left Color", "Right Color", "Mid Color"],
                    _ => unreachable!(),
                };
                let mut parameters: Vec<_> = names
                    .iter()
                    .map(|name| ParameterState {
                        extras: ParameterExtras::with_ordinary_limits(),
                        choice_memory: None,
                        name: text(name),
                        value: PointwiseParameterValue::Color(crate::ContextColorValue::unlinked(
                            [0.; 3],
                        )),
                    })
                    .collect();
                if kind == PointwiseKind::WizardSlideColor {
                    let step =
                        DecimalBits::new([1, 0, 0, 1 << 16]).map_err(|_| ImageError::Argument)?;
                    let limits = SharedLimits::from_definition_with_profile(
                        LimitsDefinition {
                            decimal_places: 3,
                            step: LimitsStep::Derived(step),
                            minimum: decimal(0)?,
                            maximum: decimal(1)?,
                            used: true,
                            use_range: None,
                            curve_range: None,
                        },
                        crate::LimitsArithmeticProfile::Framework64OleAut32TenFirst,
                    )
                    .map_err(|_| ImageError::Overflow)?;
                    parameters.push(ParameterState {
                        extras: ParameterExtras::default(),
                        choice_memory: None,
                        name: text("Mid Position"),
                        value: PointwiseParameterValue::Number {
                            current: 0.5,
                            previous: 0.5,
                            limits,
                        },
                    });
                }
                parameters
            }
            PointwiseKind::Rotate => vec![choice(
                "Rotation",
                &["None", "90 Clockwise", "180", "90 Counterclockwise"],
                1,
            )],
            PointwiseKind::GaussianBlur => vec![
                choice("Direction", &["All", "Horizontal", "Vertical"], 0),
                blur_number("Radius", 4, 0.1, 0.2)?,
                choice("Seamless", &["Yes", "No"], 0),
            ],
            PointwiseKind::MotionBlur => motion_parameters()?,
            PointwiseKind::ShapeBlur => vec![
                blur_number("Radius", 4, 0.1, 0.2)?,
                choice("Seamless", &["Yes", "No"], 0),
            ],
            PointwiseKind::MedianBlur => vec![
                blur_number("Radius", 4, 0.1, 0.2)?,
                choice("Seamless", &["Yes", "No"], 0),
            ],
            PointwiseKind::Equalize => vec![blur_number("Radius", 4, 0.1, 0.2)?],
            PointwiseKind::PointLight => point_light::parameters()?,
            PointwiseKind::IlluminateG2 => point_light::illuminate_g2_parameters()?,
            PointwiseKind::FullLight => point_light::full_light_parameters()?,
            PointwiseKind::Shadow => point_light::shadow_parameters()?,
            PointwiseKind::ShadowG2 => point_light::shadow_g2_parameters()?,
            PointwiseKind::Polygon => transform_parameters::polygon_parameters(mode)?,
            PointwiseKind::OklabColor => oklab_parameters()?,
            PointwiseKind::Checkerboard => checkerboard_parameters()?,
            PointwiseKind::Shader => Vec::new(),
            PointwiseKind::LegacyBricksBoards => transform_parameters::parameters(kind, mode)?,
            PointwiseKind::Regions => vec![
                point_light::numeric(
                    "Threshold",
                    0.3,
                    3,
                    DecimalBits::new([1, 0, 0, 2 << 16]).map_err(|_| ImageError::Argument)?,
                    0,
                    1,
                    None,
                )?,
                choice("Remove Gaps", &["Yes", "No"], 0),
                choice("Fill Mode", &super::regions::FILL_MODES, 0),
                choice("Rotate Image", &["Yes", "No"], 0),
                number("Seed", 0, 0, 1_000_000, false, false)?,
            ],
            PointwiseKind::Caustics | PointwiseKind::Transmit => {
                let hundredth =
                    DecimalBits::new([1, 0, 0, 2 << 16]).map_err(|_| ImageError::Argument)?;
                let index = optic_number("Refractive Index", 1.333, -10, 10, 0.3, (1, 3), 3)?;
                let mut elevation = optic_number("Elevation", 1.0, -1, 10, 0.5, (0, 5), 3)?;
                if let PointwiseParameterValue::Number { limits, .. } = &mut elevation.value {
                    let mut range = RangeOptionsRecord::new(decimal(-1)?, decimal(10)?);
                    range.multiplier = 0.5;
                    range.flags = [false, true, true];
                    *limits = SharedLimits::from_definition_with_profile(
                        LimitsDefinition {
                            decimal_places: 3,
                            step: LimitsStep::Derived(hundredth),
                            minimum: decimal(0)?,
                            maximum: decimal(1_000_000)?,
                            used: true,
                            use_range: Some(SharedRangeOptionsRecord::new(range)),
                            curve_range: Some(SharedCurveRangeRecord::new(CurveRangeRecord {
                                minimum: decimal(0)?,
                                maximum: decimal(5)?,
                                wrap_y: false,
                            })),
                        },
                        crate::LimitsArithmeticProfile::Framework64OleAut32TenFirst,
                    )
                    .map_err(|_| ImageError::Overflow)?;
                }
                let height = point_light::numeric(
                    "Height Factor",
                    0.1,
                    4,
                    hundredth,
                    -1_000_000,
                    1_000_000,
                    Some((-3, 3, 0.1, [false, true, true])),
                )?;
                if kind == PointwiseKind::Transmit {
                    let density = point_light::numeric(
                        "Sample Density",
                        3.0,
                        0,
                        decimal(1)?,
                        1,
                        8,
                        Some((1, 8, 1.0, [false, false, false])),
                    )?;
                    vec![
                        index,
                        elevation,
                        height,
                        density,
                        choice("Sample Pattern", &["Regular", "Random"], 1),
                        choice("Blend Mode", &["Add", "Screen"], 0),
                    ]
                } else {
                    let mut exposure_range = RangeOptionsRecord::new(
                        decimal(0)?,
                        DecimalBits::new([3, 0, 0, 1 << 16]).map_err(|_| ImageError::Argument)?,
                    );
                    exposure_range.multiplier = 1.0;
                    let exposure_limits = SharedLimits::from_definition_with_profile(
                        LimitsDefinition {
                            decimal_places: 3,
                            step: LimitsStep::Derived(hundredth),
                            minimum: decimal(0)?,
                            maximum: decimal(1)?,
                            used: true,
                            use_range: Some(SharedRangeOptionsRecord::new(exposure_range)),
                            curve_range: None,
                        },
                        crate::LimitsArithmeticProfile::Framework64OleAut32TenFirst,
                    )
                    .map_err(|_| ImageError::Overflow)?;
                    let exposure = ParameterState {
                        extras: ParameterExtras::default(),
                        choice_memory: None,
                        name: text("Exposure"),
                        value: PointwiseParameterValue::Number {
                            current: 0.1,
                            previous: 0.1,
                            limits: exposure_limits,
                        },
                    };
                    let mut density = point_light::numeric(
                        "Sample Density",
                        2.0,
                        0,
                        decimal(1)?,
                        1,
                        8,
                        Some((1, 8, 1.0, [false, false, false])),
                    )?;
                    if let PointwiseParameterValue::Number { limits, .. } = &mut density.value {
                        let mut range = RangeOptionsRecord::new(decimal(1)?, decimal(8)?);
                        range.flags = [false, false, false];
                        *limits = SharedLimits::from_definition_with_profile(
                            LimitsDefinition {
                                decimal_places: 0,
                                step: LimitsStep::Explicit {
                                    increment: decimal(1)?,
                                    big_increment: decimal(1)?,
                                },
                                minimum: decimal(1)?,
                                maximum: decimal(8)?,
                                used: true,
                                use_range: Some(SharedRangeOptionsRecord::new(range)),
                                curve_range: None,
                            },
                            crate::LimitsArithmeticProfile::Framework64OleAut32TenFirst,
                        )
                        .map_err(|_| ImageError::Overflow)?;
                    }
                    let mut range = RangeOptionsRecord::new(decimal(-10)?, decimal(10)?);
                    range.decimal_places = 0;
                    let slant_limits = SharedLimits::from_definition_with_profile(
                        LimitsDefinition {
                            decimal_places: 3,
                            step: LimitsStep::Derived(
                                DecimalBits::new([1, 0, 0, 1 << 16])
                                    .map_err(|_| ImageError::Argument)?,
                            ),
                            minimum: decimal(-1_000_000)?,
                            maximum: decimal(1_000_000)?,
                            used: true,
                            use_range: Some(SharedRangeOptionsRecord::new(range)),
                            curve_range: Some(SharedCurveRangeRecord::new(CurveRangeRecord {
                                minimum: decimal(-10)?,
                                maximum: decimal(10)?,
                                wrap_y: false,
                            })),
                        },
                        crate::LimitsArithmeticProfile::Framework64OleAut32TenFirst,
                    )
                    .map_err(|_| ImageError::Overflow)?;
                    let slant = ParameterState {
                        extras: ParameterExtras::default(),
                        choice_memory: None,
                        name: text("Slant"),
                        value: PointwiseParameterValue::Pair {
                            current: [0.0; 2],
                            previous: [0.0; 2],
                            limits: slant_limits,
                        },
                    };
                    vec![
                        index,
                        elevation,
                        height,
                        exposure,
                        density,
                        choice("Sample Pattern", &["Regular", "Random"], 0),
                        slant,
                    ]
                }
            }
            PointwiseKind::Stereogram => {
                let make = |name: &str,
                            initial: f64,
                            places: i32,
                            step: DecimalBits,
                            lo: i32,
                            hi: i32|
                 -> Result<ParameterState<E>, PointwiseError<E>> {
                    let limits = SharedLimits::from_definition_with_profile(
                        LimitsDefinition {
                            decimal_places: places,
                            step: LimitsStep::Derived(step),
                            minimum: decimal(lo)?,
                            maximum: decimal(hi)?,
                            used: true,
                            use_range: None,
                            curve_range: None,
                        },
                        crate::LimitsArithmeticProfile::Framework64OleAut32TenFirst,
                    )
                    .map_err(|_| ImageError::Overflow)?;
                    Ok(ParameterState {
                        extras: ParameterExtras::default(),
                        choice_memory: None,
                        name: text(name),
                        value: PointwiseParameterValue::Number {
                            current: initial,
                            previous: initial,
                            limits,
                        },
                    })
                };
                vec![
                    make(
                        "Depth",
                        0.2,
                        3,
                        DecimalBits::new([1, 0, 0, 2 << 16]).map_err(|_| ImageError::Argument)?,
                        -1,
                        1,
                    )?,
                    make("Repetitions", 5.0, 0, decimal(1)?, 1, 100)?,
                ]
            }
            PointwiseKind::Reflect | PointwiseKind::Refract => {
                let height = point_light::numeric(
                    "Height Factor",
                    if kind == PointwiseKind::Reflect {
                        0.01
                    } else {
                        0.1
                    },
                    4,
                    DecimalBits::new([1, 0, 0, 2 << 16]).map_err(|_| ImageError::Argument)?,
                    -1_000_000,
                    1_000_000,
                    Some((-3, 3, 0.1, [false, true, true])),
                )?;
                let elevation = optic_number(
                    "Elevation",
                    if kind == PointwiseKind::Reflect {
                        0.0
                    } else {
                        1.0
                    },
                    -1,
                    10,
                    0.5,
                    (0, 5),
                    3,
                )?;
                if kind == PointwiseKind::Reflect {
                    vec![elevation, height]
                } else {
                    vec![
                        optic_number("Index", 1.333, -10, 10, 0.3, (1, 3), 3)?,
                        elevation,
                        height,
                    ]
                }
            }
            PointwiseKind::ReflectEnvironment | PointwiseKind::RefractEnvironment => {
                let hundredth =
                    DecimalBits::new([1, 0, 0, 2 << 16]).map_err(|_| ImageError::Argument)?;
                let height = point_light::numeric(
                    "Height Factor",
                    0.05,
                    4,
                    hundredth,
                    -1_000_000,
                    1_000_000,
                    Some((-3, 3, 0.1, [false, true, true])),
                )?;
                let skip = choice("Skip for Color Only", &["Yes", "No"], 0);
                if kind == PointwiseKind::ReflectEnvironment {
                    vec![height, skip]
                } else {
                    let mut index = number("Index", 3, -1_000_000, 1_000_000, false, false)?;
                    if let PointwiseParameterValue::Number {
                        current,
                        previous,
                        limits,
                    } = &mut index.value
                    {
                        *current = 1.333;
                        *previous = 1.333;
                        let step = LimitsStep::Derived(
                            DecimalBits::new([1, 0, 0, 2 << 16])
                                .map_err(|_| ImageError::Argument)?,
                        );
                        let mut range = RangeOptionsRecord::new(decimal(-10)?, decimal(10)?);
                        range.multiplier = 0.3;
                        range.flags = [false, true, true];
                        let definition = LimitsDefinition {
                            decimal_places: 3,
                            step,
                            minimum: decimal(-1_000_000)?,
                            maximum: decimal(1_000_000)?,
                            used: true,
                            use_range: Some(SharedRangeOptionsRecord::new(range)),
                            curve_range: Some(SharedCurveRangeRecord::new(CurveRangeRecord {
                                minimum: decimal(1)?,
                                maximum: decimal(3)?,
                                wrap_y: false,
                            })),
                        };
                        *limits = SharedLimits::from_definition(definition)
                            .map_err(|_| ImageError::Overflow)?;
                    }
                    vec![height, index, skip]
                }
            }
            PointwiseKind::DiffuseEnvironment => vec![
                choice("Direction", &["All", "Horizontal", "Vertical"], 0),
                environment_number("Radius", 30.0, true)?,
            ],
            PointwiseKind::ScatterEnvironment => vec![
                environment_number("Radius", 30.0, true)?,
                environment_number("Strength", 50.0, false)?,
            ],
            PointwiseKind::Atmosphere => point_light::atmosphere_parameters()?,
            PointwiseKind::Hair => point_light::hair_parameters()?,
            PointwiseKind::ConvertEnvironment => {
                let formats = [
                    "Latitude/Longitude",
                    "Light Probe (Angular)",
                    "Mirrored Ball",
                    "Horizontal Cross",
                    "Vertical Cross",
                ];
                vec![
                    choice("Input Format", &formats, 0),
                    choice("Output Format", &formats, 2),
                ]
            }
            PointwiseKind::MakeEnvironment => vec![
                ParameterState {
                    name: text("Environment"),
                    value: PointwiseParameterValue::Environment(Arc::default()),
                    extras: ParameterExtras::default(),
                    choice_memory: None,
                },
                choice(
                    "Resolution Limit",
                    &[
                        "None",
                        "3200 x 1600",
                        "1600 x 800",
                        "800 x 400",
                        "400 x 200",
                        "200 x 100",
                        "100 x 50",
                    ],
                    0,
                ),
            ],
            PointwiseKind::RotateEnvironment => vec![
                number("Heading", 1, -360, 360, true, false)?,
                number("Pitch", 1, -360, 360, true, false)?,
                number("Roll", 1, -360, 360, true, false)?,
            ],
            PointwiseKind::Frequency => nonlinear_parameters::frequency()?,
            PointwiseKind::Fractalize => nonlinear_parameters::fractalize()?,
            PointwiseKind::Exposure => nonlinear_parameters::exposure()?,
            PointwiseKind::Selector => nonlinear_parameters::selector()?,
            PointwiseKind::Distort
            | PointwiseKind::FreeTransform
            | PointwiseKind::Kaleidoscope
            | PointwiseKind::MoveRepeat
            | PointwiseKind::Remap
            | PointwiseKind::RotateTile => transform_parameters::parameters(kind, mode)?,
            PointwiseKind::MatchColor => nonlinear_parameters::match_color()?,
            PointwiseKind::ArtBlur => vec![
                blur_number("Radius", 4, 0.1, 0.2)?,
                blur_number("Threshold", 4, 0.1, 0.2)?,
                choice("Seamless", &["Yes", "No"], 0),
            ],
            PointwiseKind::Sharpen => vec![
                blur_number("Strength", 3, 0.015, 0.15)?,
                blur_number("Radius", 3, 0.005, 0.15)?,
            ],
            PointwiseKind::ChangeBci => vec![
                choice("Auto Fix", &["Yes", "No"], 1),
                number("Brightness", 1, -100, 100, false, false)?,
                number("Contrast", 1, -100, 3000, false, true)?,
                number("Intensity", 1, -100, 3000, false, true)?,
            ],
            PointwiseKind::ChangeHsb => vec![
                number("Hue", 0, -360, 360, true, false)?,
                number("Saturation", 0, -100, 100, false, false)?,
                number("Brightness", 0, -100, 100, false, false)?,
            ],
            PointwiseKind::Colorize => vec![
                ParameterState {
                    extras: ParameterExtras::with_ordinary_limits(),
                    choice_memory: None,
                    name: text("Gradient"),
                    value: PointwiseParameterValue::Gradient(None),
                },
                choice("Keep Transparency", &["Yes", "No"], 0),
            ],
            PointwiseKind::ColorizeG2 => {
                let gradient = PointwiseCustomObject::new_legacy_gradient(Default::default());
                vec![
                    ParameterState {
                        extras: ParameterExtras::default(),
                        choice_memory: None,
                        name: text("Color Gradient"),
                        value: PointwiseParameterValue::Matrix {
                            current: Some(gradient.raw()),
                            previous: Some(gradient.source_clone()?.raw()),
                        },
                    },
                    choice("Keep Transparency", &["Yes", "No"], 0),
                ]
            }
            PointwiseKind::Levels => vec![ParameterState {
                extras: ParameterExtras::default(),
                choice_memory: None,
                name: text("Histogram"),
                value: {
                    let current = PointwiseCustomObject::new_histogram();
                    let previous = current.source_clone()?;
                    PointwiseParameterValue::Matrix {
                        current: Some(current.raw()),
                        previous: Some(previous.raw()),
                    }
                },
            }],
            PointwiseKind::GetChannel => vec![choice(
                "Channel",
                &[
                    "Red",
                    "Green",
                    "Blue",
                    "Transparency",
                    "Hue",
                    "Saturation",
                    "Brightness",
                ],
                0,
            )],
            PointwiseKind::Combine => vec![
                choice(
                    "Mode",
                    &[
                        "Normal",
                        "Backlit",
                        "Add",
                        "Subtract",
                        "Multiply",
                        "Divide",
                        "Difference",
                        "Negate",
                        "Greatest",
                        "Least",
                        "Color",
                        "Hue",
                        "Saturation",
                        "Brightness",
                        "Color Lighten",
                        "Color Darken",
                        "Screen",
                        "Tint",
                        "Highlight",
                    ],
                    0,
                ),
                choice("First Input", &["Input 1", "Input 2"], 0),
                choice(
                    "Transparency",
                    &[
                        "Normal", "Product", "Greatest", "Sum", "Top", "Bottom", "None",
                    ],
                    0,
                ),
                map_opacity()?,
                choice("Combine Effect Maps", &["Yes", "No"], 1),
            ],
            PointwiseKind::ToneCurve | PointwiseKind::ToneCurveG2 => {
                let curve = if kind == PointwiseKind::ToneCurveG2 {
                    let legacy = PointwiseCustomObject::new_legacy_tone_curve([1.0, 0.0, 1.0]);
                    ParameterState {
                        extras: ParameterExtras::default(),
                        choice_memory: None,
                        name: text("Tone Curve"),
                        value: PointwiseParameterValue::Matrix {
                            current: Some(legacy.raw()),
                            previous: Some(legacy.source_clone()?.raw()),
                        },
                    }
                } else {
                    ParameterState {
                        extras: ParameterExtras::with_ordinary_limits(),
                        choice_memory: None,
                        name: text("Tone Curve"),
                        value: PointwiseParameterValue::Curve(None),
                    }
                };
                vec![
                    curve,
                    choice("Affect Red", &["Yes", "No"], 0),
                    choice("Affect Green", &["Yes", "No"], 0),
                    choice("Affect Blue", &["Yes", "No"], 0),
                    choice("Affect Transparency", &["Yes", "No"], 1),
                ]
            }
            PointwiseKind::LayerMaps => vec![
                map_choice("First Map", 0),
                map_choice("Second Map", 1),
                choice(
                    "Combine Mode",
                    &[
                        "Normal",
                        "Add",
                        "Subtract",
                        "Multiply",
                        "Inverse Multiply",
                        "Difference",
                        "Negate",
                        "Greatest",
                        "Least",
                    ],
                    3,
                ),
                map_opacity()?,
                stored_text("New Map Name"),
                choice(
                    "Clear Original",
                    &["First", "Second", "Both", "All", "None"],
                    4,
                ),
                choice("Fix Contrast", &["Yes", "No"], 1),
            ],
            PointwiseKind::ClearMaps => vec![
                choice("Action", &["Keep Selection", "Delete Selection"], 0),
                choice(
                    "Selection",
                    &[
                        "None", "First", "Second", "Third", "Fourth", "Fifth", "Sixth", "Seventh",
                        "Eighth", "Ninth", "Tenth", "Last",
                    ],
                    0,
                ),
                stored_text("Kept Name"),
            ],
            PointwiseKind::StoreMap => vec![
                stored_text("Map Name"),
                choice("Clear Previous", &["Yes", "No"], 1),
                choice("Fix Contrast", &["Yes", "No"], 1),
            ],
            PointwiseKind::MergeMaps => vec![
                stored_text("New Map Name"),
                choice("Clear Originals", &["Yes", "No"], 1),
                choice("Fix Contrast", &["Yes", "No"], 1),
            ],
            _ => Vec::new(),
        };
        for p in &mut parameters {
            let compound = matches!(
                p.value,
                PointwiseParameterValue::Text(_)
                    | PointwiseParameterValue::Variable(_)
                    | PointwiseParameterValue::Gradient(_)
                    | PointwiseParameterValue::Environment(_)
                    | PointwiseParameterValue::Curve(_)
            );
            let mut animatable = !compound;
            let mut adjustable = !matches!(
                p.value,
                PointwiseParameterValue::Text(_)
                    | PointwiseParameterValue::Variable(_)
                    | PointwiseParameterValue::Matrix { .. }
                    | PointwiseParameterValue::Region { .. }
                    | PointwiseParameterValue::Histogram(_)
            );
            let name = String::from_utf16_lossy(&p.name);
            if matches!(
                (kind, name.as_str()),
                (
                    PointwiseKind::Colorize | PointwiseKind::ColorizeG2,
                    "Keep Transparency"
                ) | (PointwiseKind::MakeEnvironment, "Resolution Limit")
                    | (PointwiseKind::StoreMap, "Clear Previous" | "Fix Contrast")
                    | (PointwiseKind::MergeMaps, "Clear Originals" | "Fix Contrast")
                    | (PointwiseKind::ClearMaps, "Action" | "Selection")
                    | (
                        PointwiseKind::LayerMaps,
                        "First Map" | "Second Map" | "Clear Original" | "Fix Contrast"
                    )
                    | (PointwiseKind::Combine, "Combine Effect Maps")
                    | (PointwiseKind::Synthesis, "Scale" | "Seed")
                    | (
                        PointwiseKind::PointLight
                            | PointwiseKind::IlluminateG2
                            | PointwiseKind::Shadow
                            | PointwiseKind::ShadowG2
                            | PointwiseKind::FullLight
                            | PointwiseKind::ReflectEnvironment
                            | PointwiseKind::RefractEnvironment,
                        "Skip for Color Only"
                    )
            ) {
                animatable = false;
            }
            if matches!(
                (kind, name.as_str()),
                (PointwiseKind::LayerMaps, "First Map" | "Second Map")
                    | (PointwiseKind::MakeEnvironment, "Resolution Limit")
            ) {
                adjustable = false;
            }
            if matches!(
                kind,
                PointwiseKind::PointLight
                    | PointwiseKind::IlluminateG2
                    | PointwiseKind::Shadow
                    | PointwiseKind::ShadowG2
                    | PointwiseKind::FullLight
                    | PointwiseKind::ReflectEnvironment
                    | PointwiseKind::RefractEnvironment
            ) && name == "Skip for Color Only"
            {
                adjustable = false;
            }
            if matches!(
                kind,
                PointwiseKind::WizardGroup
                    | PointwiseKind::Group
                    | PointwiseKind::MapVariable
                    | PointwiseKind::WizardReference
                    | PointwiseKind::Canvas
            ) && matches!(
                name.as_str(),
                "Inputs" | "Skip for Color Only" | "Catch Adjusters"
            ) {
                animatable = false;
                adjustable = false;
            }
            if matches!(kind, PointwiseKind::TemplateLab(_))
                && matches!(name.as_str(), "Inputs" | "Scale" | "Seed")
            {
                animatable = false;
                adjustable = name != "Inputs";
            }
            p.extras.wire.set_capabilities(animatable, adjustable);
        }
        let core = Arc::new(Cohort {
            filters: Mutex::new(vec![FilterState {
                factor: 1.0,
                canvas: (kind == PointwiseKind::Canvas).then(Arc::default),
                style: (kind == PointwiseKind::Style).then(Arc::default),
                synthesis: (kind == PointwiseKind::Synthesis).then(Arc::default),
                synthesis_wire: (kind == PointwiseKind::Synthesis)
                    .then(|| wire::LegacyWireMetadata::new(false, true)),
                synthesis_catalog: None,
                lab_inheritance: (1., 0.),
                wizard_template: None,
                previous_templates: Default::default(),
                clone_mode: mode,
                kind,
                parameters,
                wizard_dynamic: None,
                wizard_label: None,
                wizard_positions: Vec::new(),
                group_guid: matches!(kind, PointwiseKind::Group | PointwiseKind::MapVariable)
                    .then(|| uuid::Uuid::new_v4().to_bytes_le()),
                group_presets: (kind == PointwiseKind::Group)
                    .then(|| Arc::new(Mutex::new(crate::GtxPresetCollection::default()))),
                cache: None,
                handlers: Vec::new(),
            }]),
        });
        let filter = Self::publish(core, 0);
        if kind.is_group() || kind == PointwiseKind::Canvas {
            filter.node().append_content(Vec::new());
        }
        Ok(filter)
    }
    pub(super) fn publish(core: Arc<Cohort<E>>, ix: usize) -> Self {
        let graph_id = super::region::NodeId::new();
        let node = super::graph::NodeCore::filter(
            graph_id,
            Arc::clone(&core),
            ix,
            guard(&core.filters)[ix].kind,
        );
        let sponsor = super::region::Sponsor::publish(&node);
        let filter = Self {
            core,
            ix,
            graph_id,
            sponsor,
        };
        filter.bind_matrix_objects();
        filter
    }
    pub(super) fn from_graph(
        core: Arc<Cohort<E>>,
        ix: usize,
        graph_id: super::region::NodeId,
        sponsor: Arc<super::region::Sponsor<E>>,
    ) -> Self {
        Self {
            core,
            ix,
            graph_id,
            sponsor,
        }
    }
    pub fn node(&self) -> super::PointwiseNode<E> {
        super::PointwiseNode::from_filter(self)
    }

    pub(crate) fn downgrade(&self) -> WeakPointwiseFilter<E> {
        WeakPointwiseFilter(Arc::downgrade(&self.node().core()))
    }
    /// Actual CIL/capture accepts null/default; only unrecognized nonnull names
    /// fail. This differs from the recovered decompiler switch spelling.
    pub fn legacy_channel(name: Option<&[u16]>) -> Result<Self, PointwiseError<E>>
    where
        E: 'static,
    {
        let filter = Self::new(PointwiseKind::GetChannel)?;
        let index = match name {
            None => 0,
            Some(v) if v == text("Hue") => 4,
            Some(v) if v == text("Saturation") => 5,
            Some(v) if v == text("Brightness") => 6,
            _ => return Err(PointwiseError::Exception),
        };
        filter.parameter(0)?.set_index(index)?;
        Ok(filter)
    }
    pub fn kind(&self) -> PointwiseKind {
        guard(&self.core.filters)[self.ix].kind
    }
    pub(super) fn lab_inheritance(&self) -> (f64, f64) {
        guard(&self.core.filters)[self.ix].lab_inheritance
    }
    pub(super) fn canvas_document(&self) -> Option<Arc<super::canvas::CanvasDocument>> {
        guard(&self.core.filters)[self.ix].canvas.clone()
    }

    pub(super) fn style(&self) -> Option<Arc<super::canvas::CanvasStyle>> {
        guard(&self.core.filters)[self.ix].style.clone()
    }

    pub(super) fn set_style(&self, style: super::canvas::CanvasStyle) {
        guard(&self.core.filters)[self.ix].style = Some(Arc::new(style));
        self.node().invalidate();
    }

    pub(super) fn exchange_style(
        &self,
        expected: &Arc<super::canvas::CanvasStyle>,
        replacement: Arc<super::canvas::CanvasStyle>,
    ) -> bool {
        let mut filters = guard(&self.core.filters);
        let style = &mut filters[self.ix].style;
        if !style
            .as_ref()
            .is_some_and(|value| Arc::ptr_eq(value, expected))
        {
            return false;
        }
        *style = Some(replacement);
        true
    }

    pub(super) fn synthesis_texture(&self) -> Option<Arc<super::canvas::SynthesisTexture>> {
        let (catalog, local) = {
            let filters = guard(&self.core.filters);
            let filter = &filters[self.ix];
            (filter.synthesis_catalog.clone(), filter.synthesis.clone())
        };
        catalog.map(|catalog| catalog.lock().clone()).or(local)
    }

    pub(crate) fn synthesis_wire(&self) -> Option<wire::LegacyWireMetadata<E>> {
        guard(&self.core.filters)[self.ix].synthesis_wire.clone()
    }

    pub(crate) fn attach_synthesis_catalog(&self, value: crate::SynthesisCatalogValue) {
        guard(&self.core.filters)[self.ix].synthesis_catalog = Some(value);
        self.node().invalidate();
    }

    pub(crate) fn synthesis_catalog(&self) -> Option<crate::SynthesisCatalogValue> {
        guard(&self.core.filters)[self.ix].synthesis_catalog.clone()
    }

    pub fn is_synthesis_adjusted(&self) -> bool {
        self.synthesis_wire()
            .and_then(|wire| wire.retained_adjustment())
            .is_some_and(|link| link.property_type() == 10 && link.is_active().unwrap_or(false))
    }

    pub(crate) fn synthesis_link_state(&self) -> Option<SynthesisLinkState<E>> {
        let filters = guard(&self.core.filters);
        let filter = &filters[self.ix];
        let wire = filter.synthesis_wire.as_ref()?;
        Some(SynthesisLinkState {
            adjustment: wire.retained_adjustment(),
            catalog: filter.synthesis_catalog.clone(),
            local: crate::SynthesisRecord::from_texture(filter.synthesis.clone()?),
        })
    }

    pub(crate) fn replace_synthesis_link(
        &self,
        expected: &SynthesisLinkState<E>,
        replacement: &SynthesisLinkState<E>,
    ) -> bool {
        let mut filters = guard(&self.core.filters);
        let filter = &mut filters[self.ix];
        let same_catalog = match (&filter.synthesis_catalog, &expected.catalog) {
            (Some(actual), Some(expected)) => actual.same_reference(expected),
            (None, None) => true,
            _ => false,
        };
        if !same_catalog
            || filter
                .synthesis
                .as_ref()
                .is_none_or(|actual| !Arc::ptr_eq(actual, expected.local.texture()))
            || filter.synthesis_wire.as_ref().is_none_or(|wire| {
                !wire.compare_and_replace_adjustment(
                    expected.adjustment.as_ref(),
                    replacement.adjustment.clone(),
                )
            })
        {
            return false;
        }
        filter.synthesis_catalog = replacement.catalog.clone();
        filter.synthesis = Some(replacement.local.texture().clone());
        true
    }

    pub(crate) fn notify_synthesis_receiver_change(&self) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        self.node()
            .notify_property_change_with(crate::ContextChangeArgument::shared_empty())
    }

    pub(super) fn notify_synthesis_change(&self) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        let node = self.node();
        let catalog = self.synthesis_catalog();
        if let Some(catalog) = catalog {
            for core in node.sponsor.nodes() {
                if core.id == node.id {
                    continue;
                }
                let peer = super::PointwiseNode {
                    id: core.id,
                    sponsor: node.sponsor.clone(),
                };
                if peer
                    .filter()
                    .is_some_and(|filter| filter.uses_synthesis_material(&catalog))
                {
                    peer.invalidate();
                }
            }
        }
        self.notify_synthesis_receiver_change()
    }

    pub(super) fn set_synthesis_texture(&self, texture: super::canvas::SynthesisTexture) {
        guard(&self.core.filters)[self.ix].synthesis = Some(Arc::new(texture));
        self.node().invalidate();
    }

    pub(super) fn exchange_synthesis_texture(
        &self,
        expected: &Arc<super::canvas::SynthesisTexture>,
        replacement: Arc<super::canvas::SynthesisTexture>,
        controls: Option<([f64; 2], [f64; 2])>,
    ) -> bool {
        // Lab preset application also takes catalog locks before editing filters.
        let catalog = self.synthesis_catalog();
        let mut shared = catalog.as_ref().map(|catalog| catalog.lock());
        let mut filters = guard(&self.core.filters);
        let filter = &mut filters[self.ix];
        match (&filter.synthesis_catalog, &catalog) {
            (Some(current), Some(expected)) if current.same_reference(expected) => {}
            (None, None) => {}
            _ => return false,
        }
        let actual = shared.as_deref().or(filter.synthesis.as_ref());
        if actual.is_none_or(|value| !Arc::ptr_eq(value, expected)) {
            return false;
        }
        if let Some((before, after)) = controls {
            let [scale, seed, ..] = filter.parameters.as_mut_slice() else {
                return false;
            };
            let PointwiseParameterValue::Number {
                current: scale,
                previous: old_scale,
                ..
            } = &mut scale.value
            else {
                return false;
            };
            let PointwiseParameterValue::Number {
                current: seed,
                previous: old_seed,
                ..
            } = &mut seed.value
            else {
                return false;
            };
            if [*scale, *seed] != before {
                return false;
            }
            [*scale, *seed] = after;
            [*old_scale, *old_seed] = after;
        }
        if let Some(texture) = &mut shared {
            **texture = replacement;
        } else {
            filter.synthesis = Some(replacement);
        }
        true
    }

    pub(super) fn synthesis_preset_controls(
        &self,
        values: [f64; 2],
    ) -> Result<([f64; 2], [f64; 2]), PointwiseError<E>> {
        let filters = guard(&self.core.filters);
        let [scale, seed, ..] = filters[self.ix].parameters.as_slice() else {
            return Err(PointwiseError::InvalidCast);
        };
        let read = |parameter: &ParameterState<E>, value| {
            let PointwiseParameterValue::Number {
                current, limits, ..
            } = &parameter.value
            else {
                return Err(PointwiseError::InvalidCast);
            };
            Ok((*current, normalize_parameter(limits, value)?))
        };
        let (old_scale, scale) = read(scale, values[0])?;
        let (old_seed, seed) = read(seed, values[1])?;
        Ok(([old_scale, old_seed], [scale, seed]))
    }

    pub(super) fn set_canvas_document(&self, document: super::canvas::CanvasDocument) {
        guard(&self.core.filters)[self.ix].canvas = Some(Arc::new(document));
    }

    pub(super) fn exchange_canvas_document(
        &self,
        expected: &Arc<super::canvas::CanvasDocument>,
        replacement: Arc<super::canvas::CanvasDocument>,
    ) -> bool {
        let mut filters = guard(&self.core.filters);
        let canvas = &mut filters[self.ix].canvas;
        if canvas
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, expected))
        {
            *canvas = Some(replacement);
            true
        } else {
            false
        }
    }
    pub(super) fn set_lab_inherited_scale(&self, scale: f64) {
        guard(&self.core.filters)[self.ix].lab_inheritance.0 = scale;
    }
    pub(super) fn set_lab_inherited_seed(&self, seed: f64) {
        guard(&self.core.filters)[self.ix].lab_inheritance.1 = seed;
    }
    pub(super) fn wizard_template(&self) -> Option<&'static str> {
        guard(&self.core.filters)[self.ix].wizard_template
    }
    pub(super) fn cached_wizard_template(&self, path: &str) -> Option<super::region::NodeId> {
        guard(&self.core.filters)[self.ix]
            .previous_templates
            .get(path)
            .copied()
    }
    pub(super) fn remember_wizard_template(
        &self,
        selected: Option<&'static str>,
        previous: Option<super::region::NodeId>,
    ) {
        let mut entries = guard(&self.core.filters);
        let state = &mut entries[self.ix];
        if let (Some(path), Some(previous)) = (state.wizard_template, previous) {
            state.previous_templates.insert(path, previous);
        }
        if let Some(path) = selected {
            state.previous_templates.remove(path);
        }
        state.wizard_template = selected;
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.core, &other.core) && self.ix == other.ix
    }
    pub fn parameter(&self, ix: i32) -> Result<PointwiseParameter<E>, PointwiseError<E>> {
        let ix = usize::try_from(ix).map_err(|_| PointwiseError::Index)?;
        let entries = guard(&self.core.filters);
        let state = &entries[self.ix];
        let count = match state.kind {
            PointwiseKind::WizardGroup => WIZARD_BASE_PARAMETERS,
            PointwiseKind::WizardReference => 4,
            _ => state.parameters.len(),
        };
        if ix >= count {
            return Err(PointwiseError::Index);
        }
        drop(entries);
        Ok(PointwiseParameter {
            owner: self.clone(),
            ix,
        })
    }
    pub fn parameters(&self) -> Vec<PointwiseParameter<E>> {
        let entries = guard(&self.core.filters);
        let state = &entries[self.ix];
        let count = match state.kind {
            PointwiseKind::WizardGroup => WIZARD_BASE_PARAMETERS,
            PointwiseKind::WizardReference => 4,
            _ => state.parameters.len(),
        };
        drop(entries);
        (0..count)
            .map(|ix| PointwiseParameter {
                owner: self.clone(),
                ix,
            })
            .collect()
    }
    pub(crate) fn retained_parameters(&self) -> Vec<PointwiseParameter<E>> {
        let mut parameters = self.parameters();
        let entries = guard(&self.core.filters);
        if let Some(dynamic) = &entries[self.ix].wizard_dynamic {
            parameters.extend(dynamic.iter().map(|&ix| PointwiseParameter {
                owner: self.clone(),
                ix,
            }));
        }
        parameters
    }
    pub(super) fn wizard_control_variable_index(
        &self,
        control: &PointwiseParameter<E>,
    ) -> Option<i32> {
        if !self.same_reference(&control.owner) {
            return None;
        }
        let entries = guard(&self.core.filters);
        let state = &entries[self.ix];
        if state.kind != PointwiseKind::WizardGroup {
            return None;
        }
        let dynamic_index = state
            .wizard_dynamic
            .as_ref()?
            .iter()
            .position(|ix| *ix == control.ix)?;
        let mut active = 0;
        for index in 0..20 {
            let PointwiseParameterValue::Variable(spec) = &state.parameters[index + 3].value else {
                return None;
            };
            if spec.mode() != crate::VARIABLE_INACTIVE {
                if active == dynamic_index {
                    return Some(index as i32);
                }
                active += 1;
            }
        }
        None
    }
    /// The actual retained controls derived lazily from active Wizard variables.
    /// Rebuilding keeps old property handles valid, as the source owner does.
    pub fn wizard_dynamic_properties(
        &self,
    ) -> Result<Vec<PointwiseParameter<E>>, PointwiseError<E>> {
        let mut entries = guard(&self.core.filters);
        let state = &mut entries[self.ix];
        if state.kind == PointwiseKind::WizardReference {
            if state.wizard_dynamic.is_none() {
                let PointwiseParameterValue::Variable(spec) = &state.parameters[3].value else {
                    return Err(PointwiseError::InvalidCast);
                };
                let mut controls = Vec::new();
                if spec.options().is_some_and(|text| !text.is_empty()) {
                    let options = spec
                        .resolved_options()
                        .ok_or(PointwiseError::InvalidOperation)?;
                    let property = ParameterState {
                        extras: ParameterExtras::default(),
                        choice_memory: None,
                        name: spec.name().unwrap_or_default().to_vec(),
                        value: PointwiseParameterValue::Choice {
                            current: 0,
                            previous: 0,
                            options: SharedSelectionOptions::new(
                                options.iter().map(|(name, _)| Some(name.clone())).collect(),
                            ),
                        },
                    };
                    controls.push(state.parameters.len());
                    state.parameters.push(property);
                }
                state.wizard_dynamic = Some(controls);
            }
            return Ok(state
                .wizard_dynamic
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|&ix| PointwiseParameter {
                    owner: self.clone(),
                    ix,
                })
                .collect());
        }
        if state.kind != PointwiseKind::WizardGroup {
            return Err(PointwiseError::InvalidCast);
        }
        if state.wizard_dynamic.is_none() {
            let mut values = Vec::new();
            for ix in 3..23 {
                let PointwiseParameterValue::Variable(spec) = &state.parameters[ix].value else {
                    return Err(PointwiseError::InvalidCast);
                };
                let name = spec.name().ok_or(PointwiseError::NullReference);
                let value = match spec.mode() {
                    crate::VARIABLE_NUMERIC => {
                        let [negative, whole, nonzero] = spec.numeric_flags();
                        let mut p = number(
                            "",
                            if whole { 0 } else { 1 },
                            if negative {
                                -100
                            } else if nonzero {
                                1
                            } else {
                                0
                            },
                            100,
                            false,
                            false,
                        )?;
                        p.name = name?.to_vec();
                        p
                    }
                    crate::VARIABLE_SELECTION => {
                        let options = spec.options().ok_or(PointwiseError::NullReference)?;
                        ParameterState {
                            extras: ParameterExtras::default(),
                            choice_memory: None,
                            name: name?.to_vec(),
                            value: PointwiseParameterValue::Choice {
                                current: 0,
                                previous: 0,
                                options: SharedSelectionOptions::new(
                                    options
                                        .split(|c| *c == b';' as u16)
                                        .map(|v| Some(v.to_vec()))
                                        .collect(),
                                ),
                            },
                        }
                    }
                    crate::VARIABLE_COLOR => ParameterState {
                        extras: ParameterExtras::with_ordinary_limits(),
                        choice_memory: None,
                        name: name?.to_vec(),
                        value: PointwiseParameterValue::Color(crate::ContextColorValue::unlinked(
                            [0.; 3],
                        )),
                    },
                    crate::VARIABLE_ENVIRONMENT => wizard_environment_parameter(name?.to_vec())?,
                    crate::VARIABLE_TEXTURE_SYNTHESIS => {
                        return Err(PointwiseError::UnsupportedCapability(
                            "Wizard Group texture synthesis controls",
                        ));
                    }
                    _ => continue,
                };
                value
                    .extras
                    .wire
                    .set_capabilities(spec.mode() != crate::VARIABLE_ENVIRONMENT, true);
                values.push(value);
            }
            let start = state.parameters.len();
            state.parameters.extend(values);
            state.wizard_dynamic = Some((start..state.parameters.len()).collect());
        }
        Ok(state
            .wizard_dynamic
            .as_ref()
            .unwrap()
            .iter()
            .map(|&ix| PointwiseParameter {
                owner: self.clone(),
                ix,
            })
            .collect())
    }
    pub(crate) fn load_wizard_label(&self, label: Vec<u16>) {
        if !label.is_empty() {
            guard(&self.core.filters)[self.ix].wizard_label = Some(label);
        }
    }

    /// Add an Environment control in the first vacant Wizard slot and keep the
    /// existing control identities, values and history targets intact.
    pub fn add_wizard_environment_variable(&self) -> Result<Option<i32>, PointwiseError<E>>
    where
        E: 'static,
    {
        self.wizard_dynamic_properties()?;
        let index = {
            let mut entries = guard(&self.core.filters);
            let state = &mut entries[self.ix];
            let Some(slot) = (3..23).find(|&ix| {
                matches!(&state.parameters[ix].value,
                PointwiseParameterValue::Variable(spec) if spec.mode() == crate::VARIABLE_INACTIVE)
            }) else {
                return Ok(None);
            };
            let index = slot - 3;
            let name: Vec<u16> = format!("Environment {index}").encode_utf16().collect();
            let mut spec = crate::VariableSpec::new(crate::VARIABLE_ENVIRONMENT);
            spec.set_name(Some(&name));
            let control = wizard_environment_parameter(name)?;
            let dynamic = state
                .wizard_dynamic
                .as_mut()
                .ok_or(PointwiseError::InvalidOperation)?;
            dynamic.try_reserve(1).map_err(|_| ImageError::Allocation)?;
            state
                .parameters
                .try_reserve(1)
                .map_err(|_| ImageError::Allocation)?;
            dynamic.insert(index, state.parameters.len());
            state.parameters[slot].value = PointwiseParameterValue::Variable(spec);
            state.parameters.push(control);
            index as i32
        };
        self.parameter(index + 3)?
            .notify_change(retained_spatial::ordinary())?;
        Ok(Some(index))
    }
    pub fn wizard_label(&self) -> Option<Vec<u16>> {
        guard(&self.core.filters)[self.ix].wizard_label.clone()
    }
    pub fn wizard_content_positions(&self) -> Vec<[f32; 2]> {
        guard(&self.core.filters)[self.ix].wizard_positions.clone()
    }
    pub fn group_content_positions(&self) -> Vec<[f32; 2]> {
        self.wizard_content_positions()
    }
    pub(super) fn replace_wizard_content_positions(&self, positions: Vec<[f32; 2]>) {
        guard(&self.core.filters)[self.ix].wizard_positions = positions;
    }
    /// Add a separate group contents occurrence with its editor position.
    pub fn append_wizard_content(
        &self,
        position: [f32; 2],
    ) -> Result<super::PointwiseSlot<E>, PointwiseError<E>> {
        if !matches!(
            self.kind(),
            PointwiseKind::WizardGroup | PointwiseKind::Group | PointwiseKind::MapVariable
        ) {
            return Err(PointwiseError::InvalidCast);
        }
        let slot = self.node().append_content(Vec::new());
        guard(&self.core.filters)[self.ix]
            .wizard_positions
            .push(position);
        Ok(slot)
    }
    pub fn append_group_content(
        &self,
        position: [f32; 2],
    ) -> Result<super::PointwiseSlot<E>, PointwiseError<E>> {
        self.append_wizard_content(position)
    }
    /// Remove an empty extra contents occurrence and its editor position.
    /// The first contents occurrence is required by the group wire format.
    pub fn remove_empty_group_content(
        &self,
        target: &super::PointwiseSlot<E>,
    ) -> Result<(), PointwiseError<E>> {
        if !matches!(
            self.kind(),
            PointwiseKind::WizardGroup | PointwiseKind::Group | PointwiseKind::MapVariable
        ) {
            return Err(PointwiseError::InvalidCast);
        }
        let mut filters = guard(&self.core.filters);
        let positions = &mut filters[self.ix].wizard_positions;
        let index = self
            .node()
            .remove_empty_group_content(target, positions.len())?;
        positions.remove(index);
        Ok(())
    }
    pub fn group_guid(&self) -> Option<[u8; 16]> {
        guard(&self.core.filters)[self.ix].group_guid
    }
    pub(crate) fn set_group_guid(&self, guid: [u8; 16]) {
        guard(&self.core.filters)[self.ix].group_guid = Some(guid);
    }
    /// Shared group preset collection; unsupported preset value types still
    /// report the existing wire codec's explicit error.
    pub fn group_presets(&self) -> Option<Arc<Mutex<crate::GtxPresetCollection>>> {
        guard(&self.core.filters)[self.ix].group_presets.clone()
    }
    pub fn cached_image(&self) -> Option<SharedImageMap> {
        self.cached_rendered_output()?.still_image()
    }

    pub(super) fn cached_rendered_output(&self) -> Option<super::RenderedImage> {
        guard(&self.core.filters)[self.ix].cache.clone()
    }

    pub fn set_cached_image(&self, image: Option<SharedImageMap>) -> Result<(), PointwiseError<E>> {
        self.set_rendered_output(image.map(super::RenderedImage::Still))
    }

    pub(super) fn set_rendered_output(
        &self,
        image: Option<super::RenderedImage>,
    ) -> Result<(), PointwiseError<E>> {
        let handlers = {
            let mut entries = guard(&self.core.filters);
            let state = &mut entries[self.ix];
            state.cache = image;
            state.handlers.clone()
        };
        // No filter/map/parameter guard is alive during actual reentry.
        for handler in handlers {
            (handler.0)(self).map_err(PointwiseError::Observer)?;
        }
        self.node().notify_rendered_image_changed()
    }
    pub(super) fn clear_cache_without_notification(&self) {
        let old = guard(&self.core.filters)[self.ix].cache.take();
        drop(old);
    }
    pub fn add_cache_handler(&self, handler: PointwiseCacheHandler<E>) {
        guard(&self.core.filters)[self.ix].handlers.push(handler);
    }
    pub fn remove_cache_handler(&self, handler: &PointwiseCacheHandler<E>) {
        let removed = {
            let mut entries = guard(&self.core.filters);
            let handlers = &mut entries[self.ix].handlers;
            handlers
                .iter()
                .rposition(|h| h.same_binding(handler))
                .map(|i| handlers.remove(i))
        };
        drop(removed);
    }
    pub fn source_clone(&self) -> Result<Self, PointwiseError<E>>
    where
        E: 'static,
    {
        self.node()
            .source_clone()?
            .filter()
            .ok_or(PointwiseError::NullReference)
    }
    pub(super) fn point_light_from_illuminate(legacy: &Self) -> Result<Self, PointwiseError<E>>
    where
        E: 'static,
    {
        let light = Self::new(PointwiseKind::PointLight)?;
        let legacy_position = legacy
            .parameter(1)?
            .custom_object()?
            .ok_or(PointwiseError::NullReference)?;
        let position = light
            .parameter(0)?
            .custom_object()?
            .ok_or(PointwiseError::NullReference)?;
        let [x, y] = legacy_position.light_position()?;
        position.set_light_position_x(x)?;
        position.set_light_position_y(y)?;

        let height = legacy.parameter(0)?;
        light.parameter(1)?.set_number(height.number())?;
        let extras = height.copy_extras()?;
        guard(&light.core.filters)[light.ix].parameters[1].extras = extras;
        let half = legacy
            .parameter(2)?
            .text()?
            .is_some_and(|name| name.into_iter().eq("Half".encode_utf16()));
        light
            .parameter(3)?
            .set_number(if half { 0.0 } else { 100.0 })?;
        light
            .parameter(6)?
            .set_index(legacy.parameter(3)?.index())?;
        Ok(light)
    }
    pub(super) fn copy_local(&self) -> Result<Self, PointwiseError<E>>
    where
        E: 'static,
    {
        // Gradient copy evaluates the original lazy getter BEFORE cloning.
        let original = (0..guard(&self.core.filters)[self.ix].parameters.len())
            .map(|ix| PointwiseParameter {
                owner: self.clone(),
                ix,
            })
            .collect::<Vec<_>>();
        let mut parameters = Vec::with_capacity(original.len());
        for p in original {
            let mut value = p.raw_value();
            match value {
                PointwiseParameterValue::Color(ref color) => {
                    // os copy starts at default rr and invokes component-equality
                    // setter; zero RGB therefore does not adopt source APV.
                    if color.components() == [0.; 3] {
                        value = PointwiseParameterValue::Color(crate::ContextColorValue::unlinked(
                            [0.; 3],
                        ));
                    }
                }
                PointwiseParameterValue::Gradient(_) => {
                    value = PointwiseParameterValue::Gradient(Some(p.gradient()?.source_clone()?))
                }
                PointwiseParameterValue::Curve(_) => {
                    // os copy calls the original lazy a6 getter, then rl copy.
                    // Both child operations are outside the owning cohort guard.
                    value = PointwiseParameterValue::Curve(Some(p.curve()?.source_clone()?))
                }
                PointwiseParameterValue::Region { ref current, .. } => {
                    let fresh = PointwiseCustomObject::new_region_for_property();
                    if let Some(current) = current {
                        fresh.region()?.copy_fields_from(&current.region()?);
                    }
                    let previous = fresh.source_clone()?;
                    value = PointwiseParameterValue::Region {
                        current: Some(fresh.raw()),
                        previous: Some(previous.raw()),
                    };
                }
                PointwiseParameterValue::Matrix { ref current, .. } => {
                    let fresh = if matches!(
                        self.kind(),
                        PointwiseKind::PointLight
                            | PointwiseKind::IlluminateG2
                            | PointwiseKind::Shadow
                            | PointwiseKind::ShadowG2
                            | PointwiseKind::FullLight
                            | PointwiseKind::Atmosphere
                            | PointwiseKind::Hair
                    ) {
                        let fresh = PointwiseCustomObject::new_light_position();
                        if let Some(current) = current {
                            fresh.set_light_position_x(current.light_position_x()?)?;
                            fresh.set_light_position_y(current.light_position_y()?)?;
                        }
                        fresh
                    } else if self.kind() == PointwiseKind::ToneCurveG2 {
                        let fields = current
                            .as_ref()
                            .map_or(Ok([1.0, 0.0, 1.0]), |object| object.legacy_tone_curve())?;
                        PointwiseCustomObject::new_legacy_tone_curve(fields)
                    } else if self.kind() == PointwiseKind::ColorizeG2 {
                        let value = current.as_ref().map_or_else(
                            || Ok(Default::default()),
                            |value| value.legacy_gradient(),
                        )?;
                        PointwiseCustomObject::new_legacy_gradient(value)
                    } else if self.kind() == PointwiseKind::Levels {
                        let fresh = PointwiseCustomObject::new_histogram();
                        if let Some(source) =
                            current.as_ref().and_then(PointwiseCustomObject::histogram)
                        {
                            let target = fresh.histogram().ok_or(PointwiseError::NullReference)?;
                            target.set_channel(source.selected_channel());
                            for i in 0..4 {
                                for (a, b) in target.arrays().into_iter().zip(source.arrays()) {
                                    a.write(i, b.read(i)?)?;
                                }
                            }
                        }
                        fresh
                    } else {
                        let fresh = PointwiseMatrix::new()?;
                        if let Some(current) = current {
                            fresh
                                .payload()
                                .copy_from(Some(&current.matrix()?.payload()))
                                .map_err(matrix_error)?;
                        }
                        fresh.custom_object()
                    };
                    let previous = fresh.source_clone()?;
                    value = PointwiseParameterValue::Matrix {
                        current: Some(fresh.raw()),
                        previous: Some(previous.raw()),
                    };
                }
                PointwiseParameterValue::Histogram(ref h) => {
                    value = PointwiseParameterValue::Histogram(h.source_clone())
                }
                _ => (),
            };
            let choice_memory = {
                guard(&p.owner.core.filters)[p.owner.ix].parameters[p.ix]
                    .choice_memory
                    .clone()
                    .filter(|memory| memory.enabled)
            };
            parameters.push(ParameterState {
                extras: p.copy_extras()?,
                choice_memory,
                name: p.name(),
                value,
            });
        }
        let kind = self.kind();
        let group_presets = self
            .group_presets()
            .map(|presets| {
                guard(&presets)
                    .source_copy()
                    .map(|copy| Arc::new(Mutex::new(copy)))
                    .map_err(PointwiseError::Gradient)
            })
            .transpose()?;
        let synthesis_wire = self
            .synthesis_wire()
            .map(|wire| wire.source_clone())
            .transpose()?;
        let ix = {
            let mut entries = guard(&self.core.filters);
            let wizard_dynamic = entries[self.ix].wizard_dynamic.clone();
            let wizard_label = entries[self.ix].wizard_label.clone();
            let wizard_positions = entries[self.ix].wizard_positions.clone();
            let group_guid = entries[self.ix].group_guid;
            let lab_inheritance = entries[self.ix].lab_inheritance;
            let canvas = entries[self.ix].canvas.clone();
            let style = entries[self.ix].style.clone();
            let synthesis = entries[self.ix].synthesis.clone();
            let synthesis_catalog = entries[self.ix].synthesis_catalog.clone();
            let wizard_template = entries[self.ix].wizard_template;
            let ix = entries.len();
            entries.push(FilterState {
                factor: 1.0,
                canvas,
                style,
                synthesis,
                synthesis_wire,
                synthesis_catalog,
                lab_inheritance,
                wizard_template,
                previous_templates: Default::default(),
                clone_mode: PointwiseCloneMode::Original,
                kind,
                parameters,
                wizard_dynamic,
                wizard_label,
                wizard_positions,
                group_guid,
                group_presets,
                cache: None,
                handlers: Vec::new(),
            });
            ix
        };
        let copy = Self::publish(Arc::clone(&self.core), ix);
        guard(&copy.core.filters)[copy.ix].factor = self.factor();
        guard(&copy.core.filters)[copy.ix].clone_mode = self.clone_mode();
        Ok(copy)
    }
}
const WIZARD_BASE_PARAMETERS: usize = 24;

impl<E> PointwiseParameter<E> {
    pub fn same_reference(&self, other: &Self) -> bool {
        self.ix == other.ix && self.owner.same_reference(&other.owner)
    }
    pub fn owner(&self) -> PointwiseFilter<E> {
        self.owner.clone()
    }

    pub(crate) fn downgrade(&self) -> WeakPointwiseParameter<E> {
        WeakPointwiseParameter {
            owner: self.owner.downgrade(),
            index: self.ix,
        }
    }
    pub fn wizard_variable_index(&self) -> Option<i32> {
        self.owner.wizard_control_variable_index(self)
    }

    pub fn is_retired_wizard_control(&self) -> bool {
        self.ix >= WIZARD_BASE_PARAMETERS
            && self.owner.kind() == PointwiseKind::WizardGroup
            && self.wizard_variable_index().is_none()
    }
    pub fn name(&self) -> Vec<u16> {
        guard(&self.owner.core.filters)[self.owner.ix].parameters[self.ix]
            .name
            .clone()
    }
    pub fn value(&self) -> PointwiseParameterValue<E> {
        let value = guard(&self.owner.core.filters)[self.owner.ix].parameters[self.ix]
            .value
            .clone();
        match value {
            PointwiseParameterValue::Region { current, previous } => {
                PointwiseParameterValue::Region {
                    current: current.map(|o| o.external()),
                    previous: previous.map(|o| o.external()),
                }
            }
            PointwiseParameterValue::Matrix { current, previous } => {
                if self.owner.kind() == PointwiseKind::Levels
                    && let Some(h) = current.as_ref().and_then(PointwiseCustomObject::histogram)
                {
                    return PointwiseParameterValue::Histogram(h);
                }
                PointwiseParameterValue::Matrix {
                    current: current.map(|o| o.external()),
                    previous: previous.map(|o| o.external()),
                }
            }
            other => other,
        }
    }
    pub fn number(&self) -> f64 {
        match self.value() {
            PointwiseParameterValue::Number { current, .. } => current,
            _ => 0.0,
        }
    }
    /// The bound setter writes the raw value before the caller publishes a
    /// change. Number and pair setters also update their previous values.
    pub(super) fn write_bound_value(
        &self,
        value: &super::MappedVariableValue,
        mapped_axis: Option<usize>,
        force: bool,
    ) -> Result<bool, PointwiseError<E>>
    where
        E: 'static,
    {
        let raw = self.raw_value();
        let number = match &raw {
            PointwiseParameterValue::Number { limits, .. }
            | PointwiseParameterValue::Pair { limits, .. } => {
                normalize_parameter(limits, value.number())?
            }
            _ => value.number(),
        };
        let mut entries = guard(&self.owner.core.filters);
        let current_value = &mut entries[self.owner.ix].parameters[self.ix].value;
        let changed = match current_value {
            PointwiseParameterValue::Number {
                current, previous, ..
            } => {
                if *current == number && !force {
                    return Ok(false);
                }
                *current = number;
                *previous = number;
                true
            }
            PointwiseParameterValue::Pair {
                current, previous, ..
            } => {
                if let Some(axis) = mapped_axis {
                    if current[axis] == number && !force {
                        return Ok(false);
                    }
                    current[axis] = number;
                    previous[axis] = number;
                } else {
                    if *current == [number; 2] && !force {
                        return Ok(false);
                    }
                    *current = [number; 2];
                    *previous = [number; 2];
                }
                drop(entries);
                self.normalize_size()?;
                true
            }
            PointwiseParameterValue::Choice {
                current, options, ..
            } => {
                let options = options.entries();
                let index = options
                    .iter()
                    .position(|entry| entry.as_deref() == value.selection())
                    .map_or_else(
                        || crate::framework_cast::integer(number),
                        |index| index as i32,
                    );
                if *current == index && !force {
                    return Ok(false);
                }
                if index < 0 || index as usize >= options.len() {
                    return Err(PointwiseError::Exception);
                }
                *current = index;
                true
            }
            PointwiseParameterValue::Color(current) => {
                let color = crate::ContextColorValue::unlinked(value.color());
                if current.equals_components(&color) && !force {
                    return Ok(false);
                }
                let old = std::mem::replace(current, color);
                drop(entries);
                drop(old);
                true
            }
            _ => {
                return Err(PointwiseError::UnsupportedCapability(
                    "variable binding property type",
                ));
            }
        };
        Ok(changed)
    }

    pub fn set_number(&self, value: f64) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        let value = self.store_number(value)?;
        if let Some((old, value)) = value {
            let undo = if self.can_create_property_undo()? {
                Some(Arc::new(crate::DoubleUndoInfo::new(
                    Some(self.name()),
                    0,
                    old,
                    value,
                )))
            } else {
                None
            };
            self.notify_change(Arc::new(crate::ContextChangeArgument::Property(
                crate::ContextPropertyChangeArgs::with_undo(undo),
            )))?;
        }
        Ok(())
    }

    pub(super) fn set_group_inputs_for_graph_edit(
        &self,
        value: f64,
    ) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        debug_assert!(self.ix == 0 && self.owner.kind().is_group());
        if let Some((old, new)) = self.store_number(value)? {
            self.owner.node().notify_group_input_count_change(
                old,
                new,
                crate::ContextChangeArgument::shared_empty(),
            )?;
        }
        Ok(())
    }

    fn store_number(&self, value: f64) -> Result<Option<(f64, f64)>, PointwiseError<E>> {
        if self.owner.kind() == PointwiseKind::Checkerboard && value.fract() != 0.0 {
            return Err(ImageError::Argument.into());
        }
        let limits = match self.value() {
            PointwiseParameterValue::Number { limits, .. } => limits,
            _ => return Ok(None),
        };
        let value = normalize_parameter(&limits, value)?;
        let mut entries = guard(&self.owner.core.filters);
        let mut old = None;
        if let PointwiseParameterValue::Number {
            current, previous, ..
        } = &mut entries[self.owner.ix].parameters[self.ix].value
            && (*current != value || *previous != *current)
        {
            old = Some(if *previous != *current {
                *previous
            } else {
                *current
            });
            *current = value;
            *previous = value;
        }
        Ok(old.map(|old| (old, value)))
    }
    pub(super) fn can_create_property_undo(&self) -> Result<bool, PointwiseError<E>>
    where
        E: 'static,
    {
        if self.owner.clone_mode() != PointwiseCloneMode::Original
            || (!self.owner.node().has_property_history_target()
                && self.wizard_variable_index().is_none())
        {
            return Ok(false);
        }
        let wire = guard(&self.owner.core.filters)[self.owner.ix].parameters[self.ix]
            .extras
            .wire
            .clone();
        Ok(!wire.is_adjusted()?)
    }
    pub(in crate::pointwise) fn apply_choice_history(
        &self,
        value: &crate::ChoiceUndoValue,
    ) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        let crate::ChoiceUndoValue::Remembered(value) = value else {
            if let crate::ChoiceUndoValue::Index(value) = value {
                return self.set_index(*value);
            }
            unreachable!();
        };
        let old =
            {
                let mut entries = guard(&self.owner.core.filters);
                let parameter = &mut entries[self.owner.ix].parameters[self.ix];
                let PointwiseParameterValue::Choice { current, .. } = &mut parameter.value else {
                    return Err(PointwiseError::InvalidCast);
                };
                let memory = parameter.choice_memory.get_or_insert(ChoiceMemory {
                    enabled: false,
                    index: 0,
                    text: None,
                });
                let old = crate::ChoiceUndoValue::Remembered(Arc::new(
                    crate::ChoiceUndoAnchor::new(*current, memory.index, memory.text.clone()),
                ));
                // os.a(ae7) deliberately does NOT update m_ad (previous).
                *current = value.index();
                memory.index = value.anchor();
                memory.text = value.text().map(<[u16]>::to_vec);
                old
            };
        let undo = if self.can_create_property_undo()? {
            Some(Arc::new(crate::ChoiceUndoInfo::new(
                Some(self.name()),
                old,
                crate::ChoiceUndoValue::Remembered(Arc::new(crate::ChoiceUndoAnchor::new(
                    value.index(),
                    value.anchor(),
                    value.text().map(<[u16]>::to_vec),
                ))),
            )))
        } else {
            None
        };
        self.notify_change(Arc::new(crate::ContextChangeArgument::Property(
            crate::ContextPropertyChangeArgs::with_choice_undo(undo),
        )))
    }
    pub fn index(&self) -> i32 {
        match self.value() {
            PointwiseParameterValue::Choice { current, .. } => current,
            _ => 0,
        }
    }
    pub fn set_index(&self, value: i32) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        if self.owner.kind() == PointwiseKind::WizardReference && self.ix >= 4 {
            if !self
                .owner
                .wizard_dynamic_properties()?
                .iter()
                .any(|property| property.same_reference(self))
            {
                return Err(PointwiseError::InvalidOperation);
            }
            super::node_wire::switch_wizard_template(&self.owner, value)?;
        }
        self.set_index_from_wire(value)
    }

    pub(super) fn set_index_from_wire(&self, value: i32) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        let mut undo_values = None;
        let mut roots = guard(&self.owner.core.filters);
        let parameter = &mut roots[self.owner.ix].parameters[self.ix];
        if let PointwiseParameterValue::Choice {
            current,
            previous,
            options,
        } = &mut parameter.value
            && value >= 0
            && (*current != value || *previous != value)
        {
            let old_index = if *previous != value {
                *previous
            } else {
                *current
            };
            let old = match parameter
                .choice_memory
                .as_ref()
                .filter(|memory| memory.enabled)
            {
                Some(memory) => crate::ChoiceUndoValue::Remembered(Arc::new(
                    crate::ChoiceUndoAnchor::new(old_index, memory.index, memory.text.clone()),
                )),
                None => crate::ChoiceUndoValue::Index(old_index),
            };
            *current = value;
            *previous = value;
            let new = if let Some(memory) = parameter
                .choice_memory
                .as_mut()
                .filter(|memory| memory.enabled)
            {
                memory.index = value;
                if let Some(text) = options.entries().get(value as usize) {
                    memory.text = text.clone();
                }
                crate::ChoiceUndoValue::Remembered(Arc::new(crate::ChoiceUndoAnchor::new(
                    value,
                    memory.index,
                    memory.text.clone(),
                )))
            } else {
                crate::ChoiceUndoValue::Index(value)
            };
            undo_values = Some((old, new));
        }
        drop(roots);
        if let Some((old, new)) = undo_values {
            let undo = if self.can_create_property_undo()? {
                Some(Arc::new(crate::ChoiceUndoInfo::new(
                    Some(self.name()),
                    old,
                    new,
                )))
            } else {
                None
            };
            self.notify_change(Arc::new(crate::ContextChangeArgument::Property(
                crate::ContextPropertyChangeArgs::with_choice_undo(undo),
            )))?;
        }
        Ok(())
    }
    /// Actual os.aa selection anchor (bn/z), where present. This read never
    /// resolves text, changes options or publishes a property notification.
    pub fn remembered_selection(&self) -> Option<(i32, Option<Vec<u16>>)> {
        guard(&self.owner.core.filters)[self.owner.ix].parameters[self.ix]
            .choice_memory
            .as_ref()
            .filter(|memory| memory.enabled)
            .map(|m| (m.index, m.text.clone()))
    }
    pub fn options(&self) -> Result<SharedSelectionOptions, PointwiseError<E>> {
        match self.value() {
            PointwiseParameterValue::Choice { options, .. } => Ok(options),
            _ => Err(PointwiseError::NullReference),
        }
    }
    pub fn set_options(
        &self,
        value: Option<SharedSelectionOptions>,
    ) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        let value = value.ok_or(PointwiseError::NullReference)?;
        let entries = value.entries();
        if self.options()?.entries() == entries {
            return Ok(());
        }
        let mut roots = guard(&self.owner.core.filters);
        let parameter = &mut roots[self.owner.ix].parameters[self.ix];
        if let PointwiseParameterValue::Choice {
            options, current, ..
        } = &mut parameter.value
        {
            // Source publishes m_i before o() resolves the remembered anchor.
            // o() changes current, not previous/anchor, then emits a
            // controls-only PropertyChangedEventArgs(false,true). zl.k7 does
            // not invalidate rendering for that event. Generic Changed delegate
            // delivery/typed arguments remain outside this parameter API.
            *options = value;
            let (index, text) = parameter
                .choice_memory
                .as_ref()
                .map_or((0, None), |m| (m.index, m.text.as_ref()));
            let selected = if index < entries.len() as i32 {
                let at = usize::try_from(index).map_err(|_| PointwiseError::Index)?;
                if entries[at].as_ref() == text {
                    Some(index)
                } else {
                    None
                }
            } else {
                None
            };
            *current = selected
                .or_else(|| {
                    entries
                        .iter()
                        .position(|v| v.as_ref() == text)
                        .map(|i| i as i32)
                })
                .unwrap_or(index);
        }
        Ok(())
    }
    pub fn text(&self) -> Result<Option<Vec<u16>>, PointwiseError<E>> {
        let value = self.value();
        match value {
            PointwiseParameterValue::Choice {
                current, options, ..
            } => options
                .entries()
                .get(usize::try_from(current).map_err(|_| PointwiseError::Index)?)
                .cloned()
                .ok_or(PointwiseError::Index),
            PointwiseParameterValue::Text(value) => Ok(value),
            _ => Err(PointwiseError::NullReference),
        }
    }

    pub(crate) fn compare_and_replace_text(
        &self,
        expected: Option<&[u16]>,
        next: Option<&[u16]>,
    ) -> bool {
        let mut entries = guard(&self.owner.core.filters);
        let PointwiseParameterValue::Text(current) =
            &mut entries[self.owner.ix].parameters[self.ix].value
        else {
            return false;
        };
        if current.as_deref() != expected {
            return false;
        }
        *current = next.map(<[u16]>::to_vec);
        true
    }
    /// Source os.e(string) for stored o1.c values. String.Copy(old) occurs
    /// before assignment: after null commits, a different next value fails.
    pub fn set_text(&self, value: Option<&[u16]>) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        let mut entries = guard(&self.owner.core.filters);
        let PointwiseParameterValue::Text(current) =
            &mut entries[self.owner.ix].parameters[self.ix].value
        else {
            return Err(PointwiseError::NullReference);
        };
        if current.as_deref() == value {
            return Ok(());
        }
        if current.is_none() {
            return Err(PointwiseError::ArgumentNull);
        }
        *current = value.map(<[u16]>::to_vec);
        drop(entries);
        self.notify_change(retained_spatial::ordinary())?;
        Ok(())
    }
    pub fn variable(&self) -> Result<crate::VariableSpec, PointwiseError<E>> {
        match self.value() {
            PointwiseParameterValue::Variable(value) => Ok(value),
            _ => Err(PointwiseError::NullReference),
        }
    }
    /// os.a(an3) compares the serialized value and publishes before notifying.
    pub fn set_variable(&self, value: crate::VariableSpec) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        let mut entries = guard(&self.owner.core.filters);
        let PointwiseParameterValue::Variable(current) =
            &mut entries[self.owner.ix].parameters[self.ix].value
        else {
            return Err(PointwiseError::NullReference);
        };
        if current.source_equals(&value) {
            return Ok(());
        }
        *current = value;
        if (entries[self.owner.ix].kind == PointwiseKind::WizardGroup && (3..23).contains(&self.ix))
            || (entries[self.owner.ix].kind == PointwiseKind::WizardReference && self.ix == 3)
        {
            entries[self.owner.ix].wizard_dynamic = None;
        }
        drop(entries);
        self.notify_change(retained_spatial::ordinary())
    }
    pub fn gradient(&self) -> Result<SharedGradient, PointwiseError<E>> {
        match self.value() {
            PointwiseParameterValue::Gradient(Some(g)) => return Ok(g),
            PointwiseParameterValue::Gradient(None) => (),
            _ => return Err(PointwiseError::NullReference),
        }
        let g = SharedGradient::new();
        // os.ai publishes its lazy Gradient directly, without a Changed event.
        {
            let mut entries = guard(&self.owner.core.filters);
            if let PointwiseParameterValue::Gradient(value) =
                &mut entries[self.owner.ix].parameters[self.ix].value
            {
                *value = Some(g.clone());
            }
        }
        g.add_normalized_color(0.0, 0.0, 0.0, 0.0)?;
        g.add_normalized_color(1.0, 1.0, 1.0, 1.0)?;
        Ok(g)
    }
    pub fn set_environment(
        &self,
        environment: Arc<crate::EnvironmentRecord>,
    ) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        self.replace_environment(None, environment).map(|_| ())
    }

    pub(crate) fn write_environment(
        &self,
        environment: Arc<crate::EnvironmentRecord>,
        force: bool,
    ) -> Result<bool, PointwiseError<E>> {
        let mut entries = guard(&self.owner.core.filters);
        let PointwiseParameterValue::Environment(current) =
            &mut entries[self.owner.ix].parameters[self.ix].value
        else {
            return Err(PointwiseError::InvalidCast);
        };
        if !force && Arc::ptr_eq(current, &environment) {
            return Ok(false);
        }
        *current = environment;
        Ok(true)
    }

    pub(crate) fn replace_environment_adjustment(
        &self,
        expected: Option<&super::SharedPropertyAdjustment<E>>,
        adjustment: Option<super::SharedPropertyAdjustment<E>>,
        map: Arc<crate::EnvironmentRecord>,
    ) -> bool {
        let mut entries = guard(&self.owner.core.filters);
        let parameter = &mut entries[self.owner.ix].parameters[self.ix];
        let PointwiseParameterValue::Environment(current) = &mut parameter.value else {
            return false;
        };
        if !parameter
            .extras
            .wire
            .compare_and_replace_adjustment(expected, adjustment)
        {
            return false;
        }
        *current = map;
        true
    }

    pub(crate) fn notify_environment_change(&self) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        self.notify_change(crate::ContextChangeArgument::shared_empty())
    }

    /// Replaces the value only if it is still the version the caller edited.
    pub fn compare_and_set_environment(
        &self,
        expected: &Arc<crate::EnvironmentRecord>,
        environment: Arc<crate::EnvironmentRecord>,
    ) -> Result<bool, PointwiseError<E>>
    where
        E: 'static,
    {
        self.replace_environment(Some(expected), environment)
    }

    fn replace_environment(
        &self,
        expected: Option<&Arc<crate::EnvironmentRecord>>,
        environment: Arc<crate::EnvironmentRecord>,
    ) -> Result<bool, PointwiseError<E>>
    where
        E: 'static,
    {
        let old = {
            let mut entries = guard(&self.owner.core.filters);
            let PointwiseParameterValue::Environment(value) =
                &mut entries[self.owner.ix].parameters[self.ix].value
            else {
                return Err(PointwiseError::InvalidCast);
            };
            if expected.is_some_and(|expected| !Arc::ptr_eq(value, expected)) {
                return Ok(false);
            }
            if Arc::ptr_eq(value, &environment) {
                return Ok(true);
            }
            std::mem::replace(value, environment.clone())
        };
        let undo = if self.can_create_property_undo()? {
            Some(Arc::new(crate::EnvironmentUndoInfo::new(
                self.name(),
                old,
                environment,
            )))
        } else {
            None
        };
        self.notify_change(Arc::new(crate::ContextChangeArgument::Property(
            crate::ContextPropertyChangeArgs::with_environment_undo(undo),
        )))?;
        Ok(true)
    }

    pub(super) fn replace_adjusted_gradient(
        &self,
        expected: &Option<SharedGradient>,
        value: &Option<SharedGradient>,
    ) -> bool {
        let mut entries = guard(&self.owner.core.filters);
        let PointwiseParameterValue::Gradient(current) =
            &mut entries[self.owner.ix].parameters[self.ix].value
        else {
            return false;
        };
        if !super::reference_adjustment::same_gradient(current, expected) {
            return false;
        }
        *current = value.clone();
        true
    }

    pub fn set_gradient(&self, value: Option<SharedGradient>) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        let old = {
            let mut entries = guard(&self.owner.core.filters);
            match &mut entries[self.owner.ix].parameters[self.ix].value {
                PointwiseParameterValue::Gradient(g) => {
                    if super::reference_adjustment::same_gradient(g, &value) {
                        return Ok(());
                    }
                    std::mem::replace(g, value)
                }
                _ => return Err(PointwiseError::NullReference),
            }
        };
        drop(old);
        self.notify_change(retained_spatial::ordinary())?;
        Ok(())
    }

    /// Install a detached editor gradient and record its replacement in the document.
    pub fn edit_gradient(
        &self,
        expected: &SharedGradient,
        gradient: SharedGradient,
    ) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        let edit = GradientEdit {
            property: self.downgrade(),
            before: Some(expected.clone()),
            after: Some(gradient),
        };
        let history_error = |error| PointwiseError::History(Box::new(error));
        let slot = self
            .owner()
            .node()
            .texture_node_logic()
            .and_then(|logic| logic.slot());
        edit.replace(self, HistoryDirection::Redo)
            .map_err(history_error)?;
        if let Some(slot) = slot
            && let Some(history) = slot.history()
        {
            let sequence = history.edit_sequence();
            let recorded = GradientEdit {
                property: self.downgrade(),
                before: edit.before.clone(),
                after: edit.after.clone(),
            };
            if let Err(error) =
                history.record_graph_edit(slot.history_target(), self.name(), Box::new(recorded))
            {
                if history.edit_sequence() == sequence {
                    let _ = edit.replace(self, HistoryDirection::Undo);
                }
                return Err(history_error(error));
            }
        }
        Ok(())
    }
    /// Actual os.a6 lazy curve. The new root publishes before the default Arc
    /// layer is appended. Retaining this handle keeps the real curve/cohort alive;
    /// direct child edits do not invent a property Changed event.
    pub fn curve(&self) -> Result<SharedCurve<E>, PointwiseError<E>> {
        match self.value() {
            PointwiseParameterValue::Curve(Some(curve)) => return Ok(curve),
            PointwiseParameterValue::Curve(None) => (),
            _ => return Err(PointwiseError::NullReference),
        }
        let curve = SharedCurve::new(0.0, 1.0, false);
        {
            let mut entries = guard(&self.owner.core.filters);
            if let PointwiseParameterValue::Curve(value) =
                &mut entries[self.owner.ix].parameters[self.ix].value
            {
                *value = Some(curve.clone());
            }
        }
        curve.insert(0, Some(&CurveLayerHandle::new(AnalyticCurveKind::Arc)))?;
        Ok(curve)
    }
    /// Change the Curve link and value together before notifying the graph.
    pub(crate) fn replace_curve_adjustment(
        &self,
        expected: (
            Option<&super::SharedPropertyAdjustment<E>>,
            Option<&SharedCurve<E>>,
        ),
        replacement: (
            Option<super::SharedPropertyAdjustment<E>>,
            Option<SharedCurve<E>>,
        ),
    ) -> bool {
        let mut entries = guard(&self.owner.core.filters);
        let parameter = &mut entries[self.owner.ix].parameters[self.ix];
        let PointwiseParameterValue::Curve(current) = &mut parameter.value else {
            return false;
        };
        let matches = match (current.as_ref(), expected.1) {
            (Some(current), Some(expected)) => current.same_reference(expected),
            (None, None) => true,
            _ => false,
        };
        if !matches
            || !parameter
                .extras
                .wire
                .compare_and_replace_adjustment(expected.0, replacement.0)
        {
            return false;
        }
        *current = replacement.1;
        true
    }

    pub(crate) fn notify_curve_adjustment(&self) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        self.notify_change(retained_spatial::ordinary())
    }

    /// Actual os.a(rl) reference replacement. Null remains lazy until the next
    /// getter; old roots survive independently and notifications occur after the
    /// property guard is released, through existing node invalidation.
    pub fn set_curve(&self, value: Option<SharedCurve<E>>) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        self.swap_curve(value, None)
    }

    fn swap_curve(
        &self,
        value: Option<SharedCurve<E>>,
        expected: Option<&SharedCurve<E>>,
    ) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        let installed = expected.and(value.as_ref()).cloned();
        let old = {
            let mut entries = guard(&self.owner.core.filters);
            match &mut entries[self.owner.ix].parameters[self.ix].value {
                PointwiseParameterValue::Curve(curve) => {
                    if expected.is_some_and(|expected| {
                        !curve
                            .as_ref()
                            .is_some_and(|old| old.same_reference(expected))
                    }) {
                        return Err(PointwiseError::History(Box::new(
                            HistoryError::InputChanged,
                        )));
                    }
                    let same = match (&*curve, &value) {
                        (None, None) => true,
                        (Some(a), Some(b)) => a.same_reference(b),
                        _ => false,
                    };
                    if same {
                        return Ok(());
                    }
                    std::mem::replace(curve, value)
                }
                _ => return Err(PointwiseError::NullReference),
            }
        };
        drop(old);
        self.notify_change(retained_spatial::ordinary())?;
        // An observer can replace the property again during notification.
        // Editor commits and document history must detect that stale root.
        if let Some(installed) = installed
            && !matches!(self.raw_value(), PointwiseParameterValue::Curve(Some(current)) if current.same_reference(&installed))
        {
            return Err(PointwiseError::History(Box::new(
                HistoryError::InputChanged,
            )));
        }
        Ok(())
    }

    /// Replace or prepend the preset layers while keeping the destination's range and wrapping.
    pub fn apply_curve_preset(
        &self,
        expected: &SharedCurve<E>,
        preset: &SharedCurve<E>,
        insert: bool,
    ) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        let preset = expected.with_preset_layers(preset, insert)?;
        let label = if insert {
            "Insert Curve Preset"
        } else {
            "Replace Curve Layers"
        };
        self.edit_curve_with_label(expected, preset, label)
    }

    /// Publish a detached curve edit and retain both roots for document history.
    pub fn edit_curve(
        &self,
        expected: &SharedCurve<E>,
        curve: SharedCurve<E>,
    ) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        self.edit_curve_with_label(expected, curve, "Edit Curve")
    }

    pub(crate) fn edit_curve_with_label(
        &self,
        expected: &SharedCurve<E>,
        curve: SharedCurve<E>,
        label: &str,
    ) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        if expected.same_reference(&curve) {
            return self.swap_curve(Some(curve), Some(expected));
        }
        let slot = self
            .owner()
            .node()
            .texture_node_logic()
            .and_then(|logic| logic.slot());
        let history = slot.as_ref().and_then(|slot| slot.history());
        self.swap_curve(Some(curve.clone()), Some(expected))?;
        if let Some((slot, history)) = slot.zip(history) {
            history
                .record_graph_edit(
                    slot.history_target(),
                    label.encode_utf16().collect(),
                    Box::new(CurveEdit {
                        property: self.downgrade(),
                        before: expected.clone(),
                        after: curve,
                    }),
                )
                .map_err(|error| PointwiseError::History(Box::new(error)))?;
        }
        Ok(())
    }
    pub fn histogram(&self) -> Result<Histogram, PointwiseError<E>> {
        match self.raw_value() {
            PointwiseParameterValue::Matrix { current, .. } => current
                .ok_or(PointwiseError::NullReference)?
                .histogram()
                .ok_or(PointwiseError::InvalidCast),
            PointwiseParameterValue::Histogram(h) => Ok(h),
            _ => Err(PointwiseError::NullReference),
        }
    }

    /// Edit both light coordinates in place and record their prior values.
    pub fn edit_light_position(
        &self,
        expected: &PointwiseCustomObject<E>,
        before: [f64; 2],
        after: [f64; 2],
    ) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        if !after.into_iter().all(f64::is_finite) {
            return Err(ImageError::Argument.into());
        }
        if !self
            .custom_object()?
            .is_some_and(|current| current.same_reference(expected))
        {
            return Err(ImageError::Argument.into());
        }
        if !apply_light_position(expected, before, after)? {
            return Ok(());
        }
        if let Some(slot) = self
            .owner()
            .node()
            .texture_node_logic()
            .and_then(|logic| logic.slot())
            && let Some(history) = slot.history()
        {
            let sequence = history.edit_sequence();
            if let Err(error) = history.record_graph_edit(
                slot.history_target(),
                self.name(),
                Box::new(LightPositionEdit {
                    property: self.downgrade(),
                    object: expected.raw(),
                    before,
                    after,
                }),
            ) {
                if history.edit_sequence() == sequence {
                    let _ = apply_light_position(expected, after, before);
                }
                return Err(PointwiseError::History(Box::new(error)));
            }
        }
        Ok(())
    }
}

fn apply_light_position<E: 'static>(
    object: &PointwiseCustomObject<E>,
    before: [f64; 2],
    after: [f64; 2],
) -> Result<bool, PointwiseError<E>> {
    let changed = object.update_light_position(before, after)?;
    if changed && let Err(error) = object.notify() {
        if object.update_light_position(after, before).is_ok() {
            let _ = object.notify();
        }
        return Err(error);
    }
    Ok(changed)
}

struct LightPositionEdit<E> {
    property: WeakPointwiseParameter<E>,
    object: PointwiseCustomObject<E>,
    before: [f64; 2],
    after: [f64; 2],
}

impl<E: 'static> HistoryGraphEdit<E> for LightPositionEdit<E> {
    fn apply(&self, direction: HistoryDirection) -> Result<(), HistoryError<E>> {
        let property = self.property.upgrade().ok_or(HistoryError::NullTarget)?;
        let object = property
            .custom_object()
            .map_err(|error| HistoryError::Property(Box::new(error)))?
            .filter(|current| current.same_reference(&self.object))
            .ok_or(HistoryError::NullTarget)?;
        let (before, after) = match direction {
            HistoryDirection::Undo => (self.after, self.before),
            HistoryDirection::Redo => (self.before, self.after),
        };
        apply_light_position(&object, before, after)
            .map(|_| ())
            .map_err(|error| HistoryError::Property(Box::new(error)))
    }
}

struct CurveEdit<E> {
    property: WeakPointwiseParameter<E>,
    before: SharedCurve<E>,
    after: SharedCurve<E>,
}

struct GradientEdit<E> {
    property: WeakPointwiseParameter<E>,
    before: Option<SharedGradient>,
    after: Option<SharedGradient>,
}

impl<E: 'static> GradientEdit<E> {
    fn replace(
        &self,
        property: &PointwiseParameter<E>,
        direction: HistoryDirection,
    ) -> Result<(), HistoryError<E>> {
        let (before, after) = match direction {
            HistoryDirection::Undo => (&self.after, &self.before),
            HistoryDirection::Redo => (&self.before, &self.after),
        };
        if !property.replace_adjusted_gradient(before, after) {
            return Err(HistoryError::InputChanged);
        }
        if let Err(error) = property.notify_change(retained_spatial::ordinary()) {
            if property.replace_adjusted_gradient(after, before) {
                let _ = property.notify_change(retained_spatial::ordinary());
            }
            return Err(HistoryError::Property(Box::new(error)));
        }
        if !matches!(property.raw_value(), PointwiseParameterValue::Gradient(current)
            if super::reference_adjustment::same_gradient(&current, after))
        {
            return Err(HistoryError::InputChanged);
        }
        Ok(())
    }
}

impl<E: 'static> HistoryGraphEdit<E> for GradientEdit<E> {
    fn apply(&self, direction: HistoryDirection) -> Result<(), HistoryError<E>> {
        let property = self.property.upgrade().ok_or(HistoryError::NullTarget)?;
        self.replace(&property, direction)
    }
}

impl<E: 'static> HistoryGraphEdit<E> for CurveEdit<E> {
    fn apply(&self, direction: HistoryDirection) -> Result<(), HistoryError<E>> {
        let property = self.property.upgrade().ok_or(HistoryError::NullTarget)?;
        let (before, after) = match direction {
            HistoryDirection::Undo => (&self.after, &self.before),
            HistoryDirection::Redo => (&self.before, &self.after),
        };
        property
            .swap_curve(Some(after.clone()), Some(before))
            .map_err(|error| match error {
                PointwiseError::History(error) => *error,
                error => HistoryError::Property(Box::new(error)),
            })
    }
}

#[path = "capture.rs"]
pub(crate) mod capture;
pub use capture::{
    EnvironmentRenderOverride, FrozenRenderBatch, FrozenRenderValues, MaterializedRenderBatch,
    PendingFilterRender, PendingGradientRender, PendingNodeRender, PointwiseFinalExportCache,
    PointwiseNodeRenderSnapshot, PointwiseRenderSnapshot, RenderCaptureSession,
    RenderFreezeSession,
};

#[path = "retained_spatial.rs"]
mod retained_spatial;
#[path = "transform_parameters.rs"]
mod transform_parameters;
pub(super) use super::parameter_ops::normalize as normalize_parameter;
pub use retained_spatial::{
    ImportedImageContainer, ImportedImageDecodeError, ImportedImageReporting, PointwiseCloneMode,
    PointwiseCustomObject, PointwiseImportedImage, PointwiseMatrix, PointwiseMatrixHandler,
    PointwisePropertyChange, PointwisePropertyHandler, PointwiseRegion,
};
use retained_spatial::{ParameterExtras, matrix_error, matrix_parameters, pixelate_parameters};

pub(in crate::pointwise) use retained_spatial::matrix_object::MatrixBinding as CustomPropertyBinding;

#[cfg(test)]
#[path = "transform_test.rs"]
mod transform_tests;
