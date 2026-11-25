//! Concrete Invert ownership region. Internal parent coordinates are weak;
//! outward node/property handles retain the complete connected owner region.
use crate::selection::NodeCore;
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};
static NEXT: AtomicU64 = AtomicU64::new(1);
static MERGE: Mutex<()> = Mutex::new(());
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct NodeId(u64);
impl NodeId {
    pub(crate) fn new() -> Self {
        Self(
            NEXT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| v.checked_add(1))
                .expect("selection ownership identity exhausted"),
        )
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
pub(crate) struct Sponsor<E> {
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
    pub(crate) fn publish(node: &Arc<NodeCore<E>>) -> Arc<Self> {
        let region = Arc::new(Region {
            rank: node.id,
            state: Mutex::new(RegionState::Active(BTreeMap::from([(
                node.id,
                Arc::clone(node),
            )]))),
        });
        let sponsor = Arc::new(Self {
            root: OnceLock::new(),
        });
        assert!(sponsor.root.set(Arc::clone(&region)).is_ok());
        sponsor
    }
    pub(crate) fn node(&self, id: NodeId) -> Arc<NodeCore<E>> {
        let _coord = MERGE.lock().unwrap_or_else(|e| e.into_inner());
        let region = canonical(self.root.get().expect("published selection sponsor"));
        let state = region.state.lock().unwrap_or_else(|e| e.into_inner());
        match &*state {
            RegionState::Active(nodes) => Arc::clone(
                nodes
                    .get(&id)
                    .expect("selection parent is a retained actual node"),
            ),
            RegionState::Redirect(_) => unreachable!(),
        }
    }
    pub(crate) fn merge(&self, other: &Self) {
        let _coord = MERGE.lock().unwrap_or_else(|e| e.into_inner());
        let a = canonical(self.root.get().expect("published selection sponsor"));
        let b = canonical(other.root.get().expect("published selection sponsor"));
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
            nodes.extend(moved);
        }
    }
}
