use std::{
    error::Error,
    fmt,
    sync::{Arc, Mutex},
};

type Callback<A, E> = dyn Fn(&ChangeNotifier<A, E>, Option<Arc<A>>) -> Result<(), E> + Send + Sync;
/// A retained callback identity. Reuse/cloning represents the same source
/// target/method binding; constructing another handle defines another binding.
pub struct ChangeHandler<A, E>(Arc<Callback<A, E>>);
impl<A, E> Clone for ChangeHandler<A, E> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}
impl<A, E> fmt::Debug for ChangeHandler<A, E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ChangeHandler({:p})", Arc::as_ptr(&self.0))
    }
}
impl<A, E> ChangeHandler<A, E> {
    pub fn new(
        callback: impl Fn(&ChangeNotifier<A, E>, Option<Arc<A>>) -> Result<(), E>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self(Arc::new(callback))
    }
    pub fn same_binding(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
/// Immutable source-profile delegate representation. Multicast arrays retain
/// identity even when their contents are empty or contain only one callback.
pub struct ChangeDelegate<A, E>(DelegateValue<A, E>);
enum DelegateValue<A, E> {
    Absent,
    Single(ChangeHandler<A, E>),
    Multi(Arc<[ChangeHandler<A, E>]>),
}
impl<A, E> Clone for ChangeDelegate<A, E> {
    fn clone(&self) -> Self {
        Self(match &self.0 {
            DelegateValue::Absent => DelegateValue::Absent,
            DelegateValue::Single(h) => DelegateValue::Single(h.clone()),
            DelegateValue::Multi(a) => DelegateValue::Multi(Arc::clone(a)),
        })
    }
}
impl<A, E> Default for ChangeDelegate<A, E> {
    fn default() -> Self {
        Self(DelegateValue::Absent)
    }
}
impl<A, E> fmt::Debug for ChangeDelegate<A, E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            DelegateValue::Absent => f.write_str("ChangeDelegate(absent)"),
            DelegateValue::Single(h) => h.fmt(f),
            DelegateValue::Multi(a) => write!(
                f,
                "ChangeDelegate(array {:p}, len {})",
                Arc::as_ptr(a),
                a.len()
            ),
        }
    }
}
impl<A, E> ChangeDelegate<A, E> {
    pub fn from_handlers(handlers: &[ChangeHandler<A, E>]) -> Self {
        match handlers {
            [] => Self::default(),
            [h] => Self(DelegateValue::Single(h.clone())),
            _ => Self(DelegateValue::Multi(handlers.to_vec().into())),
        }
    }
    pub fn is_present(&self) -> bool {
        !matches!(self.0, DelegateValue::Absent)
    }
    pub fn is_multicast(&self) -> bool {
        matches!(self.0, DelegateValue::Multi(_))
    }
    pub fn handlers(&self) -> &[ChangeHandler<A, E>] {
        match &self.0 {
            DelegateValue::Absent => &[],
            DelegateValue::Single(h) => std::slice::from_ref(h),
            DelegateValue::Multi(a) => a,
        }
    }
    pub fn same_invocation_array(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (DelegateValue::Multi(a), DelegateValue::Multi(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
    fn combine(&self, other: &Self) -> Self {
        if !self.is_present() {
            return other.clone();
        }
        if !other.is_present() {
            return self.clone();
        }
        let mut handlers = self.handlers().to_vec();
        handlers.extend_from_slice(other.handlers());
        Self(DelegateValue::Multi(handlers.into()))
    }
    fn remove(&self, value: &Self) -> Result<Self, ChangeNotificationFailure> {
        if !value.is_present() || !self.is_present() {
            return Ok(self.clone());
        }
        match (&self.0, &value.0) {
            (DelegateValue::Single(a), DelegateValue::Single(b)) => Ok(if a.same_binding(b) {
                Self::default()
            } else {
                self.clone()
            }),
            (DelegateValue::Single(a), DelegateValue::Multi(b)) => {
                Ok(if b.iter().any(|b| a.same_binding(b)) {
                    Self::default()
                } else {
                    self.clone()
                })
            }
            (DelegateValue::Multi(a), DelegateValue::Single(b)) => {
                let Some(index) = a.iter().rposition(|h| h.same_binding(b)) else {
                    return Ok(self.clone());
                };
                if a.len() <= 1 {
                    return Err(ChangeNotificationFailure::SingletonRemoval);
                }
                if a.len() == 2 {
                    return Ok(Self(DelegateValue::Single(a[1 - index].clone())));
                }
                let mut rest = a.to_vec();
                rest.remove(index);
                Ok(Self(DelegateValue::Multi(rest.into())))
            }
            (DelegateValue::Multi(a), DelegateValue::Multi(b)) => {
                if Arc::ptr_eq(a, b) {
                    return Ok(Self::default());
                }
                if a.len() < b.len() {
                    return Ok(self.clone());
                }
                if b.is_empty() && !a.is_empty() {
                    return Err(ChangeNotificationFailure::EmptyNeedle);
                }
                let index = (0..=a.len() - b.len()).rev().find(|&i| {
                    a[i..i + b.len()]
                        .iter()
                        .zip(b.iter())
                        .all(|(a, b)| a.same_binding(b))
                });
                let Some(index) = index else {
                    return Ok(self.clone());
                };
                let mut rest = Vec::with_capacity(a.len() - b.len());
                rest.extend_from_slice(&a[..index]);
                rest.extend_from_slice(&a[index + b.len()..]);
                Ok(Self(DelegateValue::Multi(rest.into())))
            }
            _ => Ok(self.clone()),
        }
    }
}
struct NotificationState<A, E> {
    allow: bool,
    pending: bool,
    delegate: ChangeDelegate<A, E>,
}
/// Synchronous nongeneric-source dispatch with typed Rust callback context.
/// The caller supplies the shared default/empty argument root. A does not imply
/// parity with the distinct CLR ChangableEntity<T> default virtual method.
/// Callback errors keep their type; no private state lock is held during calls.
pub struct ChangeNotifier<A, E> {
    state: Arc<Mutex<NotificationState<A, E>>>,
    empty: Arc<A>,
}
impl<A, E> Clone for ChangeNotifier<A, E> {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
            empty: Arc::clone(&self.empty),
        }
    }
}
impl<A, E> fmt::Debug for ChangeNotifier<A, E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ChangeNotifier({:p})", Arc::as_ptr(&self.state))
    }
}
impl<A, E> ChangeNotifier<A, E> {
    pub fn new(empty: Arc<A>) -> Self {
        Self {
            state: Arc::new(Mutex::new(NotificationState {
                allow: true,
                pending: false,
                delegate: ChangeDelegate::default(),
            })),
            empty,
        }
    }
    pub fn same_notifier(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.state, &other.state)
    }
    pub fn snapshot(&self) -> ChangeNotificationSnapshot<A, E> {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        ChangeNotificationSnapshot {
            allow: s.allow,
            pending: s.pending,
            delegate: s.delegate.clone(),
        }
    }
    pub fn set_allow(&self, allow: bool) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).allow = allow;
    }
    pub fn lock_changes(&self) {
        self.set_allow(false);
    }
    pub fn add(&self, delegate: &ChangeDelegate<A, E>) {
        let old = {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let next = s.delegate.combine(delegate);
            std::mem::replace(&mut s.delegate, next)
        };
        drop(old);
    }
    pub fn remove(&self, delegate: &ChangeDelegate<A, E>) -> Result<(), ChangeNotificationFailure> {
        let old = {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let next = s.delegate.remove(delegate)?;
            std::mem::replace(&mut s.delegate, next)
        };
        drop(old);
        Ok(())
    }
    pub fn emit(&self, args: Option<Arc<A>>) -> Result<(), ChangeDispatchError<E>> {
        let captured = {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if !s.allow {
                s.pending = true;
                return Ok(());
            }
            s.pending = false;
            s.delegate.clone()
        };
        if captured.is_multicast() && captured.handlers().is_empty() {
            return Err(ChangeDispatchError::Framework(
                ChangeNotificationFailure::EmptyInvocation,
            ));
        }
        for handler in captured.handlers() {
            (handler.0)(self, args.clone()).map_err(ChangeDispatchError::Handler)?;
        }
        Ok(())
    }
    pub fn emit_empty(&self) -> Result<(), ChangeDispatchError<E>> {
        self.emit(Some(Arc::clone(&self.empty)))
    }
    pub fn unlock(&self, process_pending: bool) -> Result<(), ChangeDispatchError<E>> {
        let pending = {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            s.allow = true;
            s.pending
        };
        if process_pending && pending {
            self.emit_empty()
        } else {
            Ok(())
        }
    }
}
pub struct ChangeNotificationSnapshot<A, E> {
    pub allow: bool,
    pub pending: bool,
    pub delegate: ChangeDelegate<A, E>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChangeNotificationFailure {
    SingletonRemoval,
    EmptyNeedle,
    EmptyInvocation,
}
impl ChangeNotificationFailure {
    pub const fn source_class(self) -> &'static str {
        match self {
            Self::SingletonRemoval => "InvalidOperationException",
            Self::EmptyNeedle | Self::EmptyInvocation => "IndexOutOfRangeException",
        }
    }
}
impl fmt::Display for ChangeNotificationFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.source_class())
    }
}
impl Error for ChangeNotificationFailure {}
#[derive(Debug)]
pub enum ChangeDispatchError<E> {
    Framework(ChangeNotificationFailure),
    Handler(E),
}
impl<E: fmt::Display> fmt::Display for ChangeDispatchError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Framework(e) => e.fmt(f),
            Self::Handler(e) => e.fmt(f),
        }
    }
}
impl<E: Error + 'static> Error for ChangeDispatchError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Framework(e) => Some(e),
            Self::Handler(e) => Some(e),
        }
    }
}
