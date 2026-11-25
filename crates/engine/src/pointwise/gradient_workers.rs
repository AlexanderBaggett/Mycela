//! Scoped execution of independent Gradient partitions.
use crate::{FloatBuffer, ImageError, PointwiseError, ProceduralExecution};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, RecvTimeoutError},
    },
    thread,
    time::Duration,
};

#[derive(Debug, PartialEq)]
pub(super) enum WorkerError {
    Image(ImageError),
    Index,
    DivideByZero,
    Cancelled,
}

pub(super) type WorkerResult<T = ()> = Result<T, WorkerError>;

impl From<ImageError> for WorkerError {
    fn from(error: ImageError) -> Self {
        Self::Image(error)
    }
}

impl<E> From<WorkerError> for PointwiseError<E> {
    fn from(error: WorkerError) -> Self {
        match error {
            WorkerError::Image(error) => Self::Image(error),
            WorkerError::Index => Self::Index,
            WorkerError::DivideByZero => Self::DivideByZero,
            WorkerError::Cancelled => Self::Cancelled,
        }
    }
}

pub(super) const BLOCK_PIXELS: usize = 256;

pub(super) struct PixelBlock {
    pixels: [f32; BLOCK_PIXELS],
    start: usize,
    stride: usize,
    len: usize,
}

impl PixelBlock {
    pub(super) fn new(stride: usize) -> Self {
        Self {
            pixels: [0.; BLOCK_PIXELS],
            start: 0,
            stride,
            len: 0,
        }
    }

    fn flush(&mut self, buffer: &FloatBuffer) -> WorkerResult {
        buffer.write_block(self.start, self.stride, &self.pixels[..self.len])?;
        self.len = 0;
        Ok(())
    }
}

pub(super) struct PixelSink<'a> {
    buffer: &'a FloatBuffer,
    block: Option<&'a mut PixelBlock>,
}

impl<'a> PixelSink<'a> {
    pub(super) fn new(buffer: &'a FloatBuffer, block: Option<&'a mut PixelBlock>) -> Self {
        Self { buffer, block }
    }

    pub(super) fn write(&mut self, index: usize, value: f32) -> WorkerResult {
        let Some(block) = &mut self.block else {
            return Ok(self.buffer.write(index, value)?);
        };
        if block.len != 0 && index != block.start + block.len * block.stride {
            block.flush(self.buffer)?;
        }
        if block.len == 0 {
            block.start = index;
        }
        block.pixels[block.len] = value;
        block.len += 1;
        if block.len == BLOCK_PIXELS {
            block.flush(self.buffer)?;
        }
        Ok(())
    }

    pub(super) fn finish(self) -> WorkerResult {
        if let Some(block) = self.block {
            block.flush(self.buffer)?;
        }
        Ok(())
    }
}

pub(super) fn worker_count(
    execution: ProceduralExecution,
    width: i32,
    height: i32,
    pixels: usize,
) -> usize {
    let ProceduralExecution::WorkerPool { processor_count } = execution else {
        return 1;
    };
    if width <= 0
        || height <= 0
        || width.checked_mul(height).map(|area| area as usize) != Some(pixels)
    {
        // Preserve the worker's ordered errors for wrapped and malformed geometry.
        return 1;
    }
    // Starting native threads is useful only when there is enough work per
    // worker. Bound this per render so large previews don't create a thread
    // for every logical core or every source partition.
    const MIN_PIXELS_PER_WORKER: usize = 65_536;
    const MAX_WORKERS: usize = 8;
    (pixels / MIN_PIXELS_PER_WORKER)
        .clamp(1, MAX_WORKERS)
        .min(processor_count.max(1) as usize)
}

struct StopOnDrop<'a>(&'a AtomicBool);
impl Drop for StopOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

pub(super) fn run_parallel(
    ranges: &[(i32, i32)],
    worker_count: usize,
    cancelled: &dyn Fn() -> bool,
    render_range: &(impl Fn((i32, i32), &dyn Fn() -> bool) -> WorkerResult + Sync),
) -> WorkerResult {
    if cancelled() {
        return Err(WorkerError::Cancelled);
    }
    let next = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    let (sender, receiver) = mpsc::channel();
    thread::scope(|scope| {
        let _guard = StopOnDrop(&stop);
        let mut started = 0;
        for _ in 0..worker_count {
            let sender = sender.clone();
            let (next, stop) = (&next, &stop);
            let worker = move || {
                let stopped = || stop.load(Ordering::Relaxed);
                while !stopped() {
                    let Some(&range) = ranges.get(next.fetch_add(1, Ordering::Relaxed)) else {
                        break;
                    };
                    if let Err(error) = render_range(range, &stopped) {
                        stop.store(true, Ordering::Relaxed);
                        let _ = sender.send(error);
                        break;
                    }
                }
            };
            if thread::Builder::new().spawn_scoped(scope, worker).is_err() {
                break;
            }
            started += 1;
        }
        drop(sender);
        if started == 0 {
            for &range in ranges {
                render_range(range, cancelled)?;
            }
            return Ok(());
        }
        let mut caller_cancelled = false;
        let mut worker_failure = None;
        // Only the caller invokes the borrowed predicate. It need not be Sync
        // and it runs without any image or model lock held.
        loop {
            if !caller_cancelled && cancelled() {
                caller_cancelled = true;
                stop.store(true, Ordering::Relaxed);
            }
            match receiver.recv_timeout(Duration::from_millis(4)) {
                Ok(WorkerError::Cancelled) | Err(RecvTimeoutError::Timeout) => {}
                Ok(error) => {
                    worker_failure.get_or_insert(error);
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        if caller_cancelled {
            Err(WorkerError::Cancelled)
        } else {
            worker_failure.map_or(Ok(()), Err)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn gradient_parallel_workers_poll_cancellation_on_the_caller_and_join() {
        let caller = thread::current().id();
        let entered = AtomicBool::new(false);
        let exited = AtomicBool::new(false);
        let polls = Cell::new(0);
        let cancelled = || {
            assert_eq!(thread::current().id(), caller);
            polls.set(polls.get() + 1);
            entered.load(Ordering::Acquire)
        };
        let result = run_parallel(&[(0, 1), (1, 1)], 2, &cancelled, &|_, stopped| {
            assert_ne!(thread::current().id(), caller);
            entered.store(true, Ordering::Release);
            while !stopped() {
                thread::park_timeout(Duration::from_millis(1));
            }
            exited.store(true, Ordering::Release);
            Err(WorkerError::Cancelled)
        });
        assert_eq!(result, Err(WorkerError::Cancelled));
        assert!(polls.get() > 1);
        assert!(exited.load(Ordering::Acquire));
    }
}
