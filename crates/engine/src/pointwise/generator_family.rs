//! Retained os scalar authority and family constructor profiles. Values here
//! are genuine constructor arguments, never original after-state observations.
use super::super::parameter_ops::seed_modifier;
use super::*;
use crate::{LimitsArithmeticProfile, ProceduralExecution};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageGeneratorKind {
    Gradient,
    Noise,
    Cells,
    WoodRings,
    BricksBoards,
    ImportedImage,
}
impl ImageGeneratorKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Gradient => "Gradient",
            Self::Noise => "Noise",
            Self::Cells => "Cells",
            Self::WoodRings => "Wood Rings",
            Self::BricksBoards => "Bricks & Boards",
            Self::ImportedImage => "Imported Image",
        }
    }
    pub(super) fn parameters(self) -> &'static [GradientGeneratorParameterKind] {
        use GradientGeneratorParameterKind::*;
        match self {
            Self::Gradient => &[Size, Location, Mode],
            Self::ImportedImage => &[Size, Location, Image, ForceSquareRatio, UpsizeSmoothing],
            Self::Noise => &[Size, Location, Mode, Seed, Iterations],
            Self::Cells => &[
                Size,
                Location,
                Mode,
                Density,
                Seed,
                Irregularity,
                Distribution,
            ],
            Self::WoodRings => &[Size, Location, Density, Anomalies, Seed, BlendWidth],
            Self::BricksBoards => &[
                Size,
                Location,
                BlockDensity,
                BorderWidth,
                Shifts,
                WidthVariation,
                SlideVariation,
                Orientation,
            ],
        }
    }
}
#[derive(Clone)]
pub(super) struct Scalar {
    pub(super) current: f64,
    pub(super) previous: f64,
    pub(super) modifier: f64,
    pub(super) derived: f64,
    pub(super) limits: Option<SharedLimits>,
}
fn decimal(value: i32, scale: u32) -> Result<DecimalBits, ImageError> {
    DecimalBits::new([
        value.unsigned_abs(),
        0,
        0,
        (scale << 16) | if value < 0 { 0x80000000 } else { 0 },
    ])
    .map_err(|_| ImageError::Argument)
}
fn range(
    min: i32,
    max: i32,
    places: i32,
    multiplier: f64,
    flags: [bool; 3],
) -> Result<SharedRangeOptionsRecord, ImageError> {
    Ok(SharedRangeOptionsRecord::new(RangeOptionsRecord {
        minimum: dec(min)?,
        maximum: dec(max)?,
        decimal_places: places,
        multiplier,
        flags,
    }))
}
fn curve_range(min: i32, max: i32) -> Result<SharedCurveRangeRecord, ImageError> {
    Ok(SharedCurveRangeRecord::new(CurveRangeRecord {
        minimum: dec(min)?,
        maximum: dec(max)?,
        wrap_y: false,
    }))
}
#[allow(clippy::too_many_arguments)] // Literal source constructor overload fields.
fn limits(
    places: i32,
    increment: DecimalBits,
    min: i32,
    max: i32,
    use_range: Option<SharedRangeOptionsRecord>,
    curve_range: Option<SharedCurveRangeRecord>,
) -> Result<SharedLimits, ImageError> {
    SharedLimits::from_definition_with_profile(
        LimitsDefinition {
            decimal_places: places,
            step: LimitsStep::Derived(increment),
            minimum: dec(min)?,
            maximum: dec(max)?,
            used: true,
            use_range,
            curve_range,
        },
        LimitsArithmeticProfile::Framework64OleAut32TenFirst,
    )
    .map_err(|_| ImageError::Overflow)
}
impl Scalar {
    fn new(current: f64, limits: SharedLimits) -> Self {
        // os ctor invokes the value setter before assigning its Limits field.
        Self {
            current,
            previous: current,
            modifier: 0.0,
            derived: seed_modifier(current, 0.0, false),
            limits: Some(limits),
        }
    }

    pub(super) fn set_current(&mut self, value: f64) {
        self.current = value;
        let negative = self
            .limits
            .as_ref()
            .is_some_and(|l| l.snapshot().minimum.to_framework_f64() < 0.0);
        self.derived = seed_modifier(value, self.modifier, negative);
    }
}
fn scalar_limits(
    kind: ImageGeneratorKind,
    parameter: GradientGeneratorParameterKind,
) -> Result<SharedLimits, ImageError> {
    use GradientGeneratorParameterKind::*;
    match parameter {
        Seed => limits(
            0,
            dec(1)?,
            -1_000_001,
            1_000_000,
            Some(range(0, 1000, 3, 1.0, [true, true, true])?),
            None,
        ),
        Iterations => limits(0, dec(1)?, 1, 10, None, None),
        Density if kind == ImageGeneratorKind::Cells => limits(
            0,
            dec(1)?,
            1,
            100,
            Some(range(1, 3, 0, 1.0, [false, false, true])?),
            Some(curve_range(1, 10)?),
        ),
        Density => limits(
            3,
            decimal(1, 1)?,
            -1_000_000,
            1_000_000,
            Some(range(-100, 100, 3, 0.1, [false, true, true])?),
            Some(curve_range(0, 5)?),
        ),
        Anomalies => limits(
            0,
            dec(1)?,
            1,
            100,
            Some(range(1, 3, 0, 1.0, [false, false, true])?),
            Some(curve_range(1, 10)?),
        ),
        Irregularity => limits(
            3,
            decimal(1, 1)?,
            -100,
            100,
            Some(range(-10, 10, 3, 0.1, [false, true, true])?),
            Some(curve_range(-3, 3)?),
        ),
        Distribution => limits(3, decimal(1, 2)?, 0, 1, None, None),
        BorderWidth => limits(
            3,
            decimal(1, 2)?,
            0,
            1,
            Some(range(0, 1, 3, 0.3, [false, false, true])?),
            None,
        ),
        Shifts => limits(
            3,
            dec(1)?,
            -1_000_000,
            1_000_000,
            Some(range(-30, 30, 0, 1.0, [false, false, true])?),
            Some(curve_range(-20, 20)?),
        ),
        WidthVariation => limits(
            3,
            dec(1)?,
            0,
            1_000_000,
            Some(range(0, 5, 0, 1.0, [false, false, true])?),
            Some(curve_range(0, 10)?),
        ),
        SlideVariation => limits(
            3,
            dec(1)?,
            -1_000_000,
            1_000_000,
            Some(range(-5, 5, 3, 1.0, [false, false, true])?),
            Some(curve_range(0, 10)?),
        ),
        BlendWidth => limits(
            3,
            dec(1)?,
            -1_000_000,
            1_000_000,
            Some(range(-10, 10, 3, 0.06, [false, false, true])?),
            Some(curve_range(0, 50)?),
        ),
        _ => Err(ImageError::Argument),
    }
}
impl<E> GeneratorState<E> {
    pub(super) fn with_kind(kind: ImageGeneratorKind) -> Result<Self, ImageError> {
        let size_limits = if matches!(
            kind,
            ImageGeneratorKind::Gradient
                | ImageGeneratorKind::BricksBoards
                | ImageGeneratorKind::ImportedImage
        ) {
            pair_limits(false, true)?
        } else {
            limits(
                3,
                dec(1)?,
                -1_000_000,
                1_000_000,
                Some(range(-10, 10, 0, 1.0, [false, false, true])?),
                Some(curve_range(0, 10)?),
            )?
        };
        let location_limits = if matches!(
            kind,
            ImageGeneratorKind::Gradient
                | ImageGeneratorKind::BricksBoards
                | ImageGeneratorKind::ImportedImage
        ) {
            pair_limits(true, true)?
        } else {
            limits(
                3,
                decimal(1, 1)?,
                -1_000_000,
                1_000_000,
                Some(range(-10, 10, 0, 1.0, [false, false, true])?),
                Some(curve_range(0, 10)?),
            )?
        };
        let mut state = Self {
            kind,
            scalars: std::array::from_fn(|_| None),
            factor: 1.0,
            pairs: vec![
                Pair {
                    current: [1.0; 2],
                    previous: [1.0; 2],
                    normalized: [1.0; 2],
                    scale: 1.0,
                    limits: size_limits,
                },
                Pair {
                    current: [0.0; 2],
                    previous: [0.0; 2],
                    normalized: [0.0; 2],
                    scale: 1.0,
                    limits: location_limits,
                },
            ],
            selection: Selection {
                current: 0,
                previous: 0,
                options: SharedSelectionOptions::new(Vec::new()),
            },
            handlers: std::array::from_fn(|_| DelegateCore::Absent),
            allow_changes: [true; PARAMETER_COUNT],
            animations: std::array::from_fn(|_| None),
            wire: std::array::from_fn(|ix| {
                super::super::property::wire::LegacyWireMetadata::new(
                    true,
                    ix != GradientGeneratorParameterKind::Image.ix(),
                )
            }),
            cache: None,
            render_caches: super::cached_render::GradientRenderCaches::new(),
            final_clone: false,
            final_cache: super::final_render::FinalImageCache::default(),
            imported: if kind == ImageGeneratorKind::ImportedImage {
                Some(imported_node::ImportedState::new())
            } else {
                None
            },
        };
        if kind == ImageGeneratorKind::BricksBoards {
            let limits = limits(
                0,
                dec(1)?,
                1,
                1_000_000,
                Some(range(1, 20, 0, 1.0, [false, false, true])?),
                Some(curve_range(1, 30)?),
            )?;
            state.pairs.push(Pair {
                current: [8.0, 3.0],
                previous: [8.0, 3.0],
                normalized: [0.0, 0.0],
                scale: 1.0,
                limits,
            });
        }
        let size = match kind {
            ImageGeneratorKind::Cells => [4.0, 4.0],
            ImageGeneratorKind::WoodRings => [8.0, 1.0],
            _ => [1.0, 1.0],
        };
        state.pairs[0].current = size;
        state.pairs[0].previous = size;
        state.pairs[0].normalized = size;
        let names: &[&str] = match kind {
            ImageGeneratorKind::Gradient => &MODE_NAMES,
            ImageGeneratorKind::Noise => &["None", "Linear", "Cosine", "Cubic"],
            ImageGeneratorKind::Cells => &[
                "Cells",
                "Bubbles",
                "Jagged Cells",
                "Circles",
                "Squares",
                "Square Bubbles",
                "Manhattan",
            ],
            ImageGeneratorKind::BricksBoards => &["Horizontal", "Vertical"],
            _ => &[],
        };
        let index = if kind == ImageGeneratorKind::Noise {
            3
        } else {
            0
        };
        state.selection = Selection {
            current: index,
            previous: index,
            options: SharedSelectionOptions::new(
                names
                    .iter()
                    .map(|s| Some(s.encode_utf16().collect()))
                    .collect(),
            ),
        };
        for parameter in kind.parameters() {
            if matches!(
                parameter,
                GradientGeneratorParameterKind::Size
                    | GradientGeneratorParameterKind::Location
                    | GradientGeneratorParameterKind::Mode
                    | GradientGeneratorParameterKind::BlockDensity
                    | GradientGeneratorParameterKind::Orientation
                    | GradientGeneratorParameterKind::Image
                    | GradientGeneratorParameterKind::ForceSquareRatio
                    | GradientGeneratorParameterKind::UpsizeSmoothing
            ) {
                continue;
            }
            let value = match parameter {
                GradientGeneratorParameterKind::Seed
                | GradientGeneratorParameterKind::Irregularity => 0.0,
                GradientGeneratorParameterKind::BlendWidth
                | GradientGeneratorParameterKind::Shifts => 3.0,
                GradientGeneratorParameterKind::BorderWidth => 0.03,
                GradientGeneratorParameterKind::WidthVariation
                | GradientGeneratorParameterKind::SlideVariation => 0.0,
                _ => 1.0,
            };
            state.scalars[parameter.ix()] =
                Some(Scalar::new(value, scalar_limits(kind, *parameter)?));
        }
        Ok(state)
    }
}
impl<E> GradientGeneratorParameter<E> {
    pub(super) fn scalar(&self) -> Result<Scalar, PointwiseError<E>> {
        guard(&self.owner.state()).scalars[self.kind.ix()]
            .clone()
            .ok_or(PointwiseError::NullReference)
    }
    pub fn retained_limits(&self) -> Result<Option<SharedLimits>, PointwiseError<E>> {
        if let Ok(ix) = self.pair_ix() {
            return Ok(Some(guard(&self.owner.state()).pairs[ix].limits.clone()));
        }
        Ok(self.scalar()?.limits)
    }
    pub fn number(&self) -> Result<f64, PointwiseError<E>> {
        Ok(self.scalar()?.current)
    }
    pub fn previous_number(&self) -> Result<f64, PointwiseError<E>> {
        Ok(self.scalar()?.previous)
    }
    pub fn modifier(&self) -> Result<f64, PointwiseError<E>> {
        Ok(self.scalar()?.modifier)
    }
    pub fn derived_seed(&self) -> Result<f64, PointwiseError<E>> {
        Ok(self.scalar()?.derived)
    }
    pub(super) fn rounded_scalar(&self, value: f64) -> Result<f64, PointwiseError<E>> {
        let Some(limits) = self.scalar()?.limits else {
            return Ok(value);
        };
        let limits = limits.snapshot();
        let value = round_decimal_places(value, limits.decimal_places)
            .map_err(|_| ImageError::ArgumentOutOfRange)?;
        Ok(min64(
            limits.maximum.to_framework_f64(),
            max64(limits.minimum.to_framework_f64(), value),
        ))
    }
    pub(in crate::pointwise) fn write_bound_number(
        &self,
        value: f64,
        force: bool,
    ) -> Result<bool, PointwiseError<E>> {
        let value = self.rounded_scalar(value)?;
        let state = self.owner.state();
        let mut state = guard(&state);
        let scalar = state.scalars[self.kind.ix()]
            .as_mut()
            .ok_or(PointwiseError::NullReference)?;
        if scalar.current == value && !force {
            return Ok(false);
        }
        scalar.previous = value;
        scalar.set_current(value);
        Ok(true)
    }
    pub fn set_number(&self, value: f64) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        self.assign_number(value, false)
    }
    /// os.SetValueSpeed commits current but deliberately preserves prior.
    pub fn set_number_speed(&self, value: f64) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        self.assign_number(value, true)
    }
    fn assign_number(&self, value: f64, speed: bool) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        let value = self.rounded_scalar(value)?;
        let state = self.owner.state();
        let mut state = guard(&state);
        let scalar = state.scalars[self.kind.ix()]
            .as_mut()
            .ok_or(PointwiseError::NullReference)?;
        if scalar.current == value && (speed || scalar.previous == scalar.current) {
            return Ok(());
        }
        let old = scalar.previous;
        if !speed {
            scalar.previous = value;
        }
        scalar.set_current(value);
        let handlers = state.handlers[self.kind.ix()].clone();
        drop(state);
        self.owner.notify(
            self.kind,
            handlers,
            if speed {
                Arc::new(ContextChangeArgument::Property(
                    ContextPropertyChangeArgs::with_speed(true),
                ))
            } else {
                self.ordinary_edit_args(|name| {
                    let undo = crate::DoubleUndoInfo::new(Some(name), 0, old, value);
                    ContextChangeArgument::Property(ContextPropertyChangeArgs::with_undo(Some(
                        Arc::new(undo),
                    )))
                })?
            },
        )
    }
    /// os.aa commits modifier first, then derived state, then Empty args.
    pub fn set_modifier(&self, value: f64) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        let state = self.owner.state();
        let mut state = guard(&state);
        let scalar = state.scalars[self.kind.ix()]
            .as_mut()
            .ok_or(PointwiseError::NullReference)?;
        if scalar.modifier == value {
            return Ok(());
        }
        scalar.modifier = value;
        let negative = scalar
            .limits
            .as_ref()
            .is_some_and(|l| l.snapshot().minimum.to_framework_f64() < 0.0);
        scalar.derived = seed_modifier(scalar.current, value, negative);
        let handlers = state.handlers[self.kind.ix()].clone();
        drop(state);
        self.owner
            .notify(self.kind, handlers, ContextChangeArgument::shared_empty())
    }
    /// Actual ordinary os.o/a getter. Scalar integer precision floors AFTER
    /// sampling; this differs from paired Round and performs no output clamp.
    pub fn effective_value_at(&self, time: f64) -> Result<f64, PointwiseError<E>>
    where
        E: 'static,
    {
        self.ensure_property_evaluation_supported()?;
        self.scalar()?;
        if let Some(value) = self.animated_binding_value(time)? {
            return Ok(value.number());
        }
        super::super::parameter_ops::scalar_value(
            || self.retained_animation(),
            || self.limits(),
            || self.number(),
            time,
        )
    }
    /// os.i/y bypasses ordinary ancestor evaluation while inactive.
    pub fn effective_seed_at(&self, time: f64) -> Result<f64, PointwiseError<E>>
    where
        E: 'static,
    {
        self.ensure_property_evaluation_supported()?;
        if self.is_animated()? || self.animated_binding_value(time)?.is_some() {
            self.effective_value_at(time)
        } else {
            self.derived_seed()
        }
    }
    pub fn multiply_number(&self, factor: f64) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        self.scalar()?;
        if self.is_animated()? {
            let curve = self.animation_curve(0)?;
            let minimum = self.limits()?.snapshot().minimum.to_framework_f32();
            let maximum = self.limits()?.snapshot().maximum.to_framework_f32();
            let increment = self.limits()?.snapshot().increment.to_framework_f32();
            curve
                .scale_bounds(factor as f32, minimum, maximum, increment)
                .map_err(AnimationError::<E>::Bounds)?;
            Ok(())
        } else {
            self.set_number(self.number()? * factor)
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct CapturedChoice {
    index: i32,
    entries: Vec<Option<Vec<u16>>>,
}
impl CapturedChoice {
    pub(super) fn capture<E: 'static>(
        parameter: &GradientGeneratorParameter<E>,
        time: f64,
    ) -> Result<Self, PointwiseError<E>> {
        Ok(Self {
            index: parameter.effective_index_at(time)?,
            entries: parameter.options()?.entries(),
        })
    }
    fn text<E>(&self) -> Result<Option<Vec<u16>>, PointwiseError<E>> {
        self.entries
            .get(usize::try_from(self.index).map_err(|_| PointwiseError::Index)?)
            .cloned()
            .ok_or(PointwiseError::Index)
    }
}
#[derive(Clone, Debug)]
pub(super) enum ProceduralFrame {
    Noise {
        size: [f64; 2],
        location: [f64; 2],
        seed: f64,
        iterations: f64,
        smoothing: CapturedChoice,
    },
    Cells {
        size: [f64; 2],
        location: [f64; 2],
        density: f64,
        seed: f64,
        irregularity: f64,
        distribution: f64,
        mode: CapturedChoice,
    },
    Bricks {
        size: [f64; 2],
        location: [f64; 2],
        resolved: [f64; 4],
        width_variation: f64,
        slide_variation: f64,
        orientation: CapturedChoice,
        factor: f64,
    },
    Wood {
        size: [f64; 2],
        location: [f64; 2],
        density: f64,
        anomalies: f64,
        seed: f64,
        blend: f64,
    },
}
impl GradientRenderSnapshot {
    /// Exact owned render data comparison used to reject obsolete frames after
    /// silent source edits. This is neither model identity nor source Equals.
    pub fn same_render_values(&self, other: &Self) -> bool {
        match (&self.imported, &other.imported) {
            (Some(a), Some(b)) => return a.same_render_values(b),
            (Some(_), None) | (None, Some(_)) => return false,
            _ => {}
        }
        fn pair(a: [f64; 2], b: [f64; 2]) -> bool {
            a.map(f64::to_bits) == b.map(f64::to_bits)
        }
        fn numbers(a: &[f64], b: &[f64]) -> bool {
            a.iter().zip(b).all(|(a, b)| a.to_bits() == b.to_bits()) && a.len() == b.len()
        }
        fn choice(a: &CapturedChoice, b: &CapturedChoice) -> bool {
            a.index == b.index && a.entries == b.entries
        }
        match (&self.family, &other.family) {
            (None, None) => {
                self.mode == other.mode
                    && pair(self.size, other.size)
                    && pair(self.location, other.location)
                    && match (&self.selection, &other.selection) {
                        (Some(a), Some(b)) => choice(a, b),
                        (None, None) => true,
                        _ => false,
                    }
            }
            (
                Some(ProceduralFrame::Noise {
                    size: a,
                    location: b,
                    seed: c,
                    iterations: d,
                    smoothing: e,
                }),
                Some(ProceduralFrame::Noise {
                    size: f,
                    location: g,
                    seed: h,
                    iterations: i,
                    smoothing: j,
                }),
            ) => pair(*a, *f) && pair(*b, *g) && numbers(&[*c, *d], &[*h, *i]) && choice(e, j),
            (
                Some(ProceduralFrame::Cells {
                    size: a,
                    location: b,
                    density: c,
                    seed: d,
                    irregularity: e,
                    distribution: f,
                    mode: g,
                }),
                Some(ProceduralFrame::Cells {
                    size: h,
                    location: i,
                    density: j,
                    seed: k,
                    irregularity: l,
                    distribution: m,
                    mode: n,
                }),
            ) => {
                pair(*a, *h)
                    && pair(*b, *i)
                    && numbers(&[*c, *d, *e, *f], &[*j, *k, *l, *m])
                    && choice(g, n)
            }
            (
                Some(ProceduralFrame::Wood {
                    size: a,
                    location: b,
                    density: c,
                    anomalies: d,
                    seed: e,
                    blend: f,
                }),
                Some(ProceduralFrame::Wood {
                    size: g,
                    location: h,
                    density: i,
                    anomalies: j,
                    seed: k,
                    blend: l,
                }),
            ) => pair(*a, *g) && pair(*b, *h) && numbers(&[*c, *d, *e, *f], &[*i, *j, *k, *l]),
            (
                Some(ProceduralFrame::Bricks {
                    size: a,
                    location: b,
                    resolved: c,
                    width_variation: d,
                    slide_variation: e,
                    orientation: f,
                    factor: g,
                }),
                Some(ProceduralFrame::Bricks {
                    size: h,
                    location: i,
                    resolved: j,
                    width_variation: k,
                    slide_variation: l,
                    orientation: m,
                    factor: n,
                }),
            ) => {
                pair(*a, *h)
                    && pair(*b, *i)
                    && numbers(c, j)
                    && numbers(&[*d, *e, *g], &[*k, *l, *n])
                    && choice(f, m)
            }
            _ => false,
        }
    }
    pub fn kind(&self) -> ImageGeneratorKind {
        if self.imported.is_some() {
            return ImageGeneratorKind::ImportedImage;
        }
        match &self.family {
            None => ImageGeneratorKind::Gradient,
            Some(ProceduralFrame::Noise { .. }) => ImageGeneratorKind::Noise,
            Some(ProceduralFrame::Cells { .. }) => ImageGeneratorKind::Cells,
            Some(ProceduralFrame::Wood { .. }) => ImageGeneratorKind::WoodRings,
            Some(ProceduralFrame::Bricks { .. }) => ImageGeneratorKind::BricksBoards,
        }
    }
    /// Owned evaluated frame: source capture is a separate caller operation,
    /// not the original k8 allocation/property exception-order contract.
    pub fn render_with_preflight<E>(
        &self,
        width: i32,
        height: i32,
        execution: ProceduralExecution,
        preflight: &mut impl FnMut(i32, i32) -> Result<(), PointwiseError<E>>,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        self.render_with_preflight_and_cancel(width, height, execution, preflight, &|| false)
    }

    /// Render an owned frame with a borrowed job predicate. Analytic Gradient
    /// workers poll during pixel writes. Other generators poll on entry.
    pub fn render_with_preflight_and_cancel<E>(
        &self,
        width: i32,
        height: i32,
        execution: ProceduralExecution,
        preflight: &mut impl FnMut(i32, i32) -> Result<(), PointwiseError<E>>,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        use crate::{CellsRenderOptions, NoiseOptions, WoodRingsRenderOptions};
        super::super::check_render_cancelled(cancelled)?;
        if let Some(imported) = &self.imported {
            return imported.render(width, height, execution);
        }
        match &self.family {
            Some(ProceduralFrame::Bricks {
                size,
                location,
                resolved,
                width_variation,
                slide_variation,
                orientation,
                ..
            }) => {
                let text = orientation.text()?;
                crate::render_bricks(
                    &crate::BricksRenderOptions::new(
                        *size,
                        *location,
                        [resolved[0], resolved[1]],
                        resolved[2],
                        resolved[3],
                        execution,
                    )
                    .with_variations(*width_variation, *slide_variation)
                    .with_orientation_text(text.as_deref()),
                    crate::ImageGeometry::new(width, height),
                )
            }
            None => self.render_gradient_frame(width, height, execution, cancelled),
            Some(ProceduralFrame::Noise {
                size,
                location,
                seed,
                iterations,
                smoothing,
            }) => crate::render_noise_with_preflight(
                NoiseOptions::new(
                    *size,
                    *location,
                    *seed,
                    *iterations,
                    smoothing.index,
                    execution,
                ),
                width,
                height,
                preflight,
            ),
            Some(ProceduralFrame::Cells {
                size,
                location,
                density,
                seed,
                irregularity,
                distribution,
                mode,
            }) => crate::procedural_cells::render_cells_with_inputs(
                crate::ImageGeometry::new(width, height),
                || {
                    Ok(CellsRenderOptions::new(*size, *location, execution)
                        .with_density(*density)
                        .with_seed(*seed)
                        .with_irregularity(*irregularity)
                        .with_distribution(*distribution))
                },
                &mut || mode.text(),
            ),
            Some(ProceduralFrame::Wood {
                size,
                location,
                density,
                anomalies,
                seed,
                blend,
            }) => crate::render_wood_rings(
                &WoodRingsRenderOptions::new(*size, *location, execution)
                    .with_density(*density)
                    .with_anomalies(*anomalies)
                    .with_seed(*seed)
                    .with_blend_width(*blend),
                crate::ImageGeometry::new(width, height),
            ),
        }
    }
}
impl<E: 'static> GradientGenerator<E> {
    pub(super) fn capture_family_operands(
        &self,
        time: f64,
    ) -> Result<GradientRenderSnapshot, PointwiseError<E>> {
        use GradientGeneratorParameterKind::*;
        if self.kind() == ImageGeneratorKind::ImportedImage {
            return Err(PointwiseError::UnsupportedCapability(
                "Imported Image owned encoded/cache operand capture",
            ));
        }
        let number = |kind| self.parameter(kind).effective_value_at(time);
        let seed = if self.kind() == ImageGeneratorKind::BricksBoards {
            0.0
        } else {
            self.parameter(Seed).effective_seed_at(time)?
        };
        let family = match self.kind() {
            ImageGeneratorKind::BricksBoards => ProceduralFrame::Bricks {
                resolved: self.bricks_resolved_values(self.factor(), time)?,
                orientation: CapturedChoice::capture(&self.parameter(Orientation), time)?,
                size: self.size_at(time)?,
                location: self.location_at(time)?,
                width_variation: number(WidthVariation)?,
                slide_variation: number(SlideVariation)?,
                factor: self.factor(),
            },
            ImageGeneratorKind::Noise => ProceduralFrame::Noise {
                size: self.size_at(time)?,
                location: self.location_at(time)?,
                seed,
                iterations: number(Iterations)?,
                smoothing: CapturedChoice::capture(&self.parameter(Mode), time)?,
            },
            ImageGeneratorKind::Cells => ProceduralFrame::Cells {
                size: self.size_at(time)?,
                location: self.location_at(time)?,
                density: number(Density)?,
                seed,
                irregularity: number(Irregularity)?,
                distribution: number(Distribution)?,
                mode: CapturedChoice::capture(&self.parameter(Mode), time)?,
            },
            ImageGeneratorKind::WoodRings => ProceduralFrame::Wood {
                size: self.size_at(time)?,
                location: self.location_at(time)?,
                density: number(Density)?,
                anomalies: number(Anomalies)?,
                seed,
                blend: number(BlendWidth)?,
            },
            ImageGeneratorKind::Gradient => return self.render_operands(time),
            ImageGeneratorKind::ImportedImage => {
                return Err(PointwiseError::UnsupportedCapability(
                    "Imported Image owned encoded/cache render capture",
                ));
            }
        };
        Ok(GradientRenderSnapshot {
            imported: None,
            selection: None,
            family: Some(family),
            mode: None,
            size: [0.0; 2],
            location: [0.0; 2],
        })
    }
    /// Original k8 property reads occur inside the leaf AFTER actual image
    /// allocation/materialization; Mode is invoked at the true late body seam.
    pub fn render_with_preflight(
        &self,
        width: i32,
        height: i32,
        time: f64,
        execution: ProceduralExecution,
        preflight: &mut impl FnMut(i32, i32) -> Result<(), PointwiseError<E>>,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        use crate::{CellsRenderOptions, ImageGeometry, NoiseOptions, WoodRingsRenderOptions};
        use GradientGeneratorParameterKind::*;
        let number = |kind| self.parameter(kind).effective_value_at(time);
        match self.kind() {
            ImageGeneratorKind::BricksBoards => {
                self.render_bricks_retained(width, height, time, execution)
            }
            ImageGeneratorKind::Gradient => {
                self.render_gradient(width, height, time, execution, &|| false)
            }
            ImageGeneratorKind::ImportedImage => self.render_imported(
                width,
                height,
                time,
                super::super::PointwiseRenderOptions::new(false, false),
                execution,
            ),
            ImageGeneratorKind::Noise => crate::procedural_noise::render_noise_with_inputs(
                width,
                height,
                || {
                    let size = self.size_at(time)?;
                    let location = self.location_at(time)?;
                    let seed = self.parameter(Seed).effective_seed_at(time)?;
                    let iterations = number(Iterations)?;
                    Ok(NoiseOptions::new(
                        size, location, seed, iterations, 0, execution,
                    ))
                },
                &mut || self.parameter(Mode).effective_index_at(time),
                preflight,
            ),
            ImageGeneratorKind::Cells => crate::procedural_cells::render_cells_with_inputs(
                ImageGeometry::new(width, height),
                || {
                    let seed = self.parameter(Seed).effective_seed_at(time)?;
                    let density = number(Density)?;
                    let size = self.size_at(time)?;
                    let location = self.location_at(time)?;
                    let distribution = number(Distribution)?;
                    let irregularity = number(Irregularity)?;
                    Ok(CellsRenderOptions::new(size, location, execution)
                        .with_seed(seed)
                        .with_density(density)
                        .with_distribution(distribution)
                        .with_irregularity(irregularity))
                },
                &mut || self.parameter(Mode).effective_text_at(time),
            ),
            ImageGeneratorKind::WoodRings => crate::procedural_cells::render_wood_with_inputs(
                ImageGeometry::new(width, height),
                || {
                    let seed = self.parameter(Seed).effective_seed_at(time)?;
                    let blend = number(BlendWidth)?;
                    let density = number(Density)?;
                    let size = self.size_at(time)?;
                    let location = self.location_at(time)?;
                    let anomalies = number(Anomalies)?;
                    Ok(WoodRingsRenderOptions::new(size, location, execution)
                        .with_seed(seed)
                        .with_blend_width(blend)
                        .with_density(density)
                        .with_anomalies(anomalies))
                },
            ),
        }
    }
}
