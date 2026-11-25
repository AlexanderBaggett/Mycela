//! Concrete live pointwise graph ownership region. Internal node and parent coordinates are IDs;
//! outward node/slot/property handles retain the complete connected owner region.
use super::graph::NodeCore;
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicU64, Ordering},
    },
};
static NEXT: AtomicU64 = AtomicU64::new(1);
static MERGE: Mutex<()> = Mutex::new(());
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(super) struct NodeId(u64);
impl NodeId {
    pub(super) fn get(self) -> u64 {
        self.0
    }

    pub(super) fn new() -> Self {
        Self(
            NEXT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| v.checked_add(1))
                .expect("pointwise graph ownership identity exhausted"),
        )
    }
}
// Cores retain a weak route to their CURRENT merged region. Dispatch uses a
// real outward edit frame to keep that region alive, then takes a strong
// sponsor for the callback. A weak pointer to the original Sponsor would fail
// after its original outward handle was dropped following a merge.
pub(super) struct NodeRoute<E> {
    owner: Mutex<Weak<Region<E>>>,
}
impl<E> NodeRoute<E> {
    pub(super) fn new() -> Self {
        Self {
            owner: Mutex::new(Weak::new()),
        }
    }
}
struct Region<E> {
    rank: NodeId,
    state: Mutex<RegionState<E>>,
}
enum RegionState<E> {
    Active(BTreeMap<NodeId, Arc<NodeCore<E>>>),
    Redirect(Arc<Region<E>>),
}
pub(super) struct Sponsor<E> {
    root: OnceLock<Arc<Region<E>>>,
}
fn canonical<E>(root: &Arc<Region<E>>) -> Arc<Region<E>> {
    let mut current = Arc::clone(root);
    loop {
        let next = {
            let state = current.state.lock().unwrap_or_else(|e| e.into_inner());
            match &*state {
                RegionState::Active(_) => None,
                RegionState::Redirect(next) => Some(Arc::clone(next)),
            }
        };
        match next {
            Some(next) => current = next,
            None => return current,
        }
    }
}
impl<E> Sponsor<E> {
    pub(super) fn publish(node: &Arc<NodeCore<E>>) -> Arc<Self> {
        let region = Arc::new(Region {
            rank: node.id,
            state: Mutex::new(RegionState::Active(BTreeMap::from([(
                node.id,
                Arc::clone(node),
            )]))),
        });
        *node.route.owner.lock().unwrap_or_else(|e| e.into_inner()) = Arc::downgrade(&region);
        let sponsor = Arc::new(Self {
            root: OnceLock::new(),
        });
        assert!(sponsor.root.set(Arc::clone(&region)).is_ok());
        sponsor
    }
    pub(super) fn for_node(node: &Arc<NodeCore<E>>) -> Option<Arc<Self>> {
        let _coord = MERGE.lock().unwrap_or_else(|e| e.into_inner());
        let owner = node
            .route
            .owner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .upgrade()?;
        let region = canonical(&owner);
        {
            let state = region.state.lock().unwrap_or_else(|e| e.into_inner());
            match &*state {
                RegionState::Active(nodes)
                    if nodes
                        .get(&node.id)
                        .is_some_and(|actual| Arc::ptr_eq(actual, node)) => {}
                _ => return None,
            }
        }
        let sponsor = Arc::new(Self {
            root: OnceLock::new(),
        });
        assert!(sponsor.root.set(region).is_ok());
        Some(sponsor)
    }
    pub(super) fn node(&self, id: NodeId) -> Arc<NodeCore<E>> {
        let _coord = MERGE.lock().unwrap_or_else(|e| e.into_inner());
        let region = canonical(self.root.get().expect("published pointwise graph sponsor"));
        let state = region.state.lock().unwrap_or_else(|e| e.into_inner());
        match &*state {
            RegionState::Active(nodes) => Arc::clone(
                nodes
                    .get(&id)
                    .expect("pointwise graph member is a retained actual node"),
            ),
            RegionState::Redirect(_) => unreachable!(),
        }
    }
    // Snapshot only owned cores; no source getters/user callbacks under locks.
    pub(super) fn nodes(&self) -> Vec<Arc<NodeCore<E>>> {
        let _coord = MERGE.lock().unwrap_or_else(|e| e.into_inner());
        let region = canonical(self.root.get().expect("published pointwise graph sponsor"));
        let state = region.state.lock().unwrap_or_else(|e| e.into_inner());
        match &*state {
            RegionState::Active(nodes) => nodes.values().cloned().collect(),
            RegionState::Redirect(_) => unreachable!(),
        }
    }
    pub(super) fn merge(&self, other: &Self) {
        let _coord = MERGE.lock().unwrap_or_else(|e| e.into_inner());
        let a = canonical(self.root.get().expect("published pointwise graph sponsor"));
        let b = canonical(other.root.get().expect("published pointwise graph sponsor"));
        if Arc::ptr_eq(&a, &b) {
            return;
        }
        let (keep, move_from) = if a.rank < b.rank { (a, b) } else { (b, a) };
        let moved = {
            let mut state = move_from.state.lock().unwrap_or_else(|e| e.into_inner());
            match std::mem::replace(&mut *state, RegionState::Redirect(Arc::clone(&keep))) {
                RegionState::Active(nodes) => nodes,
                RegionState::Redirect(_) => unreachable!(),
            }
        };
        let mut state = keep.state.lock().unwrap_or_else(|e| e.into_inner());
        if let RegionState::Active(nodes) = &mut *state {
            for node in moved.values() {
                *node.route.owner.lock().unwrap_or_else(|e| e.into_inner()) = Arc::downgrade(&keep);
            }
            nodes.extend(moved);
        }
    }
}
