//! Concrete borrowed object operands for source virtual Equals dispatch.
//! No erased fixture objects or identity/hash normalization enters this boundary.
use crate::{
    DefaultContext, GradientAnimation, SharedCurve, SharedCurveRangeRecord, SharedGradient,
    SharedLimits,
};
#[non_exhaustive]
pub enum SourceObject<'a, E = DefaultContext> {
    Null,
    Animation(&'a GradientAnimation<E>),
    Curve(&'a SharedCurve<E>),
    Gradient(&'a SharedGradient),
    Limits(&'a SharedLimits),
    CurveRange(&'a SharedCurveRangeRecord),
}
