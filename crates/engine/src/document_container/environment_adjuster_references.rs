//! Catalog Color and Gradient inputs to the Lab transaction.
use super::{GraphCatalog, GtxCatalogRecord};
use crate::environment_record::EnvironmentLabReference;
use crate::{
    AdjustmentRecord, GradientError, ReferenceAdjusterPayload, SharedGradientRecord,
    ValueAdjusterPayload,
};
use std::collections::HashMap;

pub(in crate::document_container) struct MapReferenceValues<'a> {
    values: HashMap<([u8; 16], i32), NamedReferences<'a>>,
}

type NamedReferences<'a> = HashMap<&'a [u16], LoadedReference<'a>>;

enum LoadedReference<'a> {
    Color([f64; 3]),
    Gradient {
        source: &'a SharedGradientRecord,
        snapshot: Option<SharedGradientRecord>,
    },
}

impl<'a> MapReferenceValues<'a> {
    pub(in crate::document_container) fn read(catalog: &'a GraphCatalog) -> Option<Self> {
        Self::from_entries(catalog.entries().map(|(entry, _)| entry))
    }

    pub(in crate::document_container) fn from_entries(
        entries: impl IntoIterator<Item = &'a GtxCatalogRecord>,
    ) -> Option<Self> {
        let mut values: HashMap<_, NamedReferences<'_>> = HashMap::new();
        for entry in entries {
            let metadata = entry.metadata();
            let Some(name) = metadata.name.as_deref().filter(|name| !name.is_empty()) else {
                continue;
            };
            let value = match (metadata.type_tag, entry) {
                (6, GtxCatalogRecord::Value(record)) => match record.payload {
                    ValueAdjusterPayload::Color { value, .. } => LoadedReference::Color(value),
                    _ => continue,
                },
                (7, GtxCatalogRecord::Reference(record)) => match &record.payload {
                    ReferenceAdjusterPayload::Gradient {
                        current: Some(source),
                        ..
                    } => LoadedReference::Gradient {
                        source,
                        snapshot: None,
                    },
                    _ => continue,
                },
                _ => continue,
            };
            values
                .entry((metadata.group_id, metadata.type_tag))
                .or_default()
                .entry(name)
                .or_insert(value);
        }
        (!values.is_empty()).then_some(Self { values })
    }

    pub(in crate::document_container) fn color_value(
        &self,
        (group, tag, name): ([u8; 16], i32, &[u16]),
    ) -> Option<[f64; 3]> {
        match self.values.get(&(group, tag))?.get(name)? {
            LoadedReference::Color(value) => Some(*value),
            LoadedReference::Gradient { .. } => None,
        }
    }

    pub(in crate::document_container) fn graph_value(
        &self,
        (group, tag, name): ([u8; 16], i32, &[u16]),
    ) -> Option<EnvironmentLabReference> {
        match self.values.get(&(group, tag))?.get(name)? {
            LoadedReference::Color(value) => Some(EnvironmentLabReference::Color(*value)),
            LoadedReference::Gradient { source, .. } => {
                Some(EnvironmentLabReference::Gradient((*source).clone()))
            }
        }
    }

    pub(super) fn resolve(
        &mut self,
        adjustment: &AdjustmentRecord,
    ) -> Result<Option<EnvironmentLabReference>, GradientError> {
        let Some(component) = adjustment
            .components()
            .first()
            .filter(|component| component.active)
        else {
            return Ok(None);
        };
        let value = match self
            .values
            .get_mut(&(adjustment.guid, adjustment.property_type))
            .and_then(|values| values.get_mut(component.name.as_slice()))
        {
            Some(LoadedReference::Color(value)) => EnvironmentLabReference::Color(*value),
            Some(LoadedReference::Gradient { source, snapshot }) => {
                // Materialize once and share across all resolved receiver snapshots.
                // External wire handles can remain mutable without changing a captured map.
                if snapshot.is_none() {
                    *snapshot = Some(SharedGradientRecord::new(source.live().wire_record()?));
                }
                EnvironmentLabReference::Gradient(
                    snapshot
                        .as_ref()
                        .expect("initialized Gradient snapshot")
                        .clone(),
                )
            }
            None => return Ok(None),
        };
        Ok(Some(value))
    }
}
