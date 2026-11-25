use super::catalog::GraphCatalog;
use super::environment_adjusters::{
    EnvironmentAdjusterBatch, enclosing_adjuster_group, graph_nodes,
};
use super::numeric_adjusters::{NumericCatalogUpdate, map_retained_number, numeric_catalog_values};
use super::style_synthesis_numbers::SynthesisLabTarget;
use super::{Gtx375GraphDocument, GtxCatalogRecord, GtxGraphError};
use crate::pointwise::{
    EnvironmentPaintEdit, SynthesisPaint,
    property::{SynthesisLinkState, WeakPointwiseFilter},
};
use crate::{
    AdjusterMetadataRecord, AdjustmentRecord, CanvasLayerEditor, HistoryDirection, HistoryError,
    ImageGeometry, NodeWireError, PointwiseError, PointwiseFilter, PointwiseKind, PointwiseNode,
    StyleEdit, SynthesisCatalogValue, SynthesisEdit, SynthesisRecord,
};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

#[path = "synthesis_draft_preview.rs"]
mod draft_preview;
pub use draft_preview::SynthesisDraftPreviewTarget;

#[derive(Clone, Debug)]
pub struct SynthesisAdjuster {
    name: Vec<u16>,
    pub(super) value: SynthesisCatalogValue,
}

impl SynthesisAdjuster {
    pub(super) fn from_catalog(
        metadata: &AdjusterMetadataRecord,
        value: &SynthesisCatalogValue,
    ) -> Option<Self> {
        let name = metadata.name.as_ref().filter(|name| !name.is_empty())?;
        Some(Self {
            name: name.clone(),
            value: value.clone(),
        })
    }

    pub fn name(&self) -> &[u16] {
        &self.name
    }

    pub fn same_reference(&self, other: &Self) -> bool {
        self.value.same_reference(&other.value)
    }

    /// Starts an editor draft for the shared texture rather than a specific receiving node.
    pub fn synthesis_edit(&self) -> SynthesisEdit {
        self.value.record().draft()
    }
}

#[derive(Clone)]
pub enum SynthesisAdjusterLink {
    Create(String),
    Existing(SynthesisAdjuster),
    Unlink,
}

struct MaterialLinkDecision {
    destination: Option<SynthesisAdjuster>,
    create: bool,
    label: &'static str,
}

struct MaterialDraft {
    value: SynthesisCatalogValue,
    before: SynthesisRecord,
    after: SynthesisRecord,
}

pub(super) type SynthesisLayoutReceiver<'a> = (u64, &'a GtxCatalogRecord, &'a Arc<AtomicBool>);

/// Definitions vacated by removal of all their receivers in this graph edit.
/// History tombstones from previous edits are outside the lookup.
pub(super) struct RetiredSynthesisDefinitions<'a>(Vec<&'a GtxCatalogRecord>);

impl<'a> RetiredSynthesisDefinitions<'a> {
    pub(super) fn removed_by_layout<E: 'static>(
        previous: Vec<SynthesisLayoutReceiver<'a>>,
        projected: &[(PointwiseNode<E>, [u8; 16])],
    ) -> Self {
        if previous.is_empty() {
            return Self(Vec::new());
        }
        let live_nodes: HashSet<_> = projected
            .iter()
            .map(|(node, _)| node.occurrence_id())
            .collect();
        let live_controls: HashSet<_> = previous
            .iter()
            .filter(|(node, _, _)| live_nodes.contains(node))
            .map(|(_, _, marker)| Arc::as_ptr(marker))
            .collect();
        Self(
            previous
                .into_iter()
                .filter(|(_, _, marker)| !live_controls.contains(&Arc::as_ptr(marker)))
                .map(|(_, entry, _)| entry)
                .collect(),
        )
    }

    pub(super) fn stage_destination(
        &self,
        staged: &mut Vec<GtxCatalogRecord>,
        source: &GtxCatalogRecord,
        scope: [u8; 16],
    ) -> Option<usize> {
        let name = source.metadata().name.as_deref()?;
        let matches_key = |entry: &GtxCatalogRecord| {
            let metadata = entry.metadata();
            metadata.type_tag == 10
                && metadata.group_id == scope
                && metadata.name.as_deref() == Some(name)
        };
        let retired = self.0.iter().rev().find(|entry| matches_key(entry))?;
        if let Some(index) = staged.iter().position(matches_key) {
            return Some(index);
        }
        let GtxCatalogRecord::Synthesis { metadata, value } = retired else {
            return None;
        };
        let index = staged.len();
        staged.push(GtxCatalogRecord::Synthesis {
            metadata: metadata.clone(),
            value: SynthesisCatalogValue::new(value.record()),
        });
        Some(index)
    }
}

fn material_scope<E: 'static>(receiver: &PointwiseFilter<E>) -> [u8; 16] {
    enclosing_adjuster_group(&receiver.node())
        .and_then(|group| group.group_guid())
        .unwrap_or_default()
}

impl<E: 'static> Gtx375GraphDocument<E> {
    pub fn synthesis_adjuster(&self, receiver: &PointwiseFilter<E>) -> Option<SynthesisAdjuster> {
        let selected = receiver.synthesis_catalog()?;
        self.synthesis_adjusters()
            .map(|(_, adjuster)| adjuster)
            .find(|adjuster| adjuster.value.same_reference(&selected))
    }

    pub fn available_synthesis_adjusters(
        &self,
        receiver: &PointwiseFilter<E>,
    ) -> Vec<SynthesisAdjuster> {
        self.synthesis_adjusters_in_group(material_scope(receiver))
            .collect()
    }

    fn synthesis_adjusters(&self) -> impl Iterator<Item = ([u8; 16], SynthesisAdjuster)> {
        self.catalog.entries().filter_map(|(entry, _)| {
            let GtxCatalogRecord::Synthesis { metadata, value } = entry else {
                return None;
            };
            Some((
                metadata.group_id,
                SynthesisAdjuster::from_catalog(metadata, value)?,
            ))
        })
    }

    fn synthesis_adjusters_in_group(
        &self,
        group_id: [u8; 16],
    ) -> impl Iterator<Item = SynthesisAdjuster> {
        self.synthesis_adjusters()
            .filter_map(move |(scope, adjuster)| (scope == group_id).then_some(adjuster))
    }

    /// Validates a shared Material handle against the active Lab section.
    pub fn active_lab_synthesis_adjuster(
        &self,
        selected: Option<&PointwiseNode<E>>,
        adjuster: &SynthesisAdjuster,
    ) -> Option<SynthesisAdjuster> {
        let group = super::lab_adjuster_metadata::section_id(selected);
        self.synthesis_adjusters_in_group(group)
            .find(|current| current.same_reference(adjuster))
    }

    /// Replaces the catalog texture and its live receivers in one history edit.
    pub fn edit_lab_synthesis_adjuster(
        &mut self,
        selected: Option<&PointwiseNode<E>>,
        adjuster: &SynthesisAdjuster,
        edit: SynthesisEdit,
        label: &str,
    ) -> Result<bool, GtxGraphError<E>> {
        let branches = self.branches();
        let Some(MaterialDraft {
            value,
            before,
            after,
        }) = self.prepare_lab_material_draft(&branches, selected, adjuster, edit)?
        else {
            return Ok(false);
        };
        self.commit_shared_material(value, before, after, label)
    }

    fn prepare_lab_material_draft(
        &self,
        branches: &[Option<PointwiseNode<E>>],
        selected: Option<&PointwiseNode<E>>,
        adjuster: &SynthesisAdjuster,
        edit: SynthesisEdit,
    ) -> Result<Option<MaterialDraft>, GtxGraphError<E>> {
        if selected.is_some_and(|selected| {
            !graph_nodes(branches).any(|node| node.same_reference(selected))
        }) {
            return Err(stale_lab_material());
        }
        let material = self
            .active_lab_synthesis_adjuster(selected, adjuster)
            .ok_or_else(stale_lab_material)?;
        let before = material.value.record();
        let after = before
            .apply_draft(edit)
            .map_err(GtxGraphError::SynthesisAdjuster)?;
        Ok(after.map(|after| MaterialDraft {
            value: material.value,
            before,
            after,
        }))
    }

    /// The resolved Material of a Texture Synthesis paint in this Style draft.
    pub fn style_synthesis_adjuster(
        &self,
        style: &StyleEdit,
        pass: usize,
    ) -> Option<SynthesisAdjuster> {
        let value = style.synthesis_paint(pass)?.catalog()?;
        self.synthesis_adjusters()
            .map(|(_, adjuster)| adjuster)
            .find(|adjuster| adjuster.value.same_reference(value))
    }

    pub fn available_style_synthesis_adjusters(
        &self,
        owner: &PointwiseNode<E>,
    ) -> Vec<SynthesisAdjuster> {
        let scope = enclosing_adjuster_group(owner)
            .and_then(|group| group.group_guid())
            .unwrap_or_default();
        self.synthesis_adjusters_in_group(scope).collect()
    }

    /// Links the active Texture Synthesis effect of a Style pass or Canvas layer.
    pub fn set_style_synthesis_adjuster(
        &mut self,
        owner: &PointwiseNode<E>,
        canvas: Option<&CanvasLayerEditor<E>>,
        expected: &StyleEdit,
        pass: usize,
        link: SynthesisAdjusterLink,
    ) -> Result<bool, GtxGraphError<E>> {
        let branches = self.branches();
        let (filter, mut style) = Self::material_style_owner(&branches, owner, canvas, expected)?;
        let before = style
            .synthesis_paint(pass)
            .ok_or_else(stale_material_paint)?;
        let texture = before.record();
        let scope = material_scope(&filter);
        let Some(MaterialLinkDecision {
            destination,
            create,
            label,
        }) = self.select_material_destination(
            scope,
            before.catalog(),
            before.key().is_some(),
            &texture,
            link,
        )?
        else {
            return Ok(false);
        };
        let (mut retired, retired_records) = self.retired_synthesis_materials(
            &branches,
            before.catalog().cloned().map(|value| (value, 1)),
        );
        let removed_local = (before.catalog().is_none()).then(|| texture.clone());
        let added = match &destination {
            None => Some(texture.clone()),
            Some(adjuster) if create => Some(adjuster.value.record()),
            Some(_) => None,
        };
        let removed = SynthesisRecord::removed_lab_links_between(
            retired_records.into_iter().chain(removed_local),
            added,
        );
        let keys: Vec<_> = removed
            .iter()
            .map(|(scope, tag, name)| (*scope, *tag, name.as_slice()))
            .collect();
        retired.extend(self.retired_synthesis_number_links(&branches, &keys));
        retired.extend(self.retired_synthesis_color_links(&branches, &keys));

        let after = if let Some(adjuster) = &destination {
            let mut adjustment = before
                .adjustment
                .as_deref()
                .filter(|record| record.property_type == 10)
                .cloned()
                .unwrap_or_else(|| AdjustmentRecord::new(&[[0., 0.]]));
            adjustment.property_type = 10;
            adjustment.guid = scope;
            let component = &mut adjustment.components_mut()[0];
            component.name = adjuster.name.clone();
            component.active = true;
            let local = if before.catalog().is_some() {
                before.local.clone()
            } else {
                texture
                    .without_numeric_adjustments()
                    .without_color_adjustments()
                    .texture()
                    .clone()
            };
            SynthesisPaint {
                local,
                adjustment: Some(Arc::new(adjustment)),
                resolved: Some((adjuster.value.clone(), adjuster.value.record())),
            }
        } else {
            SynthesisPaint {
                local: texture.texture().clone(),
                adjustment: None,
                resolved: None,
            }
        };
        if !style.replace_synthesis_paint(pass, after) {
            return Err(stale_material_paint());
        }
        let edit = EnvironmentPaintEdit::from_style_edit(&filter, canvas, style)
            .ok_or_else(stale_material_paint)?;
        let created = destination
            .filter(|_| create)
            .map(|adjuster| self.new_material_record(scope, adjuster));
        self.commit_synthesis_catalog_change(
            SynthesisLabTarget::Paint(edit),
            created,
            retired,
            label,
        )
    }

    /// Publishes a draft opened explicitly against the shared Material of a Style or Canvas pass.
    pub fn edit_style_synthesis_material(
        &mut self,
        owner: &PointwiseNode<E>,
        canvas: Option<&CanvasLayerEditor<E>>,
        expected: &StyleEdit,
        pass: usize,
        edit: SynthesisEdit,
        label: &str,
    ) -> Result<bool, GtxGraphError<E>> {
        let branches = self.branches();
        let (_, style) = Self::material_style_owner(&branches, owner, canvas, expected)?;
        let Some(MaterialDraft {
            value,
            before,
            after,
        }) = self.prepare_style_material_draft(&style, pass, edit)?
        else {
            return Ok(false);
        };
        self.commit_shared_material(value, before, after, label)
    }

    fn prepare_style_material_draft(
        &self,
        style: &StyleEdit,
        pass: usize,
        edit: SynthesisEdit,
    ) -> Result<Option<MaterialDraft>, GtxGraphError<E>> {
        let Some((value, before, after)) = style
            .prepare_synthesis_material(pass, edit)
            .map_err(GtxGraphError::SynthesisAdjuster)?
        else {
            return Ok(None);
        };
        if !self
            .style_synthesis_adjuster(style, pass)
            .is_some_and(|current| {
                current.value.same_reference(&value)
                    && Arc::ptr_eq(current.value.record().texture(), before.texture())
            })
        {
            return Err(stale_material_paint());
        }
        Ok(Some(MaterialDraft {
            value,
            before,
            after,
        }))
    }

    fn commit_shared_material(
        &mut self,
        value: SynthesisCatalogValue,
        before: SynthesisRecord,
        after: SynthesisRecord,
        label: &str,
    ) -> Result<bool, GtxGraphError<E>> {
        let branches = self.branches();
        let removed = SynthesisRecord::removed_lab_links_between([before.clone()], [after.clone()]);
        let keys: Vec<_> = removed
            .iter()
            .map(|(scope, tag, name)| (*scope, *tag, name.as_slice()))
            .collect();
        let mut retired = self.retired_synthesis_number_links(&branches, &keys);
        retired.extend(self.retired_synthesis_color_links(&branches, &keys));
        let materials = SynthesisAdjusterBatch::replace_material(&branches, value, before, after);
        self.commit_synthesis_catalog_change(
            SynthesisLabTarget::Material(materials),
            None,
            retired,
            label,
        )
    }

    fn material_style_owner(
        branches: &[Option<PointwiseNode<E>>],
        owner: &PointwiseNode<E>,
        canvas: Option<&CanvasLayerEditor<E>>,
        expected: &StyleEdit,
    ) -> Result<(PointwiseFilter<E>, StyleEdit), GtxGraphError<E>> {
        if !graph_nodes(branches).any(|node| node.same_reference(owner)) {
            return Err(stale_material_paint());
        }
        let filter = owner.filter().ok_or_else(stale_material_paint)?;
        let style = match canvas {
            Some(canvas) if canvas.belongs_to(&filter) => canvas.style_edit(),
            Some(_) => None,
            None => filter.style_edit(),
        }
        .filter(|style| style.same_revision(expected))
        .ok_or_else(stale_material_paint)?;
        Ok((filter, style))
    }

    pub(super) fn retired_synthesis_materials(
        &self,
        branches: &[Option<PointwiseNode<E>>],
        removed: impl IntoIterator<Item = (SynthesisCatalogValue, usize)>,
    ) -> (Vec<Arc<AtomicBool>>, Vec<SynthesisRecord>) {
        let mut markers = Vec::new();
        let mut records = Vec::new();
        for (value, count) in removed {
            let live: usize = graph_nodes(branches)
                .filter_map(|node| node.filter())
                .map(|filter| {
                    let mut local = usize::from(
                        filter
                            .synthesis_catalog()
                            .is_some_and(|catalog| catalog.same_reference(&value)),
                    );
                    filter.visit_synthesis_material_paints(|paint| {
                        local += usize::from(
                            paint
                                .catalog()
                                .is_some_and(|catalog| catalog.same_reference(&value)),
                        );
                    });
                    local
                })
                .sum();
            if count != live {
                continue;
            }
            if let Some((_, marker)) = self.catalog.entries().find(|(entry, _)| {
                matches!(entry, GtxCatalogRecord::Synthesis { value: current, .. }
                    if current.same_reference(&value))
            }) {
                markers.push(marker.clone());
                records.push(value.record());
            }
        }
        (markers, records)
    }

    fn new_material_record(
        &self,
        scope: [u8; 16],
        adjuster: SynthesisAdjuster,
    ) -> GtxCatalogRecord {
        let order = self
            .catalog
            .entries()
            .filter(|(entry, _)| entry.metadata().group_id == scope)
            .map(|(entry, _)| entry.metadata().order)
            .max()
            .map_or(0, |last| last.saturating_add(1));
        GtxCatalogRecord::Synthesis {
            metadata: AdjusterMetadataRecord {
                name: Some(adjuster.name),
                group_id: scope,
                type_tag: 10,
                order,
                loaded: false,
            },
            value: adjuster.value,
        }
    }

    fn select_material_destination(
        &self,
        scope: [u8; 16],
        current: Option<&SynthesisCatalogValue>,
        adjusted: bool,
        texture: &SynthesisRecord,
        link: SynthesisAdjusterLink,
    ) -> Result<Option<MaterialLinkDecision>, GtxGraphError<E>> {
        let decision = match link {
            SynthesisAdjusterLink::Create(name) => {
                let name: Vec<_> = name.trim().encode_utf16().collect();
                if name.is_empty() {
                    return Err(GtxGraphError::SynthesisAdjuster(
                        "Enter a name for the Synthesis adjuster.",
                    ));
                }
                if self
                    .synthesis_adjusters_in_group(scope)
                    .any(|other| other.name == name)
                {
                    return Err(GtxGraphError::SynthesisAdjuster(
                        "A Synthesis adjuster already uses that name in this section.",
                    ));
                }
                MaterialLinkDecision {
                    destination: Some(SynthesisAdjuster {
                        name,
                        value: SynthesisCatalogValue::new(
                            texture
                                .without_numeric_adjustments()
                                .without_color_adjustments(),
                        ),
                    }),
                    create: true,
                    label: "Create Synthesis Adjuster",
                }
            }
            SynthesisAdjusterLink::Existing(adjuster) => {
                let adjuster = self
                    .synthesis_adjusters_in_group(scope)
                    .find(|other| other.same_reference(&adjuster))
                    .ok_or(GtxGraphError::SynthesisAdjuster(
                        "This Synthesis adjuster is no longer available in this section.",
                    ))?;
                if current.is_some_and(|value| value.same_reference(&adjuster.value)) {
                    return Ok(None);
                }
                MaterialLinkDecision {
                    destination: Some(adjuster),
                    create: false,
                    label: "Link Synthesis Adjuster",
                }
            }
            SynthesisAdjusterLink::Unlink => {
                if !adjusted {
                    return Ok(None);
                }
                MaterialLinkDecision {
                    destination: None,
                    create: false,
                    label: "Unlink Synthesis Adjuster",
                }
            }
        };
        Ok(Some(decision))
    }

    /// Connect a Synthesis Texture to the current Lab section. Unlinking keeps
    /// the texture that the node was using at the time of the command.
    pub fn set_synthesis_adjuster(
        &mut self,
        receiver: &PointwiseFilter<E>,
        link: SynthesisAdjusterLink,
    ) -> Result<bool, GtxGraphError<E>> {
        let stale = GtxGraphError::SynthesisAdjuster("This Synthesis node is no longer available.");
        let before = receiver.synthesis_link_state().ok_or(stale)?;
        let target = receiver.node();
        if !graph_nodes(&self.branches()).any(|node| node.same_reference(&target)) {
            return Err(GtxGraphError::SynthesisAdjuster(
                "This Synthesis node is no longer available.",
            ));
        }
        let scope = material_scope(receiver);
        let texture = before
            .catalog
            .as_ref()
            .map_or_else(|| before.local.clone(), SynthesisCatalogValue::record);
        let Some(MaterialLinkDecision {
            destination,
            create,
            label,
        }) = self.select_material_destination(
            scope,
            before.catalog.as_ref(),
            receiver.is_synthesis_adjusted(),
            &texture,
            link,
        )?
        else {
            return Ok(false);
        };
        let wire = receiver
            .synthesis_wire()
            .ok_or(GtxGraphError::SynthesisAdjuster(
                "This Synthesis node is no longer available.",
            ))?;
        let adjustment = destination
            .as_ref()
            .map(|adjuster| wire.new_reference_adjustment(10, &adjuster.name, scope))
            .transpose()
            .map_err(|error| GtxGraphError::Node(NodeWireError::Property(error)))?;
        let branches = self.branches();
        let (retired_materials, _) = self
            .retired_synthesis_materials(&branches, before.catalog.clone().map(|value| (value, 1)));
        // Shared Material records count once, independently of how many nodes
        // currently use them. An unlinked node keeps the currently visible
        // record, including any nested links in an imported Material.
        let removed_texture =
            (before.catalog.is_none() || !retired_materials.is_empty()).then(|| texture.clone());
        let added_texture = match &destination {
            None => Some(texture.clone()),
            Some(adjuster) if create => Some(adjuster.value.record()),
            Some(_) => None,
        };
        let removed = SynthesisRecord::removed_lab_links_between(removed_texture, added_texture);
        let keys: Vec<_> = removed
            .iter()
            .map(|(group, tag, name)| (*group, *tag, name.as_slice()))
            .collect();
        let mut retired = self.retired_synthesis_number_links(&branches, &keys);
        retired.extend(retired_materials);
        let after = SynthesisLinkState {
            adjustment,
            catalog: destination.as_ref().map(|adjuster| adjuster.value.clone()),
            local: if destination.is_none() {
                texture
            } else {
                before.local.clone()
            },
        };
        let created = destination
            .filter(|_| create)
            .map(|adjuster| self.new_material_record(scope, adjuster));
        self.commit_synthesis_catalog_change(
            SynthesisLabTarget::LocalTexture(SynthesisReceiverEdit::new(receiver, before, after)),
            created,
            retired,
            label,
        )
    }
}

fn stale_material_paint<E>() -> GtxGraphError<E> {
    GtxGraphError::SynthesisAdjuster("This Texture Synthesis pass is no longer available.")
}

fn stale_lab_material<E>() -> GtxGraphError<E> {
    GtxGraphError::SynthesisAdjuster("This Material is no longer available in this Lab section.")
}

pub(super) struct SynthesisReceiverEdit<E> {
    receiver: WeakPointwiseFilter<E>,
    before: SynthesisLinkState<E>,
    after: SynthesisLinkState<E>,
}

impl<E: 'static> SynthesisReceiverEdit<E> {
    pub(super) fn new(
        receiver: &PointwiseFilter<E>,
        before: SynthesisLinkState<E>,
        after: SynthesisLinkState<E>,
    ) -> Self {
        Self {
            receiver: receiver.downgrade(),
            before,
            after,
        }
    }

    fn replace(&self, filter: &PointwiseFilter<E>, undo: bool) -> bool {
        let (current, next) = if undo {
            (&self.after, &self.before)
        } else {
            (&self.before, &self.after)
        };
        filter.replace_synthesis_link(current, next)
    }

    pub(super) fn publish_batch(
        edits: &[Self],
        undo: bool,
    ) -> Result<Vec<PointwiseFilter<E>>, HistoryError<E>> {
        let receivers = edits
            .iter()
            .map(|edit| edit.receiver.upgrade().ok_or(HistoryError::NullTarget))
            .collect::<Result<Vec<_>, _>>()?;
        for (index, (edit, receiver)) in edits.iter().zip(&receivers).enumerate() {
            if !edit.replace(receiver, undo) {
                Self::rollback_batch(&edits[..index], &receivers[..index], undo);
                return Err(HistoryError::InputChanged);
            }
        }
        Ok(receivers)
    }

    pub(super) fn rollback_batch(edits: &[Self], receivers: &[PointwiseFilter<E>], undo: bool) {
        for (edit, receiver) in edits.iter().zip(receivers).rev() {
            edit.replace(receiver, !undo);
        }
    }
}

pub(super) fn restore_synthesis_catalog<E: 'static>(
    branches: &[Option<PointwiseNode<E>>],
    catalog: &[GtxCatalogRecord],
) -> Result<(), PointwiseError<E>> {
    let mut materials = HashMap::new();
    for entry in catalog {
        if let GtxCatalogRecord::Synthesis { metadata, value } = entry
            && let Some(name) = metadata.name.as_ref().filter(|name| !name.is_empty())
        {
            materials
                .entry((metadata.group_id, name.clone()))
                .or_insert(value);
        }
    }
    if materials.is_empty() {
        return Ok(());
    }
    for node in graph_nodes(branches) {
        let Some(filter) = node.filter() else {
            continue;
        };
        if let Some(adjustment) = filter
            .synthesis_wire()
            .and_then(|wire| wire.retained_adjustment())
            && adjustment.property_type() == 10
            && adjustment.is_active()?
            && let Some(name) = adjustment.names().entries().into_iter().next().flatten()
            && let Some(value) = materials.get(&(adjustment.guid(), name))
        {
            filter.attach_synthesis_catalog((*value).clone());
        }
        let mut embedded = Vec::new();
        EnvironmentPaintEdit::stage_synthesis_materials(&mut embedded, &filter, |paint| {
            let resolved = paint
                .key()
                .and_then(|(scope, name)| materials.get(&(scope, name.to_vec())))
                .map(|value| ((*value).clone(), value.record()));
            Ok::<_, Infallible>(resolved.map(|resolved| SynthesisPaint {
                resolved: Some(resolved),
                ..paint.clone()
            }))
        })
        .unwrap_or_else(|never| match never {});
        if !embedded.is_empty() {
            EnvironmentPaintEdit::publish_all(&embedded, false)
                .map_err(|error| PointwiseError::History(Box::new(error)))?;
        }
    }
    Ok(())
}

struct SynthesisAdjusterEdit<E> {
    value: SynthesisCatalogValue,
    before: SynthesisRecord,
    after: SynthesisRecord,
    receivers: Vec<WeakPointwiseFilter<E>>,
}

pub(super) struct SynthesisAdjusterBatch<E> {
    shared: Vec<SynthesisAdjusterEdit<E>>,
    local: Vec<SynthesisReceiverEdit<E>>,
    embedded: Vec<EnvironmentPaintEdit<E>>,
}

pub(super) enum PatchShapeInputs<'a> {
    Authoring(&'a [(u64, ImageGeometry)]),
    SavedValues,
}

impl<E: 'static> SynthesisAdjusterBatch<E> {
    pub(super) fn collect_draft_preview(
        &self,
        preview: &mut crate::pointwise::property::capture::DraftRenderOverrides,
    ) {
        for (receiver, after) in self
            .local
            .iter()
            .map(|edit| (&edit.receiver, &edit.after.local))
            .chain(self.shared.iter().flat_map(|edit| {
                edit.receivers
                    .iter()
                    .map(|receiver| (receiver, &edit.after))
            }))
        {
            if let Some(filter) = receiver.upgrade() {
                preview
                    .synthesis
                    .insert(filter.node().occurrence_id(), after.clone());
            }
        }
        for paint in &self.embedded {
            paint.collect_draft_preview(preview);
        }
    }

    pub(super) fn replace_material(
        branches: &[Option<PointwiseNode<E>>],
        value: SynthesisCatalogValue,
        before: SynthesisRecord,
        after: SynthesisRecord,
    ) -> Self {
        let mut receivers = Vec::new();
        let mut embedded = Vec::new();
        for filter in graph_nodes(branches).filter_map(|node| node.filter()) {
            if filter
                .synthesis_catalog()
                .is_some_and(|catalog| value.same_reference(&catalog))
            {
                receivers.push(filter.downgrade());
            }
            EnvironmentPaintEdit::stage_synthesis_materials(&mut embedded, &filter, |paint| {
                Ok::<_, Infallible>(paint.with_material_record(&value, &after))
            })
            .unwrap_or_else(|never| match never {});
        }
        Self {
            shared: vec![SynthesisAdjusterEdit {
                value,
                before,
                after,
                receivers,
            }],
            local: Vec::new(),
            embedded,
        }
    }

    pub(super) fn prepare(
        branches: &[Option<PointwiseNode<E>>],
        catalog: &GraphCatalog,
        environment: &mut EnvironmentAdjusterBatch<E>,
        updates: Vec<(SynthesisCatalogValue, SynthesisRecord)>,
        numeric: &[NumericCatalogUpdate],
        references: Option<&super::environment_adjusters::MapReferenceValues<'_>>,
        patch_shapes: PatchShapeInputs<'_>,
    ) -> Result<Self, GtxGraphError<E>> {
        if numeric.is_empty() && updates.is_empty() && references.is_none() {
            return Ok(Self {
                shared: Vec::new(),
                local: Vec::new(),
                embedded: Vec::new(),
            });
        }
        let mut edits = BTreeMap::new();
        // Repeated preset records select the last value for a catalog entry.
        // Iteration order also fixes the order in which catalog cells are locked.
        let existing = if updates.is_empty() {
            Vec::new()
        } else {
            numeric_catalog_values(catalog)
        };
        let existing_colors = (!updates.is_empty())
            .then(|| super::environment_adjusters::MapReferenceValues::read(catalog))
            .flatten();
        for (value, after) in updates {
            // Replacement records can carry links to numbers not in the preset.
            let after = resolve_synthesis_numbers(&after, &existing, None, false)?.unwrap_or(after);
            let after = existing_colors
                .as_ref()
                .and_then(|colors| after.map_color_adjustments(|key| colors.color_value(key)))
                .unwrap_or(after);
            edits.insert(
                value.lock_order(),
                SynthesisAdjusterEdit {
                    before: value.record(),
                    value,
                    after,
                    receivers: Vec::new(),
                },
            );
        }
        let (input_sizes, preserve_graph_shapes) = match patch_shapes {
            PatchShapeInputs::Authoring(sizes) => (sizes, false),
            PatchShapeInputs::SavedValues => (&[][..], true),
        };
        let mut map = |record: &SynthesisRecord, input| {
            let numbers = resolve_synthesis_numbers(record, numeric, input, preserve_graph_shapes)?;
            let colors = references.and_then(|colors| {
                numbers
                    .as_ref()
                    .unwrap_or(record)
                    .map_color_adjustments(|key| colors.color_value(key))
            });
            Ok(colors.or(numbers))
        };
        Self::stage(
            branches,
            catalog,
            environment,
            edits,
            input_sizes,
            (!numeric.is_empty() || references.is_some()).then_some(&mut map),
        )
    }

    pub(super) fn prepare_mapped(
        branches: &[Option<PointwiseNode<E>>],
        catalog: &GraphCatalog,
        environment: &mut EnvironmentAdjusterBatch<E>,
        mut map: impl FnMut(&SynthesisRecord) -> Result<Option<SynthesisRecord>, GtxGraphError<E>>,
    ) -> Result<Self, GtxGraphError<E>> {
        let mut map = |record: &SynthesisRecord, _| map(record);
        Self::stage(
            branches,
            catalog,
            environment,
            BTreeMap::new(),
            &[],
            Some(&mut map),
        )
    }

    fn stage(
        branches: &[Option<PointwiseNode<E>>],
        catalog: &GraphCatalog,
        environment: &mut EnvironmentAdjusterBatch<E>,
        mut edits: BTreeMap<usize, SynthesisAdjusterEdit<E>>,
        input_sizes: &[(u64, ImageGeometry)],
        mut map: Option<
            &mut impl FnMut(
                &SynthesisRecord,
                Option<ImageGeometry>,
            ) -> Result<Option<SynthesisRecord>, GtxGraphError<E>>,
        >,
    ) -> Result<Self, GtxGraphError<E>> {
        if let Some(map) = &mut map {
            for (entry, _) in catalog.entries() {
                let GtxCatalogRecord::Synthesis { value, .. } = entry else {
                    continue;
                };
                match edits.entry(value.lock_order()) {
                    std::collections::btree_map::Entry::Occupied(mut entry) => {
                        if let Some(after) = map(&entry.get().after, None)? {
                            entry.get_mut().after = after;
                        }
                    }
                    std::collections::btree_map::Entry::Vacant(entry) => {
                        let before = value.record();
                        if let Some(after) = map(&before, None)? {
                            entry.insert(SynthesisAdjusterEdit {
                                value: value.clone(),
                                before,
                                after,
                                receivers: Vec::new(),
                            });
                        }
                    }
                }
            }
        }
        let mut local = Vec::new();
        for filter in graph_nodes(branches).filter_map(|node| node.filter()) {
            if let Some(value) = filter.synthesis_catalog()
                && let Some(edit) = edits.get_mut(&value.lock_order())
            {
                edit.receivers.push(filter.downgrade());
            }
            if let Some(map) = &mut map {
                let input_size = |canvas_input| {
                    if input_sizes.is_empty() {
                        return None;
                    }
                    let source = synthesis_patch_input(&filter, canvas_input)?;
                    input_sizes.iter().find_map(|(id, geometry)| {
                        (*id == source.occurrence_id()).then_some(*geometry)
                    })
                };
                if let Some(before) = filter.synthesis_link_state()
                    && before.catalog.is_none()
                    && let Some(after) = map(&before.local, input_size(None))?
                {
                    let after = SynthesisLinkState {
                        adjustment: before.adjustment.clone(),
                        catalog: None,
                        local: after,
                    };
                    local.push(SynthesisReceiverEdit::new(&filter, before, after));
                }
                environment.stage_synthesis_paints(&filter, |record, canvas_input| {
                    map(record, input_size(canvas_input))
                })?;
            }
            environment.stage_synthesis_materials(&filter, |paint| {
                let replacement = paint
                    .catalog()
                    .and_then(|value| edits.get(&value.lock_order()))
                    .and_then(|edit| paint.with_material_record(&edit.value, &edit.after));
                Ok(replacement)
            })?;
        }
        Ok(Self {
            shared: edits.into_values().collect(),
            local,
            embedded: Vec::new(),
        })
    }

    pub(super) fn publish<T>(
        &self,
        direction: HistoryDirection,
        publish_environment: impl FnOnce() -> Result<T, HistoryError<E>>,
    ) -> Result<(T, Vec<PointwiseFilter<E>>), HistoryError<E>> {
        let undo = matches!(direction, HistoryDirection::Undo);
        let mut values = Vec::with_capacity(self.shared.len());
        for edit in &self.shared {
            let current = edit.value.lock();
            let expected = if undo { &edit.after } else { &edit.before };
            if !Arc::ptr_eq(&current, expected.texture()) {
                return Err(HistoryError::InputChanged);
            }
            values.push(current);
        }
        let mut receivers = Vec::new();
        for edit in &self.shared {
            for target in &edit.receivers {
                let filter = target.upgrade().ok_or(HistoryError::NullTarget)?;
                if filter
                    .synthesis_catalog()
                    .is_none_or(|value| !edit.value.same_reference(&value))
                {
                    return Err(HistoryError::InputChanged);
                }
                receivers.push(filter);
            }
        }
        let local = SynthesisReceiverEdit::publish_batch(&self.local, undo)?;
        let embedded =
            EnvironmentPaintEdit::publish_all(&self.embedded, undo).inspect_err(|_| {
                SynthesisReceiverEdit::rollback_batch(&self.local, &local, undo);
            })?;
        let environment = publish_environment().inspect_err(|_| {
            EnvironmentPaintEdit::rollback_batch(&self.embedded, &embedded, undo);
            SynthesisReceiverEdit::rollback_batch(&self.local, &local, undo);
        })?;
        for (edit, current) in self.shared.iter().zip(&mut values) {
            let next = if undo { &edit.before } else { &edit.after };
            **current = next.texture().clone();
        }
        receivers.extend(local);
        receivers.extend(embedded);
        Ok((environment, receivers))
    }
}

fn resolve_synthesis_numbers<E: 'static>(
    record: &SynthesisRecord,
    updates: &[NumericCatalogUpdate],
    input: Option<ImageGeometry>,
    preserve_graph_shapes: bool,
) -> Result<Option<SynthesisRecord>, GtxGraphError<E>> {
    let resolve = |adjustment: &AdjustmentRecord, axis| {
        map_retained_number(updates, adjustment, axis).map(|mapped| mapped.map(|(_, value)| value))
    };
    if preserve_graph_shapes {
        record.restore_numeric_adjustments_with_saved_graph_shapes(resolve)
    } else if input.is_some() {
        record.map_numeric_adjustments_with_input_geometry(input, resolve)
    } else {
        record.map_numeric_adjustments(resolve)
    }
    .map_err(|error| GtxGraphError::Node(error.into()))
}

fn synthesis_patch_input<E: 'static>(
    filter: &PointwiseFilter<E>,
    canvas_input: Option<i32>,
) -> Option<PointwiseNode<E>> {
    let node = filter.node();
    match filter.kind() {
        PointwiseKind::Canvas => node
            .reference_inputs()
            .ok()?
            .get(usize::try_from(canvas_input?).ok()?)?
            .node(),
        PointwiseKind::Style => node.slot(2).ok()?.node(),
        _ => node.slot(0).ok()?.node(),
    }
}

pub(super) fn connected_patch_adjuster_inputs<E: 'static>(
    branches: &[Option<PointwiseNode<E>>],
    scope: [u8; 16],
    name: &[u16],
) -> Vec<PointwiseNode<E>> {
    connected_patch_adjuster_inputs_for_keys(branches, &[(scope, name)])
}

pub(super) fn connected_patch_adjuster_inputs_for_keys<E: 'static>(
    branches: &[Option<PointwiseNode<E>>],
    keys: &[([u8; 16], &[u16])],
) -> Vec<PointwiseNode<E>> {
    if keys.is_empty() {
        return Vec::new();
    }
    let mut seen = HashSet::new();
    let mut inputs = Vec::new();
    for filter in graph_nodes(branches).filter_map(|node| node.filter()) {
        let mut visit = |record: &SynthesisRecord, canvas_input| {
            if record.uses_connected_patch_shape_adjuster(keys)
                && let Some(source) = synthesis_patch_input(&filter, canvas_input)
                && seen.insert(source.occurrence_id())
            {
                inputs.push(source);
            }
        };
        if let Some(state) = filter
            .synthesis_link_state()
            .filter(|state| state.catalog.is_none())
        {
            visit(&state.local, None);
        }
        filter.visit_retained_synthesis_paints_with_inputs(visit);
    }
    inputs
}

#[cfg(test)]
#[path = "embedded_synthesis_material_tests.rs"]
mod embedded_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AdjustmentRecord, BinaryWriter, CanvasPrimitive, ContextUndoInfo, EnvironmentField,
        EnvironmentImage, EnvironmentLayerKind, EnvironmentRecord, History, HistoryCommand,
        HistoryEffect, HistoryServices, HistoryTarget, ImageGeometry, LabNumericValue,
        LabPresetOperation, NodeWireReadPolicy, PointwiseKind, PointwiseRenderEffects,
        PointwiseRenderOptions, ProceduralExecution, RenderCaptureSession, StylePaintKind,
        StyleSection, SynthesisLayerKind, ValueAdjusterKind, ValueAdjusterPayload,
        ValueAdjusterRecord,
    };
    use std::sync::atomic::Ordering;

    pub(super) struct TestHistory;

    impl HistoryServices<()> for TestHistory {
        fn route_property_history(
            &self,
            _: &History<()>,
            _: &HistoryTarget<()>,
            _: Option<&ContextUndoInfo<()>>,
        ) -> Result<Option<History<()>>, HistoryError<()>> {
            Ok(None)
        }

        fn resolve_command_context(
            &self,
            _: &History<()>,
            _: &HistoryCommand<()>,
        ) -> Result<Option<HistoryTarget<()>>, HistoryError<()>> {
            Ok(None)
        }

        fn effect(
            &self,
            _: &History<()>,
            _: Option<&HistoryCommand<()>>,
            _: HistoryEffect,
        ) -> Result<(), HistoryError<()>> {
            Ok(())
        }

        fn pending_commands(
            &self,
            _: &History<()>,
            _: &HistoryCommand<()>,
        ) -> Result<Vec<HistoryCommand<()>>, HistoryError<()>> {
            Ok(Vec::new())
        }
    }

    #[test]
    #[ignore = "requires the installed Crumpled Paper preset"]
    fn embedded_synthesis_and_environment_share_lab_history_in_style_and_canvas() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
            "../../../genetica/Presets/Filter/Texture Effect/Print & Emboss/Crumpled Paper.gtx",
        );
        let installed = Gtx375GraphDocument::<()>::load(&std::fs::read(path).unwrap()).unwrap();
        let authored = graph_nodes(&installed.branches())
            .filter_map(|node| node.filter())
            .find_map(|filter| filter.synthesis_edit())
            .unwrap();
        let mut texture = BinaryWriter::default();
        authored.save_preset(&[], &mut texture).unwrap();

        // A nonuniform embedded source drives the installed Patches placement.
        let photo = || {
            let mut tga = vec![0; 18];
            tga[2] = 2;
            tga[12] = 64;
            tga[14] = 64;
            tga[16] = 24;
            for y in 0..64u8 {
                for x in 0..64u8 {
                    tga.extend([x * 4, y * 4, (x ^ y) * 4]);
                }
            }
            EnvironmentImage::from_file_bytes(tga, "tga").unwrap()
        };
        let make_owner = |kind| {
            let filter = PointwiseFilter::<()>::new(kind).unwrap();
            let layer = if kind == PointwiseKind::Canvas {
                filter
                    .edit_add_canvas_shape(CanvasPrimitive::Rectangle, None)
                    .unwrap();
                Some(filter.canvas_layer_editors().remove(0))
            } else {
                None
            };
            let mut style = match &layer {
                Some(layer) => layer.style_edit(),
                None => filter.style_edit(),
            }
            .unwrap();
            style.set_paint(0, StylePaintKind::Environment).unwrap();
            style.add(StylePaintKind::Synthesis).unwrap();
            let mut synthesis = style.synthesis_edit(0).unwrap();
            synthesis.load_preset(texture.bytes()).unwrap();
            assert!(synthesis.set_image_source(0, photo()));
            assert!(style.set_synthesis(0, synthesis).unwrap());
            let mut map = EnvironmentRecord::default();
            map.add_layer(EnvironmentLayerKind::Color).unwrap();
            map.set_exposure(-1.);
            let mut link = AdjustmentRecord::new(&[[-2., 2.]]);
            let component = &mut link.components_mut()[0];
            component.name = "Irregularity".encode_utf16().collect();
            component.active = true;
            component.curve_mut();
            map.install_test_adjustment(EnvironmentField::Exposure, link);
            let expected = style.environment_map(1).unwrap().clone();
            style.replace_environment_map(1, &expected, map).unwrap();
            if let Some(layer) = layer {
                layer.edit_style(style, "Set linked paints").unwrap();
            } else {
                for pass in 0..2 {
                    let mut area: Vec<_> = style
                        .controls(pass, StyleSection::Area)
                        .iter()
                        .map(|control| control.value())
                        .collect();
                    area[0] = 0.;
                    style.set_controls(pass, StyleSection::Area, &area).unwrap();
                }
                filter.edit_style(style, "Set linked paints").unwrap();
            }
            filter
        };
        let style = make_owner(PointwiseKind::Style);
        let canvas = make_owner(PointwiseKind::Canvas);
        let group = PointwiseFilter::new(PointwiseKind::Group).unwrap();
        group
            .node()
            .content(0)
            .unwrap()
            .set_node(Some(&style.node()))
            .unwrap();
        let mut document =
            Gtx375GraphDocument::new(vec![Some(group.node()), Some(canvas.node())]).unwrap();
        let mut entry = ValueAdjusterRecord::new(
            ValueAdjusterKind::Number,
            Some("Irregularity".encode_utf16().collect()),
            [0; 16],
        );
        let ValueAdjusterPayload::Number(number) = &mut entry.payload else {
            unreachable!()
        };
        number.value = 100.;
        number.previous = 100.;
        document
            .catalog
            .stage(GtxCatalogRecord::Value(Box::new(entry)))
            .store(true, Ordering::Release);
        let mut gtx = BinaryWriter::default();
        document.save_compatible_gtx(&mut gtx).unwrap();
        let mut document = Gtx375GraphDocument::<()>::load(gtx.bytes()).unwrap();
        let owners = |doc: &Gtx375GraphDocument<()>| {
            let group = doc.branch(0).flatten().unwrap();
            [
                group.content(0).unwrap().node().unwrap(),
                doc.branch(1).flatten().unwrap(),
            ]
        };
        let [style, canvas] = owners(&document);
        let render = |node: &PointwiseNode<()>| {
            let capture = RenderCaptureSession::new().capture_node(node, 0.).unwrap();
            let geometry = ImageGeometry::new(64, 64);
            let workers = ProceduralExecution::CallingThread;
            let mut capacity = |_| Ok(true);
            let mut preflight = |_, _| Ok(());
            let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight);
            let preview = capture
                .render_preview_with_effects(geometry, workers, &mut effects)
                .unwrap();
            let export = capture
                .render_export_with_effects(
                    geometry,
                    workers,
                    PointwiseRenderOptions::new(false, false),
                    &mut effects,
                )
                .unwrap();
            let values = |image: crate::SharedImageMap| {
                (0..4)
                    .flat_map(|channel| image.channel(channel, true).unwrap().values())
                    .collect::<Vec<_>>()
            };
            assert!(
                preview.caught_errors().is_empty(),
                "{:?}",
                preview.caught_errors()
            );
            assert!(
                export.caught_errors().is_empty(),
                "{:?}",
                export.caught_errors()
            );
            let pixels = values(preview.image().unwrap());
            assert_eq!(pixels, values(export.image().unwrap()));
            pixels
        };
        let check = |node: &PointwiseNode<()>, name: &str, exposure| {
            let filter = node.filter().unwrap();
            let name = name.encode_utf16().collect::<Vec<_>>();
            let mut jitter_links = 0;
            filter.visit_synthesis_paints(|record| {
                record.visit_numeric_adjustments(|adjustment| {
                    if adjustment.components()[0].name == name {
                        jitter_links += 1;
                    }
                });
            });
            assert_eq!(jitter_links, 3);
            let edit = filter
                .canvas_layer_editors()
                .first()
                .map_or_else(|| filter.style_edit(), |layer| layer.style_edit())
                .unwrap();
            assert_eq!(edit.environment_map(1).unwrap().exposure(), exposure);
        };
        let synthesize = |node: &PointwiseNode<()>| {
            let filter = node.filter().unwrap();
            let edit = filter
                .canvas_layer_editors()
                .first()
                .map_or_else(|| filter.style_edit(), |layer| layer.style_edit())
                .unwrap();
            let mut capacity = |_| Ok(true);
            let mut preflight = |_, _| Ok(());
            let mut effects = PointwiseRenderEffects::<()>::new(&mut capacity, &mut preflight);
            let image = edit
                .synthesis_edit(0)
                .unwrap()
                .render_preset_thumbnail(None, ProceduralExecution::CallingThread, &mut effects)
                .unwrap();
            image.channel(0, true).unwrap().values()
        };
        let original = [render(&style), render(&canvas)];
        let original_synthesis = [synthesize(&style), synthesize(&canvas)];
        check(&style, "Irregularity", 2.);
        check(&canvas, "Irregularity", 2.);
        let history = document
            .bind_property_history(Arc::new(TestHistory))
            .unwrap();
        history.set_recording(true);
        let group = document.branch(0).flatten().unwrap().filter().unwrap();
        assert!(
            document
                .set_group_catch_adjusters(&group, true, Some("Surface paints"))
                .unwrap()
        );
        assert_eq!(render(&style), original[0]);
        let control = document.lab_adjuster_metadata(Some(&style)).remove(0);
        assert!(
            document
                .edit_lab_adjuster_metadata(Some(&style), &control, "Spacing", control.order())
                .unwrap()
        );
        check(&style, "Spacing", 2.);
        check(&canvas, "Irregularity", 2.);
        assert_eq!(render(&style), original[0]);
        document
            .edit_lab_preset(Some(&style), LabPresetOperation::Save("Default".into()))
            .unwrap();
        let control = document.numeric_lab_adjusters(Some(&style)).remove(0);
        assert!(
            document
                .edit_numeric_lab_adjuster(Some(&style), &control, LabNumericValue::Number(0.))
                .unwrap()
        );
        check(&style, "Spacing", -2.);
        let changed = render(&style);
        assert_ne!(changed, original[0]);
        assert_ne!(synthesize(&style), original_synthesis[0]);
        assert_eq!(render(&canvas), original[1]);

        let preset = document.lab_presets(Some(&style)).presets.remove(0);
        document
            .edit_lab_preset(Some(&style), LabPresetOperation::Apply(preset))
            .unwrap();
        check(&style, "Spacing", 2.);
        assert_eq!(render(&style), original[0]);
        history.undo().unwrap();
        check(&style, "Spacing", -2.);
        assert_eq!(render(&style), changed);
        history.redo().unwrap();
        assert_eq!(render(&style), original[0]);
        history.undo().unwrap();

        let canvas_control = document.numeric_lab_adjusters(None).remove(0);
        assert_eq!(canvas_control.name(), "Irregularity");
        assert!(
            document
                .edit_numeric_lab_adjuster(None, &canvas_control, LabNumericValue::Number(0.))
                .unwrap()
        );
        check(&canvas, "Irregularity", -2.);
        let changed_canvas = render(&canvas);
        assert_ne!(changed_canvas, original[1]);
        assert_ne!(synthesize(&canvas), original_synthesis[1]);
        assert_eq!(render(&style), changed);
        history.undo().unwrap();
        assert_eq!(render(&canvas), original[1]);
        history.redo().unwrap();
        assert_eq!(render(&canvas), changed_canvas);

        let reopen = |document: &mut Gtx375GraphDocument<()>, native| {
            let mut wire = BinaryWriter::default();
            if native {
                document.save_project(&mut wire).unwrap();
                Gtx375GraphDocument::load_project(wire.bytes(), NodeWireReadPolicy::new(128, 8192))
            } else {
                document.save_compatible_gtx(&mut wire).unwrap();
                Gtx375GraphDocument::load(wire.bytes())
            }
            .unwrap()
        };
        for native in [true, false] {
            let loaded = reopen(&mut document, native);
            let [style, canvas] = owners(&loaded);
            check(&style, "Spacing", -2.);
            check(&canvas, "Irregularity", -2.);
            assert_eq!(
                [render(&style), render(&canvas)],
                [changed.clone(), changed_canvas.clone()]
            );
        }

        // The existing value-edit route gives an independent reference for the
        // destination's pixels, including Synthesis without the Environment pass.
        let smooth_synthesis = synthesize(&canvas);
        history.undo().unwrap();
        let mut destination = ValueAdjusterRecord::new(
            ValueAdjusterKind::Number,
            Some("Smooth".encode_utf16().collect()),
            [0; 16],
        );
        let ValueAdjusterPayload::Number(number) = &mut destination.payload else {
            unreachable!()
        };
        number.value = 0.;
        number.previous = 0.;
        document
            .catalog
            .stage(GtxCatalogRecord::Value(Box::new(destination)))
            .store(true, Ordering::Release);
        let source = document
            .lab_adjuster_metadata(None)
            .into_iter()
            .find(|control| control.name() == "Irregularity")
            .unwrap();
        let cursor = history.cursor();
        assert!(
            document
                .edit_lab_adjuster_metadata(None, &source, "Smooth", source.order())
                .unwrap()
        );
        assert_eq!(history.cursor(), cursor + 1);

        let check_merged = |document: &Gtx375GraphDocument<()>| {
            let controls = document.numeric_lab_adjusters(None);
            assert_eq!(controls.len(), 1);
            assert_eq!(controls[0].name(), "Smooth");
            assert_eq!(controls[0].value(), LabNumericValue::Number(0.));
            let [style, canvas] = owners(document);
            check(&style, "Spacing", -2.);
            check(&canvas, "Smooth", -2.);
            assert_eq!(render(&style), changed);
            assert_eq!(render(&canvas), changed_canvas);
            assert_eq!(synthesize(&canvas), smooth_synthesis);
        };
        check_merged(&document);

        history.undo().unwrap();
        let controls = document.numeric_lab_adjusters(None);
        assert_eq!(controls.len(), 2);
        for (name, value) in [("Irregularity", 100.), ("Smooth", 0.)] {
            assert!(controls.iter().any(|control| {
                control.name() == name && control.value() == LabNumericValue::Number(value)
            }));
        }
        check(&style, "Spacing", -2.);
        check(&canvas, "Irregularity", 2.);
        assert_eq!(render(&style), changed);
        assert_eq!(render(&canvas), original[1]);
        assert_eq!(synthesize(&canvas), original_synthesis[1]);
        history.redo().unwrap();
        check_merged(&document);
        for native in [true, false] {
            check_merged(&reopen(&mut document, native));
        }
    }

    #[test]
    fn synthesis_replacement_revives_a_detached_material_for_this_edit() {
        let material = |layers| {
            let filter = PointwiseFilter::<()>::new(PointwiseKind::Synthesis).unwrap();
            let mut texture = filter.synthesis_edit().unwrap();
            for _ in 0..layers {
                texture.add_layer(SynthesisLayerKind::Solid);
            }
            filter.edit_synthesis(texture, "Set material").unwrap();
            filter
        };
        let departing = material(2);
        let arriving = material(1);
        let group = PointwiseFilter::new(PointwiseKind::Group).unwrap();
        let slot = group.node().content(0).unwrap();
        slot.set_node(Some(&departing.node())).unwrap();
        let mut document =
            Gtx375GraphDocument::new(vec![Some(group.node()), Some(arriving.node())]).unwrap();
        document
            .set_group_catch_adjusters(&group, true, Some("Surfaces"))
            .unwrap();
        for filter in [&departing, &arriving] {
            document
                .set_synthesis_adjuster(filter, SynthesisAdjusterLink::Create("Material".into()))
                .unwrap();
        }
        let original_group = document.synthesis_adjuster(&departing).unwrap();
        let original_root = document.synthesis_adjuster(&arriving).unwrap();
        let history = document
            .bind_property_history(Arc::new(TestHistory))
            .unwrap();
        history.set_recording(true);

        document.edit_slot(&slot, Some(&arriving.node())).unwrap();
        let revived = document.synthesis_adjuster(&arriving).unwrap();
        let layers =
            || [&departing, &arriving].map(|filter| filter.synthesis_edit().unwrap().layers());
        assert_eq!(layers(), [2, 2]);
        let mut changed = arriving.synthesis_edit().unwrap();
        changed.add_layer(SynthesisLayerKind::Solid);
        arriving
            .edit_synthesis(changed, "Edit replacement material")
            .unwrap();
        assert_eq!(layers(), [2, 3]);
        assert!(!revived.same_reference(&original_group));
        assert!(!revived.same_reference(&original_root));
        assert!(document.synthesis_adjuster(&departing).is_none());
        assert_eq!(document.catalog().unwrap().len(), 1);
        assert_eq!(history.commands().len(), 2);

        history.undo().unwrap();
        assert_eq!(layers(), [2, 2]);
        assert!(slot.node().unwrap().same_reference(&arriving.node()));
        history.undo().unwrap();
        assert_eq!(layers(), [2, 1]);
        assert!(slot.node().unwrap().same_reference(&departing.node()));
        assert!(
            document
                .branch(1)
                .flatten()
                .unwrap()
                .same_reference(&arriving.node())
        );
        for (filter, expected) in [(&departing, &original_group), (&arriving, &original_root)] {
            assert!(
                document
                    .synthesis_adjuster(filter)
                    .unwrap()
                    .same_reference(expected)
            );
        }
        assert_eq!(document.catalog().unwrap().len(), 2);

        history.redo().unwrap();
        assert_eq!(layers(), [2, 2]);
        assert!(slot.node().unwrap().same_reference(&arriving.node()));
        assert!(document.branch(1).unwrap().is_none());
        assert!(
            document
                .synthesis_adjuster(&arriving)
                .unwrap()
                .same_reference(&revived)
        );
        assert_eq!(document.catalog().unwrap().len(), 1);
        history.redo().unwrap();
        assert_eq!(layers(), [2, 3]);
    }

    #[test]
    fn stale_receiver_rolls_back_previous_synthesis_link_edits() {
        let first = PointwiseFilter::<()>::new(crate::PointwiseKind::Synthesis).unwrap();
        let second = PointwiseFilter::new(crate::PointwiseKind::Synthesis).unwrap();
        let destination = SynthesisCatalogValue::new(SynthesisRecord::default());
        let stage = |receiver: &PointwiseFilter<()>| {
            let before = receiver.synthesis_link_state().unwrap();
            let after = SynthesisLinkState {
                adjustment: Some(
                    receiver
                        .synthesis_wire()
                        .unwrap()
                        .new_reference_adjustment(10, &[u16::from(b'M')], [0; 16])
                        .unwrap(),
                ),
                catalog: Some(destination.clone()),
                local: before.local.clone(),
            };
            SynthesisReceiverEdit::new(receiver, before, after)
        };
        let edits = [stage(&first), stage(&second)];
        let mut texture = second.synthesis_edit().unwrap();
        texture.add_layer(crate::SynthesisLayerKind::Solid);
        second
            .edit_synthesis(texture, "Change second texture")
            .unwrap();
        assert!(matches!(
            SynthesisReceiverEdit::publish_batch(&edits, false),
            Err(HistoryError::InputChanged),
        ));
        assert!(!first.is_synthesis_adjusted());
        assert!(first.synthesis_catalog().is_none());
        assert!(!second.is_synthesis_adjusted());
        assert_eq!(second.synthesis_edit().unwrap().layers(), 1);
    }

    #[test]
    fn stale_material_prevents_other_lab_values_from_publishing() {
        let first = SynthesisCatalogValue::new(SynthesisRecord::default());
        let second = SynthesisCatalogValue::new(SynthesisRecord::default());
        let batch = SynthesisAdjusterBatch::<()>::prepare(
            &[],
            &GraphCatalog::default(),
            &mut EnvironmentAdjusterBatch::empty(),
            vec![
                (first.clone(), SynthesisRecord::default()),
                (second.clone(), SynthesisRecord::default()),
            ],
            &[],
            None,
            PatchShapeInputs::Authoring(&[]),
        )
        .unwrap();
        let original = first.record();
        let changed = SynthesisRecord::default();
        *second.lock() = changed.texture().clone();

        let result: Result<((), _), _> = batch.publish(HistoryDirection::Redo, || {
            panic!("other Lab values must wait for Synthesis validation")
        });

        assert!(matches!(result, Err(HistoryError::InputChanged)));
        assert!(Arc::ptr_eq(first.record().texture(), original.texture()));
        assert!(Arc::ptr_eq(second.record().texture(), changed.texture()));
    }
}
