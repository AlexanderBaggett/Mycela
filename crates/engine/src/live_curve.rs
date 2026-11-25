//! Retained analytic, Noise and Custom curves. Full wire coverage and live
//! property wrappers remain separate capabilities.
mod codec;
mod custom;
pub(crate) mod frozen;
mod record_modulation;
mod wire;
pub use custom::{CustomPointHandle, CustomPointList};
mod noise;
mod shapes;
pub use noise::{CurveCache, CurveCachePoint};

use crate::{
    DefaultContext,
    curve_region::{ObjectId, Sponsor},
    ordinary_events::{DelegateCore, HandlerCore},
};
use std::{
    fmt,
    sync::{Arc, Mutex},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AnalyticCurveKind {
    Sine,
    Linear,
    Arc,
    Bell,
    Gain,
    Steps,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CurveLayerKind {
    Sine,
    Linear,
    Arc,
    Bell,
    Gain,
    Steps,
    Noise,
    Custom,
}
impl From<AnalyticCurveKind> for CurveLayerKind {
    fn from(value: AnalyticCurveKind) -> Self {
        match value {
            AnalyticCurveKind::Sine => Self::Sine,
            AnalyticCurveKind::Linear => Self::Linear,
            AnalyticCurveKind::Arc => Self::Arc,
            AnalyticCurveKind::Bell => Self::Bell,
            AnalyticCurveKind::Gain => Self::Gain,
            AnalyticCurveKind::Steps => Self::Steps,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CurveControlKind {
    Toggle,
    Scalar,
    Direction,
    Blend,
    Modulated,
}
#[derive(Debug, PartialEq)]
pub enum CurveError<E = DefaultContext> {
    NullReference,
    Index,
    ArgumentIndex,
    InvalidBlend,
    WrongControl,
    Overflow,
    Allocation,
    Binary(crate::BinaryError),
    InvalidData,
    UnsupportedCapability,
    Observer(E),
}
impl<E: fmt::Debug> fmt::Display for CurveError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl<E: fmt::Debug> std::error::Error for CurveError<E> {}
/// rl.a(Single,Single,Single,Single) numeric failure or a deliberately
/// unexecuted nonterminating increment. This does not change CurveError's
/// existing caller/control error contracts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CurveBoundsScaleError {
    Precision(crate::DecimalRoundingError),
    NonpositiveIncrement,
}
impl CurveBoundsScaleError {
    pub fn source_class(&self) -> &'static str {
        match self {
            Self::Precision(e) => e.source_class(),
            Self::NonpositiveIncrement => "UnsupportedCapability",
        }
    }
}
impl fmt::Display for CurveBoundsScaleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for CurveBoundsScaleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Precision(e) => Some(e),
            Self::NonpositiveIncrement => None,
        }
    }
}
type Events<E> = DelegateCore<Root<E>, (), CurveError<E>>;
pub struct CurveControlHandler<E = DefaultContext>(Arc<HandlerCore<Root<E>, (), CurveError<E>>>);
impl<E> Clone for CurveControlHandler<E> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}
impl<E> CurveControlHandler<E> {
    pub fn new(callback: impl Fn() -> Result<(), E> + Send + Sync + 'static) -> Self {
        Self(Arc::new(HandlerCore {
            callback: Box::new(move |_, ()| callback().map_err(CurveError::Observer)),
        }))
    }
}
/// Captured ordered callback chain; retains the emitting cohort for bound receivers.
pub struct CurveControlDelegate<E = DefaultContext> {
    events: Events<E>,
    owner: Root<E>,
}
impl<E> Clone for CurveControlDelegate<E> {
    fn clone(&self) -> Self {
        Self {
            events: self.events.clone(),
            owner: self.owner.clone(),
        }
    }
}
impl<E> CurveControlDelegate<E> {
    pub fn same_reference(&self, other: &Self) -> bool {
        self.events.same(&other.events)
    }
    pub fn invocation_count(&self) -> usize {
        self.events.entries().len()
    }
    pub fn invocations(&self) -> Vec<Self> {
        self.events
            .entries()
            .iter()
            .map(|handler| Self {
                events: Events::Single(Arc::clone(handler)),
                owner: self.owner.clone(),
            })
            .collect()
    }
    pub fn invoke(&self) -> Result<(), CurveError<E>> {
        self.events.invoke(&self.owner, ())
    }
}
impl<E> CurveControlHandler<E> {
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub fn matches_delegate(&self, other: &CurveControlDelegate<E>) -> bool {
        Events::Single(Arc::clone(&self.0)).same(&other.events)
    }
}
struct Root<E> {
    core: Arc<ObjectCore<E>>,
    sponsor: Arc<Sponsor<E>>,
}
impl<E> Clone for Root<E> {
    fn clone(&self) -> Self {
        Self {
            core: Arc::clone(&self.core),
            sponsor: Arc::clone(&self.sponsor),
        }
    }
}
impl<E> Root<E> {
    fn new(state: Object<E>) -> Self {
        let core = Arc::new(ObjectCore {
            id: ObjectId::new(),
            state: Mutex::new(state),
        });
        let sponsor = Sponsor::publish(&core);
        Self { core, sponsor }
    }
    fn at(&self, id: ObjectId) -> Self {
        Self {
            core: self.sponsor.node(id),
            sponsor: Arc::clone(&self.sponsor),
        }
    }
    fn read<T>(&self, f: impl FnOnce(&Object<E>) -> T) -> T {
        f(&self.core.state.lock().unwrap_or_else(|e| e.into_inner()))
    }
    fn write<T>(&self, f: impl FnOnce(&mut Object<E>) -> T) -> T {
        f(&mut self.core.state.lock().unwrap_or_else(|e| e.into_inner()))
    }
    fn join(&self, other: &Self) {
        self.sponsor.merge(&other.sponsor);
    }
}
pub(crate) struct ObjectCore<E> {
    pub(crate) id: ObjectId,
    state: Mutex<Object<E>>,
}
enum Object<E> {
    Curve(Composite),
    Layer(Layer),
    Control(Control<E>),
}
#[derive(Clone)]
struct Composite {
    layers: Vec<Option<ObjectId>>,
    minimum: f32,
    maximum: f32,
    wrap: bool,
}
#[derive(Clone)]
struct Layer {
    kind: CurveLayerKind,
    mode: i32,
    common: Vec<Option<ObjectId>>,
    extra: Vec<ObjectId>,
    noise: Option<noise::State>,
    custom: Option<custom::State>,
}
#[derive(Clone, Copy)]
enum Value {
    Toggle(bool),
    Scalar(f32),
    Direction(i32),
    Blend(i32),
    Modulated {
        scalar: f32,
        mode: i32,
        child: Option<ObjectId>,
    },
}
struct Control<E> {
    id: i32,
    name: Option<Vec<u16>>,
    active: bool,
    value: Value,
    bounds: Option<[f32; 2]>,
    events: Events<E>,
}
/// Retains one live composite and its connected layer/control roots.
/// Handle Clone retains identity; source_clone performs the source copy operation.
pub struct SharedCurve<E = DefaultContext>(Root<E>);
/// Retains an actual analytic layer; common controls and nested children stay live.
pub struct CurveLayerHandle<E = DefaultContext>(Root<E>);
/// Retains one actual control independently of its current layer membership.
pub struct CurveControlHandle<E = DefaultContext>(Root<E>);
macro_rules! handle {
    ($name:ident) => {
        impl<E> Clone for $name<E> {
            fn clone(&self) -> Self {
                Self(self.0.clone())
            }
        }
        impl<E> $name<E> {
            pub fn same_reference(&self, other: &Self) -> bool {
                Arc::ptr_eq(&self.0.core, &other.0.core)
            }
        }
        impl<E> fmt::Debug for $name<E> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_tuple(stringify!($name))
                    .field(&self.0.core.id)
                    .finish()
            }
        }
    };
}
handle!(SharedCurve);
handle!(CurveLayerHandle);
handle!(CurveControlHandle);
impl<E> SharedCurve<E> {
    pub fn new(minimum: f32, maximum: f32, wrap: bool) -> Self {
        Self(Root::new(Object::Curve(Composite {
            layers: Vec::new(),
            minimum,
            maximum,
            wrap,
        })))
    }
    pub fn minimum(&self) -> f32 {
        self.0.read(|s| match s {
            Object::Curve(c) => c.minimum,
            _ => unreachable!(),
        })
    }
    pub fn maximum(&self) -> f32 {
        self.0.read(|s| match s {
            Object::Curve(c) => c.maximum,
            _ => unreachable!(),
        })
    }
    pub fn is_wrapped(&self) -> bool {
        self.0.read(|s| match s {
            Object::Curve(c) => c.wrap,
            _ => unreachable!(),
        })
    }
    pub fn set_minimum(&self, value: f32) {
        self.0.write(|s| {
            if let Object::Curve(c) = s {
                c.minimum = value
            }
        });
    }
    pub fn set_maximum(&self, value: f32) {
        self.0.write(|s| {
            if let Object::Curve(c) = s {
                c.maximum = value
            }
        });
    }
    /// Actual public rl.a scale operation. Both raw f32 products publish
    /// before increment analysis, then Double Round narrows each bound in order.
    /// No layer/cache replacement or callback is invented for direct bound edits.
    pub fn scale_bounds(
        &self,
        factor: f32,
        minimum: f32,
        maximum: f32,
        increment: f32,
    ) -> Result<(), CurveBoundsScaleError> {
        self.set_minimum(self.minimum() * factor);
        self.set_maximum(self.maximum() * factor);
        // The source loop cannot progress for +/-0 or negative values. Preserve
        // its already committed products, but never hang the portable engine.
        if increment <= 0.0 {
            return Err(CurveBoundsScaleError::NonpositiveIncrement);
        }
        let mut digits = 0;
        let mut step = f64::from(increment);
        while step < 1.0 {
            step *= 10.0;
            digits += 1;
        }
        let value = crate::round_decimal_places(f64::from(self.minimum()), digits)
            .map_err(CurveBoundsScaleError::Precision)? as f32;
        self.set_minimum(value);
        let value = crate::round_decimal_places(f64::from(self.maximum()), digits)
            .map_err(CurveBoundsScaleError::Precision)? as f32;
        self.set_maximum(value);
        if self.minimum() < minimum {
            self.set_minimum(minimum);
        }
        if self.maximum() > maximum {
            self.set_maximum(maximum);
        }
        Ok(())
    }
    pub fn set_wrapped(&self, value: bool) {
        self.0.write(|s| {
            if let Object::Curve(c) = s {
                c.wrap = value
            }
        });
    }
    pub fn layers(&self) -> Vec<Option<CurveLayerHandle<E>>> {
        let ids = self.0.read(|s| match s {
            Object::Curve(c) => c.layers.clone(),
            _ => unreachable!(),
        });
        ids.into_iter()
            .map(|id| id.map(|id| CurveLayerHandle(self.0.at(id))))
            .collect()
    }
    pub fn append(&self, layers: &[Option<CurveLayerHandle<E>>]) {
        for l in layers.iter().flatten() {
            self.0.join(&l.0);
        }
        self.0.write(|s| {
            if let Object::Curve(c) = s {
                c.layers
                    .extend(layers.iter().map(|l| l.as_ref().map(|l| l.0.core.id)))
            }
        });
    }
    pub fn insert(
        &self,
        ix: i32,
        layer: Option<&CurveLayerHandle<E>>,
    ) -> Result<(), CurveError<E>> {
        let len = self.layers().len();
        let ix = usize::try_from(ix)
            .ok()
            .filter(|&i| i <= len)
            .ok_or(CurveError::ArgumentIndex)?;
        if let Some(l) = layer {
            self.0.join(&l.0);
        }
        self.0.write(|s| {
            if let Object::Curve(c) = s {
                c.layers.insert(ix, layer.map(|l| l.0.core.id))
            }
        });
        Ok(())
    }
    pub fn remove(&self, ix: i32) -> Result<(), CurveError<E>> {
        self.0.write(|s| {
            let Object::Curve(c) = s else { unreachable!() };
            let ix = usize::try_from(ix)
                .ok()
                .filter(|&i| i < c.layers.len())
                .ok_or(CurveError::ArgumentIndex)?;
            c.layers.remove(ix);
            Ok(())
        })
    }
    pub fn clear(&self) {
        self.0.write(|s| {
            if let Object::Curve(c) = s {
                c.layers.clear()
            }
        });
    }
    pub fn source_clone(&self) -> Result<Self, CurveError<E>> {
        let result = Self::new(self.minimum(), self.maximum(), self.is_wrapped());
        for l in self.layers() {
            let l = l.ok_or(CurveError::NullReference)?.source_clone()?;
            result.append(&[Some(l)]);
        }
        Ok(result)
    }
    /// Apply the preset's layers to a copy of this composite's range and wrapping.
    pub(crate) fn with_preset_layers(
        &self,
        preset: &Self,
        insert: bool,
    ) -> Result<Self, CurveError<E>> {
        let next = preset.source_clone()?;
        next.set_minimum(self.minimum());
        next.set_maximum(self.maximum());
        next.set_wrapped(self.is_wrapped());
        if insert {
            next.append(&self.layers());
        }
        Ok(next)
    }
    pub fn sample(&self, time: f32) -> Result<f32, CurveError<E>> {
        let layers = self.layers();
        let Some(last) = layers.last() else {
            return Ok(self.minimum());
        };
        let mut value = last
            .as_ref()
            .ok_or(CurveError::NullReference)?
            .sample(time)?;
        for layer in layers[..layers.len() - 1].iter().rev() {
            let layer = layer.as_ref().ok_or(CurveError::NullReference)?;
            let strength = layer.common(13)?.evaluate(time)?;
            if strength != 0.0 {
                let blend = layer.common(12)?;
                // rl.c explicitly casts cn; Direction has an adjacent enum payload
                // but is a different original concrete control.
                if blend.kind() != CurveControlKind::Blend {
                    return Err(CurveError::NullReference);
                }
                let mode = blend.enumeration()?;
                let b = match mode {
                    0 => layer.sample(time)?,
                    1 => add(value, layer.sample(time)?),
                    2 => mul(value, layer.sample(time)?),
                    3 => add(0.5, mul(sub(value, 0.5), layer.sample(time)?)),
                    4 => add(1.0, mul(sub(value, 1.0), layer.sample(time)?)),
                    5 => min(value, layer.sample(time)?),
                    6 => max(value, layer.sample(time)?),
                    7 => layer.sample(value)?,
                    _ => return Err(CurveError::InvalidBlend),
                };
                value = add(mul(strength, b), mul(sub(1.0, strength), value));
            }
        }
        value = add(
            self.minimum(),
            mul(sub(self.maximum(), self.minimum()), value),
        );
        if self.is_wrapped() {
            let low = min(self.minimum(), self.maximum());
            let high = max(self.minimum(), self.maximum());
            if value > high {
                value = add(low, rem(sub(value, high), sub(high, low)));
            } else if value < low {
                value = add(high, rem(sub(value, high), sub(high, low)));
            }
        }
        Ok(value)
    }
}
impl<E> CurveLayerHandle<E> {
    pub fn new(kind: AnalyticCurveKind) -> Self {
        Self::structure(kind.into())
    }
    fn structure(kind: CurveLayerKind) -> Self {
        let root = Root::new(Object::Layer(Layer {
            kind,
            mode: 2,
            common: Vec::new(),
            extra: Vec::new(),
            noise: (kind == CurveLayerKind::Noise).then(noise::State::default),
            custom: (kind == CurveLayerKind::Custom).then(custom::State::default),
        }));
        let mut controls = Vec::new();
        let definitions = [
            (1, "Mirror", Value::Toggle(false)),
            (2, "Frequency", Value::Scalar(1.0)),
            (3, "Initial Delay", Value::Scalar(0.0)),
            (4, "Repeat Delay", Value::Scalar(0.0)),
            (5, "Mirror Delay", Value::Scalar(0.0)),
            (
                6,
                "Phase",
                Value::Modulated {
                    scalar: 0.0,
                    mode: 0,
                    child: None,
                },
            ),
            (
                7,
                "Amplitude",
                Value::Modulated {
                    scalar: 100.0,
                    mode: 2,
                    child: None,
                },
            ),
            (
                8,
                "Shift",
                Value::Modulated {
                    scalar: 50.0,
                    mode: 2,
                    child: None,
                },
            ),
            (
                9,
                "Remap X",
                Value::Modulated {
                    scalar: 0.0,
                    mode: 0,
                    child: None,
                },
            ),
            (
                10,
                "Remap Y",
                Value::Modulated {
                    scalar: 0.0,
                    mode: 2,
                    child: None,
                },
            ),
            (11, "Minimum", Value::Scalar(0.0)),
            (12, "Maximum", Value::Scalar(100.0)),
            (13, "Mode", Value::Blend(0)),
            (
                14,
                "Strength",
                Value::Modulated {
                    scalar: 100.0,
                    mode: 1,
                    child: None,
                },
            ),
        ];
        for (id, name, value) in definitions {
            let control = CurveControlHandle::make(id, name.encode_utf16().collect(), false, value);
            root.join(&control.0);
            controls.push(Some(control.0.core.id));
        }
        let definitions = match kind {
            CurveLayerKind::Linear => vec![(15, "Direction", Value::Direction(0))],
            CurveLayerKind::Arc => vec![(16, "Curve", Value::Scalar(75.0))],
            CurveLayerKind::Bell => vec![
                (
                    17,
                    "Width",
                    Value::Modulated {
                        scalar: 50.0,
                        mode: 1,
                        child: None,
                    },
                ),
                (
                    18,
                    "Skew",
                    Value::Modulated {
                        scalar: 50.0,
                        mode: 1,
                        child: None,
                    },
                ),
            ],
            CurveLayerKind::Gain => vec![(
                19,
                "Gain",
                Value::Modulated {
                    scalar: 75.0,
                    mode: 1,
                    child: None,
                },
            )],
            CurveLayerKind::Steps => vec![
                (26, "Steps", Value::Scalar(4.0)),
                (
                    27,
                    "Smoothness",
                    Value::Modulated {
                        scalar: 50.0,
                        mode: 1,
                        child: None,
                    },
                ),
            ],
            CurveLayerKind::Noise => vec![
                (20, "Base Frequency", Value::Scalar(10.0)),
                (
                    21,
                    "Gain",
                    Value::Modulated {
                        scalar: 50.0,
                        mode: 1,
                        child: None,
                    },
                ),
                (
                    22,
                    "Jitter Amplitude",
                    Value::Modulated {
                        scalar: 0.0,
                        mode: 1,
                        child: None,
                    },
                ),
                (23, "Jitter Frequency", Value::Scalar(10.0)),
                (24, "Seed", Value::Scalar(0.0)),
                (25, "Loop", Value::Toggle(true)),
            ],
            CurveLayerKind::Sine | CurveLayerKind::Custom => Vec::new(),
        };
        let mut extra = Vec::new();
        for (id, name, value) in definitions {
            let control = CurveControlHandle::make(id, name.encode_utf16().collect(), false, value);
            root.join(&control.0);
            extra.push(control.0.core.id);
        }
        root.write(|s| {
            if let Object::Layer(l) = s {
                l.common = controls;
                l.extra = extra;
            }
        });
        Self(root)
    }
    fn common(&self, ix: usize) -> Result<CurveControlHandle<E>, CurveError<E>> {
        self.control_at(ix)?.ok_or(CurveError::NullReference)
    }
    // qy.c's explicit ac3 casts do not accept ef even though both expose
    // a scalar payload. Raw getters and virtual Save/Load keep base-aox dispatch.
    fn common_scalar(&self, ix: usize) -> Result<f32, CurveError<E>> {
        let control = self.common(ix)?;
        if control.kind() != CurveControlKind::Scalar {
            return Err(CurveError::NullReference);
        }
        control.scalar()
    }
    /// Read an actual nullable source slot without dereferencing its value.
    pub fn control_at(&self, ix: usize) -> Result<Option<CurveControlHandle<E>>, CurveError<E>> {
        let id = self.0.read(|s| match s {
            Object::Layer(l) => l.common.get(ix).copied().ok_or(CurveError::Index),
            _ => unreachable!(),
        })?;
        Ok(id.map(|id| CurveControlHandle(self.0.at(id))))
    }
    /// qy.b(aox): raw Frequency slot assignment, with no event/subscription edits.
    pub fn set_frequency(&self, value: Option<&CurveControlHandle<E>>) {
        if let Some(value) = value {
            self.0.join(&value.0);
        }
        let old = self.0.write(|s| match s {
            Object::Layer(l) => std::mem::replace(&mut l.common[1], value.map(|v| v.0.core.id)),
            _ => unreachable!(),
        });
        let _ = old;
    }
    pub fn kind(&self) -> CurveLayerKind {
        self.0.read(|s| match s {
            Object::Layer(l) => l.kind,
            _ => unreachable!(),
        })
    }
    pub fn mode(&self) -> i32 {
        self.0.read(|s| match s {
            Object::Layer(l) => l.mode,
            _ => unreachable!(),
        })
    }
    pub fn set_mode(&self, mode: i32) {
        self.0.write(|s| {
            if let Object::Layer(l) = s {
                l.mode = mode
            }
        });
    }
    /// Nullable entries of the actual retained source control list.
    pub fn controls(&self) -> Vec<Option<CurveControlHandle<E>>> {
        let ids = self.0.read(|s| match s {
            Object::Layer(l) => l.common.clone(),
            _ => unreachable!(),
        });
        ids.into_iter()
            .map(|id| id.map(|id| CurveControlHandle(self.0.at(id))))
            .collect()
    }
    /// Source FindIndex reads entries sequentially; an early match avoids later nulls.
    pub fn control(&self, id: i32) -> Result<Option<CurveControlHandle<E>>, CurveError<E>> {
        for ix in 0..14 {
            let control = self.common(ix)?;
            if control.id() == id {
                return Ok(Some(control));
            }
        }
        Ok(None)
    }
    /// The first declared shape control, preserving the original single-control API.
    pub fn shape_control(&self) -> Option<CurveControlHandle<E>> {
        self.shape_controls().into_iter().next()
    }
    /// Declared shape controls in constructor order: Bell Width, Skew; Steps
    /// Count, Smoothness. Common control(id) intentionally does not search here.
    pub fn shape_controls(&self) -> Vec<CurveControlHandle<E>> {
        self.0
            .read(|s| match s {
                Object::Layer(l) => l.extra.clone(),
                _ => unreachable!(),
            })
            .into_iter()
            .map(|id| CurveControlHandle(self.0.at(id)))
            .collect()
    }
    pub fn replace_control(&self, control: &CurveControlHandle<E>) -> Result<bool, CurveError<E>> {
        let id = control.id();
        for ix in 0..14 {
            if self.common(ix)?.id() == id {
                self.0.join(&control.0);
                self.0.write(|s| {
                    if let Object::Layer(l) = s {
                        l.common[ix] = Some(control.0.core.id);
                    }
                });
                return Ok(true);
            }
        }
        Ok(false)
    }
    pub fn source_clone(&self) -> Result<Self, CurveError<E>> {
        let result = Self::structure(self.kind());
        for ix in 0..14 {
            let old = self.common(ix)?;
            let new = result.common(ix)?;
            if [5, 6, 7, 8, 9, 13].contains(&ix) {
                let cloned = old.source_clone()?;
                result.0.join(&cloned.0);
                result.0.write(|s| {
                    if let Object::Layer(l) = s {
                        l.common[ix] = Some(cloned.0.core.id)
                    }
                });
            } else {
                new.copy_value(&old)?;
            }
        }
        result.set_mode(self.mode());
        for (ix, old) in self.shape_controls().into_iter().enumerate() {
            let new = if old.kind() == CurveControlKind::Modulated {
                old.source_clone()?
            } else {
                let new = result.shape_controls()[ix].clone();
                new.copy_value(&old)?;
                new
            };
            result.0.join(&new.0);
            result.0.write(|s| {
                if let Object::Layer(l) = s {
                    l.extra[ix] = new.0.core.id;
                }
            });
        }
        if result.kind() == CurveLayerKind::Custom {
            result.copy_custom(self)?;
        }
        if result.kind() == CurveLayerKind::Noise {
            result.bind_noise();
            result.rebuild_noise(false)?;
        }
        Ok(result)
    }
    // Preserve qy ordered comparisons: RangeInclusive negation changes NaN branches.
    #[allow(clippy::manual_clamp, clippy::manual_range_contains)]
    pub fn sample(&self, input: f32) -> Result<f32, CurveError<E>> {
        let mut time = input;
        if self.common(8)?.is_active() {
            time = self.common(8)?.evaluate(input)?;
        }
        let mut delayed = false;
        let delay = mul(0.01, self.common_scalar(2)?);
        if time > delay {
            time = div(sub(time, delay), sub(1.0, delay));
        } else {
            time = 0.0;
            delayed = true;
        }
        let phase = self.common(5)?.evaluate(input)?;
        time = add(time, phase);
        if time > 1.0 {
            time = sub(time, 1.0);
        }
        if 0.0 < time && time < 1.0 {
            time = mul(time, libm::round(f64::from(self.common_scalar(1)?)) as f32);
            time = sub(time, (f64::from(time).trunc()) as f32);
        }
        let repeat = mul(0.01, self.common_scalar(3)?);
        time = if time < sub(1.0, repeat) {
            div(time, sub(1.0, repeat))
        } else {
            1.0
        };
        if self.common(0)?.toggle()? {
            let mirror = mul(0.01, self.common_scalar(4)?);
            let first = div(sub(1.0, mirror), 2.0);
            let second = sub(1.0, first);
            time = if time < first {
                div(time, first)
            } else if time > second {
                div(sub(1.0, time), sub(1.0, second))
            } else {
                1.0
            };
        }
        let mut value = match self.kind() {
            CurveLayerKind::Noise => self.sample_noise(time)?,
            CurveLayerKind::Custom => self.sample_custom(time)?,
            CurveLayerKind::Sine => mul(
                (libm::sin(std::f64::consts::PI * 2.0 * f64::from(time)) + 1.0) as f32,
                0.5,
            ),
            CurveLayerKind::Linear => {
                if self
                    .shape_control()
                    .expect("Linear direction")
                    .enumeration()?
                    == 0
                {
                    time
                } else {
                    sub(1.0, time)
                }
            }
            CurveLayerKind::Arc => {
                shapes::arc(time, self.shape_control().expect("Arc curve").scalar()?)
            }
            CurveLayerKind::Bell => {
                // aeq.h1 resolves Skew before Width at the same original time.
                let controls = self.shape_controls();
                let skew = controls[1].evaluate(time)?;
                let width = controls[0].evaluate(time)?;
                shapes::bell(time, skew, width)
            }
            CurveLayerKind::Steps => {
                let controls = self.shape_controls();
                let count = controls[0].scalar()?;
                shapes::steps(time, count, || controls[1].evaluate(time))?
            }
            CurveLayerKind::Gain => gain(
                time,
                mul(
                    self.shape_control().expect("Gain control").evaluate(time)?,
                    100.0,
                ),
            ),
        };
        let mut original = if delayed { delay } else { input };
        original = div(sub(original, delay), sub(1.0, delay));
        let mut derived = add(original, phase);
        if derived > 1.0 {
            derived = sub(derived, 1.0);
        }
        value = mul(value, self.common(6)?.evaluate(derived)?);
        value = add(
            value,
            mul(2.0, sub(self.common(7)?.evaluate(derived)?, 0.5)),
        );
        if self.common(9)?.is_active() {
            if value > 1.0 {
                value = 1.0;
            }
            if value < 0.0 {
                value = 0.0;
            }
            value = self.common(9)?.evaluate(value)?;
        }
        let low = div(self.common_scalar(10)?, 100.0);
        let high = div(self.common_scalar(11)?, 100.0);
        value = add(low, mul(value, sub(high, low)));
        match self.mode() {
            0 => {
                if value < 0.0 || value > 1.0 {
                    value = sub(value, (f64::from(value).floor()) as f32);
                }
            }
            1 => {
                if value > 1.0 {
                    value = 1.0;
                } else if value < 0.0 {
                    value = 0.0;
                }
            }
            _ => {}
        }
        Ok(value)
    }
}
impl<E> CurveControlHandle<E> {
    fn make(id: i32, name: Vec<u16>, active: bool, value: Value) -> Self {
        Self(Root::new(Object::Control(Control {
            id,
            name: Some(name),
            active,
            value,
            bounds: match value {
                Value::Scalar(_) => Some(match id {
                    20 => [1.0, 1000.0],
                    23 | 2 | 26 => [1.0, 100.0],
                    24 => [0.0, 1_000_000.0],
                    _ => [0.0, 100.0],
                }),
                Value::Modulated { .. } => Some([0.0, 100.0]),
                _ => None,
            },
            events: Events::Absent,
        })))
    }
    pub fn new_toggle(id: i32, name: Vec<u16>, value: bool) -> Self {
        Self::make(id, name, false, Value::Toggle(value))
    }
    fn value(&self) -> Value {
        self.0.read(|s| match s {
            Object::Control(c) => c.value,
            _ => unreachable!(),
        })
    }
    pub fn bounds(&self) -> Option<[f32; 2]> {
        self.0.read(|s| match s {
            Object::Control(c) => c.bounds,
            _ => unreachable!(),
        })
    }
    pub fn id(&self) -> i32 {
        self.0.read(|s| match s {
            Object::Control(c) => c.id,
            _ => unreachable!(),
        })
    }
    pub fn name(&self) -> Option<Vec<u16>> {
        self.0.read(|s| match s {
            Object::Control(c) => c.name.clone(),
            _ => unreachable!(),
        })
    }
    /// Direct public aox.g assignment, retaining exact null/UTF16 and emitting no event.
    pub fn set_name(&self, name: Option<Vec<u16>>) {
        let old = self.0.write(|s| match s {
            Object::Control(c) => std::mem::replace(&mut c.name, name),
            _ => unreachable!(),
        });
        drop(old);
    }
    pub fn is_active(&self) -> bool {
        self.0.read(|s| match s {
            Object::Control(c) => c.active,
            _ => unreachable!(),
        })
    }
    /// Source public activation-field assignment emits no event.
    pub fn set_active(&self, value: bool) {
        self.0.write(|s| {
            if let Object::Control(c) = s {
                c.active = value
            }
        });
    }
    pub fn kind(&self) -> CurveControlKind {
        match self.value() {
            Value::Toggle(_) => CurveControlKind::Toggle,
            Value::Scalar(_) => CurveControlKind::Scalar,
            Value::Direction(_) => CurveControlKind::Direction,
            Value::Blend(_) => CurveControlKind::Blend,
            Value::Modulated { .. } => CurveControlKind::Modulated,
        }
    }
    pub fn scalar(&self) -> Result<f32, CurveError<E>> {
        match self.value() {
            Value::Scalar(v) | Value::Modulated { scalar: v, .. } => Ok(v),
            _ => Err(CurveError::NullReference),
        }
    }
    pub fn enumeration(&self) -> Result<i32, CurveError<E>> {
        match self.value() {
            Value::Direction(v) | Value::Blend(v) => Ok(v),
            _ => Err(CurveError::NullReference),
        }
    }
    pub fn toggle(&self) -> Result<bool, CurveError<E>> {
        match self.value() {
            Value::Toggle(v) => Ok(v),
            _ => Err(CurveError::NullReference),
        }
    }
    fn edit(
        &self,
        f: impl FnOnce(&mut Value) -> Result<(), CurveError<E>>,
    ) -> Result<(), CurveError<E>> {
        let events = self.0.write(|s| {
            let Object::Control(c) = s else {
                unreachable!()
            };
            f(&mut c.value)?;
            Ok::<_, CurveError<E>>(c.events.clone())
        })?;
        events.invoke(&self.0, ())
    }
    pub fn set_scalar(&self, v: f32) -> Result<(), CurveError<E>> {
        self.edit(|value| match value {
            Value::Scalar(x) | Value::Modulated { scalar: x, .. } => {
                *x = v;
                Ok(())
            }
            _ => Err(CurveError::WrongControl),
        })
    }
    pub fn set_enumeration(&self, v: i32) -> Result<(), CurveError<E>> {
        self.edit(|value| match value {
            Value::Direction(x) | Value::Blend(x) => {
                *x = v;
                Ok(())
            }
            _ => Err(CurveError::WrongControl),
        })
    }
    pub fn set_toggle(&self, v: bool) -> Result<(), CurveError<E>> {
        self.edit(|value| match value {
            Value::Toggle(x) => {
                *x = v;
                Ok(())
            }
            _ => Err(CurveError::WrongControl),
        })
    }
    pub fn retained_child(&self) -> Option<CurveLayerHandle<E>> {
        match self.value() {
            Value::Modulated { child, .. } => child.map(|id| CurveLayerHandle(self.0.at(id))),
            _ => None,
        }
    }
    pub fn modulation_mode(&self) -> Option<i32> {
        match self.value() {
            Value::Modulated { mode, .. } => Some(mode),
            _ => None,
        }
    }
    pub fn set_child(&self, child: Option<&CurveLayerHandle<E>>) -> Result<(), CurveError<E>> {
        let mode = self.modulation_mode().ok_or(CurveError::WrongControl)?;
        if let Some(c) = child {
            self.0.join(&c.0);
        }
        self.0.write(|s| {
            if let Object::Control(Control {
                value: Value::Modulated { child: slot, .. },
                ..
            }) = s
            {
                *slot = child.map(|c| c.0.core.id)
            }
        });
        child.ok_or(CurveError::NullReference)?.set_mode(mode);
        let events = self.0.read(|s| match s {
            Object::Control(c) => c.events.clone(),
            _ => unreachable!(),
        });
        events.invoke(&self.0, ())
    }
    fn attach_loaded_child(&self, child: &CurveLayerHandle<E>) {
        self.0.join(&child.0);
        self.0.write(|s| {
            if let Object::Control(Control {
                value: Value::Modulated { child: slot, .. },
                ..
            }) = s
            {
                *slot = Some(child.0.core.id);
            }
        });
    }

    pub fn clear_child(&self) -> Result<(), CurveError<E>> {
        self.0.write(|s| {
            let Object::Control(c) = s else {
                unreachable!()
            };
            let Value::Modulated { child, .. } = &mut c.value else {
                return Err(CurveError::WrongControl);
            };
            *child = None;
            Ok(())
        })
    }
    pub fn evaluate(&self, time: f32) -> Result<f32, CurveError<E>> {
        if self.kind() != CurveControlKind::Modulated {
            return Err(CurveError::NullReference);
        }
        if self.is_active() {
            self.retained_child()
                .ok_or(CurveError::NullReference)?
                .sample(time)
        } else {
            Ok(div(self.scalar()?, 100.0))
        }
    }
    pub fn changed_delegate(&self) -> Option<CurveControlDelegate<E>> {
        let events = self.0.read(|s| match s {
            Object::Control(c) => c.events.clone(),
            _ => unreachable!(),
        });
        (!events.is_absent()).then(|| CurveControlDelegate {
            events,
            owner: self.0.clone(),
        })
    }
    pub fn add_changed(&self, handler: &CurveControlHandler<E>) {
        let h = Events::Single(Arc::clone(&handler.0));
        let old = self.0.write(|s| {
            let Object::Control(c) = s else {
                unreachable!()
            };
            let next = c.events.combine(&h);
            std::mem::replace(&mut c.events, next)
        });
        drop(old);
    }
    pub fn remove_changed(&self, handler: &CurveControlHandler<E>) {
        let h = Events::Single(Arc::clone(&handler.0));
        let old = self.0.write(|s| {
            let Object::Control(c) = s else {
                unreachable!()
            };
            let next = c.events.remove(&h);
            std::mem::replace(&mut c.events, next)
        });
        drop(old);
    }
    fn copy_value(&self, source: &Self) -> Result<(), CurveError<E>> {
        let value = source.value();
        self.0.write(|s| {
            let Object::Control(c) = s else {
                unreachable!()
            };
            if std::mem::discriminant(&c.value) != std::mem::discriminant(&value) {
                return Err(CurveError::NullReference);
            }
            c.value = value;
            Ok(())
        })
    }
    fn source_clone(&self) -> Result<Self, CurveError<E>> {
        let Value::Modulated { scalar, mode, .. } = self.value() else {
            return Err(CurveError::NullReference);
        };
        let child = self
            .retained_child()
            .map(|c| c.source_clone())
            .transpose()?;
        let cloned = Self::make(
            self.id(),
            Vec::new(),
            self.is_active(),
            Value::Modulated {
                scalar,
                mode,
                child: child.as_ref().map(|c| c.0.core.id),
            },
        );
        cloned.set_name(self.name());
        if let Some(c) = child {
            cloned.0.join(&c.0);
        }
        Ok(cloned)
    }
}
// Keep each f32 operation explicit. The differential replay covers raw payloads;
// no tolerance, fused operation or broad NaN normalization is permitted.
fn add(a: f32, b: f32) -> f32 {
    a + b
}
fn sub(a: f32, b: f32) -> f32 {
    a - b
}
fn mul(a: f32, b: f32) -> f32 {
    a * b
}
fn div(a: f32, b: f32) -> f32 {
    a / b
}
fn rem(a: f32, b: f32) -> f32 {
    a % b
}
fn min(a: f32, b: f32) -> f32 {
    if a < b || a.is_nan() { a } else { b }
}
fn max(a: f32, b: f32) -> f32 {
    if a > b || a.is_nan() { a } else { b }
}
fn gain(mut t: f32, g: f32) -> f32 {
    if g == 0.0 {
        return 0.5;
    }
    if g == 50.0 {
        return t;
    }
    if g == 100.0 {
        return if t == 0.5 {
            0.5
        } else if t > 0.5 {
            1.0
        } else {
            0.0
        };
    }
    let mut n = g;
    if g > 50.0 {
        n = sub(100.0, n);
    }
    n = div(n, sub(100.0, mul(2.0, n)));
    if t < 0.5 {
        t = mul(t, 2.0);
        if g < 50.0 {
            t = sub(1.0, t);
            div(sub(1.0, div(mul(n, t), sub(add(1.0, n), t))), 2.0)
        } else {
            div(div(mul(n, t), sub(add(1.0, n), t)), 2.0)
        }
    } else {
        t = mul(sub(t, 0.5), 2.0);
        if g < 50.0 {
            t = sub(1.0, t);
            add(div(sub(1.0, div(mul(t, add(n, 1.0)), add(t, n))), 2.0), 0.5)
        } else {
            add(div(div(mul(t, add(n, 1.0)), add(t, n)), 2.0), 0.5)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn bell_derived_controls_keep_separate_copy_paths_for_shared_children() {
        // Source-body law: aeq copy invokes ef copy separately for Width and
        // Skew. The runtime11-case corpus activates Width only; this is not a
        // new source runtime observation for active Skew cloning.
        let layer = CurveLayerHandle::<()>::new(AnalyticCurveKind::Bell);
        let controls = layer.shape_controls();
        assert_eq!(
            controls.iter().map(|c| c.id()).collect::<Vec<_>>(),
            [17, 18]
        );
        assert!(!controls[0].same_reference(&controls[1]));
        assert!(layer.control(17).unwrap().is_none());
        assert!(layer.control(18).unwrap().is_none());
        let shared = CurveLayerHandle::new(AnalyticCurveKind::Arc);
        for control in &controls {
            control.set_active(true);
            control.set_child(Some(&shared)).unwrap();
        }
        let cloned = layer.source_clone().unwrap();
        let copied = cloned.shape_controls();
        for (old, new) in controls.iter().zip(&copied) {
            assert!(!old.same_reference(new));
            assert!(new.is_active());
            assert!(!new.retained_child().unwrap().same_reference(&shared));
        }
        assert!(
            !copied[0]
                .retained_child()
                .unwrap()
                .same_reference(&copied[1].retained_child().unwrap())
        );
    }

    // Ownership/class checks, not additional original numerical observations.
    #[test]
    fn clone_nominal_modulator_wrong_kind_has_source_null_failure() {
        // qy copy casts its fixed ef slots with `as`; ef(null) dereferences
        // that null. This extends the existing wrong-common-control input as
        // a source-body law, not as a new original runtime observation.
        let layer = CurveLayerHandle::<()>::new(AnalyticCurveKind::Linear);
        let wrong = CurveControlHandle::new_toggle(6, "Phase".encode_utf16().collect(), false);
        assert!(layer.replace_control(&wrong).unwrap());
        assert!(matches!(
            layer.source_clone(),
            Err(CurveError::NullReference)
        ));
    }

    #[test]
    fn retained_control_sponsors_removed_layer_and_internal_cycle_reclaims() {
        let layer = CurveLayerHandle::<()>::new(AnalyticCurveKind::Linear);
        let weak = Arc::downgrade(&layer.0.core);
        let control = layer.control(6).unwrap().unwrap();
        control.set_active(true);
        control.set_child(Some(&layer)).unwrap();
        let curve = SharedCurve::new(0.0, 1.0, false);
        curve.append(&[Some(layer.clone())]);
        curve.clear();
        drop(layer);
        drop(curve);
        let retained = control.retained_child().unwrap();
        assert!(Arc::ptr_eq(&retained.0.core, &weak.upgrade().unwrap()));
        drop(retained);
        drop(control);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn scalar_commit_reentry_and_typed_failure_use_actual_control() {
        let layer = CurveLayerHandle::new(AnalyticCurveKind::Linear);
        let control = layer.control(2).unwrap().unwrap();
        let values = Arc::new(Mutex::new(Vec::new()));
        let once = Arc::new(AtomicBool::new(false));
        let nested = control.clone();
        let observed = values.clone();
        let handler = CurveControlHandler::new(move || {
            observed
                .lock()
                .unwrap()
                .push(nested.scalar().unwrap().to_bits());
            if !once.swap(true, Ordering::SeqCst) {
                nested.set_scalar(7.0).map_err(|_| "nested")?;
            }
            Ok(())
        });
        let throwing = CurveControlHandler::new(|| Err("typed observer"));
        control.add_changed(&handler);
        control.add_changed(&throwing);
        let result = control.set_scalar(37.5);
        assert_eq!(result, Err(CurveError::Observer("nested")));
        assert_eq!(control.scalar().unwrap().to_bits(), 7.0f32.to_bits());
        assert_eq!(
            *values.lock().unwrap(),
            [37.5f32.to_bits(), 7.0f32.to_bits()]
        );
        control.remove_changed(&handler);
        control.remove_changed(&throwing);
    }
}
