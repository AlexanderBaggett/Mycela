//! Frozen operands and edges used to retain final images across export jobs.
use super::*;
use crate::pointwise::property::capture::{capture_values::Comparison, node_payload_values};
use std::sync::Arc;

pub(in crate::pointwise) struct FinalExportCacheTree<E> {
    root: Occurrence,
    nodes: BTreeMap<Occurrence, CacheNode<E>>,
}

impl<E: 'static> FinalExportCacheTree<E> {
    pub(in crate::pointwise) fn uses_environment_library(&self) -> bool {
        self.nodes.values().any(|node| {
            let Some(filter) = node.original.filter() else {
                return false;
            };
            match filter.kind() {
                PointwiseKind::MakeEnvironment => filter
                    .parameter(0)
                    .and_then(|parameter| parameter.effective_environment())
                    .is_ok_and(|value| value.uses_library_images()),
                PointwiseKind::Synthesis => filter
                    .synthesis_texture()
                    .is_some_and(|texture| texture.uses_library()),
                PointwiseKind::Style => filter
                    .style()
                    .is_some_and(|style| style.uses_environment_library()),
                PointwiseKind::Canvas => filter
                    .canvas_document()
                    .is_some_and(|canvas| canvas.uses_environment_library()),
                _ => false,
            }
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Occurrence {
    source: NodeId,
    ordinal: usize,
}

struct CacheNode<E> {
    original: PointwiseNode<E>,
    recipient: NodeId,
    inputs: CacheInputs,
}

#[derive(PartialEq, Eq)]
struct CacheInputs {
    property_values: Arc<[u64]>,
    slots: Vec<Option<Occurrence>>,
    contents: Vec<Option<Occurrence>>,
    variable_owners: Vec<(Occurrence, Arc<[u64]>)>,
    reference_targets: [Option<Occurrence>; 2],
    reference_needs_context: bool,
}

impl<E: 'static> PointwiseFinalGraph<E> {
    pub(in crate::pointwise) fn export_cache_tree(
        &self,
        origins: &BTreeMap<NodeId, NodeId>,
        time: f64,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<FinalExportCacheTree<E>, PointwiseError<E>> {
        let mut ids = BTreeMap::new();
        let mut counts = BTreeMap::<NodeId, usize>::new();
        let mut pending = vec![self.tree.clone()];
        while let Some(node) = pending.pop() {
            check_render_cancelled(cancelled)?;
            let original = &self.originals[&node.id];
            let source = *origins
                .get(&original.id)
                .ok_or(PointwiseError::InvalidOperation)?;
            let ordinal = counts.entry(source).or_default();
            ids.insert(
                node.id,
                Occurrence {
                    source,
                    ordinal: *ordinal,
                },
            );
            *ordinal += 1;
            pending.extend(
                node.slots()
                    .into_iter()
                    .chain(node.contents())
                    .rev()
                    .filter_map(|slot| slot.node()),
            );
        }
        let mut properties = BTreeMap::<_, Arc<[u64]>>::new();
        for &id in self.originals.keys() {
            check_render_cancelled(cancelled)?;
            let node = self.tree.at(id);
            // Fresh comparison per node means an unrelated branch's alias
            // numbering cannot change these operands.
            let (kind, values) = node_payload_values(&node, time, &mut Comparison::default())?;
            let mut words = vec![kind, u64::from(node.is_filter())];
            words.extend(values);
            properties.insert(id, words.into());
        }
        let mut nodes = BTreeMap::new();
        for (&id, original) in &self.originals {
            check_render_cancelled(cancelled)?;
            let node = self.tree.at(id);
            let (reference_targets, reference_needs_context) =
                if node.kind() == Some(PointwiseKind::Reference) {
                    // A Reference can read the saved slot or receive a prepared
                    // sample from its Group. Retain both dependencies and require
                    // an identical request when those routes differ or a selector
                    // could not be sampled.
                    let saved = node
                        .reference_target()
                        .map(|target| target.map(|target| ids[&target.id]));
                    let sampled = node
                        .reference_target_at(time)
                        .map(|target| target.map(|target| ids[&target.id]));
                    let needs_context = !matches!((&saved, &sampled), (Ok(a), Ok(b)) if a == b);
                    (
                        [saved.ok().flatten(), sampled.ok().flatten()],
                        needs_context,
                    )
                } else {
                    ([None; 2], false)
                };
            let mut variable_owners = Vec::new();
            if node.filter().is_some_and(|filter| {
                filter
                    .parameters()
                    .iter()
                    .any(|property| property.binding_index() >= 0)
            }) || node.image_generator_handle().is_some_and(|generator| {
                generator
                    .parameters()
                    .iter()
                    .any(|property| property.binding_index() >= 0)
            }) {
                let mut ancestor = node.parent();
                while let Some(parent) = ancestor {
                    check_render_cancelled(cancelled)?;
                    if matches!(
                        parent.kind(),
                        Some(PointwiseKind::MapVariable | PointwiseKind::WizardGroup)
                    ) {
                        variable_owners.push((ids[&parent.id], properties[&parent.id].clone()));
                    }
                    ancestor = parent.parent();
                }
            }
            let slots = node
                .slots()
                .iter()
                .map(|slot| slot.node().map(|child| ids[&child.id]))
                .collect();
            let contents = node
                .contents()
                .iter()
                .map(|slot| slot.node().map(|child| ids[&child.id]))
                .collect();
            nodes.insert(
                ids[&id],
                CacheNode {
                    original: original.clone(),
                    recipient: id,
                    inputs: CacheInputs {
                        property_values: properties[&id].clone(),
                        slots,
                        contents,
                        variable_owners,
                        reference_targets,
                        reference_needs_context,
                    },
                },
            );
        }
        Ok(FinalExportCacheTree {
            root: ids[&self.tree.id],
            nodes,
        })
    }
}

impl<E: 'static> FinalExportCacheTree<E> {
    pub(in crate::pointwise) fn same_root(&self, other: &Self) -> bool {
        self.root == other.root
    }

    pub(in crate::pointwise) fn inherit(
        &self,
        old: &Self,
        same_settings: bool,
        same_context: bool,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), PointwiseError<E>> {
        let mut invalid = BTreeSet::new();
        if same_settings {
            let mut consumers = BTreeMap::<Occurrence, Vec<Occurrence>>::new();
            let mut changed = Vec::new();
            for (&id, node) in &self.nodes {
                check_render_cancelled(cancelled)?;
                let input = &node.inputs;
                if (!same_context && input.reference_needs_context)
                    || old.nodes.get(&id).is_none_or(|old| old.inputs != *input)
                {
                    changed.push(id);
                }
                for &dependency in input
                    .slots
                    .iter()
                    .chain(&input.contents)
                    .chain(&input.reference_targets)
                    .flatten()
                {
                    consumers.entry(dependency).or_default().push(id);
                }
            }
            while let Some(id) = changed.pop() {
                check_render_cancelled(cancelled)?;
                if invalid.insert(id)
                    && let Some(dependents) = consumers.get(&id)
                {
                    changed.extend(dependents);
                }
            }
        }

        // Registering this job issued a new recipient. Read the previous
        // revision's images once while preparing the new detached owners.
        for (&id, node) in &self.nodes {
            check_render_cancelled(cancelled)?;
            let Some(old) = old.nodes.get(&id) else {
                continue;
            };
            let (enabled, image) = old.original.with_final_cache(|cache| {
                if cache.recipient != Some(old.recipient) {
                    return (false, None);
                }
                let enabled = cache.enabled && !cache.destruct_mode;
                let image = if enabled && same_settings && !invalid.contains(&id) {
                    cache.image.clone()
                } else {
                    None
                };
                (enabled, image)
            });
            let replaced = node.original.with_final_cache(|cache| {
                if cache.recipient != Some(node.recipient) {
                    return None;
                }
                cache.enabled = enabled;
                cache.destruct_mode = false;
                std::mem::replace(&mut cache.image, image)
            });
            drop(replaced);
        }
        Ok(())
    }
}
