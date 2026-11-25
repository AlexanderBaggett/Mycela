//! Shared ordinary CLR EventHandler invocation storage. Callers own sender and
//! argument meaning; snapshots retain ordered real callback captures.
use std::sync::Arc;
type Callback<S, A, E> = dyn Fn(&S, A) -> Result<(), E> + Send + Sync;
pub(crate) struct HandlerCore<S, A, E> {
    pub(crate) callback: Box<Callback<S, A, E>>,
}
pub(crate) enum DelegateCore<S, A, E> {
    Absent,
    Single(Arc<HandlerCore<S, A, E>>),
    Multi(Arc<[Arc<HandlerCore<S, A, E>>]>),
}
impl<S, A, E> Clone for DelegateCore<S, A, E> {
    fn clone(&self) -> Self {
        match self {
            Self::Absent => Self::Absent,
            Self::Single(h) => Self::Single(Arc::clone(h)),
            Self::Multi(a) => Self::Multi(Arc::clone(a)),
        }
    }
}
impl<S, A, E> DelegateCore<S, A, E> {
    pub(crate) fn is_absent(&self) -> bool {
        matches!(self, Self::Absent)
    }
    pub(crate) fn entries(&self) -> &[Arc<HandlerCore<S, A, E>>] {
        match self {
            Self::Absent => &[],
            Self::Single(h) => std::slice::from_ref(h),
            Self::Multi(a) => a,
        }
    }
    pub(crate) fn from_entries(a: &[Arc<HandlerCore<S, A, E>>]) -> Self {
        match a {
            [] => Self::Absent,
            [h] => Self::Single(Arc::clone(h)),
            _ => Self::Multi(a.to_vec().into()),
        }
    }
    pub(crate) fn combine(&self, other: &Self) -> Self {
        if self.is_absent() {
            return other.clone();
        }
        if other.is_absent() {
            return self.clone();
        }
        let mut a = self.entries().to_vec();
        a.extend_from_slice(other.entries());
        Self::Multi(a.into())
    }
    pub(crate) fn remove(&self, other: &Self) -> Self {
        let a = self.entries();
        let b = other.entries();
        if b.is_empty() || b.len() > a.len() {
            return self.clone();
        }
        let Some(at) = (0..=a.len() - b.len()).rev().find(|&at| {
            a[at..at + b.len()]
                .iter()
                .zip(b)
                .all(|(a, b)| Arc::ptr_eq(a, b))
        }) else {
            return self.clone();
        };
        let mut result = Vec::with_capacity(a.len() - b.len());
        result.extend_from_slice(&a[..at]);
        result.extend_from_slice(&a[at + b.len()..]);
        Self::from_entries(&result)
    }
    pub(crate) fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Absent, Self::Absent) => true,
            (Self::Single(a), Self::Single(b)) => Arc::ptr_eq(a, b),
            (Self::Multi(a), Self::Multi(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
    pub(crate) fn invoke(&self, sender: &S, args: A) -> Result<(), E>
    where
        A: Clone,
    {
        for h in self.entries() {
            (h.callback)(sender, args.clone())?;
        }
        Ok(())
    }
}
