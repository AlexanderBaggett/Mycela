//! Color and Gradient property replacements staged with Lab catalog edits.
use super::graph::NodeCore;
use super::property::{PointwiseParameterValue, WeakPointwiseParameter};
use super::solid::solid_property_error;
use super::{PointwiseError, PointwiseNode, PointwiseParameter, SharedPropertyAdjustment};
use crate::environment_record::EnvironmentLabReference;
use crate::{
    ContextChangeArgument, ContextColorValue, HistoryError, SharedGradient,
    SharedSolidColorProperty,
};
use std::sync::{Arc, Weak};

pub(crate) struct LabReferenceEdit<E> {
    owner: WeakProperty<E>,
    before_adjustment: Option<SharedPropertyAdjustment<E>>,
    after_adjustment: Option<SharedPropertyAdjustment<E>>,
    before: Value<E>,
    after: Value<E>,
}

pub(crate) type ReferenceEditPlan<E> =
    (SharedPropertyAdjustment<E>, Option<EnvironmentLabReference>);

enum WeakProperty<E> {
    Filter(WeakPointwiseParameter<E>),
    Solid(Weak<NodeCore<E>>),
}

enum Value<E> {
    Color(ContextColorValue<E>),
    ResolvedFilterColor(Option<[f64; 3]>),
    Gradient(Option<SharedGradient>),
}

pub(crate) enum ReferenceReceiver<E> {
    Filter(PointwiseParameter<E>),
    Solid(SharedSolidColorProperty<E>),
}

impl<E: 'static> LabReferenceEdit<E> {
    pub(crate) fn collect_draft_preview(
        &self,
        preview: &mut super::property::capture::DraftRenderOverrides,
    ) {
        match (&self.after, &self.owner) {
            (Value::ResolvedFilterColor(Some(rgb)), WeakProperty::Filter(property)) => {
                if let Some(property) = property.upgrade() {
                    preview.colors.insert(
                        (
                            property.owner().node().occurrence_id(),
                            property.index() as usize,
                        ),
                        *rgb,
                    );
                }
            }
            (Value::Color(color), WeakProperty::Solid(node)) => {
                if let Some(node) = node
                    .upgrade()
                    .and_then(|core| PointwiseNode::from_core(&core))
                {
                    preview
                        .solids
                        .insert(node.occurrence_id(), color.components());
                }
            }
            _ => {}
        }
    }

    pub(crate) fn color_link(
        property: &PointwiseParameter<E>,
        after: Option<[f64; 3]>,
        adjustment: Option<SharedPropertyAdjustment<E>>,
    ) -> Self {
        Self {
            owner: WeakProperty::Filter(property.downgrade()),
            before_adjustment: property.retained_adjustment(),
            after_adjustment: adjustment,
            before: Value::ResolvedFilterColor(property.wire_metadata().resolved_lab_color()),
            after: Value::ResolvedFilterColor(after),
        }
    }

    pub(crate) fn gradient_link(
        property: &PointwiseParameter<E>,
        before: SharedGradient,
        after: SharedGradient,
        adjustment: Option<SharedPropertyAdjustment<E>>,
    ) -> Self {
        Self {
            owner: WeakProperty::Filter(property.downgrade()),
            before_adjustment: property.retained_adjustment(),
            after_adjustment: adjustment,
            before: Value::Gradient(Some(before)),
            after: Value::Gradient(Some(after)),
        }
    }

    pub(crate) fn node_uses_control(
        node: &PointwiseNode<E>,
        (group, tag, name): ([u8; 16], i32, &[u16]),
    ) -> bool {
        Self::node_uses_control_except(node, (group, tag, name), None)
    }

    pub(crate) fn node_uses_control_except(
        node: &PointwiseNode<E>,
        key: ([u8; 16], i32, &[u16]),
        excluded: Option<&PointwiseParameter<E>>,
    ) -> bool {
        let mut found = false;
        Self::visit_node_links_except(node, excluded, |other| found |= key == other);
        found
    }

    pub(crate) fn visit_node_links(
        node: &PointwiseNode<E>,
        visit: impl FnMut(([u8; 16], i32, &[u16])),
    ) {
        Self::visit_node_links_except(node, None, visit);
    }

    fn visit_node_links_except(
        node: &PointwiseNode<E>,
        excluded: Option<&PointwiseParameter<E>>,
        mut visit: impl FnMut(([u8; 16], i32, &[u16])),
    ) {
        let mut visit_adjustment = |adjustment: SharedPropertyAdjustment<E>| {
            let tag = adjustment.property_type();
            if matches!(tag, 6 | 7)
                && adjustment.is_active().unwrap_or(false)
                && let Some(name) = adjustment
                    .names()
                    .entries()
                    .into_iter()
                    .next()
                    .flatten()
                    .filter(|name| !name.is_empty())
            {
                visit((adjustment.guid(), tag, &name));
            }
        };
        if let Some(filter) = node.filter() {
            filter
                .retained_parameters()
                .iter()
                .filter(|property| excluded.is_none_or(|current| !property.same_reference(current)))
                .filter_map(PointwiseParameter::retained_adjustment)
                .for_each(visit_adjustment);
        } else if let Ok(solid) = node.solid_model()
            && let Some(adjustment) = solid.color_property().wire_metadata().retained_adjustment()
        {
            visit_adjustment(adjustment);
        }
    }

    pub(crate) fn prepare_node(
        node: &PointwiseNode<E>,
        resolve: impl FnMut(([u8; 16], i32, &[u16])) -> Option<EnvironmentLabReference>,
    ) -> Result<Vec<Self>, PointwiseError<E>> {
        Self::prepare_scoped_node(node, None, resolve)
    }

    pub(crate) fn prepare_scoped_node(
        node: &PointwiseNode<E>,
        scope: Option<[u8; 16]>,
        mut resolve: impl FnMut(([u8; 16], i32, &[u16])) -> Option<EnvironmentLabReference>,
    ) -> Result<Vec<Self>, PointwiseError<E>> {
        Self::prepare_edited_node(node, |current, name| {
            let value = resolve((current.guid(), current.property_type(), name))?;
            let next = if let Some(group) = scope.filter(|group| *group != current.guid()) {
                let mut parts = current.capture_parts();
                parts.guid = group;
                SharedPropertyAdjustment::from_capture_parts(parts)
            } else {
                current.clone()
            };
            Some((next, Some(value)))
        })
    }

    pub(crate) fn prepare_edited_node(
        node: &PointwiseNode<E>,
        mut edit: impl FnMut(&SharedPropertyAdjustment<E>, &[u16]) -> Option<ReferenceEditPlan<E>>,
    ) -> Result<Vec<Self>, PointwiseError<E>> {
        let mut changes = Vec::new();
        if let Some(filter) = node.filter() {
            for property in filter.retained_parameters() {
                let before = match property.raw_value() {
                    PointwiseParameterValue::Color(_) => {
                        Value::ResolvedFilterColor(property.wire_metadata().resolved_lab_color())
                    }
                    PointwiseParameterValue::Gradient(gradient) => Value::Gradient(gradient),
                    _ => continue,
                };
                if let Some(change) = Self::prepare(
                    WeakProperty::Filter(property.downgrade()),
                    property.retained_adjustment(),
                    before,
                    &mut edit,
                )? {
                    changes.push(change);
                }
            }
        } else if let Ok(solid) = node.solid_model() {
            let property = solid.color_property();
            if let Some(change) = Self::prepare(
                WeakProperty::Solid(Arc::downgrade(&node.core())),
                property.wire_metadata().retained_adjustment(),
                Value::Color(property.color_value().map_err(solid_property_error)?),
                &mut edit,
            )? {
                changes.push(change);
            }
        }
        Ok(changes)
    }

    fn prepare(
        owner: WeakProperty<E>,
        adjustment: Option<SharedPropertyAdjustment<E>>,
        before: Value<E>,
        edit: &mut impl FnMut(&SharedPropertyAdjustment<E>, &[u16]) -> Option<ReferenceEditPlan<E>>,
    ) -> Result<Option<Self>, PointwiseError<E>> {
        let Some(adjustment) = adjustment else {
            return Ok(None);
        };
        let tag = adjustment.property_type();
        if tag != before.tag() || !adjustment.is_active()? {
            return Ok(None);
        }
        let Some(name) = adjustment
            .names()
            .entries()
            .into_iter()
            .next()
            .flatten()
            .filter(|name| !name.is_empty())
        else {
            return Ok(None);
        };
        let Some((after_adjustment, value)) = edit(&adjustment, &name) else {
            return Ok(None);
        };
        let after = match value {
            Some(EnvironmentLabReference::Color(value))
                if matches!(&before, Value::ResolvedFilterColor(_)) =>
            {
                Value::ResolvedFilterColor(Some(value))
            }
            Some(EnvironmentLabReference::Color(value)) => {
                Value::Color(ContextColorValue::unlinked(value))
            }
            Some(EnvironmentLabReference::Gradient(value)) => Value::Gradient(Some(value.live())),
            None => match &before {
                Value::Color(value) => Value::Color(value.clone()),
                Value::ResolvedFilterColor(value) => Value::ResolvedFilterColor(*value),
                Value::Gradient(value) => Value::Gradient(value.clone()),
            },
        };
        if adjustment.same_reference(&after_adjustment) && before.same_value(&after) {
            return Ok(None);
        }
        Ok(Some(Self {
            owner,
            before_adjustment: Some(adjustment),
            after_adjustment: Some(after_adjustment),
            before,
            after,
        }))
    }

    pub(crate) fn publish_with<T>(
        changes: &[Self],
        undo: bool,
        other: impl FnOnce() -> Result<T, HistoryError<E>>,
    ) -> Result<(T, Vec<ReferenceReceiver<E>>), HistoryError<E>> {
        let targets = changes
            .iter()
            .map(|edit| {
                let target = edit.owner.upgrade().ok_or(HistoryError::NullTarget)?;
                let expected = if undo {
                    &edit.after_adjustment
                } else {
                    &edit.before_adjustment
                };
                if !same_adjustment(&target.adjustment(), expected) {
                    return Err(HistoryError::InputChanged);
                }
                Ok(target)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let rollback = |count| {
            for (edit, target) in changes[..count].iter().zip(&targets[..count]).rev() {
                edit.replace(target, !undo);
            }
        };
        for (index, (edit, target)) in changes.iter().zip(&targets).enumerate() {
            if !edit.replace(target, undo) {
                rollback(index);
                return Err(HistoryError::InputChanged);
            }
        }
        let result = other().inspect_err(|_| rollback(changes.len()))?;
        Ok((result, targets))
    }

    fn replace(&self, target: &ReferenceReceiver<E>, undo: bool) -> bool {
        let (before, after) = if undo {
            (&self.after, &self.before)
        } else {
            (&self.before, &self.after)
        };
        let (before_adjustment, after_adjustment) = if undo {
            (&self.after_adjustment, &self.before_adjustment)
        } else {
            (&self.before_adjustment, &self.after_adjustment)
        };
        let moves = !same_adjustment(before_adjustment, after_adjustment);
        if moves && !target.replace_adjustment(before_adjustment, after_adjustment) {
            return false;
        }
        if before.same_value(after) {
            return true;
        }
        let applied = match target {
            ReferenceReceiver::Filter(property) => match (before, after) {
                (Value::ResolvedFilterColor(before), Value::ResolvedFilterColor(after)) => property
                    .wire_metadata()
                    .replace_resolved_lab_color(*before, *after),
                (Value::Gradient(before), Value::Gradient(after)) => {
                    property.replace_adjusted_gradient(before, after)
                }
                _ => false,
            },
            ReferenceReceiver::Solid(property) => match (before, after) {
                (Value::Color(before), Value::Color(after)) => {
                    property.replace_adjusted_color(before, after)
                }
                _ => false,
            },
        };
        if moves && !applied {
            target.replace_adjustment(after_adjustment, before_adjustment);
        }
        applied
    }
}

impl<E: 'static> WeakProperty<E> {
    fn upgrade(&self) -> Option<ReferenceReceiver<E>> {
        Some(match self {
            Self::Filter(property) => ReferenceReceiver::Filter(property.upgrade()?),
            Self::Solid(node) => ReferenceReceiver::Solid(
                PointwiseNode::from_core(&node.upgrade()?)?
                    .solid_model()
                    .ok()?
                    .color_property(),
            ),
        })
    }
}

impl<E: 'static> ReferenceReceiver<E> {
    fn adjustment(&self) -> Option<SharedPropertyAdjustment<E>> {
        match self {
            Self::Filter(property) => property.retained_adjustment(),
            Self::Solid(property) => property.wire_metadata().retained_adjustment(),
        }
    }

    fn replace_adjustment(
        &self,
        before: &Option<SharedPropertyAdjustment<E>>,
        after: &Option<SharedPropertyAdjustment<E>>,
    ) -> bool {
        let metadata = match self {
            Self::Filter(property) => property.wire_metadata(),
            Self::Solid(property) => property.wire_metadata(),
        };
        metadata.compare_and_replace_adjustment(before.as_ref(), after.clone())
    }

    pub(crate) fn notify(&self) -> Result<(), HistoryError<E>> {
        let result = match self {
            Self::Filter(property) => property.notify_change(ContextChangeArgument::shared_empty()),
            Self::Solid(property) => property
                .notify_adjusted_color()
                .map_err(solid_property_error),
        };
        result.map_err(|error| HistoryError::Property(Box::new(error)))
    }
}

fn same_adjustment<E>(
    a: &Option<SharedPropertyAdjustment<E>>,
    b: &Option<SharedPropertyAdjustment<E>>,
) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.same_reference(b),
        (None, None) => true,
        _ => false,
    }
}

impl<E> Value<E> {
    fn tag(&self) -> i32 {
        match self {
            Self::Color(_) | Self::ResolvedFilterColor(_) => 6,
            Self::Gradient(_) => 7,
        }
    }

    fn same_value(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Color(a), Self::Color(b)) => {
                a.equals_components(b)
                    || a.components().map(f64::to_bits) == b.components().map(f64::to_bits)
            }
            (Self::ResolvedFilterColor(a), Self::ResolvedFilterColor(b)) => {
                a.map(|rgb| rgb.map(f64::to_bits)) == b.map(|rgb| rgb.map(f64::to_bits))
            }
            (Self::Gradient(a), Self::Gradient(b)) => same_gradient(a, b),
            _ => false,
        }
    }
}

pub(super) fn same_gradient(a: &Option<SharedGradient>, b: &Option<SharedGradient>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.same_reference(b),
        (None, None) => true,
        _ => false,
    }
}
