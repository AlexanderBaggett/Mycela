//! Retained po.qh/qi variable mapping. The x0 dynamic owner and rendering
//! remain separate capabilities; values and ancestry use the actual graph.
use super::{PointwiseError, PointwiseFilter, PointwiseKind, PointwiseNode, graph::Payload};
use crate::{EnvironmentRecord, SharedSolidColorProperty, SolidColorProperty};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct MappedVariableValue {
    number: f64,
    color: [f64; 3],
    selection: Option<Vec<u16>>,
    environment: Option<Arc<EnvironmentRecord>>,
}
impl PartialEq for MappedVariableValue {
    fn eq(&self, other: &Self) -> bool {
        self.number == other.number
            && self.color == other.color
            && self.selection == other.selection
            && match (&self.environment, &other.environment) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
    }
}
impl MappedVariableValue {
    pub fn number(&self) -> f64 {
        self.number
    }
    pub fn color(&self) -> [f64; 3] {
        self.color
    }
    pub fn selection(&self) -> Option<&[u16]> {
        self.selection.as_deref()
    }
    pub fn environment(&self) -> Option<&Arc<EnvironmentRecord>> {
        self.environment.as_ref()
    }
}
type BindingAxisAndPoints = (Option<usize>, Option<Vec<(f64, f64)>>);

// The shipped binding maps use invariant decimal text, including signed
// values and leading decimal points. Other Double.Parse cultures still
// require explicit host behavior.
fn binding_points(text: &[u16]) -> Option<Vec<(f64, f64)>> {
    fn number(value: &[u16]) -> Option<f64> {
        let literal = String::from_utf16(value).ok()?;
        let number: f64 = literal.trim().parse().ok()?;
        number.is_finite().then_some(number)
    }
    let mut points = Vec::new();
    for pair in text.split(|ch| *ch == u16::from(b';')) {
        let mut fields = pair.split(|ch| *ch == u16::from(b'>'));
        let x = number(fields.next()?)?;
        let y = number(fields.next()?)?;
        if fields.next().is_some() {
            return None;
        }
        points.push((x, y));
    }
    points.sort_by(|a, b| a.0.total_cmp(&b.0));
    if points.windows(2).any(|p| p[0].0 == p[1].0) {
        return None;
    }
    Some(points)
}
fn apply_binding_points(points: &[(f64, f64)], value: f64) -> f64 {
    let index = points
        .iter()
        .position(|point| point.0 >= value)
        .unwrap_or(points.len());
    if index == 0 {
        points[0].1
    } else if index == points.len() {
        points[index - 1].1
    } else {
        let (x0, y0) = points[index - 1];
        let (x1, y1) = points[index];
        y0 + (value - x0) / (x1 - x0) * (y1 - y0)
    }
}

fn binding_owner<E: 'static>(
    node: &super::PointwiseNode<E>,
) -> Result<Option<PointwiseFilter<E>>, PointwiseError<E>> {
    let mut current = node.parent();
    let mut seen = std::collections::BTreeSet::new();
    while let Some(node) = current {
        if !seen.insert(node.id) {
            return Err(PointwiseError::UnsupportedCapability(
                "cyclic variable-owner ancestry",
            ));
        }
        if let Some(filter) = node.filter()
            && matches!(
                filter.kind(),
                PointwiseKind::MapVariable | PointwiseKind::WizardGroup
            )
        {
            return Ok(Some(filter));
        }
        current = node.parent();
    }
    Ok(None)
}
fn binding_axis_and_points(name: Option<Vec<u16>>, is_pair: bool) -> Option<BindingAxisAndPoints> {
    let name = name.unwrap_or_default();
    let (axis, text) = if is_pair && !name.is_empty() {
        match name[0] {
            87 | 88 | 119 | 120 => (Some(0), &name[1..]),
            72 | 89 | 104 | 121 => (Some(1), &name[1..]),
            _ => (None, name.as_slice()),
        }
    } else {
        (None, name.as_slice())
    };
    let points = if text.is_empty() {
        None
    } else {
        Some(binding_points(text)?)
    };
    Some((axis, points))
}

const SOLID_PROPERTIES: [SolidColorProperty; 2] =
    [SolidColorProperty::Color, SolidColorProperty::Transparency];

fn solid_bound_value<E: 'static>(
    property: &SharedSolidColorProperty<E>,
    owner: &PointwiseFilter<E>,
    time: f64,
) -> Result<MappedVariableValue, PointwiseError<E>> {
    let index = property.binding_index();
    let mut value = match owner.kind() {
        PointwiseKind::MapVariable => owner.mapped_variable_value(index, time)?,
        PointwiseKind::WizardGroup => owner.wizard_variable_value(index, time)?,
        _ => return Err(PointwiseError::InvalidCast),
    };
    let (_, points) = binding_axis_and_points(property.binding_name(), false).ok_or(
        PointwiseError::UnsupportedCapability("positive live property binding context"),
    )?;
    if let Some(points) = points {
        value.number = apply_binding_points(&points, value.number);
        if property.kind() == SolidColorProperty::Color {
            value.color = [value.number * 0.01; 3];
        }
    }
    Ok(value)
}

fn solid_binding_is_animated<E: 'static>(
    owner: &PointwiseFilter<E>,
    index: i32,
) -> Result<bool, PointwiseError<E>> {
    match owner.kind() {
        PointwiseKind::MapVariable => owner.mapped_variable_is_animated(index),
        PointwiseKind::WizardGroup => owner.wizard_variable_is_animated(index),
        _ => Err(PointwiseError::InvalidCast),
    }
}

impl<E: 'static> PointwiseNode<E> {
    pub fn solid_binding_index(&self, kind: SolidColorProperty) -> Result<i32, PointwiseError<E>> {
        Ok(self.solid_property(kind)?.binding_index())
    }

    pub fn solid_binding_mapping(
        &self,
        kind: SolidColorProperty,
    ) -> Result<Option<Vec<u16>>, PointwiseError<E>> {
        Ok(self.solid_property(kind)?.binding_name())
    }

    pub fn set_solid_binding_index(
        &self,
        kind: SolidColorProperty,
        index: i32,
    ) -> Result<(), PointwiseError<E>> {
        let property = self.solid_property(kind)?;
        if property.binding_index() != index {
            property.set_binding_index(index);
            self.invalidate();
            self.refresh_solid_binding(&property, self.binding_sample_time(), true, false)?;
        }
        Ok(())
    }

    pub fn set_solid_binding_mapping(
        &self,
        kind: SolidColorProperty,
        mapping: Option<&[u16]>,
    ) -> Result<(), PointwiseError<E>> {
        let property = self.solid_property(kind)?;
        if property.binding_name().as_deref().unwrap_or_default() != mapping.unwrap_or_default() {
            property.set_binding_name(mapping);
            self.invalidate();
            self.refresh_solid_binding(&property, self.binding_sample_time(), true, false)?;
        }
        Ok(())
    }

    fn solid_binding_owner(
        &self,
        property: &SharedSolidColorProperty<E>,
    ) -> Result<Option<PointwiseFilter<E>>, PointwiseError<E>> {
        if property.binding_index() < 0 {
            return Ok(None);
        }
        binding_owner(self)
    }

    pub(super) fn animated_solid_binding_value(
        &self,
        kind: SolidColorProperty,
        time: f64,
    ) -> Result<Option<MappedVariableValue>, PointwiseError<E>> {
        let property = self.solid_property(kind)?;
        let Some(owner) = self.solid_binding_owner(&property)? else {
            return Ok(None);
        };
        if !solid_binding_is_animated(&owner, property.binding_index())? {
            return Ok(None);
        }
        solid_bound_value(&property, &owner, time).map(Some)
    }

    pub(super) fn solid_property_is_animated(
        &self,
        property: &SharedSolidColorProperty<E>,
    ) -> Result<bool, PointwiseError<E>> {
        if property.is_animated()? {
            return Ok(true);
        }
        let Some(owner) = self.solid_binding_owner(property)? else {
            return Ok(false);
        };
        solid_binding_is_animated(&owner, property.binding_index())
    }

    fn refresh_solid_binding(
        &self,
        property: &SharedSolidColorProperty<E>,
        time: f64,
        notify: bool,
        force: bool,
    ) -> Result<(), PointwiseError<E>> {
        if notify && self.is_clone_mode() {
            return Ok(());
        }
        let Some(owner) = self.solid_binding_owner(property)? else {
            return Ok(());
        };
        let value = solid_bound_value(property, &owner, time)?;
        property
            .refresh_bound_value(&value, force, notify)
            .map_err(super::solid::solid_property_error)
    }

    pub(super) fn refresh_solid_bindings_for_preview(
        &self,
        time: f64,
    ) -> Result<(), PointwiseError<E>> {
        for kind in SOLID_PROPERTIES {
            self.refresh_solid_binding(&self.solid_property(kind)?, time, false, false)?;
        }
        Ok(())
    }
}

impl<E: 'static> super::PointwiseParameter<E> {
    fn binding_owner(&self) -> Result<Option<PointwiseFilter<E>>, PointwiseError<E>> {
        binding_owner(&self.owner().node())
    }
    // os.a strips the first W/X/H/Y marker only for pair properties.
    fn binding_axis_and_points(&self) -> Option<BindingAxisAndPoints> {
        let is_pair = matches!(
            self.raw_value(),
            super::PointwiseParameterValue::Pair { .. }
        );
        binding_axis_and_points(self.binding_name(), is_pair)
    }
    pub(super) fn binding_applies_to_axis(&self, axis: usize) -> Result<bool, PointwiseError<E>> {
        let (selected, _) =
            self.binding_axis_and_points()
                .ok_or(PointwiseError::UnsupportedCapability(
                    "positive live property binding context",
                ))?;
        Ok(selected.is_none_or(|selected| selected == axis))
    }
    pub(super) fn animated_binding_supported(&self) -> Result<bool, PointwiseError<E>> {
        let pair = matches!(
            self.raw_value(),
            super::PointwiseParameterValue::Pair { .. }
        );
        if !pair
            && !matches!(
                self.raw_value(),
                super::PointwiseParameterValue::Number { .. }
                    | super::PointwiseParameterValue::Choice { .. }
                    | super::PointwiseParameterValue::Color(_)
            )
        {
            return Ok(false);
        }
        let own_animation = super::parameter_ops::is_animated(|| self.retained_animation())?;
        if !pair && own_animation {
            return Ok(true);
        }
        if pair && self.axis_is_animated(0)? && self.axis_is_animated(1)? {
            return Ok(true);
        }
        let Some(owner) = self.binding_owner()? else {
            // os.aj has no variable owner; getters use the stored value.
            return Ok(true);
        };
        let active = match owner.kind() {
            PointwiseKind::MapVariable => {
                owner.mapped_variable_is_animated(self.binding_index())?
            }
            PointwiseKind::WizardGroup => {
                owner.wizard_variable_is_animated(self.binding_index())?
            }
            _ => false,
        };
        if active || (pair && own_animation) {
            Ok(self.binding_axis_and_points().is_some())
        } else {
            // Nonanimated bindings read their retained value; source change
            // notifications update that value separately.
            Ok(true)
        }
    }
    pub(super) fn has_animated_binding(&self) -> Result<bool, PointwiseError<E>> {
        if self.binding_index() < 0
            || matches!(
                self.raw_value(),
                super::PointwiseParameterValue::Environment(_)
            )
        {
            return Ok(false);
        }
        let Some(owner) = self.binding_owner()? else {
            return Ok(false);
        };
        match owner.kind() {
            PointwiseKind::MapVariable => owner.mapped_variable_is_animated(self.binding_index()),
            PointwiseKind::WizardGroup => owner.wizard_variable_is_animated(self.binding_index()),
            _ => Ok(false),
        }
    }
    pub(super) fn refresh_binding_for_preview(&self, time: f64) -> Result<(), PointwiseError<E>> {
        self.refresh_bound_value(time, false, false)
    }
    pub(super) fn animated_binding_value(
        &self,
        time: f64,
    ) -> Result<Option<MappedVariableValue>, PointwiseError<E>> {
        let pair = matches!(
            self.raw_value(),
            super::PointwiseParameterValue::Pair { .. }
        );
        let own_animation = super::parameter_ops::is_animated(|| self.retained_animation())?;
        if self.binding_index() < 0 || (own_animation && !pair) {
            return Ok(None);
        }
        let Some(owner) = self.binding_owner()? else {
            return Ok(None);
        };
        let index = self.binding_index();
        match owner.kind() {
            PointwiseKind::MapVariable
                if (pair && own_animation) || owner.mapped_variable_is_animated(index)? => {}
            PointwiseKind::WizardGroup
                if (pair && own_animation) || owner.wizard_variable_is_animated(index)? => {}
            _ => return Ok(None),
        }
        self.bound_value_from_owner(&owner, time).map(Some)
    }
    fn bound_value_from_owner(
        &self,
        owner: &PointwiseFilter<E>,
        time: f64,
    ) -> Result<MappedVariableValue, PointwiseError<E>> {
        let index = self.binding_index();
        let mut value = match owner.kind() {
            PointwiseKind::MapVariable => owner.mapped_variable_value(index, time)?,
            PointwiseKind::WizardGroup => owner.wizard_variable_value(index, time)?,
            _ => return Err(PointwiseError::InvalidCast),
        };
        let (_, points) =
            self.binding_axis_and_points()
                .ok_or(PointwiseError::UnsupportedCapability(
                    "positive live property binding context",
                ))?;
        if let Some(points) = points {
            value.number = apply_binding_points(&points, value.number);
            if matches!(self.raw_value(), super::PointwiseParameterValue::Color(_)) {
                value.color = [value.number * 0.01; 3];
            }
        }
        Ok(value)
    }
    /// Write a bound owner's value at the given sample time and publish the
    /// source's empty change argument when the value changes.
    pub fn apply_variable_binding_at(&self, time: f64) -> Result<(), PointwiseError<E>> {
        self.refresh_bound_value(time, true, false)
    }

    fn refresh_bound_value(
        &self,
        time: f64,
        notify: bool,
        force: bool,
    ) -> Result<(), PointwiseError<E>> {
        if self.binding_index() < 0 || (notify && self.owner().node().is_clone_mode()) {
            return Ok(());
        }
        if matches!(
            self.raw_value(),
            super::PointwiseParameterValue::Environment(_)
        ) {
            if let Some(source) = self.environment_binding_source()? {
                let value = source.effective_environment()?;
                if self.write_environment(value, force)? && notify {
                    self.notify_change(crate::ContextChangeArgument::shared_empty())?;
                }
            }
            return Ok(());
        }
        let Some(owner) = self.binding_owner()? else {
            return Ok(());
        };
        let value = self.bound_value_from_owner(&owner, time)?;
        let (axis, _) =
            self.binding_axis_and_points()
                .ok_or(PointwiseError::UnsupportedCapability(
                    "positive live property binding context",
                ))?;
        if self.write_bound_value(&value, axis, force)? && notify {
            self.notify_change(crate::ContextChangeArgument::shared_empty())?;
        }
        Ok(())
    }

    pub(super) fn refresh_binding_after_edit(&self) -> Result<(), PointwiseError<E>> {
        self.apply_variable_binding_at(self.owner().node().binding_sample_time())
    }

    /// Resolve an Environment binding through the actual group ancestry.
    pub fn environment_binding_source(&self) -> Result<Option<Self>, PointwiseError<E>> {
        let index = self.binding_index();
        if index < 0 || self.owner().node().is_final_graph_clone() {
            return Ok(None);
        }
        let mut owner = self.binding_owner()?;
        let mut seen = std::collections::BTreeSet::new();
        while let Some(filter) = owner {
            let node = filter.node();
            if !seen.insert(node.id) {
                return Err(PointwiseError::UnsupportedCapability(
                    "cyclic variable-owner ancestry",
                ));
            }
            if filter.kind() == PointwiseKind::WizardGroup {
                if !(0..20).contains(&index)
                    || filter.parameter(index + 3)?.variable()?.mode()
                        != crate::VARIABLE_ENVIRONMENT
                {
                    return Ok(None);
                }
                return filter
                    .wizard_variable_definition_control(index)
                    .map(|(_, control)| control);
            }
            if filter.parameter(9)?.number() as i32 == index {
                return Ok(None);
            }
            owner = binding_owner(&node)?;
        }
        Ok(None)
    }

    /// The original Environment getter uses time zero. Static bound controls
    /// also live on detached graph copies so captures can resolve their source.
    pub fn effective_environment(&self) -> Result<Arc<EnvironmentRecord>, PointwiseError<E>> {
        let mut property = self.clone();
        let mut seen = Vec::new();
        while let Some(owner) = property.environment_binding_source()? {
            if seen.iter().any(|old: &Self| old.same_reference(&owner)) {
                return Err(PointwiseError::UnsupportedCapability(
                    "cyclic Environment binding",
                ));
            }
            seen.push(owner.clone());
            property = owner;
        }
        match property.raw_value() {
            super::PointwiseParameterValue::Environment(value) => Ok(value),
            _ => Err(PointwiseError::InvalidCast),
        }
    }
}

impl<E: 'static> super::GradientGeneratorParameter<E> {
    fn binding_source_is_animated(
        &self,
        owner: &PointwiseFilter<E>,
    ) -> Result<bool, PointwiseError<E>> {
        match owner.kind() {
            PointwiseKind::MapVariable => owner.mapped_variable_is_animated(self.binding_index()),
            PointwiseKind::WizardGroup => owner.wizard_variable_is_animated(self.binding_index()),
            _ => Err(PointwiseError::InvalidCast),
        }
    }

    pub(super) fn has_animated_binding(&self) -> Result<bool, PointwiseError<E>> {
        if self.binding_index() < 0 {
            return Ok(false);
        }
        match binding_owner(&self.owner().node())? {
            Some(owner) => self.binding_source_is_animated(&owner),
            None => Ok(false),
        }
    }

    pub(super) fn animated_binding_supported(&self) -> Result<bool, PointwiseError<E>> {
        if !(self.is_scalar() || self.is_paired() || self.is_choice()) {
            return Ok(false);
        }
        let own_animation = self.is_animated()?;
        if !self.is_paired() && own_animation {
            return Ok(true);
        }
        if self.is_paired() && self.axis_is_animated(0)? && self.axis_is_animated(1)? {
            return Ok(true);
        }
        let Some(owner) = binding_owner(&self.owner().node())? else {
            return Ok(true);
        };
        let active = self.binding_source_is_animated(&owner)?;
        Ok(!(active || (self.is_paired() && own_animation))
            || binding_axis_and_points(self.binding_name(), self.is_paired()).is_some())
    }
    pub(super) fn animated_binding_value(
        &self,
        time: f64,
    ) -> Result<Option<MappedVariableValue>, PointwiseError<E>> {
        let own_animation = self.is_animated()?;
        if self.binding_index() < 0 || (own_animation && !self.is_paired()) {
            return Ok(None);
        }
        let Some(owner) = binding_owner(&self.owner().node())? else {
            return Ok(None);
        };
        let active = self.binding_source_is_animated(&owner)?;
        if !active && !(self.is_paired() && own_animation) {
            return Ok(None);
        }
        self.bound_value_from_owner(&owner, time).map(Some)
    }
    pub(super) fn binding_applies_to_axis(&self, axis: usize) -> Result<bool, PointwiseError<E>> {
        let (selected, _) = binding_axis_and_points(self.binding_name(), self.is_paired()).ok_or(
            PointwiseError::UnsupportedCapability("positive live property binding context"),
        )?;
        Ok(selected.is_none_or(|selected| selected == axis))
    }
    /// Change binding metadata and refresh the retained value as os.k does.
    pub fn set_binding_index_notifying(&self, index: i32) -> Result<(), PointwiseError<E>> {
        if self.binding_index() != index {
            self.set_binding_index(index);
            self.refresh_binding_after_edit()?;
        }
        Ok(())
    }
    /// Null and empty binding names compare equal in the original setter.
    pub fn set_binding_name_notifying(
        &self,
        name: Option<&[u16]>,
    ) -> Result<(), PointwiseError<E>> {
        if self.binding_name().as_deref().unwrap_or_default() != name.unwrap_or_default() {
            self.set_binding_name(name);
            self.refresh_binding_after_edit()?;
        }
        Ok(())
    }
    fn refresh_binding_after_edit(&self) -> Result<(), PointwiseError<E>> {
        self.refresh_bound_generator_value(self.owner().node().binding_sample_time(), true, false)
    }

    pub(super) fn refresh_binding_for_preview(&self, time: f64) -> Result<(), PointwiseError<E>> {
        self.refresh_bound_generator_value(time, false, false)
    }

    fn refresh_bound_generator_value(
        &self,
        time: f64,
        notify: bool,
        force: bool,
    ) -> Result<(), PointwiseError<E>> {
        if self.binding_index() < 0 || (notify && self.owner().node().is_clone_mode()) {
            return Ok(());
        }
        let Some(owner) = binding_owner(&self.owner().node())? else {
            return Ok(());
        };
        let value = self.bound_value_from_owner(&owner, time)?;
        let (axis, _) = binding_axis_and_points(self.binding_name(), self.is_paired()).ok_or(
            PointwiseError::UnsupportedCapability("positive live property binding context"),
        )?;
        let changed = if self.is_paired() {
            self.write_bound_pair(axis, value.number, force)?
        } else if self.is_choice() {
            let options = self.options()?.entries();
            let index = options
                .iter()
                .position(|item| item.as_deref() == value.selection())
                .map_or_else(
                    || crate::framework_cast::integer(value.number()),
                    |index| index as i32,
                );
            if index < 0 || index as usize >= options.len() {
                return Err(PointwiseError::Exception);
            }
            self.write_bound_choice(index, force)
        } else if self.is_scalar() {
            self.write_bound_number(value.number, force)?
        } else {
            return Err(PointwiseError::UnsupportedCapability(
                "variable binding generator property type",
            ));
        };
        if changed && notify {
            self.notify_bound_change()?;
        }
        Ok(())
    }
    fn bound_value_from_owner(
        &self,
        owner: &PointwiseFilter<E>,
        time: f64,
    ) -> Result<MappedVariableValue, PointwiseError<E>> {
        let index = self.binding_index();
        let mut value = match owner.kind() {
            PointwiseKind::MapVariable => owner.mapped_variable_value(index, time)?,
            PointwiseKind::WizardGroup => owner.wizard_variable_value(index, time)?,
            _ => return Err(PointwiseError::InvalidCast),
        };
        let (_, points) = binding_axis_and_points(self.binding_name(), self.is_paired()).ok_or(
            PointwiseError::UnsupportedCapability("positive live property binding context"),
        )?;
        if let Some(points) = points {
            value.number = apply_binding_points(&points, value.number);
        }
        Ok(value)
    }
}

impl<E: 'static> PointwiseFilter<E> {
    // zl.g(index, temporary) walks actual contents and input occurrences.
    // A Map Variable may additionally remap the changed index before the
    // original index continues through its slots and contents.
    pub(super) fn propagate_variable_change(
        &self,
        index: i32,
        force: bool,
    ) -> Result<(), PointwiseError<E>> {
        if self.kind() != PointwiseKind::WizardGroup {
            return Err(PointwiseError::InvalidCast);
        }
        enum Frame<E> {
            Enter(super::PointwiseNode<E>, i32),
            Leave(super::region::NodeId),
        }
        let mut frames = Vec::new();
        for slot in self.node().contents().into_iter().rev() {
            if let Some(child) = slot.node() {
                frames.push(Frame::Enter(child, index));
            }
        }
        let mut active = std::collections::BTreeSet::new();
        while let Some(frame) = frames.pop() {
            match frame {
                Frame::Leave(id) => {
                    active.remove(&id);
                }
                Frame::Enter(node, variable) => {
                    if !active.insert(node.id) {
                        return Err(PointwiseError::UnsupportedCapability(
                            "cyclic variable propagation",
                        ));
                    }
                    let mut remap = None;
                    if let Some(filter) = node.filter() {
                        let properties = filter.parameters();
                        let mut mapped = false;
                        for property in properties {
                            if property.binding_index() == variable {
                                mapped = true;
                                if !node.is_clone_mode() {
                                    property.refresh_bound_value(
                                        node.binding_sample_time(),
                                        true,
                                        force,
                                    )?;
                                }
                            }
                        }
                        if filter.kind() == PointwiseKind::MapVariable && mapped {
                            let target = filter.parameter(9)?.number() as i32;
                            if target != variable {
                                remap = Some(target);
                            }
                        }
                    }
                    if let Some(generator) = node.image_generator_handle() {
                        for property in generator.parameters() {
                            if property.binding_index() == variable && !node.is_clone_mode() {
                                property.refresh_bound_generator_value(
                                    node.binding_sample_time(),
                                    true,
                                    force,
                                )?;
                            }
                        }
                    }
                    if let Payload::Solid { model, .. } = &node.core().payload {
                        for property in [model.color_property(), model.transparency_property()] {
                            if property.binding_index() == variable {
                                node.refresh_solid_binding(
                                    &property,
                                    node.binding_sample_time(),
                                    true,
                                    force,
                                )?;
                            }
                        }
                    }
                    frames.push(Frame::Leave(node.id));
                    if node.kind() != Some(PointwiseKind::WizardGroup) {
                        for slot in node.contents().into_iter().rev() {
                            if let Some(child) = slot.node() {
                                frames.push(Frame::Enter(child, variable));
                            }
                        }
                    }
                    for slot in node.slots().into_iter().rev() {
                        if let Some(child) = slot.node() {
                            frames.push(Frame::Enter(child, variable));
                        }
                    }
                    if let Some(mapped) = remap {
                        for slot in node.contents().into_iter().rev() {
                            if let Some(child) = slot.node() {
                                frames.push(Frame::Enter(child, mapped));
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
    /// Evaluate this mapping or its nearest retained variable-owner ancestor.
    pub fn mapped_variable_value(
        &self,
        index: i32,
        time: f64,
    ) -> Result<MappedVariableValue, PointwiseError<E>> {
        if self.kind() != PointwiseKind::MapVariable {
            return Err(PointwiseError::InvalidCast);
        }
        let mut current = Some(self.node());
        let mut seen = std::collections::BTreeSet::new();
        while let Some(node) = current {
            if !seen.insert(node.id) {
                return Err(PointwiseError::UnsupportedCapability(
                    "cyclic variable-owner ancestry",
                ));
            }
            if let Some(filter) = node.filter() {
                match filter.kind() {
                    PointwiseKind::MapVariable if filter.parameter(9)?.number() as i32 == index => {
                        let a = filter.parameter(5)?.effective_value_at(time)?;
                        let b = filter.parameter(6)?.effective_value_at(time)?;
                        let c = filter.parameter(7)?.effective_value_at(time)?;
                        let d = filter.parameter(8)?.effective_value_at(time)?;
                        let mut number =
                            (filter.parameter(3)?.effective_value_at(time)? * a + b) * c + d;
                        let round = filter.parameter(10)?;
                        let selected = round.effective_index_at(time)?;
                        let yes: Vec<u16> = "Yes".encode_utf16().collect();
                        let options = round.options()?.entries();
                        if usize::try_from(selected)
                            .ok()
                            .and_then(|i| options.get(i))
                            .and_then(|item| item.as_deref())
                            == Some(yes.as_slice())
                        {
                            number = number.round_ties_even();
                        }
                        let color = filter
                            .parameter(4)?
                            .effective_color_at(time)?
                            .components()
                            .map(|v| (v * a + b) * c + d);
                        return Ok(MappedVariableValue {
                            number,
                            color,
                            selection: None,
                            environment: None,
                        });
                    }
                    PointwiseKind::WizardGroup => return filter.wizard_variable_value(index, time),
                    _ => {}
                }
            }
            current = node.parent();
        }
        Ok(MappedVariableValue {
            number: 0.,
            color: [0.; 3],
            selection: None,
            environment: None,
        })
    }
    /// aff.qh maps a variable specification to its retained dynamic control.
    pub fn wizard_variable_value(
        &self,
        index: i32,
        time: f64,
    ) -> Result<MappedVariableValue, PointwiseError<E>> {
        let (spec, control) = self.wizard_variable_control(index)?;
        let mut result = MappedVariableValue {
            number: 0.,
            color: [0.; 3],
            selection: None,
            environment: None,
        };
        let Some(control) = control else {
            return Ok(result);
        };
        match spec.mode() {
            crate::VARIABLE_NUMERIC => {
                result.number = control.effective_value_at(time)?;
                if spec.numeric_flags()[0] {
                    result.number = (result.number + 100.) / 2.;
                }
                result.color = [result.number / 100.; 3];
            }
            crate::VARIABLE_SELECTION => {
                let selected = control.effective_index_at(time)?;
                result.number = f64::from(selected);
                let options = control.options()?.entries();
                result.selection = options
                    .get(usize::try_from(selected).map_err(|_| PointwiseError::Index)?)
                    .ok_or(PointwiseError::Index)?
                    .clone();
                result.color = [result.number / 100.; 3];
            }
            crate::VARIABLE_COLOR => {
                result.color = control.effective_color_at(time)?.components();
                let [r, g, b] = result.color.map(|value| value as f32);
                let maximum = if r > g { r } else { g };
                let maximum = if b > maximum { b } else { maximum };
                result.number = f64::from(maximum * 100.0_f32);
            }
            crate::VARIABLE_ENVIRONMENT => {
                result.environment = Some(control.effective_environment()?)
            }
            _ => {
                return Err(PointwiseError::UnsupportedCapability(
                    "Wizard Group variable mode",
                ));
            }
        }
        Ok(result)
    }
    /// aff.qi reads the active derived control's actual animation state.
    pub fn wizard_variable_is_animated(&self, index: i32) -> Result<bool, PointwiseError<E>> {
        // aff.qi returns false for a final clone, which has no lab owner.
        if self.kind() == PointwiseKind::WizardGroup && self.node().is_final_graph_clone() {
            return Ok(false);
        }
        let (_, control) = self.wizard_variable_control(index)?;
        match control {
            Some(control) => super::parameter_ops::is_animated(|| control.retained_animation()),
            None => Ok(false),
        }
    }
    fn wizard_variable_control(
        &self,
        index: i32,
    ) -> Result<(crate::VariableSpec, Option<super::PointwiseParameter<E>>), PointwiseError<E>>
    {
        if self.kind() != PointwiseKind::WizardGroup {
            return Err(PointwiseError::InvalidCast);
        }
        if self.node().is_clone_mode() {
            return Err(PointwiseError::UnsupportedCapability(
                "Wizard Group clone variable owner",
            ));
        }
        self.wizard_variable_definition_control(index)
    }

    fn wizard_variable_definition_control(
        &self,
        index: i32,
    ) -> Result<(crate::VariableSpec, Option<super::PointwiseParameter<E>>), PointwiseError<E>>
    {
        if !(0..20).contains(&index) {
            return Err(PointwiseError::Index);
        }
        let spec = self.parameter(index + 3)?.variable()?;
        if spec.mode() == crate::VARIABLE_INACTIVE {
            return Ok((spec, None));
        }
        let mut control_index = 0;
        for previous in 0..index {
            if self.parameter(previous + 3)?.variable()?.mode() != crate::VARIABLE_INACTIVE {
                control_index += 1;
            }
        }
        let controls = self.wizard_dynamic_properties()?;
        let control = controls
            .get(control_index)
            .cloned()
            .ok_or(PointwiseError::Index)?;
        Ok((spec, Some(control)))
    }
    /// po.qi checks the six mapped-value properties, including their own
    /// variable-driven animation, but excludes Round and the index.
    pub fn mapped_variable_is_animated(&self, index: i32) -> Result<bool, PointwiseError<E>> {
        if self.kind() != PointwiseKind::MapVariable {
            return Err(PointwiseError::InvalidCast);
        }
        self.mapped_variable_is_animated_inner(index, &mut std::collections::BTreeSet::new())
    }
    fn mapped_variable_is_animated_inner(
        &self,
        index: i32,
        active: &mut std::collections::BTreeSet<super::region::NodeId>,
    ) -> Result<bool, PointwiseError<E>> {
        let node = self.node();
        if !active.insert(node.id) {
            return Err(PointwiseError::UnsupportedCapability(
                "cyclic variable-owner ancestry",
            ));
        }
        let result = (|| {
            if self.parameter(9)?.number() as i32 == index {
                for i in 3..=8 {
                    let property = self.parameter(i)?;
                    if super::parameter_ops::is_animated(|| property.retained_animation())? {
                        return Ok(true);
                    }
                    if property.binding_index() >= 0
                        && let Some(owner) = property.binding_owner()?
                    {
                        let animated = match owner.kind() {
                            PointwiseKind::MapVariable => owner.mapped_variable_is_animated_inner(
                                property.binding_index(),
                                active,
                            )?,
                            PointwiseKind::WizardGroup => {
                                owner.wizard_variable_is_animated(property.binding_index())?
                            }
                            _ => false,
                        };
                        if animated {
                            return Ok(true);
                        }
                    }
                }
                return Ok(false);
            }
            let mut parent = node.parent();
            let mut seen = std::collections::BTreeSet::new();
            while let Some(candidate) = parent {
                if !seen.insert(candidate.id) {
                    return Err(PointwiseError::UnsupportedCapability(
                        "cyclic variable-owner ancestry",
                    ));
                }
                if let Some(filter) = candidate.filter() {
                    match filter.kind() {
                        PointwiseKind::MapVariable => {
                            return filter.mapped_variable_is_animated_inner(index, active);
                        }
                        PointwiseKind::WizardGroup => {
                            return filter.wizard_variable_is_animated(index);
                        }
                        _ => {}
                    }
                }
                parent = candidate.parent();
            }
            Ok(false)
        })();
        active.remove(&node.id);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BinaryWriter, ContextColorValue, ImageGeometry, SharedImageMap};

    #[test]
    fn wizard_environment_binding_survives_map_variable_wire_and_frozen_final_render() {
        use crate::{
            EnvironmentField, EnvironmentLayerKind, Gtx375GraphDocument, PointwiseRenderEffects,
            PointwiseRenderOptions, ProceduralExecution, RenderCaptureSession,
        };
        let wizard = PointwiseFilter::<()>::new(PointwiseKind::WizardGroup).unwrap();
        let mut numeric = crate::VariableSpec::new(crate::VARIABLE_NUMERIC);
        numeric.set_name(Some(&"Strength".encode_utf16().collect::<Vec<_>>()));
        wizard.parameter(4).unwrap().set_variable(numeric).unwrap();
        let number = wizard.wizard_dynamic_properties().unwrap().remove(0);
        number.set_number(76.).unwrap();
        assert_eq!(wizard.add_wizard_environment_variable().unwrap(), Some(0));
        let controls = wizard.wizard_dynamic_properties().unwrap();
        let source = &controls[0];
        assert!(!source.is_animatable());
        assert!(controls[1].same_reference(&number));
        assert_eq!(number.number(), 76.);
        let group = PointwiseFilter::<()>::new(PointwiseKind::MapVariable).unwrap();
        group.parameter(9).unwrap().set_number(1.).unwrap();
        let make = PointwiseFilter::<()>::new(PointwiseKind::MakeEnvironment).unwrap();
        wizard
            .node()
            .content(0)
            .unwrap()
            .set_node(Some(&group.node()))
            .unwrap();
        group
            .node()
            .content(0)
            .unwrap()
            .set_node(Some(&make.node()))
            .unwrap();
        let property = make.parameter(0).unwrap();
        property.set_binding_index_notifying(0).unwrap();
        let red = |value| {
            let mut map = EnvironmentRecord::default();
            map.add_layer(EnvironmentLayerKind::Color).unwrap();
            map.set_field(EnvironmentField::Red(0), value).unwrap();
            Arc::new(map)
        };
        let before = red(0.625);
        source.set_environment(before.clone()).unwrap();
        assert!(Arc::ptr_eq(
            &property.effective_environment().unwrap(),
            &before
        ));
        assert!(
            matches!(property.raw_value(), super::super::PointwiseParameterValue::Environment(value) if Arc::ptr_eq(&value, &before))
        );
        property.set_environment(red(0.125)).unwrap();
        let capture = RenderCaptureSession::new()
            .capture_node(&make.node(), 0.)
            .unwrap();
        source.set_environment(red(0.25)).unwrap();
        let channel = |map: SharedImageMap| map.channel(0, true).unwrap().read(0).unwrap();
        assert_eq!(
            channel(
                make.node()
                    .render_original(16, 16)
                    .unwrap()
                    .image()
                    .unwrap()
            ),
            0.25
        );
        let mut capacity = |_| Ok(true);
        let mut preflight = |_, _| Ok(());
        let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight);
        let frozen = capture
            .render_export_with_effects(
                ImageGeometry::new(16, 16),
                ProceduralExecution::CallingThread,
                PointwiseRenderOptions::new(false, false),
                &mut effects,
            )
            .unwrap();
        assert!(frozen.caught_errors().is_empty());
        assert_eq!(channel(frozen.image().unwrap()), 0.625);

        let mut doc = Gtx375GraphDocument::new(vec![Some(wizard.node())]).unwrap();
        let mut bytes = BinaryWriter::default();
        doc.save(&mut bytes).unwrap();
        let restored = Gtx375GraphDocument::<()>::load(bytes.bytes()).unwrap();
        let wizard = restored.branch(0).unwrap().unwrap();
        let restored_child = wizard
            .content(0)
            .unwrap()
            .node()
            .unwrap()
            .content(0)
            .unwrap()
            .node()
            .unwrap();
        assert_eq!(
            channel(
                restored_child
                    .render_original(16, 16)
                    .unwrap()
                    .image()
                    .unwrap()
            ),
            0.25
        );
    }

    #[test]
    fn wizard_static_controls_update_pair_choice_and_color_properties() {
        struct NoFiles;
        impl crate::VariableFileService for NoFiles {
            type Error = ();
            fn files(&mut self, _: &[u16]) -> Result<Vec<Vec<u16>>, ()> {
                panic!("literal options")
            }
        }
        let wizard = PointwiseFilter::<()>::new(PointwiseKind::WizardGroup).unwrap();
        for (index, mode, name) in [
            (0, crate::VARIABLE_NUMERIC, "Offset"),
            (1, crate::VARIABLE_SELECTION, "Toggle"),
            (2, crate::VARIABLE_COLOR, "Tint"),
        ] {
            let mut spec = crate::VariableSpec::new(mode);
            spec.set_name(Some(&name.encode_utf16().collect::<Vec<_>>()));
            if mode == crate::VARIABLE_SELECTION {
                spec.set_options(
                    Some(&"No;Yes".encode_utf16().collect::<Vec<_>>()),
                    &[],
                    &mut NoFiles,
                )
                .unwrap();
            }
            wizard
                .parameter(index + 3)
                .unwrap()
                .set_variable(spec)
                .unwrap();
        }
        let controls = wizard.wizard_dynamic_properties().unwrap();
        let map = PointwiseFilter::<()>::new(PointwiseKind::MapVariable).unwrap();
        map.parameter(9).unwrap().set_number(19.).unwrap();
        wizard
            .node()
            .content(0)
            .unwrap()
            .set_node(Some(&map.node()))
            .unwrap();
        let transform = PointwiseFilter::<()>::new(PointwiseKind::FreeTransform).unwrap();
        map.node()
            .content(0)
            .unwrap()
            .set_node(Some(&transform.node()))
            .unwrap();
        let colorizer = PointwiseFilter::<()>::new(PointwiseKind::WizardColor).unwrap();
        transform
            .node()
            .slot(0)
            .unwrap()
            .set_node(Some(&colorizer.node()))
            .unwrap();
        let offset = transform.parameter(1).unwrap();
        offset
            .set_binding_name_notifying(Some(&"X".encode_utf16().collect::<Vec<_>>()))
            .unwrap();
        offset.set_binding_index_notifying(0).unwrap();
        let choice = map.parameter(10).unwrap();
        choice.set_binding_index_notifying(1).unwrap();
        let color = colorizer.parameter(0).unwrap();
        color.set_binding_index_notifying(2).unwrap();
        controls[0].set_number(40.).unwrap();
        controls[1].set_index(1).unwrap();
        controls[2]
            .set_color(ContextColorValue::unlinked([0.2, 0.4, 0.8]))
            .unwrap();
        assert_eq!(offset.pair().unwrap(), [40., 0.]);
        assert_eq!(choice.index(), 0); // "Yes" is first in Round.
        assert_eq!(color.color().unwrap().components(), [0.2, 0.4, 0.8]);
    }

    #[test]
    fn wizard_static_changes_update_generator_properties() {
        use super::super::GradientGeneratorParameterKind as Kind;
        struct NoFiles;
        impl crate::VariableFileService for NoFiles {
            type Error = ();
            fn files(&mut self, _: &[u16]) -> Result<Vec<Vec<u16>>, ()> {
                panic!("literal options")
            }
        }
        let wizard = PointwiseFilter::<()>::new(PointwiseKind::WizardGroup).unwrap();
        let mut numeric = crate::VariableSpec::new(crate::VARIABLE_NUMERIC);
        numeric.set_name(Some(&"Offset".encode_utf16().collect::<Vec<_>>()));
        wizard.parameter(3).unwrap().set_variable(numeric).unwrap();
        let mut selection = crate::VariableSpec::new(crate::VARIABLE_SELECTION);
        selection.set_name(Some(&"Mode".encode_utf16().collect::<Vec<_>>()));
        selection
            .set_options(
                Some(&"Vertical;Horizontal".encode_utf16().collect::<Vec<_>>()),
                &[],
                &mut NoFiles,
            )
            .unwrap();
        wizard
            .parameter(4)
            .unwrap()
            .set_variable(selection)
            .unwrap();
        let controls = wizard.wizard_dynamic_properties().unwrap();
        let map = PointwiseFilter::<()>::new(PointwiseKind::MapVariable).unwrap();
        map.parameter(9).unwrap().set_number(19.).unwrap();
        wizard
            .node()
            .content(0)
            .unwrap()
            .set_node(Some(&map.node()))
            .unwrap();
        let generator = super::super::GradientGenerator::<()>::new().unwrap();
        map.node()
            .content(0)
            .unwrap()
            .set_node(Some(&generator.node()))
            .unwrap();
        let location = generator.parameter(Kind::Location);
        location
            .set_binding_name_notifying(Some(&[b'Y' as u16]))
            .unwrap();
        location.set_binding_index_notifying(0).unwrap();
        let mode = generator.parameter(Kind::Mode);
        mode.set_binding_index_notifying(1).unwrap();
        controls[0].set_number(40.).unwrap();
        controls[1].set_index(1).unwrap();
        assert_eq!(location.pair().unwrap()[1], 40.);
        assert_eq!(mode.index().unwrap(), 0); // Horizontal is first in the generator.
    }

    #[test]
    fn wizard_static_change_updates_bound_properties_through_map_variable() {
        use std::sync::{Arc, Mutex};
        let wizard = PointwiseFilter::<()>::new(PointwiseKind::WizardGroup).unwrap();
        let mut spec = crate::VariableSpec::new(crate::VARIABLE_NUMERIC);
        spec.set_name(Some(&"Factor".encode_utf16().collect::<Vec<_>>()));
        wizard.parameter(3).unwrap().set_variable(spec).unwrap();
        let control = wizard.wizard_dynamic_properties().unwrap().remove(0);
        let map = PointwiseFilter::<()>::new(PointwiseKind::MapVariable).unwrap();
        map.parameter(9).unwrap().set_number(1.).unwrap();
        map.parameter(5).unwrap().set_number(2.).unwrap();
        wizard
            .node()
            .content(0)
            .unwrap()
            .set_node(Some(&map.node()))
            .unwrap();
        let blur = PointwiseFilter::<()>::new(PointwiseKind::GaussianBlur).unwrap();
        map.node()
            .content(0)
            .unwrap()
            .set_node(Some(&blur.node()))
            .unwrap();
        let mapped_original = map.parameter(3).unwrap();
        let radius = blur.parameter(1).unwrap();
        mapped_original.set_binding_index_notifying(0).unwrap();
        radius.set_binding_index_notifying(1).unwrap();
        let order = Arc::new(Mutex::new(Vec::new()));
        let first = order.clone();
        mapped_original.add_change_handler(crate::PointwisePropertyHandler::new(move |_, _| {
            first.lock().unwrap().push("map");
            Ok(())
        }));
        let second = order.clone();
        radius.add_change_handler(crate::PointwisePropertyHandler::new(move |_, _| {
            second.lock().unwrap().push("radius");
            Ok(())
        }));
        control.set_number(0.3).unwrap();
        assert_eq!(mapped_original.number(), 0.3);
        assert_eq!(radius.number(), 0.6);
        assert_eq!(*order.lock().unwrap(), vec!["map", "radius"]);
        control.set_number(0.4).unwrap();
        assert_eq!(radius.number(), 0.8);
    }

    #[test]
    fn gradient_preview_refreshes_bound_size_and_reference_captures_the_selected_frame() {
        use crate::{GradientCachedRenderMode::Preview, GradientGeneratorParameterKind::Size};
        use crate::{
            PointwiseRenderEffects, PointwiseRenderOptions, ProceduralExecution::CallingThread,
        };
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };

        let map = PointwiseFilter::<()>::new(PointwiseKind::MapVariable).unwrap();
        map.parameter(0).unwrap().set_number(1.).unwrap();
        let variable = map.parameter(3).unwrap();
        variable.set_axis_animated(0, true).unwrap();
        let curve = variable.animation_curve(0).unwrap();
        curve.set_minimum(0.);
        curve.set_maximum(1.);
        curve.append(&[Some(crate::CurveLayerHandle::new(
            crate::AnalyticCurveKind::Linear,
        ))]);

        let gradient = crate::GradientGenerator::new().unwrap();
        let size = gradient.parameter(Size);
        size.set_binding_index(0);
        size.set_binding_name(Some(&[b'X' as u16]));
        let notifications = Arc::new(AtomicUsize::new(0));
        let count = notifications.clone();
        size.add_changed_handler(&crate::GradientGeneratorChangeHandler::new(move |_, _| {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }));
        map.node()
            .slot(0)
            .unwrap()
            .set_node(Some(&gradient.node()))
            .unwrap();
        let reference = PointwiseFilter::new(PointwiseKind::Reference).unwrap();
        map.node()
            .content(0)
            .unwrap()
            .set_node(Some(&reference.node()))
            .unwrap();

        let output = gradient
            .render_cached(Preview, 4, 2, 0., CallingThread)
            .unwrap()
            .output()
            .unwrap();
        assert_eq!(output.frames().len(), 32);
        let middle = &output.frames()[16];
        assert_eq!(middle.materialize(0).unwrap().read(1).unwrap(), 0.125);
        assert_eq!(notifications.load(Ordering::SeqCst), 0);
        assert!(size.pair().unwrap()[0] > 0.9);

        let mut capacity = |_| Ok(true);
        let mut preflight = |_, _| Ok(());
        let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight);
        let options = PointwiseRenderOptions::new(false, false);
        let reference_node = reference.node();
        let live = reference_node
            .render_original_with_effects(4, 2, options, 0.5, CallingThread, &mut effects)
            .unwrap()
            .image()
            .unwrap();
        assert!(live.same_reference(middle));
        let past_end = reference_node
            .render_original_with_effects(4, 2, options, 1., CallingThread, &mut effects)
            .unwrap();
        assert!(matches!(past_end.caught_errors(), [PointwiseError::Index]));
        let snapshot = crate::RenderCaptureSession::new()
            .capture_node(&reference_node, 0.5)
            .unwrap();
        size.set_axis(0, 0.25).unwrap();
        assert!(gradient.cached_render_output(Preview).is_none());
        let frozen = snapshot
            .render_with_execution_and_effects(4, 2, CallingThread, options, &mut effects)
            .unwrap()
            .image()
            .unwrap();
        assert!(!frozen.same_reference(middle));
        assert_eq!(frozen.materialize(0).unwrap().read(1).unwrap(), 0.125);
    }

    #[test]
    fn animated_map_binding_drives_generator_and_detached_capture() {
        use super::super::GradientGeneratorParameterKind as Kind;
        let map = PointwiseFilter::<()>::new(PointwiseKind::MapVariable).unwrap();
        let variable = map.parameter(3).unwrap();
        variable.set_axis_animated(0, true).unwrap();
        let curve = variable.animation_curve(0).unwrap();
        curve.set_minimum(0.);
        curve.set_maximum(100.);
        curve.append(&[Some(crate::CurveLayerHandle::new(
            crate::AnalyticCurveKind::Linear,
        ))]);
        let generator = super::super::GradientGenerator::<()>::new().unwrap();
        map.node()
            .content(0)
            .unwrap()
            .set_node(Some(&generator.node()))
            .unwrap();
        let location = generator.parameter(Kind::Location);
        location.set_pair([2., 3.]).unwrap();
        location.set_binding_index(0);
        location.set_binding_name(Some(&"X0>10;100>50".encode_utf16().collect::<Vec<_>>()));
        assert_eq!(location.raw_value_at(0, 0.5).unwrap(), 30.);
        assert_eq!(location.raw_value_at(1, 0.5).unwrap(), 3.);
        let mode = generator.parameter(Kind::Mode);
        mode.set_binding_index(0);
        mode.set_binding_name(Some(&"0>0;100>2".encode_utf16().collect::<Vec<_>>()));
        assert_eq!(mode.effective_index_at(0.5).unwrap(), 1);
        mode.set_binding_index(-1);
        let noise = super::super::GradientGenerator::<()>::with_kind(
            super::super::ImageGeneratorKind::Noise,
        )
        .unwrap();
        map.node()
            .slot(0)
            .unwrap()
            .set_node(Some(&noise.node()))
            .unwrap();
        let iterations = noise.parameter(Kind::Iterations);
        iterations.set_binding_index(0);
        iterations.set_binding_name(Some(&"0>1;100>5".encode_utf16().collect::<Vec<_>>()));
        assert_eq!(iterations.effective_value_at(0.5).unwrap(), 3.);
        let snapshot = crate::RenderCaptureSession::new()
            .capture_generator(&generator, 0.5)
            .unwrap();
        curve.set_maximum(0.);
        let actual = snapshot
            .render(4, 4)
            .unwrap()
            .materialize(0)
            .unwrap()
            .values();
        let expected = super::super::GradientGenerator::<()>::new().unwrap();
        expected
            .parameter(Kind::Location)
            .set_pair([30., 3.])
            .unwrap();
        let expected = crate::RenderCaptureSession::new()
            .capture_generator(&expected, 0.)
            .unwrap()
            .render(4, 4)
            .unwrap()
            .materialize(0)
            .unwrap()
            .values();
        assert_eq!(actual, expected);
    }

    #[test]
    fn animated_pair_bindings_honor_axis_prefixes_and_local_animation() {
        let owner = PointwiseFilter::<()>::new(PointwiseKind::MapVariable).unwrap();
        let variable = owner.parameter(3).unwrap();
        variable.set_axis_animated(0, true).unwrap();
        let curve = variable.animation_curve(0).unwrap();
        curve.set_minimum(0.);
        curve.set_maximum(100.);
        curve.append(&[Some(crate::CurveLayerHandle::new(
            crate::AnalyticCurveKind::Linear,
        ))]);
        let transform = PointwiseFilter::<()>::new(PointwiseKind::FreeTransform).unwrap();
        owner
            .node()
            .content(0)
            .unwrap()
            .set_node(Some(&transform.node()))
            .unwrap();
        let position = transform.parameter(1).unwrap();
        position.set_pair([2., 3.]).unwrap();
        position.set_binding_index(0);
        assert_eq!(position.raw_value_at(0, 0.5).unwrap(), 50.);
        assert_eq!(position.raw_value_at(1, 0.5).unwrap(), 50.);
        position.set_binding_name(Some(&"X".encode_utf16().collect::<Vec<_>>()));
        assert_eq!(position.raw_value_at(0, 0.5).unwrap(), 50.);
        assert_eq!(position.raw_value_at(1, 0.5).unwrap(), 3.);
        position.set_binding_name(Some(&"w".encode_utf16().collect::<Vec<_>>()));
        assert_eq!(position.raw_value_at(0, 0.5).unwrap(), 50.);
        assert_eq!(position.raw_value_at(1, 0.5).unwrap(), 3.);
        position.set_binding_name(Some(&"H".encode_utf16().collect::<Vec<_>>()));
        assert_eq!(position.raw_value_at(0, 0.5).unwrap(), 2.);
        assert_eq!(position.raw_value_at(1, 0.5).unwrap(), 50.);
        position.set_binding_name(Some(&"y0>10;100>50".encode_utf16().collect::<Vec<_>>()));
        assert_eq!(position.raw_value_at(0, 0.5).unwrap(), 2.);
        assert_eq!(position.raw_value_at(1, 0.5).unwrap(), 30.);
        position.set_axis_animated(0, true).unwrap();
        let local = position.animation_curve(0).unwrap();
        local.set_minimum(7.);
        local.set_maximum(7.);
        local.append(&[Some(crate::CurveLayerHandle::new(
            crate::AnalyticCurveKind::Linear,
        ))]);
        assert_eq!(position.raw_value_at(0, 0.5).unwrap(), 7.);
        assert_eq!(position.raw_value_at(1, 0.5).unwrap(), 30.);
        variable.set_axis_animated(0, false).unwrap();
        variable.set_number(13.).unwrap();
        position.set_binding_name(Some(&"Y".encode_utf16().collect::<Vec<_>>()));
        assert_eq!(position.raw_value_at(1, 0.5).unwrap(), 13.); // Local X animation activates aj().
        position.set_binding_name(Some(&"Y.5>-1.5".encode_utf16().collect::<Vec<_>>()));
        assert_eq!(position.raw_value_at(1, 0.5).unwrap(), -1.5);
        position.set_binding_name(Some(&"Y0.5.5>1".encode_utf16().collect::<Vec<_>>()));
        assert!(matches!(
            position.raw_value_at(1, 0.5),
            Err(PointwiseError::UnsupportedCapability(
                "positive live property binding context"
            ))
        ));
    }

    #[test]
    fn animated_binding_interpolates_integer_points_for_scalar_and_color() {
        let map = PointwiseFilter::<()>::new(PointwiseKind::MapVariable).unwrap();
        let variable = map.parameter(3).unwrap();
        variable.set_axis_animated(0, true).unwrap();
        let curve = variable.animation_curve(0).unwrap();
        curve.set_minimum(0.);
        curve.set_maximum(100.);
        curve.append(&[Some(crate::CurveLayerHandle::new(
            crate::AnalyticCurveKind::Linear,
        ))]);
        let blur = PointwiseFilter::<()>::new(PointwiseKind::GaussianBlur).unwrap();
        let radius = blur.parameter(1).unwrap();
        radius.set_binding_index(0);
        radius.set_binding_name(Some(&"100>80;0>20".encode_utf16().collect::<Vec<_>>()));
        map.node()
            .content(0)
            .unwrap()
            .set_node(Some(&blur.node()))
            .unwrap();
        assert_eq!(radius.effective_value_at(0.).unwrap(), 20.);
        assert_eq!(radius.effective_value_at(0.5).unwrap(), 50.);
        assert_eq!(radius.effective_value_at(1.).unwrap(), 80.);
        let colorizer = PointwiseFilter::<()>::new(PointwiseKind::WizardColor).unwrap();
        let color = colorizer.parameter(0).unwrap();
        color.set_binding_index(0);
        color.set_binding_name(Some(&"100>80;0>20".encode_utf16().collect::<Vec<_>>()));
        map.node()
            .content(0)
            .unwrap()
            .set_node(Some(&colorizer.node()))
            .unwrap();
        assert_eq!(color.effective_color_at(0.).unwrap().components(), [0.2; 3]);
        assert_eq!(
            color.effective_color_at(0.5).unwrap().components(),
            [0.5; 3]
        );
        assert_eq!(color.effective_color_at(1.).unwrap().components(), [0.8; 3]);
        color.set_binding_name(Some(&"0>1;0>2".encode_utf16().collect::<Vec<_>>()));
        assert!(matches!(
            color.effective_color_at(0.),
            Err(PointwiseError::UnsupportedCapability(
                "positive live property binding context"
            ))
        ));
    }

    #[test]
    fn nested_map_variable_binding_propagates_ancestor_animation() {
        let outer = PointwiseFilter::<()>::new(PointwiseKind::MapVariable).unwrap();
        let original = outer.parameter(3).unwrap();
        original.set_axis_animated(0, true).unwrap();
        let curve = original.animation_curve(0).unwrap();
        curve.set_minimum(0.1);
        curve.set_maximum(0.8);
        curve.append(&[Some(crate::CurveLayerHandle::new(
            crate::AnalyticCurveKind::Linear,
        ))]);
        let inner = PointwiseFilter::<()>::new(PointwiseKind::MapVariable).unwrap();
        inner.parameter(9).unwrap().set_number(1.).unwrap();
        inner.parameter(3).unwrap().set_binding_index(0);
        outer
            .node()
            .content(0)
            .unwrap()
            .set_node(Some(&inner.node()))
            .unwrap();
        let blur = PointwiseFilter::<()>::new(PointwiseKind::GaussianBlur).unwrap();
        blur.parameter(1).unwrap().set_binding_index(1);
        inner
            .node()
            .content(0)
            .unwrap()
            .set_node(Some(&blur.node()))
            .unwrap();
        assert!(inner.mapped_variable_is_animated(1).unwrap());
        let expected = outer.mapped_variable_value(0, 1.).unwrap().number();
        assert_eq!(
            inner.mapped_variable_value(1, 1.).unwrap().number(),
            expected
        );
        assert_eq!(
            blur.parameter(1).unwrap().effective_value_at(1.).unwrap(),
            expected
        );
        let frozen = crate::RenderCaptureSession::new()
            .capture_node(&outer.node(), 1.)
            .unwrap();
        let mut capacity = |_| Ok(true);
        let mut preflight = |_, _| Ok(());
        let mut effects = crate::PointwiseRenderEffects::new(&mut capacity, &mut preflight);
        assert!(
            frozen
                .render_with_execution_and_effects(
                    2,
                    2,
                    crate::ProceduralExecution::CallingThread,
                    crate::PointwiseRenderOptions::new(false, false),
                    &mut effects
                )
                .is_ok()
        );
    }

    #[test]
    fn animated_variable_binding_maps_selection_by_text_then_numeric_fallback() {
        struct NoFiles;
        impl crate::VariableFileService for NoFiles {
            type Error = ();
            fn files(&mut self, _: &[u16]) -> Result<Vec<Vec<u16>>, ()> {
                panic!("literal options")
            }
        }
        let wizard = PointwiseFilter::<()>::new(PointwiseKind::WizardGroup).unwrap();
        let mut spec = crate::VariableSpec::new(crate::VARIABLE_SELECTION);
        spec.set_name(Some(&"Toggle".encode_utf16().collect::<Vec<_>>()));
        spec.set_options(
            Some(&"No;Yes".encode_utf16().collect::<Vec<_>>()),
            &[],
            &mut NoFiles,
        )
        .unwrap();
        wizard
            .parameter(3)
            .unwrap()
            .set_variable(spec.clone())
            .unwrap();
        let control = wizard.wizard_dynamic_properties().unwrap().remove(0);
        control.set_axis_animated(0, true).unwrap();
        let curve = control.animation_curve(0).unwrap();
        curve.set_minimum(2.);
        curve.set_maximum(2.);
        curve.append(&[Some(crate::CurveLayerHandle::new(
            crate::AnalyticCurveKind::Linear,
        ))]);
        let map = PointwiseFilter::<()>::new(PointwiseKind::MapVariable).unwrap();
        wizard
            .node()
            .content(0)
            .unwrap()
            .set_node(Some(&map.node()))
            .unwrap();
        let choice = map.parameter(10).unwrap();
        choice.set_binding_index(0);
        assert_eq!(choice.effective_index_at(0.).unwrap(), 0); // Yes is index zero in Round.
        choice.set_binding_name(Some(&"0>99".encode_utf16().collect::<Vec<_>>()));
        assert_eq!(choice.effective_index_at(0.).unwrap(), 0); // Text still takes precedence.
        choice.set_binding_name(None);
        spec.set_options(
            Some(&"Other;Unknown".encode_utf16().collect::<Vec<_>>()),
            &[],
            &mut NoFiles,
        )
        .unwrap();
        wizard.parameter(3).unwrap().set_variable(spec).unwrap();
        let control = wizard.wizard_dynamic_properties().unwrap().remove(0);
        control.set_axis_animated(0, true).unwrap();
        let curve = control.animation_curve(0).unwrap();
        curve.set_minimum(2.);
        curve.set_maximum(2.);
        curve.append(&[Some(crate::CurveLayerHandle::new(
            crate::AnalyticCurveKind::Linear,
        ))]);
        choice.set_binding_name(Some(&"0>99".encode_utf16().collect::<Vec<_>>()));
        assert_eq!(choice.effective_index_at(0.).unwrap(), 99); // No matching text: use mapped number.
        choice.set_binding_name(None);
        assert_eq!(choice.effective_index_at(0.).unwrap(), 1); // Unmapped source index.
        choice.set_binding_name(Some(&"0.5.5>1".encode_utf16().collect::<Vec<_>>()));
        assert!(matches!(
            choice.effective_index_at(0.),
            Err(PointwiseError::UnsupportedCapability(
                "positive live property binding context"
            ))
        ));
    }

    #[test]
    fn animated_map_binding_returns_retained_color_components() {
        let map = PointwiseFilter::<()>::new(PointwiseKind::MapVariable).unwrap();
        map.parameter(4)
            .unwrap()
            .set_color(ContextColorValue::unlinked([0.2, 0.4, 0.6]))
            .unwrap();
        map.parameter(5).unwrap().set_number(2.).unwrap();
        map.parameter(3)
            .unwrap()
            .set_axis_animated(0, true)
            .unwrap();
        let colorizer = PointwiseFilter::<()>::new(PointwiseKind::WizardColor).unwrap();
        let color = colorizer.parameter(0).unwrap();
        color.set_binding_index(0);
        map.node()
            .content(0)
            .unwrap()
            .set_node(Some(&colorizer.node()))
            .unwrap();
        assert_eq!(
            color.effective_color_at(0.).unwrap().components(),
            [0.4, 0.8, 1.2]
        );
        color.set_binding_name(Some(&"0.5.5>1".encode_utf16().collect::<Vec<_>>()));
        assert!(matches!(
            color.effective_color_at(0.),
            Err(PointwiseError::UnsupportedCapability(
                "positive live property binding context"
            ))
        ));
        color.set_axis_animated(0, true).unwrap();
        assert!(color.effective_color_at(0.).is_ok());
    }

    #[test]
    fn animated_wizard_variable_binding_uses_dynamic_control_and_guards_static_values() {
        let wizard = PointwiseFilter::<()>::new(PointwiseKind::WizardGroup).unwrap();
        let mut spec = crate::VariableSpec::new(crate::VARIABLE_NUMERIC);
        spec.set_name(Some(&"Factor".encode_utf16().collect::<Vec<_>>()));
        spec.set_numeric_flags(true, false, false);
        wizard.parameter(3).unwrap().set_variable(spec).unwrap();
        let control = wizard.wizard_dynamic_properties().unwrap().remove(0);
        let blur = PointwiseFilter::<()>::new(PointwiseKind::GaussianBlur).unwrap();
        let radius = blur.parameter(1).unwrap();
        radius.set_binding_index(0);
        wizard
            .node()
            .content(0)
            .unwrap()
            .set_node(Some(&blur.node()))
            .unwrap();
        assert_eq!(radius.effective_value_at(0.).unwrap(), radius.number());
        control.set_axis_animated(0, true).unwrap();
        let curve = control.animation_curve(0).unwrap();
        curve.set_minimum(-40.);
        curve.set_maximum(60.);
        curve.append(&[Some(crate::CurveLayerHandle::new(
            crate::AnalyticCurveKind::Linear,
        ))]);
        let expected = wizard.wizard_variable_value(0, 1.).unwrap().number();
        assert_eq!(radius.effective_value_at(1.).unwrap(), expected);
        assert_eq!(expected, 80.);
    }

    #[test]
    fn animated_map_binding_drives_retained_scalar_and_captured_image_graph() {
        let map = PointwiseFilter::<()>::new(PointwiseKind::MapVariable).unwrap();
        let variable = map.parameter(3).unwrap();
        variable.set_axis_animated(0, true).unwrap();
        let curve = variable.animation_curve(0).unwrap();
        curve.set_minimum(0.1);
        curve.set_maximum(0.8);
        curve.append(&[Some(crate::CurveLayerHandle::new(
            crate::AnalyticCurveKind::Linear,
        ))]);
        let input = crate::PointwiseNode::<()>::input();
        let image = SharedImageMap::new(ImageGeometry::new(5, 5));
        image
            .set_buffer(
                0,
                Some(&crate::FloatBuffer::new(
                    (0..25).map(|i| i as f32 / 25.).collect(),
                )),
            )
            .unwrap();
        input.set_input_image(Some(image)).unwrap();
        let blur = PointwiseFilter::<()>::new(PointwiseKind::GaussianBlur).unwrap();
        blur.node().slot(0).unwrap().set_node(Some(&input)).unwrap();
        let radius = blur.parameter(1).unwrap();
        radius.set_binding_index(0);
        map.node()
            .content(0)
            .unwrap()
            .set_node(Some(&blur.node()))
            .unwrap();
        assert_eq!(
            radius.effective_value_at(1.).unwrap(),
            map.mapped_variable_value(0, 1.).unwrap().number()
        );
        assert!(radius.effective_value_at(1.).unwrap() > radius.effective_value_at(0.).unwrap());
        let snapshot = crate::RenderCaptureSession::new()
            .capture_node(&map.node(), 1.)
            .unwrap();
        let mut capacity = |_| Ok(true);
        let mut preflight = |_, _| Ok(());
        let mut effects = crate::PointwiseRenderEffects::new(&mut capacity, &mut preflight);
        let result = snapshot
            .render_with_execution_and_effects(
                5,
                5,
                crate::ProceduralExecution::CallingThread,
                crate::PointwiseRenderOptions::new(false, false),
                &mut effects,
            )
            .unwrap();
        assert!(result.image().is_some());
        radius.set_binding_name(Some(&"0.5.5>1;100>2".encode_utf16().collect::<Vec<_>>()));
        assert!(matches!(
            radius.effective_value_at(1.),
            Err(PointwiseError::UnsupportedCapability(
                "positive live property binding context"
            ))
        ));
        radius.set_axis_animated(0, true).unwrap();
        assert!(radius.effective_value_at(1.).is_ok()); // Own animation precedes binding.
    }

    #[test]
    fn map_variable_original_graph_and_capture_preserve_reference_input() {
        let map = PointwiseFilter::<()>::new(PointwiseKind::MapVariable).unwrap();
        map.parameter(0).unwrap().set_number(1.).unwrap();
        map.parameter(3).unwrap().set_number(12.5).unwrap();
        let source = crate::PointwiseNode::<()>::solid_color().unwrap();
        source.set_solid_color([0.2, 0.4, 0.6]).unwrap();
        map.node().slot(0).unwrap().set_node(Some(&source)).unwrap();
        let reference = PointwiseFilter::<()>::new(PointwiseKind::Reference).unwrap();
        let invert = crate::PointwiseNode::<()>::invert();
        invert
            .slot(0)
            .unwrap()
            .set_node(Some(&reference.node()))
            .unwrap();
        map.node()
            .content(0)
            .unwrap()
            .set_node(Some(&invert))
            .unwrap();
        let options = crate::PointwiseRenderOptions::new(true, false);
        let output = map
            .node()
            .render_original_with_options(2, 2, options)
            .unwrap()
            .image()
            .unwrap();
        assert!((output.channel(0, false).unwrap().read(0).unwrap() - 0.8).abs() < 1e-6);
        let clone = map.source_clone().unwrap();
        assert_eq!(clone.mapped_variable_value(0, 0.).unwrap().number(), 12.5);
        let frozen = crate::RenderCaptureSession::new()
            .capture_node(&map.node(), 0.)
            .unwrap();
        source.set_solid_color([0.7, 0.4, 0.6]).unwrap();
        map.parameter(3).unwrap().set_number(90.).unwrap();
        assert_eq!(clone.mapped_variable_value(0, 0.).unwrap().number(), 12.5);
        let mut capacity = |_| Ok(true);
        let mut preflight = |_, _| Ok(());
        let mut effects = crate::PointwiseRenderEffects::new(&mut capacity, &mut preflight);
        let captured = frozen
            .render_with_execution_and_effects(
                2,
                2,
                crate::ProceduralExecution::CallingThread,
                options,
                &mut effects,
            )
            .unwrap()
            .image()
            .unwrap();
        assert!((captured.channel(0, false).unwrap().read(0).unwrap() - 0.8).abs() < 1e-6);
        let live = map
            .node()
            .render_original_with_options(2, 2, options)
            .unwrap()
            .image()
            .unwrap();
        assert!((live.channel(0, false).unwrap().read(0).unwrap() - 0.3).abs() < 1e-6);
    }

    #[test]
    fn map_variable_document_roundtrip_retains_properties_inputs_and_contents() {
        let map = PointwiseFilter::<()>::new(PointwiseKind::MapVariable).unwrap();
        map.parameter(3).unwrap().set_number(12.5).unwrap();
        map.parameter(9).unwrap().set_number(4.).unwrap();
        map.parameter(10).unwrap().set_index(0).unwrap();
        map.load_wizard_label("Map editor".encode_utf16().collect());
        let input = crate::PointwiseNode::<()>::solid_color().unwrap();
        input.set_solid_color([0.2, 0.4, 0.6]).unwrap();
        map.node().slot(0).unwrap().set_node(Some(&input)).unwrap();
        let reference = PointwiseFilter::<()>::new(PointwiseKind::Reference).unwrap();
        map.node()
            .content(0)
            .unwrap()
            .set_node(Some(&reference.node()))
            .unwrap();
        let extra = map.append_group_content([17., 42.]).unwrap();
        extra
            .set_node(Some(&crate::PointwiseNode::<()>::invert()))
            .unwrap();
        let mut document = crate::Gtx375GraphDocument::new(vec![Some(map.node())]).unwrap();
        let mut writer = BinaryWriter::default();
        document.save(&mut writer).unwrap();
        let mut loaded = crate::Gtx375GraphDocument::<()>::load(writer.bytes()).unwrap();
        let node = loaded.branches()[0].clone().unwrap();
        let restored = node.filter().unwrap();
        assert_eq!(restored.kind(), PointwiseKind::MapVariable);
        assert_eq!(restored.parameter(3).unwrap().number(), 12.5);
        assert_eq!(restored.parameter(9).unwrap().number(), 4.);
        assert_eq!(restored.parameter(10).unwrap().index(), 0);
        assert_eq!(
            restored.wizard_label().unwrap(),
            "Map editor".encode_utf16().collect::<Vec<_>>()
        );
        assert_eq!(restored.group_content_positions(), vec![[17., 42.]]);
        assert_eq!(node.contents().len(), 2);
        assert_eq!(
            node.content(0).unwrap().node().unwrap().kind(),
            Some(PointwiseKind::Reference)
        );
        assert_eq!(
            node.content(1).unwrap().node().unwrap().identity_name(),
            "Invert".encode_utf16().collect::<Vec<_>>()
        );
        assert_eq!(
            node.slot(0).unwrap().node().unwrap().identity_name(),
            "Solid Color".encode_utf16().collect::<Vec<_>>()
        );
        let mut second = BinaryWriter::default();
        loaded.save(&mut second).unwrap();
        assert_eq!(writer.bytes(), second.bytes());
    }

    #[test]
    fn wizard_variables_evaluate_dynamic_numeric_selection_and_color_through_map_ancestry() {
        struct NoFiles;
        impl crate::VariableFileService for NoFiles {
            type Error = ();
            fn files(&mut self, _: &[u16]) -> Result<Vec<Vec<u16>>, ()> {
                panic!("literal options need no filesystem")
            }
        }
        let wizard = PointwiseFilter::<()>::new(PointwiseKind::WizardGroup).unwrap();
        let mut numeric = crate::VariableSpec::new(crate::VARIABLE_NUMERIC);
        numeric.set_name(Some(&"Depth".encode_utf16().collect::<Vec<_>>()));
        numeric.set_numeric_flags(true, false, false);
        wizard.parameter(5).unwrap().set_variable(numeric).unwrap();
        let mut select = crate::VariableSpec::new(crate::VARIABLE_SELECTION);
        select.set_name(Some(&"Shape".encode_utf16().collect::<Vec<_>>()));
        select
            .set_options(
                Some(&"Circle;Square;Triangle".encode_utf16().collect::<Vec<_>>()),
                &[],
                &mut NoFiles,
            )
            .unwrap();
        wizard.parameter(8).unwrap().set_variable(select).unwrap();
        let mut color = crate::VariableSpec::new(crate::VARIABLE_COLOR);
        color.set_name(Some(&"Tint".encode_utf16().collect::<Vec<_>>()));
        wizard.parameter(10).unwrap().set_variable(color).unwrap();
        let controls = wizard.wizard_dynamic_properties().unwrap();
        assert_eq!(controls.len(), 3);
        controls[0].set_number(-25.).unwrap();
        controls[1].set_index(2).unwrap();
        controls[2]
            .set_color(ContextColorValue::unlinked([0.25, 0.75, 0.5]))
            .unwrap();
        let map = PointwiseFilter::<()>::new(PointwiseKind::MapVariable).unwrap();
        map.parameter(9).unwrap().set_number(19.).unwrap();
        wizard
            .node()
            .content(0)
            .unwrap()
            .set_node(Some(&map.node()))
            .unwrap();
        let number = map.mapped_variable_value(2, 0.).unwrap();
        assert_eq!(number.number(), 37.5);
        assert_eq!(number.color(), [0.375; 3]);
        let selection = map.mapped_variable_value(5, 0.).unwrap();
        assert_eq!(selection.number(), 2.);
        assert_eq!(selection.color(), [0.02; 3]);
        assert_eq!(
            selection.selection().unwrap(),
            "Triangle".encode_utf16().collect::<Vec<_>>()
        );
        let color = map.mapped_variable_value(7, 0.).unwrap();
        assert_eq!(color.number(), 75.);
        assert_eq!(color.color(), [0.25, 0.75, 0.5]);
        assert_eq!(map.mapped_variable_value(1, 0.).unwrap().number(), 0.);
        assert!(!map.mapped_variable_is_animated(2).unwrap());
        controls[0].set_axis_animated(0, true).unwrap();
        assert!(map.mapped_variable_is_animated(2).unwrap());
        assert!(matches!(
            wizard.wizard_variable_value(20, 0.),
            Err(PointwiseError::Index)
        ));
    }

    #[test]
    fn map_variable_retains_source_properties_and_maps_scalar_and_color() {
        let f = PointwiseFilter::<()>::new(PointwiseKind::MapVariable).unwrap();
        let names: Vec<_> = f
            .parameters()
            .iter()
            .map(|p| String::from_utf16_lossy(&p.name()))
            .collect();
        assert_eq!(
            names,
            [
                "Inputs",
                "Iterations",
                "Name",
                "Original",
                "Original Color",
                "Multiply 1",
                "Add 1",
                "Multiply 2",
                "Add 2",
                "Map to Variable",
                "Round"
            ]
        );
        assert_eq!(f.node().slots().len(), 20);
        assert_eq!(f.node().contents().len(), 1);
        f.parameter(3).unwrap().set_number(1.25).unwrap();
        f.parameter(4)
            .unwrap()
            .set_color(ContextColorValue::unlinked([1., 2., 3.]))
            .unwrap();
        for (index, value) in [(5, 2.), (6, 1.), (7, 3.), (8, 4.)] {
            f.parameter(index).unwrap().set_number(value).unwrap();
        }
        let mapped = f.mapped_variable_value(0, 0.).unwrap();
        assert_eq!(mapped.number(), 14.5);
        assert_eq!(mapped.color(), [13., 19., 25.]);
        f.parameter(10).unwrap().set_index(0).unwrap();
        assert_eq!(f.mapped_variable_value(0, 0.).unwrap().number(), 14.);
        assert_eq!(
            f.mapped_variable_value(0, 0.).unwrap().color(),
            [13., 19., 25.]
        );
        assert!(!f.mapped_variable_is_animated(0).unwrap());
        f.parameter(10).unwrap().set_axis_animated(0, true).unwrap();
        assert!(!f.mapped_variable_is_animated(0).unwrap());
        f.parameter(3).unwrap().set_axis_animated(0, true).unwrap();
        assert!(f.mapped_variable_is_animated(0).unwrap());
        assert_eq!(f.mapped_variable_value(4, 0.).unwrap().number(), 0.);
    }

    #[test]
    fn map_variable_follows_actual_parent_and_guards_unported_contexts() {
        let outer = PointwiseFilter::<()>::new(PointwiseKind::MapVariable).unwrap();
        outer.parameter(9).unwrap().set_number(3.).unwrap();
        outer.parameter(3).unwrap().set_number(7.).unwrap();
        let inner = PointwiseFilter::<()>::new(PointwiseKind::MapVariable).unwrap();
        outer
            .node()
            .content(0)
            .unwrap()
            .set_node(Some(&inner.node()))
            .unwrap();
        assert_eq!(inner.mapped_variable_value(3, 0.).unwrap().number(), 7.);
        assert_eq!(inner.mapped_variable_value(2, 0.).unwrap().number(), 0.);
        let wizard = PointwiseFilter::<()>::new(PointwiseKind::WizardGroup).unwrap();
        wizard
            .node()
            .content(0)
            .unwrap()
            .set_node(Some(&outer.node()))
            .unwrap();
        assert_eq!(inner.mapped_variable_value(2, 0.).unwrap().number(), 0.);
        assert!(outer.node().source_clone().is_ok());
        assert!(
            crate::RenderCaptureSession::new()
                .capture_node(&outer.node(), 0.)
                .is_ok()
        );
        let mut writer = BinaryWriter::default();
        outer.node().save_binary(&mut writer).unwrap();
        assert!(!writer.bytes().is_empty());
        outer
            .node()
            .set_cached_image(Some(SharedImageMap::new(ImageGeometry::new(2, 2))))
            .unwrap();
        assert!(outer.node().render_original(2, 2).is_ok());
        let factory = crate::create_node_by_identity::<()>(Some(
            &"Map Variable".encode_utf16().collect::<Vec<_>>(),
        ))
        .unwrap();
        assert_eq!(factory.kind(), Some(PointwiseKind::MapVariable));
    }
}
