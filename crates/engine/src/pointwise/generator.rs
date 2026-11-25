//! Shared retained zj/al0/i1/av9 image-generator authority. Properties sponsor
//! the actual graph node; family constructor profiles do not duplicate models.
//! Paired and scalar properties retain real animation owners/curves. Positive
//! adjusted/editor contexts and animated choices remain explicit capabilities.
use super::graph::Payload;
#[path = "generator_bricks.rs"]
mod bricks;
#[path = "gradient_cached_render.rs"]
mod cached_render;
#[path = "generator_family.rs"]
mod family;
#[path = "gradient_final_render.rs"]
mod final_render;
pub use cached_render::{GradientCachedRenderMode, GradientRenderHandler};
pub(in crate::pointwise) use final_render::FinalImageCache;
pub use final_render::GradientFinalClone;
#[path = "imported_node.rs"]
pub(super) mod imported_node;
#[path = "generator_wire.rs"]
mod wire;
pub use family::ImageGeneratorKind;
use family::Scalar;
pub use imported_node::ImportedImageNode;

pub type ImageGenerator<E = ()> = GradientGenerator<E>;
pub type ImageGeneratorParameter<E = ()> = GradientGeneratorParameter<E>;
pub type ImageGeneratorParameterKind = GradientGeneratorParameterKind;
pub type ImageGeneratorChangeHandler<E = ()> = GradientGeneratorChangeHandler<E>;
pub type ImageGeneratorRenderSnapshot = GradientRenderSnapshot;
const PARAMETER_COUNT: usize = 19;
use super::property::{NumericAdjustment, NumericPropertyState};
use super::{PointwiseError, PointwiseNode, SharedImageMap, guard, max64, min64};
use crate::ordinary_events::{DelegateCore, HandlerCore};
use crate::{
    AnimationError, ContextChangeArgument, ContextPropertyChangeArgs, CurveRangeRecord,
    DecimalBits, GradientAnimation, ImageError, LimitsDefinition, LimitsStep, RangeOptionsRecord,
    SharedCurveRangeRecord, SharedLimits, SharedRangeOptionsRecord, SharedSelectionOptions,
    round_decimal_places,
};
use std::{
    fmt,
    sync::{Arc, Mutex},
};

pub(super) const MODE_NAMES: [&str; 13] = [
    "Horizontal",
    "Vertical",
    "Conical",
    "Radial",
    "Pyramidal",
    "Square to Circle",
    "Star",
    "Pillow",
    "Saddle",
    "Dimples",
    "Dimpled Pyramid",
    "Dimpled Star",
    "Dimpled Pillow",
];

type GeneratorEvents<E> =
    DelegateCore<GradientGeneratorParameter<E>, Arc<ContextChangeArgument<E>>, E>;

/// The fixed property identities in the original generator's construction order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GradientGeneratorParameterKind {
    Size,
    Location,
    Mode,
    Seed,
    Iterations,
    Density,
    Irregularity,
    Distribution,
    Anomalies,
    BlendWidth,
    BlockDensity,
    BorderWidth,
    Shifts,
    WidthVariation,
    SlideVariation,
    Orientation,
    Image,
    ForceSquareRatio,
    UpsizeSmoothing,
}
impl GradientGeneratorParameterKind {
    fn ix(self) -> usize {
        match self {
            Self::Size => 0,
            Self::Location => 1,
            Self::Mode => 2,
            Self::Seed => 3,
            Self::Iterations => 4,
            Self::Density => 5,
            Self::Irregularity => 6,
            Self::Distribution => 7,
            Self::Anomalies => 8,
            Self::BlendWidth => 9,
            Self::BlockDensity => 10,
            Self::BorderWidth => 11,
            Self::Shifts => 12,
            Self::WidthVariation => 13,
            Self::SlideVariation => 14,
            Self::Orientation => 15,
            Self::Image => 16,
            Self::ForceSquareRatio => 17,
            Self::UpsizeSmoothing => 18,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Size => "Size",
            Self::Location => "Location",
            Self::Mode => "Gradient Type",
            Self::Seed => "Seed",
            Self::Iterations => "Iterations",
            Self::Density => "Density",
            Self::Irregularity => "Irregularity",
            Self::Distribution => "Distribution",
            Self::Anomalies => "Anomalies",
            Self::BlendWidth => "Blend Width",
            Self::BlockDensity => "Block Density",
            Self::BorderWidth => "Border Width",
            Self::Shifts => "Shifts",
            Self::WidthVariation => "Width Variation",
            Self::SlideVariation => "Slide Variation",
            Self::Orientation => "Orientation",
            Self::Image => "Image",
            Self::ForceSquareRatio => "Force Square Ratio",
            Self::UpsizeSmoothing => "Upsize Smoothing",
        }
    }
}
#[derive(Clone)]
struct Pair {
    current: [f64; 2],
    previous: [f64; 2],
    // Actual os normalized Size fields and m_u, not a second editable pair.
    normalized: [f64; 2],
    scale: f64,
    limits: SharedLimits,
}
struct Selection {
    current: i32,
    previous: i32,
    options: SharedSelectionOptions,
}
pub(super) struct GeneratorState<E> {
    kind: ImageGeneratorKind,
    scalars: [Option<Scalar>; PARAMETER_COUNT],
    pairs: Vec<Pair>,
    factor: f64,
    selection: Selection,
    handlers: [GeneratorEvents<E>; PARAMETER_COUNT],
    allow_changes: [bool; PARAMETER_COUNT],
    animations: [Option<GradientAnimation<E>>; PARAMETER_COUNT],
    wire: [super::property::wire::LegacyWireMetadata<E>; PARAMETER_COUNT],
    pub(super) cache: Option<super::RenderedImage>,
    pub(super) render_caches: cached_render::GradientRenderCaches<E>,
    pub(super) final_clone: bool,
    pub(super) final_cache: FinalImageCache,
    imported: Option<imported_node::ImportedState<E>>,
}
fn dec(value: i32) -> Result<DecimalBits, ImageError> {
    DecimalBits::new([
        value.unsigned_abs(),
        0,
        0,
        if value < 0 { 0x8000_0000 } else { 0 },
    ])
    .map_err(|_| ImageError::Argument)
}
fn pair_limits(location: bool, gradient: bool) -> Result<SharedLimits, ImageError> {
    let d = LimitsDefinition::default();
    SharedLimits::from_definition(LimitsDefinition {
        decimal_places: if location { 3 } else { d.decimal_places },
        step: if location {
            LimitsStep::Derived(
                DecimalBits::new([1, 0, 0, 1 << 16]).map_err(|_| ImageError::Argument)?,
            )
        } else {
            d.step
        },
        minimum: if location {
            dec(-1_000_000)?
        } else {
            d.minimum
        },
        maximum: if location { dec(1_000_000)? } else { d.maximum },
        used: d.used,
        use_range: Some(SharedRangeOptionsRecord::new(RangeOptionsRecord {
            minimum: dec(-10)?,
            maximum: dec(10)?,
            decimal_places: 0,
            multiplier: 1.0,
            flags: [false, false, true],
        })),
        curve_range: Some(SharedCurveRangeRecord::new(CurveRangeRecord {
            minimum: dec(0)?,
            maximum: dec(if location && gradient { 1 } else { 10 })?,
            wrap_y: location && gradient,
        })),
    })
    .map_err(|_| ImageError::Overflow)
}
/// A real retained leaf in the PointwiseNode graph. New uses original/unbound
/// property semantics; it does not fabricate a document, history or APV host.
pub struct GradientGenerator<E = ()> {
    node: PointwiseNode<E>,
}
impl<E> Clone for GradientGenerator<E> {
    fn clone(&self) -> Self {
        Self {
            node: self.node.clone(),
        }
    }
}
impl<E> fmt::Debug for GradientGenerator<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "GradientGenerator({:?})", self.node)
    }
}
impl<E> GradientGenerator<E> {
    pub fn new() -> Result<Self, PointwiseError<E>> {
        Self::with_kind(ImageGeneratorKind::Gradient)
    }
    pub fn with_kind(kind: ImageGeneratorKind) -> Result<Self, PointwiseError<E>> {
        let generator = Self {
            node: PointwiseNode::publish_image_generator(Arc::new(Mutex::new(
                GeneratorState::with_kind(kind)?,
            ))),
        };
        if kind == ImageGeneratorKind::ImportedImage {
            generator.bind_imported();
        }
        Ok(generator)
    }
    pub fn kind(&self) -> ImageGeneratorKind {
        guard(&self.state()).kind
    }

    pub(super) fn from_node(node: &PointwiseNode<E>) -> Option<Self> {
        matches!(&node.core().payload, Payload::ImageGenerator { .. })
            .then(|| Self { node: node.clone() })
    }
    fn state(&self) -> Arc<Mutex<GeneratorState<E>>> {
        match &self.node.core().payload {
            Payload::ImageGenerator { model } => Arc::clone(model),
            _ => unreachable!("ImageGenerator is constructed only from its matching payload"),
        }
    }
    pub fn node(&self) -> PointwiseNode<E> {
        self.node.clone()
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        self.node.same_reference(&other.node)
    }
    pub fn parameter(&self, kind: GradientGeneratorParameterKind) -> GradientGeneratorParameter<E> {
        GradientGeneratorParameter {
            owner: self.clone(),
            kind,
        }
    }
    pub fn parameters(&self) -> Vec<GradientGeneratorParameter<E>> {
        self.kind()
            .parameters()
            .iter()
            .map(|kind| self.parameter(*kind))
            .collect()
    }
    /// os.a(scale, flags) unbound Size route. zj ignores the per-node flags;
    /// scale commits before normalization, including a subsequent Math.Sign error.
    pub fn set_size_scale(&self, scale: f64) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        self.set_size_property_scale(scale)
    }
    /// Local unbound ok.s6 -> zl.s6 operation: normalize/notify the actual Size
    /// property first, then commit Factor. This is distinct from os.a above.
    /// Full Slots/Contents scale propagation remains a graph capability gap.
    pub fn set_local_node_scale(&self, scale: f64) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        // Base s6 still runs after an unchanged successful property call;
        // normalization or observer failure prevents the Factor write.
        self.set_size_property_scale(scale)?;
        guard(&self.state()).factor = scale;
        Ok(())
    }
    fn set_size_property_scale(&self, scale: f64) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        let state = self.state();
        let mut s = guard(&state);
        if s.pairs[0].scale == scale {
            return Ok(());
        }
        s.pairs[0].scale = scale;
        drop(s);
        self.normalize_size()?;
        let handlers = guard(&state).handlers[0].clone();
        self.notify(
            GradientGeneratorParameterKind::Size,
            handlers,
            ContextChangeArgument::shared_empty(),
        )
    }
    /// Local ok.py: compute virtual iz, commit X then Y, then virtual gb.
    /// The full attached-node/quality traversal of zl.py is not this local API.
    pub fn apply_size_scale(&self, scale: f64) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        let values = self.scaled_size_extents(scale)?;
        let p = self.parameter(GradientGeneratorParameterKind::Size);
        p.set_extent(0, values[0])?;
        p.set_extent(1, values[1])?;
        if self.kind() == ImageGeneratorKind::BricksBoards {
            self.commit_bricks_scale(scale)?;
        }
        Ok(())
    }
    fn scaled_size_extents(&self, scale: f64) -> Result<[f64; 2], PointwiseError<E>>
    where
        E: 'static,
    {
        let p = self.parameter(GradientGeneratorParameterKind::Size);
        if self.kind() == ImageGeneratorKind::BricksBoards && self.bricks_scale_predicate(scale)? {
            return Ok([p.extent(0)?, p.extent(1)?]);
        }
        let mut values = [p.extent(0)? * scale, p.extent(1)? * scale];
        if matches!(
            self.kind(),
            ImageGeneratorKind::Cells | ImageGeneratorKind::WoodRings
        ) {
            for value in &mut values {
                if value.abs() < 1.0 {
                    *value = if *value < 0.0 {
                        -1.0
                    } else if *value > 0.0 {
                        1.0
                    } else {
                        0.0
                    };
                }
            }
        } else if matches!(
            self.kind(),
            ImageGeneratorKind::Gradient
                | ImageGeneratorKind::BricksBoards
                | ImageGeneratorKind::ImportedImage
        ) {
            round_scaled(&mut values, scale)?;
        }
        Ok(values)
    }
    fn normalize_size(&self) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        let p = self.parameter(GradientGeneratorParameterKind::Size);
        let scale = p.scale()?;
        let state = self.state();
        // iz writes X before the Y extent read. Preserve this partial state
        // even when a malformed Y owner or later Math.Sign rejects the call.
        let special = self.kind() == ImageGeneratorKind::BricksBoards
            && self.bricks_scale_predicate(scale)?;
        let x = if special {
            p.extent(0)?
        } else {
            p.extent(0)? * scale
        };
        guard(&state).pairs[0].normalized[0] = x;
        let y = if special {
            p.extent(1)?
        } else {
            p.extent(1)? * scale
        };
        guard(&state).pairs[0].normalized[1] = y;
        let mut values = [x, y];
        if matches!(
            self.kind(),
            ImageGeneratorKind::Cells | ImageGeneratorKind::WoodRings
        ) {
            for value in &mut values {
                if value.abs() < 1.0 {
                    *value = if *value < 0.0 {
                        -1.0
                    } else if *value > 0.0 {
                        1.0
                    } else {
                        0.0
                    };
                }
            }
        } else if !special
            && matches!(
                self.kind(),
                ImageGeneratorKind::Gradient
                    | ImageGeneratorKind::BricksBoards
                    | ImageGeneratorKind::ImportedImage
            )
        {
            round_scaled(&mut values, scale)?;
        }
        guard(&state).pairs[0].normalized = values;
        Ok(())
    }
    /// Actual zl.ca stored Factor, with no public field replacement or provider.
    pub fn factor(&self) -> f64 {
        guard(&self.state()).factor
    }
    /// Actual os.a2/az normalized stored Size fields, without time evaluation.
    /// This is a read-only state observation, not an animated render shortcut.
    pub fn normalized_size(&self) -> [f64; 2] {
        guard(&self.state()).pairs[0].normalized
    }
    /// Source os.j/l: evaluate each active Size axis, otherwise use its stored
    /// normalized value. Curve errors are genuine failures, never fallbacks.
    pub fn size_at(&self, time: f64) -> Result<[f64; 2], PointwiseError<E>>
    where
        E: 'static,
    {
        let p = self.parameter(GradientGeneratorParameterKind::Size);
        Ok([p.value_at(0, time)?, p.value_at(1, time)?])
    }
    /// Source os.e/g: active Location axes sample their actual owner curves;
    /// inactive unbound axes use their current raw pair values.
    pub fn location_at(&self, time: f64) -> Result<[f64; 2], PointwiseError<E>>
    where
        E: 'static,
    {
        let p = self.parameter(GradientGeneratorParameterKind::Location);
        Ok([p.value_at(0, time)?, p.value_at(1, time)?])
    }
    pub fn source_clone(&self) -> Result<Self, PointwiseError<E>>
    where
        E: 'static,
    {
        Self::from_node(&self.node.source_clone()?).ok_or(PointwiseError::NullReference)
    }
    pub(super) fn copy_local(&self) -> Result<Self, PointwiseError<E>>
    where
        E: 'static,
    {
        let state = self.state();
        let s = guard(&state);
        let kind = s.kind;
        let factor = s.factor;
        let final_clone = s.final_clone;
        let scalars = s.scalars.clone();
        let pairs = s.pairs.clone();
        let selection = Selection {
            current: s.selection.current,
            previous: s.selection.previous,
            options: s.selection.options.clone(),
        };
        let animations = s.animations.clone();
        let source_wire = s.wire.clone();
        let imported_copy = s.imported.as_ref().map(|_| self.clone());
        drop(s);
        // os copy constructs Size, then Location; each real bh copy has fresh
        // arrays and per-occurrence Curve copies but shares its Limits array.
        // No generator lock crosses potentially fallible child construction.
        let mut copied_animations = std::array::from_fn(|_| None);
        let mut wire: [_; PARAMETER_COUNT] =
            std::array::from_fn(|_| super::property::wire::LegacyWireMetadata::new(false, false));
        for parameter in kind.parameters() {
            let ix = parameter.ix();
            copied_animations[ix] = animations[ix]
                .as_ref()
                .map(GradientAnimation::source_clone)
                .transpose()?;
            wire[ix] = source_wire[ix].source_clone()?;
        }
        let animations = copied_animations;
        let copy = Self {
            node: PointwiseNode::publish_image_generator(Arc::new(Mutex::new(GeneratorState {
                kind,
                factor,
                scalars,
                pairs,
                selection,
                animations,
                wire,
                handlers: std::array::from_fn(|_| DelegateCore::Absent),
                allow_changes: [true; PARAMETER_COUNT],
                cache: None,
                render_caches: cached_render::GradientRenderCaches::new(),
                final_clone,
                final_cache: final_render::FinalImageCache::default(),
                imported: None,
            }))),
        };
        if let Some(source) = imported_copy {
            copy.copy_imported_from(&source)?;
        }
        Ok(copy)
    }
    fn notify(
        &self,
        kind: GradientGeneratorParameterKind,
        handlers: GeneratorEvents<E>,
        args: Arc<ContextChangeArgument<E>>,
    ) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        // os.a(EventArgs) replaces the outgoing arguments when AllowChanges is
        // false. The setter still commits; default args are controls-only, so
        // the parent node receives them before deciding whether to invalidate.
        let args = if guard(&self.state()).allow_changes[kind.ix()] {
            args
        } else {
            Arc::new(ContextChangeArgument::Property(
                ContextPropertyChangeArgs::default(),
            ))
        };
        // zl.k7 suppresses sh/invalidation only for UpdateControlsOnly. Empty
        // EventArgs (the Size scale route) and ordinary null-undo args still
        // invalidate before external property observers execute.
        if !matches!(args.as_ref(), ContextChangeArgument::Property(p) if p.update_controls_only())
        {
            self.node.notify_property_change_with(args.clone())?;
        }
        handlers
            .invoke(&self.parameter(kind), args)
            .map_err(PointwiseError::Observer)
    }
    /// Actual protected-k8 translation. Public graph rendering wraps ordinary
    /// failures through the existing evaluator; this entry retains the cause.
    pub fn render_unbound(
        &self,
        width: i32,
        height: i32,
        time: f64,
    ) -> Result<SharedImageMap, PointwiseError<E>>
    where
        E: 'static,
    {
        self.ensure_property_evaluation_supported()?;
        if self.kind() == ImageGeneratorKind::ImportedImage {
            return self.render_imported(
                width,
                height,
                time,
                super::PointwiseRenderOptions::new(false, false),
                crate::ProceduralExecution::CallingThread,
            );
        }
        if self.kind() != ImageGeneratorKind::Gradient {
            return Err(PointwiseError::UnsupportedCapability(
                "procedural generator requires explicit execution and preflight context",
            ));
        }
        self.render_gradient(
            width,
            height,
            time,
            crate::ProceduralExecution::CallingThread,
            &|| false,
        )
    }

    pub(super) fn render_gradient(
        &self,
        width: i32,
        height: i32,
        time: f64,
        execution: crate::ProceduralExecution,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<SharedImageMap, PointwiseError<E>>
    where
        E: 'static,
    {
        super::check_render_cancelled(cancelled)?;
        self.ensure_property_evaluation_supported()?;
        let geometry = crate::ImageGeometry::new(width, height);
        let out = SharedImageMap::new(geometry);
        let destination = out.materialize(0)?;
        // Original ordering: materialize, resolve mode, resolve both Size axes,
        // then read Location only in a recognized branch.
        let operands = self.render_operands(time)?;
        if let Some(mode) = operands.mode {
            super::generator_kernel::render(
                mode,
                &destination,
                geometry,
                operands.size,
                operands.location,
                execution,
                cancelled,
            )?;
        } else {
            super::check_render_cancelled(cancelled)?;
        }
        Ok(out)
    }
}
// The unbound ordinary setter calls e((IPropertyUndoInfo)null): a genuine
// PropertyChangedEventArgs with both flags false and no invented undo object.
fn ordinary_args<E>() -> Arc<ContextChangeArgument<E>> {
    Arc::new(ContextChangeArgument::Property(
        ContextPropertyChangeArgs::with_undo(None),
    ))
}
fn controls_args<E>() -> Arc<ContextChangeArgument<E>> {
    Arc::new(ContextChangeArgument::Property(
        ContextPropertyChangeArgs::with_flags(false, true),
    ))
}
fn round_scaled<E>(result: &mut [f64; 2], scale: f64) -> Result<(), PointwiseError<E>> {
    if scale != 1.0 {
        let sign = |v: f64| -> Result<f64, PointwiseError<E>> {
            if v.is_nan() {
                Err(PointwiseError::Arithmetic)
            } else if v < 0.0 {
                Ok(-1.0)
            } else if v > 0.0 {
                Ok(1.0)
            } else {
                Ok(0.0)
            }
        };
        // ok.g computes BOTH signs before rounding either output.
        let signs = [sign(result[0])?, sign(result[1])?];
        for axis in 0..2 {
            result[axis] = round_decimal_places(result[axis], 0)
                .map_err(|_| ImageError::ArgumentOutOfRange)?;
            if result[axis] == 0.0 {
                result[axis] = signs[axis];
            }
        }
    }
    Ok(())
}
/// One retained source property root. Clone shares identity; source node copy
/// creates new property roots and shares actual Limits/Options objects.
pub struct GradientGeneratorParameter<E = ()> {
    owner: GradientGenerator<E>,
    kind: GradientGeneratorParameterKind,
}
impl<E> Clone for GradientGeneratorParameter<E> {
    fn clone(&self) -> Self {
        Self {
            owner: self.owner.clone(),
            kind: self.kind,
        }
    }
}
impl<E> fmt::Debug for GradientGeneratorParameter<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "GradientGeneratorParameter({:?}/{:?})",
            self.owner, self.kind
        )
    }
}
/// Ordered source property notification binding with actual shared argument
/// metadata. Captures stay strong and run outside all state guards. Controls-only
/// option replacements retain warm graph caches; ordinary and Empty args do not.
pub struct GradientGeneratorChangeHandler<E = ()>(GeneratorEvents<E>);
impl<E> Clone for GradientGeneratorChangeHandler<E> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<E> fmt::Debug for GradientGeneratorChangeHandler<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("GradientGeneratorChangeHandler")
    }
}
impl<E> GradientGeneratorChangeHandler<E> {
    pub fn new(
        callback: impl Fn(
            &GradientGeneratorParameter<E>,
            Arc<ContextChangeArgument<E>>,
        ) -> Result<(), E>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self(DelegateCore::Single(Arc::new(HandlerCore {
            callback: Box::new(callback),
        })))
    }
    pub fn same_binding(&self, other: &Self) -> bool {
        self.0.same(&other.0)
    }
}
impl<E> GradientGeneratorParameter<E> {
    pub fn owner(&self) -> GradientGenerator<E> {
        self.owner.clone()
    }

    pub(super) fn prepare_numeric_adjustment(
        &self,
        adjustment: NumericAdjustment,
    ) -> Result<(NumericPropertyState, NumericPropertyState), PointwiseError<E>> {
        use NumericPropertyState as State;
        Ok(match adjustment {
            NumericAdjustment::Number(value) => {
                let scalar = self.scalar()?;
                let value = self.rounded_scalar(value)?;
                (
                    State::Number {
                        current: scalar.current,
                        previous: scalar.previous,
                    },
                    State::Number {
                        current: value,
                        previous: value,
                    },
                )
            }
            NumericAdjustment::Pair(value) => {
                let normalized = [self.rounded(value[0])?, self.rounded(value[1])?];
                let ix = self.pair_ix()?;
                let owner = self.owner.state();
                let state = guard(&owner);
                let pair = &state.pairs[ix];
                (
                    State::Pair {
                        current: pair.current,
                        previous: pair.previous,
                    },
                    State::Pair {
                        current: normalized,
                        previous: normalized,
                    },
                )
            }
            NumericAdjustment::Choice(index) => {
                let owner = self.owner.state();
                let state = guard(&owner);
                let choice = state.choice(self.kind);
                (
                    State::Choice {
                        current: choice.current,
                        previous: choice.previous,
                        memory: None,
                    },
                    State::Choice {
                        current: index,
                        previous: index,
                        memory: None,
                    },
                )
            }
        })
    }

    pub(super) fn write_numeric_adjustment(&self, value: &NumericPropertyState) {
        use NumericPropertyState as State;
        let pair_ix = match value {
            State::Pair { .. } => Some(self.pair_ix().ok().expect("retained generator pair")),
            _ => None,
        };
        let owner = self.owner.state();
        let mut state = guard(&owner);
        match value {
            State::Number { current, previous } => {
                let scalar = state.scalars[self.kind.ix()]
                    .as_mut()
                    .expect("retained generator scalar");
                scalar.set_current(*current);
                scalar.previous = *previous;
            }
            State::Pair { current, previous } => {
                let pair = &mut state.pairs[pair_ix.expect("retained generator pair")];
                pair.current = *current;
                pair.previous = *previous;
            }
            State::Choice {
                current, previous, ..
            } => {
                let choice = state.choice_mut(self.kind);
                choice.current = *current;
                choice.previous = *previous;
            }
        }
    }

    pub(super) fn normalize_numeric_adjustment(&self) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        if self.kind == GradientGeneratorParameterKind::Size {
            self.owner.normalize_size()?;
        }
        Ok(())
    }

    pub(super) fn notify_numeric_adjustment(&self) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        let handlers = guard(&self.owner.state()).handlers[self.kind.ix()].clone();
        self.owner.notify(self.kind, handlers, ordinary_args())
    }
    pub fn kind(&self) -> GradientGeneratorParameterKind {
        self.kind
    }
    pub fn name(&self) -> &'static str {
        match (self.owner.kind(), self.kind) {
            (ImageGeneratorKind::Noise, GradientGeneratorParameterKind::Mode) => "Smoothing",
            (ImageGeneratorKind::Cells, GradientGeneratorParameterKind::Mode) => "Mode",
            (ImageGeneratorKind::WoodRings, GradientGeneratorParameterKind::Density) => {
                "Ring Density"
            }
            _ => self.kind.name(),
        }
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        self.kind == other.kind && self.owner.same_reference(&other.owner)
    }
    /// Source IProperty.AllowChanges changes notification metadata, not whether
    /// the property accepts edits. Updating it is silent.
    pub fn is_changes_allowed(&self) -> bool {
        guard(&self.owner.state()).allow_changes[self.kind.ix()]
    }
    pub fn set_changes_allowed(&self, allowed: bool) {
        guard(&self.owner.state()).allow_changes[self.kind.ix()] = allowed;
    }
    pub fn is_paired(&self) -> bool {
        self.pair_ix().is_ok()
    }
    pub fn is_choice(&self) -> bool {
        self.owner.kind().parameters().contains(&self.kind)
            && matches!(
                self.kind,
                GradientGeneratorParameterKind::Mode
                    | GradientGeneratorParameterKind::Orientation
                    | GradientGeneratorParameterKind::ForceSquareRatio
                    | GradientGeneratorParameterKind::UpsizeSmoothing
            )
    }
    pub fn is_scalar(&self) -> bool {
        self.owner.kind().parameters().contains(&self.kind)
            && !self.is_paired()
            && !self.is_choice()
            && self.kind != GradientGeneratorParameterKind::Image
    }
    fn pair_ix(&self) -> Result<usize, PointwiseError<E>> {
        match self.kind {
            GradientGeneratorParameterKind::Size => Ok(0),
            GradientGeneratorParameterKind::Location => Ok(1),
            GradientGeneratorParameterKind::BlockDensity
                if self.owner.kind() == ImageGeneratorKind::BricksBoards =>
            {
                Ok(2)
            }
            _ => Err(PointwiseError::NullReference),
        }
    }
    pub fn pair(&self) -> Result<[f64; 2], PointwiseError<E>> {
        let ix = self.pair_ix()?;
        Ok(guard(&self.owner.state()).pairs[ix].current)
    }
    pub fn previous_pair(&self) -> Result<[f64; 2], PointwiseError<E>> {
        let ix = self.pair_ix()?;
        Ok(guard(&self.owner.state()).pairs[ix].previous)
    }
    pub fn limits(&self) -> Result<SharedLimits, PointwiseError<E>> {
        if let Ok(ix) = self.pair_ix() {
            return Ok(guard(&self.owner.state()).pairs[ix].limits.clone());
        }
        self.scalar()?.limits.ok_or(PointwiseError::NullReference)
    }
    /// A read of the stored owner never invokes its lazy constructor.
    pub fn retained_animation(&self) -> Result<Option<GradientAnimation<E>>, PointwiseError<E>> {
        let ix = self.animation_ix()?;
        Ok(guard(&self.owner.state()).animations[ix].clone())
    }
    fn animation_ix(&self) -> Result<usize, PointwiseError<E>> {
        if self.owner.kind().parameters().contains(&self.kind) {
            Ok(self.kind.ix())
        } else {
            Err(PointwiseError::NullReference)
        }
    }
    /// Source os.get_AnimationProperty for a paired property: count=2, one
    /// retained Limits entry, false flags and null Curve slots. No event fires.
    pub fn animation(&self) -> Result<GradientAnimation<E>, PointwiseError<E>>
    where
        E: 'static,
    {
        let ix = self.animation_ix()?;
        if let Some(animation) = self.retained_animation()? {
            return Ok(animation);
        }
        let animation = GradientAnimation::new(
            if self.pair_ix().is_ok() { 2 } else { 1 },
            self.wire_limits()?,
        )?;
        guard(&self.owner.state()).animations[ix] = Some(animation.clone());
        Ok(animation)
    }
    /// Source os.AnimationProperty replacement commits even the same or null
    /// reference. Eligible history keeps both actual animation roots.
    pub fn set_animation(
        &self,
        value: Option<GradientAnimation<E>>,
    ) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        let ix = self.animation_ix()?;
        let state = self.owner.state();
        let mut s = guard(&state);
        let old = std::mem::replace(&mut s.animations[ix], value.clone());
        let handlers = s.handlers[self.kind.ix()].clone();
        drop(s);
        let args = self.ordinary_edit_args(|name| {
            let undo = crate::AnimationUndoInfo::replacement(Some(name), old, value);
            ContextChangeArgument::Property(ContextPropertyChangeArgs::with_animation_undo(
                Arc::new(undo),
            ))
        })?;
        self.owner.notify(self.kind, handlers, args)
    }
    pub fn is_animated(&self) -> Result<bool, PointwiseError<E>>
    where
        E: 'static,
    {
        match self.retained_animation()? {
            Some(owner) => Ok(owner.is_active()?),
            None => Ok(false),
        }
    }
    // os.a(int) scans current flags BEFORE checking the stored component count.
    // Arrays remain separately replaceable public roots; no synthesized slots.
    pub(super) fn axis_is_animated(&self, axis: usize) -> Result<bool, PointwiseError<E>>
    where
        E: 'static,
    {
        super::parameter_ops::axis_active(|| self.retained_animation(), axis)
    }
    fn animation_curve(&self, axis: usize) -> Result<crate::SharedCurve<E>, PointwiseError<E>>
    where
        E: 'static,
    {
        super::parameter_ops::curve(|| self.retained_animation(), axis)
    }
    /// os.bm/bj for a paired property: active axes use the largest absolute
    /// current Curve bound; inactive axes use the raw stored pair value.
    /// Only Size uses this extent to compute normalized rendering dimensions.
    pub fn extent(&self, axis: usize) -> Result<f64, PointwiseError<E>>
    where
        E: 'static,
    {
        self.animation_ix()?;
        if axis >= 2 {
            return Err(PointwiseError::Index);
        }
        if !self.axis_is_animated(axis)? {
            return Ok(self.pair()?[axis]);
        }
        let minimum = self.animation_curve(axis)?.minimum().abs();
        let maximum = self.animation_curve(axis)?.maximum().abs();
        Ok(f64::from(super::max32(minimum, maximum)))
    }
    /// os.t/v: request an effective extent. An active axis divides by its
    /// current live extent and scales its actual Curve bounds; an inactive axis
    /// edits the raw pair through the ordinary source setter.
    pub fn set_extent(&self, axis: usize, desired: f64) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        self.animation_ix()?;
        if axis >= 2 {
            return Err(PointwiseError::Index);
        }
        if self.axis_is_animated(axis)? {
            self.multiply_axis(axis, desired / self.extent(axis)?)
        } else {
            self.set_axis(axis, desired)
        }
    }
    /// os.s/w: multiply one active Curve's bounds, or multiply the raw value
    /// through the real setter while inactive. This also applies to Location.
    pub fn multiply_axis(&self, axis: usize, factor: f64) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        self.animation_ix()?;
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
                .map_err(AnimationError::<E>::Bounds)?;
            Ok(())
        } else {
            self.set_axis(axis, self.pair()?[axis] * factor)
        }
    }
    /// os.u/x: evaluate an active Curve with optional integer-place rounding,
    /// otherwise read the raw current value. For Size this differs from
    /// value_at, which uses normalized stored values on inactive axes.
    pub fn raw_value_at(&self, axis: usize, time: f64) -> Result<f64, PointwiseError<E>>
    where
        E: 'static,
    {
        self.ensure_property_evaluation_supported()?;
        self.animation_ix()?;
        if axis >= 2 {
            return Err(PointwiseError::Index);
        }
        if !self.axis_is_animated(axis)?
            && let Some(value) = self.animated_binding_value(time)?
            && self.binding_applies_to_axis(axis)?
        {
            return Ok(value.number());
        }
        super::parameter_ops::pair_value(
            || self.retained_animation(),
            || self.limits(),
            || self.pair(),
            axis,
            time,
        )
    }
    /// Actual time getter for one of the paired source axes. Size has the
    /// original repeated active-axis check before u/x; Location uses u/x directly.
    pub fn value_at(&self, axis: usize, time: f64) -> Result<f64, PointwiseError<E>>
    where
        E: 'static,
    {
        if self.kind == GradientGeneratorParameterKind::Size {
            self.normalized_value_at(axis, time)
        } else {
            self.raw_value_at(axis, time)
        }
    }
    /// Source os.r/p for either paired property. Inactive axes expose their
    /// stored normalized field; active axes delegate to the actual raw u/x
    /// getter. Location rendering uses raw_value_at instead of these fields.
    pub fn normalized_value_at(&self, axis: usize, time: f64) -> Result<f64, PointwiseError<E>>
    where
        E: 'static,
    {
        self.ensure_property_evaluation_supported()?;
        self.animation_ix()?;
        if axis >= 2 {
            return Err(PointwiseError::Index);
        }
        if !self.axis_is_animated(axis)? {
            let ix = self.pair_ix()?;
            return Ok(guard(&self.owner.state()).pairs[ix].normalized[axis]);
        }
        self.raw_value_at(axis, time)
    }
    pub fn scale(&self) -> Result<f64, PointwiseError<E>> {
        let ix = self.pair_ix()?;
        Ok(guard(&self.owner.state()).pairs[ix].scale)
    }
    fn rounded(&self, value: f64) -> Result<f64, PointwiseError<E>> {
        super::property::normalize_parameter(&self.limits()?, value)
    }
    fn ordinary_edit_args(
        &self,
        make_undo: impl FnOnce(Vec<u16>) -> ContextChangeArgument<E>,
    ) -> Result<Arc<ContextChangeArgument<E>>, PointwiseError<E>>
    where
        E: 'static,
    {
        // os.m qualifies original properties on a retained TextureNodeLogic.
        if self.owner.node().has_property_history_target() && !self.wire_metadata().is_adjusted()? {
            Ok(Arc::new(make_undo(self.name().encode_utf16().collect())))
        } else {
            Ok(ordinary_args())
        }
    }
    // The bound pair setter compares only the current value.
    pub(super) fn write_bound_pair(
        &self,
        axis: Option<usize>,
        value: f64,
        force: bool,
    ) -> Result<bool, PointwiseError<E>>
    where
        E: 'static,
    {
        let value = self.rounded(value)?;
        let ix = self.pair_ix()?;
        let state = self.owner.state();
        let mut state = guard(&state);
        let pair = &mut state.pairs[ix];
        match axis {
            Some(axis) if force || pair.current[axis] != value => {
                pair.current[axis] = value;
                pair.previous[axis] = value;
            }
            None if force || pair.current != [value; 2] => {
                pair.current = [value; 2];
                pair.previous = [value; 2];
            }
            _ => return Ok(false),
        }
        drop(state);
        if ix == 0 {
            self.owner.normalize_size()?;
        }
        Ok(true)
    }

    // The bound selection setter keeps its previous index.
    pub(super) fn write_bound_choice(&self, index: i32, force: bool) -> bool {
        let state = self.owner.state();
        let mut state = guard(&state);
        let choice = state.choice_mut(self.kind);
        if choice.current == index && !force {
            return false;
        }
        choice.current = index;
        true
    }

    pub(super) fn notify_bound_change(&self) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        let handlers = guard(&self.owner.state()).handlers[self.kind.ix()].clone();
        self.owner
            .notify(self.kind, handlers, ContextChangeArgument::shared_empty())
    }
    pub fn set_axis(&self, axis: usize, value: f64) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        let ix = self.pair_ix()?;
        if axis >= 2 {
            return Err(PointwiseError::Index);
        }
        let value = self.rounded(value)?;
        let state = self.owner.state();
        let mut s = guard(&state);
        let pair = &mut s.pairs[ix];
        let old = pair.previous[axis];
        if !super::parameter_ops::commit_axis(&mut pair.current, &mut pair.previous, axis, value) {
            return Ok(());
        }
        drop(s);
        if ix == 0 {
            self.owner.normalize_size()?;
        }
        let args = self.ordinary_edit_args(|name| {
            let undo = crate::DoubleUndoInfo::new(Some(name), axis as i32 + 1, old, value);
            ContextChangeArgument::Property(ContextPropertyChangeArgs::with_undo(Some(Arc::new(
                undo,
            ))))
        })?;
        let handlers = guard(&state).handlers[self.kind.ix()].clone();
        self.owner.notify(self.kind, handlers, args)
    }
    pub fn set_pair(&self, values: [f64; 2]) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        let ix = self.pair_ix()?;
        let values = [self.rounded(values[0])?, self.rounded(values[1])?];
        let state = self.owner.state();
        let mut s = guard(&state);
        let p = &mut s.pairs[ix];
        let old = p.previous;
        if !super::parameter_ops::commit_pair(&mut p.current, &mut p.previous, values) {
            return Ok(());
        }
        drop(s);
        if ix == 0 {
            self.owner.normalize_size()?;
        }
        let args = self.ordinary_edit_args(|name| {
            let undo = crate::PairUndoInfo::new(
                Some(name),
                crate::PairUndoArray::new(old),
                crate::PairUndoArray::new(values),
            );
            ContextChangeArgument::Property(ContextPropertyChangeArgs::with_pair_undo(Some(
                Arc::new(undo),
            )))
        })?;
        let handlers = guard(&state).handlers[self.kind.ix()].clone();
        self.owner.notify(self.kind, handlers, args)
    }
    /// Source os.e(double,double) speed-rendering edit: retain the previous
    /// committed pair, normalize the new pair, then notify with speed=true.
    pub fn set_pair_speed(&self, values: [f64; 2]) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        let ix = self.pair_ix()?;
        let values = [self.rounded(values[0])?, self.rounded(values[1])?];
        let state = self.owner.state();
        let mut s = guard(&state);
        let p = &mut s.pairs[ix];
        if p.current == values {
            return Ok(());
        }
        p.previous = p.current;
        p.current = values;
        drop(s);
        if ix == 0 {
            self.owner.normalize_size()?;
        }
        let handlers = guard(&state).handlers[self.kind.ix()].clone();
        self.owner.notify(
            self.kind,
            handlers,
            Arc::new(ContextChangeArgument::Property(
                ContextPropertyChangeArgs::with_speed(true),
            )),
        )
    }
    pub fn index(&self) -> Result<i32, PointwiseError<E>> {
        if !self.is_choice() {
            return Err(PointwiseError::NullReference);
        }
        Ok(guard(&self.owner.state()).choice(self.kind).current)
    }
    pub fn set_index(&self, value: i32) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        if !self.is_choice() {
            return Err(PointwiseError::NullReference);
        }
        let state = self.owner.state();
        let mut s = guard(&state);
        if value < 0
            || (s.choice(self.kind).current == value && s.choice(self.kind).previous == value)
        {
            return Ok(());
        }
        let choice = s.choice_mut(self.kind);
        let old = if choice.previous != value {
            choice.previous
        } else {
            choice.current
        };
        choice.current = value;
        choice.previous = value;
        let handlers = s.handlers[self.kind.ix()].clone();
        drop(s);
        let args = self.ordinary_edit_args(|name| {
            let undo = crate::ChoiceUndoInfo::new(
                Some(name),
                crate::ChoiceUndoValue::Index(old),
                crate::ChoiceUndoValue::Index(value),
            );
            ContextChangeArgument::Property(ContextPropertyChangeArgs::with_choice_undo(Some(
                Arc::new(undo),
            )))
        })?;
        self.owner.notify(self.kind, handlers, args)
    }
    pub fn options(&self) -> Result<SharedSelectionOptions, PointwiseError<E>> {
        if !self.is_choice() {
            return Err(PointwiseError::NullReference);
        }
        Ok(guard(&self.owner.state()).choice(self.kind).options.clone())
    }
    pub fn set_options(
        &self,
        options: Option<SharedSelectionOptions>,
    ) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        let options = options.ok_or(PointwiseError::NullReference)?;
        if self.options()?.entries() == options.entries() {
            return Ok(());
        }
        let selected = options
            .entries()
            .iter()
            .position(Option::is_none)
            .unwrap_or(0) as i32;
        let state = self.owner.state();
        let mut s = guard(&state);
        s.choice_mut(self.kind).options = options;
        s.choice_mut(self.kind).current = selected;
        let handlers = s.handlers[self.kind.ix()].clone();
        drop(s);
        self.owner.notify(self.kind, handlers, controls_args())
    }
    pub fn set_text(&self, value: Option<&[u16]>) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        if self.text()?.as_deref() == value {
            return Ok(());
        }
        let options = self.options()?;
        let ix = options
            .entries()
            .iter()
            .position(|v| v.as_deref() == value)
            .ok_or(PointwiseError::Exception)?;
        self.set_index(ix as i32)
    }
    pub fn text(&self) -> Result<Option<Vec<u16>>, PointwiseError<E>> {
        let index = self.index()?;
        let options = self.options()?;
        options
            .entries()
            .get(usize::try_from(index).map_err(|_| PointwiseError::Index)?)
            .cloned()
            .ok_or(PointwiseError::Index)
    }
    /// The original selection getter samples the first curve as a one-based
    /// option number, rounds to even, and clamps against the current options.
    pub fn effective_index_at(&self, time: f64) -> Result<i32, PointwiseError<E>>
    where
        E: 'static,
    {
        self.ensure_property_evaluation_supported()?;
        if self.is_animated()? {
            let value = f64::from(
                self.animation_curve(0)?
                    .sample(time as f32)
                    .map_err(AnimationError::Curve)?,
            );
            let mut index = crate::framework_cast::integer(
                round_decimal_places(value, 0).map_err(|_| ImageError::ArgumentOutOfRange)?,
            )
            .wrapping_sub(1);
            let count = self.options()?.entries().len();
            if index < 0 {
                index = 0;
            } else if index as i64 >= count as i64 {
                index = (count as i32).wrapping_sub(1);
            }
            Ok(index)
        } else if let Some(value) = self.animated_binding_value(time)? {
            let entries = self.options()?.entries();
            Ok(entries
                .iter()
                .position(|entry| entry.as_deref() == value.selection())
                .map_or_else(
                    || crate::framework_cast::integer(value.number()),
                    |index| index as i32,
                ))
        } else {
            self.index()
        }
    }
    pub fn effective_text_at(&self, time: f64) -> Result<Option<Vec<u16>>, PointwiseError<E>>
    where
        E: 'static,
    {
        let index = self.effective_index_at(time)?;
        self.options()?
            .entries()
            .get(usize::try_from(index).map_err(|_| PointwiseError::Index)?)
            .cloned()
            .ok_or(PointwiseError::Index)
    }
    pub fn add_changed_handler(&self, handler: &GradientGeneratorChangeHandler<E>) {
        let state = self.owner.state();
        let mut s = guard(&state);
        let next = s.handlers[self.kind.ix()].combine(&handler.0);
        let old = std::mem::replace(&mut s.handlers[self.kind.ix()], next);
        drop(s);
        drop(old);
    }
    pub fn remove_changed_handler(&self, handler: &GradientGeneratorChangeHandler<E>) {
        let state = self.owner.state();
        let mut s = guard(&state);
        let next = s.handlers[self.kind.ix()].remove(&handler.0);
        let old = std::mem::replace(&mut s.handlers[self.kind.ix()], next);
        drop(s);
        drop(old);
    }
}

/// Owned click-time frame: evaluated procedural operands, or a detached
/// Imported Image authority retaining its copied bytes/cache/property aliases.
/// Source Clone and live mutable authorities are never capture substitutes.
#[derive(Clone, Debug)]
pub struct GradientRenderSnapshot {
    imported: Option<imported_node::capture::ImportedFrame>,
    selection: Option<family::CapturedChoice>,
    family: Option<family::ProceduralFrame>,
    mode: Option<usize>,
    size: [f64; 2],
    location: [f64; 2],
}
impl GradientRenderSnapshot {
    pub fn render(&self, width: i32, height: i32) -> Result<SharedImageMap, PointwiseError<()>> {
        if let Some(imported) = &self.imported {
            return imported.render(width, height, crate::ProceduralExecution::CallingThread);
        }
        if self.family.is_some() {
            return Err(PointwiseError::UnsupportedCapability(
                "procedural frame requires explicit execution and preflight context",
            ));
        }
        self.render_gradient_frame(
            width,
            height,
            crate::ProceduralExecution::CallingThread,
            &|| false,
        )
    }

    fn render_gradient_frame<E>(
        &self,
        width: i32,
        height: i32,
        execution: crate::ProceduralExecution,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        super::check_render_cancelled(cancelled)?;
        let geometry = crate::ImageGeometry::new(width, height);
        let out = SharedImageMap::new(geometry);
        let destination = out.materialize(0)?;
        if let Some(mode) = self.mode {
            super::generator_kernel::render(
                mode,
                &destination,
                geometry,
                self.size,
                self.location,
                execution,
                cancelled,
            )?;
        } else {
            super::check_render_cancelled(cancelled)?;
        }
        Ok(out)
    }
}
impl<E> GradientGenerator<E> {
    fn render_operands(&self, time: f64) -> Result<GradientRenderSnapshot, PointwiseError<E>>
    where
        E: 'static,
    {
        self.ensure_property_evaluation_supported()?;
        let selected = self
            .parameter(GradientGeneratorParameterKind::Mode)
            .effective_text_at(time)?;
        let size = self.size_at(time)?;
        let mode = selected.as_deref().and_then(|v| {
            MODE_NAMES
                .iter()
                .position(|name| v.iter().copied().eq(name.encode_utf16()))
        });
        let location = if let Some(mode) = mode {
            let p = self.parameter(GradientGeneratorParameterKind::Location);
            match mode {
                0 => [p.value_at(0, time)?, 0.0],
                1 => [0.0, p.value_at(1, time)?],
                _ => self.location_at(time)?,
            }
        } else {
            [0.0; 2]
        };
        Ok(GradientRenderSnapshot {
            imported: None,
            selection: None,
            family: None,
            mode,
            size,
            location,
        })
    }
    /// Standalone frame capture. A whole scene should share RenderCaptureSession.
    /// Copies raw state first; evaluation mutates only the isolated child caches.
    pub fn capture_render(&self, time: f64) -> Result<GradientRenderSnapshot, PointwiseError<E>>
    where
        E: 'static,
    {
        super::RenderCaptureSession::new().capture_gradient(self, time)
    }
    pub(super) fn raw_capture_values(
        &self,
        comparison: &mut super::property::capture::capture_values::Comparison<E>,
    ) -> Result<Vec<u64>, PointwiseError<E>>
    where
        E: 'static,
    {
        if self.kind() == ImageGeneratorKind::ImportedImage {
            return self.imported_capture_values(comparison);
        }
        let (kind, factor, scalars, pairs, selection, animations, allow) = {
            let state = self.state();
            let s = guard(&state);
            (
                s.kind,
                s.factor,
                s.scalars.clone(),
                s.pairs.clone(),
                Selection {
                    current: s.selection.current,
                    previous: s.selection.previous,
                    options: s.selection.options.clone(),
                },
                s.animations.clone(),
                s.allow_changes,
            )
        };
        let mut out = vec![kind as u64, factor.to_bits(), pairs.len() as u64];
        for pair in pairs {
            out.extend(pair.current.map(f64::to_bits));
            out.extend(pair.previous.map(f64::to_bits));
            out.extend(pair.normalized.map(f64::to_bits));
            out.push(pair.scale.to_bits());
            comparison.limits(&pair.limits, &mut out);
        }
        for scalar in scalars {
            match scalar {
                None => out.push(0),
                Some(s) => {
                    out.extend([
                        1,
                        s.current.to_bits(),
                        s.previous.to_bits(),
                        s.modifier.to_bits(),
                        s.derived.to_bits(),
                    ]);
                    match s.limits {
                        None => out.push(0),
                        Some(l) => {
                            out.push(1);
                            comparison.limits(&l, &mut out);
                        }
                    }
                }
            }
        }
        out.extend([selection.current as u64, selection.previous as u64]);
        comparison.options(&selection.options, &mut out);
        out.extend(allow.map(u64::from));
        for a in animations {
            comparison.animation(a, &mut out)?;
        }
        let wire = guard(&self.state()).wire.clone();
        for metadata in wire {
            comparison.metadata(&metadata, &mut out)?;
        }
        Ok(out)
    }
    pub(super) fn copy_raw_with(
        &self,
        session: &mut super::RenderCaptureSession<E>,
    ) -> Result<Self, PointwiseError<E>>
    where
        E: 'static,
    {
        if self.kind() == ImageGeneratorKind::ImportedImage {
            return self.capture_imported_with(session);
        }
        let (kind, factor, mut scalars, mut pairs, selection, animations, allow_changes) = {
            let state = self.state();
            let s = guard(&state);
            (
                s.kind,
                s.factor,
                s.scalars.clone(),
                s.pairs.clone(),
                Selection {
                    current: s.selection.current,
                    previous: s.selection.previous,
                    options: s.selection.options.clone(),
                },
                s.animations.clone(),
                s.allow_changes,
            )
        };
        for pair in &mut pairs {
            pair.limits = session.copy_limits(&pair.limits);
        }
        let selection = Selection {
            options: session.copy_options(&selection.options),
            ..selection
        };
        for scalar in scalars.iter_mut().flatten() {
            scalar.limits = scalar
                .limits
                .as_ref()
                .map(|limits| session.copy_limits(limits));
        }
        let mut copies = std::array::from_fn(|_| None);
        for (ix, animation) in animations.iter().enumerate() {
            copies[ix] = animation
                .as_ref()
                .map(|a| session.copy_animation(a))
                .transpose()?;
        }
        let animations = copies;
        let source_wire = guard(&self.state()).wire.clone();
        let mut copied_wire = Vec::with_capacity(PARAMETER_COUNT);
        for metadata in source_wire {
            copied_wire.push(metadata.capture_with(|a| session.copy_adjustment(a))?);
        }
        let wire: [_; PARAMETER_COUNT] =
            copied_wire.try_into().map_err(|_| PointwiseError::Index)?;
        Ok(Self {
            node: PointwiseNode::publish_image_generator(Arc::new(Mutex::new(GeneratorState {
                kind,
                factor,
                scalars,
                pairs,
                selection,
                animations,
                wire,
                allow_changes,
                handlers: std::array::from_fn(|_| DelegateCore::Absent),
                cache: None,
                render_caches: cached_render::GradientRenderCaches::new(),
                final_clone: false,
                final_cache: final_render::FinalImageCache::default(),
                imported: None,
            }))),
        })
    }
    pub(super) fn capture_with(
        &self,
        session: &mut super::RenderCaptureSession<E>,
        time: f64,
    ) -> Result<GradientRenderSnapshot, PointwiseError<E>>
    where
        E: 'static,
    {
        let copy = session
            .copy_node_raw(&self.node())?
            .image_generator_handle()
            .ok_or(PointwiseError::InvalidCast)?;
        copy.complete_captured_operands(time)
    }
    /// Only existing detached capture owners reach this private finalization.
    pub(in crate::pointwise) fn complete_captured_operands(
        &self,
        time: f64,
    ) -> Result<GradientRenderSnapshot, PointwiseError<E>>
    where
        E: 'static,
    {
        let kind = self.kind();
        if kind == ImageGeneratorKind::ImportedImage {
            self.capture_imported_frame(time)
        } else if kind == ImageGeneratorKind::Gradient {
            let mut frame = self.render_operands(time)?;
            frame.selection = Some(family::CapturedChoice::capture(
                &self.parameter(GradientGeneratorParameterKind::Mode),
                time,
            )?);
            Ok(frame)
        } else {
            self.capture_family_operands(time)
        }
    }
}

#[cfg(test)]
#[path = "generator_family_test.rs"]
mod family_test;
