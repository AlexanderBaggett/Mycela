//! The view owns one actual Invert model and its four property subscriptions.
//! Engine callbacks only request a foreground presentation; they never borrow
//! the view. Removing the effect invalidates queued work before unsubscribing.
use crate::{ForegroundError, color_binding_effects::BindingRuntime};
use mycela_engine::{SelectionCloneMode, SelectionHandler, SharedInvertNode};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub(crate) struct LiveInvert {
    model: SharedInvertNode<ForegroundError>,
    listener: SelectionHandler<ForegroundError>,
    active: Arc<AtomicBool>,
}
impl LiveInvert {
    pub(crate) fn new(runtime: &Arc<BindingRuntime>) -> Self {
        let model = SharedInvertNode::new(SelectionCloneMode::Original);
        let active = Arc::new(AtomicBool::new(true));
        let weak_active = Arc::downgrade(&active);
        let weak_runtime = Arc::downgrade(runtime);
        let listener = SelectionHandler::new(move |_, _| {
            if !weak_active
                .upgrade()
                .is_some_and(|a| a.load(Ordering::Acquire))
            {
                return Ok(());
            }
            weak_runtime
                .upgrade()
                .ok_or(ForegroundError::OwnerClosed)?
                .request_effect_presentation(weak_active.clone())
        });
        for channel in 0..4 {
            model
                .property(channel)
                .expect("four Invert properties")
                .add_changed(&listener);
        }
        Self {
            model,
            listener,
            active,
        }
    }
    pub(crate) fn model(&self) -> SharedInvertNode<ForegroundError> {
        self.model.clone()
    }
    pub(crate) fn affected(&self, channel: i32) -> Result<bool, ForegroundError> {
        self.model
            .property(channel)
            .and_then(|p| p.text())
            .map(|value| value.as_deref() != Some(&[78, 111]))
            .map_err(|e| ForegroundError::Operation(e.to_string()))
    }
    pub(crate) fn set_affected(&self, channel: i32, affected: bool) -> Result<(), ForegroundError> {
        self.model
            .property(channel)
            .and_then(|p| {
                p.set_text(Some(
                    if affected { "Yes" } else { "No" }.encode_utf16().collect(),
                ))
            })
            .map_err(|e| ForegroundError::Operation(e.to_string()))
    }
}
impl Drop for LiveInvert {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
        for channel in 0..4 {
            self.model
                .property(channel)
                .expect("four Invert properties")
                .remove_changed(&self.listener);
        }
    }
}
