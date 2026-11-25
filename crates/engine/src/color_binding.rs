//! Actual retained Color input/property receiver and explicit platform effects.
use crate::{
    ColorInputDelegate, ColorInputEventArgs, ColorInputHandler, ContextColorValue,
    SharedColorInput, SharedSolidColorProperty, SolidColorPropertyDelegate,
    SolidColorPropertyHandler,
    color_region::{CoreKind, CoreRoute, Member, SponsorAnchor, merge},
};
use std::sync::{Arc, Mutex, Weak};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColorBindingTimer {
    Changed,
    Quick,
}

/// The application owns real foreground/timer/dialog resources. Effects run
/// after engine guards are released; missing resources return the caller's E.
pub trait ColorBindingEffects<E>: Send + Sync {
    fn check_available(&self) -> Result<(), E>;
    fn now_local_ticks(&self) -> Result<i64, E>;
    fn timer_interval(&self, timer: ColorBindingTimer) -> Result<u64, E>;
    fn start_timer(
        &self,
        timer: ColorBindingTimer,
        interval_ms: u64,
        frame: SharedColorBinding<E>,
    ) -> Result<(), E>;
    fn stop_timer(&self, timer: ColorBindingTimer) -> Result<(), E>;
    fn dialog_visible(&self) -> Result<bool, E>;
    fn refresh(&self, frame: SharedColorBinding<E>) -> Result<(), E>;
    fn presentation_changed(&self) -> Result<(), E>;
    fn engine_failure(&self, message: String) -> E;
}
#[cfg(test)]
#[path = "color_binding_tests.rs"]
mod tests;
pub(crate) struct BindingCore<E> {
    pub(crate) route: CoreRoute<E>,
    state: Mutex<BindingState<E>>,
    effects: Arc<dyn ColorBindingEffects<E>>,
}
struct BindingState<E> {
    input: crate::color_region::CoreId,
    property: Option<(crate::color_region::CoreId, crate::SolidColorProperty)>,
    listening: bool,
    stamp: i64,
    property_handler: Option<SolidColorPropertyDelegate<E>>,
    input_handler: Option<ColorInputDelegate<E>>,
}
pub struct SharedColorBinding<E> {
    pub(crate) core: Arc<BindingCore<E>>,
    pub(crate) anchor: Arc<SponsorAnchor<E>>,
}
impl<E> Clone for SharedColorBinding<E> {
    fn clone(&self) -> Self {
        Self {
            core: Arc::clone(&self.core),
            anchor: Arc::clone(&self.anchor),
        }
    }
}
impl<E: Send + Sync + std::fmt::Display + 'static> SharedColorBinding<E> {
    pub fn new(
        input: &SharedColorInput<E>,
        effects: Arc<dyn ColorBindingEffects<E>>,
    ) -> Result<Self, E> {
        let stamp = effects.now_local_ticks()?;
        let core = Arc::new(BindingCore {
            route: CoreRoute::new(CoreKind::Binding),
            effects,
            state: Mutex::new(BindingState {
                input: input.core.route.id,
                property: None,
                listening: true,
                stamp,
                property_handler: None,
                input_handler: None,
            }),
        });
        let anchor = SponsorAnchor::unpublished();
        anchor.publish(Member::Binding(Arc::clone(&core)));
        merge(&anchor, &input.anchor);
        let weak = Arc::downgrade(&core);
        let c = SolidColorPropertyHandler::new(move |_, _| {
            let frame = Self::from_route(&weak)?;
            let listening = frame
                .core
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .listening;
            if listening {
                frame.core.effects.refresh(frame.clone())?;
            }
            Ok(())
        });
        let weak = Arc::downgrade(&core);
        let d = ColorInputHandler::new(move |_, _| Self::from_route(&weak)?.input_changed());
        {
            let mut state = core.state.lock().unwrap_or_else(|e| e.into_inner());
            state.property_handler = Some(SolidColorPropertyDelegate::from_handlers(&[c]));
            state.input_handler = Some(ColorInputDelegate::from_handlers(&[d]));
        }
        Ok(Self { core, anchor })
    }
    fn from_route(weak: &Weak<BindingCore<E>>) -> Result<Self, E> {
        // A subscribed sender/frame retains the actual region member. There is
        // no arbitrary public identity or successful weak-only receiver loss.
        let core = weak.upgrade().expect("subscribed binding member retained");
        let anchor = SponsorAnchor::for_binding(&core);
        Ok(Self { core, anchor })
    }
    pub fn input(&self) -> SharedColorInput<E> {
        let id = self
            .core
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .input;
        self.anchor.input_handle(id)
    }
    pub fn property(&self) -> Option<SharedSolidColorProperty<E>> {
        let locator = self
            .core
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .property;
        locator.map(|(id, kind)| self.anchor.property_handle(id, kind))
    }
    pub fn set_listening(&self, value: bool) {
        self.core
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .listening = value;
    }
    pub fn set_property(&self, property: Option<&SharedSolidColorProperty<E>>) -> Result<(), E> {
        let handler = self
            .core
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .property_handler
            .as_ref()
            .expect("stable receiver")
            .clone();
        if let Some(old) = self.property() {
            old.remove(&handler)
                .map_err(|e| self.core.effects.engine_failure(e.to_string()))?;
        }
        if let Some(new) = property {
            merge(&self.anchor, new.binding_anchor());
        }
        self.core
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .property = property.map(|p| p.binding_locator());
        if let Some(new) = property {
            new.add(&handler);
        }
        self.refresh_current()
    }
    pub fn refresh_current(&self) -> Result<(), E> {
        let d = self
            .core
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .input_handler
            .as_ref()
            .expect("stable receiver")
            .clone();
        let input = self.input();
        input.remove_input_changed(&d);
        if let Some(property) = self.property() {
            let value = property
                .color_value()
                .map_err(|e| self.core.effects.engine_failure(e.to_string()))?;
            input
                .set_value(&value)
                .map_err(|e| self.core.effects.engine_failure(e.to_string()))?;
            property
                .stamp_color_input_reference(Some(&input))
                .map_err(|e| self.core.effects.engine_failure(e.to_string()))?;
            input.add_input_changed(&d);
        }
        self.core.effects.presentation_changed()
    }
    /// Captured input value plus real input event, with no direct property write.
    pub fn submit_input(&self, value: &ContextColorValue<E>) -> Result<(), E> {
        self.core.effects.check_available()?;
        let input = self.input();
        input
            .set_value(value)
            .map_err(|e| self.core.effects.engine_failure(e.to_string()))?;
        input.raise_input_changed(Some(Arc::new(ColorInputEventArgs::new())))
    }
    fn input_changed(&self) -> Result<(), E> {
        let now = self.core.effects.now_local_ticks()?;
        let old = self
            .core
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .stamp;
        let elapsed = (now - old) as f64 * 0.0001;
        let threshold = self
            .core
            .effects
            .timer_interval(ColorBindingTimer::Changed)?;
        let timer = if elapsed < threshold as f64 {
            self.core.effects.stop_timer(ColorBindingTimer::Quick)?;
            self.core.effects.stop_timer(ColorBindingTimer::Changed)?;
            ColorBindingTimer::Changed
        } else {
            ColorBindingTimer::Quick
        };
        let interval = self.core.effects.timer_interval(timer)?;
        // Original Beta mi.a swallows each Start exception, including both.
        if self
            .core
            .effects
            .start_timer(timer, interval, self.clone())
            .is_err()
        {
            let _ = self.core.effects.start_timer(timer, interval, self.clone());
        }
        let stamp = self.core.effects.now_local_ticks()?;
        self.core
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .stamp = stamp;
        Ok(())
    }
    pub fn timer_fired(&self, timer: ColorBindingTimer) -> Result<(), E> {
        self.core.effects.stop_timer(timer)?;
        if let Some(property) = self.property() {
            let value = self.input().value();
            let temporary = self.core.effects.dialog_visible()?;
            property
                .set_color_value_temporary(&value, temporary)
                .map_err(|e| self.core.effects.engine_failure(e.to_string()))?;
        }
        self.core.effects.presentation_changed()
    }
}
