//! Immutable read capabilities for owned spill generations. Names are unique
//! and never rewritten by this storage implementation. External inode tampering
//! is outside this private-cache contract; it is not a native concurrency proof.
use super::*;

pub(super) struct SpillFile {
    id: u64,
    file: Mutex<Option<File>>,
}
impl SpillFile {
    pub(super) fn open(path: &Path) -> Result<Arc<Self>, ImportedImageError> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Ok(Arc::new(Self {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            file: Mutex::new(Some(File::open(path)?)),
        }))
    }
    pub(super) fn id(&self) -> u64 {
        self.id
    }
    #[cfg(test)]
    pub(super) fn lock_reader_for_test(&self) -> std::sync::MutexGuard<'_, Option<File>> {
        guard(&self.file)
    }
    pub(super) fn read(&self) -> Result<Vec<u8>, ImportedImageError> {
        // Only workers/legacy synchronous capture enter this IO lock. Foreground
        // freeze clones Arc<SpillFile> without touching this mutex.
        let mut held = guard(&self.file);
        let file = held.as_mut().ok_or(ImportedImageError::ObjectDisposed)?;
        file.seek(SeekFrom::Start(0))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(bytes)
    }
}
impl Drop for SpillFile {
    fn drop(&mut self) {
        if let Some(file) = self
            .file
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            retire::submit(retire::Resource::File(file));
        }
    }
}
/// Narrow Rust lifetime normalization, independent of source explicit Dispose.
/// This worker only closes owned capture files and removes owned spill paths.
/// It never runs render work, source callbacks, pressure policy or shared-temp
/// scans. Sending/dropping a cancelled pending frame performs no filesystem IO.
pub(super) mod retire {
    use super::*;
    use std::sync::mpsc::{self, Sender};
    pub(crate) enum Resource {
        File(File),
        Path(PathBuf),
        #[cfg(test)]
        Barrier(std::sync::mpsc::Sender<std::thread::ThreadId>),
    }
    fn release(value: Resource) {
        match value {
            Resource::File(file) => drop(file),
            Resource::Path(path) => {
                let _ = fs::remove_file(path);
            }
            #[cfg(test)]
            Resource::Barrier(sender) => {
                let _ = sender.send(std::thread::current().id());
            }
        }
    }
    #[cfg(test)]
    pub(crate) fn flush_for_test() -> std::thread::ThreadId {
        let (sender, receiver) = mpsc::channel();
        submit(Resource::Barrier(sender));
        receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
    }
    pub(in crate::encoded_image::storage) fn submit(value: Resource) {
        static QUEUE: OnceLock<Sender<Resource>> = OnceLock::new();
        let sender = QUEUE.get_or_init(|| {
            let (sender, receiver) = mpsc::channel();
            // Thread resource exhaustion follows std::thread::spawn's explicit
            // process failure; never report successful cleanup while leaking it.
            let _ = std::thread::spawn(move || {
                for value in receiver {
                    release(value);
                }
            });
            sender
        });
        if let Err(value) = sender.send(value) {
            let _ = std::thread::spawn(move || release(value.0));
        }
    }
}
