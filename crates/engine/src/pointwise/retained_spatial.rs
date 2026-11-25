//! Actual unbound os pair/custom-object state. Source constructors and event
//! bridges are qualified in spatial-filters-source/{retained-adapters,object-events}.
use super::*;
use crate::{AnimationError, GradientAnimation, MatrixKernel, MatrixKernelError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PointwiseCloneMode {
    Original,
    Clone,
}
pub type PointwisePropertyChange<E = ()> = crate::ContextChangeArgument<E>;
pub(super) fn ordinary<E>() -> Arc<PointwisePropertyChange<E>> {
    Arc::new(crate::ContextChangeArgument::Property(
        crate::ContextPropertyChangeArgs::with_undo(None),
    ))
}
fn speed<E>() -> Arc<PointwisePropertyChange<E>> {
    Arc::new(crate::ContextChangeArgument::Property(
        crate::ContextPropertyChangeArgs::with_speed(true),
    ))
}
type PropertyCallback<E> =
    dyn Fn(&PointwiseParameter<E>, Arc<PointwisePropertyChange<E>>) -> Result<(), E> + Send + Sync;
pub struct PointwisePropertyHandler<E>(Arc<PropertyCallback<E>>);
impl<E> Clone for PointwisePropertyHandler<E> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<E> fmt::Debug for PointwisePropertyHandler<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PointwisePropertyHandler({:p})", Arc::as_ptr(&self.0))
    }
}
impl<E> PointwisePropertyHandler<E> {
    pub fn new(
        f: impl Fn(&PointwiseParameter<E>, Arc<PointwisePropertyChange<E>>) -> Result<(), E>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self(Arc::new(f))
    }
    pub fn same_binding(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
/// Actual os m_x/m_y, m_v and m_w storage for inherited ok Size only.
#[derive(Clone)]
pub(super) struct SizeNormalization {
    pub(super) normalized: [f64; 2],
    pub(super) scale: f64,
    pub(super) flags: Option<Vec<bool>>,
}
pub(super) struct ParameterExtras<E> {
    // Ordinary os.d for source default-g non-numeric properties. Numeric/pair
    // limits stay in their value; option/custom factories deliberately use null.
    pub(super) ordinary_limits: Option<SharedLimits>,
    pub(super) size: Option<SizeNormalization>,
    pub(super) seed_modifier: f64,
    pub(super) wire: wire::LegacyWireMetadata<E>,
    pub(super) animation: Option<GradientAnimation<E>>,
    pub(super) allow_changes: bool,
    pub(super) handlers: Vec<PointwisePropertyHandler<E>>,
    pub(super) binding: Option<Arc<MatrixBinding<E>>>,
}
impl<E> Default for ParameterExtras<E> {
    fn default() -> Self {
        Self {
            ordinary_limits: None,
            size: None,
            seed_modifier: 0.,
            wire: wire::LegacyWireMetadata::new(true, true),
            animation: None,
            allow_changes: true,
            handlers: Vec::new(),
            binding: None,
        }
    }
}
impl<E> ParameterExtras<E> {
    pub(super) fn with_ordinary_limits() -> Self {
        Self {
            ordinary_limits: Some(SharedLimits::new(crate::LimitsValues::default())),
            ..Self::default()
        }
    }
}
impl<E> Clone for ParameterExtras<E> {
    fn clone(&self) -> Self {
        Self {
            ordinary_limits: self.ordinary_limits.clone(),
            size: self.size.clone(),
            seed_modifier: self.seed_modifier,
            wire: self.wire.clone(),
            animation: self.animation.clone(),
            allow_changes: self.allow_changes,
            handlers: self.handlers.clone(),
            binding: self.binding.clone(),
        }
    }
}
#[path = "matrix_object.rs"]
pub(super) mod matrix_object;
use matrix_object::MatrixBinding;
pub use matrix_object::{
    ImportedImageContainer, ImportedImageDecodeError, ImportedImageReporting,
    PointwiseCustomObject, PointwiseImportedImage, PointwiseMatrix, PointwiseMatrixHandler,
    PointwiseRegion,
};
pub(super) fn matrix_error<E>(e: MatrixKernelError) -> PointwiseError<E> {
    match e {
        MatrixKernelError::NullReference => PointwiseError::NullReference,
        MatrixKernelError::Index => PointwiseError::Index,
        MatrixKernelError::Allocation => ImageError::Allocation.into(),
        MatrixKernelError::Binary(_) => {
            PointwiseError::UnsupportedCapability("Matrix binary operation in retained render")
        }
    }
}
pub(super) use super::super::parameter_ops::normalize;
fn spatial_limits(
    places: i32,
    min: i32,
    max: i32,
    range: [i32; 2],
    curve: [i32; 2],
    range_places: i32,
) -> Result<SharedLimits, ImageError> {
    let mut use_range = RangeOptionsRecord::new(decimal(range[0])?, decimal(range[1])?);
    use_range.decimal_places = range_places;
    SharedLimits::from_definition(LimitsDefinition {
        decimal_places: places,
        step: LimitsStep::Derived(decimal(1)?),
        minimum: decimal(min)?,
        maximum: decimal(max)?,
        used: true,
        use_range: Some(SharedRangeOptionsRecord::new(use_range)),
        curve_range: Some(SharedCurveRangeRecord::new(CurveRangeRecord {
            minimum: decimal(curve[0])?,
            maximum: decimal(curve[1])?,
            wrap_y: false,
        })),
    })
    .map_err(|_| ImageError::Overflow)
}
pub(super) fn pixelate_parameters<E>() -> Result<Vec<ParameterState<E>>, PointwiseError<E>> {
    Ok(vec![
        ParameterState {
            extras: ParameterExtras::default(),
            choice_memory: None,
            name: text("Pixels"),
            value: PointwiseParameterValue::Pair {
                current: [10.0; 2],
                previous: [10.0; 2],
                limits: spatial_limits(0, 1, 1000000, [1, 50], [1, 100], 3)?,
            },
        },
        choice("Smoothing", &["None", "Linear", "Cosine", "Cubic"], 0),
    ])
}
pub(super) fn matrix_parameters<E>() -> Result<Vec<ParameterState<E>>, PointwiseError<E>> {
    let current = PointwiseMatrix::new()?;
    let previous = current.custom_object().source_clone()?;
    let iteration_limits = spatial_limits(0, 0, 10000, [1, 10], [1, 10], 0)?;
    Ok(vec![
        ParameterState {
            extras: ParameterExtras::default(),
            choice_memory: None,
            name: text("Matrix"),
            value: PointwiseParameterValue::Matrix {
                current: Some(current.custom_object().raw()),
                previous: Some(previous.raw()),
            },
        },
        ParameterState {
            extras: ParameterExtras::default(),
            choice_memory: None,
            name: text("Iterations"),
            value: PointwiseParameterValue::Number {
                current: 1.0,
                previous: 1.0,
                limits: iteration_limits,
            },
        },
    ])
}
pub(super) fn wizard_matrix_parameters<E>() -> Result<Vec<ParameterState<E>>, PointwiseError<E>> {
    let mut values = matrix_parameters()?;
    let mut range = RangeOptionsRecord::new(decimal(1)?, decimal(10)?);
    range.decimal_places = 0;
    let iterations = SharedLimits::from_definition(LimitsDefinition {
        decimal_places: 0,
        step: LimitsStep::Derived(decimal(1)?),
        minimum: decimal(0)?,
        maximum: decimal(10000)?,
        used: true,
        use_range: Some(SharedRangeOptionsRecord::new(range)),
        curve_range: None,
    })
    .map_err(|_| ImageError::Overflow)?;
    if let PointwiseParameterValue::Number { limits, .. } = &mut values[1].value {
        *limits = iterations;
    }
    let decimal_fraction = |coefficient: u32, scale: u32| {
        DecimalBits::new([coefficient, 0, 0, scale << 16]).map_err(|_| ImageError::Argument)
    };
    let radius_limits = SharedLimits::from_definition(LimitsDefinition {
        decimal_places: 3,
        step: LimitsStep::Derived(decimal_fraction(1, 2)?),
        minimum: decimal(0)?,
        maximum: decimal_fraction(5, 1)?,
        used: true,
        use_range: None,
        curve_range: None,
    })
    .map_err(|_| ImageError::Overflow)?;
    values.push(ParameterState {
        extras: ParameterExtras::default(),
        choice_memory: None,
        name: text("Radius"),
        value: PointwiseParameterValue::Number {
            current: 0.02,
            previous: 0.02,
            limits: radius_limits,
        },
    });
    values.push(number("Angle", 1, 0, 360, false, false)?);
    Ok(values)
}
pub(super) fn smear_parameters<E>(
    mode: PointwiseCloneMode,
) -> Result<Vec<ParameterState<E>>, PointwiseError<E>> {
    // ana -> ok -> os setters: the constructor-owned mode gates normalization
    // before any normalized field or twenty-element flags publication. Raw/prior
    // still commit6/6 in both modes; no source-copy operation runs here.
    let size = ParameterExtras {
        size: Some(SizeNormalization {
            normalized: if mode == PointwiseCloneMode::Original {
                [6.; 2]
            } else {
                [0.; 2]
            },
            scale: 1.,
            flags: if mode == PointwiseCloneMode::Original {
                Some(vec![false; 20])
            } else {
                None
            },
        }),
        ..ParameterExtras::default()
    };
    Ok(vec![
        ParameterState {
            extras: size,
            choice_memory: None,
            name: text("Size"),
            value: PointwiseParameterValue::Pair {
                current: [6.; 2],
                previous: [6.; 2],
                limits: spatial_limits(3, -1000000, 1000000, [-10, 10], [0, 10], 0)?,
            },
        },
        ParameterState {
            extras: ParameterExtras::default(),
            choice_memory: None,
            name: text("Samples"),
            value: PointwiseParameterValue::Number {
                current: 20.,
                previous: 20.,
                limits: spatial_limits(0, 1, 100, [1, 100], [1, 30], 3)?,
            },
        },
        choice("Anti-Alias", &["Yes", "No"], 0),
        choice("Seamless", &["Yes", "No"], 0),
    ])
}
impl<E> PointwiseFilter<E> {
    pub fn with_clone_mode(
        kind: PointwiseKind,
        mode: PointwiseCloneMode,
    ) -> Result<Self, PointwiseError<E>> {
        Self::new_in_mode(kind, mode)
    }
    pub fn clone_mode(&self) -> PointwiseCloneMode {
        guard(&self.core.filters)[self.ix].clone_mode
    }

    pub fn factor(&self) -> f64 {
        guard(&self.core.filters)[self.ix].factor
    }
    /// Local zl.s6 effect only. Attached child/quality traversal is a distinct gap.
    pub fn set_local_factor(&self, value: f64) {
        guard(&self.core.filters)[self.ix].factor = value;
    }
    pub fn commit_spatial_scale(&self, scale: f64) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        match self.kind() {
            // These concrete types inherit the empty zl.gb body.
            PointwiseKind::Polygon
            | PointwiseKind::ConvertEnvironment
            | PointwiseKind::MakeEnvironment
            | PointwiseKind::RotateEnvironment
            | PointwiseKind::Frequency
            | PointwiseKind::Fractalize
            | PointwiseKind::Exposure
            | PointwiseKind::Selector
            | PointwiseKind::MatchColor
            | PointwiseKind::ArtBlur
            | PointwiseKind::Style => Ok(()),
            PointwiseKind::Synthesis => {
                let property = self.parameter(0)?;
                let current = property.number();
                let adjusted = if current < 0. {
                    1. / (1. - current)
                } else {
                    current + 1.
                } * scale;
                property.set_number(if adjusted < 1. {
                    1. - 1. / adjusted
                } else {
                    adjusted - 1.
                })
            }
            PointwiseKind::Canvas => {
                let size = self.parameter(3)?;
                let extents = [size.extent(0)? * scale, size.extent(1)? * scale];
                for (axis, value) in super::super::polygon::round_scaled_size(extents, scale)?
                    .into_iter()
                    .enumerate()
                {
                    size.set_extent(axis, value)?;
                }
                Ok(())
            }
            PointwiseKind::ChangeBci
            | PointwiseKind::ChangeHsb
            | PointwiseKind::Colorize
            | PointwiseKind::ColorizeG2
            | PointwiseKind::Levels
            | PointwiseKind::GetChannel
            | PointwiseKind::Add
            | PointwiseKind::Multiply
            | PointwiseKind::FromHsb
            | PointwiseKind::OklabColor
            | PointwiseKind::Checkerboard
            | PointwiseKind::Shader
            | PointwiseKind::ClearMaps
            | PointwiseKind::StoreMap
            | PointwiseKind::MergeMaps
            | PointwiseKind::FixMapContrast
            | PointwiseKind::LayerMaps
            | PointwiseKind::ToneCurve
            | PointwiseKind::ToneCurveG2
            | PointwiseKind::Combine
            | PointwiseKind::WizardColor
            | PointwiseKind::WizardFiveColor
            | PointwiseKind::WizardSlideColor
            | PointwiseKind::WizardToneCurve
            | PointwiseKind::Rotate
            | PointwiseKind::MedianBlur
            | PointwiseKind::MotionBlur
            | PointwiseKind::SmartBlur
            | PointwiseKind::Smear
            | PointwiseKind::VariableBlur
            | PointwiseKind::Regions
            | PointwiseKind::WizardGroup
            | PointwiseKind::Group
            | PointwiseKind::TemplateLab(_)
            | PointwiseKind::WizardReference
            | PointwiseKind::Reference
            | PointwiseKind::MapVariable => Ok(()),
            PointwiseKind::Distort
            | PointwiseKind::FreeTransform
            | PointwiseKind::Kaleidoscope
            | PointwiseKind::WizardKaleidoscope
            | PointwiseKind::RotateTile
            | PointwiseKind::Remap => Ok(()),
            PointwiseKind::MoveRepeat => {
                let p = self.parameter(1)?;
                let mut xy = [p.extent(0)?, p.extent(1)?];
                if super::super::transform::no_scale_move(self)? {
                    xy[0] /= scale;
                    xy[1] /= scale;
                }
                if !p
                    .animation()?
                    .flags()
                    .ok_or(AnimationError::NullReference)?
                    .value(0)
                    .map_err(|_| AnimationError::<E>::Index)?
                {
                    p.set_axis(0, xy[0])?;
                }
                if !p
                    .animation()?
                    .flags()
                    .ok_or(AnimationError::NullReference)?
                    .value(1)
                    .map_err(|_| AnimationError::<E>::Index)?
                {
                    p.set_axis(1, xy[1])?;
                }
                Ok(())
            }
            PointwiseKind::FullLight => super::super::full_light::commit_scale(self, scale),
            PointwiseKind::Hair => super::super::hair::commit_scale(self, scale),
            PointwiseKind::LegacyBricksBoards => {
                super::super::legacy_bricks::commit_scale(self, scale)
            }
            PointwiseKind::Caustics | PointwiseKind::Transmit => {
                super::super::caustics::commit_scale(self, scale)
            }
            PointwiseKind::Stereogram => super::super::stereogram::commit_scale(self, scale),
            PointwiseKind::Reflect | PointwiseKind::Refract => {
                super::super::planar_optics::commit_scale(self, scale)
            }
            PointwiseKind::ReflectEnvironment | PointwiseKind::RefractEnvironment => {
                super::super::environment_optics::commit_scale(self, scale)
            }
            PointwiseKind::DiffuseEnvironment | PointwiseKind::ScatterEnvironment => {
                super::super::environment_blur::commit_scale(self, scale)
            }
            PointwiseKind::PointLight => self.commit_point_light_scale(scale),
            PointwiseKind::Shadow => super::super::shadow::commit_scale(self, scale),
            PointwiseKind::ShadowG2 | PointwiseKind::IlluminateG2 => {
                super::super::shadow::commit_height_scale(self, scale)
            }
            PointwiseKind::Atmosphere => {
                let detail = self.parameter(4)?;
                if super::super::parameter_ops::is_animated(|| detail.retained_animation())? {
                    super::super::point_light::scale_point_light_curve(&detail, 1.0 / scale)?;
                } else {
                    detail.set_number(detail.number() * scale)?;
                }
                Ok(())
            }
            PointwiseKind::Pixelate => {
                let p = self.parameter(0)?;
                p.multiply_axis(0, scale)?;
                p.multiply_axis(1, scale)?;
                Ok(())
            }
            PointwiseKind::WizardMatrix => {
                let radius = self.parameter(2)?;
                if super::super::parameter_ops::is_animated(|| radius.retained_animation())? {
                    super::super::point_light::scale_point_light_curve(&radius, scale)?;
                } else {
                    radius.set_number(radius.number() * (1.0 / scale))?;
                }
                Ok(())
            }
            PointwiseKind::Matrix => {
                let lhs = self
                    .parameter(0)?
                    .matrix()?
                    .ok_or(PointwiseError::NullReference)?;
                let rhs = self
                    .parameter(0)?
                    .matrix()?
                    .ok_or(PointwiseError::NullReference)?
                    .payload()
                    .radius()
                    / scale;
                lhs.payload().set_radius(rhs);
                Ok(())
            }
            PointwiseKind::Equalize
            | PointwiseKind::GaussianBlur
            | PointwiseKind::Sharpen
            | PointwiseKind::ShapeBlur => self.scale_blur_radius(scale),
        }
    }
    pub(super) fn bind_matrix_objects(&self) {
        for p in self.parameters() {
            let current = match p.raw_value() {
                PointwiseParameterValue::Matrix { current, .. }
                | PointwiseParameterValue::Region { current, .. } => current,
                _ => None,
            };
            if let Some(current) = current {
                let b = PointwiseCustomObject::bind(&p);
                current.initialize_region_owner(&b);
                current.subscribe(b.clone());
                guard(&self.core.filters)[self.ix].parameters[p.ix]
                    .extras
                    .binding = Some(b);
            }
        }
    }
}
impl<E> PointwiseParameter<E> {
    pub fn is_paired(&self) -> bool {
        matches!(self.value(), PointwiseParameterValue::Pair { .. })
    }
    pub fn limits(&self) -> Result<SharedLimits, PointwiseError<E>> {
        match self.value() {
            PointwiseParameterValue::Pair { limits, .. }
            | PointwiseParameterValue::Number { limits, .. } => Ok(limits),
            _ => self.retained_limits().ok_or(PointwiseError::NullReference),
        }
    }
    pub fn pair(&self) -> Result<[f64; 2], PointwiseError<E>> {
        match self.value() {
            PointwiseParameterValue::Pair { current, .. } => Ok(current),
            _ => Err(PointwiseError::NullReference),
        }
    }
    pub fn previous_pair(&self) -> Result<[f64; 2], PointwiseError<E>> {
        match self.value() {
            PointwiseParameterValue::Pair { previous, .. } => Ok(previous),
            _ => Err(PointwiseError::NullReference),
        }
    }
    pub fn previous_number(&self) -> f64 {
        match self.value() {
            PointwiseParameterValue::Number { previous, .. } => previous,
            _ => 0.0,
        }
    }
    pub fn set_pair(&self, values: [f64; 2]) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        let limits = self.limits()?;
        let values = [
            normalize(&limits, values[0])?,
            normalize(&limits, values[1])?,
        ];
        let old = {
            let mut entries = guard(&self.owner.core.filters);
            match &mut entries[self.owner.ix].parameters[self.ix].value {
                PointwiseParameterValue::Pair {
                    current, previous, ..
                } => {
                    let old = std::array::from_fn(|axis| {
                        if previous[axis] != current[axis] {
                            previous[axis]
                        } else {
                            current[axis]
                        }
                    });
                    super::super::parameter_ops::commit_pair(current, previous, values)
                        .then_some(old)
                }
                _ => return Err(PointwiseError::NullReference),
            }
        };
        if let Some(old) = old {
            self.normalize_size()?;
            let undo = if self.can_create_property_undo()? {
                Some(Arc::new(crate::PairUndoInfo::new(
                    Some(self.name()),
                    crate::PairUndoArray::new(old),
                    crate::PairUndoArray::new(self.pair()?),
                )))
            } else {
                None
            };
            self.notify_change(Arc::new(crate::ContextChangeArgument::Property(
                crate::ContextPropertyChangeArgs::with_pair_undo(undo),
            )))?;
        }
        Ok(())
    }
    /// Actual os.e(Double,Double) speed route: retain old committed values.
    pub fn set_pair_speed(&self, values: [f64; 2]) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        let limits = self.limits()?;
        let values = [
            normalize(&limits, values[0])?,
            normalize(&limits, values[1])?,
        ];
        let changed = {
            let mut entries = guard(&self.owner.core.filters);
            match &mut entries[self.owner.ix].parameters[self.ix].value {
                PointwiseParameterValue::Pair {
                    current, previous, ..
                } => {
                    if current[0] == values[0] && current[1] == values[1] {
                        false
                    } else {
                        *previous = *current;
                        *current = values;
                        true
                    }
                }
                _ => return Err(PointwiseError::NullReference),
            }
        };
        if changed {
            self.normalize_size()?;
            self.notify_change(speed())?;
        }
        Ok(())
    }
    pub fn set_axis(&self, axis: usize, value: f64) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        if axis >= 2 {
            return Err(PointwiseError::Index);
        }
        let value = normalize(&self.limits()?, value)?;
        let old = {
            let mut entries = guard(&self.owner.core.filters);
            match &mut entries[self.owner.ix].parameters[self.ix].value {
                PointwiseParameterValue::Pair {
                    current, previous, ..
                } => {
                    let old = if previous[axis] != current[axis] {
                        previous[axis]
                    } else {
                        current[axis]
                    };
                    super::super::parameter_ops::commit_axis(current, previous, axis, value)
                        .then_some(old)
                }
                _ => return Err(PointwiseError::NullReference),
            }
        };
        if let Some(old) = old {
            self.normalize_size()?;
            let undo = if self.can_create_property_undo()? {
                Some(Arc::new(crate::DoubleUndoInfo::new(
                    Some(self.name()),
                    axis as i32 + 1,
                    old,
                    self.pair()?[axis],
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
    pub fn retained_limits(&self) -> Option<SharedLimits> {
        match self.value() {
            PointwiseParameterValue::Number { limits, .. }
            | PointwiseParameterValue::Pair { limits, .. } => Some(limits),
            _ => guard(&self.owner.core.filters)[self.owner.ix].parameters[self.ix]
                .extras
                .ordinary_limits
                .clone(),
        }
    }
    pub(in crate::pointwise::property) fn animation_limits(
        &self,
    ) -> Result<Option<SharedLimits>, PointwiseError<E>> {
        if matches!(self.raw_value(), PointwiseParameterValue::Color(_)) {
            let step = DecimalBits::new([1, 0, 0, 1 << 16]).map_err(|_| ImageError::Argument)?;
            return Ok(Some(
                SharedLimits::from_definition(LimitsDefinition {
                    decimal_places: 1,
                    step: LimitsStep::Derived(step),
                    minimum: decimal(0)?,
                    maximum: decimal(100)?,
                    used: true,
                    use_range: None,
                    curve_range: None,
                })
                .map_err(|_| ImageError::Overflow)?,
            ));
        }
        if let PointwiseParameterValue::Choice { options, .. } = self.value() {
            let half = DecimalBits::new([5, 0, 0, 1 << 16]).map_err(|_| ImageError::Argument)?;
            let len = u32::try_from(options.entries().len()).map_err(|_| ImageError::Overflow)?;
            let maximum = DecimalBits::new([
                len.checked_mul(10)
                    .and_then(|v| v.checked_add(5))
                    .ok_or(ImageError::Overflow)?,
                0,
                0,
                1 << 16,
            ])
            .map_err(|_| ImageError::Argument)?;
            let inc = DecimalBits::new([1, 0, 0, 1 << 16]).map_err(|_| ImageError::Argument)?;
            return Ok(Some(
                SharedLimits::from_definition(LimitsDefinition {
                    decimal_places: 1,
                    step: LimitsStep::Derived(inc),
                    minimum: half,
                    maximum,
                    used: true,
                    use_range: None,
                    curve_range: None,
                })
                .map_err(|_| ImageError::Overflow)?,
            ));
        }
        Ok(self.retained_limits())
    }
    pub fn effective_index_at(&self, time: f64) -> Result<i32, PointwiseError<E>>
    where
        E: 'static,
    {
        self.ensure_property_evaluation_supported()?;
        let options = self.options()?;
        if super::super::parameter_ops::is_animated(|| self.retained_animation())? {
            super::super::parameter_ops::animated_choice_index(
                self.animation_curve(0)?,
                options.entries().len(),
                time,
            )
        } else if let Some(value) = self.animated_binding_value(time)? {
            let entries = options.entries();
            if let Some(index) = entries
                .iter()
                .position(|entry| entry.as_deref() == value.selection())
            {
                Ok(index as i32)
            } else {
                Ok(crate::framework_cast::integer(value.number()))
            }
        } else {
            Ok(self.index())
        }
    }
    pub fn retained_animation(&self) -> Result<Option<GradientAnimation<E>>, PointwiseError<E>> {
        Ok(
            guard(&self.owner.core.filters)[self.owner.ix].parameters[self.ix]
                .extras
                .animation
                .clone(),
        )
    }
    pub fn animation(&self) -> Result<GradientAnimation<E>, PointwiseError<E>>
    where
        E: 'static,
    {
        if let Some(a) = self.retained_animation()? {
            return Ok(a);
        }
        let a = self.detached_animation()?;
        guard(&self.owner.core.filters)[self.owner.ix].parameters[self.ix]
            .extras
            .animation = Some(a.clone());
        Ok(a)
    }
    /// Build the default animation without changing the stored property.
    pub fn detached_animation(&self) -> Result<GradientAnimation<E>, PointwiseError<E>>
    where
        E: 'static,
    {
        Ok(GradientAnimation::new(
            if self.is_paired() { 2 } else { 1 },
            self.animation_limits()?,
        )?)
    }
    pub fn set_animation(
        &self,
        value: Option<GradientAnimation<E>>,
    ) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        let old = std::mem::replace(
            &mut guard(&self.owner.core.filters)[self.owner.ix].parameters[self.ix]
                .extras
                .animation,
            value.clone(),
        );
        let args = if self.can_create_property_undo()? {
            let undo = crate::AnimationUndoInfo::replacement(Some(self.name()), old, value);
            Arc::new(crate::ContextChangeArgument::Property(
                crate::ContextPropertyChangeArgs::with_animation_undo(Arc::new(undo)),
            ))
        } else {
            ordinary()
        };
        self.notify_change(args)
    }
    pub fn axis_is_animated(&self, axis: usize) -> Result<bool, PointwiseError<E>>
    where
        E: 'static,
    {
        super::super::parameter_ops::axis_active(|| self.retained_animation(), axis)
    }
    pub fn set_axis_animated(&self, axis: usize, active: bool) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        let a = self.animation()?;
        let flags = a.flags().ok_or(AnimationError::NullReference)?;
        if flags
            .value(axis as i32)
            .map_err(|_| AnimationError::<E>::Index)?
            != active
        {
            if self.wizard_variable_index().is_some() {
                let replacement = a.source_clone()?;
                if active {
                    replacement.ensure_curve(axis as i32)?;
                }
                replacement
                    .flags()
                    .ok_or(AnimationError::NullReference)?
                    .set(axis as i32, active)
                    .map_err(|_| AnimationError::<E>::Index)?;
                return self.set_animation(Some(replacement));
            }
            if active {
                a.ensure_curve(axis as i32)?;
            }
            flags
                .set(axis as i32, active)
                .map_err(|_| AnimationError::<E>::Index)?;
            self.notify_change(ordinary())?;
        }
        Ok(())
    }
    pub fn animation_curve(&self, axis: usize) -> Result<SharedCurve<E>, PointwiseError<E>>
    where
        E: 'static,
    {
        super::super::parameter_ops::curve(|| self.retained_animation(), axis)
    }
    pub fn raw_value_at(&self, axis: usize, time: f64) -> Result<f64, PointwiseError<E>>
    where
        E: 'static,
    {
        self.ensure_property_evaluation_supported()?;
        if axis >= 2 {
            return Err(PointwiseError::Index);
        }
        if !self.axis_is_animated(axis)?
            && let Some(value) = self.animated_binding_value(time)?
            && self.binding_applies_to_axis(axis)?
        {
            return Ok(value.number());
        }
        super::super::parameter_ops::pair_value(
            || self.retained_animation(),
            || self.limits(),
            || self.pair(),
            axis,
            time,
        )
    }
    /// os.r/p: active samples are raw u/x; inactive inherited Size uses m_x/m_y.
    /// Pixelate's zl Pixels pair has no Size normalization and stays raw.
    pub fn value_at(&self, axis: usize, time: f64) -> Result<f64, PointwiseError<E>>
    where
        E: 'static,
    {
        if axis >= 2 {
            return Err(PointwiseError::Index);
        }
        let size = guard(&self.owner.core.filters)[self.owner.ix].parameters[self.ix]
            .extras
            .size
            .clone();
        if let Some(size) = size
            && !self.axis_is_animated(axis)?
        {
            return Ok(size.normalized[axis]);
        }
        self.raw_value_at(axis, time)
    }
    /// Actual os.a2/az committed normalization fields, without time evaluation.
    pub fn normalized_pair(&self) -> Result<[f64; 2], PointwiseError<E>> {
        let entries = guard(&self.owner.core.filters);
        Ok(entries[self.owner.ix].parameters[self.ix]
            .extras
            .size
            .as_ref()
            .ok_or(PointwiseError::NullReference)?
            .normalized)
    }
    /// Actual os scale-field getter. This is property state, not node Factor.
    pub fn size_scale(&self) -> Result<f64, PointwiseError<E>> {
        Ok(
            guard(&self.owner.core.filters)[self.owner.ix].parameters[self.ix]
                .extras
                .size
                .as_ref()
                .ok_or(PointwiseError::NullReference)?
                .scale,
        )
    }
    /// Unbound os.a(Double,Boolean[]) Size route. Changed scale commits before
    /// the caller flags are cloned, and normalization may partially publish X.
    /// No node-scale/child traversal or Factor commit is implied.
    pub fn set_size_scale(
        &self,
        scale: f64,
        flags: Option<&[bool]>,
    ) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        {
            let mut entries = guard(&self.owner.core.filters);
            let size = entries[self.owner.ix].parameters[self.ix]
                .extras
                .size
                .as_mut()
                .ok_or(PointwiseError::NullReference)?;
            if scale == size.scale {
                return Ok(());
            }
            size.scale = scale;
        }
        let flags = flags.ok_or(PointwiseError::NullReference)?.to_vec();
        guard(&self.owner.core.filters)[self.owner.ix].parameters[self.ix]
            .extras
            .size
            .as_mut()
            .ok_or(PointwiseError::NullReference)?
            .flags = Some(flags);
        self.normalize_size()?;
        self.notify_change(crate::ContextChangeArgument::shared_empty())
    }
    /// Actual os.bm/bj: inactive raw extent; active live curve is re-fetched twice.
    pub fn extent(&self, axis: usize) -> Result<f64, PointwiseError<E>>
    where
        E: 'static,
    {
        if axis >= 2 {
            return Err(PointwiseError::Index);
        }
        if !self.axis_is_animated(axis)? {
            return Ok(self.pair()?[axis]);
        }
        let minimum = self.animation_curve(axis)?.minimum().abs();
        let maximum = self.animation_curve(axis)?.maximum().abs();
        Ok(f64::from(super::super::max32(minimum, maximum)))
    }
    pub(super) fn normalize_size(&self) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        let size = guard(&self.owner.core.filters)[self.owner.ix].parameters[self.ix]
            .extras
            .size
            .clone();
        let Some(size) = size else {
            return Ok(());
        };
        // os.i: constructor owner Original OR actual animation; exposed Parent is
        // not substituted for that owner. Attached ancestor adjustment remains separate.
        if self.owner.clone_mode() != PointwiseCloneMode::Original
            && !super::super::parameter_ops::is_animated(|| self.retained_animation())?
        {
            return Ok(());
        }
        {
            let mut entries = guard(&self.owner.core.filters);
            let state = entries[self.owner.ix].parameters[self.ix]
                .extras
                .size
                .as_mut()
                .ok_or(PointwiseError::NullReference)?;
            if state.flags.is_none() {
                state.flags = Some(vec![false; 20]);
            }
        }
        let kind = self.owner.kind();
        let round_polygon = if kind == PointwiseKind::Polygon {
            super::super::polygon::rounds_scaled_size(&self.owner)?
        } else {
            false
        };
        let repeated_transform = super::super::transform::uses_repeat_scale(kind);
        let mut scale = size.scale;
        let legacy_bricks = kind == PointwiseKind::LegacyBricksBoards;
        let mut round =
            round_polygon || repeated_transform || legacy_bricks || kind == PointwiseKind::Canvas;
        if legacy_bricks && super::super::legacy_bricks::preserves_size(self, scale)? {
            scale = 1.;
            round = false;
        }
        if repeated_transform
            && super::super::transform::retains_repeat_size(
                &self.owner,
                size.flags.as_deref().unwrap_or(&[]),
            )?
        {
            scale = 1.;
            round = false;
        }
        let x = self.extent(0)? * scale;
        guard(&self.owner.core.filters)[self.owner.ix].parameters[self.ix]
            .extras
            .size
            .as_mut()
            .ok_or(PointwiseError::NullReference)?
            .normalized[0] = x;
        let y = self.extent(1)? * scale;
        guard(&self.owner.core.filters)[self.owner.ix].parameters[self.ix]
            .extras
            .size
            .as_mut()
            .ok_or(PointwiseError::NullReference)?
            .normalized[1] = y;
        if round && scale != 1.0 {
            let rounded = super::super::polygon::round_scaled_size([x, y], scale)?;
            let mut entries = guard(&self.owner.core.filters);
            let normalized = &mut entries[self.owner.ix].parameters[self.ix]
                .extras
                .size
                .as_mut()
                .ok_or(PointwiseError::NullReference)?
                .normalized;
            *normalized = rounded;
        }
        Ok(())
    }
    pub fn effective_value_at(&self, time: f64) -> Result<f64, PointwiseError<E>>
    where
        E: 'static,
    {
        self.ensure_property_evaluation_supported()?;
        if let Some(value) = self.animated_binding_value(time)? {
            return Ok(value.number());
        }
        super::super::parameter_ops::scalar_value(
            || self.retained_animation(),
            || self.limits(),
            || Ok(self.number()),
            time,
        )
    }
    pub(in crate::pointwise) fn seed_modifier(&self) -> f64 {
        guard(&self.owner.core.filters)[self.owner.ix].parameters[self.ix]
            .extras
            .seed_modifier
    }

    pub(in crate::pointwise) fn set_seed_modifier(
        &self,
        modifier: f64,
    ) -> Result<bool, PointwiseError<E>>
    where
        E: 'static,
    {
        {
            let mut entries = guard(&self.owner.core.filters);
            let extras = &mut entries[self.owner.ix].parameters[self.ix].extras;
            if extras.seed_modifier == modifier {
                return Ok(false);
            }
            extras.seed_modifier = modifier;
        }
        self.notify_change(crate::ContextChangeArgument::shared_empty())?;
        Ok(true)
    }

    pub(in crate::pointwise) fn effective_seed_at(
        &self,
        time: f64,
    ) -> Result<f64, PointwiseError<E>>
    where
        E: 'static,
    {
        self.ensure_property_evaluation_supported()?;
        if super::super::parameter_ops::is_animated(|| self.retained_animation())? {
            return self.effective_value_at(time);
        }
        let entries = guard(&self.owner.core.filters);
        let parameter = &entries[self.owner.ix].parameters[self.ix];
        let PointwiseParameterValue::Number {
            current, limits, ..
        } = &parameter.value
        else {
            return Err(PointwiseError::InvalidCast);
        };
        Ok(super::super::parameter_ops::seed_modifier(
            *current,
            parameter.extras.seed_modifier,
            limits.snapshot().minimum.to_framework_f64() < 0.,
        ))
    }
    /// Actual os.t/v: animated extent divides by the current extent, otherwise raw setter.
    pub fn set_extent(&self, axis: usize, desired: f64) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        if self.axis_is_animated(axis)? {
            self.multiply_axis(axis, desired / self.extent(axis)?)
        } else {
            self.set_axis(axis, desired)
        }
    }
    pub fn multiply_axis(&self, axis: usize, factor: f64) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        if axis >= 2 {
            return Err(PointwiseError::Index);
        }
        if self.axis_is_animated(axis)? {
            let curve = self.animation_curve(axis)?;
            let minimum = self.limits()?.snapshot().minimum.to_framework_f32();
            let maximum = self.limits()?.snapshot().maximum.to_framework_f32();
            let increment = self.limits()?.snapshot().increment.to_framework_f32();
            curve
                .scale_bounds(factor as f32, minimum, maximum, increment)
                .map_err(AnimationError::Bounds)?;
            Ok(())
        } else {
            self.set_axis(axis, self.pair()?[axis] * factor)
        }
    }
    pub fn allow_changes(&self) -> bool {
        guard(&self.owner.core.filters)[self.owner.ix].parameters[self.ix]
            .extras
            .allow_changes
    }
    pub fn set_allow_changes(&self, value: bool) {
        guard(&self.owner.core.filters)[self.owner.ix].parameters[self.ix]
            .extras
            .allow_changes = value;
    }
    pub fn add_change_handler(&self, h: PointwisePropertyHandler<E>) {
        guard(&self.owner.core.filters)[self.owner.ix].parameters[self.ix]
            .extras
            .handlers
            .push(h);
    }
    pub fn remove_change_handler(&self, h: &PointwisePropertyHandler<E>) {
        let old = {
            let mut entries = guard(&self.owner.core.filters);
            let v = &mut entries[self.owner.ix].parameters[self.ix].extras.handlers;
            v.iter()
                .rposition(|x| x.same_binding(h))
                .map(|i| v.remove(i))
        };
        drop(old);
    }
    pub(in crate::pointwise) fn notify_change(
        &self,
        mut args: Arc<PointwisePropertyChange<E>>,
    ) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        if !matches!(args.as_ref(),crate::ContextChangeArgument::Property(a)if a.speed_rendering())
            && self.owner.clone_mode() != PointwiseCloneMode::Original
        {
            return Ok(());
        }
        let (allow, handlers) = {
            let entries = guard(&self.owner.core.filters);
            let e = &entries[self.owner.ix].parameters[self.ix].extras;
            (e.allow_changes, e.handlers.clone())
        };
        if !allow {
            args = Arc::new(crate::ContextChangeArgument::Property(
                crate::ContextPropertyChangeArgs::default(),
            ));
        }
        if !matches!(args.as_ref(),crate::ContextChangeArgument::Property(a)if a.update_controls_only())
        {
            let node = self.owner.node();
            if (matches!(self.owner.kind(), PointwiseKind::TemplateLab(_))
                && matches!(self.ix, 3 | 4)
                || self.owner.kind() == PointwiseKind::WizardReference && self.ix >= 4)
                && !matches!(args.as_ref(), crate::ContextChangeArgument::Property(change) if change.speed_rendering())
            {
                if matches!(self.owner.kind(), PointwiseKind::TemplateLab(_)) {
                    super::super::template_lab::refresh_control(&self.owner, self.ix as i32)?;
                }
                if let crate::ContextChangeArgument::Property(change) = args.as_ref()
                    && let Some(undo) = change.typed_undo_info()
                {
                    undo.mark_lab_undo();
                }
                node.notify_property_change_with(args.clone())?;
            } else if let Some(index) = self.owner.wizard_control_variable_index(self)
                && !matches!(args.as_ref(), crate::ContextChangeArgument::Property(change) if change.speed_rendering())
            {
                node.notify_wizard_control_change(&self.owner, index, args.clone())?;
            } else if self.ix == 0
                && matches!(
                    self.owner.kind(),
                    PointwiseKind::Group | PointwiseKind::MapVariable | PointwiseKind::WizardGroup
                )
                && let crate::ContextChangeArgument::Property(payload) = args.as_ref()
                && let Some(undo) = payload.undo_info()
                && undo.property_change() == 0
            {
                node.notify_group_input_count_change(
                    undo.old_value(),
                    undo.new_value(),
                    args.clone(),
                )?;
            } else {
                node.notify_property_change_with(args.clone())?;
            }
        }
        for h in &handlers {
            (h.0)(self, args.clone()).map_err(PointwiseError::Observer)?;
        }
        Ok(())
    }
    pub fn custom_object(&self) -> Result<Option<PointwiseCustomObject<E>>, PointwiseError<E>>
    where
        E: 'static,
    {
        match self.raw_value() {
            PointwiseParameterValue::Matrix { current, .. }
            | PointwiseParameterValue::Region { current, .. } => Ok(current.map(|o| o.external())),
            _ => Err(PointwiseError::NullReference),
        }
    }
    pub fn previous_custom_object(
        &self,
    ) -> Result<Option<PointwiseCustomObject<E>>, PointwiseError<E>>
    where
        E: 'static,
    {
        match self.raw_value() {
            PointwiseParameterValue::Matrix { previous, .. }
            | PointwiseParameterValue::Region { previous, .. } => {
                Ok(previous.map(|o| o.external()))
            }
            _ => Err(PointwiseError::NullReference),
        }
    }
    pub fn matrix(&self) -> Result<Option<PointwiseMatrix<E>>, PointwiseError<E>>
    where
        E: 'static,
    {
        self.custom_object()?
            .as_ref()
            .map(PointwiseCustomObject::matrix)
            .transpose()
    }
    pub fn previous_matrix(&self) -> Result<Option<PointwiseMatrix<E>>, PointwiseError<E>>
    where
        E: 'static,
    {
        self.previous_custom_object()?
            .as_ref()
            .map(PointwiseCustomObject::matrix)
            .transpose()
    }
    pub fn set_matrix(&self, value: Option<&PointwiseMatrix<E>>) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        let object = value.map(PointwiseMatrix::custom_object);
        self.set_custom_object(object.as_ref())
    }
    pub fn set_custom_object(
        &self,
        value: Option<&PointwiseCustomObject<E>>,
    ) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        let (old, binding) = {
            let mut entries = guard(&self.owner.core.filters);
            let p = &mut entries[self.owner.ix].parameters[self.ix];
            let current = match &p.value {
                PointwiseParameterValue::Matrix { current, .. }
                | PointwiseParameterValue::Region { current, .. } => current.clone(),
                _ => return Err(PointwiseError::NullReference),
            };
            if match (&current, value) {
                (None, None) => true,
                (Some(a), Some(b)) => a.same_reference(b),
                _ => false,
            } {
                return Ok(());
            }
            let binding = p.extras.binding.take();
            (current, binding)
        };
        if let (Some(old), Some(b)) = (&old, &binding) {
            old.unsubscribe(b);
        }
        {
            let mut entries = guard(&self.owner.core.filters);
            if let PointwiseParameterValue::Matrix { current, .. }
            | PointwiseParameterValue::Region { current, .. } =
                &mut entries[self.owner.ix].parameters[self.ix].value
            {
                *current = value.map(PointwiseCustomObject::raw);
            }
        }
        if let Some(value) = value {
            for owner in value.owner_nodes() {
                self.owner.node().merge_region(&owner);
            }
            let binding = PointwiseCustomObject::bind(self);
            value.subscribe(binding.clone());
            value.attach_route(&binding);
            guard(&self.owner.core.filters)[self.owner.ix].parameters[self.ix]
                .extras
                .binding = Some(binding);
        }
        drop(old);
        drop(binding);
        self.matrix_event(false)
    }
    pub(super) fn matrix_event(&self, is_speed: bool) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        if is_speed {
            return self.notify_change(speed());
        }
        self.notify_change(ordinary())?;
        let current = match self.raw_value() {
            PointwiseParameterValue::Matrix { current, .. }
            | PointwiseParameterValue::Region { current, .. } => current,
            _ => return Err(PointwiseError::NullReference),
        };
        let previous = current
            .ok_or(PointwiseError::NullReference)?
            .source_clone()?
            .raw();
        let old = {
            let mut entries = guard(&self.owner.core.filters);
            match &mut entries[self.owner.ix].parameters[self.ix].value {
                PointwiseParameterValue::Matrix { previous: p, .. }
                | PointwiseParameterValue::Region { previous: p, .. } => p.replace(previous),
                _ => return Err(PointwiseError::NullReference),
            }
        };
        drop(old);
        Ok(())
    }
    pub(super) fn copy_extras(&self) -> Result<ParameterExtras<E>, PointwiseError<E>>
    where
        E: 'static,
    {
        let (a, size, seed_modifier, wire, ordinary_limits) = {
            let entries = guard(&self.owner.core.filters);
            let e = &entries[self.owner.ix].parameters[self.ix].extras;
            (
                e.animation.clone(),
                e.size.clone(),
                e.seed_modifier,
                e.wire.clone(),
                e.ordinary_limits.clone(),
            )
        };
        let animation = a
            .as_ref()
            .map(GradientAnimation::source_clone)
            .transpose()?;
        let wire = wire.source_clone()?;
        Ok(ParameterExtras {
            // os2426 passes A0.d directly to the new property constructor.
            ordinary_limits,
            size,
            seed_modifier,
            wire,
            animation,
            ..ParameterExtras::default()
        })
    }
}

#[cfg(test)]
#[path = "retained_spatial_test.rs"]
mod tests;

#[cfg(test)]
mod adaptive_mode_laws {
    use super::*;
    #[test]
    fn smear_constructor_mode_precedes_normalization_but_source_copy_keeps_its_fields() {
        // ana3069 -> ok0997 -> os.i2463. Source-static, not an original golden.
        for (mode, normalized, has_flags) in [
            (PointwiseCloneMode::Original, [6.; 2], true),
            (PointwiseCloneMode::Clone, [0.; 2], false),
        ] {
            let f = PointwiseFilter::<()>::with_clone_mode(PointwiseKind::Smear, mode).unwrap();
            let size = f.parameter(0).unwrap();
            assert_eq!(size.pair().unwrap(), [6.; 2]);
            assert_eq!(size.previous_pair().unwrap(), [6.; 2]);
            assert_eq!(size.normalized_pair().unwrap(), normalized);
            assert_eq!(size.value_at(0, 0.).unwrap(), normalized[0]);
            assert_eq!(size.raw_value_at(0, 0.).unwrap(), 6.);
            let flags = guard(&f.core.filters)[f.ix].parameters[0]
                .extras
                .size
                .as_ref()
                .unwrap()
                .flags
                .clone();
            assert_eq!(flags.is_some(), has_flags);
            if let Some(flags) = flags {
                assert_eq!(flags, vec![false; 20]);
            }
            let copied = f.source_clone().unwrap();
            assert_eq!(
                copied.parameter(0).unwrap().normalized_pair().unwrap(),
                normalized
            );
            let captured = f.capture_render().unwrap();
            assert_eq!(
                captured
                    .model
                    .parameter(0)
                    .unwrap()
                    .normalized_pair()
                    .unwrap(),
                normalized
            );
            assert_eq!(
                guard(&captured.model.core.filters)[captured.model.ix].parameters[0]
                    .extras
                    .size
                    .as_ref()
                    .unwrap()
                    .flags
                    .is_some(),
                has_flags
            );
            size.set_pair([2., 3.]).unwrap();
            assert_eq!(
                size.normalized_pair().unwrap(),
                if mode == PointwiseCloneMode::Original {
                    [2., 3.]
                } else {
                    [0.; 2]
                }
            );
            assert_eq!(
                copied.parameter(0).unwrap().normalized_pair().unwrap(),
                normalized
            );
            assert_eq!(
                captured
                    .model
                    .parameter(0)
                    .unwrap()
                    .normalized_pair()
                    .unwrap(),
                normalized
            );
        }
    }
}
