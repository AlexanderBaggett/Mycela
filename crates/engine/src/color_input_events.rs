//! Ordinary input EventHandler transport. Separate from the source's legacy
//! ChangeDelegate removal rules. Protected raising is private; setters do not
//! dispatch an input event, modal, binding or history operation.
use crate::{DefaultContext, SharedColorInput, color_region::SponsorAnchor};
use std::{fmt, sync::Arc};

/// Empty base EventArgs payload. Each caller-created Arc is a distinct argument
/// object; dispatch shares that exact Arc. No subclass/payload provider is implied.
#[derive(Default)]
pub struct ColorInputEventArgs {
    _private: (),
}
impl ColorInputEventArgs {
    pub fn new() -> Self {
        Self::default()
    }
}
impl fmt::Debug for ColorInputEventArgs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ColorInputEventArgs")
    }
}
pub(crate) type InputHandlerCore<E> =
    crate::ordinary_events::HandlerCore<SharedColorInput<E>, Option<Arc<ColorInputEventArgs>>, E>;
/// Retained receiver binding. Clone shares this receiver; a new construction
/// defines another receiver. General CLR method/target equality is not claimed.
pub struct ColorInputHandler<E = DefaultContext> {
    pub(crate) core: Arc<InputHandlerCore<E>>,
    sponsors: Vec<Arc<SponsorAnchor<E>>>,
}
impl<E> ColorInputHandler<E> {
    pub fn new(
        callback: impl Fn(&SharedColorInput<E>, Option<Arc<ColorInputEventArgs>>) -> Result<(), E>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            core: Arc::new(InputHandlerCore {
                callback: Box::new(callback),
            }),
            sponsors: Vec::new(),
        }
    }
    pub fn same_binding(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.core, &other.core)
    }
}
impl<E> Clone for ColorInputHandler<E> {
    fn clone(&self) -> Self {
        Self {
            core: Arc::clone(&self.core),
            sponsors: self.sponsors.clone(),
        }
    }
}
impl<E> fmt::Debug for ColorInputHandler<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ColorInputHandler({:p})", Arc::as_ptr(&self.core))
    }
}
pub(crate) type InputDelegateCore<E> =
    crate::ordinary_events::DelegateCore<SharedColorInput<E>, Option<Arc<ColorInputEventArgs>>, E>;
/// Immutable ordinary invocation sequence. Empty is source null. Snapshot
/// sponsors retain the input outward; registered storage contains only cores,
/// whose actual callback captures are never stripped, erased or weakened.
pub struct ColorInputDelegate<E = DefaultContext> {
    pub(crate) core: InputDelegateCore<E>,
    sponsors: Vec<Arc<SponsorAnchor<E>>>,
}
impl<E> Default for ColorInputDelegate<E> {
    fn default() -> Self {
        Self {
            core: InputDelegateCore::Absent,
            sponsors: Vec::new(),
        }
    }
}
impl<E> Clone for ColorInputDelegate<E> {
    fn clone(&self) -> Self {
        Self {
            core: self.core.clone(),
            sponsors: self.sponsors.clone(),
        }
    }
}
impl<E> fmt::Debug for ColorInputDelegate<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.core {
            InputDelegateCore::Absent => f.write_str("ColorInputDelegate(null)"),
            InputDelegateCore::Single(h) => write!(f, "ColorInputDelegate({:p})", Arc::as_ptr(h)),
            InputDelegateCore::Multi(a) => {
                write!(f, "ColorInputDelegate({:p}, {})", Arc::as_ptr(a), a.len())
            }
        }
    }
}
impl<E> ColorInputDelegate<E> {
    pub fn from_handlers(handlers: &[ColorInputHandler<E>]) -> Self {
        let cores: Vec<_> = handlers.iter().map(|h| Arc::clone(&h.core)).collect();
        let mut sponsors = Vec::new();
        for h in handlers {
            for owner in &h.sponsors {
                if !sponsors.iter().any(|s| Arc::ptr_eq(s, owner)) {
                    sponsors.push(Arc::clone(owner));
                }
            }
        }
        Self {
            core: InputDelegateCore::from_entries(&cores),
            sponsors,
        }
    }
    pub fn is_null(&self) -> bool {
        self.core.is_absent()
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        self.core.same(&other.core)
    }
    /// Each non-null query returns a fresh real immutable array retaining the
    /// same ordered receiver cores. A held array remains unchanged by updates.
    pub fn invocation_list(&self) -> Option<Arc<[ColorInputHandler<E>]>> {
        if self.is_null() {
            return None;
        }
        Some(
            self.core
                .entries()
                .iter()
                .map(|core| ColorInputHandler {
                    core: Arc::clone(core),
                    sponsors: self.sponsors.clone(),
                })
                .collect::<Vec<_>>()
                .into(),
        )
    }
    pub(crate) fn snapshot(core: InputDelegateCore<E>, anchor: &Arc<SponsorAnchor<E>>) -> Self {
        Self {
            core,
            sponsors: vec![Arc::clone(anchor)],
        }
    }
}

#[cfg(test)]
#[path = "color_input_events_tests.rs"]
mod tests;
