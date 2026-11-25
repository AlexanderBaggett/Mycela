use super::environment_adjusters::{environment_adjuster_scope, graph_nodes};
use super::lab_adjuster_metadata::display_order;
use super::lab_preset_scope::{LabProjection, PresetRefresh};
use super::numeric_adjusters::{
    NumericCatalogUpdate, environment_mapping_curve, limits, map_retained_number,
};
use super::numeric_links::initial_value;
use super::synthesis_adjusters::{
    SynthesisAdjuster, SynthesisAdjusterBatch, SynthesisReceiverEdit,
};
use super::*;
use crate::pointwise::property::SynthesisLinkState;
use crate::pointwise::reference_adjustment::LabReferenceEdit;
use crate::pointwise::{EnvironmentPaintEdit, TextureEdit};
use crate::{
    AdjustmentRecord, CanvasLayerEditor, HistoryDirection, HistoryError, PointwiseFilter,
    StyleEdit, SynthesisColorTarget, SynthesisEdit, SynthesisLayerKind, SynthesisNumberField,
    SynthesisPairField, SynthesisRecord, ValueAdjusterPayload,
};
use std::sync::atomic::{AtomicBool, Ordering};

#[path = "style_synthesis_number_mapping.rs"]
mod mapping;
pub use mapping::StyleSynthesisNumberMapping;
#[path = "style_synthesis_pairs.rs"]
mod pairs;
pub use pairs::SynthesisPairTarget;
#[cfg(test)]
#[path = "standalone_synthesis_number_tests.rs"]
mod standalone_tests;
#[path = "synthesis_structure.rs"]
mod structure;

pub(super) type LinkKey<'a> = ([u8; 16], i32, &'a [u16]);

/// A Number receiver in one Texture Synthesis paint of a Style or Canvas.
#[derive(Clone, Copy)]
pub struct StyleSynthesisNumberTarget {
    pass: usize,
    layer: Option<usize>,
    field: SynthesisNumberField,
}

impl StyleSynthesisNumberTarget {
    pub fn new(pass: usize, layer: Option<usize>, field: SynthesisNumberField) -> Self {
        Self { pass, layer, field }
    }
}

impl<E: 'static> Gtx375GraphDocument<E> {
    /// Reads the adjuster of a field in a standalone, locally edited Texture.
    pub fn synthesis_number_adjuster(
        &self,
        owner: &PointwiseNode<E>,
        expected: &SynthesisEdit,
        layer: Option<usize>,
        field: SynthesisNumberField,
    ) -> Option<LabNumericAdjuster> {
        let filter = owner.filter()?;
        standalone_synthesis_state(&filter).ok()?;
        if !filter.synthesis_edit()?.same_revision(expected) {
            return None;
        }
        self.style_synthesis_number_adjuster(expected, layer, field)
    }

    pub fn available_synthesis_number_adjusters(
        &self,
        owner: &PointwiseNode<E>,
        expected: &SynthesisEdit,
        layer: Option<usize>,
        field: SynthesisNumberField,
    ) -> Vec<LabNumericAdjuster> {
        if !owner.filter().is_some_and(|filter| {
            standalone_synthesis_state(&filter).is_ok()
                && filter
                    .synthesis_edit()
                    .is_some_and(|current| current.same_revision(expected))
        }) {
            return Vec::new();
        }
        self.available_style_synthesis_number_adjusters(owner, expected, layer, field)
    }

    pub fn style_synthesis_number_adjuster(
        &self,
        edit: &SynthesisEdit,
        layer: Option<usize>,
        field: SynthesisNumberField,
    ) -> Option<LabNumericAdjuster> {
        let adjustment = edit.number_adjustment(layer, field)?;
        let (metadata, value) = self.synthesis_mapping_control(adjustment, 0)?;
        LabNumericAdjuster::read(metadata, value)
    }

    pub fn available_style_synthesis_number_adjusters(
        &self,
        owner: &PointwiseNode<E>,
        edit: &SynthesisEdit,
        layer: Option<usize>,
        field: SynthesisNumberField,
    ) -> Vec<LabNumericAdjuster> {
        if edit.number_value(layer, field).is_none() {
            return Vec::new();
        }
        self.available_texture_numeric_adjusters(owner, false)
    }

    fn available_texture_numeric_adjusters(
        &self,
        owner: &PointwiseNode<E>,
        paired: bool,
    ) -> Vec<LabNumericAdjuster> {
        let group = environment_adjuster_scope(owner);
        let parked = self.parked_only_lab_names(&self.branches(), Some(group));
        let mut choices: Vec<_> = self
            .catalog
            .entries()
            .filter_map(|(entry, _)| {
                let GtxCatalogRecord::Numeric { metadata, value } = entry else {
                    return None;
                };
                let compatible = if paired {
                    metadata.type_tag == 1
                } else {
                    matches!(metadata.type_tag, 0 | 4)
                };
                if metadata.group_id != group || !compatible || parked.contains(metadata) {
                    return None;
                }
                let choice = LabNumericAdjuster::read(metadata, value)?;
                choice
                    .range()
                    .is_none_or(|[min, max]| min < max)
                    .then_some((metadata, choice))
            })
            .collect();
        choices.sort_by_cached_key(|(metadata, control)| {
            display_order(metadata.order, control.name(), metadata.type_tag)
        });
        choices.into_iter().map(|(_, control)| control).collect()
    }

    /// Connects one field to a Number adjuster in the paint owner's Lab section.
    /// Changing the link and the Lab catalog is one undoable edit.
    pub fn set_style_synthesis_number_adjuster(
        &mut self,
        owner: &PointwiseNode<E>,
        canvas: Option<&CanvasLayerEditor<E>>,
        expected: &SynthesisEdit,
        target: StyleSynthesisNumberTarget,
        link: LabNumericLink,
    ) -> Result<bool, GtxGraphError<E>> {
        let StyleSynthesisNumberTarget { pass, layer, field } = target;
        self.set_texture_number_adjuster(
            owner,
            SynthesisNumberOwner::paint(canvas, pass),
            expected,
            (layer, field),
            link,
        )
    }

    /// Connects a field of an unlinked standalone Texture to its owning Lab section.
    pub fn set_synthesis_number_adjuster(
        &mut self,
        owner: &PointwiseNode<E>,
        expected: &SynthesisEdit,
        layer: Option<usize>,
        field: SynthesisNumberField,
        link: LabNumericLink,
    ) -> Result<bool, GtxGraphError<E>> {
        self.set_texture_number_adjuster(
            owner,
            SynthesisNumberOwner::Texture,
            expected,
            (layer, field),
            link,
        )
    }

    fn set_texture_number_adjuster(
        &mut self,
        owner: &PointwiseNode<E>,
        receiver: SynthesisNumberOwner<E>,
        expected: &SynthesisEdit,
        (layer, field): (Option<usize>, SynthesisNumberField),
        link: LabNumericLink,
    ) -> Result<bool, GtxGraphError<E>> {
        let branches = self.branches();
        let mut draft = SynthesisNumberDraft::capture(&branches, owner, &receiver, expected)?;
        let synthesis = &mut draft.synthesis;
        if !matches!(link, LabNumericLink::Unlink) && !field_is_active(synthesis, layer, field) {
            return Err(GtxGraphError::NumericAdjuster(
                "Enable the field before linking an adjuster.",
            ));
        }
        let current = synthesis
            .number_value(layer, field)
            .ok_or_else(stale_paint)?;
        let previous = synthesis
            .number_adjustment(layer, field)
            .and_then(|adjustment| link_key(adjustment, 0));
        let group = environment_adjuster_scope(owner);
        let link = self.resolve_parked_numeric_link(link, group, 0);
        let mut created = None;
        let destination = match link {
            LabNumericLink::Create(name) => {
                let name: Vec<_> = name.trim().encode_utf16().collect();
                if name.is_empty() {
                    return Err(GtxGraphError::NumericAdjuster("Enter an adjuster name."));
                }
                if self.catalog.entries().any(|(entry, _)| {
                    let info = entry.metadata();
                    info.group_id == group
                        && matches!(info.type_tag, 0 | 4)
                        && info.name.as_deref() == Some(&name)
                }) {
                    return Err(GtxGraphError::NumericAdjuster(
                        "A numeric adjuster already uses that name in this Lab section.",
                    ));
                }
                let mut entry =
                    ValueAdjusterRecord::new(ValueAdjusterKind::Number, Some(name), group);
                entry.metadata.order = self
                    .catalog
                    .entries()
                    .filter_map(|(entry, _)| {
                        let info = entry.metadata();
                        (info.group_id == group).then_some(info.order)
                    })
                    .max()
                    .map_or(0, |order| order.saturating_add(1));
                let ValueAdjusterPayload::Number(number) = &mut entry.payload else {
                    unreachable!("new Number adjuster");
                };
                let adjustment = connection_record(field, &entry.metadata)?;
                let [min, max] = field.mapping_range();
                number.value = if field.range() == field.mapping_range()
                    && !matches!(
                        field,
                        SynthesisNumberField::MasterColor(_)
                            | SynthesisNumberField::LayerColor(_)
                            | SynthesisNumberField::Scale
                            | SynthesisNumberField::EqualizeStrength
                            | SynthesisNumberField::RepeatFrequency
                            | SynthesisNumberField::RepeatFeather
                            | SynthesisNumberField::BlendOpacity
                            | SynthesisNumberField::BlendErode
                            | SynthesisNumberField::BlendSharpness
                    )
                    && !SynthesisNumberField::PHOTO_BLEND_FIELDS.contains(&field)
                {
                    (current - min) / (max - min) * 100.0
                } else {
                    let record = adjustment.components()[0]
                        .curve
                        .as_ref()
                        .ok_or_else(unavailable_control)?;
                    let curve = environment_mapping_curve(record)
                        .map_err(|error| GtxGraphError::Node(error.into()))?;
                    let range = limits(&number.numeric).ok_or_else(unavailable_control)?;
                    let places = number
                        .numeric
                        .limits
                        .as_ref()
                        .ok_or_else(unavailable_control)?
                        .decimal_places;
                    initial_value(&curve, current, field.choice_count(), range, places)
                        .map_err(|error| GtxGraphError::Node(error.into()))?
                };
                number.previous = number.value;
                let value = NumericCatalogValue::new(entry.payload);
                let mapped = sample_connection(&adjustment, 0, &entry.metadata, &value)?;
                created = Some(GtxCatalogRecord::Numeric {
                    metadata: entry.metadata,
                    value,
                });
                Some((adjustment, mapped))
            }
            LabNumericLink::Existing(control) => {
                let (metadata, value) = self
                    .catalog
                    .entries()
                    .find_map(|(entry, _)| match entry {
                        GtxCatalogRecord::Numeric { metadata, value }
                            if metadata.group_id == group
                                && matches!(metadata.type_tag, 0 | 4)
                                && control.matches(value) =>
                        {
                            Some((metadata, value))
                        }
                        _ => None,
                    })
                    .ok_or_else(unavailable_control)?;
                if previous.is_some_and(|key| matches_key(metadata, key)) {
                    return Ok(false);
                }
                let adjustment = connection_record(field, metadata)?;
                let mapped = sample_connection(&adjustment, 0, metadata, value)?;
                Some((adjustment, mapped))
            }
            LabNumericLink::Unlink => None,
        };
        let label = if created.is_some() {
            "Create Synthesis Field Adjuster"
        } else if destination.is_some() {
            "Link Synthesis Field Adjuster"
        } else {
            "Unlink Synthesis Field Adjuster"
        };
        let retired = self.retired_synthesis_number_links(&branches, previous.as_slice());
        if !synthesis
            .set_number_adjustment(layer, field, destination)
            .ok_or_else(stale_paint)?
        {
            return Ok(false);
        }
        self.commit_synthesis_number_change(draft.into_edit()?, created, retired, label)
    }

    pub fn set_style_synthesis_automatic_placement(
        &mut self,
        owner: &PointwiseNode<E>,
        canvas: Option<&CanvasLayerEditor<E>>,
        expected: &SynthesisEdit,
        pass: usize,
        layer: usize,
        enabled: bool,
    ) -> Result<bool, GtxGraphError<E>> {
        self.set_texture_automatic_placement(
            owner,
            SynthesisNumberOwner::paint(canvas, pass),
            expected,
            layer,
            enabled,
        )
    }

    /// Updates standalone automatic placement and detaches Auto receivers on disable.
    pub fn set_synthesis_automatic_placement(
        &mut self,
        owner: &PointwiseNode<E>,
        expected: &SynthesisEdit,
        layer: usize,
        enabled: bool,
    ) -> Result<bool, GtxGraphError<E>> {
        self.set_texture_automatic_placement(
            owner,
            SynthesisNumberOwner::Texture,
            expected,
            layer,
            enabled,
        )
    }

    /// Changes Auto Placement while explicitly editing a shared Material.
    pub fn set_material_synthesis_automatic_placement(
        &mut self,
        owner: &PointwiseNode<E>,
        expected: &SynthesisEdit,
        layer: usize,
        enabled: bool,
        material: &SynthesisAdjuster,
    ) -> Result<bool, GtxGraphError<E>> {
        let branches = self.branches();
        if !graph_nodes(&branches).any(|node| node.same_reference(owner)) {
            return Err(stale_paint());
        }
        let filter = owner.filter().ok_or_else(stale_paint)?;
        let value = self
            .synthesis_adjuster(&filter)
            .filter(|active| active.same_reference(material))
            .map(|active| active.value)
            .ok_or_else(stale_paint)?;
        let mut synthesis = filter
            .synthesis_edit()
            .filter(|current| current.same_revision(expected))
            .ok_or_else(stale_paint)?;
        if !change_automatic_placement(&mut synthesis, layer, enabled)? {
            return Ok(false);
        }
        let edit = filter
            .prepare_synthesis_edit(synthesis)
            .map_err(|error| GtxGraphError::Node(error.into()))?
            .ok_or_else(stale_paint)?;
        let [before, after] = edit.records();
        let removed = SynthesisRecord::removed_lab_links_between([before.clone()], [after.clone()]);
        let keys: Vec<_> = removed
            .iter()
            .map(|(group, tag, name)| (*group, *tag, name.as_slice()))
            .collect();
        let retired = self.retired_synthesis_number_links(&branches, &keys);
        let material = SynthesisAdjusterBatch::replace_material(&branches, value, before, after);
        self.commit_synthesis_catalog_change(
            SynthesisLabTarget::Material(material),
            None,
            retired,
            "Set Material Automatic Placement",
        )
    }

    fn set_texture_automatic_placement(
        &mut self,
        owner: &PointwiseNode<E>,
        receiver: SynthesisNumberOwner<E>,
        expected: &SynthesisEdit,
        layer: usize,
        enabled: bool,
    ) -> Result<bool, GtxGraphError<E>> {
        let branches = self.branches();
        let mut draft = SynthesisNumberDraft::capture(&branches, owner, &receiver, expected)?;
        let synthesis = &mut draft.synthesis;
        if synthesis.automatic_placement(layer) == Some(enabled) {
            return Ok(false);
        }
        let mut retired = Vec::new();
        if !enabled {
            let previous: Vec<_> = SynthesisNumberField::AUTOMATIC_FIELDS
                .into_iter()
                .filter_map(|field| synthesis.number_adjustment(Some(layer), field))
                .chain(
                    SynthesisPairField::AUTOMATIC
                        .into_iter()
                        .filter_map(|field| synthesis.pair_adjustment(layer, field)),
                )
                .flat_map(link_keys)
                .collect();
            retired = self.retired_synthesis_number_links(&branches, &previous);
        }
        change_automatic_placement(synthesis, layer, enabled)?;
        self.commit_synthesis_number_change(
            draft.into_edit()?,
            None,
            retired,
            "Set Automatic Placement",
        )
    }

    fn commit_synthesis_number_change(
        &mut self,
        target: SynthesisLabTarget<E>,
        created: Option<GtxCatalogRecord>,
        retired: Vec<Arc<AtomicBool>>,
        label: &str,
    ) -> Result<bool, GtxGraphError<E>> {
        let created = created
            .into_iter()
            .map(|entry| self.catalog.stage(entry))
            .collect();
        self.commit_synthesis_lab_edit(target, created, retired, label)
    }

    pub(super) fn commit_synthesis_catalog_change(
        &mut self,
        target: SynthesisLabTarget<E>,
        created: Option<GtxCatalogRecord>,
        mut retired: Vec<Arc<AtomicBool>>,
        label: &str,
    ) -> Result<bool, GtxGraphError<E>> {
        let projection =
            LabProjection::read(&self.branches(), None, None, crate::PointwiseSlot::node);
        let projected: Vec<_> = self
            .catalog
            .entries()
            .filter_map(|(entry, marker)| {
                (!retired.iter().any(|old| Arc::ptr_eq(old, marker))).then_some(entry)
            })
            .chain(created.as_ref())
            .collect();
        let refresh = PresetRefresh::stage_projected(&mut self.presets, &projection, &projected);
        retired.extend(refresh.retired);
        let mut added = refresh.added;
        added.extend(created.map(|entry| self.catalog.stage(entry)));
        self.commit_synthesis_lab_edit(target, added, retired, label)
    }

    fn commit_synthesis_lab_edit(
        &self,
        target: SynthesisLabTarget<E>,
        created: Vec<Arc<AtomicBool>>,
        retired: Vec<Arc<AtomicBool>>,
        label: &str,
    ) -> Result<bool, GtxGraphError<E>> {
        let edit = Arc::new(SynthesisLabEdit {
            target,
            created,
            retired,
        });
        let owners = edit
            .publish(HistoryDirection::Redo)
            .map_err(GtxGraphError::History)?;
        let history = self.property_history();
        let before = history.as_ref().map(crate::History::edit_sequence);
        if let Err(error) = self.record_lab_edit(label, edit.clone()) {
            if before == history.as_ref().map(crate::History::edit_sequence) {
                edit.publish(HistoryDirection::Undo)
                    .map_err(GtxGraphError::History)?;
            }
            return Err(GtxGraphError::History(error));
        }
        edit.target
            .notify(&owners)
            .map_err(GtxGraphError::History)?;
        Ok(true)
    }

    /// Current Lab Color for a retained Texture Synthesis property.
    pub fn synthesis_color_adjuster(
        &self,
        edit: &SynthesisEdit,
        target: SynthesisColorTarget,
    ) -> Option<LabColorAdjuster> {
        let (entry, marker) = self.color_entry(edit.color_link_key(target)?)?;
        LabColorAdjuster::read(entry, marker)
    }

    /// Color controls in the receiver's current Lab section.
    pub fn available_synthesis_color_adjusters(
        &self,
        owner: &PointwiseNode<E>,
    ) -> Vec<LabColorAdjuster> {
        self.color_lab_adjusters(Some(owner))
            .into_iter()
            .filter(|adjuster| adjuster.type_label() == "Color")
            .collect()
    }

    /// Link or unlink a Color on a local standalone, Style, or Canvas texture.
    pub fn set_synthesis_color_adjuster(
        &mut self,
        owner: &PointwiseNode<E>,
        canvas: Option<&CanvasLayerEditor<E>>,
        pass: Option<usize>,
        expected: &SynthesisEdit,
        target: SynthesisColorTarget,
        link: ColorAdjusterLink,
    ) -> Result<bool, GtxGraphError<E>> {
        let branches = self.branches();
        let receiver = match pass {
            Some(pass) => SynthesisNumberOwner::paint(canvas, pass),
            None if canvas.is_none() => SynthesisNumberOwner::Texture,
            None => return Err(stale_paint()),
        };
        let mut draft = SynthesisNumberDraft::capture(&branches, owner, &receiver, expected)?;
        if let SynthesisColorTarget::Layer(layer) = target
            && draft
                .synthesis
                .layer(layer)
                .is_some_and(|(_, kind)| kind == SynthesisLayerKind::Solid)
        {
            return Err(GtxGraphError::LabColorAdjuster(
                "Select the Solid Color swatch.",
            ));
        }
        let previous = draft.synthesis.color_link_key(target);
        let group = environment_adjuster_scope(owner);
        let tag = ValueAdjusterKind::Color.tag();
        let error =
            || GtxGraphError::LabColorAdjuster("This Color adjuster is no longer available.");
        let (destination, created, label) = match link {
            ColorAdjusterLink::Create(name) => {
                let name: Vec<_> = name.trim().encode_utf16().collect();
                if name.is_empty() || self.color_entry((group, tag, &name)).is_some() {
                    return Err(GtxGraphError::LabColorAdjuster(
                        "Enter an unused Color adjuster name in this Lab section.",
                    ));
                }
                let rgb = draft.synthesis.color_value(target).ok_or_else(error)?;
                let mut entry =
                    ValueAdjusterRecord::new(ValueAdjusterKind::Color, Some(name.clone()), group);
                entry.metadata.order = self.next_color_adjuster_order(group);
                entry.payload = ValueAdjusterPayload::Color {
                    value: rgb,
                    previous: rgb,
                };
                (
                    Some((name, rgb)),
                    Some(GtxCatalogRecord::Value(Box::new(entry))),
                    "Create Color Adjuster",
                )
            }
            ColorAdjusterLink::Existing(control) => {
                let destination = self
                    .catalog
                    .entries()
                    .find_map(|(entry, marker)| {
                        let metadata = entry.metadata();
                        if metadata.group_id != group
                            || metadata.type_tag != tag
                            || !control.matches(marker)
                        {
                            return None;
                        }
                        let GtxCatalogRecord::Value(entry) = entry else {
                            return None;
                        };
                        let ValueAdjusterPayload::Color { value, .. } = entry.payload else {
                            return None;
                        };
                        Some((metadata.name.clone()?, value))
                    })
                    .ok_or_else(error)?;
                (Some(destination), None, "Link Color Adjuster")
            }
            ColorAdjusterLink::Unlink => {
                if previous.is_none() {
                    return Ok(false);
                }
                (None, None, "Unlink Color Adjuster")
            }
        };
        let retired = self.retired_synthesis_color_links(&branches, previous.as_slice());
        let connection = destination.map(|(name, rgb)| {
            let mut record = AdjustmentRecord::new(&[[0., 1.]]);
            record.guid = group;
            record.property_type = tag;
            let component = &mut record.components_mut()[0];
            component.active = true;
            component.name = name;
            component.curve_mut();
            (record, rgb)
        });
        if !draft
            .synthesis
            .set_color_adjustment(target, connection)
            .ok_or_else(error)?
        {
            return Ok(false);
        }
        self.commit_synthesis_number_change(draft.into_edit()?, created, retired, label)
    }

    pub(super) fn count_synthesis_color_links(
        &self,
        branches: &[Option<PointwiseNode<E>>],
        key: LinkKey<'_>,
    ) -> usize {
        if key.1 != 6 {
            return 0;
        }
        let mut count = 0;
        let mut scan = |record: &SynthesisRecord| {
            record.visit_color_adjustments(|linked| count += usize::from(linked == key));
        };
        for (entry, _) in self.catalog.entries() {
            if let GtxCatalogRecord::Synthesis { value, .. } = entry {
                scan(&value.record());
            }
        }
        for filter in graph_nodes(branches).filter_map(|node| node.filter()) {
            if let Some(state) = filter
                .synthesis_link_state()
                .filter(|state| state.catalog.is_none())
            {
                scan(&state.local);
            }
            filter.visit_retained_synthesis_paints(&mut scan);
        }
        count
    }

    pub(super) fn retired_synthesis_color_links(
        &self,
        branches: &[Option<PointwiseNode<E>>],
        removed: &[LinkKey<'_>],
    ) -> Vec<Arc<AtomicBool>> {
        if !removed.iter().any(|key| key.1 == 6) {
            return Vec::new();
        }
        self.catalog
            .entries()
            .filter_map(|(entry, active)| {
                let metadata = entry.metadata();
                let key = (
                    metadata.group_id,
                    metadata.type_tag,
                    metadata.name.as_deref()?,
                );
                let count = removed.iter().filter(|removed| **removed == key).count();
                (count > 0
                    && key.1 == 6
                    && self.count_synthesis_color_links(branches, key) == count
                    && !graph_nodes(branches)
                        .any(|node| LabReferenceEdit::node_uses_control_except(&node, key, None))
                    && self.count_environment_links(branches, |map| map.count_color_adjuster(key))
                        == 0)
                    .then(|| active.clone())
            })
            .collect()
    }

    pub(super) fn retired_synthesis_number_links(
        &self,
        branches: &[Option<PointwiseNode<E>>],
        removed: &[LinkKey<'_>],
    ) -> Vec<Arc<AtomicBool>> {
        if removed.is_empty() {
            return Vec::new();
        }
        self.catalog
            .entries()
            .filter_map(|(entry, active)| {
                let GtxCatalogRecord::Numeric { metadata, .. } = entry else {
                    return None;
                };
                let drops = removed
                    .iter()
                    .filter(|key| matches_key(metadata, **key))
                    .count();
                let name = metadata.name.as_deref()?;
                (drops > 0
                    && !Self::graph_uses_numeric_adjuster(branches, metadata)
                    && (metadata.type_tag == 1
                        || self.count_environment_links(branches, |map| {
                            map.count_numeric_adjuster(metadata.group_id, name)
                        }) == 0)
                    && self.count_synthesis_number_links(branches, metadata) == drops)
                    .then(|| active.clone())
            })
            .collect()
    }

    pub(super) fn count_synthesis_number_links(
        &self,
        branches: &[Option<PointwiseNode<E>>],
        metadata: &AdjusterMetadataRecord,
    ) -> usize {
        let mut count = 0;
        let mut scan = |record: &crate::SynthesisRecord| {
            record.visit_numeric_adjustments(|adjustment| {
                count += link_keys(adjustment)
                    .filter(|key| matches_key(metadata, *key))
                    .count();
            });
        };
        for (entry, _) in self.catalog.entries() {
            if let GtxCatalogRecord::Synthesis { value, .. } = entry {
                scan(&value.record());
            }
        }
        for filter in graph_nodes(branches).filter_map(|node| node.filter()) {
            if let Some(state) = filter
                .synthesis_link_state()
                .filter(|state| state.catalog.is_none())
            {
                scan(&state.local);
            }
            filter.visit_retained_synthesis_paints(&mut scan);
        }
        count
    }
}

fn change_automatic_placement<E>(
    synthesis: &mut SynthesisEdit,
    layer: usize,
    enabled: bool,
) -> Result<bool, GtxGraphError<E>> {
    if synthesis.automatic_placement(layer) == Some(enabled) {
        return Ok(false);
    }
    if !synthesis.set_automatic_placement(layer, enabled) {
        return Err(stale_paint());
    }
    Ok(true)
}

fn field_is_active(
    synthesis: &SynthesisEdit,
    layer: Option<usize>,
    field: SynthesisNumberField,
) -> bool {
    if SynthesisNumberField::AUTOMATIC_FIELDS.contains(&field) {
        layer.and_then(|layer| synthesis.automatic_placement(layer)) == Some(true)
    } else {
        match field {
            SynthesisNumberField::EqualizeStrength => layer.is_some_and(|index| {
                synthesis.layer_has_source(index) && synthesis.equalizes_source(index) == Some(true)
            }),
            SynthesisNumberField::PhotoStarFeather => {
                layer.and_then(|index| synthesis.photo_blend_is_fold(index)) == Some(false)
            }
            field if SynthesisNumberField::PHOTO_BLEND_FIELDS.contains(&field) => {
                layer.and_then(|index| synthesis.photo_blend_is_fold(index)) == Some(true)
            }
            SynthesisNumberField::RepeatFrequency | SynthesisNumberField::RepeatFeather => {
                layer.is_some_and(|index| synthesis.layer_has_source(index))
            }
            field if field.patch_slot().is_some() => {
                layer.is_some_and(|index| synthesis.layer_has_source(index))
            }
            _ => true,
        }
    }
}

enum SynthesisNumberOwner<E> {
    Paint {
        canvas: Option<CanvasLayerEditor<E>>,
        pass: usize,
    },
    Texture,
}

impl<E> SynthesisNumberOwner<E> {
    fn paint(canvas: Option<&CanvasLayerEditor<E>>, pass: usize) -> Self {
        Self::Paint {
            canvas: canvas.cloned(),
            pass,
        }
    }
}

enum SynthesisNumberDraftOwner<E> {
    Paint {
        style: StyleEdit,
        canvas: Option<CanvasLayerEditor<E>>,
        pass: usize,
    },
    Texture(SynthesisLinkState<E>),
}

struct SynthesisNumberDraft<E> {
    filter: PointwiseFilter<E>,
    owner: SynthesisNumberDraftOwner<E>,
    synthesis: SynthesisEdit,
}

impl<E: 'static> SynthesisNumberDraft<E> {
    fn capture(
        branches: &[Option<PointwiseNode<E>>],
        owner: &PointwiseNode<E>,
        receiver: &SynthesisNumberOwner<E>,
        expected: &SynthesisEdit,
    ) -> Result<Self, GtxGraphError<E>> {
        if !graph_nodes(branches).any(|node| node.same_reference(owner)) {
            return Err(stale_paint());
        }
        let filter = owner.filter().ok_or_else(stale_paint)?;
        let (owner, synthesis) = match receiver {
            SynthesisNumberOwner::Paint { canvas, pass } => {
                let style = match canvas {
                    Some(canvas) if canvas.belongs_to(&filter) => canvas.style_edit(),
                    Some(_) => None,
                    None => filter.style_edit(),
                }
                .ok_or_else(stale_paint)?;
                if style.is_synthesis_adjusted(*pass) {
                    return Err(GtxGraphError::SynthesisAdjuster(
                        "The selected texture uses a shared Material.",
                    ));
                }
                let synthesis = style.synthesis_edit(*pass);
                let owner = SynthesisNumberDraftOwner::Paint {
                    style,
                    canvas: canvas.clone(),
                    pass: *pass,
                };
                (owner, synthesis)
            }
            SynthesisNumberOwner::Texture => (
                SynthesisNumberDraftOwner::Texture(standalone_synthesis_state(&filter)?),
                filter.synthesis_edit(),
            ),
        };
        let synthesis = synthesis
            .filter(|current| current.same_revision(expected))
            .ok_or_else(stale_paint)?;
        Ok(Self {
            filter,
            owner,
            synthesis,
        })
    }

    fn into_edit(self) -> Result<SynthesisLabTarget<E>, GtxGraphError<E>> {
        match self.owner {
            SynthesisNumberDraftOwner::Paint {
                mut style,
                canvas,
                pass,
            } => {
                style
                    .set_synthesis(pass, self.synthesis)
                    .map_err(GtxGraphError::NumericAdjuster)?;
                EnvironmentPaintEdit::from_style_edit(&self.filter, canvas.as_ref(), style)
                    .map(SynthesisLabTarget::Paint)
                    .ok_or_else(stale_paint)
            }
            SynthesisNumberDraftOwner::Texture(before) => {
                let texture = self
                    .filter
                    .prepare_synthesis_edit(self.synthesis)
                    .map_err(|error| GtxGraphError::Node(error.into()))?
                    .ok_or_else(stale_paint)?;
                let [expected, local] = texture.records();
                if !Arc::ptr_eq(before.local.texture(), expected.texture()) {
                    return Err(stale_paint());
                }
                let after = SynthesisLinkState {
                    adjustment: before.adjustment.clone(),
                    catalog: None,
                    local,
                };
                Ok(SynthesisLabTarget::LocalTexture(
                    SynthesisReceiverEdit::new(&self.filter, before, after),
                ))
            }
        }
    }
}

fn standalone_synthesis_state<E: 'static>(
    filter: &PointwiseFilter<E>,
) -> Result<SynthesisLinkState<E>, GtxGraphError<E>> {
    let state = filter.synthesis_link_state().ok_or_else(stale_paint)?;
    let adjusted = match &state.adjustment {
        Some(adjustment) if adjustment.property_type() == 10 => adjustment
            .is_active()
            .map_err(|error| GtxGraphError::Node(error.into()))?,
        _ => false,
    };
    if state.catalog.is_some() || adjusted {
        return Err(GtxGraphError::NumericAdjuster(
            "Unlink the whole Texture before editing a Synthesis field.",
        ));
    }
    Ok(state)
}

pub(super) fn link_keys(adjustment: &AdjustmentRecord) -> impl Iterator<Item = LinkKey<'_>> {
    (0..adjustment.components().len()).filter_map(|axis| link_key(adjustment, axis))
}

pub(super) fn link_key(adjustment: &AdjustmentRecord, axis: usize) -> Option<LinkKey<'_>> {
    let component = adjustment.components().get(axis)?;
    (component.active && !component.name.is_empty()).then_some((
        adjustment.guid,
        adjustment.property_type,
        &component.name,
    ))
}

pub(super) fn matches_key(
    metadata: &AdjusterMetadataRecord,
    (group, tag, name): LinkKey<'_>,
) -> bool {
    metadata.group_id == group
        && matches!((tag, metadata.type_tag), (0 | 4, 0 | 4) | (1, 1))
        && metadata.name.as_deref() == Some(name)
}

fn connection_record<E>(
    field: SynthesisNumberField,
    metadata: &AdjusterMetadataRecord,
) -> Result<AdjustmentRecord, GtxGraphError<E>> {
    let name = metadata.name.as_ref().ok_or_else(unavailable_control)?;
    let mut adjustment = AdjustmentRecord::new(&[field.mapping_range().map(|bound| bound as f32)]);
    adjustment.guid = metadata.group_id;
    adjustment.property_type = match field {
        SynthesisNumberField::BlendMode | SynthesisNumberField::ErodeMode => 4,
        _ => metadata.type_tag,
    };
    let component = &mut adjustment.components_mut()[0];
    component.active = true;
    component.name.clone_from(name);
    component.curve_mut();
    Ok(adjustment)
}

fn sample_connection<E: 'static>(
    adjustment: &AdjustmentRecord,
    axis: usize,
    metadata: &AdjusterMetadataRecord,
    value: &NumericCatalogValue,
) -> Result<f64, GtxGraphError<E>> {
    let update =
        NumericCatalogUpdate::reconnect(metadata, value).ok_or_else(unavailable_control)?;
    let (_, mapped) = map_retained_number(&[update], adjustment, axis)
        .map_err(|error| GtxGraphError::Node(error.into()))?
        .ok_or_else(unavailable_control)?;
    Ok(mapped)
}

fn stale_paint<E>() -> GtxGraphError<E> {
    GtxGraphError::NumericAdjuster(
        "The selected Texture Synthesis paint changed. Select its field again.",
    )
}

fn unavailable_control<E>() -> GtxGraphError<E> {
    GtxGraphError::NumericAdjuster(
        "This Number adjuster is no longer available in this Lab section.",
    )
}

pub(super) enum SynthesisLabTarget<E> {
    Paint(EnvironmentPaintEdit<E>),
    Texture(TextureEdit<E>),
    LocalTexture(SynthesisReceiverEdit<E>),
    Material(SynthesisAdjusterBatch<E>),
}

impl<E: 'static> SynthesisLabTarget<E> {
    fn notify(&self, owners: &[PointwiseFilter<E>]) -> Result<(), HistoryError<E>> {
        for owner in owners {
            match self {
                Self::Paint(_) => EnvironmentPaintEdit::notify(owner),
                Self::Texture(_) => TextureEdit::notify(owner),
                Self::LocalTexture(_) | Self::Material(_) => owner
                    .notify_synthesis_receiver_change()
                    .map_err(|error| HistoryError::Property(Box::new(error))),
            }?;
        }
        Ok(())
    }
}

struct SynthesisLabEdit<E> {
    target: SynthesisLabTarget<E>,
    created: Vec<Arc<AtomicBool>>,
    retired: Vec<Arc<AtomicBool>>,
}

impl<E: 'static> SynthesisLabEdit<E> {
    fn publish(
        &self,
        direction: HistoryDirection,
    ) -> Result<Vec<PointwiseFilter<E>>, HistoryError<E>> {
        let undo = matches!(direction, HistoryDirection::Undo);
        if self
            .created
            .iter()
            .any(|active| active.load(Ordering::Acquire) != undo)
            || self
                .retired
                .iter()
                .any(|active| active.load(Ordering::Acquire) == undo)
        {
            return Err(HistoryError::InputChanged);
        }
        let owners = match &self.target {
            SynthesisLabTarget::Paint(paint) => {
                EnvironmentPaintEdit::publish_all(std::slice::from_ref(paint), undo)?
            }
            SynthesisLabTarget::Texture(texture) => vec![texture.publish(direction)?],
            SynthesisLabTarget::LocalTexture(texture) => {
                SynthesisReceiverEdit::publish_batch(std::slice::from_ref(texture), undo)?
            }
            SynthesisLabTarget::Material(material) => material.publish(direction, || Ok(()))?.1,
        };
        for active in &self.created {
            active.store(!undo, Ordering::Release);
        }
        for active in &self.retired {
            active.store(undo, Ordering::Release);
        }
        Ok(owners)
    }
}

impl<E: 'static> HistoryGraphEdit<E> for Arc<SynthesisLabEdit<E>> {
    fn apply(&self, direction: HistoryDirection) -> Result<(), HistoryError<E>> {
        self.target.notify(&self.publish(direction)?)
    }
}
