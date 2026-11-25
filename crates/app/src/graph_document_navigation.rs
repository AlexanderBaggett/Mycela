use super::model::{LiveDocument, Node, guard};
use crate::ForegroundError;
use gpui_kit::component::tree::TreeItem;
use mycela_engine::PointwiseSlot;
use std::collections::HashSet;

pub(super) fn editable_contents(node: &Node) -> Vec<PointwiseSlot<ForegroundError>> {
    if matches!(
        node.kind(),
        Some(
            mycela_engine::PointwiseKind::TemplateLab(_)
                | mycela_engine::PointwiseKind::WizardReference
                | mycela_engine::PointwiseKind::Canvas
        )
    ) {
        // The specialized editor owns these inner structures.
        Vec::new()
    } else {
        node.contents()
    }
}

#[derive(Clone)]
pub(super) enum Target {
    Branch(usize, Option<Node>),
    Slot(PointwiseSlot<ForegroundError>),
}
impl Target {
    pub fn node(&self) -> Option<Node> {
        match self {
            Self::Branch(_, node) => node.clone(),
            Self::Slot(slot) => slot.node().filter(|node| !node.is_default_input()),
        }
    }
}
pub(super) struct Row {
    pub key: String,
    pub label: String,
    pub target: Target,
}
#[derive(Default)]
pub(super) struct Navigation {
    pub rows: Vec<Row>,
    pub branch_positions: Vec<[f32; 2]>,
    branches: Vec<u64>,
    slots: Vec<(PointwiseSlot<ForegroundError>, u64)>,
    next: u64,
}
impl Navigation {
    fn serial(&mut self) -> u64 {
        self.next += 1;
        self.next
    }
    fn children(
        &mut self,
        node: &Node,
        parent: &str,
        seen: &mut Vec<Node>,
        reachable: &mut HashSet<u64>,
    ) -> Vec<TreeItem> {
        if seen.iter().any(|n| n.same_reference(node)) {
            return Vec::new();
        }
        seen.push(node.clone());
        let mut items = Vec::new();
        let contents = editable_contents(node);
        let multiple_contents = contents.len() > 1;
        let mut inputs = node.slots();
        if matches!(
            node.kind(),
            Some(
                mycela_engine::PointwiseKind::TemplateLab(_)
                    | mycela_engine::PointwiseKind::WizardReference
                    | mycela_engine::PointwiseKind::Canvas
            )
        ) && let Ok(active) = node.reference_inputs()
        {
            let mut index = 0;
            inputs.retain(|slot| {
                let visible = index < active.len() || slot.node().is_some();
                index += 1;
                visible
            });
        }
        let occurrences = inputs.into_iter().map(|slot| (slot, None)).chain(
            contents
                .into_iter()
                .enumerate()
                .map(|(index, slot)| (slot, Some(index))),
        );
        for (slot, contents_index) in occurrences {
            let id = if let Some((_, id)) =
                self.slots.iter().find(|(old, _)| old.same_reference(&slot))
            {
                *id
            } else {
                let id = self.serial();
                self.slots.push((slot.clone(), id));
                id
            };
            reachable.insert(id);
            let key = format!("{parent}/slot-{id}");
            let child = slot.node();
            let name = match contents_index {
                Some(index) if multiple_contents => format!("Contents {}", index + 1),
                Some(_) => "Contents".to_string(),
                None => String::from_utf16_lossy(&slot.name()),
            };
            let label = format!(
                "{name}: {}",
                child
                    .as_ref()
                    .map(|node| if node.is_default_input() {
                        "Default preview input".into()
                    } else {
                        String::from_utf16_lossy(&node.identity_name())
                    })
                    .unwrap_or_else(|| if contents_index.is_some() {
                        "Empty".into()
                    } else {
                        "No input".into()
                    })
            );
            let children = child
                .as_ref()
                .map(|n| self.children(n, &key, seen, reachable))
                .unwrap_or_default();
            self.rows.push(Row {
                key: key.clone(),
                label: label.clone(),
                target: Target::Slot(slot),
            });
            items.push(TreeItem::new(key, label).expanded(true).children(children));
        }
        seen.pop();
        items
    }
    pub fn rebuild(&mut self, document: &LiveDocument) -> Vec<TreeItem> {
        let (roots, positions) = {
            let document = guard(document);
            let roots = document
                .branches()
                .into_owned()
                .into_iter()
                .enumerate()
                .filter_map(|(index, node)| Some((document.branch_occurrence_id(index)?, node)))
                .collect::<Vec<_>>();
            (roots, document.positions().into_owned())
        };
        self.branch_positions = positions;
        self.rows.clear();
        let capacity = roots
            .iter()
            .map(|(occurrence, _)| occurrence + 1)
            .max()
            .unwrap_or(0);
        while self.branches.len() < capacity {
            let id = self.serial();
            self.branches.push(id);
        }
        let mut items = Vec::new();
        let mut reachable = HashSet::new();
        for (ix, (occurrence, node)) in roots.into_iter().enumerate() {
            let key = format!("branch-{}", self.branches[occurrence]);
            let label = format!(
                "Branch {}: {}",
                ix + 1,
                node.as_ref()
                    .map(|n| String::from_utf16_lossy(&n.identity_name()))
                    .unwrap_or_else(|| "Empty".into())
            );
            let children = node
                .as_ref()
                .map(|n| self.children(n, &key, &mut Vec::new(), &mut reachable))
                .unwrap_or_default();
            self.rows.push(Row {
                key: key.clone(),
                label: label.clone(),
                target: Target::Branch(ix, node),
            });
            items.push(TreeItem::new(key, label).expanded(true).children(children));
        }
        // Only current occurrences sponsor their graph regions. Retired rows
        // have been replaced; callers holding an old qi retain it independently.
        self.slots.retain(|(_, id)| reachable.contains(id));
        items
    }
    pub fn target(&self, key: &str) -> Option<Target> {
        self.rows
            .iter()
            .find(|r| r.key == key)
            .map(|r| r.target.clone())
    }
    pub fn label(&self, key: &str) -> Option<&str> {
        self.rows
            .iter()
            .find(|r| r.key == key)
            .map(|r| r.label.as_str())
    }
    pub fn first(&self) -> Option<String> {
        self.rows
            .iter()
            .find(|r| matches!(r.target, Target::Branch(_, Some(_))))
            .or_else(|| self.rows.first())
            .map(|r| r.key.clone())
    }
}

#[cfg(test)]
mod retirement_laws {
    use super::*;
    use mycela_engine::{Gtx375GraphDocument, PointwiseNode, PointwiseNodeChangeHandler};
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    struct DropProbe(Arc<AtomicUsize>);
    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    fn slot_key(navigation: &Navigation, slot: &PointwiseSlot<ForegroundError>) -> String {
        navigation
            .rows
            .iter()
            .find_map(|row| match &row.target {
                Target::Slot(current) if current.same_reference(slot) => Some(row.key.clone()),
                _ => None,
            })
            .expect("reachable actual occurrence")
    }
    #[test]
    fn retired_navigation_releases_owner_after_external_held_slot_drops() {
        let owner = PointwiseNode::<ForegroundError>::invert();
        let held = owner.slot(0).unwrap();
        let drops = Arc::new(AtomicUsize::new(0));
        let probe = Arc::new(DropProbe(drops.clone()));
        let weak = Arc::downgrade(&probe);
        owner.add_change_handler(PointwiseNodeChangeHandler::new(move |_, _| {
            let _ = &probe;
            Ok(())
        }));
        let document = Arc::new(Mutex::new(
            Gtx375GraphDocument::new(vec![Some(owner.clone())]).unwrap(),
        ));
        let mut navigation = Navigation::default();
        drop(navigation.rebuild(&document));
        let old_key = slot_key(&navigation, &held);
        drop(navigation.rebuild(&document));
        assert_eq!(
            slot_key(&navigation, &held),
            old_key,
            "current occurrence keeps its domain ID"
        );
        let replacement = PointwiseNode::invert();
        let replacement_slot = replacement.slot(0).unwrap();
        guard(&document).set_branch(0, Some(replacement)).unwrap();
        drop(owner);
        drop(navigation.rebuild(&document));
        let new_key = slot_key(&navigation, &replacement_slot);
        assert_ne!(new_key, old_key, "retired domain IDs are never recycled");
        drop(navigation.rebuild(&document));
        assert_eq!(slot_key(&navigation, &replacement_slot), new_key);
        assert!(
            weak.upgrade().is_some(),
            "an explicitly held old occurrence still owns its actual owner"
        );
        assert!(
            held.node().is_none(),
            "the old occurrence remains valid after its branch is replaced"
        );
        drop(held);
        assert!(
            weak.upgrade().is_none(),
            "the navigation ledger must no longer sponsor the retired owner/handler"
        );
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }
}
