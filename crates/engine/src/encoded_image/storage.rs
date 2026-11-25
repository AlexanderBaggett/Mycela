//! Qualified StorableArray/auv ownership, weak queues and byte spill protocol.
//! CLR collection is represented by real weak-root sweeps and deterministic
//! Arc/Drop release, not a claim about CLR finalizer scheduling or native OOM.
use super::{ByteBuffer, ImportedImageError, guard};
use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    sync::{Arc, Mutex, OnceLock, Weak},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy, Debug)]
pub enum EncodedStorageNotice {
    InsufficientDiskSpace { available_mebibytes: i32 },
    WriteDiagnostic(ImportedImageError),
    WriteFailed(ImportedImageError),
}
/// The real application's lazy memory predicate and disk observation. Calls
/// occur without queue/state guards. No success-valued default provider exists.
pub trait EncodedStorageEffects: Send + Sync {
    fn tick_count(&self) -> Result<i32, ImportedImageError>;
    fn capacity_available(&self, width: i32, quality: i32) -> Result<bool, ImportedImageError>;
    fn available_disk_bytes(&self, directory: &Path) -> Result<u64, ImportedImageError>;
    fn report(&self, notice: EncodedStorageNotice) -> Result<(), ImportedImageError>;
    /// The unqualified emergency/sleep/pressure/OOM retry path must be explicit.
    fn recover_allocation(&self, _width: i32) -> Result<(), ImportedImageError> {
        Err(ImportedImageError::UnsupportedCapability(
            "LowMemoryEmergency/OOM retry scheduling",
        ))
    }
}
#[derive(Clone)]
pub struct EncodedStorageEnvironment {
    root: PathBuf,
    effects: Arc<dyn EncodedStorageEffects>,
}
impl EncodedStorageEnvironment {
    /// Dedicated, caller-owned cache directory, never a shared temporary root.
    pub fn new(
        root: PathBuf,
        effects: Arc<dyn EncodedStorageEffects>,
    ) -> Result<Self, ImportedImageError> {
        if !root.is_absolute() {
            return Err(ImportedImageError::Argument);
        }
        Ok(Self { root, effects })
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
}
#[derive(Default)]
struct Queue {
    entries: Vec<Weak<Store>>,
    additions: i32,
}
impl Queue {
    fn add(&mut self, value: &Arc<Store>) {
        if self
            .entries
            .iter()
            .any(|v| v.ptr_eq(&Arc::downgrade(value)) && v.strong_count() != 0)
        {
            return;
        }
        self.entries.push(Arc::downgrade(value));
        self.additions = self.additions.wrapping_add(1);
        if self.additions > 1000 {
            self.clean();
        }
    }
    fn remove(&mut self, value: &Arc<Store>) {
        let target = Arc::downgrade(value);
        self.entries
            .retain(|v| v.strong_count() == 0 || !v.ptr_eq(&target));
    }
    fn clean(&mut self) {
        self.entries.retain(|v| v.strong_count() != 0);
        self.additions = 0;
    }
    fn snapshot(&mut self) -> Vec<Weak<Store>> {
        self.clean();
        self.entries.clone()
    }
}
#[derive(Default)]
struct Process {
    nonstorable: Vec<Weak<Store>>,
    eligible: Queue,
    disabled: bool,
    error_reported: bool,
    environment: Option<EncodedStorageEnvironment>,
}
static PROCESS: OnceLock<Mutex<Process>> = OnceLock::new();
fn process() -> &'static Mutex<Process> {
    PROCESS.get_or_init(|| Mutex::new(Process::default()))
}
fn environment() -> Result<EncodedStorageEnvironment, ImportedImageError> {
    guard(process())
        .environment
        .clone()
        .ok_or(ImportedImageError::UnsupportedCapability(
            "encoded storage disk/memory environment is not installed",
        ))
}
fn register(value: &Arc<Store>) {
    // Collect the weak list first: never hold a process guard while inspecting
    // a store. Source concurrent races are not promoted to a portability law.
    let old = guard(process()).nonstorable.clone();
    let mut retained = Vec::new();
    let mut remaining = old.into_iter();
    let mut found = false;
    while let Some(weak) = remaining.next() {
        if let Some(store) = weak.upgrade() {
            if Arc::ptr_eq(&store, value) {
                found = true;
                retained.push(weak);
                retained.extend(remaining);
                break;
            }
            if !guard(&store.state).sealed {
                retained.push(weak);
            }
        }
    }
    if !found {
        retained.push(Arc::downgrade(value));
    }
    guard(process()).nonstorable = retained;
}
fn eligible(length: i32) -> bool {
    (f64::from(length).sqrt() as i32) > 1150
}

type CaptureLease = Option<Result<Arc<SpillFile>, ImportedImageError>>;

struct StoreState {
    disposed: bool,
    sealed: bool,
    temporary: bool,
    stored: bool,
    resident: Option<ByteBuffer>,
    weak: Weak<Mutex<Arc<Vec<u8>>>>,
    path: Option<PathBuf>,
    detached_path_present: bool,
    detached_file: Option<Arc<Vec<u8>>>,
    // A detached frame owns the captured live weak target. This does not pin
    // the live caller array or share a live spill file.
    _captured_weak_anchor: Option<ByteBuffer>,
    capture_file: CaptureLease,
    busy: bool,
}
struct Store {
    length: i32,
    state: Mutex<StoreState>,
}
impl Drop for Store {
    fn drop(&mut self) {
        let state = self.state.get_mut().unwrap_or_else(|e| e.into_inner());
        if state.stored
            && let Some(path) = state.path.take()
        {
            retire::submit(retire::Resource::Path(path));
        }
    }
}
#[path = "spill_generation.rs"]
mod generation;
use generation::{SpillFile, retire};
#[derive(Clone)]
pub struct EncodedByteStore(Arc<Store>);
impl fmt::Debug for EncodedByteStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EncodedByteStore({:p})", Arc::as_ptr(&self.0))
    }
}
/// Side-effect-free owned raw state for frame comparison, never a public
/// retrieval/unseal. A spilled payload is read without ArrayReference changes.
#[derive(Clone, Debug)]
pub struct EncodedStorageSnapshot {
    pub length: i32,
    pub disposed: bool,
    pub sealed: bool,
    pub temporary: bool,
    pub stored: bool,
    pub has_file_name: bool,
    pub resident: Option<ByteBuffer>,
    pub weak: Option<ByteBuffer>,
    pub file_bytes: Option<Vec<u8>>,
}
impl EncodedByteStore {
    pub(crate) fn resident_bytes(&self) -> Option<ByteBuffer> {
        guard(&self.0.state).resident.clone()
    }

    pub fn install_environment(environment: EncodedStorageEnvironment) {
        guard(process()).environment = Some(environment);
    }
    /// The actual auv constructor: register only; no allocation preflight,
    /// disk write, resident substitution or automatic pressure success.
    pub fn from_bytes(bytes: ByteBuffer) -> Result<Self, ImportedImageError> {
        let length = i32::try_from(bytes.len()).map_err(|_| {
            ImportedImageError::UnsupportedCapability("CLR Byte[] length exceeds Int32")
        })?;
        if length == 0 {
            return Err(ImportedImageError::Argument);
        }
        let result = Self(Arc::new(Store {
            length,
            state: Mutex::new(StoreState {
                disposed: false,
                sealed: false,
                temporary: false,
                stored: false,
                resident: Some(bytes),
                weak: Weak::new(),
                path: None,
                detached_path_present: false,
                detached_file: None,
                _captured_weak_anchor: None,
                capture_file: None,
                busy: false,
            }),
        }));
        if eligible(length) {
            register(&result.0);
        }
        Ok(result)
    }
    /// mn+a constructs auv and then calls b(false), in this order.
    pub(crate) fn new(bytes: ByteBuffer) -> Result<Self, ImportedImageError> {
        let result = Self::from_bytes(bytes)?;
        result.seal(false);
        Ok(result)
    }
    fn set_sealed(&self, value: bool) {
        let changed = {
            let mut s = guard(&self.0.state);
            if s.sealed == value {
                false
            } else {
                s.sealed = value;
                true
            }
        };
        if changed && eligible(self.0.length) {
            if value {
                guard(process()).eligible.add(&self.0);
            } else {
                guard(process()).eligible.remove(&self.0);
            }
        }
    }
    pub fn seal(&self, temporary: bool) {
        let change = {
            let s = guard(&self.0.state);
            !s.sealed || s.temporary
        };
        if change {
            self.set_sealed(true);
            guard(&self.0.state).temporary = temporary;
        }
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub fn length(&self) -> i32 {
        self.0.length
    }
    pub fn is_sealed(&self) -> bool {
        guard(&self.0.state).sealed
    }
    pub fn is_temporarily_sealed(&self) -> bool {
        guard(&self.0.state).temporary
    }
    pub fn is_stored(&self) -> bool {
        guard(&self.0.state).stored
    }
    pub fn is_disposed(&self) -> bool {
        guard(&self.0.state).disposed
    }
    /// mn's actual getter passes readonly=true.
    pub fn bytes(&self) -> Result<ByteBuffer, ImportedImageError> {
        self.read_range(0, self.0.length, true)
    }
    pub fn read_range(
        &self,
        start: i32,
        end: i32,
        readonly: bool,
    ) -> Result<ByteBuffer, ImportedImageError> {
        {
            let s = guard(&self.0.state);
            if s.disposed {
                return Err(ImportedImageError::ObjectDisposed);
            }
            if s.busy {
                return Err(ImportedImageError::UnsupportedCapability(
                    "concurrent/reentrant encoded store operation",
                ));
            }
        }
        let count = end.wrapping_sub(start);
        let unseal = {
            let s = guard(&self.0.state);
            s.sealed && s.temporary && !readonly
        };
        if unseal {
            self.set_sealed(false);
            if eligible(self.0.length) {
                register(&self.0);
            }
        }
        let (stored, weak, resident, path, detached) = {
            let s = guard(&self.0.state);
            (
                s.stored,
                s.weak.upgrade(),
                s.resident.clone(),
                s.path.clone(),
                s.detached_file.clone(),
            )
        };
        let full = start == 0 && end == self.0.length;
        let result = if stored {
            if full && weak.is_some() {
                ByteBuffer(weak.ok_or(ImportedImageError::NullReference)?)
            } else {
                let mut values = safely_allocate(count)?;
                // auv.q opens and seeks BEFORE BinaryReader.Read validates the
                // destination offset; retain its nonzero-start offset bug.
                if let Some(bytes) = detached {
                    if start < 0 {
                        return Err(ImportedImageError::ArgumentOutOfRange);
                    }
                    let offset = usize::try_from(start)
                        .map_err(|_| ImportedImageError::ArgumentOutOfRange)?;
                    let num = values.len();
                    let destination = values
                        .get_mut(
                            offset
                                ..offset
                                    .checked_add(num)
                                    .ok_or(ImportedImageError::Argument)?,
                        )
                        .ok_or(ImportedImageError::Argument)?;
                    let source = bytes.get(offset..).unwrap_or(&[]);
                    let n = source.len().min(destination.len());
                    destination[..n].copy_from_slice(&source[..n]);
                } else {
                    let mut file = File::open(path.ok_or(ImportedImageError::NullReference)?)?;
                    if start < 0 {
                        return Err(ImportedImageError::ArgumentOutOfRange);
                    }
                    file.seek(SeekFrom::Start(start as u64))?;
                    let offset = usize::try_from(start)
                        .map_err(|_| ImportedImageError::ArgumentOutOfRange)?;
                    let num = values.len();
                    let destination = values
                        .get_mut(
                            offset
                                ..offset
                                    .checked_add(num)
                                    .ok_or(ImportedImageError::Argument)?,
                        )
                        .ok_or(ImportedImageError::Argument)?;
                    let _ = file.read(destination)?;
                }
                let result = ByteBuffer::new(values);
                if full {
                    guard(&self.0.state).weak = Arc::downgrade(&result.0);
                }
                result
            }
        } else if full {
            resident.ok_or(ImportedImageError::UnsupportedCapability(
                "null resident array after a native spill/flush fault",
            ))?
        } else {
            let source = resident.ok_or(ImportedImageError::ArgumentNull)?;
            let mut values = safely_allocate(count)?;
            let source = guard(&source.0);
            if start < 0 {
                return Err(ImportedImageError::ArgumentOutOfRange);
            }
            let start = start as usize;
            let range = source
                .get(
                    start
                        ..start
                            .checked_add(values.len())
                            .ok_or(ImportedImageError::Argument)?,
                )
                .ok_or(ImportedImageError::Argument)?;
            values.copy_from_slice(range);
            ByteBuffer::new(values)
        };
        if stored && self.is_temporarily_sealed() && result.len() == self.0.length as usize {
            self.cleanup();
            let mut s = guard(&self.0.state);
            s.temporary = false;
            s.resident = Some(result.clone());
        }
        Ok(result)
    }
    fn cleanup(&self) {
        let path = {
            let s = guard(&self.0.state);
            if !s.stored {
                return;
            }
            s.path.clone()
        };
        if let Some(path) = path {
            let _ = fs::remove_file(path);
        }
        let mut s = guard(&self.0.state);
        s.stored = false;
        s.detached_file = None;
        s.capture_file = None;
        s.detached_path_present = false;
        s.path = None;
    }
    pub fn dispose(&self) {
        let first = {
            let mut s = guard(&self.0.state);
            if s.disposed {
                false
            } else {
                s.disposed = true;
                true
            }
        };
        if first {
            self.set_sealed(false);
            self.cleanup();
        }
    }
    /// Real explicit h() entry. Queue dispatch applies the capacity predicate
    /// before reaching this operation; callers may invoke it directly too.
    pub fn store_now(&self) -> Result<(), ImportedImageError> {
        if guard(process()).disabled {
            return Ok(());
        }
        {
            let mut s = guard(&self.0.state);
            if s.disposed || s.stored {
                return Ok(());
            }
            if s.busy {
                return Err(ImportedImageError::UnsupportedCapability(
                    "concurrent/reentrant encoded spill",
                ));
            }
            s.busy = true;
        }
        let result = self.store_body();
        guard(&self.0.state).busy = false;
        result
    }
    fn store_body(&self) -> Result<(), ImportedImageError> {
        let env = environment()?;
        fs::create_dir_all(&env.root)?;
        // StorableArray.b creates and discards actual awm() here. Retain its
        // clock observation and seeded constructor (including Abs(MIN) error)
        // BEFORE any filename field is assigned.
        let _unused = crate::seeded_random::SeededRandom::from_seed(env.effects.tick_count()?)
            .map_err(|_| ImportedImageError::Overflow)?;
        let path = next_file(&env.root);
        guard(&self.0.state).path = Some(path.clone());
        let available = env.effects.available_disk_bytes(&env.root)?;
        let mib = crate::framework_cast::integer((available as f64) / 1_048_576.0);
        if mib < 500 {
            env.effects
                .report(EncodedStorageNotice::InsufficientDiskSpace {
                    available_mebibytes: mib,
                })?;
            guard(process()).disabled = true;
            return Ok(());
        }
        guard(process()).eligible.remove(&self.0);
        let resident = {
            let mut s = guard(&self.0.state);
            s.stored = true;
            let resident = s.resident.clone();
            s.weak = resident
                .as_ref()
                .map(|v| Arc::downgrade(&v.0))
                .unwrap_or_default();
            resident
        };
        let write_result = (|| -> Result<(), ImportedImageError> {
            // Create-new on an owned collision-resistant name is the portable
            // equivalent of source's Exists loop followed by Append.
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
            let values = resident.as_ref().ok_or(ImportedImageError::ArgumentNull)?;
            let generation = values.generation();
            file.write_all(generation.as_slice())?;
            guard(&self.0.state).resident = None;
            // Source clears the array before disposing BinaryWriter/FileStream.
            file.flush()?;
            // Capture capability acquisition cannot rewrite source write success.
            // Its error is returned only by a later freeze/capture operation.
            let capture_file = SpillFile::open(&path);
            guard(&self.0.state).capture_file = Some(capture_file);
            Ok(())
        })();
        // Release the transient strong root before the real collection sweep.
        drop(resident);
        if let Err(error) = write_result {
            env.effects
                .report(EncodedStorageNotice::WriteDiagnostic(error))?;
            let report = {
                let mut p = guard(process());
                if p.error_reported {
                    false
                } else {
                    p.error_reported = true;
                    true
                }
            };
            if report {
                env.effects
                    .report(EncodedStorageNotice::WriteFailed(error))?;
            }
            guard(&self.0.state).stored = false;
        }
        Self::sweep();
        Ok(())
    }
    /// StorableArray.a(width,oi): first enlarged capacity/collection, then the
    /// original predicate and ordered weak receiver dispatch. Receiver ev tests
    /// capacity again before h; callbacks never execute under queue guards.
    pub fn preflight(width: i32, quality: i32) -> Result<(), ImportedImageError> {
        let enlarged = crate::framework_cast::integer(f64::from(width) * 1.5);
        if !capacity(enlarged, quality)? {
            Self::sweep();
        }
        if !capacity(width, quality)? {
            let queue = guard(process()).eligible.snapshot();
            for receiver in queue {
                if let Some(store) = receiver.upgrade()
                    && !capacity(width, quality)?
                {
                    Self(store).store_now()?;
                }
            }
        }
        Ok(())
    }
    /// Genuine weak-root pruning; expired owners/files have already been
    /// released by Drop. Not an assertion that a CLR global GC was performed.
    pub fn sweep() {
        let old = guard(process()).nonstorable.clone();
        let retained = old
            .into_iter()
            .filter(|v| v.upgrade().is_some_and(|s| !guard(&s.state).sealed))
            .collect();
        let mut p = guard(process());
        p.nonstorable = retained;
        p.eligible.clean();
    }
    pub fn queue_counts() -> (usize, usize) {
        Self::sweep();
        let p = guard(process());
        (p.nonstorable.len(), p.eligible.entries.len())
    }
    // Freeze only memory operands; never acquire the file reader's mutex.
    fn memory_snapshot(
        &self,
    ) -> Result<(EncodedStorageSnapshot, CaptureLease), ImportedImageError> {
        let s = guard(&self.0.state);
        if s.busy {
            return Err(ImportedImageError::UnsupportedCapability(
                "capture during encoded spill",
            ));
        }
        Ok((
            EncodedStorageSnapshot {
                length: self.0.length,
                disposed: s.disposed,
                sealed: s.sealed,
                temporary: s.temporary,
                stored: s.stored,
                has_file_name: s.path.is_some() || s.detached_path_present,
                resident: s.resident.clone(),
                weak: s.weak.upgrade().map(ByteBuffer),
                file_bytes: s.detached_file.as_ref().map(|v| v.as_ref().clone()),
            },
            s.capture_file.clone(),
        ))
    }
    pub fn raw_snapshot(&self) -> Result<EncodedStorageSnapshot, ImportedImageError> {
        let (mut raw, lease) = self.memory_snapshot()?;
        if raw.stored && raw.file_bytes.is_none() {
            let path = guard(&self.0.state).path.clone();
            raw.file_bytes = Some(if let Some(path) = path {
                fs::read(path)?
            } else {
                lease
                    .ok_or(ImportedImageError::UnsupportedCapability(
                        "encoded spill has no immutable capture lease",
                    ))??
                    .read()?
            });
        }
        Ok(raw)
    }
    pub(crate) fn frozen_snapshot(
        &self,
    ) -> Result<(EncodedStorageSnapshot, Option<u64>), ImportedImageError> {
        let (raw, lease) = self.memory_snapshot()?;
        let id = if raw.stored && raw.file_bytes.is_none() {
            Some(
                lease
                    .ok_or(ImportedImageError::UnsupportedCapability(
                        "encoded spill has no immutable capture lease",
                    ))??
                    .id(),
            )
        } else {
            None
        };
        Ok((raw, id))
    }
    pub(crate) fn freeze_with(
        &self,
        mut copy: impl FnMut(&ByteBuffer) -> ByteBuffer,
    ) -> Result<Self, ImportedImageError> {
        let (raw, lease) = self.memory_snapshot()?;
        // Validate capability while still memory-only; actual bytes are read by
        // materialize_frozen on a worker, even when the decoded cache is warm.
        if raw.stored && raw.file_bytes.is_none() {
            lease
                .as_ref()
                .ok_or(ImportedImageError::UnsupportedCapability(
                    "encoded spill has no immutable capture lease",
                ))?
                .as_ref()
                .map_err(|e| *e)?;
        }
        let resident = raw.resident.as_ref().map(&mut copy);
        let weak = raw.weak.as_ref().map(copy);
        Ok(Self(Arc::new(Store {
            length: raw.length,
            state: Mutex::new(StoreState {
                disposed: raw.disposed,
                sealed: raw.sealed,
                temporary: raw.temporary,
                stored: raw.stored,
                resident,
                weak: weak
                    .as_ref()
                    .map(|v| Arc::downgrade(&v.0))
                    .unwrap_or_default(),
                path: None,
                detached_path_present: raw.has_file_name,
                detached_file: raw.file_bytes.map(Arc::new),
                _captured_weak_anchor: weak,
                capture_file: lease,
                busy: false,
            }),
        })))
    }
    pub(crate) fn materialize_frozen(&self) -> Result<(), ImportedImageError> {
        let (raw, lease) = self.memory_snapshot()?;
        if raw.stored && raw.file_bytes.is_none() {
            let bytes = lease
                .ok_or(ImportedImageError::UnsupportedCapability(
                    "encoded spill has no immutable capture lease",
                ))??
                .read()?;
            let lease = {
                let mut s = guard(&self.0.state);
                s.detached_file = Some(Arc::new(bytes));
                s.capture_file.take()
            };
            drop(lease);
        }
        Ok(())
    }
    pub(crate) fn capture_with(
        &self,
        mut copy: impl FnMut(&ByteBuffer) -> ByteBuffer,
    ) -> Result<Self, ImportedImageError> {
        let raw = self.raw_snapshot()?;
        let resident = raw.resident.as_ref().map(&mut copy);
        let weak = raw.weak.as_ref().map(copy);
        Ok(Self(Arc::new(Store {
            length: raw.length,
            state: Mutex::new(StoreState {
                disposed: raw.disposed,
                sealed: raw.sealed,
                temporary: raw.temporary,
                stored: raw.stored,
                resident,
                weak: weak
                    .as_ref()
                    .map(|v| Arc::downgrade(&v.0))
                    .unwrap_or_default(),
                path: None,
                detached_path_present: raw.has_file_name,
                detached_file: raw.file_bytes.map(Arc::new),
                _captured_weak_anchor: weak,
                capture_file: None,
                busy: false,
            }),
        })))
    }
}
fn capacity(width: i32, quality: i32) -> Result<bool, ImportedImageError> {
    if width < 500 {
        Ok(true)
    } else {
        environment()?.effects.capacity_available(width, quality)
    }
}
fn safely_allocate(count: i32) -> Result<Vec<u8>, ImportedImageError> {
    // Source preflight precedes newarr (including its negative-length error).
    EncodedByteStore::preflight(crate::framework_cast::integer(f64::from(count).sqrt()), 0)?;
    let count = usize::try_from(count).map_err(|_| ImportedImageError::Overflow)?;
    let mut result = Vec::new();
    if result.try_reserve_exact(count).is_err() {
        environment()?
            .effects
            .recover_allocation(crate::framework_cast::integer((count as f64).sqrt()))?;
        result
            .try_reserve_exact(count)
            .map_err(|_| ImportedImageError::Allocation)?;
    }
    result.resize(count, 0);
    Ok(result)
}
fn next_file(root: &Path) -> PathBuf {
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    loop {
        let serial = SERIAL.fetch_add(1, Ordering::Relaxed);
        let path = root.join(format!(
            "mycela-{}-{nanos}-{serial}.tmp",
            std::process::id()
        ));
        if !path.exists() {
            return path;
        }
    }
}

#[cfg(test)]
mod laws;
