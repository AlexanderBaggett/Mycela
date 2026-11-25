//! Copied legacy RGB values tagged with their callback context.
//!
//! Positive Color editor references require their actual concrete binding
//! producer. This connected phase provides no binding, UI or attachment factory.
use crate::color_region::{SponsorAnchor, StoredColorValue};
use std::{fmt, marker::PhantomData, sync::Arc};

pub type DefaultContext = std::convert::Infallible;

/// A copied original rr component value. Unlinked construction is APV-null;
/// values produced by genuine input/reference operations retain actual typed
/// input cores through a region.
pub struct ContextColorValue<E = DefaultContext> {
    stored: StoredColorValue,
    sponsor: Option<Arc<SponsorAnchor<E>>>,
    context: PhantomData<fn() -> E>,
}
impl<E> ContextColorValue<E> {
    pub fn unlinked(components: [f64; 3]) -> Self {
        Self {
            stored: StoredColorValue::unlinked(components),
            sponsor: None,
            context: PhantomData,
        }
    }
    pub fn components(&self) -> [f64; 3] {
        self.stored.components
    }
    /// Copies new RGB words while preserving this value's actual input
    /// reference. The receiver and referenced input remain unchanged.
    pub fn with_components(&self, components: [f64; 3]) -> Self {
        let mut copied = self.clone();
        copied.stored.components = components;
        copied
    }
    /// Copies this RGB and assigns a genuine input reference, like rr.lh(apv).
    /// It changes neither the receiver nor the input's local value/swatch.
    pub fn with_input_reference(&self, input: &crate::SharedColorInput<E>) -> Self {
        input.copied_reference(self.stored.components)
    }
    /// Resolves a non-null reference to its exact retained input core. Null
    /// alone returns None; arbitrary IDs/owners cannot be supplied publicly.
    pub fn input_reference(&self) -> Option<crate::SharedColorInput<E>> {
        self.stored.apv.map(|id| {
            self.sponsor
                .as_ref()
                .expect("non-null Color reference has a strong sponsor")
                .input_handle(id)
        })
    }
    pub(crate) fn stored(&self) -> StoredColorValue {
        self.stored
    }
    pub(crate) fn sponsor(&self) -> Option<&Arc<SponsorAnchor<E>>> {
        self.sponsor.as_ref()
    }
    pub(crate) fn retained(stored: StoredColorValue, sponsor: &Arc<SponsorAnchor<E>>) -> Self {
        if let Some(id) = stored.apv {
            sponsor.assert_input(id);
        }
        Self {
            stored,
            sponsor: stored.apv.map(|_| Arc::clone(sponsor)),
            context: PhantomData,
        }
    }
    /// Original rr.Equals component comparison, without RGBColor tolerance.
    pub fn equals_components(&self, other: &Self) -> bool {
        self.stored.components == other.stored.components
    }
}
impl<E> Clone for ContextColorValue<E> {
    fn clone(&self) -> Self {
        Self {
            stored: self.stored,
            sponsor: self.sponsor.clone(),
            context: PhantomData,
        }
    }
}
impl<E> fmt::Debug for ContextColorValue<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ContextColorValue")
            .field(&self.stored.components)
            .finish()
    }
}
pub type ColorValue = ContextColorValue<DefaultContext>;
