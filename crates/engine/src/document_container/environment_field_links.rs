//! Author field links on ordinary and shared Environment maps.
use super::*;
use crate::environment_record::EnvironmentLabReference;
use crate::pointwise::reference_adjustment::LabReferenceEdit;
use crate::{
    EnvironmentColorField, GradientError, LabColorAdjuster, LabColorValue,
    ReferenceAdjusterPayload, SharedGradientRecord, StyleEdit, ValueAdjusterPayload,
};

#[path = "environment_numeric_links.rs"]
mod numeric;
pub use numeric::EnvironmentNumericMapping;

pub enum EnvironmentMapTarget<'a, E> {
    Property(&'a PointwiseParameter<E>),
    Paint {
        owner: &'a PointwiseNode<E>,
        layer: Option<&'a CanvasLayerEditor<E>>,
        pass: usize,
    },
}

#[derive(Clone)]
pub enum EnvironmentColorLink {
    Create(String),
    Existing(LabColorAdjuster),
    Unlink,
}

struct Target<'a, E> {
    owner: PointwiseNode<E>,
    map: Arc<EnvironmentRecord>,
    edit: MapEdit<'a, E>,
}

enum MapEdit<'a, E> {
    Shared(EnvironmentAdjuster),
    Property(&'a PointwiseParameter<E>),
    Paint {
        style: Box<StyleEdit>,
        layer: Option<&'a CanvasLayerEditor<E>>,
        pass: usize,
    },
}

impl<E: 'static> Target<'_, E> {
    fn group(&self) -> [u8; 16] {
        match &self.edit {
            MapEdit::Shared(adjuster) => adjuster.group_id,
            _ => environment_adjuster_scope(&self.owner),
        }
    }

    fn into_batch(
        self,
        document: &Gtx375GraphDocument<E>,
        replacement: EnvironmentRecord,
    ) -> Result<EnvironmentAdjusterBatch<E>, GtxGraphError<E>> {
        let mut batch = EnvironmentAdjusterBatch::empty();
        match self.edit {
            MapEdit::Shared(adjuster) => {
                batch = document
                    .stage_environment_adjuster_maps(vec![(adjuster, Arc::new(replacement))]);
                if !Arc::ptr_eq(&self.map, &batch.shared[0].before) {
                    return Err(stale_map());
                }
                batch.stage_receivers(&document.branches(), &document.catalog, &[])?;
            }
            MapEdit::Property(property) => batch.local.push(LocalEnvironmentEdit {
                property: property.downgrade(),
                before: self.map,
                after: Arc::new(replacement),
            }),
            MapEdit::Paint {
                mut style,
                layer,
                pass,
            } => {
                style
                    .replace_environment_map(pass, &self.map, replacement)
                    .map_err(|_| stale_map())?;
                let filter = self.owner.filter().ok_or_else(stale_map)?;
                batch.paints.push(
                    EnvironmentPaintEdit::from_style_edit(&filter, layer, *style)
                        .ok_or_else(stale_map)?,
                );
            }
        }
        Ok(batch)
    }
}

impl<E: 'static> Gtx375GraphDocument<E> {
    pub fn environment_color_adjuster(
        &self,
        map: &EnvironmentRecord,
        field: EnvironmentColorField,
    ) -> Option<LabColorAdjuster> {
        let (group, name) = map.color_adjuster_key(field)?;
        let (entry, marker) = self.color_entry((group, field.type_tag(), name))?;
        LabColorAdjuster::read(entry, marker)
    }

    /// Lists controls that match the field type in the map's Lab section.
    pub fn available_environment_color_adjusters(
        &self,
        target: EnvironmentMapTarget<'_, E>,
        field: EnvironmentColorField,
    ) -> Vec<LabColorAdjuster> {
        let Some(target) = self.environment_map_target(target) else {
            return Vec::new();
        };
        let group = target.group();
        let mut entries: Vec<_> = self
            .catalog
            .entries()
            .filter(|(entry, _)| {
                entry.metadata().group_id == group && entry.metadata().type_tag == field.type_tag()
            })
            .filter_map(|(entry, active)| {
                Some((entry.metadata(), LabColorAdjuster::read(entry, active)?))
            })
            .collect();
        entries.sort_by_cached_key(|(meta, control)| {
            super::super::lab_adjuster_metadata::display_order(
                meta.order,
                control.name(),
                meta.type_tag,
            )
        });
        entries.into_iter().map(|(_, control)| control).collect()
    }

    pub fn set_environment_color_adjuster(
        &mut self,
        target: EnvironmentMapTarget<'_, E>,
        expected: &Arc<EnvironmentRecord>,
        field: EnvironmentColorField,
        link: EnvironmentColorLink,
    ) -> Result<bool, GtxGraphError<E>> {
        let target = self.validate_environment_map_target(target, expected)?;
        let branches = self.branches();
        let value =
            target
                .map
                .color_adjuster_value(field)
                .ok_or(GtxGraphError::EnvironmentAdjuster(
                    "The selected layer field is unavailable.",
                ))?;
        let old = target.map.color_adjuster_key(field);
        let group = target.group();
        let tag = field.type_tag();
        let (created, destination, label) = match link {
            EnvironmentColorLink::Create(name) => {
                let name: Vec<_> = name.trim().encode_utf16().collect();
                if name.is_empty() {
                    return Err(GtxGraphError::EnvironmentAdjuster(
                        "Enter an adjuster name.",
                    ));
                }
                if self.color_entry((group, tag, &name)).is_some() {
                    return Err(GtxGraphError::EnvironmentAdjuster(
                        "An adjuster of this type already uses that name in this Lab section.",
                    ));
                }
                let order = self
                    .catalog
                    .entries()
                    .filter(|(entry, _)| entry.metadata().group_id == group)
                    .map(|(entry, _)| entry.metadata().order)
                    .max()
                    .map_or(0, |order| order.saturating_add(1));
                let entry =
                    create_entry(name.clone(), group, order, &value).map_err(gradient_error)?;
                (
                    Some(entry),
                    Some((name, value)),
                    "Create Environment Field Adjuster",
                )
            }
            EnvironmentColorLink::Existing(adjuster) => {
                let (entry, marker) = self
                    .catalog
                    .entries()
                    .find(|(entry, marker)| {
                        let meta = entry.metadata();
                        meta.group_id == group && meta.type_tag == tag && adjuster.matches(marker)
                    })
                    .ok_or(GtxGraphError::EnvironmentAdjuster(
                        "This adjuster is no longer available in this Lab section.",
                    ))?;
                let name = entry.metadata().name.clone().ok_or_else(stale_map)?;
                if old == Some((group, name.as_slice())) {
                    return Ok(false);
                }
                let value = LabColorAdjuster::read(entry, marker)
                    .ok_or_else(stale_map)?
                    .draft_value()
                    .map_err(gradient_error)?;
                let value = match value {
                    LabColorValue::Color(rgb) => EnvironmentLabReference::Color(rgb),
                    LabColorValue::Gradient(gradient) => {
                        EnvironmentLabReference::Gradient(SharedGradientRecord::from_live(gradient))
                    }
                };
                (None, Some((name, value)), "Link Environment Field Adjuster")
            }
            EnvironmentColorLink::Unlink if old.is_some() => {
                (None, None, "Unlink Environment Field Adjuster")
            }
            EnvironmentColorLink::Unlink => return Ok(false),
        };
        let retired = old.and_then(|(group, name)| {
            let key = (group, tag, name);
            let (_, marker) = self.color_entry(key)?;
            (!self.other_color_receiver_exists(&branches, key)).then(|| marker.clone())
        });
        drop(branches);
        let mut replacement = expected.as_ref().clone();
        replacement
            .set_color_adjuster(
                field,
                destination
                    .as_ref()
                    .map(|(name, value)| (group, name.as_slice(), value.clone())),
            )
            .map_err(gradient_error)?;
        let batch = target.into_batch(self, replacement)?;
        self.commit_environment_field_link(batch, created, retired.into_iter().collect(), label)
    }

    /// Commits the replacement if it disconnects the last receivers of active
    /// Lab controls. A false result leaves the map for the caller's ordinary
    /// edit path. The catalog and map travel together in history.
    pub fn edit_environment_removing_unused_adjusters(
        &mut self,
        owner: EnvironmentMapTarget<'_, E>,
        expected: &Arc<EnvironmentRecord>,
        replacement: &EnvironmentRecord,
    ) -> Result<bool, GtxGraphError<E>> {
        let mut branches = None;
        let retired: Vec<_> = self
            .catalog
            .entries()
            .filter_map(|(entry, marker)| {
                let meta = entry.metadata();
                let key = (meta.group_id, meta.type_tag, meta.name.as_deref()?);
                let numeric = matches!(key.1, 0 | 4);
                if !numeric && !matches!(key.1, 6 | 7) {
                    return None;
                }
                let count = |map: &EnvironmentRecord| {
                    if numeric {
                        map.count_numeric_adjuster(key.0, key.2)
                    } else {
                        map.count_color_adjuster(key)
                    }
                };
                let removed = count(expected);
                if removed == 0 || count(replacement) != 0 {
                    return None;
                }
                let branches = branches.get_or_insert_with(|| self.branches());
                let used_elsewhere = if numeric {
                    Self::graph_uses_numeric_adjuster(branches, meta)
                        || self.count_synthesis_number_links(branches, meta) > 0
                } else {
                    graph_nodes(branches)
                        .any(|node| LabReferenceEdit::node_uses_control(&node, key))
                };
                (!used_elsewhere && self.count_environment_links(branches, count) == removed)
                    .then(|| marker.clone())
            })
            .collect();
        if retired.is_empty() {
            return Ok(false);
        }
        let target = self.validate_environment_map_target(owner, expected)?;
        let batch = target.into_batch(self, replacement.clone())?;
        self.commit_environment_field_link(batch, None, retired, "Edit Environment map")
    }

    fn commit_environment_field_link(
        &mut self,
        batch: EnvironmentAdjusterBatch<E>,
        created: Option<GtxCatalogRecord>,
        retired: Vec<Arc<AtomicBool>>,
        label: &str,
    ) -> Result<bool, GtxGraphError<E>> {
        let edit = EnvironmentLinkEdit {
            target: EnvironmentLinkTarget::Map(batch),
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

    fn validate_environment_map_target<'a>(
        &self,
        owner: EnvironmentMapTarget<'a, E>,
        expected: &Arc<EnvironmentRecord>,
    ) -> Result<Target<'a, E>, GtxGraphError<E>> {
        self.environment_map_target(owner)
            .filter(|target| {
                Arc::ptr_eq(&target.map, expected)
                    && graph_nodes(&self.branches()).any(|node| node.same_reference(&target.owner))
            })
            .ok_or_else(stale_map)
    }

    fn environment_map_target<'a>(
        &self,
        target: EnvironmentMapTarget<'a, E>,
    ) -> Option<Target<'a, E>> {
        match target {
            EnvironmentMapTarget::Property(property) => {
                if property.environment_binding_source().ok()?.is_some() {
                    return None;
                }
                let owner = property.owner().node();
                if let Some(adjuster) = self.environment_adjuster(property) {
                    return Some(Target {
                        owner,
                        map: adjuster.value(),
                        edit: MapEdit::Shared(adjuster),
                    });
                }
                if environment_adjustment_key(property).is_some() {
                    return None;
                }
                let PointwiseParameterValue::Environment(map) = property.raw_value() else {
                    return None;
                };
                Some(Target {
                    owner,
                    map,
                    edit: MapEdit::Property(property),
                })
            }
            EnvironmentMapTarget::Paint { owner, layer, pass } => {
                let filter = owner.filter()?;
                let style = match layer {
                    Some(layer) if layer.belongs_to(&filter) => layer.style_edit(),
                    Some(_) => return None,
                    None => filter.style_edit(),
                }?;
                let map = style.environment_map(pass)?.clone();
                let edit = if let Some(adjuster) = self.style_environment_adjuster(&style, pass) {
                    if !Arc::ptr_eq(&map, &adjuster.value()) {
                        return None;
                    }
                    MapEdit::Shared(adjuster)
                } else {
                    if style.environment_adjuster_key(pass).is_some() {
                        return None;
                    }
                    MapEdit::Paint {
                        style: Box::new(style),
                        layer,
                        pass,
                    }
                };
                Some(Target {
                    owner: owner.clone(),
                    map,
                    edit,
                })
            }
        }
    }

    pub(in crate::document_container) fn color_entry(
        &self,
        (group, tag, name): ([u8; 16], i32, &[u16]),
    ) -> Option<(&GtxCatalogRecord, &Arc<AtomicBool>)> {
        self.catalog.entries().find(|(entry, _)| {
            let metadata = entry.metadata();
            metadata.group_id == group
                && metadata.type_tag == tag
                && metadata.name.as_deref() == Some(name)
        })
    }

    fn other_color_receiver_exists(
        &self,
        branches: &[Option<PointwiseNode<E>>],
        key: ([u8; 16], i32, &[u16]),
    ) -> bool {
        graph_nodes(branches).any(|node| LabReferenceEdit::node_uses_control(&node, key))
            || self.count_environment_links(branches, |map| map.count_color_adjuster(key)) > 1
            || self.count_synthesis_color_links(branches, key) > 0
    }

    pub(in crate::document_container::graph) fn count_environment_links(
        &self,
        branches: &[Option<PointwiseNode<E>>],
        matches: impl Fn(&EnvironmentRecord) -> usize,
    ) -> usize {
        // Count the nested properties on a shared map once.
        let mut shared = HashSet::new();
        let mut count = 0;
        for (entry, _) in self.catalog.entries() {
            if let GtxCatalogRecord::Environment { metadata, value } = entry
                && let Some(name) = metadata.name.as_deref()
            {
                shared.insert((metadata.group_id, name));
                count += matches(&value.value());
            }
        }
        for property in environment_properties(branches) {
            let existing = environment_adjustment_key(&property);
            if existing
                .as_ref()
                .is_some_and(|(g, n)| shared.contains(&(*g, n.as_slice())))
            {
                continue;
            }
            if let PointwiseParameterValue::Environment(map) = property.raw_value() {
                count += matches(&map);
            }
        }
        for filter in graph_nodes(branches).filter_map(|node| node.filter()) {
            filter.visit_environment_paints(|map, whole| {
                if whole.is_none_or(|whole| !shared.contains(&whole)) {
                    count += matches(map);
                }
            });
        }
        count
    }
}

fn create_entry(
    name: Vec<u16>,
    group: [u8; 16],
    order: i32,
    value: &EnvironmentLabReference,
) -> Result<GtxCatalogRecord, GradientError> {
    Ok(match value {
        EnvironmentLabReference::Color(rgb) => {
            let mut record = ValueAdjusterRecord::new(ValueAdjusterKind::Color, Some(name), group);
            record.metadata.order = order;
            record.payload = ValueAdjusterPayload::Color {
                value: *rgb,
                previous: *rgb,
            };
            GtxCatalogRecord::Value(Box::new(record))
        }
        EnvironmentLabReference::Gradient(gradient) => {
            let mut record =
                ReferenceAdjusterRecord::new(ReferenceAdjusterKind::Gradient, Some(name), group);
            record.metadata.order = order;
            let gradient = SharedGradientRecord::new(gradient.live().wire_record()?);
            record.payload = ReferenceAdjusterPayload::Gradient {
                current: Some(gradient.clone()),
                previous: Some(gradient),
            };
            GtxCatalogRecord::Reference(Box::new(record))
        }
    })
}

fn gradient_error<E>(error: GradientError) -> GtxGraphError<E> {
    GtxGraphError::Node(PointwiseError::Gradient(error).into())
}

fn stale_map<E>() -> GtxGraphError<E> {
    GtxGraphError::EnvironmentAdjuster(
        "The selected Environment map changed. Select its layer again.",
    )
}
