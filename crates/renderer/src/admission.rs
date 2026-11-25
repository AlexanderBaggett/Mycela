//! Process-wide accounting for frame allocations and submitted GPU work.
//!
//! Each reservation records a conservative logical byte estimate. The actual
//! driver allocation and dedicated GPU memory are not observable here. A frame
//! that returns before its submitted work completes remains reserved until the
//! real queue completion callback and the last local frame owners are gone.
use crate::SceneRenderError;
use std::{
    cell::RefCell,
    collections::BTreeMap,
    sync::{Arc, Mutex, OnceLock, mpsc},
    time::Duration,
};

#[derive(Default)]
struct Ledger {
    next: u64,
    entries: BTreeMap<u64, Entry>,
}
#[derive(Default)]
struct Entry {
    bytes: u64,
    submitted: bool,
    owner_gone: bool,
    completed: bool,
    readback_done: bool,
    device: Option<wgpu::Device>,
    submission: Option<wgpu::SubmissionIndex>,
}
fn ledger() -> &'static Arc<Mutex<Ledger>> {
    static LEDGER: OnceLock<Arc<Mutex<Ledger>>> = OnceLock::new();
    LEDGER.get_or_init(|| Arc::new(Mutex::new(Ledger::default())))
}
fn lock(ledger: &Mutex<Ledger>) -> std::sync::MutexGuard<'_, Ledger> {
    ledger.lock().unwrap_or_else(|error| error.into_inner())
}
fn total<E>(state: &Ledger) -> Result<u64, SceneRenderError<E>> {
    state
        .entries
        .values()
        .try_fold(0u64, |sum, e| sum.checked_add(e.bytes))
        .ok_or(SceneRenderError::InvalidGeometry)
}
/// Before a new frame admission, reap callbacks from actual abandoned devices.
/// Poll outside the ledger lock because completion callbacks acquire it.
pub(super) fn poll_abandoned() {
    let devices: Vec<_> = lock(ledger())
        .entries
        .values()
        .filter(|entry| entry.owner_gone && !(entry.completed && entry.readback_done))
        .filter_map(|entry| entry.device.clone())
        .collect();
    for device in devices {
        let _ = device.poll(wgpu::PollType::Poll);
    }
}
thread_local! { static TEMPORARIES: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) }; }
/// A renderer mesh preparation scope holds all of its transient preflights
/// until its CPU buffers and corresponding GPU uploads have both been created.
pub(super) struct Temporary {
    reservation: Reservation,
}
impl Temporary {
    pub(super) fn begin<E>() -> Result<Self, SceneRenderError<E>> {
        let reservation = Reservation::begin()?;
        TEMPORARIES.with(|stack| stack.borrow_mut().push(reservation.id));
        Ok(Self { reservation })
    }
}
impl Drop for Temporary {
    fn drop(&mut self) {
        TEMPORARIES.with(|stack| assert_eq!(stack.borrow_mut().pop(), Some(self.reservation.id)));
    }
}
/// Non-frame transient preflights reserve atomically for the active scope.
/// Direct calls outside a render have no retained scope and only check capacity.
pub(super) fn admit_extra<E>(bytes: u64) -> Result<(), SceneRenderError<E>> {
    let active = TEMPORARIES.with(|stack| stack.borrow().last().copied());
    let available =
        mycela_system_memory::available_physical_bytes().map_err(SceneRenderError::MemoryQuery)?;
    let mut state = lock(ledger());
    let current = total::<E>(&state)?;
    let needed = current
        .checked_add(bytes)
        .ok_or(SceneRenderError::InvalidGeometry)?;
    if needed > available {
        return Err(SceneRenderError::InsufficientMemory);
    }
    if let Some(id) = active {
        let entry = state
            .entries
            .get_mut(&id)
            .expect("live temporary reservation");
        entry.bytes = entry
            .bytes
            .checked_add(bytes)
            .ok_or(SceneRenderError::InvalidGeometry)?;
    }
    Ok(())
}
pub(super) struct Reservation {
    ledger: Arc<Mutex<Ledger>>,
    id: u64,
}
impl Reservation {
    pub(super) fn begin<E>() -> Result<Self, SceneRenderError<E>> {
        let ledger = Arc::clone(ledger());
        let id = {
            let mut state = lock(&ledger);
            let id = state.next;
            state.next = id.checked_add(1).ok_or(SceneRenderError::InvalidGeometry)?;
            state.entries.insert(id, Entry::default());
            id
        };
        Ok(Self { ledger, id })
    }
    fn raise_with_available<E>(
        &self,
        bytes: u64,
        available: u64,
    ) -> Result<(), SceneRenderError<E>> {
        let mut state = lock(&self.ledger);
        let old = state.entries.get(&self.id).expect("live reservation").bytes;
        let new = old.max(bytes);
        let requested = total::<E>(&state)?
            .checked_sub(old)
            .and_then(|n| n.checked_add(new))
            .ok_or(SceneRenderError::InvalidGeometry)?;
        if requested > available {
            return Err(SceneRenderError::InsufficientMemory);
        }
        state
            .entries
            .get_mut(&self.id)
            .expect("live reservation")
            .bytes = new;
        Ok(())
    }
    pub(super) fn raise<E>(&self, bytes: u64) -> Result<(), SceneRenderError<E>> {
        let available = mycela_system_memory::available_physical_bytes()
            .map_err(SceneRenderError::MemoryQuery)?;
        self.raise_with_available(bytes, available)
    }
    pub(super) fn add<E>(&self, bytes: u64) -> Result<(), SceneRenderError<E>> {
        let current = lock(&self.ledger)
            .entries
            .get(&self.id)
            .expect("live reservation")
            .bytes;
        self.raise(
            current
                .checked_add(bytes)
                .ok_or(SceneRenderError::InvalidGeometry)?,
        )
    }
    /// Associate a callback with the actual command buffer before submitting it.
    /// Completion never releases the reservation while its frame owners exist.
    pub(super) fn prepare_submit(&self, encoder: &wgpu::CommandEncoder, device: &wgpu::Device) {
        {
            let mut state = lock(&self.ledger);
            let entry = state.entries.get_mut(&self.id).expect("live reservation");
            entry.submitted = true;
            entry.device = Some(device.clone());
        }
        let ledger = Arc::clone(&self.ledger);
        let id = self.id;
        encoder.on_submitted_work_done(move || complete(&ledger, id));
    }
    pub(super) fn readback_callback(&self) -> impl FnOnce() + Send + 'static {
        let ledger = Arc::clone(&self.ledger);
        let id = self.id;
        move || readback_complete(&ledger, id)
    }
    pub(super) fn submitted(&self, submission: wgpu::SubmissionIndex) {
        if let Some(entry) = lock(&self.ledger).entries.get_mut(&self.id) {
            entry.submission = Some(submission);
        }
    }
}
fn complete(ledger: &Mutex<Ledger>, id: u64) {
    let mut state = lock(ledger);
    if let Some(entry) = state.entries.get_mut(&id) {
        entry.completed = true;
        if entry.owner_gone && entry.readback_done {
            state.entries.remove(&id);
        }
    }
}
fn readback_complete(ledger: &Mutex<Ledger>, id: u64) {
    let mut state = lock(ledger);
    if let Some(entry) = state.entries.get_mut(&id) {
        entry.readback_done = true;
        if entry.owner_gone && entry.completed {
            state.entries.remove(&id);
        }
    }
}
struct RetireJob {
    ledger: Arc<Mutex<Ledger>>,
    id: u64,
    device: wgpu::Device,
}
fn retire_sender() -> &'static Option<mpsc::Sender<RetireJob>> {
    static SENDER: OnceLock<Option<mpsc::Sender<RetireJob>>> = OnceLock::new();
    SENDER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<RetireJob>();
        std::thread::Builder::new()
            .name("mycela-gpu-retire".into())
            .spawn(move || {
                let mut jobs: Vec<RetireJob> = Vec::new();
                loop {
                    match rx.recv_timeout(Duration::from_millis(20)) {
                        Ok(job) => jobs.push(job),
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                    while let Ok(job) = rx.try_recv() {
                        jobs.push(job);
                    }
                    jobs.retain(|job| {
                        if !lock(&job.ledger).entries.contains_key(&job.id) {
                            return false;
                        }
                        // Callbacks acquire the ledger, so never hold its guard here.
                        if job.device.poll(wgpu::PollType::Poll).is_err() {
                            return false;
                        }
                        lock(&job.ledger).entries.contains_key(&job.id)
                    });
                }
            })
            .ok()
            .map(|_| tx)
    })
}
impl Drop for Reservation {
    fn drop(&mut self) {
        let abandoned = {
            let mut state = lock(&self.ledger);
            let Some(entry) = state.entries.get_mut(&self.id) else {
                return;
            };
            entry.owner_gone = true;
            if !entry.submitted || (entry.completed && entry.readback_done) {
                state.entries.remove(&self.id);
                None
            } else {
                entry.device.clone().zip(entry.submission.clone())
            }
        };
        if let Some((device, _submission)) = abandoned {
            // One process-wide coordinator polls actual devices; cancellation
            // storms cannot create an unbounded number of retirement threads.
            let job = RetireJob {
                ledger: Arc::clone(&self.ledger),
                id: self.id,
                device,
            };
            if let Some(sender) = retire_sender() {
                let _ = sender.send(job);
            }
            // Failure keeps the device and reservation for a later render's
            // nonblocking poll; it is never reported as completed on timeout.
        }
    }
}

#[cfg(test)]
pub(super) fn submitted_frames() -> (usize, u64) {
    let state = lock(ledger());
    state
        .entries
        .values()
        .filter(|entry| entry.submitted)
        .fold((0, 0u64), |(count, bytes), entry| {
            (count + 1, bytes.saturating_add(entry.bytes))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn local() -> Arc<Mutex<Ledger>> {
        Arc::new(Mutex::new(Ledger::default()))
    }
    fn entry(ledger: &Arc<Mutex<Ledger>>, id: u64) -> Reservation {
        lock(ledger).entries.insert(id, Entry::default());
        Reservation {
            ledger: Arc::clone(ledger),
            id,
        }
    }
    #[test]
    fn active_frames_are_admitted_together_and_failure_does_not_change_reservation() {
        let ledger = local();
        let one = entry(&ledger, 1);
        let two = entry(&ledger, 2);
        one.raise_with_available::<()>(60, 100).unwrap();
        assert!(matches!(
            two.raise_with_available::<()>(41, 100),
            Err(SceneRenderError::InsufficientMemory)
        ));
        assert_eq!(total::<()>(&lock(&ledger)).unwrap(), 60);
        two.raise_with_available::<()>(40, 100).unwrap();
        drop(one);
        assert_eq!(total::<()>(&lock(&ledger)).unwrap(), 40);
    }
    #[test]
    fn concurrent_frame_admissions_serialize_their_real_reservations() {
        let ledger = local();
        let a = entry(&ledger, 1);
        let b = entry(&ledger, 2);
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let start_a = Arc::clone(&barrier);
        let start_b = Arc::clone(&barrier);
        let first = std::thread::spawn(move || {
            start_a.wait();
            let ok = a.raise_with_available::<()>(60, 100).is_ok();
            (a, ok)
        });
        let second = std::thread::spawn(move || {
            start_b.wait();
            let ok = b.raise_with_available::<()>(60, 100).is_ok();
            (b, ok)
        });
        barrier.wait();
        let (a, ok_a) = first.join().unwrap();
        let (b, ok_b) = second.join().unwrap();
        assert_ne!(ok_a, ok_b);
        assert_eq!(total::<()>(&lock(&ledger)).unwrap(), 60);
        drop(a);
        drop(b);
    }
    #[test]
    fn callback_and_owner_retirement_are_both_required_in_either_order() {
        let ledger = local();
        let one = entry(&ledger, 1);
        one.raise_with_available::<()>(30, 100).unwrap();
        lock(&ledger).entries.get_mut(&1).unwrap().submitted = true;
        complete(&ledger, 1);
        assert_eq!(total::<()>(&lock(&ledger)).unwrap(), 30);
        readback_complete(&ledger, 1);
        assert_eq!(total::<()>(&lock(&ledger)).unwrap(), 30);
        drop(one);
        assert_eq!(total::<()>(&lock(&ledger)).unwrap(), 0);
        let two = entry(&ledger, 2);
        two.raise_with_available::<()>(50, 100).unwrap();
        lock(&ledger).entries.get_mut(&2).unwrap().submitted = true;
        drop(two);
        assert_eq!(total::<()>(&lock(&ledger)).unwrap(), 50);
        complete(&ledger, 2);
        assert_eq!(total::<()>(&lock(&ledger)).unwrap(), 50);
        readback_complete(&ledger, 2);
        assert_eq!(total::<()>(&lock(&ledger)).unwrap(), 0);
        let three = entry(&ledger, 3);
        three.raise_with_available::<()>(70, 100).unwrap();
        lock(&ledger).entries.get_mut(&3).unwrap().submitted = true;
        readback_complete(&ledger, 3);
        drop(three);
        assert_eq!(total::<()>(&lock(&ledger)).unwrap(), 70);
        complete(&ledger, 3);
        assert_eq!(total::<()>(&lock(&ledger)).unwrap(), 0);
    }
}
