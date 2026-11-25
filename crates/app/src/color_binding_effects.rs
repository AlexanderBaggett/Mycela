use crate::ForegroundError;
use chrono::{Datelike, Timelike};
use gpui_kit::{BackgroundExecutor, Task};
use mycela_engine::{ColorBindingEffects, ColorBindingTimer, SharedColorBinding};
use std::{
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    thread::ThreadId,
    time::Duration,
};

pub(crate) enum BindingJob {
    Refresh(SharedColorBinding<ForegroundError>),
    Tick(ColorBindingTimer, u64, SharedColorBinding<ForegroundError>),
    Present,
    EffectPresent(Weak<AtomicBool>),
    Failure(ForegroundError),
}
#[derive(Default)]
struct TimerSlot {
    generation: u64,
    task: Option<Task<()>>,
}
pub(crate) struct BindingRuntime {
    sender: async_channel::Sender<BindingJob>,
    background: BackgroundExecutor,
    timers: Mutex<[Option<TimerSlot>; 2]>,
    open: AtomicBool,
    dialog_visible: AtomicBool,
    foreground: ThreadId,
    physical_closed: Arc<AtomicBool>,
    receiver: async_channel::Receiver<BindingJob>,
    failures: Mutex<Vec<ForegroundError>>,
}
struct Endpoint {
    runtime: Weak<BindingRuntime>,
    physical_closed: Arc<AtomicBool>,
}
impl BindingRuntime {
    pub(crate) fn new(
        background: BackgroundExecutor,
    ) -> (Arc<Self>, async_channel::Receiver<BindingJob>) {
        let (sender, receiver) = async_channel::unbounded();
        (
            Arc::new(Self {
                sender,
                background,
                timers: Mutex::new([None, None]),
                open: AtomicBool::new(true),
                dialog_visible: AtomicBool::new(false),
                foreground: std::thread::current().id(),
                physical_closed: Arc::new(AtomicBool::new(false)),
                receiver: receiver.clone(),
                failures: Mutex::new(Vec::new()),
            }),
            receiver,
        )
    }
    pub(crate) fn request_effect_presentation(
        &self,
        active: Weak<AtomicBool>,
    ) -> Result<(), ForegroundError> {
        if !self.open.load(Ordering::Acquire) {
            return Err(ForegroundError::WindowClosed);
        }
        self.sender
            .try_send(BindingJob::EffectPresent(active))
            .map_err(|_| ForegroundError::OwnerClosed)
    }
    pub(crate) fn endpoint(self: &Arc<Self>) -> Arc<dyn ColorBindingEffects<ForegroundError>> {
        Arc::new(Endpoint {
            runtime: Arc::downgrade(self),
            physical_closed: Arc::clone(&self.physical_closed),
        })
    }
    fn ix(timer: ColorBindingTimer) -> usize {
        match timer {
            ColorBindingTimer::Changed => 0,
            ColorBindingTimer::Quick => 1,
        }
    }
    fn stop(&self, timer: ColorBindingTimer) {
        let retired = {
            let mut slots = self.timers.lock().unwrap_or_else(|e| e.into_inner());
            let s = slots[Self::ix(timer)].get_or_insert_with(TimerSlot::default);
            s.generation = s.generation.wrapping_add(1);
            s.task.take()
        };
        drop(retired);
    }
    fn start(
        self: &Arc<Self>,
        timer: ColorBindingTimer,
        ms: u64,
        frame: SharedColorBinding<ForegroundError>,
    ) -> Result<(), ForegroundError> {
        if !self.open.load(Ordering::Acquire) {
            return Err(ForegroundError::WindowClosed);
        }
        let mut slots = self.timers.lock().unwrap_or_else(|e| e.into_inner());
        let s = slots[Self::ix(timer)].get_or_insert_with(TimerSlot::default);
        if s.task.is_some() {
            return Ok(());
        } // Enabled Start is not a restart.
        s.generation = s.generation.wrapping_add(1);
        let generation = s.generation;
        let weak = Arc::downgrade(self);
        // Register the actual deadline at Start, not when this task is polled.
        let wait = self.background.timer(Duration::from_millis(ms));
        s.task =
            Some(self.background.spawn(async move {
                wait.await;
                if let Some(runtime) = weak.upgrade() {
                    if let Err(error) = runtime
                        .sender
                        .try_send(BindingJob::Tick(timer, generation, frame))
                    {
                        let _ = runtime.sender.try_send(BindingJob::Failure(
                            ForegroundError::Operation(error.to_string()),
                        ));
                    }
                }
            }));
        Ok(())
    }
    pub(crate) fn is_current(&self, timer: ColorBindingTimer, generation: u64) -> bool {
        self.open.load(Ordering::Acquire)
            && self.timers.lock().unwrap_or_else(|e| e.into_inner())[Self::ix(timer)]
                .as_ref()
                .is_some_and(|s| s.task.is_some() && s.generation == generation)
    }
    pub(crate) fn is_dialog_visible(&self) -> bool {
        self.dialog_visible.load(Ordering::Acquire)
    }
    pub(crate) fn set_dialog_visible(&self, visible: bool) {
        self.dialog_visible.store(visible, Ordering::Release);
    }
    pub(crate) fn retire_pending_input(&self) {
        self.stop(ColorBindingTimer::Changed);
        self.stop(ColorBindingTimer::Quick);
        while self.receiver.try_recv().is_ok() {}
    }
    pub(crate) fn shutdown(&self) {
        self.open.store(false, Ordering::Release);
        self.sender.close();
        self.stop(ColorBindingTimer::Changed);
        self.stop(ColorBindingTimer::Quick);
        while self.receiver.try_recv().is_ok() {}
    }
    pub(crate) fn retain_failure(&self, error: ForegroundError) {
        self.failures
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(error);
    }
    pub(crate) fn window_closed(&self) {
        self.physical_closed.store(true, Ordering::Release);
        self.shutdown();
    }
}
impl Drop for BindingRuntime {
    fn drop(&mut self) {
        self.shutdown();
    }
}
impl Endpoint {
    fn runtime(&self) -> Result<Arc<BindingRuntime>, ForegroundError> {
        if self.physical_closed.load(Ordering::Acquire) {
            return Err(ForegroundError::WindowClosed);
        }
        let runtime = self.runtime.upgrade().ok_or(ForegroundError::OwnerClosed)?;
        if !runtime.open.load(Ordering::Acquire) {
            return Err(ForegroundError::WindowClosed);
        }
        Ok(runtime)
    }
}
impl ColorBindingEffects<ForegroundError> for Endpoint {
    fn check_available(&self) -> Result<(), ForegroundError> {
        self.runtime().map(|_| ())
    }
    fn now_local_ticks(&self) -> Result<i64, ForegroundError> {
        self.runtime()?;
        let now = chrono::Local::now().naive_local();
        Ok(i64::from(now.num_days_from_ce() - 1) * 864_000_000_000
            + i64::from(now.num_seconds_from_midnight()) * 10_000_000
            + i64::from(now.nanosecond() / 100))
    }
    fn timer_interval(&self, timer: ColorBindingTimer) -> Result<u64, ForegroundError> {
        let runtime = self.runtime()?;
        runtime.timers.lock().unwrap_or_else(|e| e.into_inner())[BindingRuntime::ix(timer)]
            .get_or_insert_with(TimerSlot::default);
        Ok(match timer {
            ColorBindingTimer::Changed => 300,
            ColorBindingTimer::Quick => 30,
        })
    }
    fn start_timer(
        &self,
        timer: ColorBindingTimer,
        ms: u64,
        frame: SharedColorBinding<ForegroundError>,
    ) -> Result<(), ForegroundError> {
        self.runtime()?.start(timer, ms, frame)
    }
    fn stop_timer(&self, timer: ColorBindingTimer) -> Result<(), ForegroundError> {
        self.runtime()?.stop(timer);
        Ok(())
    }
    fn dialog_visible(&self) -> Result<bool, ForegroundError> {
        Ok(self.runtime()?.dialog_visible.load(Ordering::Acquire))
    }
    fn refresh(&self, frame: SharedColorBinding<ForegroundError>) -> Result<(), ForegroundError> {
        let runtime = self.runtime()?;
        if std::thread::current().id() == runtime.foreground {
            frame.refresh_current()
        } else {
            runtime
                .sender
                .try_send(BindingJob::Refresh(frame))
                .map_err(|_| ForegroundError::OwnerClosed)
        }
    }
    fn presentation_changed(&self) -> Result<(), ForegroundError> {
        self.runtime()?
            .sender
            .try_send(BindingJob::Present)
            .map_err(|_| ForegroundError::OwnerClosed)
    }
    fn engine_failure(&self, message: String) -> ForegroundError {
        ForegroundError::Operation(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::TestAppContext;
    use mycela_engine::{SharedColorInput, SharedSolidColorNode, SolidColorCloneMode};
    #[gpui_kit::test]
    async fn start_deadline_precedes_poll_and_duplicate_start_does_not_reset(
        cx: &mut TestAppContext,
    ) {
        let (runtime, receiver) = BindingRuntime::new(cx.background_executor.clone());
        let input = SharedColorInput::new_uncreated();
        let binding = SharedColorBinding::new(&input, runtime.endpoint()).unwrap();
        let node = SharedSolidColorNode::new_unattached(SolidColorCloneMode::Original).unwrap();
        binding.set_property(Some(&node.color_property())).unwrap();
        while receiver.try_recv().is_ok() {}
        runtime
            .start(ColorBindingTimer::Changed, 300, binding.clone())
            .unwrap();
        cx.background_executor
            .advance_clock(Duration::from_millis(200));
        runtime
            .start(ColorBindingTimer::Changed, 300, binding.clone())
            .unwrap();
        runtime
            .start(ColorBindingTimer::Quick, 30, binding)
            .unwrap();
        cx.background_executor
            .advance_clock(Duration::from_millis(31));
        cx.run_until_parked();
        assert!(matches!(
            receiver.try_recv(),
            Ok(BindingJob::Tick(ColorBindingTimer::Quick, _, _))
        ));
        assert!(receiver.try_recv().is_err());
        cx.background_executor
            .advance_clock(Duration::from_millis(70));
        cx.run_until_parked();
        assert!(matches!(
            receiver.try_recv(),
            Ok(BindingJob::Tick(ColorBindingTimer::Changed, _, _))
        ));
        assert!(receiver.try_recv().is_err());
        runtime.shutdown();
    }
}
