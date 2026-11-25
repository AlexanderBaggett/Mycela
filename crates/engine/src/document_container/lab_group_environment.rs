//! Preserve the Lab scope of fields contained in Environment snapshots.
use super::*;
use crate::document_container::graph::numeric_adjusters::map_environment_number;
use crate::environment_record::EnvironmentLabReference;
use crate::{EnvironmentRecord, GradientError, SharedGradientRecord};

impl<E: 'static> ControlMigration<E> {
    pub(super) fn record_environment_links(
        &mut self,
        catalog: &GraphCatalog,
        before: &[Option<PointwiseNode<E>>],
    ) {
        for (entry, _) in catalog.entries() {
            if let GtxCatalogRecord::Environment { value, .. } = entry {
                visit_environment_links(&value.value(), |key| {
                    if let Some((_, marker)) = find_control(catalog, key) {
                        self.leave(marker);
                    }
                });
            }
        }
        for node in graph_nodes(before) {
            LabReferenceEdit::visit_node_links(&node, |key| {
                if let Some((_, marker)) = find_control(catalog, key) {
                    self.leave(marker);
                }
            });
            let Some(filter) = node.filter() else {
                continue;
            };
            let mut scan = |map: &EnvironmentRecord| {
                visit_environment_links(map, |key| {
                    if let Some((_, marker)) = find_control(catalog, key) {
                        self.leave(marker);
                    }
                });
            };
            for property in filter.retained_parameters() {
                if let PointwiseParameterValue::Environment(map) = property.raw_value() {
                    scan(&map);
                }
            }
            filter.visit_environment_paints(|map, _| scan(map));
        }
    }

    pub(super) fn keep_surviving_environment_links(&mut self, catalog: &GraphCatalog) {
        for (entry, marker) in catalog.entries() {
            if let GtxCatalogRecord::Environment { value, .. } = entry
                && (self.kept.contains(&marker_id(marker))
                    || !self
                        .moved
                        .iter()
                        .any(|retired| Arc::ptr_eq(retired, marker)))
            {
                self.keep_map_links(catalog, &value.value());
            }
        }
    }

    fn keep_map_links(&mut self, catalog: &GraphCatalog, map: &EnvironmentRecord) {
        visit_environment_links(map, |key| {
            if let Some((_, marker)) = find_control(catalog, key) {
                self.kept.insert(marker_id(marker));
            }
        });
    }

    pub(super) fn prepare_local_environment_maps(
        &mut self,
        catalog: &GraphCatalog,
        node: &PointwiseNode<E>,
        section: Option<[u8; 16]>,
        batch: &mut EnvironmentAdjusterBatch<E>,
    ) -> Result<(), GtxGraphError<E>> {
        let Some(owner) = node.filter() else {
            return Ok(());
        };
        for property in owner.retained_parameters() {
            let PointwiseParameterValue::Environment(before) = property.raw_value() else {
                continue;
            };
            if environment_adjustment_key(&property).is_some_and(|(group, name)| {
                attached_environment(catalog, (group, &name)).is_some()
            }) {
                continue;
            }
            if let Some(after) = self.relocate_environment_map(catalog, &before, section)? {
                batch.stage_local(&property, before, after);
            }
        }
        Ok(())
    }

    pub(super) fn relocate_environment_map(
        &mut self,
        catalog: &GraphCatalog,
        map: &EnvironmentRecord,
        section: Option<[u8; 16]>,
    ) -> Result<Option<Arc<EnvironmentRecord>>, GtxGraphError<E>> {
        let Some(group) = section else {
            self.keep_map_links(catalog, map);
            return Ok(None);
        };
        let mut numeric_fields = Vec::new();
        for field in map.numeric_receivers() {
            let Some((source_group, name)) = map.numeric_adjuster_key(field) else {
                continue;
            };
            let Some((order, (source, marker))) = catalog
                .entries()
                .enumerate()
                .find(|(_, (entry, _))| matches(entry.metadata(), source_group, name, 0))
            else {
                continue;
            };
            if source_group == group {
                self.kept.insert(marker_id(marker));
            } else {
                numeric_fields.push((order, field, source, marker));
            }
        }
        // Copy controls in source catalog order. Filter and radius side
        // effects also follow the resulting destination catalog's order.
        numeric_fields.sort_by_key(|(order, ..)| *order);
        let mut mapped = Vec::new();
        for (_, field, source, marker) in numeric_fields {
            self.destination(catalog, source, marker, group)?;
            if let Some(adjustment) = field.scope_adjustment(map, group) {
                mapped.push((field, adjustment));
            }
        }
        let mut after = (!mapped.is_empty()).then(|| map.clone());
        if let Some(after) = &mut after {
            let updates: Vec<_> = catalog
                .entries()
                .map(|(entry, _)| entry)
                .chain(&self.staged)
                .filter_map(|entry| match entry {
                    GtxCatalogRecord::Numeric { metadata, value } if metadata.group_id == group => {
                        NumericCatalogUpdate::reconnect(metadata, value)
                    }
                    _ => None,
                })
                .collect();
            let mut values = Vec::new();
            for (field, adjustment) in mapped {
                let sample =
                    map_environment_number::<E>(&updates, &adjustment).map_err(node_error)?;
                field.set_adjustment(after, Some(adjustment));
                if let Some((order, value)) = sample {
                    values.push((order, field, value));
                }
            }
            values.sort_by_key(|(order, ..)| *order);
            for (_, field, value) in values {
                field.apply(after, value).map_err(node_error)?;
            }
        }
        for field in map.color_receivers() {
            let Some((source_group, name)) = map.color_adjuster_key(field) else {
                continue;
            };
            let Some((source, marker)) =
                find_control(catalog, (source_group, field.type_tag(), name))
            else {
                continue;
            };
            if source_group == group {
                self.kept.insert(marker_id(marker));
                continue;
            }
            let destination = self.destination(catalog, source, marker, group)?;
            if let Some(value) = destination.color_or_gradient() {
                let value = match value {
                    EnvironmentLabReference::Gradient(gradient) => {
                        EnvironmentLabReference::Gradient(
                            copy_gradient(&gradient).map_err(gradient_error)?,
                        )
                    }
                    color => color,
                };
                after
                    .get_or_insert_with(|| map.clone())
                    .set_color_adjuster(field, Some((group, name, value)))
                    .map_err(gradient_error)?;
            }
        }
        Ok(after.map(Arc::new))
    }
}

pub(super) type LabKey<'a> = ([u8; 16], i32, &'a [u16]);

fn visit_environment_links(map: &EnvironmentRecord, mut visit: impl FnMut(LabKey<'_>)) {
    for field in map.numeric_receivers() {
        if let Some((group, name)) = map.numeric_adjuster_key(field) {
            visit((group, 0, name));
        }
    }
    for field in map.color_receivers() {
        if let Some((group, name)) = map.color_adjuster_key(field) {
            visit((group, field.type_tag(), name));
        }
    }
}

pub(super) fn find_control<'a>(
    catalog: &'a GraphCatalog,
    (group, tag, name): LabKey<'_>,
) -> Option<(&'a GtxCatalogRecord, &'a Arc<AtomicBool>)> {
    catalog
        .entries()
        .find(|(entry, _)| matches(entry.metadata(), group, name, tag))
}

pub(super) fn copy_gradient(
    gradient: &SharedGradientRecord,
) -> Result<SharedGradientRecord, GradientError> {
    Ok(SharedGradientRecord::new(gradient.live().wire_record()?))
}

pub(super) fn gradient_error<E>(error: GradientError) -> GtxGraphError<E> {
    node_error(PointwiseError::Gradient(error))
}
