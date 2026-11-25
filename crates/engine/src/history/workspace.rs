//! Active-document portion of the source asy property-history route.
//!
//! The workspace supplies its actual active document history and loading state.
//! All command context, resource, contributor, and travel operations remain
//! required services and are forwarded to the supplied implementation.
use super::{
    History, HistoryCommand, HistoryCore, HistoryEffect, HistoryError, HistoryServices,
    HistoryTarget, guard,
};
use crate::ContextUndoInfo;
use std::sync::{Arc, Mutex, Weak};

struct State<E> {
    active_history: Option<ActiveHistory<E>>,
    loading: bool,
}
struct ActiveHistory<E> {
    core: Weak<HistoryCore<E>>,
    owner: Option<Weak<dyn Send + Sync>>,
}

/// Retained active-workspace route for property changes.
///
/// This does not own the document or invent its resource services: an active
/// document remains alive through its real owner, and all other reached
/// effects are delegated to the caller's actual service implementation.
pub struct WorkspaceHistoryRouter<E> {
    state: Mutex<State<E>>,
    services: Arc<dyn HistoryServices<E>>,
}

impl<E: 'static> WorkspaceHistoryRouter<E> {
    pub fn new(services: Arc<dyn HistoryServices<E>>) -> Self {
        Self {
            state: Mutex::new(State {
                active_history: None,
                loading: false,
            }),
            services,
        }
    }

    /// Select the history of the workspace's actual active document, or clear
    /// it when there is no active document. The router never retains it alive.
    pub fn set_active_history(&self, history: Option<&History<E>>) {
        guard(&self.state).active_history = history.map(|h| ActiveHistory {
            core: Arc::downgrade(&h.core),
            owner: h.owner.as_ref().map(Arc::downgrade),
        });
    }

    pub fn active_history(&self) -> Option<History<E>> {
        let state = guard(&self.state);
        let active = state.active_history.as_ref()?;
        let core = active.core.upgrade()?;
        let owner = match &active.owner {
            Some(owner) => Some(owner.upgrade()?),
            None => None,
        };
        Some(History { core, owner })
    }

    /// Reflect the actual workspace loading state; changing this flag does not
    /// mutate a document, record a command, or complete missing services.
    pub fn set_loading(&self, loading: bool) {
        guard(&self.state).loading = loading;
    }

    pub fn is_loading(&self) -> bool {
        guard(&self.state).loading
    }
}

impl<E: Send + Sync + 'static> HistoryServices<E> for WorkspaceHistoryRouter<E> {
    fn route_property_history(
        &self,
        _: &History<E>,
        _: &HistoryTarget<E>,
        undo: Option<&ContextUndoInfo<E>>,
    ) -> Result<Option<History<E>>, HistoryError<E>> {
        // asy.b first resolves the active document, then reads loading, then
        // checks the undo. The normal History route retains the target sponsor
        // and rejects a foreign history cohort at its existing boundary.
        let active = self.active_history();
        if active.is_none() || self.is_loading() || undo.is_none() {
            return Ok(None);
        }
        Ok(active)
    }

    fn resolve_command_context(
        &self,
        history: &History<E>,
        command: &HistoryCommand<E>,
    ) -> Result<Option<HistoryTarget<E>>, HistoryError<E>> {
        self.services.resolve_command_context(history, command)
    }

    fn effect(
        &self,
        history: &History<E>,
        command: Option<&HistoryCommand<E>>,
        effect: HistoryEffect,
    ) -> Result<(), HistoryError<E>> {
        self.services.effect(history, command, effect)
    }

    fn pending_commands(
        &self,
        history: &History<E>,
        command: &HistoryCommand<E>,
    ) -> Result<Vec<HistoryCommand<E>>, HistoryError<E>> {
        self.services.pending_commands(history, command)
    }
}

#[cfg(test)]
mod tests;
