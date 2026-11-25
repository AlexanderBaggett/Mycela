//! Concrete contributor effects over the current, real document owner.
//! Other history services are forwarded to the supplied implementation.
use super::{History, HistoryCommand, HistoryEffect, HistoryError, HistoryServices, HistoryTarget};
use crate::{ContextUndoInfo, ContributorEffects, ContributorError, DocumentContributors};
use std::sync::Arc;

/// A retained handle to the specific actual source document (`eb`), not just
/// its contributor list. Each handle must retain its document owner.
pub trait ContributorHistoryDocument<E>: Send + Sync {
    fn contributors(&self) -> DocumentContributors;
    /// Capture the actual `eb` parent `rt` once, before eb.d reads its list.
    /// Keep this same parent across reentrant calls. A null parent is held and
    /// fails only when later dereferenced; an invalid cast fails here.
    fn capture_effects(&self) -> Result<Box<dyn ContributorEffects<E> + '_>, HistoryError<E>>;
    /// Called after eb.c removes the captured roots; resolve the current
    /// actual parent at this point and issue its document notification.
    fn notify_restored(&self) -> Result<(), E>;
}

/// Supplies the live workspace's actual current document on every lookup.
/// Returning `None` means there genuinely is no current document.
pub trait ContributorHistoryHost<E>: Send + Sync {
    fn current_document(
        &self,
    ) -> Result<Option<Arc<dyn ContributorHistoryDocument<E>>>, HistoryError<E>>;
}

// Keep the borrowed host's operation lease intact through all calls.
struct Effects<'a, E> {
    inner: Box<dyn ContributorEffects<E> + 'a>,
}
impl<E> ContributorEffects<E> for Effects<'_, E> {
    fn current_profile(&mut self) -> Result<crate::ContributorProfile, E> {
        self.inner.current_profile()
    }
    fn now(&mut self) -> Result<crate::ContributorTicks, E> {
        self.inner.now()
    }
    fn current_comment(&mut self) -> Result<Option<Vec<u16>>, E> {
        self.inner.current_comment()
    }
    fn names_equal(&mut self, left: &[u16], right: &[u16]) -> Result<bool, E> {
        self.inner.names_equal(left, right)
    }
    fn notify_document(&mut self) -> Result<(), E> {
        self.inner.notify_document()
    }
}

/// Wraps actual contributor effects around another real history service. It
/// does not provide missing view, resource, selection, or travel operations.
pub struct ContributorHistoryServices<E> {
    host: Arc<dyn ContributorHistoryHost<E>>,
    remaining: Arc<dyn HistoryServices<E>>,
}
impl<E> ContributorHistoryServices<E> {
    pub fn new(
        host: Arc<dyn ContributorHistoryHost<E>>,
        remaining: Arc<dyn HistoryServices<E>>,
    ) -> Self {
        Self { host, remaining }
    }
}
fn map_error<E>(error: ContributorError<E>) -> HistoryError<E> {
    match error {
        ContributorError::Effect(error) => HistoryError::Service(error),
        ContributorError::Index => HistoryError::ContributorIndex,
        ContributorError::CollectionModified => HistoryError::ContributorCollectionModified,
    }
}
impl<E: 'static> HistoryServices<E> for ContributorHistoryServices<E> {
    fn route_property_history(
        &self,
        owner: &History<E>,
        target: &HistoryTarget<E>,
        undo: Option<&ContextUndoInfo<E>>,
    ) -> Result<Option<History<E>>, HistoryError<E>> {
        self.remaining.route_property_history(owner, target, undo)
    }
    fn resolve_command_context(
        &self,
        history: &History<E>,
        command: &HistoryCommand<E>,
    ) -> Result<Option<HistoryTarget<E>>, HistoryError<E>> {
        self.remaining.resolve_command_context(history, command)
    }
    fn effect(
        &self,
        history: &History<E>,
        command: Option<&HistoryCommand<E>>,
        effect: HistoryEffect,
    ) -> Result<(), HistoryError<E>> {
        match effect {
            HistoryEffect::CaptureContributors => {
                let command = command.ok_or(HistoryError::MissingDependency(effect))?;
                // abl.b: s() is queried for the null test, then re-read for d().
                if self.host.current_document()?.is_none() {
                    return Ok(());
                }
                let document = self
                    .host
                    .current_document()?
                    .ok_or(HistoryError::NullDocument)?;
                let mut effects = Effects {
                    inner: document.capture_effects()?,
                };
                let contributors = document.contributors();
                // mt's property-command constructor supplies null additional contributors.
                command
                    .capture_document_contributors(&contributors, None, &mut effects)
                    .map_err(map_error)
            }
            HistoryEffect::RestoreContributors => {
                let command = command.ok_or(HistoryError::MissingDependency(effect))?;
                // abl.k checks m_h before either active-document lookup.
                if command.captured_contributors().is_none() {
                    return Ok(());
                }
                if self.host.current_document()?.is_none() {
                    return Ok(());
                }
                let document = self
                    .host
                    .current_document()?
                    .ok_or(HistoryError::NullDocument)?;
                let entries = command
                    .captured_contributors()
                    .ok_or(HistoryError::NullContributorList)?;
                document
                    .contributors()
                    .remove_captured_with(&entries, || document.notify_restored())
                    .map_err(map_error)
            }
            _ => self.remaining.effect(history, command, effect),
        }
    }
    fn pending_commands(
        &self,
        history: &History<E>,
        command: &HistoryCommand<E>,
    ) -> Result<Vec<HistoryCommand<E>>, HistoryError<E>> {
        self.remaining.pending_commands(history, command)
    }
}
