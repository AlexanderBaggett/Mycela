//! Editing the retained Color and Gradient controls in a Lab section.
use super::*;
use crate::history::HistoryGraphEdit;
use crate::{GradientError, PointwiseParameterValue, SharedGradient};

/// The value edited by a Color or Gradient Lab control.
#[derive(Clone, Debug)]
pub enum LabColorValue {
    Color([f64; 3]),
    Gradient(SharedGradient),
}

#[derive(Clone)]
pub enum GradientAdjusterLink {
    Create(String),
    Existing(LabColorAdjuster),
    Unlink,
}

/// Create, select, or clear a shared Color Lab control for a property.
pub type ColorAdjusterLink = GradientAdjusterLink;

/// A Color or Gradient Lab control associated with an active catalog entry.
#[derive(Clone, Debug)]
pub struct LabColorAdjuster {
    name: String,
    presence: Arc<AtomicBool>,
    value: LabColorValue,
}

impl LabColorAdjuster {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn same_reference(&self, other: &Self) -> bool {
        self.matches(&other.presence)
    }

    pub(in crate::document_container) fn matches(&self, presence: &Arc<AtomicBool>) -> bool {
        Arc::ptr_eq(&self.presence, presence)
    }

    pub fn type_label(&self) -> &'static str {
        match self.value {
            LabColorValue::Color(_) => "Color",
            LabColorValue::Gradient(_) => "Gradient",
        }
    }

    /// Read an independent value for an editor to retain as its baseline.
    pub fn draft_value(&self) -> Result<LabColorValue, GradientError> {
        Ok(match &self.value {
            LabColorValue::Color(value) => LabColorValue::Color(*value),
            LabColorValue::Gradient(value) => {
                LabColorValue::Gradient(SharedGradient::from_wire_record(value.wire_record()?))
            }
        })
    }

    pub(in crate::document_container) fn read(
        entry: &GtxCatalogRecord,
        presence: &Arc<AtomicBool>,
    ) -> Option<Self> {
        let metadata = entry.metadata();
        let name = metadata.name.as_deref().filter(|name| !name.is_empty())?;
        let value = match (metadata.type_tag, entry) {
            (6, GtxCatalogRecord::Value(record)) => match record.payload {
                ValueAdjusterPayload::Color { value, .. } => LabColorValue::Color(value),
                _ => return None,
            },
            (7, GtxCatalogRecord::Reference(record)) => match &record.payload {
                ReferenceAdjusterPayload::Gradient {
                    current: Some(gradient),
                    ..
                } => LabColorValue::Gradient(gradient.live()),
                _ => return None,
            },
            _ => return None,
        };
        Some(Self {
            name: String::from_utf16_lossy(name),
            presence: presence.clone(),
            value,
        })
    }
}

impl LabColorValue {
    fn same_value(&self, other: &Self) -> Result<bool, GradientError> {
        match (self, other) {
            (Self::Color(a), Self::Color(b)) => Ok(a.map(f64::to_bits) == b.map(f64::to_bits)),
            (Self::Gradient(a), Self::Gradient(b)) => {
                let a = a.wire_record()?;
                let b = b.wire_record()?;
                Ok(a.width == b.width
                    && a.colors.len() == b.colors.len()
                    && a.colors.iter().zip(&b.colors).all(|(a, b)| {
                        a.coordinate == b.coordinate
                            && a.argb.map(f64::to_bits) == b.argb.map(f64::to_bits)
                    })
                    && a.transparency.len() == b.transparency.len()
                    && a.transparency.iter().zip(&b.transparency).all(|(a, b)| {
                        a.coordinate == b.coordinate && a.value.to_bits() == b.value.to_bits()
                    }))
            }
            _ => Ok(false),
        }
    }
}

impl<E: 'static> Gtx375GraphDocument<E> {
    /// Find the active Color Lab control linked to an ordinary Color property.
    pub fn color_adjuster(&self, property: &PointwiseParameter<E>) -> Option<LabColorAdjuster> {
        let tag = ValueAdjusterKind::Color.tag();
        let (scope, name) = reference_link_key(property, tag)?;
        let (entry, marker) = self.color_entry((scope, tag, &name))?;
        LabColorAdjuster::read(entry, marker)
    }

    /// List the Color Lab controls in the property's current Lab section.
    pub fn available_color_adjusters(
        &self,
        property: &PointwiseParameter<E>,
    ) -> Vec<LabColorAdjuster> {
        let owner = property.owner().node();
        if !property.is_adjustable()
            || !matches!(property.raw_value(), PointwiseParameterValue::Color(_))
            || !graph_nodes(&self.branches()).any(|node| node.same_reference(&owner))
        {
            return Vec::new();
        }
        self.color_lab_adjusters(Some(&owner))
            .into_iter()
            .filter(|control| matches!(control.value, LabColorValue::Color(_)))
            .collect()
    }

    /// Change the Color Lab link in one history entry and retain the owned RGB.
    pub fn set_color_adjuster(
        &mut self,
        property: &PointwiseParameter<E>,
        command: ColorAdjusterLink,
    ) -> Result<bool, GtxGraphError<E>> {
        let owner = property.owner().node();
        let branches = self.branches();
        if !property.is_adjustable()
            || !matches!(property.raw_value(), PointwiseParameterValue::Color(_))
            || !graph_nodes(&branches).any(|node| node.same_reference(&owner))
        {
            return Err(GtxGraphError::LabColorAdjuster(
                "This Color is no longer available.",
            ));
        }
        let group = Section::new(Some(&owner)).group_id;
        let tag = ValueAdjusterKind::Color.tag();
        let old = reference_link_key(property, tag);
        let (name, value, created, label) = match command {
            ColorAdjusterLink::Create(name) => {
                let name: Vec<_> = name.trim().encode_utf16().collect();
                if name.is_empty() || self.color_entry((group, tag, &name)).is_some() {
                    return Err(GtxGraphError::LabColorAdjuster(
                        "Enter an unused Color adjuster name in this Lab section.",
                    ));
                }
                let value = property.color().map_err(pointwise_node_error)?.components();
                let mut record =
                    ValueAdjusterRecord::new(ValueAdjusterKind::Color, Some(name.clone()), group);
                record.metadata.order = self.next_color_adjuster_order(group);
                record.payload = ValueAdjusterPayload::Color {
                    value,
                    previous: value,
                };
                (
                    Some(name),
                    Some(value),
                    Some(GtxCatalogRecord::Value(Box::new(record))),
                    "Create Color Adjuster",
                )
            }
            ColorAdjusterLink::Existing(selected) => {
                let (name, value) = self
                    .catalog
                    .entries()
                    .find_map(|(entry, marker)| {
                        let metadata = entry.metadata();
                        if metadata.group_id != group
                            || metadata.type_tag != tag
                            || !selected.matches(marker)
                        {
                            return None;
                        }
                        let LabColorValue::Color(value) =
                            LabColorAdjuster::read(entry, marker)?.value
                        else {
                            return None;
                        };
                        Some((metadata.name.clone()?, value))
                    })
                    .ok_or(GtxGraphError::LabColorAdjuster(
                        "This Color adjuster is no longer available in this Lab section.",
                    ))?;
                if old
                    .as_ref()
                    .is_some_and(|(scope, current)| *scope == group && *current == name)
                {
                    return Ok(false);
                }
                (Some(name), Some(value), None, "Link Color Adjuster")
            }
            ColorAdjusterLink::Unlink if old.is_some() => {
                (None, None, None, "Unlink Color Adjuster")
            }
            ColorAdjusterLink::Unlink => return Ok(false),
        };
        let next = name
            .as_deref()
            .map(|name| {
                property
                    .wire_metadata()
                    .new_reference_adjustment(tag, name, group)
            })
            .transpose()
            .map_err(|error| GtxGraphError::Node(NodeWireError::Property(error)))?;
        let retired = old.as_ref().and_then(|(scope, name)| {
            self.unused_color_adjuster(&branches, property, (*scope, tag, name.as_slice()))
        });
        self.apply_color_adjuster_link(
            LabReferenceEdit::color_link(property, value, next),
            created,
            retired,
            label,
        )
    }

    pub fn gradient_adjuster(&self, property: &PointwiseParameter<E>) -> Option<LabColorAdjuster> {
        let (scope, name) = gradient_link_key(property)?;
        let (entry, marker) =
            self.color_entry((scope, ReferenceAdjusterKind::Gradient.tag(), &name))?;
        LabColorAdjuster::read(entry, marker)
    }

    pub fn set_gradient_adjuster(
        &mut self,
        property: &PointwiseParameter<E>,
        command: GradientAdjusterLink,
    ) -> Result<bool, GtxGraphError<E>> {
        let owner = property.owner().node();
        let branches = self.branches();
        if !property.is_adjustable()
            || !matches!(property.raw_value(), PointwiseParameterValue::Gradient(_))
            || !graph_nodes(&branches).any(|node| node.same_reference(&owner))
        {
            return Err(GtxGraphError::LabColorAdjuster(
                "This Gradient is no longer available.",
            ));
        }
        let group = Section::new(Some(&owner)).group_id;
        let tag = ReferenceAdjusterKind::Gradient.tag();
        let old = gradient_link_key(property);
        let before = property.gradient().map_err(pointwise_node_error)?;
        let (name, after, created, label) = match command {
            GradientAdjusterLink::Create(name) => {
                let name: Vec<_> = name.trim().encode_utf16().collect();
                if name.is_empty() || self.color_entry((group, tag, &name)).is_some() {
                    return Err(GtxGraphError::LabColorAdjuster(
                        "Enter an unused Gradient adjuster name in this Lab section.",
                    ));
                }
                let mut record = ReferenceAdjusterRecord::new(
                    ReferenceAdjusterKind::Gradient,
                    Some(name.clone()),
                    group,
                );
                record.metadata.order = self.next_color_adjuster_order(group);
                let value = SharedGradientRecord::from_live(before.clone());
                record.payload = ReferenceAdjusterPayload::Gradient {
                    current: Some(value.clone()),
                    previous: Some(value),
                };
                (
                    Some(name),
                    before.clone(),
                    Some(GtxCatalogRecord::Reference(Box::new(record))),
                    "Create Gradient Adjuster",
                )
            }
            GradientAdjusterLink::Existing(selected) => {
                let (name, value) = self
                    .catalog
                    .entries()
                    .find_map(|(entry, marker)| {
                        let meta = entry.metadata();
                        if meta.group_id != group
                            || meta.type_tag != tag
                            || !selected.matches(marker)
                        {
                            return None;
                        }
                        let value = LabColorAdjuster::read(entry, marker)?.value;
                        let LabColorValue::Gradient(value) = value else {
                            return None;
                        };
                        Some((meta.name.clone()?, value))
                    })
                    .ok_or(GtxGraphError::LabColorAdjuster(
                        "This Gradient adjuster is no longer available in this Lab section.",
                    ))?;
                if old
                    .as_ref()
                    .is_some_and(|(scope, current)| *scope == group && *current == name)
                {
                    return Ok(false);
                }
                (Some(name), value, None, "Link Gradient Adjuster")
            }
            GradientAdjusterLink::Unlink if old.is_some() => (
                None,
                before
                    .source_clone()
                    .map_err(|error| pointwise_node_error(PointwiseError::Gradient(error)))?,
                None,
                "Unlink Gradient Adjuster",
            ),
            GradientAdjusterLink::Unlink => return Ok(false),
        };
        let next = name
            .as_deref()
            .map(|name| {
                property
                    .wire_metadata()
                    .new_reference_adjustment(tag, name, group)
            })
            .transpose()
            .map_err(|error| GtxGraphError::Node(NodeWireError::Property(error)))?;
        let retired = old.as_ref().and_then(|(scope, name)| {
            self.unused_color_adjuster(&branches, property, (*scope, tag, name.as_slice()))
        });
        self.apply_color_adjuster_link(
            LabReferenceEdit::gradient_link(property, before, after, next),
            created,
            retired,
            label,
        )
    }

    pub(in crate::document_container) fn next_color_adjuster_order(&self, group: [u8; 16]) -> i32 {
        self.catalog
            .entries()
            .filter(|(entry, _)| entry.metadata().group_id == group)
            .map(|(entry, _)| entry.metadata().order)
            .max()
            .map_or(0, |order| order.saturating_add(1))
    }

    fn unused_color_adjuster(
        &self,
        branches: &[Option<PointwiseNode<E>>],
        property: &PointwiseParameter<E>,
        key: ([u8; 16], i32, &[u16]),
    ) -> Option<Arc<AtomicBool>> {
        let (_, marker) = self.color_entry(key)?;
        let used_elsewhere = graph_nodes(branches)
            .any(|node| LabReferenceEdit::node_uses_control_except(&node, key, Some(property)))
            || self.count_environment_links(branches, |map| map.count_color_adjuster(key)) > 0
            || self.count_synthesis_color_links(branches, key) > 0;
        (!used_elsewhere).then(|| marker.clone())
    }

    fn apply_color_adjuster_link(
        &mut self,
        property: LabReferenceEdit<E>,
        created: Option<GtxCatalogRecord>,
        retired: Option<Arc<AtomicBool>>,
        label: &str,
    ) -> Result<bool, GtxGraphError<E>> {
        let edit = ColorLabLinkEdit {
            property,
            created: created.map(|entry| self.catalog.stage(entry)),
            retired,
        };
        let affected = edit
            .publish(HistoryDirection::Redo)
            .map_err(GtxGraphError::History)?;
        self.record_lab_edit(label, edit)
            .and(affected.notify())
            .map_err(GtxGraphError::History)?;
        Ok(true)
    }

    /// Edit a gradient through its shared Lab value when the property is linked.
    pub fn edit_gradient_property(
        &mut self,
        property: &PointwiseParameter<E>,
        before: &SharedGradient,
        after: SharedGradient,
    ) -> Result<(), GtxGraphError<E>> {
        let Some(link) = property
            .retained_adjustment()
            .filter(|link| link.is_active().unwrap_or(true))
        else {
            return property
                .edit_gradient(before, after)
                .map_err(|error| GtxGraphError::Node(error.into()));
        };
        let control = link
            .names()
            .entries()
            .into_iter()
            .next()
            .flatten()
            .and_then(|name| {
                self.color_entry((link.guid(), ReferenceAdjusterKind::Gradient.tag(), &name))
            })
            .and_then(|(entry, presence)| LabColorAdjuster::read(entry, presence))
            .ok_or(GtxGraphError::LabColorAdjuster(
                "The linked Gradient adjuster is unavailable.",
            ))?;
        self.edit_color_lab_adjuster(
            Some(&property.owner().node()),
            &control,
            &LabColorValue::Gradient(before.clone()),
            LabColorValue::Gradient(after),
        )?;
        Ok(())
    }

    pub fn color_lab_adjusters(
        &self,
        selected: Option<&PointwiseNode<E>>,
    ) -> Vec<LabColorAdjuster> {
        let group = Section::new(selected).group_id;
        let parked = self.parked_only_lab_names(&self.branches(), Some(group));
        let mut controls: Vec<_> = self
            .catalog
            .entries()
            .filter(|(entry, _)| {
                entry.metadata().group_id == group && !parked.contains(entry.metadata())
            })
            .filter_map(|(entry, presence)| {
                Some((entry.metadata(), LabColorAdjuster::read(entry, presence)?))
            })
            .collect();
        controls.sort_by_cached_key(|(metadata, control)| {
            super::super::lab_adjuster_metadata::display_order(
                metadata.order,
                control.name(),
                metadata.type_tag,
            )
        });
        controls.into_iter().map(|(_, control)| control).collect()
    }

    pub fn edit_color_lab_adjuster(
        &mut self,
        selected: Option<&PointwiseNode<E>>,
        control: &LabColorAdjuster,
        before: &LabColorValue,
        value: LabColorValue,
    ) -> Result<bool, GtxGraphError<E>> {
        let Some(update) = self.prepare_color_lab_edit(selected, control, before, value)? else {
            return Ok(false);
        };
        self.commit_lab_values_with_references(
            &format!("Modify {} Adjuster", control.name()),
            LabValues {
                references: vec![update],
                ..LabValues::default()
            },
        )?;
        Ok(true)
    }

    /// Prepare a render session that sees the working Color without changing
    /// the document or its history. Use the same receiver plan as Apply.
    pub fn preview_color_lab_adjuster(
        &self,
        selected: Option<&PointwiseNode<E>>,
        control: &LabColorAdjuster,
        before: &LabColorValue,
        rgb: [f64; 3],
    ) -> Result<Option<crate::RenderFreezeSession<E>>, GtxGraphError<E>> {
        let Some(update) =
            self.prepare_color_lab_edit(selected, control, before, LabColorValue::Color(rgb))?
        else {
            return Ok(None);
        };
        let batch = self.prepare_lab_batch(LabValues::default(), &[update])?;
        let mut preview = crate::pointwise::property::capture::DraftRenderOverrides::default();
        batch.environment.collect_draft_preview(&mut preview);
        batch.synthesis.collect_draft_preview(&mut preview);
        for reference in &batch.references {
            reference.collect_draft_preview(&mut preview);
        }
        Ok(Some(crate::RenderFreezeSession::with_draft_override(
            preview,
        )))
    }

    fn prepare_color_lab_edit(
        &self,
        selected: Option<&PointwiseNode<E>>,
        control: &LabColorAdjuster,
        before: &LabColorValue,
        value: LabColorValue,
    ) -> Result<Option<ReferenceCatalogUpdate>, GtxGraphError<E>> {
        if selected.is_some_and(|selected| {
            !graph_nodes(&self.branches()).any(|node| node.same_reference(selected))
        }) {
            return Err(GtxGraphError::LabColorAdjuster(
                "The selected Lab section changed.",
            ));
        }
        let group = Section::new(selected).group_id;
        let (entry, presence) = self
            .catalog
            .entries()
            .find(|(entry, presence)| {
                entry.metadata().group_id == group && Arc::ptr_eq(&control.presence, presence)
            })
            .ok_or(GtxGraphError::LabColorAdjuster(
                "The Lab adjuster changed. Reopen it to edit its current value.",
            ))?;
        let current = LabColorAdjuster::read(entry, presence).ok_or(
            GtxGraphError::LabColorAdjuster("This Lab adjuster cannot be edited."),
        )?;
        let gradient_error = |error| GtxGraphError::Node(PointwiseError::Gradient(error).into());
        if !current.value.same_value(before).map_err(gradient_error)? {
            return Err(GtxGraphError::LabColorAdjuster(
                "The Lab adjuster changed. Reopen it to edit its current value.",
            ));
        }
        if current.value.same_value(&value).map_err(gradient_error)? {
            return Ok(None);
        }
        let value = match (&current.value, value) {
            (LabColorValue::Color(_), LabColorValue::Color(value))
                if value.iter().all(|n| n.is_finite()) =>
            {
                GtxPresetValue::Color(value)
            }
            (LabColorValue::Gradient(_), LabColorValue::Gradient(value)) => {
                GtxPresetValue::Reference(ReferenceValueHandle::Gradient(
                    SharedGradientRecord::from_live(value),
                ))
            }
            _ => {
                return Err(GtxGraphError::LabColorAdjuster(
                    "The value is invalid for this Lab adjuster.",
                ));
            }
        };
        ReferenceCatalogUpdate::from_value(entry, presence, &value).map(Some)
    }
}

fn pointwise_node_error<E>(error: PointwiseError<E>) -> GtxGraphError<E> {
    GtxGraphError::Node(error.into())
}

fn gradient_link_key<E: 'static>(property: &PointwiseParameter<E>) -> Option<([u8; 16], Vec<u16>)> {
    reference_link_key(property, ReferenceAdjusterKind::Gradient.tag())
}

fn reference_link_key<E: 'static>(
    property: &PointwiseParameter<E>,
    tag: i32,
) -> Option<([u8; 16], Vec<u16>)> {
    let link = property.retained_adjustment()?;
    if link.property_type() != tag || !link.is_active().ok()? {
        return None;
    }
    let name = link.names().entries().into_iter().next().flatten()?;
    Some((link.guid(), name))
}

struct ColorLabLinkEdit<E> {
    property: LabReferenceEdit<E>,
    created: Option<Arc<AtomicBool>>,
    retired: Option<Arc<AtomicBool>>,
}

impl<E: 'static> ColorLabLinkEdit<E> {
    fn publish(
        &self,
        direction: HistoryDirection,
    ) -> Result<ReferenceReceiver<E>, HistoryError<E>> {
        let undo = direction == HistoryDirection::Undo;
        let (_, mut changed) =
            LabReferenceEdit::publish_with(std::slice::from_ref(&self.property), undo, || {
                if let Some(marker) = &self.created {
                    marker.store(!undo, Ordering::Release);
                }
                if let Some(marker) = &self.retired {
                    marker.store(undo, Ordering::Release);
                }
                Ok(())
            })?;
        Ok(changed.remove(0))
    }
}

impl<E: 'static> HistoryGraphEdit<E> for ColorLabLinkEdit<E> {
    fn apply(&self, direction: HistoryDirection) -> Result<(), HistoryError<E>> {
        self.publish(direction)?.notify()
    }
}
