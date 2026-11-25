use super::graph::NodeCore;
use super::invert_node::graph_error;
use super::property::wire::{AdjustmentSpec, LegacyWireMetadata, LivePropertyWireError};
use super::property::{NumericAdjustment, NumericPropertyState, WeakPointwiseParameter};
use super::solid::solid_property_error;
use super::{
    ImageGeneratorParameter, ImageGeneratorParameterKind, PointwiseError, PointwiseInvertProperty,
    PointwiseNode, PointwiseParameter, PointwiseParameterValue, SharedPropertyAdjustment,
};
use crate::SharedSolidColorProperty;
use crate::selection::SelectionPosition;
use std::sync::{Arc, Weak};

pub(crate) enum NumericReceiver<E> {
    Filter(PointwiseParameter<E>),
    Generator(ImageGeneratorParameter<E>),
    Transparency(PointwiseNode<E>, SharedSolidColorProperty<E>),
    Invert(PointwiseInvertProperty<E>),
}

/// The retained receiver of a numeric Lab link.
pub struct LabNumericProperty<E = ()>(pub(crate) NumericReceiver<E>);

impl<E> Clone for LabNumericProperty<E> {
    fn clone(&self) -> Self {
        Self(match &self.0 {
            NumericReceiver::Filter(value) => NumericReceiver::Filter(value.clone()),
            NumericReceiver::Generator(value) => NumericReceiver::Generator(value.clone()),
            NumericReceiver::Transparency(node, value) => {
                NumericReceiver::Transparency(node.clone(), value.clone())
            }
            NumericReceiver::Invert(value) => NumericReceiver::Invert(value.clone()),
        })
    }
}

impl<E: 'static> LabNumericProperty<E> {
    pub fn filter(property: &PointwiseParameter<E>) -> Option<Self> {
        Self::adjustable(NumericReceiver::Filter(property.clone()))
    }

    pub fn generator(property: &ImageGeneratorParameter<E>) -> Option<Self> {
        Self::adjustable(NumericReceiver::Generator(property.clone()))
    }

    pub fn transparency(node: &PointwiseNode<E>) -> Option<Self> {
        let property = node.solid_model().ok()?.transparency_property();
        Self::adjustable(NumericReceiver::Transparency(node.clone(), property))
    }

    pub fn invert(property: &PointwiseInvertProperty<E>) -> Option<Self> {
        Self::adjustable(NumericReceiver::Invert(property.clone()))
    }

    fn adjustable(receiver: NumericReceiver<E>) -> Option<Self> {
        if receiver.metadata()?.adjustable() && receiver.shape().ok()?.is_some() {
            Some(Self(receiver))
        } else {
            None
        }
    }

    pub fn is_pair(&self) -> bool {
        matches!(self.0.shape(), Ok(Some(NumericReceiverShape::Pair(_))))
    }
}

pub(crate) enum NumericReceiverShape {
    Number,
    Pair([f64; 2]),
    Choice(usize),
}

#[derive(PartialEq)]
pub(crate) enum NumericReceiverState {
    Property(NumericPropertyState),
    Invert(SelectionPosition),
}

pub(crate) struct WeakNumericReceiver<E>(WeakReceiver<E>);

enum WeakReceiver<E> {
    Filter(WeakPointwiseParameter<E>),
    Generator(Weak<NodeCore<E>>, ImageGeneratorParameterKind),
    Transparency(Weak<NodeCore<E>>),
    Invert(Weak<NodeCore<E>>, i32),
}

impl<E: 'static> WeakNumericReceiver<E> {
    pub(crate) fn upgrade(&self) -> Option<NumericReceiver<E>> {
        let upgrade = |node: &Weak<NodeCore<E>>| PointwiseNode::from_core(&node.upgrade()?);
        Some(match &self.0 {
            WeakReceiver::Filter(value) => NumericReceiver::Filter(value.upgrade()?),
            WeakReceiver::Generator(node, parameter) => NumericReceiver::Generator(
                upgrade(node)?
                    .image_generator_handle()?
                    .parameter(*parameter),
            ),
            WeakReceiver::Transparency(node) => {
                let node = upgrade(node)?;
                let property = node.solid_model().ok()?.transparency_property();
                NumericReceiver::Transparency(node, property)
            }
            WeakReceiver::Invert(node, channel) => {
                NumericReceiver::Invert(upgrade(node)?.invert_property(*channel).ok()?)
            }
        })
    }
}

impl<E: 'static> NumericReceiver<E> {
    pub(crate) fn owner(&self) -> PointwiseNode<E> {
        match self {
            Self::Filter(value) => value.owner().node(),
            Self::Generator(value) => value.owner().node(),
            Self::Transparency(node, _) => node.clone(),
            Self::Invert(value) => value.constructor_owner(),
        }
    }

    pub(crate) fn same_reference(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Filter(a), Self::Filter(b)) => a.same_reference(b),
            (Self::Generator(a), Self::Generator(b)) => a.same_reference(b),
            (Self::Transparency(a, _), Self::Transparency(b, _)) => a.same_reference(b),
            (Self::Invert(a), Self::Invert(b)) => a.same_reference(b),
            _ => false,
        }
    }

    fn metadata(&self) -> Option<LegacyWireMetadata<E>> {
        Some(match self {
            Self::Filter(value) => value.wire_metadata(),
            Self::Generator(value) => value.wire_metadata(),
            Self::Transparency(_, value) => value.wire_metadata(),
            Self::Invert(value) => value.adjustment_metadata()?,
        })
    }

    pub(crate) fn compare_and_replace_adjustment(
        &self,
        expected: Option<&SharedPropertyAdjustment<E>>,
        next: Option<SharedPropertyAdjustment<E>>,
    ) -> bool {
        self.metadata()
            .is_some_and(|metadata| metadata.compare_and_replace_adjustment(expected, next))
    }

    pub(crate) fn new_adjustment(
        &self,
        before: Option<&SharedPropertyAdjustment<E>>,
        paired: bool,
        group: [u8; 16],
        links: &[Option<Vec<u16>>],
    ) -> Result<SharedPropertyAdjustment<E>, LivePropertyWireError<E>> {
        let tag = i32::from(paired);
        let result = if let Some(before) = before {
            let result = before.source_clone()?;
            result.set_property_type(tag);
            result
        } else {
            let spec = self.adjustment_spec()?;
            SharedPropertyAdjustment::new(tag, spec.count, spec.limits.as_ref())?
        };
        result.set_guid(group);
        for (axis, name) in links.iter().enumerate() {
            result.set_link_name(axis as i32, name.as_deref())?;
        }
        Ok(result)
    }

    fn adjustment_spec(&self) -> Result<AdjustmentSpec, LivePropertyWireError<E>> {
        Ok(match self {
            Self::Filter(value) => value.adjustment_spec()?,
            Self::Generator(value) => value.adjustment_spec()?,
            Self::Transparency(_, value) => AdjustmentSpec {
                property_type: 0,
                count: 1,
                limits: Some(value.limits()),
            },
            Self::Invert(value) => AdjustmentSpec {
                property_type: 0,
                count: 1,
                limits: Some(value.choice_limits()?),
            },
        })
    }

    pub(crate) fn mapping_bounds(&self) -> Result<[f32; 2], LivePropertyWireError<E>> {
        let limits = self
            .adjustment_spec()?
            .limits
            .ok_or(PointwiseError::NullReference)?
            .snapshot();
        let force_zero = limits
            .use_range
            .is_some_and(|range| range.copy_wire_value().flags[0]);
        Ok([
            if force_zero {
                0.
            } else {
                limits.minimum.to_framework_f64() as f32
            },
            limits.maximum.to_framework_f64() as f32,
        ])
    }

    pub(crate) fn current(&self) -> Result<NumericAdjustment, PointwiseError<E>> {
        Ok(match self {
            Self::Filter(value) => match value.raw_value() {
                PointwiseParameterValue::Number { current, .. } => {
                    NumericAdjustment::Number(current)
                }
                PointwiseParameterValue::Pair { current, .. } => NumericAdjustment::Pair(current),
                PointwiseParameterValue::Choice { current, .. } => {
                    NumericAdjustment::Choice(current)
                }
                _ => return Err(PointwiseError::InvalidCast),
            },
            Self::Generator(value) if value.is_paired() => NumericAdjustment::Pair(value.pair()?),
            Self::Generator(value) if value.is_choice() => {
                NumericAdjustment::Choice(value.index()?)
            }
            Self::Generator(value) => NumericAdjustment::Number(value.number()?),
            Self::Transparency(_, value) => NumericAdjustment::Number(value.snapshot().current),
            Self::Invert(value) => NumericAdjustment::Choice(value.current().map_err(graph_error)?),
        })
    }

    pub(crate) fn for_node(node: &PointwiseNode<E>) -> Vec<Self> {
        if let Some(filter) = node.filter() {
            return filter
                .retained_parameters()
                .into_iter()
                .map(Self::Filter)
                .collect();
        }
        if let Some(generator) = node.image_generator_handle() {
            return generator
                .parameters()
                .into_iter()
                .map(Self::Generator)
                .collect();
        }
        if let Ok(solid) = node.solid_model() {
            return vec![Self::Transparency(
                node.clone(),
                solid.transparency_property(),
            )];
        }
        node.invert_properties().map_or_else(
            |_| Vec::new(),
            |properties| properties.into_iter().map(Self::Invert).collect(),
        )
    }

    pub(crate) fn downgrade(&self) -> WeakNumericReceiver<E> {
        let downgrade = |node: PointwiseNode<E>| Arc::downgrade(&node.core());
        WeakNumericReceiver(match self {
            Self::Filter(property) => WeakReceiver::Filter(property.downgrade()),
            Self::Generator(property) => {
                WeakReceiver::Generator(downgrade(property.owner().node()), property.kind())
            }
            Self::Transparency(node, _) => WeakReceiver::Transparency(downgrade(node.clone())),
            Self::Invert(property) => {
                WeakReceiver::Invert(downgrade(property.constructor_owner()), property.channel())
            }
        })
    }

    pub(crate) fn retained_adjustment(&self) -> Option<SharedPropertyAdjustment<E>> {
        match self {
            Self::Filter(value) => value.retained_adjustment(),
            Self::Generator(value) => value.retained_adjustment(),
            Self::Transparency(_, value) => value.wire_metadata().retained_adjustment(),
            Self::Invert(value) => value.retained_adjustment(),
        }
    }

    pub(crate) fn shape(&self) -> Result<Option<NumericReceiverShape>, PointwiseError<E>> {
        use NumericReceiverShape as Shape;
        Ok(match self {
            Self::Filter(value) => match value.raw_value() {
                PointwiseParameterValue::Number { .. } => Some(Shape::Number),
                PointwiseParameterValue::Pair { current, .. } => Some(Shape::Pair(current)),
                PointwiseParameterValue::Choice { options, .. } => {
                    Some(Shape::Choice(options.entries().len()))
                }
                _ => None,
            },
            Self::Generator(value) if value.is_paired() => Some(Shape::Pair(value.pair()?)),
            Self::Generator(value) if value.is_choice() => {
                Some(Shape::Choice(value.options()?.entries().len()))
            }
            Self::Generator(value) if value.is_scalar() => Some(Shape::Number),
            Self::Generator(_) => None,
            Self::Transparency(..) => Some(Shape::Number),
            Self::Invert(value) => Some(Shape::Choice(
                value.options().map_err(graph_error)?.entries().len(),
            )),
        })
    }

    pub(crate) fn prepare(
        &self,
        adjustment: NumericAdjustment,
    ) -> Result<(NumericReceiverState, NumericReceiverState), PointwiseError<E>> {
        use NumericPropertyState as State;
        use NumericReceiverState::Property;
        let (before, after) = match (self, adjustment) {
            (Self::Filter(value), adjustment) => value.prepare_numeric_adjustment(adjustment)?,
            (Self::Generator(value), adjustment) => value.prepare_numeric_adjustment(adjustment)?,
            (Self::Transparency(_, property), NumericAdjustment::Number(value)) => {
                let snapshot = property.snapshot();
                let value = property
                    .prepare_adjusted_transparency(value)
                    .map_err(solid_property_error)?;
                (
                    State::Number {
                        current: snapshot.current,
                        previous: snapshot.previous,
                    },
                    State::Number {
                        current: value,
                        previous: value,
                    },
                )
            }
            (Self::Invert(property), NumericAdjustment::Choice(index)) => {
                let (before, after) = property.prepare_adjusted_index(index)?;
                return Ok((
                    NumericReceiverState::Invert(before),
                    NumericReceiverState::Invert(after),
                ));
            }
            _ => return Err(PointwiseError::InvalidCast),
        };
        Ok((Property(before), Property(after)))
    }

    pub(crate) fn write(&self, state: &NumericReceiverState) {
        use NumericReceiverState::Property;
        match (self, state) {
            (Self::Filter(value), Property(state)) => value.write_numeric_adjustment(state),
            (Self::Generator(value), Property(state)) => value.write_numeric_adjustment(state),
            (
                Self::Transparency(_, value),
                Property(NumericPropertyState::Number { current, previous }),
            ) => {
                value.write_adjusted_transparency(*current, *previous);
            }
            (Self::Invert(value), NumericReceiverState::Invert(state)) => {
                value.write_adjusted_index(state)
            }
            _ => unreachable!("retained numeric receiver changed kind"),
        }
    }

    pub(crate) fn normalize(&self) -> Result<(), PointwiseError<E>> {
        match self {
            Self::Filter(value) => value.normalize_numeric_adjustment(),
            Self::Generator(value) => value.normalize_numeric_adjustment(),
            _ => Ok(()),
        }
    }

    pub(crate) fn notify(&self) -> Result<(), PointwiseError<E>> {
        match self {
            Self::Filter(value) => value.notify_numeric_adjustment(),
            Self::Generator(value) => value.notify_numeric_adjustment(),
            Self::Transparency(_, value) => value
                .notify_adjusted_transparency()
                .map_err(solid_property_error),
            Self::Invert(value) => value.notify_adjusted_index(),
        }
    }
}
