//! Private ownership for actual Color node/input cores. Public binding and
//! binding operations remain unavailable; this is not a document/history host.
use crate::color_input_events::{ColorInputDelegate, ColorInputEventArgs, InputDelegateCore};
use crate::{ColorSwatchError, ColorSwatchValue, ContextColorValue, live_solid_color::Inner};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicU64, Ordering},
    },
};

static SERIAL: AtomicU64 = AtomicU64::new(1);
// Contains no context, member or callback. No effect runs while held.
static MERGE: Mutex<()> = Mutex::new(());
fn serial() -> u64 {
    SERIAL
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
        .expect("private Color ownership identity exhausted")
}
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum CoreKind {
    Node,
    Input,
    Binding,
}
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct CoreId {
    birth_owner: u64,
    local: u64,
    kind: CoreKind,
}
pub(crate) struct CoreRoute<E> {
    pub(crate) id: CoreId,
    owner: Mutex<Weak<Region<E>>>,
}
impl<E> CoreRoute<E> {
    pub(crate) fn new(kind: CoreKind) -> Self {
        Self {
            id: CoreId {
                birth_owner: serial(),
                local: 0,
                kind,
            },
            owner: Mutex::new(Weak::new()),
        }
    }
}
/// Stored rr copies never own a public APV lease. Scoped identity stays stable
/// as the region changes; equality remains the source component comparison.
#[derive(Clone, Copy, Debug)]
pub(crate) struct StoredColorValue {
    pub(crate) components: [f64; 3],
    pub(crate) apv: Option<CoreId>,
}
impl StoredColorValue {
    pub(crate) fn unlinked(components: [f64; 3]) -> Self {
        Self {
            components,
            apv: None,
        }
    }
}
pub(crate) struct InputCore<E> {
    pub(crate) route: CoreRoute<E>,
    state: Mutex<InputState>,
    pub(crate) input_changed: Mutex<InputDelegateCore<E>>,
}
struct InputState {
    copied: StoredColorValue,
    swatch: ColorSwatchValue,
}
pub(crate) enum Member<E> {
    Node(Arc<Inner<E>>),
    Input(Arc<InputCore<E>>),
    Binding(Arc<crate::color_binding::BindingCore<E>>),
}
impl<E> Clone for Member<E> {
    fn clone(&self) -> Self {
        match self {
            Self::Node(n) => Self::Node(Arc::clone(n)),
            Self::Input(i) => Self::Input(Arc::clone(i)),
            Self::Binding(b) => Self::Binding(Arc::clone(b)),
        }
    }
}
impl<E> Member<E> {
    fn route(&self) -> &CoreRoute<E> {
        match self {
            Self::Node(n) => &n.route,
            Self::Input(i) => &i.route,
            Self::Binding(b) => &b.route,
        }
    }
    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Node(a), Self::Node(b)) => Arc::ptr_eq(a, b),
            (Self::Input(a), Self::Input(b)) => Arc::ptr_eq(a, b),
            (Self::Binding(a), Self::Binding(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}
struct Region<E> {
    rank: u64,
    state: Mutex<RegionState<E>>,
}
enum RegionState<E> {
    Active(BTreeMap<CoreId, Member<E>>),
    Redirect(Arc<Region<E>>),
}
/// Shared by all outward handles made before attachment. Member cores and
/// stored/default envelopes only contain weak coordinates, never this sponsor.
pub(crate) struct SponsorAnchor<E> {
    root: OnceLock<Arc<Region<E>>>,
}
impl<E> SponsorAnchor<E> {
    pub(crate) fn for_binding(core: &Arc<crate::color_binding::BindingCore<E>>) -> Arc<Self> {
        let region = {
            let _guard = MERGE.lock().unwrap_or_else(|e| e.into_inner());
            let root = core
                .route
                .owner
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .upgrade()
                .expect("binding owner retained by sender/frame");
            let region = canonical(&root);
            {
                let state = region.state.lock().unwrap_or_else(|e| e.into_inner());
                assert!(
                    matches!(&*state,RegionState::Active(m) if matches!(m.get(&core.route.id),Some(Member::Binding(b)) if Arc::ptr_eq(b,core))),
                    "exact binding member"
                );
            }
            region
        };
        let anchor = Self::unpublished();
        assert!(anchor.root.set(region).is_ok());
        anchor
    }
    pub(crate) fn property_handle(
        self: &Arc<Self>,
        id: CoreId,
        kind: crate::SolidColorProperty,
    ) -> crate::SharedSolidColorProperty<E> {
        let owner = {
            let _guard = MERGE.lock().unwrap_or_else(|e| e.into_inner());
            let region = canonical(self.root.get().expect("published sponsor"));
            let state = region.state.lock().unwrap_or_else(|e| e.into_inner());
            match &*state {
                RegionState::Active(m) => match m.get(&id) {
                    Some(Member::Node(n)) => Arc::clone(n),
                    _ => panic!("actual property owner"),
                },
                _ => unreachable!(),
            }
        };
        crate::live_solid_color::SharedSolidColorProperty::from_binding(
            owner,
            kind,
            Arc::clone(self),
        )
    }
    pub(crate) fn unpublished() -> Arc<Self> {
        Arc::new(Self {
            root: OnceLock::new(),
        })
    }
    pub(crate) fn publish(&self, member: Member<E>) {
        let id = member.route().id;
        let region = Arc::new(Region {
            rank: serial(),
            state: Mutex::new(RegionState::Active(BTreeMap::from([(id, member.clone())]))),
        });
        *member
            .route()
            .owner
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Arc::downgrade(&region);
        // Publication occurs before an outward handle can escape.
        assert!(
            self.root.set(region).is_ok(),
            "Color sponsor published once"
        );
    }
    pub(crate) fn assert_input(&self, id: CoreId) {
        let _guard = MERGE.lock().unwrap_or_else(|e| e.into_inner());
        let region = canonical(self.root.get().expect("published Color sponsor"));
        let state = region.state.lock().unwrap_or_else(|e| e.into_inner());
        assert!(
            matches!(&*state,RegionState::Active(map) if matches!(map.get(&id),Some(Member::Input(i)) if i.route.id==id)),
            "APV is an actual scoped input member"
        );
    }
    pub(crate) fn input_handle(self: &Arc<Self>, id: CoreId) -> SharedColorInput<E> {
        let core = {
            let _guard = MERGE.lock().unwrap_or_else(|e| e.into_inner());
            let region = canonical(self.root.get().expect("published Color sponsor"));
            let state = region.state.lock().unwrap_or_else(|e| e.into_inner());
            match &*state {
                RegionState::Active(map) => match map.get(&id) {
                    Some(Member::Input(input))
                        if id.kind == CoreKind::Input && input.route.id == id =>
                    {
                        Arc::clone(input)
                    }
                    _ => panic!("APV resolves to its exact scoped input member"),
                },
                RegionState::Redirect(_) => unreachable!("canonical region under coordinator"),
            }
        };
        // Outward handle construction occurs after all membership guards. Its
        // anchor can be an earlier retaining owner; core identity is unchanged.
        SharedColorInput {
            core,
            anchor: Arc::clone(self),
        }
    }
    pub(crate) fn node_frame(self: &Arc<Self>, core: &Arc<Inner<E>>) -> DispatchLease<E> {
        let root = self.root.get().expect("published Color sponsor");
        let region = {
            let _guard = MERGE.lock().unwrap_or_else(|e| e.into_inner());
            let region = canonical(root);
            let present = {
                let state = region.state.lock().unwrap_or_else(|e| e.into_inner());
                matches!(&*state,RegionState::Active(map) if matches!(map.get(&core.route.id),Some(Member::Node(n)) if Arc::ptr_eq(n,core)))
            };
            assert!(present, "sender is an actual member of its Color sponsor");
            region
        };
        DispatchLease {
            _region: region,
            _anchor: Arc::clone(self),
        }
    }
}
/// A strong synchronous frame independent of internal weak routes. A reentrant
/// merge extends its old region by a strictly forward redirect.
pub(crate) struct DispatchLease<E> {
    _region: Arc<Region<E>>,
    _anchor: Arc<SponsorAnchor<E>>,
}
fn canonical<E>(root: &Arc<Region<E>>) -> Arc<Region<E>> {
    let mut current = Arc::clone(root);
    loop {
        let next = {
            let state = current.state.lock().unwrap_or_else(|e| e.into_inner());
            match &*state {
                RegionState::Active(_) => None,
                RegionState::Redirect(next) => {
                    assert!(
                        next.rank > current.rank,
                        "Color redirects strictly increase"
                    );
                    Some(Arc::clone(next))
                }
            }
        };
        match next {
            Some(next) => current = next,
            None => return current,
        }
    }
}
// Declared before all merge guards. Its contents can own actual callback cores;
// unwinding and normal scope exit retire them only after every guard is gone.
struct PostGuardRetirement<E> {
    states: Vec<RegionState<E>>,
    roots: Vec<Arc<Region<E>>>,
}
impl<E> PostGuardRetirement<E> {
    fn new() -> Self {
        Self {
            states: Vec::with_capacity(2),
            roots: Vec::with_capacity(3),
        }
    }
}
impl<E> Drop for PostGuardRetirement<E> {
    fn drop(&mut self) {
        self.states.clear();
        self.roots.clear();
    }
}
#[allow(dead_code)]
pub(crate) fn merge<E>(a: &Arc<SponsorAnchor<E>>, b: &Arc<SponsorAnchor<E>>) {
    // Explicit outer retirement owner: guards below are destroyed first even
    // on panic. All potentially destructive publication is infallible swaps.
    let mut retired = PostGuardRetirement::new();
    {
        let _coordinator = MERGE.lock().unwrap_or_else(|e| e.into_inner());
        let left = canonical(a.root.get().expect("published sponsor"));
        let right = canonical(b.root.get().expect("published sponsor"));
        retired.roots.push(Arc::clone(&left));
        retired.roots.push(Arc::clone(&right));
        if Arc::ptr_eq(&left, &right) {
            return;
        }
        let mut l = left.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut r = right.state.lock().unwrap_or_else(|e| e.into_inner());
        let (RegionState::Active(lm), RegionState::Active(rm)) = (&*l, &*r) else {
            unreachable!("canonical regions under coordinator")
        };
        for (id, member) in lm.iter().chain(rm.iter()) {
            assert_eq!(
                *id,
                member.route().id,
                "region key is actual immutable scoped identity"
            );
        }
        let mut joined = lm.clone();
        for (id, member) in rm {
            if let Some(existing) = joined.get(id) {
                assert!(existing.same(member), "scoped ID has one actual core");
            } else {
                joined.insert(*id, member.clone());
            }
        }
        let members: Vec<_> = joined.values().cloned().collect();
        let fresh = Arc::new(Region {
            rank: serial(),
            state: Mutex::new(RegionState::Active(joined)),
        });
        assert!(fresh.rank > left.rank && fresh.rank > right.rank);
        retired.roots.push(Arc::clone(&fresh));
        let replacements: Vec<_> = members.iter().map(|_| Arc::downgrade(&fresh)).collect();
        let mut routes: Vec<_> = members
            .iter()
            .map(|m| m.route().owner.lock().unwrap_or_else(|e| e.into_inner()))
            .collect();
        for (member, route) in members.iter().zip(&routes) {
            let owner = route.upgrade().expect("member has a live canonical owner");
            let expected = if lm.contains_key(&member.route().id) {
                &left
            } else {
                &right
            };
            assert!(
                Arc::ptr_eq(&owner, expected),
                "member weak route matches exact canonical region"
            );
        }
        let redirect_left = RegionState::Redirect(Arc::clone(&fresh));
        let redirect_right = RegionState::Redirect(Arc::clone(&fresh));
        #[cfg(test)]
        fault_at(MergeFault::BeforeCommit);
        // Commit has no allocation/user code/fallible work: retire capacity is
        // reserved, redirects/routes prepared and all locks already acquired.
        retired
            .states
            .push(std::mem::replace(&mut *l, redirect_left));
        retired
            .states
            .push(std::mem::replace(&mut *r, redirect_right));
        for (route, next) in routes.iter_mut().zip(replacements) {
            **route = next;
        }
        #[cfg(test)]
        fault_at(MergeFault::AfterCommit);
    }
    // Includes actual notifier callback containers, so this must stay outside
    // the coordinator, region and route guard scope.
    drop(retired);
}
/// Detached copies of only the actual input-value graph reached by stored
/// rr.APV. Binding receivers, UI effects and delegates are not render operands.
pub(crate) struct ColorInputCapture<E> {
    copies: Vec<(SharedColorInput<E>, SharedColorInput<E>)>,
}
impl<E> Default for ColorInputCapture<E> {
    fn default() -> Self {
        Self { copies: Vec::new() }
    }
}
impl<E> ColorInputCapture<E> {
    pub(crate) fn copy(&mut self, input: &SharedColorInput<E>) -> SharedColorInput<E> {
        if let Some((_, copy)) = self
            .copies
            .iter()
            .find(|(old, _)| old.same_reference(input))
        {
            return copy.clone();
        }
        let (stored, swatch) = {
            let s = input.core.state.lock().unwrap_or_else(|e| e.into_inner());
            (s.copied, s.swatch)
        };
        let core = Arc::new(InputCore {
            route: CoreRoute::new(CoreKind::Input),
            state: Mutex::new(InputState {
                copied: stored,
                swatch,
            }),
            input_changed: Mutex::new(InputDelegateCore::Absent),
        });
        let anchor = SponsorAnchor::unpublished();
        anchor.publish(Member::Input(core.clone()));
        let copy = SharedColorInput { core, anchor };
        self.copies.push((input.clone(), copy.clone()));
        if let Some(id) = stored.apv {
            let child = self.copy(&input.anchor.input_handle(id));
            merge(&copy.anchor, &child.anchor);
            copy.core
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .copied
                .apv = Some(child.core.route.id);
        }
        copy
    }
}
pub(crate) struct ColorInputValues<E> {
    ids: Vec<SharedColorInput<E>>,
}
impl<E> Default for ColorInputValues<E> {
    fn default() -> Self {
        Self { ids: Vec::new() }
    }
}
impl<E> ColorInputValues<E> {
    pub(crate) fn read(&mut self, input: &SharedColorInput<E>) -> Vec<u64> {
        fn visit<E>(
            input: &SharedColorInput<E>,
            ids: &mut Vec<SharedColorInput<E>>,
            seen: &mut Vec<CoreId>,
            out: &mut Vec<u64>,
        ) {
            let id = if let Some(id) = ids.iter().position(|v| v.same_reference(input)) {
                id
            } else {
                ids.push(input.clone());
                ids.len() - 1
            };
            out.push(id as u64);
            if seen.contains(&input.core.route.id) {
                out.push(0);
                return;
            }
            seen.push(input.core.route.id);
            out.push(1);
            let (stored, swatch) = {
                let s = input.core.state.lock().unwrap_or_else(|e| e.into_inner());
                (s.copied, s.swatch)
            };
            out.extend(stored.components.map(f64::to_bits));
            out.push(swatch.color().packed() as u64);
            match stored.apv {
                None => out.push(0),
                Some(id) => {
                    out.push(1);
                    visit(&input.anchor.input_handle(id), ids, seen, out);
                }
            }
        }
        let mut out = Vec::new();
        visit(input, &mut self.ids, &mut Vec::new(), &mut out);
        out
    }
}
/// Retained Color input value core for the characterized uncreated-handle path.
/// This capability has no bind/factory or
/// subscriber operations. The ann setter itself does not emit its input event.
/// This represents the characterized uncreated-handle value path, not a native
/// Control, a successful invalidation provider, a modal or a binding factory.
pub struct SharedColorInput<E = crate::DefaultContext> {
    pub(crate) core: Arc<InputCore<E>>,
    pub(crate) anchor: Arc<SponsorAnchor<E>>,
}
impl<E> Clone for SharedColorInput<E> {
    fn clone(&self) -> Self {
        Self {
            core: Arc::clone(&self.core),
            anchor: Arc::clone(&self.anchor),
        }
    }
}
impl<E> std::fmt::Debug for SharedColorInput<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SharedColorInput({:p})", Arc::as_ptr(&self.core))
    }
}
impl<E> SharedColorInput<E> {
    /// Creates copied zero RGB/APV-null and an opaque-black stored swatch.
    /// This value core does not create a native or GPUI control.
    pub fn new_uncreated() -> Self {
        let core = Arc::new(InputCore {
            route: CoreRoute::new(CoreKind::Input),
            state: Mutex::new(InputState {
                copied: StoredColorValue::unlinked([0.0; 3]),
                swatch: ColorSwatchValue::new(),
            }),
            input_changed: Mutex::new(InputDelegateCore::Absent),
        });
        let anchor = SponsorAnchor::unpublished();
        anchor.publish(Member::Input(Arc::clone(&core)));
        Self { core, anchor }
    }
    /// Copies the caller's RGB/reference, converts the stored swatch, then
    /// stamps this input on success. The captured NaN-red failure retains the
    /// incoming copy/reference and previous swatch. No listener or UI effect
    /// is dispatched by this characterized value operation.
    pub fn set_value(&self, value: &ContextColorValue<E>) -> Result<(), ColorSwatchError> {
        if let Some(sponsor) = value.sponsor() {
            merge(&self.anchor, sponsor);
        }
        let copied = value.stored();
        let mut state = self.core.state.lock().unwrap_or_else(|e| e.into_inner());
        // Source ann.a(rr): copy first, convert second, self-stamp only after
        // conversion returns. NaN failure keeps copied foreign APV + old swatch.
        state.copied = copied;
        state.swatch.set_opaque_rgb(copied.components)?;
        state.copied.apv = Some(self.core.route.id);
        Ok(())
    }
    /// Returns an owned RGB/reference copy; changes to the copy cannot mutate
    /// this input. A non-null reference retains its actual input core.
    pub fn value(&self) -> ContextColorValue<E> {
        let copied = self
            .core
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .copied;
        ContextColorValue::retained(copied, &self.anchor)
    }
    /// Actual input reference identity, independent of region/anchor identity.
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.core, &other.core)
    }
    /// Copies the stored opaque swatch color. This is not a painted pixel or
    /// a native invalidation/readback operation.
    pub fn swatch_color(&self) -> crate::ColorBgra {
        self.core
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .swatch
            .color()
    }
    /// Appends an ordinary invocation sequence; the empty delegate is null.
    pub fn add_input_changed(&self, delegate: &ColorInputDelegate<E>) {
        let retired = {
            let mut slot = self
                .core
                .input_changed
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let next = slot.combine(&delegate.core);
            std::mem::replace(&mut *slot, next)
        };
        drop(retired);
    }
    /// Removes the last whole matching contiguous receiver sequence.
    pub fn remove_input_changed(&self, delegate: &ColorInputDelegate<E>) {
        let retired = {
            let mut slot = self
                .core
                .input_changed
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let next = slot.remove(&delegate.core);
            std::mem::replace(&mut *slot, next)
        };
        drop(retired);
    }
    pub fn input_changed_delegate(&self) -> ColorInputDelegate<E> {
        let core = self
            .core
            .input_changed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        ColorInputDelegate::snapshot(core, &self.anchor)
    }
    // Original protected ann.a(EventArgs), not a user edit/modal/bind API. The
    // equivalent is exercised by crate-private tests; setters never call it.
    #[allow(dead_code)]
    pub(crate) fn raise_input_changed(
        &self,
        args: Option<Arc<ColorInputEventArgs>>,
    ) -> Result<(), E> {
        let frame = self.anchor.input_handle(self.core.route.id);
        assert!(
            frame.same_reference(self),
            "actual retained input sender frame"
        );
        let captured = self
            .core
            .input_changed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        captured.invoke(&frame, args)
    }
    pub(crate) fn copied_reference(&self, components: [f64; 3]) -> ContextColorValue<E> {
        ContextColorValue::retained(
            StoredColorValue {
                components,
                apv: Some(self.core.route.id),
            },
            &self.anchor,
        )
    }
    #[cfg(test)]
    fn new() -> Self {
        Self::new_uncreated()
    }
    #[cfg(test)]
    fn set(&self, value: &ContextColorValue<E>) -> Result<(), ColorSwatchError> {
        self.set_value(value)
    }
}
#[cfg(test)]
type ColorInput<E> = SharedColorInput<E>;

#[cfg(test)]
#[derive(Clone, Copy, Eq, PartialEq)]
enum MergeFault {
    None,
    BeforeCommit,
    AfterCommit,
}
#[cfg(test)]
thread_local! { static MERGE_FAULT:std::cell::Cell<MergeFault>=const { std::cell::Cell::new(MergeFault::None) }; }
#[cfg(test)]
fn fault_at(site: MergeFault) {
    MERGE_FAULT.with(|fault| {
        if fault.get() == site {
            fault.set(MergeFault::None);
            panic!("injected merge interruption");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ContextChangeArgument, ContextColorUndoInfo, ContextPropertyChangeArgs, ContextUndoInfo,
        SharedSolidColorNode, SharedSolidColorProperty, SolidColorCloneMode,
        SolidColorNodeDelegate, SolidColorNodeHandler, SolidColorPropertyDelegate,
        SolidColorPropertyHandler,
    };
    use std::{
        cell::RefCell,
        panic::{AssertUnwindSafe, catch_unwind},
        rc::Rc,
        sync::atomic::AtomicUsize,
    };
    fn node() -> SharedSolidColorNode<Rc<()>> {
        SharedSolidColorNode::new_unattached(SolidColorCloneMode::Original)
            .unwrap_or_else(|_| panic!("actual node constructor"))
    }
    fn linked(input: &ColorInput<Rc<()>>, rgb: [f64; 3]) -> ContextColorValue<Rc<()>> {
        ContextColorValue::retained(
            StoredColorValue {
                components: rgb,
                apv: Some(input.core.route.id),
            },
            &input.anchor,
        )
    }
    fn active<E>(anchor: &Arc<SponsorAnchor<E>>) -> Arc<Region<E>> {
        let _guard = MERGE.lock().unwrap_or_else(|e| e.into_inner());
        canonical(anchor.root.get().unwrap())
    }
    #[test]
    fn reciprocal_failed_inputs_keep_actual_peers_then_drop_without_backedges() {
        let a = ColorInput::<Rc<()>>::new();
        let b = ColorInput::<Rc<()>>::new();
        a.set(&ContextColorValue::unlinked([0.25, 0.5, 0.75]))
            .unwrap();
        b.set(&ContextColorValue::unlinked([0.75, 0.5, 0.25]))
            .unwrap();
        let wa = Arc::downgrade(&a.core);
        let wb = Arc::downgrade(&b.core);
        let ac = a.core.state.lock().unwrap().swatch.color();
        let bc = b.core.state.lock().unwrap().swatch.color();
        let bad_b = linked(&b, [f64::from_bits(0x7ff8000000001234), 0.5, 0.75]);
        let bad_a = linked(&a, [f64::from_bits(0x7ff8000000005678), 0.5, 0.75]);
        assert_eq!(
            a.set(&bad_b).unwrap_err().source_class(),
            "ArgumentException"
        );
        assert_eq!(
            b.set(&bad_a).unwrap_err().source_class(),
            "ArgumentException"
        );
        assert_eq!(a.core.state.lock().unwrap().swatch.color(), ac);
        assert_eq!(b.core.state.lock().unwrap().swatch.color(), bc);
        assert_eq!(a.value().stored().apv, Some(b.core.route.id));
        assert_eq!(b.value().stored().apv, Some(a.core.route.id));
        assert_eq!(a.value().components()[0].to_bits(), 0x7ff8000000001234);
        let held = a.value();
        drop((a, b, bad_a, bad_b));
        assert!(wa.upgrade().is_some() && wb.upgrade().is_some());
        drop(held);
        assert!(wa.upgrade().is_none() && wb.upgrade().is_none());
    }
    #[test]
    fn copied_positive_values_and_same_undo_argument_arc_retain_one_core_and_flags() {
        let a = ColorInput::<Rc<()>>::new();
        let b = ColorInput::<Rc<()>>::new();
        a.set(&ContextColorValue::unlinked([0.1, 0.2, 0.3]))
            .unwrap();
        b.set(&ContextColorValue::unlinked([0.4, 0.5, 0.6]))
            .unwrap();
        let wa = Arc::downgrade(&a.core);
        let wb = Arc::downgrade(&b.core);
        let old = a.value();
        let new = b.value();
        let undo = Arc::new(ContextColorUndoInfo::with_values(
            Some(vec![65]),
            7,
            old.clone(),
            new.clone(),
            false,
        ));
        let args = Arc::new(ContextChangeArgument::Property(
            ContextPropertyChangeArgs::with_color_undo(Some(Arc::clone(&undo))),
        ));
        undo.set_temporary_change(true);
        undo.set_lab_undo(true);
        let ContextChangeArgument::Property(property) = args.as_ref() else {
            panic!("property")
        };
        let Some(ContextUndoInfo::Color(same)) = property.typed_undo_info() else {
            panic!("Color root")
        };
        assert!(Arc::ptr_eq(same, &undo));
        assert!(same.temporary_change() && same.is_lab_undo());
        assert_eq!(same.old_color_value().stored().apv, old.stored().apv);
        assert_eq!(same.new_color_value().stored().apv, new.stored().apv);
        let readback = same.old_color_value();
        drop((a, b, old, new, undo));
        assert!(wa.upgrade().is_some() && wb.upgrade().is_some());
        drop(args);
        assert!(wa.upgrade().is_some() && wb.upgrade().is_some());
        drop(readback);
        assert!(wa.upgrade().is_none() && wb.upgrade().is_none());
    }
    enum Held {
        Node(SharedSolidColorNode<Rc<()>>),
        Property(SharedSolidColorProperty<Rc<()>>),
        NodeDelegate(SolidColorNodeDelegate<Rc<()>>),
        PropertyDelegate(SolidColorPropertyDelegate<Rc<()>>),
        NodeHandler(SolidColorNodeHandler<Rc<()>>),
        PropertyHandler(SolidColorPropertyHandler<Rc<()>>),
    }
    impl Held {
        fn inspect(&self) {
            match self {
                Self::Node(n) => {
                    let _ = n.snapshot();
                }
                Self::Property(p) => {
                    let _ = p.snapshot();
                }
                Self::NodeDelegate(d) => {
                    let _ = d.is_present();
                }
                Self::PropertyDelegate(d) => {
                    let _ = d.is_present();
                }
                Self::NodeHandler(h) => assert!(h.same_binding(h)),
                Self::PropertyHandler(h) => assert!(h.same_binding(h)),
            }
        }
    }
    #[test]
    fn all_outward_constructor_classes_promote_already_held_handles() {
        for class in 0..11 {
            let n = node();
            let input = ColorInput::<Rc<()>>::new();
            let peer = ColorInput::<Rc<()>>::new();
            let wn = Arc::downgrade(&n.0);
            let wi = Arc::downgrade(&input.core);
            let wp = Arc::downgrade(&peer.core);
            n.add(&SolidColorNodeDelegate::from_handlers(&[
                SolidColorNodeHandler::new(|_, _| Ok(())),
            ]));
            let property = n.color_property();
            let held = match class {
                0 => Held::Node(n.clone()),
                1 => Held::Property(property.clone()),
                2 => Held::Property(n.transparency_property()),
                3 => Held::Node(property.parent()),
                4 => Held::NodeDelegate(n.changed_delegate().clone()),
                5 => Held::PropertyDelegate(property.changed_delegate().clone()),
                6 => Held::PropertyHandler(property.forwarding_handler().clone()),
                7 => Held::NodeHandler(n.changed_delegate().handlers().remove(0)),
                8 => Held::PropertyHandler(property.changed_delegate().handlers().remove(0)),
                9 => Held::NodeDelegate(SolidColorNodeDelegate::from_handlers(
                    &n.changed_delegate().handlers(),
                )),
                _ => Held::PropertyDelegate(SolidColorPropertyDelegate::from_handlers(
                    &property.changed_delegate().handlers(),
                )),
            };
            merge(&n.1, &input.anchor);
            merge(&input.anchor, &peer.anchor);
            held.inspect();
            drop((n, property, input, peer));
            assert!(
                wn.upgrade().is_some() && wi.upgrade().is_some() && wp.upgrade().is_some(),
                "held class {class}"
            );
            drop(held);
            assert!(
                wn.upgrade().is_none() && wi.upgrade().is_none() && wp.upgrade().is_none(),
                "drop class {class}"
            );
        }
    }
    #[test]
    fn actual_sender_and_argument_frames_survive_reentrant_merge_and_throw() {
        struct Received {
            node: Option<SharedSolidColorNode<Rc<()>>>,
            property: Option<SharedSolidColorProperty<Rc<()>>>,
            args: Option<Arc<ContextChangeArgument<Rc<()>>>>,
        }
        let n = node();
        let input = ColorInput::<Rc<()>>::new();
        let second = ColorInput::<Rc<()>>::new();
        let received = Arc::new(Mutex::new(Received {
            node: None,
            property: None,
            args: None,
        }));
        let recorder = Arc::downgrade(&received);
        let nh = SolidColorNodeHandler::new(move |sender, args| {
            let received = recorder.upgrade().unwrap();
            let mut received = received.lock().unwrap();
            received.node = Some(sender.clone());
            received.args = args;
            Ok(())
        });
        n.add(&SolidColorNodeDelegate::from_handlers(&[nh]));
        let recorder = Arc::downgrade(&received);
        let second_anchor = Arc::downgrade(&second.anchor);
        let ph = SolidColorPropertyHandler::new(move |sender, args| {
            let parent = sender.parent();
            merge(&parent.1, &second_anchor.upgrade().unwrap());
            let received = recorder.upgrade().unwrap();
            let mut received = received.lock().unwrap();
            assert!(received.node.as_ref().unwrap().same_reference(&parent));
            assert!(Arc::ptr_eq(
                received.args.as_ref().unwrap(),
                args.as_ref().unwrap()
            ));
            received.property = Some(sender.clone());
            Err(Rc::new(()))
        });
        n.color_property()
            .add(&SolidColorPropertyDelegate::from_handlers(&[ph]));
        merge(&n.1, &input.anchor);
        assert!(n.color_property().set_color([0.25, 0.5, 0.75]).is_err());
        assert_eq!(n.snapshot().color, [0.25, 0.5, 0.75]);
        let wn = Arc::downgrade(&n.0);
        let wi = Arc::downgrade(&input.core);
        let ws = Arc::downgrade(&second.core);
        drop((n, input, second));
        assert!(wn.upgrade().is_some() && wi.upgrade().is_some() && ws.upgrade().is_some());
        assert_eq!(
            received
                .lock()
                .unwrap()
                .property
                .as_ref()
                .unwrap()
                .snapshot()
                .color,
            [0.25, 0.5, 0.75]
        );
        drop(received);
        assert!(wn.upgrade().is_none() && wi.upgrade().is_none() && ws.upgrade().is_none());
    }
    #[test]
    fn merge_prepublication_failure_and_postcommit_unwind_keep_exact_routes() {
        for fault in [MergeFault::BeforeCommit, MergeFault::AfterCommit] {
            let a = node();
            let b = ColorInput::<Rc<()>>::new();
            let before_a = active(&a.1);
            let before_b = active(&b.anchor);
            MERGE_FAULT.with(|f| f.set(fault));
            assert!(catch_unwind(AssertUnwindSafe(|| merge(&a.1, &b.anchor))).is_err());
            let after_a = active(&a.1);
            let after_b = active(&b.anchor);
            if fault == MergeFault::BeforeCommit {
                assert!(Arc::ptr_eq(&before_a, &after_a) && Arc::ptr_eq(&before_b, &after_b));
            } else {
                assert!(Arc::ptr_eq(&after_a, &after_b));
                assert!(after_a.rank > before_a.rank && after_a.rank > before_b.rank);
            }
            merge(&a.1, &b.anchor);
            let _frame = a.1.node_frame(&a.0);
            assert!(Arc::ptr_eq(&active(&a.1), &active(&b.anchor)));
        }
    }
    #[test]
    fn foreign_scoped_apv_and_corrupt_membership_are_rejected_before_publication() {
        let a = ColorInput::<Rc<()>>::new();
        let b = ColorInput::<Rc<()>>::new();
        assert!(
            catch_unwind(AssertUnwindSafe(|| ContextColorValue::retained(
                StoredColorValue {
                    components: [0.0; 3],
                    apv: Some(b.core.route.id)
                },
                &a.anchor
            )))
            .is_err()
        );
        let before_a = active(&a.anchor);
        let before_b = active(&b.anchor);
        {
            let mut state = before_b.state.lock().unwrap();
            let RegionState::Active(map) = &mut *state else {
                unreachable!()
            };
            let member = map.remove(&b.core.route.id).unwrap();
            let mut wrong = b.core.route.id;
            wrong.local = 99;
            map.insert(wrong, member);
        }
        assert!(catch_unwind(AssertUnwindSafe(|| merge(&a.anchor, &b.anchor))).is_err());
        assert!(
            Arc::ptr_eq(&before_a, &active(&a.anchor))
                && Arc::ptr_eq(&before_b, &active(&b.anchor))
        );
    }
    #[test]
    fn real_callback_destructors_retire_after_guards_in_normal_and_unwind_paths() {
        struct Reenter {
            a: SharedSolidColorNode<Rc<()>>,
            b: ColorInput<Rc<()>>,
            calls: Arc<AtomicUsize>,
        }
        impl Drop for Reenter {
            fn drop(&mut self) {
                merge(&self.a.1, &self.b.anchor);
                self.a
                    .color_property()
                    .set_color([0.1, 0.2, 0.3])
                    .unwrap_or_else(|_| panic!("drop reentry"));
                self.calls.fetch_add(1, Ordering::SeqCst);
            }
        }
        for panic in [false, true] {
            let calls = Arc::new(AtomicUsize::new(0));
            let target = node();
            let probe = Reenter {
                a: node(),
                b: ColorInput::new(),
                calls: Arc::clone(&calls),
            };
            let handler = SolidColorNodeHandler::new(move |_, _| {
                let _ = &probe;
                Ok(())
            });
            target.add(&SolidColorNodeDelegate::from_handlers(&[handler]));
            let core = Arc::clone(&target.0);
            let id = core.route.id;
            drop(target);
            // The retirement owner holds an ACTUAL Inner, including notifiers,
            // weak default envelopes and a real user callback destructor.
            let result = catch_unwind(AssertUnwindSafe(|| {
                let mut retirement = PostGuardRetirement::new();
                retirement.states.push(RegionState::Active(BTreeMap::from([(
                    id,
                    Member::Node(core),
                )])));
                let _coordinator = MERGE.lock().unwrap_or_else(|e| e.into_inner());
                if panic {
                    panic!("retirement unwind");
                }
            }));
            assert_eq!(result.is_err(), panic);
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }
    }
    #[test]
    fn typed_positive_payload_debug_writer_can_reenter_without_context_bounds() {
        struct Writer<'a> {
            input: &'a ColorInput<Rc<()>>,
            undo: &'a ContextColorUndoInfo<Rc<()>>,
        }
        impl std::fmt::Write for Writer<'_> {
            fn write_str(&mut self, _: &str) -> std::fmt::Result {
                let _ = self.input.value();
                self.undo.set_temporary_change(true);
                Ok(())
            }
        }
        let a = ColorInput::<Rc<()>>::new();
        a.set(&ContextColorValue::unlinked([0.1, 0.2, 0.3]))
            .unwrap();
        let value = a.value();
        let undo = ContextColorUndoInfo::with_values(None, 0, value.clone(), value.clone(), false);
        std::fmt::write(
            &mut Writer {
                input: &a,
                undo: &undo,
            },
            format_args!("{value:?} {undo:?}"),
        )
        .unwrap();
        assert!(undo.temporary_change());
        // Rc error context is neither Send/Sync nor Debug-constrained. The actual
        // recursive region types remain Send+Sync because E is only returned.
        fn send_sync<T: Send + Sync>() {}
        send_sync::<ContextColorValue<Rc<RefCell<()>>>>();
        send_sync::<ContextChangeArgument<Rc<RefCell<()>>>>();
    }
    #[test]
    fn actual_original_twelve_input_cases_preserve_copies_apv_and_partial_errors() {
        use serde_json::Value;
        use sha2::{Digest, Sha256};
        let mut total = (0, 0, 0);
        for (name, sha) in [
            (
                "attachment-swatch-swatch-r2-original.jsonl",
                "56db2d80e3c9641e04df795b5f3df50826e7a17bd15c3f3244ec3fdaaa183f7c",
            ),
            (
                "attachment-swatch-swatch-r3-original.jsonl",
                "624f7d6b668385e5ab7dfe083e6c37d75a501ae547cc42994d058c020746bf31",
            ),
        ] {
            let bytes = std::fs::read(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../../port/evidence")
                    .join(name),
            )
            .unwrap();
            assert_eq!(format!("{:x}", Sha256::digest(&bytes)), sha);
            let mut count = (0, 0, 0);
            for line in std::str::from_utf8(&bytes).unwrap().lines() {
                let row: Value = serde_json::from_str(line).unwrap();
                if row["kind"] != "swatch-case" {
                    continue;
                }
                let a = ColorInput::<Rc<()>>::new();
                let b = ColorInput::<Rc<()>>::new();
                let source_a = row["before"]["input"].as_i64().unwrap();
                let source_b = row["before_B"]["input"].as_i64().unwrap();
                let mapped = |apv: Option<CoreId>| match apv {
                    None => -1,
                    Some(id) if id == a.core.route.id => source_a,
                    Some(id) if id == b.core.route.id => source_b,
                    _ => panic!("exact APV input identity"),
                };
                let observe = |target: &ColorInput<Rc<()>>, source: &Value| {
                    let state = target.core.state.lock().unwrap();
                    for (i, component) in state.copied.components.iter().enumerate() {
                        assert_eq!(
                            format!("{:016x}", component.to_bits()),
                            source["local_value"]["components"][i].as_str().unwrap()
                        );
                    }
                    assert_eq!(
                        mapped(state.copied.apv),
                        source["local_value"]["apv_root"].as_i64().unwrap()
                    );
                    assert_eq!(
                        format!("{:08x}", state.swatch.color().packed()),
                        source["argb_hex"].as_str().unwrap()
                    );
                };
                observe(&a, &row["before"]);
                observe(&b, &row["before_B"]);
                // Only supplied input words and declared incoming APV enter the
                // producer. Native before/after/getter words are comparisons.
                let rgb = std::array::from_fn(|i| {
                    f64::from_bits(
                        u64::from_str_radix(row["supplied"]["components"][i].as_str().unwrap(), 16)
                            .unwrap(),
                    )
                });
                let supplied = match row["supplied"]["apv_root"].as_i64().unwrap() {
                    -1 => ContextColorValue::unlinked(rgb),
                    id if id == source_b => linked(&b, rgb),
                    _ => panic!("declared input APV"),
                };
                let result = a.set(&supplied);
                let source_error = row["error"]["class"]
                    .as_str()
                    .map(|c| c.strip_prefix("System.").unwrap());
                assert_eq!(
                    result.as_ref().err().map(|e| e.source_class()),
                    source_error
                );
                observe(&a, &row["after"]);
                observe(&b, &row["after_B"]);
                let getter = a.value();
                for (i, component) in getter.components().iter().enumerate() {
                    assert_eq!(
                        format!("{:016x}", component.to_bits()),
                        row["getter"]["components"][i].as_str().unwrap()
                    );
                    assert_eq!(
                        format!("{:016x}", supplied.components()[i].to_bits()),
                        row["supplied_after"]["components"][i].as_str().unwrap()
                    );
                }
                assert_eq!(
                    mapped(getter.stored().apv),
                    row["getter"]["apv_root"].as_i64().unwrap()
                );
                assert_eq!(
                    mapped(supplied.stored().apv),
                    row["supplied_after"]["apv_root"].as_i64().unwrap()
                );
                assert!(
                    a.core.input_changed.lock().unwrap().is_absent()
                        && b.core.input_changed.lock().unwrap().is_absent()
                );
                count.0 += 1;
                if result.is_ok() {
                    count.1 += 1
                } else {
                    count.2 += 1
                }
            }
            assert_eq!(count, (12, 10, 2));
            total.0 += count.0;
            total.1 += count.1;
            total.2 += count.2;
        }
        assert_eq!(total, (24, 20, 4));
        eprintln!(
            "Private input parity:12 original units/2 captures/24 comparisons/20 success/4 partial errors/0 differences"
        );
    }
}
