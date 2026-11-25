//! Sponsored Solid Color graph edits over the existing live model. No public
//! method returns its underlying node/property handle or a duplicate color state.
use super::{
    PointwiseError,
    graph::{Payload, PointwiseNode},
};
use crate::{
    ChangeDispatchError, ContextChangeArgument, GradientAnimation, LiveSolidColorError,
    SharedLimits, SharedSolidColorNode, SharedSolidColorProperty, SolidColorCallbackError,
    SolidColorNodeDelegate, SolidColorNodeHandler, SolidColorProperty,
};
use std::{error::Error, fmt, sync::Arc};

/// Source failures retain the original typed notification/rounding/channel
/// cause. Wrong-node/foreign-binding errors belong to this target API, not a
/// fabricated original exception class.
#[derive(Debug)]
pub enum PointwiseSolidEditError<E = ()> {
    NotSolidColor,
    HandlerOwnerMismatch,
    Source(LiveSolidColorError<E>),
}
impl<E> From<LiveSolidColorError<E>> for PointwiseSolidEditError<E> {
    fn from(error: LiveSolidColorError<E>) -> Self {
        Self::Source(error)
    }
}
impl<E> PointwiseSolidEditError<E> {
    pub(super) fn into_property_error(self) -> PointwiseError<E> {
        match self {
            Self::Source(error) => solid_property_error(error),
            _ => PointwiseError::InvalidCast,
        }
    }
    pub fn source_error(&self) -> Option<&LiveSolidColorError<E>> {
        match self {
            Self::Source(error) => Some(error),
            _ => None,
        }
    }
    pub fn source_class_with(&self, observer: impl Fn(&E) -> &'static str) -> Option<&'static str> {
        match self {
            Self::Source(LiveSolidColorError::Property(error))
                if error.is_unsupported_capability() =>
            {
                None
            }
            _ => self
                .source_error()
                .map(|error| error.source_class_with(observer)),
        }
    }
}
impl<E: fmt::Display> fmt::Display for PointwiseSolidEditError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotSolidColor => f.write_str("The graph node is not Solid Color."),
            Self::HandlerOwnerMismatch => {
                f.write_str("The change handler belongs to another graph node.")
            }
            Self::Source(error) => error.fmt(f),
        }
    }
}
impl<E: Error + 'static> Error for PointwiseSolidEditError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source_error().map(|error| error as _)
    }
}

/// A real ordinary node delegate bound to one sponsored graph node. Cloning
/// retains the same delegate identity. Notifier storage contains only its
/// underlying core; it never stores this strong sponsor wrapper.
pub struct PointwiseSolidChangeHandler<E = ()> {
    owner: PointwiseNode<E>,
    binding: SolidColorNodeDelegate<E>,
}
impl<E> Clone for PointwiseSolidChangeHandler<E> {
    fn clone(&self) -> Self {
        Self {
            owner: self.owner.clone(),
            binding: self.binding.clone(),
        }
    }
}
impl<E> fmt::Debug for PointwiseSolidChangeHandler<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PointwiseSolidChangeHandler")
            .field("owner", &self.owner)
            .field("binding", &self.binding)
            .finish()
    }
}
impl<E> PointwiseSolidChangeHandler<E> {
    pub fn same_binding(&self, other: &Self) -> bool
    where
        E: 'static,
    {
        // This wrapper always contains one ordinary handler. Invocation-array
        // identity applies only to multicast delegates and is false for singles.
        self.owner.same_reference(&other.owner)
            && self.binding.handlers()[0].same_binding(&other.binding.handlers()[0])
    }
    pub fn owner(&self) -> PointwiseNode<E> {
        self.owner.clone()
    }
}

impl<E: 'static> PointwiseNode<E> {
    /// Host the same retained Solid Color in a graph editor.
    pub fn from_solid_color(model: SharedSolidColorNode<E>) -> Self {
        Self::publish_solid(model)
    }

    pub(crate) fn solid_model(
        &self,
    ) -> Result<SharedSolidColorNode<E>, PointwiseSolidEditError<E>> {
        match &self.core().payload {
            Payload::Solid { model, .. } => Ok(model.clone()),
            _ => Err(PointwiseSolidEditError::NotSolidColor),
        }
    }
    pub(super) fn install_solid_invalidation(&self) {
        let core = self.core();
        let Payload::Solid { model, .. } = &core.payload else {
            return;
        };
        let weak = Arc::downgrade(&core);
        let handler = SolidColorNodeHandler::from_callback(move |_, _| {
            let Some(frame) = weak.upgrade().as_ref().and_then(Self::from_core) else {
                // A retained standalone property can outlive its graph wrapper.
                return Ok(());
            };
            // First node listener: source values have committed, but external
            // node listeners have not run. No model/region/slot guard survives.
            frame.invalidate();
            Ok(())
        });
        model.add(&SolidColorNodeDelegate::from_handlers(&[handler]));
    }
    pub fn solid_color_values(&self) -> Result<[f64; 3], PointwiseSolidEditError<E>> {
        Ok(self.solid_model()?.snapshot().color)
    }
    pub fn solid_transparency(&self) -> Result<f64, PointwiseSolidEditError<E>> {
        Ok(self.solid_model()?.snapshot().transparency)
    }
    pub fn solid_channel(&self, channel: i32) -> Result<f32, PointwiseSolidEditError<E>> {
        Ok(self.solid_model()?.constant_channel(channel)?)
    }
    /// RGB uses the actual unlinked rr setter. Equality, NaN and notification
    /// ordering remain owned by SharedSolidColorProperty.
    pub fn set_solid_color(&self, color: [f64; 3]) -> Result<(), PointwiseSolidEditError<E>> {
        self.set_solid_color_value(&crate::ContextColorValue::unlinked(color))
    }
    pub(super) fn set_solid_color_value(
        &self,
        value: &crate::ContextColorValue<E>,
    ) -> Result<(), PointwiseSolidEditError<E>> {
        let property = self.solid_model()?.color_property();
        let args = property.edit_color_value(value, false, self.has_property_history_target())?;
        self.record_solid_edit(args)
    }
    pub fn set_solid_color_component(
        &self,
        channel: i32,
        value: f64,
    ) -> Result<(), PointwiseSolidEditError<E>> {
        let property = self.solid_model()?.color_property();
        let args =
            property.edit_color_component(channel, value, self.has_property_history_target())?;
        self.record_solid_edit(args)
    }
    pub fn set_solid_color_object(
        &self,
        color: Option<[f64; 3]>,
    ) -> Result<(), PointwiseSolidEditError<E>> {
        match color {
            Some(color) => self.set_solid_color(color),
            None => {
                self.solid_model()?;
                Err(LiveSolidColorError::NullColor.into())
            }
        }
    }
    pub fn set_solid_transparency(&self, value: f64) -> Result<(), PointwiseSolidEditError<E>> {
        let property = self.solid_model()?.transparency_property();
        let args = property.edit_transparency(value, self.has_property_history_target())?;
        self.record_solid_edit(args)
    }
    pub fn set_solid_channel(
        &self,
        channel: i32,
        value: f32,
    ) -> Result<(), PointwiseSolidEditError<E>> {
        match channel {
            0..=2 => self.set_solid_color_component(channel, f64::from(value)),
            3 => self.set_solid_transparency(f64::from(value)),
            _ => Ok(self.solid_model()?.set_channel(channel, value)?),
        }
    }
    fn record_solid_edit(
        &self,
        args: Option<Arc<ContextChangeArgument<E>>>,
    ) -> Result<(), PointwiseSolidEditError<E>> {
        if let Some(args) = args {
            self.record_property_history(&args)
                .map_err(LiveSolidColorError::Property)?;
        }
        Ok(())
    }
    /// The real retained Limits root has no parent backedge; returning it does
    /// not leak an unsponsored Solid Color property. Clone keeps these roots.
    pub fn solid_limits(
        &self,
        property: SolidColorProperty,
    ) -> Result<SharedLimits, PointwiseSolidEditError<E>> {
        let model = self.solid_model()?;
        Ok(match property {
            SolidColorProperty::Color => model.color_property().limits(),
            SolidColorProperty::Transparency => model.transparency_property().limits(),
        })
    }
    pub(super) fn solid_property(
        &self,
        kind: SolidColorProperty,
    ) -> Result<SharedSolidColorProperty<E>, PointwiseError<E>> {
        let model = self
            .solid_model()
            .map_err(|_| PointwiseError::InvalidCast)?;
        Ok(match kind {
            SolidColorProperty::Color => model.color_property(),
            SolidColorProperty::Transparency => model.transparency_property(),
        })
    }
    pub fn retained_solid_animation(
        &self,
        property: SolidColorProperty,
    ) -> Result<Option<GradientAnimation<E>>, PointwiseError<E>> {
        Ok(self.solid_property(property)?.retained_animation())
    }
    pub fn solid_animation(
        &self,
        property: SolidColorProperty,
    ) -> Result<GradientAnimation<E>, PointwiseError<E>> {
        self.solid_property(property)?.ensure_animation()
    }
    pub fn set_solid_animation(
        &self,
        property: SolidColorProperty,
        animation: Option<GradientAnimation<E>>,
    ) -> Result<(), PointwiseError<E>> {
        let args = self
            .solid_property(property)?
            .replace_animation(animation, self.has_property_history_target())
            .map_err(solid_property_error)?;
        self.record_solid_edit(args)
            .map_err(PointwiseSolidEditError::into_property_error)
    }
    pub fn solid_change_handler(
        &self,
        callback: impl Fn(&PointwiseNode<E>, Option<Arc<ContextChangeArgument<E>>>) -> Result<(), E>
        + Send
        + Sync
        + 'static,
    ) -> Result<PointwiseSolidChangeHandler<E>, PointwiseSolidEditError<E>> {
        self.solid_model()?;
        let weak = Arc::downgrade(&self.core());
        let handler = SolidColorNodeHandler::from_callback(move |_, args| {
            let core = weak
                .upgrade()
                .ok_or(SolidColorCallbackError::SenderReleased)?;
            let frame = Self::from_core(&core).ok_or(SolidColorCallbackError::SenderReleased)?;
            callback(&frame, args).map_err(SolidColorCallbackError::Observer)
        });
        Ok(PointwiseSolidChangeHandler {
            owner: self.clone(),
            binding: SolidColorNodeDelegate::from_handlers(&[handler]),
        })
    }
    pub fn add_solid_change_handler(
        &self,
        handler: &PointwiseSolidChangeHandler<E>,
    ) -> Result<(), PointwiseSolidEditError<E>> {
        if !self.same_reference(&handler.owner) {
            return Err(PointwiseSolidEditError::HandlerOwnerMismatch);
        }
        self.solid_model()?.add(&handler.binding);
        Ok(())
    }
    pub fn remove_solid_change_handler(
        &self,
        handler: &PointwiseSolidChangeHandler<E>,
    ) -> Result<(), PointwiseSolidEditError<E>> {
        if !self.same_reference(&handler.owner) {
            return Err(PointwiseSolidEditError::HandlerOwnerMismatch);
        }
        Ok(self.solid_model()?.remove(&handler.binding)?)
    }
}

pub(super) fn solid_property_error<E>(error: LiveSolidColorError<E>) -> PointwiseError<E> {
    match error {
        LiveSolidColorError::Property(error) => error,
        LiveSolidColorError::Notification(error) => solid_notification_error(error),
        LiveSolidColorError::Rounding(error) => PointwiseError::DecimalRounding(error),
        LiveSolidColorError::Kind | LiveSolidColorError::NumberCast => PointwiseError::InvalidCast,
        _ => PointwiseError::InvalidOperation,
    }
}

fn solid_notification_error<E>(
    error: ChangeDispatchError<SolidColorCallbackError<E>>,
) -> PointwiseError<E> {
    match error {
        ChangeDispatchError::Framework(error) => PointwiseError::PropertyNotification(error),
        ChangeDispatchError::Handler(SolidColorCallbackError::Observer(error)) => {
            PointwiseError::Observer(error)
        }
        ChangeDispatchError::Handler(SolidColorCallbackError::Node(error)) => {
            solid_notification_error(*error)
        }
        ChangeDispatchError::Handler(SolidColorCallbackError::SenderReleased) => {
            PointwiseError::InvalidOperation
        }
    }
}

impl<E: 'static> PointwiseNode<E> {
    pub(super) fn save_solid_wire_property(
        &self,
        ix: usize,
        writer: &mut crate::BinaryWriter,
    ) -> Result<(), super::NodeWireError<E>> {
        let model = self.solid_model().map_err(super::NodeWireError::Solid)?;
        let property = match ix {
            0 => model.color_property(),
            1 => model.transparency_property(),
            _ => return Err(super::NodeWireError::PropertyIndex),
        };
        property.save_wire(writer)
    }
    pub(super) fn load_solid_wire_property(
        &self,
        ix: usize,
        reader: &mut crate::BinaryCursor<'_>,
        version: f64,
    ) -> Result<(), super::NodeWireError<E>> {
        let model = self.solid_model().map_err(super::NodeWireError::Solid)?;
        let property = match ix {
            0 => model.color_property(),
            1 => model.transparency_property(),
            _ => return Err(super::NodeWireError::PropertyIndex),
        };
        property.load_wire(reader, version)
    }
    pub(super) fn load_solid_wire_legacy(
        &self,
        reader: &mut crate::BinaryCursor<'_>,
        _version: f64,
        _count: u8,
    ) -> Result<(), super::NodeWireError<E>> {
        reader.read_string()?;
        // All four reads precede either setter in the actual kx override.
        let color = [reader.read_f64()?, reader.read_f64()?, reader.read_f64()?];
        let transparency = reader.read_f64()?;
        self.set_solid_color(color)
            .map_err(super::NodeWireError::Solid)?;
        self.set_solid_transparency(transparency)
            .map_err(super::NodeWireError::Solid)?;
        reader.read_bool()?;
        reader.read_bool()?;
        Ok(())
    }
}
