//! Imported mn receivers use actual aoi body IDs inside a merged sponsor, just
//! as graph backedges use IDs. Internal bodies/containers never own this region.
use super::Body;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
static MERGE: Mutex<()> = Mutex::new(());

pub(super) struct Region<E> {
    rank: u64,
    state: Mutex<State<E>>,
}
enum State<E> {
    Active(BTreeMap<u64, Arc<Body<E>>>),
    Redirect(Arc<Region<E>>),
}
fn guard<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|error| error.into_inner())
}
fn canonical<E>(region: &Arc<Region<E>>) -> Arc<Region<E>> {
    let mut current = region.clone();
    loop {
        let next = match &*guard(&current.state) {
            State::Active(_) => None,
            State::Redirect(next) => Some(next.clone()),
        };
        match next {
            Some(next) => current = next,
            None => return current,
        }
    }
}
impl<E> Region<E> {
    pub(super) fn publish(body: &Arc<Body<E>>) -> Arc<Self> {
        let region = Arc::new(Self {
            rank: body.id,
            state: Mutex::new(State::Active(BTreeMap::from([(body.id, body.clone())]))),
        });
        *guard(&body.imported_route) = Arc::downgrade(&region);
        region
    }
    pub(super) fn for_body(body: &Arc<Body<E>>) -> Option<Arc<Self>> {
        let _coordinate = guard(&MERGE);
        let region = canonical(&guard(&body.imported_route).upgrade()?);
        let valid = match &*guard(&region.state) {
            State::Active(bodies) => bodies
                .get(&body.id)
                .is_some_and(|actual| Arc::ptr_eq(actual, body)),
            State::Redirect(_) => false,
        };
        valid.then_some(region)
    }
    pub(super) fn body(region: &Arc<Self>, id: u64) -> Arc<Body<E>> {
        let _coordinate = guard(&MERGE);
        let region = canonical(region);
        let state = guard(&region.state);
        match &*state {
            State::Active(bodies) => bodies
                .get(&id)
                .expect("retained imported dimensions receiver")
                .clone(),
            State::Redirect(_) => unreachable!(),
        }
    }
    pub(super) fn merge(a: &Arc<Self>, b: &Arc<Self>) {
        let _coordinate = guard(&MERGE);
        let a = canonical(a);
        let b = canonical(b);
        if Arc::ptr_eq(&a, &b) {
            return;
        }
        let (keep, old) = if a.rank < b.rank { (a, b) } else { (b, a) };
        let moved = match std::mem::replace(&mut *guard(&old.state), State::Redirect(keep.clone()))
        {
            State::Active(bodies) => bodies,
            State::Redirect(_) => unreachable!(),
        };
        let mut state = guard(&keep.state);
        if let State::Active(bodies) = &mut *state {
            for body in moved.values() {
                *guard(&body.imported_route) = Arc::downgrade(&keep);
            }
            bodies.extend(moved);
        }
    }
}
