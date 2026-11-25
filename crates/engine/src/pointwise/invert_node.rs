//! Sponsored graph access to the actual ordinary Invert/selection authority.
//! Stored delegates contain weak graph-core routes; outward edit/handler frames
//! retain the CURRENT merged region. No unsponsored model/property escapes.
use super::graph::{Payload, PointwiseNode};
use super::{PointwiseError, SharedImageMap};
use crate::selection::SelectionText;
use crate::{
    ImageError, SelectionCloneMode, SelectionDelegate, SelectionError, SelectionEventArgs,
    SelectionHandler, SelectionRenderError, SelectionSender, SelectionSnapshot, SelectionStamp,
    SelectionValue, SharedInvertNode, SharedSelectionOptions, SharedSelectionProperty,
};
use std::{fmt, sync::Arc};

pub(super) enum InvertCallbackError<E> {
    Observer(E),
    OwnerReleased,
}
/// API misuse and graph-lifetime failures are separate from actual source
/// selection/image errors; they are not invented original exception classes.
#[derive(Debug)]
pub enum PointwiseInvertError<E = ()> {
    NotInvert,
    HandlerOwnerMismatch,
    OwnerReleased,
    Wire(PointwiseError<E>),
    Selection(SelectionError<E>),
    Image(ImageError),
}
impl<E> PointwiseInvertError<E> {
    pub fn source_class_with(&self, observer: impl Fn(&E) -> &'static str) -> Option<&'static str> {
        match self {
            Self::Selection(SelectionError::Index) => Some("IndexOutOfRangeException"),
            Self::Selection(SelectionError::NullReference) => Some("NullReferenceException"),
            Self::Selection(SelectionError::InvalidCast) => Some("InvalidCastException"),
            Self::Selection(SelectionError::UnrecognizedText) => Some("Exception"),
            Self::Selection(SelectionError::Observer(e)) => Some(observer(e)),
            Self::Image(e) => Some(e.source_class()),
            Self::Wire(error) => super::node_wire::pointwise_source_class_with(error, &observer),
            Self::NotInvert | Self::HandlerOwnerMismatch | Self::OwnerReleased => None,
        }
    }
}
impl<E: fmt::Debug> fmt::Display for PointwiseInvertError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl<E: fmt::Debug + 'static> std::error::Error for PointwiseInvertError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Selection(e) => Some(e),
            Self::Image(e) => Some(e),
            Self::Wire(e) => Some(e),
            _ => None,
        }
    }
}
fn selection_error<E>(error: SelectionError<InvertCallbackError<E>>) -> PointwiseInvertError<E> {
    use SelectionError::*;
    match error {
        Index => PointwiseInvertError::Selection(Index),
        NullReference => PointwiseInvertError::Selection(NullReference),
        InvalidCast => PointwiseInvertError::Selection(InvalidCast),
        UnrecognizedText => PointwiseInvertError::Selection(UnrecognizedText),
        Observer(InvertCallbackError::Observer(e)) => PointwiseInvertError::Selection(Observer(e)),
        Observer(InvertCallbackError::OwnerReleased) => PointwiseInvertError::OwnerReleased,
    }
}
fn render_error<E>(error: SelectionRenderError<InvertCallbackError<E>>) -> PointwiseInvertError<E> {
    match error {
        SelectionRenderError::Property(e) => selection_error(e),
        SelectionRenderError::Image(e) => PointwiseInvertError::Image(e),
    }
}
pub(super) fn graph_error<E>(error: PointwiseInvertError<E>) -> PointwiseError<E> {
    match error {
        PointwiseInvertError::Selection(e) => PointwiseError::Selection(e),
        PointwiseInvertError::Image(e) => PointwiseError::Image(e),
        PointwiseInvertError::OwnerReleased => {
            PointwiseError::UnsupportedCapability("Invert graph owner released")
        }
        PointwiseInvertError::Wire(error) => error,
        PointwiseInvertError::NotInvert | PointwiseInvertError::HandlerOwnerMismatch => {
            PointwiseError::UnsupportedCapability("wrong Invert graph receiver")
        }
    }
}
/// Ordinary property identity is the actual constructor owner plus channel.
/// ParentNode is independent and may be another retained Invert or null.
pub struct PointwiseInvertProperty<E = ()> {
    owner: PointwiseNode<E>,
    channel: i32,
}
impl<E> Clone for PointwiseInvertProperty<E> {
    fn clone(&self) -> Self {
        Self {
            owner: self.owner.clone(),
            channel: self.channel,
        }
    }
}
impl<E> fmt::Debug for PointwiseInvertProperty<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PointwiseInvertProperty")
            .field(&self.owner)
            .field(&self.channel)
            .finish()
    }
}
/// A genuine selection binding with an outward graph sponsor. Storage inside
/// the model receives only binding, never this owner-retaining wrapper.
pub struct PointwiseInvertHandler<E = ()> {
    owner: PointwiseNode<E>,
    channel: Option<i32>,
    binding: SelectionHandler<InvertCallbackError<E>>,
}
impl<E> Clone for PointwiseInvertHandler<E> {
    fn clone(&self) -> Self {
        Self {
            owner: self.owner.clone(),
            channel: self.channel,
            binding: self.binding.clone(),
        }
    }
}
impl<E> fmt::Debug for PointwiseInvertHandler<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PointwiseInvertHandler")
            .field(&self.owner)
            .field(&self.channel)
            .finish()
    }
}
impl<E> PointwiseInvertHandler<E> {
    pub fn owner(&self) -> PointwiseNode<E> {
        self.owner.clone()
    }
    pub fn same_binding(&self, other: &Self) -> bool {
        self.owner.same_reference(&other.owner)
            && self.channel == other.channel
            && self.binding.same_binding(&other.binding)
    }
}
/// Captured actual invocation-list identity. It retains both the real delegate
/// captures and the graph region, without exposing a raw selection owner.
pub struct PointwiseInvertChangeSnapshot<E = ()> {
    owner: PointwiseInvertProperty<E>,
    actual: SelectionDelegate<InvertCallbackError<E>>,
}
impl<E> Clone for PointwiseInvertChangeSnapshot<E> {
    fn clone(&self) -> Self {
        Self {
            owner: self.owner.clone(),
            actual: self.actual.clone(),
        }
    }
}
impl<E> PointwiseInvertChangeSnapshot<E> {
    pub fn owner(&self) -> PointwiseInvertProperty<E> {
        self.owner.clone()
    }
    pub fn is_null(&self) -> bool {
        self.actual.is_null()
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        self.actual.same_reference(&other.actual)
    }
}
impl<E> PointwiseNode<E> {
    pub(super) fn invert_model(
        &self,
    ) -> Result<SharedInvertNode<InvertCallbackError<E>>, PointwiseInvertError<E>> {
        match &self.core().payload {
            Payload::Invert { model, .. } => Ok(model.clone()),
            _ => Err(PointwiseInvertError::NotInvert),
        }
    }
    pub fn invert_property(
        &self,
        channel: i32,
    ) -> Result<PointwiseInvertProperty<E>, PointwiseInvertError<E>> {
        self.invert_model()?
            .property(channel)
            .map_err(selection_error)?;
        Ok(PointwiseInvertProperty {
            owner: self.clone(),
            channel,
        })
    }
    pub fn invert_properties(
        &self,
    ) -> Result<[PointwiseInvertProperty<E>; 4], PointwiseInvertError<E>> {
        self.invert_model()?;
        Ok(std::array::from_fn(|ix| PointwiseInvertProperty {
            owner: self.clone(),
            channel: ix as i32,
        }))
    }
    pub fn invert_clone_mode(&self) -> Result<SelectionCloneMode, PointwiseInvertError<E>> {
        Ok(self.invert_model()?.mode())
    }
    /// xe.g(): real sequential setters; an observer failure interrupts later
    /// channels, preserving all earlier commits.
    pub fn set_invert_red_only(&self) -> Result<(), PointwiseInvertError<E>> {
        for (channel, value) in [0, 1, 1, 1].into_iter().enumerate() {
            self.invert_property(channel as i32)?.set_index(value)?;
        }
        Ok(())
    }
    pub fn invert_scalar(&self, channel: i32, input: f32) -> Result<f32, PointwiseInvertError<E>>
    where
        E: 'static,
    {
        if (0..4).contains(&channel) {
            self.ensure_invert_wire_property_evaluation_supported(channel as usize)
                .map_err(PointwiseInvertError::Wire)?;
        }
        Ok(self.invert_model()?.scalar(channel, input))
    }
    /// Exact protected xe.k8 seam over an explicitly resolved retained input.
    /// Clone-mode xe.aa optimization is a separate graph capability.
    pub fn render_invert_supplied(
        &self,
        input: Option<&SharedImageMap>,
        width: i32,
        height: i32,
    ) -> Result<SharedImageMap, PointwiseInvertError<E>>
    where
        E: 'static,
    {
        self.ensure_invert_wire_evaluation_supported()
            .map_err(PointwiseInvertError::Wire)?;
        self.invert_model()?
            .render_shared(input, width, height)
            .map_err(render_error)
    }
}
impl<E: 'static> PointwiseNode<E> {
    pub(in crate::pointwise) fn render_invert_graph_frame(
        &self,
        input: Option<&SharedImageMap>,
        width: i32,
        height: i32,
        time: f64,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        self.ensure_invert_evaluation_supported(true)?;
        let mut ignored = [false; 4];
        for (skip, property) in ignored
            .iter_mut()
            .zip(self.invert_properties().map_err(graph_error)?)
        {
            let entries = property.options().map_err(graph_error)?.entries();
            let index = property.effective_index_at(entries.len(), time)?;
            let text = usize::try_from(index)
                .ok()
                .and_then(|index| entries.get(index))
                .ok_or(PointwiseError::Selection(SelectionError::Index))?;
            *skip = text.as_deref() == Some(&[b'N' as u16, b'o' as u16]);
        }
        Ok(SharedImageMap::render_invert_resolved(
            input, width, height, ignored,
        )?)
    }

    pub(super) fn install_invert_invalidation(&self) {
        let core = self.core();
        let Payload::Invert { model, .. } = &core.payload else {
            return;
        };
        let weak = Arc::downgrade(&core);
        model.add_changed(&SelectionHandler::new(move |_, _| {
            let core = weak.upgrade().ok_or(InvertCallbackError::OwnerReleased)?;
            let frame = Self::from_core(&core).ok_or(InvertCallbackError::OwnerReleased)?;
            // The existing selection forwarder invokes Node.changed only for
            // original ordinary changes. Speed/controls-only events stop there.
            frame.invalidate();
            Ok(())
        }));
    }
    pub fn invert_change_handler(
        &self,
        callback: impl Fn(&Self, Arc<SelectionEventArgs>) -> Result<(), E> + Send + Sync + 'static,
    ) -> Result<PointwiseInvertHandler<E>, PointwiseInvertError<E>> {
        self.invert_model()?;
        let weak = Arc::downgrade(&self.core());
        let binding = SelectionHandler::new(move |sender, args| {
            let core = weak.upgrade().ok_or(InvertCallbackError::OwnerReleased)?;
            let frame = Self::from_core(&core).ok_or(InvertCallbackError::OwnerReleased)?;
            let SelectionSender::Node(actual) = sender else {
                return Err(InvertCallbackError::OwnerReleased);
            };
            let model = frame
                .invert_model()
                .map_err(|_| InvertCallbackError::OwnerReleased)?;
            if !model.same_reference(actual) {
                return Err(InvertCallbackError::OwnerReleased);
            }
            callback(&frame, args).map_err(InvertCallbackError::Observer)
        });
        Ok(PointwiseInvertHandler {
            owner: self.clone(),
            channel: None,
            binding,
        })
    }
    pub fn add_invert_change_handler(
        &self,
        handler: &PointwiseInvertHandler<E>,
    ) -> Result<(), PointwiseInvertError<E>> {
        if !self.same_reference(&handler.owner) || handler.channel.is_some() {
            return Err(PointwiseInvertError::HandlerOwnerMismatch);
        }
        self.invert_model()?.add_changed(&handler.binding);
        Ok(())
    }
    pub fn remove_invert_change_handler(
        &self,
        handler: &PointwiseInvertHandler<E>,
    ) -> Result<(), PointwiseInvertError<E>> {
        if !self.same_reference(&handler.owner) || handler.channel.is_some() {
            return Err(PointwiseInvertError::HandlerOwnerMismatch);
        }
        self.invert_model()?.remove_changed(&handler.binding);
        Ok(())
    }
}
impl<E> PointwiseInvertProperty<E> {
    fn actual(
        &self,
    ) -> Result<SharedSelectionProperty<InvertCallbackError<E>>, PointwiseInvertError<E>> {
        self.owner
            .invert_model()?
            .property(self.channel)
            .map_err(selection_error)
    }
    pub fn constructor_owner(&self) -> PointwiseNode<E> {
        self.owner.clone()
    }
    pub fn channel(&self) -> i32 {
        self.channel
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        self.channel == other.channel && self.owner.same_reference(&other.owner)
    }
    pub fn snapshot(&self) -> Result<SelectionSnapshot, PointwiseInvertError<E>> {
        Ok(self.actual()?.snapshot())
    }
    pub fn current(&self) -> Result<i32, PointwiseInvertError<E>> {
        Ok(self.actual()?.current())
    }
    pub fn text(&self) -> Result<SelectionText, PointwiseInvertError<E>> {
        self.actual()?.text().map_err(selection_error)
    }
    pub fn options(&self) -> Result<SharedSelectionOptions, PointwiseInvertError<E>> {
        Ok(self.actual()?.options())
    }
    pub fn set_index(&self, value: i32) -> Result<(), PointwiseInvertError<E>> {
        self.actual()?.set_index(value).map_err(selection_error)
    }

    pub(super) fn prepare_adjusted_index(
        &self,
        index: i32,
    ) -> Result<
        (
            crate::selection::SelectionPosition,
            crate::selection::SelectionPosition,
        ),
        PointwiseError<E>,
    > {
        Ok(self
            .actual()
            .map_err(graph_error)?
            .prepare_adjusted_index(index))
    }

    pub(super) fn write_adjusted_index(&self, value: &crate::selection::SelectionPosition) {
        self.actual()
            .ok()
            .expect("retained Invert receiver")
            .write_adjusted_index(value);
    }

    pub(super) fn notify_adjusted_index(&self) -> Result<(), PointwiseError<E>> {
        self.actual()
            .map_err(graph_error)?
            .notify_adjusted_index()
            .map_err(selection_error)
            .map_err(graph_error)
    }
    pub fn set_index_speed(&self, value: i32) -> Result<(), PointwiseInvertError<E>> {
        self.actual()?
            .set_index_speed(value)
            .map_err(selection_error)
    }
    pub fn set_text(&self, value: SelectionText) -> Result<(), PointwiseInvertError<E>> {
        self.actual()?.set_text(value).map_err(selection_error)
    }
    pub fn set_value(&self, value: i32, temporary: bool) -> Result<(), PointwiseInvertError<E>> {
        self.actual()?
            .set_value(value, temporary)
            .map_err(selection_error)
    }
    pub fn set_stamp(&self, value: Option<&SelectionStamp>) -> Result<(), PointwiseInvertError<E>> {
        self.actual()?.set_stamp(value).map_err(selection_error)
    }
    pub fn set_object_value(&self, value: &SelectionValue) -> Result<(), PointwiseInvertError<E>> {
        self.actual()?
            .set_object_value(value)
            .map_err(selection_error)
    }
    pub fn set_object_value_temporary(
        &self,
        value: &SelectionValue,
        temporary: bool,
    ) -> Result<(), PointwiseInvertError<E>> {
        self.actual()?
            .set_object_value_temporary(value, temporary)
            .map_err(selection_error)
    }
    pub fn replace_options(
        &self,
        value: Option<SharedSelectionOptions>,
    ) -> Result<(), PointwiseInvertError<E>> {
        self.actual()?
            .replace_options(value)
            .map_err(selection_error)
    }
    pub fn set_allow_changes(&self, value: bool) -> Result<(), PointwiseInvertError<E>> {
        self.actual()?.set_allow_changes(value);
        Ok(())
    }
    pub fn set_enabled(&self, value: bool) -> Result<(), PointwiseInvertError<E>> {
        self.actual()?.set_enabled(value);
        Ok(())
    }
    pub fn enable_tracking(&self) -> Result<(), PointwiseInvertError<E>> {
        self.actual()?.enable_tracking();
        Ok(())
    }
    pub fn set_binding_index(&self, value: i32) -> Result<(), PointwiseInvertError<E>> {
        self.actual()?.set_binding_index(value);
        Ok(())
    }
    pub fn set_binding_name(&self, value: SelectionText) -> Result<(), PointwiseInvertError<E>> {
        self.actual()?.set_binding_name(value);
        Ok(())
    }
    pub fn clear_binding(&self) -> Result<(), PointwiseInvertError<E>> {
        self.actual()?.clear_binding();
        Ok(())
    }
    pub fn changed_snapshot(
        &self,
    ) -> Result<PointwiseInvertChangeSnapshot<E>, PointwiseInvertError<E>> {
        Ok(PointwiseInvertChangeSnapshot {
            owner: self.clone(),
            actual: self.actual()?.changed_snapshot(),
        })
    }
    pub fn parent(&self) -> Result<Option<PointwiseNode<E>>, PointwiseInvertError<E>> {
        let Some(parent) = self.actual()?.parent() else {
            return Ok(None);
        };
        self.owner
            .retained_nodes()
            .into_iter()
            .find(|n| n.invert_model().is_ok_and(|m| m.same_reference(&parent)))
            .map(Some)
            .ok_or(PointwiseInvertError::OwnerReleased)
    }
    pub fn set_parent(
        &self,
        parent: Option<&PointwiseNode<E>>,
    ) -> Result<(), PointwiseInvertError<E>> {
        let actual = parent.map(PointwiseNode::invert_model).transpose()?;
        if let Some(parent) = parent {
            self.owner.merge_region(parent);
        }
        self.actual()?.set_parent(actual.as_ref());
        Ok(())
    }
}
impl<E: 'static> PointwiseInvertProperty<E> {
    pub fn change_handler(
        &self,
        callback: impl Fn(&Self, Arc<SelectionEventArgs>) -> Result<(), E> + Send + Sync + 'static,
    ) -> PointwiseInvertHandler<E> {
        let weak = Arc::downgrade(&self.owner.core());
        let channel = self.channel;
        let binding = SelectionHandler::new(move |sender, args| {
            let core = weak.upgrade().ok_or(InvertCallbackError::OwnerReleased)?;
            let owner =
                PointwiseNode::from_core(&core).ok_or(InvertCallbackError::OwnerReleased)?;
            let frame = Self { owner, channel };
            let SelectionSender::Property(actual) = sender else {
                return Err(InvertCallbackError::OwnerReleased);
            };
            let expected = frame
                .actual()
                .map_err(|_| InvertCallbackError::OwnerReleased)?;
            if !expected.same_reference(actual) {
                return Err(InvertCallbackError::OwnerReleased);
            }
            callback(&frame, args).map_err(InvertCallbackError::Observer)
        });
        PointwiseInvertHandler {
            owner: self.owner.clone(),
            channel: Some(channel),
            binding,
        }
    }
    pub fn add_changed_handler(
        &self,
        handler: &PointwiseInvertHandler<E>,
    ) -> Result<(), PointwiseInvertError<E>> {
        if !self.owner.same_reference(&handler.owner) || handler.channel != Some(self.channel) {
            return Err(PointwiseInvertError::HandlerOwnerMismatch);
        }
        self.actual()?.add_changed(&handler.binding);
        Ok(())
    }
    pub fn remove_changed_handler(
        &self,
        handler: &PointwiseInvertHandler<E>,
    ) -> Result<(), PointwiseInvertError<E>> {
        if !self.owner.same_reference(&handler.owner) || handler.channel != Some(self.channel) {
            return Err(PointwiseInvertError::HandlerOwnerMismatch);
        }
        self.actual()?.remove_changed(&handler.binding);
        Ok(())
    }
}

#[path = "invert_wire.rs"]
mod wire;
pub(in crate::pointwise) use wire::InvertWireOwner;
