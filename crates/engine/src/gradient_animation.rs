//! Retained animation arrays, actual Limits-bound curve creation and sequential wire IO.
//! AnimationRecord remains a detached wire schema. Object identity hash and full
//! outer property-time/context integration remain explicit capabilities.
use crate::{
    BinaryCursor, BinaryError, BinaryWriter, CurveError, DefaultContext, GradientError,
    SharedCurve, SharedGradient, SharedLimits,
};
use std::{
    fmt,
    sync::{Arc, Mutex, MutexGuard},
};
fn drop_value<T>(value: T) {
    drop(value)
}
fn guard<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
#[derive(Debug, PartialEq)]
pub enum AnimationError<E = DefaultContext> {
    NullReference,
    Index,
    Overflow,
    Allocation,
    Binary(BinaryError),
    Gradient(GradientError),
    Curve(CurveError<E>),
    Bounds(crate::CurveBoundsScaleError),
}
impl<E> AnimationError<E> {
    pub(crate) fn is_unsupported_capability(&self) -> bool {
        matches!(
            self,
            Self::Curve(CurveError::UnsupportedCapability)
                | Self::Bounds(crate::CurveBoundsScaleError::NonpositiveIncrement)
        )
    }
    /// Preserve the actual nested failure rather than converting an internal
    /// Curve/codec failure into an arbitrary external observer error.
    pub fn source_class(&self) -> &'static str {
        match self {
            Self::NullReference => "NullReferenceException",
            Self::Index => "IndexOutOfRangeException",
            Self::Overflow => "OverflowException",
            Self::Allocation => "unproved-allocation-error",
            Self::Binary(e) => e.source_class(),
            Self::Gradient(e) => e.source_class(),
            Self::Bounds(e) => e.source_class(),
            Self::Curve(e) => match e {
                CurveError::NullReference => "NullReferenceException",
                CurveError::Index => "IndexOutOfRangeException",
                CurveError::ArgumentIndex => "ArgumentOutOfRangeException",
                CurveError::InvalidBlend | CurveError::InvalidData => "Exception",
                CurveError::Overflow => "OverflowException",
                CurveError::Binary(e) => e.source_class(),
                CurveError::Observer(_) => "ObserverError",
                CurveError::UnsupportedCapability => "UnsupportedCapability",
                CurveError::WrongControl => "unproved-Rust-kind-error",
                CurveError::Allocation => "unproved-allocation-error",
            },
        }
    }
}
impl<E: fmt::Debug> fmt::Display for AnimationError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl<E: fmt::Debug + 'static> std::error::Error for AnimationError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Binary(e) => Some(e),
            Self::Gradient(e) => Some(e),
            Self::Curve(e) => Some(e),
            Self::Bounds(e) => Some(e),
            _ => None,
        }
    }
}
impl<E> From<BinaryError> for AnimationError<E> {
    fn from(error: BinaryError) -> Self {
        Self::Binary(error)
    }
}
impl<E> From<GradientError> for AnimationError<E> {
    fn from(e: GradientError) -> Self {
        Self::Gradient(e)
    }
}
macro_rules! array {
    ($name:ident,$value:ty) => {
        #[derive(Clone)]
        pub struct $name(Arc<Mutex<Box<[$value]>>>);
        impl $name {
            pub fn from_values(values: Vec<$value>) -> Self {
                Self(Arc::new(Mutex::new(values.into_boxed_slice())))
            }
            pub fn len(&self) -> usize {
                guard(&self.0).len()
            }
            pub fn is_empty(&self) -> bool {
                self.len() == 0
            }
            pub fn same_reference(&self, other: &Self) -> bool {
                Arc::ptr_eq(&self.0, &other.0)
            }
            pub fn value(&self, index: i32) -> Result<$value, AnimationError> {
                guard(&self.0)
                    .get(index as usize)
                    .cloned()
                    .ok_or(AnimationError::Index)
            }
            pub fn set(&self, index: i32, value: $value) -> Result<(), AnimationError> {
                let old = {
                    let mut a = guard(&self.0);
                    std::mem::replace(
                        a.get_mut(index as usize).ok_or(AnimationError::Index)?,
                        value,
                    )
                };
                drop_value(old);
                Ok(())
            }
        }
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(
                    f,
                    concat!(stringify!($name), "({:p})"),
                    Arc::as_ptr(&self.0)
                )
            }
        }
    };
}
array!(AnimationFlags, bool);
array!(AnimationLimits, Option<SharedLimits>);
type CurveSlots<E> = Box<[Option<SharedCurve<E>>]>;
pub struct AnimationCurves<E = DefaultContext>(Arc<Mutex<CurveSlots<E>>>);
impl<E> Clone for AnimationCurves<E> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}
impl<E> fmt::Debug for AnimationCurves<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AnimationCurves({:p})", Arc::as_ptr(&self.0))
    }
}
impl<E> AnimationCurves<E> {
    pub fn from_values(values: Vec<Option<SharedCurve<E>>>) -> Self {
        Self(Arc::new(Mutex::new(values.into_boxed_slice())))
    }
    pub fn len(&self) -> usize {
        guard(&self.0).len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub fn value(&self, index: i32) -> Result<Option<SharedCurve<E>>, AnimationError<E>> {
        guard(&self.0)
            .get(index as usize)
            .cloned()
            .ok_or(AnimationError::Index)
    }
    pub fn set(&self, index: i32, value: Option<SharedCurve<E>>) -> Result<(), AnimationError<E>> {
        let old = {
            let mut a = guard(&self.0);
            std::mem::replace(
                a.get_mut(index as usize).ok_or(AnimationError::Index)?,
                value,
            )
        };
        drop_value(old);
        Ok(())
    }
}
struct State<E> {
    count: i32,
    flags: Option<AnimationFlags>,
    curves: Option<AnimationCurves<E>>,
    limits: Option<AnimationLimits>,
    gradient: Option<SharedGradient>,
}
pub struct GradientAnimation<E = DefaultContext>(Arc<Mutex<State<E>>>);
impl<E> Clone for GradientAnimation<E> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}
impl<E> fmt::Debug for GradientAnimation<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "GradientAnimation({:p})", Arc::as_ptr(&self.0))
    }
}
impl<E: 'static> GradientAnimation<E> {
    pub fn new(count: i32, limits: Option<SharedLimits>) -> Result<Self, AnimationError<E>> {
        Self::with_limits_array(count, Some(AnimationLimits::from_values(vec![limits])))
    }
    pub fn with_limits_array(
        count: i32,
        limits: Option<AnimationLimits>,
    ) -> Result<Self, AnimationError<E>> {
        if count < 0 {
            return Err(AnimationError::Overflow);
        }
        let mut flags = Vec::new();
        flags
            .try_reserve_exact(count as usize)
            .map_err(|_| AnimationError::Allocation)?;
        flags.resize(count as usize, false);
        let mut curves = Vec::new();
        curves
            .try_reserve_exact(count as usize)
            .map_err(|_| AnimationError::Allocation)?;
        curves.resize_with(count as usize, || None);
        Ok(Self(Arc::new(Mutex::new(State {
            count,
            flags: Some(AnimationFlags::from_values(flags)),
            curves: Some(AnimationCurves::from_values(curves)),
            limits,
            gradient: None,
        }))))
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub fn count(&self) -> i32 {
        guard(&self.0).count
    }
    pub fn flags(&self) -> Option<AnimationFlags> {
        guard(&self.0).flags.clone()
    }
    pub fn curves(&self) -> Option<AnimationCurves<E>> {
        guard(&self.0).curves.clone()
    }
    pub fn limits(&self) -> Option<AnimationLimits> {
        guard(&self.0).limits.clone()
    }
    pub fn retained_gradient(&self) -> Option<SharedGradient> {
        guard(&self.0).gradient.clone()
    }
    pub fn set_flags(&self, value: Option<AnimationFlags>) {
        let old = std::mem::replace(&mut guard(&self.0).flags, value);
        drop(old);
    }
    pub fn set_curves(&self, value: Option<AnimationCurves<E>>) {
        let old = std::mem::replace(&mut guard(&self.0).curves, value);
        drop(old);
    }
    pub fn set_gradient(&self, value: Option<SharedGradient>) {
        let old = std::mem::replace(&mut guard(&self.0).gradient, value);
        drop(old);
    }
    pub fn gradient(&self) -> Result<SharedGradient, AnimationError<E>> {
        if let Some(g) = self.retained_gradient() {
            return Ok(g);
        }
        let g = SharedGradient::new();
        self.set_gradient(Some(g.clone()));
        g.add_normalized_color(0.0, 0.0, 0.0, 0.0)?;
        g.add_normalized_color(1.0, 1.0, 1.0, 1.0)?;
        Ok(g)
    }
    pub(crate) fn sample_axis(&self, axis: i32, time: f64) -> Result<f32, AnimationError<E>> {
        self.curves()
            .ok_or(AnimationError::NullReference)?
            .value(axis)?
            .ok_or(AnimationError::NullReference)?
            .sample(time as f32)
            .map_err(AnimationError::Curve)
    }

    /// os.q samples a percentage along its retained Color palette.
    pub(crate) fn sample_color(&self, time: f64) -> Result<[f64; 3], AnimationError<E>> {
        let position = (f64::from(self.sample_axis(0, time)?) / 100.0).clamp(0.0, 1.0);
        let gradient = self.gradient()?;
        gradient.rebuild()?;
        let [_, red, green, blue] = gradient.sample(position)?.argb();
        Ok([red, green, blue])
    }
    /// Read the actual retained Limits array with bh's one-slot fallback.
    /// A null array returns null before any index check; an empty array errors.
    pub fn limits_for(&self, index: i32) -> Result<Option<SharedLimits>, AnimationError<E>> {
        let Some(limits) = self.limits() else {
            return Ok(None);
        };
        limits
            .value(if limits.len() <= 1 { 0 } else { index })
            .map_err(|_| AnimationError::Index)
    }
    fn required_limits(&self, index: i32) -> Result<SharedLimits, AnimationError<E>> {
        self.limits_for(index)?.ok_or(AnimationError::NullReference)
    }
    /// Create one real empty curve only when the current slot is null.
    /// Bounds come from actual source constructor/getter inputs, never a DTO.
    /// No owner/array guard crosses child reads, conversion or publication.
    pub fn ensure_curve(&self, index: i32) -> Result<(), AnimationError<E>> {
        if self
            .curves()
            .ok_or(AnimationError::NullReference)?
            .value(index)?
            .is_some()
        {
            return Ok(());
        }
        let minimum = self
            .required_limits(index)?
            .snapshot()
            .minimum
            .to_framework_f32();
        let maximum = self
            .required_limits(index)?
            .snapshot()
            .maximum
            .to_framework_f32();
        let curve = SharedCurve::new(minimum, maximum, false);
        let range = self.required_limits(index)?.snapshot().curve_range;
        if range.is_some() {
            let range = self
                .required_limits(index)?
                .snapshot()
                .curve_range
                .ok_or(AnimationError::NullReference)?;
            curve.set_minimum(range.copy_wire_value().minimum.to_framework_f32());
            let range = self
                .required_limits(index)?
                .snapshot()
                .curve_range
                .ok_or(AnimationError::NullReference)?;
            curve.set_maximum(range.copy_wire_value().maximum.to_framework_f32());
            let range = self
                .required_limits(index)?
                .snapshot()
                .curve_range
                .ok_or(AnimationError::NullReference)?;
            curve.set_wrapped(range.copy_wire_value().wrap_y);
        } else if self
            .required_limits(index)?
            .snapshot()
            .use_range
            .ok_or(AnimationError::NullReference)?
            .copy_wire_value()
            .flags[1]
        {
            let range = self
                .required_limits(index)?
                .snapshot()
                .use_range
                .ok_or(AnimationError::NullReference)?;
            curve.set_minimum(range.copy_wire_value().minimum.to_framework_f32());
            let range = self
                .required_limits(index)?
                .snapshot()
                .use_range
                .ok_or(AnimationError::NullReference)?;
            curve.set_maximum(range.copy_wire_value().maximum.to_framework_f32());
        } else {
            if curve.minimum() < -1000.0 {
                curve.set_minimum(-1000.0);
            }
            if curve.maximum() > 1000.0 {
                curve.set_maximum(1000.0);
            }
        }
        self.curves()
            .ok_or(AnimationError::NullReference)?
            .set(index, Some(curve))
    }
    /// Scan current flag-array length; stored constructor count is independent.
    pub fn is_active(&self) -> Result<bool, AnimationError<E>> {
        let mut index = 0;
        while index < self.flags().ok_or(AnimationError::NullReference)?.len() {
            if self
                .flags()
                .ok_or(AnimationError::NullReference)?
                .value(index as i32)
                .map_err(|_| AnimationError::Index)?
            {
                return Ok(true);
            }
            index += 1;
        }
        Ok(false)
    }
    /// Write flags before active children; a child error preserves partial bytes.
    /// Reading retained Gradient state here never invokes the lazy getter.
    pub fn save(&self, writer: &mut BinaryWriter) -> Result<(), AnimationError<E>> {
        let mut index = 0;
        while index < self.flags().ok_or(AnimationError::NullReference)?.len() {
            writer.write_bool(
                self.flags()
                    .ok_or(AnimationError::NullReference)?
                    .value(index as i32)
                    .map_err(|_| AnimationError::Index)?,
            )?;
            if self
                .flags()
                .ok_or(AnimationError::NullReference)?
                .value(index as i32)
                .map_err(|_| AnimationError::Index)?
            {
                self.curves()
                    .ok_or(AnimationError::NullReference)?
                    .value(index as i32)?
                    .ok_or(AnimationError::NullReference)?
                    .save(writer)
                    .map_err(AnimationError::Curve)?;
            }
            index += 1;
        }
        let present = self.retained_gradient().is_some()
            && !self
                .flags()
                .ok_or(AnimationError::NullReference)?
                .is_empty()
            && self
                .flags()
                .ok_or(AnimationError::NullReference)?
                .value(0)
                .map_err(|_| AnimationError::Index)?;
        writer.write_bool(present)?;
        if present {
            self.retained_gradient()
                .ok_or(AnimationError::NullReference)?
                .save(writer)?;
        }
        Ok(())
    }
    /// Mutate actual flags/children sequentially, publishing a missing child
    /// before nested reads. Inactive slots and a false Gradient marker retain.
    pub fn load(
        &self,
        reader: &mut BinaryCursor<'_>,
        version: f64,
    ) -> Result<(), AnimationError<E>> {
        let mut index = 0;
        while index < self.flags().ok_or(AnimationError::NullReference)?.len() {
            // Source evaluates the array receiver before ReadBoolean.
            let flags = self.flags().ok_or(AnimationError::NullReference)?;
            let active = reader.read_bool()?;
            flags
                .set(index as i32, active)
                .map_err(|_| AnimationError::Index)?;
            if self
                .flags()
                .ok_or(AnimationError::NullReference)?
                .value(index as i32)
                .map_err(|_| AnimationError::Index)?
            {
                if self
                    .curves()
                    .ok_or(AnimationError::NullReference)?
                    .value(index as i32)?
                    .is_none()
                {
                    self.curves()
                        .ok_or(AnimationError::NullReference)?
                        .set(index as i32, Some(SharedCurve::new(0.0, 0.0, false)))?;
                }
                self.curves()
                    .ok_or(AnimationError::NullReference)?
                    .value(index as i32)?
                    .ok_or(AnimationError::NullReference)?
                    .load(reader, version)
                    .map_err(AnimationError::Curve)?;
            }
            index += 1;
        }
        if version > 3.15 && reader.read_bool()? {
            if self.retained_gradient().is_none() {
                self.set_gradient(Some(SharedGradient::new()));
            }
            self.retained_gradient()
                .ok_or(AnimationError::NullReference)?
                .load(reader)?;
        }
        Ok(())
    }
    /// Actual bh(bh) constructor. Null is an argument, not a failed producer.
    pub fn from_source(source: Option<&Self>) -> Result<Self, AnimationError<E>> {
        source.ok_or(AnimationError::NullReference)?.source_clone()
    }
    /// Nullable caller transport; receiver failure occurs on the first actual write.
    pub fn save_nullable(
        &self,
        writer: Option<&mut BinaryWriter>,
    ) -> Result<(), AnimationError<E>> {
        match writer {
            Some(w) => self.save(w),
            None => self.save(&mut BinaryWriter::null_receiver()),
        }
    }
    /// A legacy zero-flag load can succeed without dereferencing a null reader.
    pub fn load_nullable(
        &self,
        reader: Option<&mut BinaryCursor<'_>>,
        version: f64,
    ) -> Result<(), AnimationError<E>> {
        match reader {
            Some(r) => self.load(r, version),
            None => self.load(&mut BinaryCursor::null_receiver(), version),
        }
    }
    /// Real object dispatch, including null, wrong concrete kind and self.
    pub fn source_equals_object(
        &self,
        other: crate::SourceObject<'_, E>,
    ) -> Result<bool, AnimationError<E>> {
        let crate::SourceObject::Animation(other) = other else {
            return Ok(false);
        };
        let gradient_equal = match self.retained_gradient() {
            Some(g) => g.source_equals(other.retained_gradient().as_ref())?,
            None => other.retained_gradient().is_none(),
        };
        let limits_equal = if let Some(limits) = self.limits() {
            let right = other.limits().ok_or(AnimationError::NullReference)?;
            if limits.len() != right.len() {
                return Ok(false);
            }
            for ix in 0..limits.len() {
                let l = limits.value(ix as i32).map_err(|_| AnimationError::Index)?;
                let r = right.value(ix as i32).map_err(|_| AnimationError::Index)?;
                if let Some(l) = l {
                    if !l.source_equals(r.as_ref()) {
                        return Ok(false);
                    }
                } else if r.is_some() {
                    return Ok(false);
                }
            }
            true
        } else {
            other.limits().is_none()
        };
        let left_len = self.flags().ok_or(AnimationError::NullReference)?.len();
        let right_len = other.flags().ok_or(AnimationError::NullReference)?.len();
        if left_len != right_len || !limits_equal || !gradient_equal {
            return Ok(false);
        }
        for ix in 0..left_len {
            let left = self
                .curves()
                .ok_or(AnimationError::NullReference)?
                .value(ix as i32)?;
            let curve_equal = match left {
                Some(curve) => {
                    let right = other
                        .curves()
                        .ok_or(AnimationError::NullReference)?
                        .value(ix as i32)?;
                    curve
                        .source_equals_object(
                            right
                                .as_ref()
                                .map_or(crate::SourceObject::Null, crate::SourceObject::Curve),
                        )
                        .map_err(AnimationError::Curve)?
                }
                None => other
                    .curves()
                    .ok_or(AnimationError::NullReference)?
                    .value(ix as i32)?
                    .is_none(),
            };
            let left = self
                .flags()
                .ok_or(AnimationError::NullReference)?
                .value(ix as i32)
                .map_err(|_| AnimationError::Index)?;
            let right = other
                .flags()
                .ok_or(AnimationError::NullReference)?
                .value(ix as i32)
                .map_err(|_| AnimationError::Index)?;
            if left != right || (left && !curve_equal) {
                return Ok(false);
            }
        }
        Ok(true)
    }
    pub fn source_clone(&self) -> Result<Self, AnimationError<E>> {
        let flags = self.flags().ok_or(AnimationError::NullReference)?;
        let next = Self::with_limits_array(flags.len() as i32, self.limits())?;
        for i in 0..flags.len() {
            next.flags()
                .unwrap()
                .set(
                    i as i32,
                    flags.value(i as i32).map_err(|_| AnimationError::Index)?,
                )
                .map_err(|_| AnimationError::Index)?;
            let curve = self
                .curves()
                .ok_or(AnimationError::NullReference)?
                .value(i as i32)?;
            next.curves().unwrap().set(
                i as i32,
                curve
                    .map(|c| c.source_clone())
                    .transpose()
                    .map_err(AnimationError::Curve)?,
            )?;
        }
        if let Some(g) = self.retained_gradient() {
            next.set_gradient(Some(g.source_clone()?));
        }
        Ok(next)
    }

    /// Build an editable replacement for an animation's preset operation.
    pub fn with_curve_preset(
        &self,
        axis: i32,
        preset: &SharedCurve<E>,
        insert: bool,
    ) -> Result<Self, AnimationError<E>> {
        let next = self.source_clone()?;
        let curves = next.curves().ok_or(AnimationError::NullReference)?;
        let curve = curves.value(axis)?.ok_or(AnimationError::NullReference)?;
        curves.set(
            axis,
            Some(
                curve
                    .with_preset_layers(preset, insert)
                    .map_err(AnimationError::Curve)?,
            ),
        )?;
        Ok(next)
    }
}
impl<E: 'static> GradientAnimation<E> {
    /// Export-frame raw copy; unlike bh source Clone, preserve stored count,
    /// nullable/malformed arrays and caches without lazy publication or callbacks.
    pub(crate) fn capture_render_owner(
        &self,
        session: &mut crate::RenderCaptureSession<E>,
    ) -> Result<Self, AnimationError<E>> {
        let (count, flags, curves, limits, gradient) = {
            let state = guard(&self.0);
            (
                state.count,
                state.flags.clone(),
                state.curves.clone(),
                state.limits.clone(),
                state.gradient.clone(),
            )
        };
        let flags = flags
            .as_ref()
            .map(|v| session.copy_animation_flags(v))
            .transpose()?;
        let curves = curves
            .as_ref()
            .map(|v| session.copy_animation_curves(v))
            .transpose()?;
        let limits = limits
            .as_ref()
            .map(|v| session.copy_animation_limits(v))
            .transpose()?;
        let gradient = gradient.as_ref().map(|g| session.copy_gradient(g));
        Ok(Self(Arc::new(Mutex::new(State {
            count,
            flags,
            curves,
            limits,
            gradient,
        }))))
    }
}

#[cfg(test)]
mod source_laws {
    use super::*;
    // Original bh copy reads curves only inside the flags-length loop.
    // This is a static source-body adjacency check, not a new native golden.
    #[test]
    fn zero_flag_length_does_not_read_null_curve_array() {
        let original = GradientAnimation::<()>::new(1, None).unwrap();
        original.set_flags(Some(AnimationFlags::from_values(vec![])));
        original.set_curves(None);
        let gradient = original.gradient().unwrap();
        let copy = original.source_clone().unwrap();
        assert_eq!(copy.count(), 0);
        assert!(copy.flags().unwrap().is_empty());
        assert!(copy.curves().unwrap().is_empty());
        assert!(!gradient.same_reference(&copy.gradient().unwrap()));
        assert!(original.curves().is_none());
    }
    #[test]
    fn copy_uses_current_flag_length_fresh_arrays_shared_limits_and_occurrence_curves() {
        let a = GradientAnimation::<()>::new(1, None).unwrap();
        let flags = AnimationFlags::from_values(vec![true, false]);
        a.set_flags(Some(flags.clone()));
        let curve = SharedCurve::<()>::new(-1.0, 1.0, false);
        let curves = AnimationCurves::from_values(vec![Some(curve.clone()), Some(curve.clone())]);
        a.set_curves(Some(curves.clone()));
        let g = a.gradient().unwrap();
        let b = a.source_clone().unwrap();
        assert_eq!(a.count(), 1);
        assert_eq!(b.count(), 2);
        assert!(!flags.same_reference(&b.flags().unwrap()));
        assert!(!curves.same_reference(&b.curves().unwrap()));
        assert!(a.limits().unwrap().same_reference(&b.limits().unwrap()));
        let x = b.curves().unwrap().value(0).unwrap().unwrap();
        let y = b.curves().unwrap().value(1).unwrap().unwrap();
        assert!(!x.same_reference(&y));
        assert!(!x.same_reference(&curve));
        assert!(!g.same_reference(&b.gradient().unwrap()));
        assert!(b.gradient().unwrap().cache().is_none());
    }
}
