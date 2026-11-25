//! Refresh saved Lab values against the catalog projected by a scope change.
use super::environment_adjusters::catching_group;
use super::*;
use crate::{
    PointwiseFilter, PointwiseSlot, ReferenceAdjusterPayload, ReferenceValueHandle,
    ValueAdjusterPayload,
};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

type GroupId = [u8; 16];

pub(super) struct PresetRefresh {
    pub(super) added: Vec<Arc<AtomicBool>>,
    pub(super) retired: Vec<Arc<AtomicBool>>,
}

impl PresetRefresh {
    pub(super) fn stage<E: 'static>(
        document: &mut GtxPresetCollection,
        roots: &[Option<PointwiseNode<E>>],
        group: &PointwiseNode<E>,
        catch: bool,
        catalog: &[&GtxCatalogRecord],
    ) -> Self {
        let projection =
            LabProjection::read(roots, Some((group, catch)), None, PointwiseSlot::node);
        Self::stage_projected(document, &projection, catalog)
    }

    pub(super) fn stage_projected<E: 'static>(
        document: &mut GtxPresetCollection,
        projection: &LabProjection<E>,
        catalog: &[&GtxCatalogRecord],
    ) -> Self {
        let mut refresh = Self {
            added: Vec::new(),
            retired: Vec::new(),
        };
        let mut coverage = HashMap::from([(GroupId::default(), vec![vec![GroupId::default()]])]);
        for section in &projection.sections {
            let mut path = Vec::new();
            for id in section.path.iter().rev() {
                path.push(*id);
                coverage.entry(*id).or_default().push(path.clone());
            }
        }
        refresh.collection(document, catalog, &coverage[&GroupId::default()]);
        for section in &projection.sections {
            if let Some(collection) = section.group.group_presets() {
                refresh.collection(
                    &mut collection.lock().unwrap_or_else(|error| error.into_inner()),
                    catalog,
                    &coverage[section.path.last().expect("section GUID")],
                );
            }
        }
        refresh
    }

    fn collection(
        &mut self,
        collection: &mut GtxPresetCollection,
        catalog: &[&GtxCatalogRecord],
        passes: &[Vec<GroupId>],
    ) {
        if collection.groups().is_empty() {
            return;
        }
        let mut staged: Vec<_> = collection
            .groups()
            .iter()
            .filter(|group| group.presence().load(Ordering::Acquire))
            .map(|group| (group, group.presets().to_vec()))
            .collect();
        let mut removed = HashSet::new();
        for path in passes {
            let entries: Vec<_> = catalog
                .iter()
                .copied()
                .filter(|entry| {
                    let metadata = entry.metadata();
                    path.contains(&metadata.group_id)
                        && metadata.name.as_ref().is_some_and(|name| !name.is_empty())
                })
                .collect();
            for (index, (_, values)) in staged.iter_mut().enumerate() {
                if !removed.contains(&index) {
                    refresh_values(values, &entries);
                }
            }
            // sp.b walks the original collection order. When two visible groups
            // name the same preset, it appends the other group's dormant values.
            for index in 0..staged.len() {
                let (group, values) = &staged[index];
                if removed.contains(&index) || values.iter().all(GtxPresetRecord::is_hidden) {
                    continue;
                }
                if let Some(other) =
                    staged
                        .iter()
                        .enumerate()
                        .position(|(other, (candidate, values))| {
                            other != index
                                && !removed.contains(&other)
                                && group.name() == candidate.name()
                                && group.group_id() == candidate.group_id()
                                && values.iter().any(|value| !value.is_hidden())
                        })
                {
                    let dormant: Vec<_> = staged[other]
                        .1
                        .iter()
                        .cloned()
                        .map(|mut value| {
                            value.set_hidden(true);
                            value
                        })
                        .collect();
                    staged[index].1.extend(dormant);
                    removed.insert(other);
                }
            }
        }
        let staged: Vec<_> = staged
            .into_iter()
            .enumerate()
            .filter_map(|(index, (group, values))| {
                let retired = group.presence().clone();
                if removed.contains(&index) {
                    Some((retired, None))
                } else {
                    let changed = values.len() != group.presets().len()
                        || values
                            .iter()
                            .zip(group.presets())
                            .any(|(after, before)| after.is_hidden() != before.is_hidden())
                        || values.iter().all(GtxPresetRecord::is_hidden) != group.is_hidden();
                    changed.then(|| (retired, Some(group.refreshed(values))))
                }
            })
            .collect();
        for (retired, next) in staged {
            if let Some(next) = next {
                self.added.push(next.presence().clone());
                collection.stage_lab_snapshot(next, Some(&retired));
            }
            self.retired.push(retired);
        }
    }
}

fn refresh_values(values: &mut Vec<GtxPresetRecord>, entries: &[&GtxCatalogRecord]) {
    for value in values.iter_mut() {
        value.set_hidden(!entries.iter().any(|entry| matches(value, entry)));
    }
    for entry in entries {
        if !values.iter().any(|value| matches(value, entry))
            && let Some(value) = capture(entry)
        {
            values.push(value);
        }
    }
}

fn matches(value: &GtxPresetRecord, entry: &GtxCatalogRecord) -> bool {
    let metadata = entry.metadata();
    value.group_id() == metadata.group_id
        && value.type_tag() == metadata.type_tag
        && value.name() == metadata.name.as_deref()
}

pub(super) fn capture(entry: &GtxCatalogRecord) -> Option<GtxPresetRecord> {
    let metadata = entry.metadata();
    let name = metadata.name.as_ref().filter(|name| !name.is_empty())?;
    let kind = GtxPresetKind::from_tag(metadata.type_tag)?;
    let value = match entry {
        GtxCatalogRecord::Numeric { value, .. } => value.preset_value(kind)?,
        GtxCatalogRecord::Environment { value, .. } => {
            GtxPresetValue::Environment(Box::new((*value.value()).clone()))
        }
        GtxCatalogRecord::Synthesis { value, .. } => GtxPresetValue::Synthesis(value.record()),
        GtxCatalogRecord::Value(value) => match (&value.payload, kind) {
            (ValueAdjusterPayload::Color { value, .. }, GtxPresetKind::Color) => {
                GtxPresetValue::Color(*value)
            }
            (ValueAdjusterPayload::Vector(value), GtxPresetKind::Vector) => {
                GtxPresetValue::Vector(value.value)
            }
            _ => return None,
        },
        GtxCatalogRecord::Reference(value) => {
            let handle = match (&value.payload, kind) {
                (
                    ReferenceAdjusterPayload::Gradient {
                        current: Some(value),
                        ..
                    },
                    GtxPresetKind::Gradient,
                ) => ReferenceValueHandle::Gradient(crate::SharedGradientRecord::new(
                    value.live().wire_record().ok()?,
                )),
                (
                    ReferenceAdjusterPayload::Curve {
                        current: Some(value),
                        ..
                    },
                    GtxPresetKind::Curve,
                ) => ReferenceValueHandle::Curve(value.detached_copy()),
                _ => return None,
            };
            GtxPresetValue::Reference(handle)
        }
    };
    Some(GtxPresetRecord::new(kind, Some(name.clone()), metadata.group_id).with_value(value))
}

struct LabSection<E> {
    group: PointwiseFilter<E>,
    path: Vec<GroupId>,
}

pub(super) struct LabProjection<E> {
    pub(super) nodes: Vec<(PointwiseNode<E>, GroupId)>,
    sections: Vec<LabSection<E>>,
}

impl<E: 'static> LabProjection<E> {
    pub(super) fn read(
        roots: &[Option<PointwiseNode<E>>],
        toggle: Option<(&PointwiseNode<E>, bool)>,
        enclosure: Option<PointwiseNode<E>>,
        child: impl Fn(&PointwiseSlot<E>) -> Option<PointwiseNode<E>>,
    ) -> Self {
        let mut pending = Vec::new();
        let document = vec![GroupId::default()];
        if let Some(enclosure) = &enclosure {
            pending.push((enclosure.clone(), document, false));
        } else {
            pending.extend(
                roots
                    .iter()
                    .rev()
                    .flatten()
                    .cloned()
                    .map(|node| (node, document.clone(), false)),
            );
        }
        let mut projection = Self {
            nodes: Vec::new(),
            sections: Vec::new(),
        };
        let mut seen = HashSet::new();
        let mut visited = BTreeSet::new();
        while let Some((node, mut path, inputs_visited)) = pending.pop() {
            if !inputs_visited {
                if !visited.insert(node.graph_identity()) {
                    continue;
                }
                pending.push((node.clone(), path.clone(), true));
                pending.extend(
                    node.slots()
                        .iter()
                        .rev()
                        .filter_map(&child)
                        .map(|node| (node, path.clone(), false)),
                );
                continue;
            }
            projection
                .nodes
                .push((node.clone(), *path.last().expect("Lab section")));
            // Exterior slots and properties belong to the surrounding section.
            let section = match toggle {
                Some((target, catches)) if node.same_reference(target) => {
                    if catches {
                        node.filter()
                    } else {
                        None
                    }
                }
                _ => catching_group(&node),
            };
            if let Some(group) = section
                && let Some(id) = group.group_guid()
            {
                path.push(id);
                if seen.insert(id) {
                    projection.sections.push(LabSection {
                        group,
                        path: path.clone(),
                    });
                }
            }
            if enclosure
                .as_ref()
                .is_some_and(|enclosure| enclosure.same_reference(&node))
            {
                pending.extend(
                    roots
                        .iter()
                        .rev()
                        .flatten()
                        .cloned()
                        .map(|node| (node, path.clone(), false)),
                );
            } else {
                pending.extend(
                    node.contents()
                        .iter()
                        .rev()
                        .filter_map(&child)
                        .map(|node| (node, path.clone(), false)),
                );
            }
        }
        projection
    }
}
