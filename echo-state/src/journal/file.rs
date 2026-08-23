//! File-backed journal and checkpoint implementations.
//!
//! [`FileEventJournal`] persists one JSON line per record. On open it scans the
//! file, validates 1-based contiguous sequences, tolerates one torn trailing
//! record (partial write after a crash) by truncating it, and rejects gaps or
//! mid-file corruption loudly. [`FileCheckpointStore`] writes one atomic
//! snapshot file that pairs the reducer state with its applied sequence.

use super::{
    CheckpointFrame, CheckpointStore, EventJournal, JournalAppendReceipt, JournalDurabilityStatus,
    JournalEvent, JournalRecord,
};
use echo_core::error::{ReactError, Result};
use echo_core::utils::canonical_json::canonical_json_bytes;
use echo_core::utils::fs::{
    ExclusiveFileLease, FileDurability, append_existing, atomic_write, create_dir_all_durable,
    read_existing, read_existing_from, read_existing_lines_from, truncate_existing,
    try_exclusive_file_lease,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::any::TypeId;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

fn io_error(context: &str, error: std::io::Error) -> ReactError {
    ReactError::Other(format!("{context}: {error}"))
}

#[cfg(unix)]
fn sync_directory(directory: &Path) -> std::io::Result<()> {
    std::fs::File::open(directory)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_directory: &Path) -> std::io::Result<()> {
    Ok(())
}

fn finish_new_journal_creation(
    file: &std::fs::File,
    parent: &Path,
    sync_parent: impl FnOnce(&Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    file.sync_data()?;
    sync_parent(parent)
}

/// Scanned state of a journal file on open.
struct ScannedJournal {
    /// Byte length of the valid prefix (records plus their newlines).
    valid_len: u64,
    /// Next sequence a fresh append should assign.
    next_sequence: u64,
    /// Byte offset of every record, indexed by `sequence - 1`.
    record_offsets: Vec<u64>,
}

/// Parse the journal bytes and validate sequencing.
///
/// Returns the valid prefix length, the next sequence, and whether the file
/// ended with a torn record that should be truncated. An empty line in the
/// middle is corruption; a missing trailing newline only invalidates the last
/// record.
fn scan_journal<E: JournalEvent>(context: &str, bytes: &[u8]) -> Result<ScannedJournal> {
    let mut valid_len: u64 = 0;
    let mut next_sequence: u64 = 1;
    let mut offset: usize = 0;
    let mut record_offsets = Vec::new();
    while offset < bytes.len() {
        let Some(newline) = bytes[offset..].iter().position(|byte| *byte == b'\n') else {
            // Trailing chunk without a newline: a torn write. It is only
            // tolerated as the final record; drop it.
            break;
        };
        let line_end = offset
            .checked_add(newline)
            .ok_or_else(|| ReactError::Other(format!("{context}: journal byte offset overflow")))?;
        let line = &bytes[offset..line_end];
        let record: JournalRecord<E> = serde_json::from_slice(line).map_err(|error| {
            ReactError::Other(format!(
                "{context}: corrupt journal record at sequence {next_sequence}: {error}"
            ))
        })?;
        if record.sequence != next_sequence {
            return Err(ReactError::Other(format!(
                "{context}: journal sequence gap, expected {next_sequence} but found {}",
                record.sequence
            )));
        }
        record_offsets.push(u64::try_from(offset).map_err(|_| {
            ReactError::Other(format!("{context}: journal exceeds supported size"))
        })?);
        next_sequence = record
            .sequence
            .checked_add(1)
            .ok_or_else(|| ReactError::Other(format!("{context}: journal sequence exhausted")))?;
        let next_offset = line_end
            .checked_add(1)
            .ok_or_else(|| ReactError::Other(format!("{context}: journal byte offset overflow")))?;
        valid_len = u64::try_from(next_offset)
            .map_err(|_| ReactError::Other(format!("{context}: journal exceeds supported size")))?;
        offset = next_offset;
    }
    Ok(ScannedJournal {
        valid_len,
        next_sequence,
        record_offsets,
    })
}

#[derive(Debug)]
struct FileJournalState {
    next_sequence: u64,
    valid_len: u64,
    record_offsets: Vec<u64>,
    poison: Option<String>,
}

#[derive(Debug)]
struct SharedFileJournalState {
    event_type: TypeId,
    durability: FileDurability,
    state: Mutex<FileJournalState>,
    _lease: ExclusiveFileLease,
}

fn file_journal_registry() -> &'static Mutex<HashMap<PathBuf, Weak<SharedFileJournalState>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, Weak<SharedFileJournalState>>>> =
        OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn scan_and_repair_journal<E: JournalEvent>(
    path: &Path,
    context: &str,
    durability: FileDurability,
) -> Result<ScannedJournal> {
    match read_existing(path) {
        Ok(bytes) => {
            let scanned = scan_journal::<E>(context, &bytes)?;
            let file_len = u64::try_from(bytes.len()).map_err(|_| {
                ReactError::Other(format!("{context}: journal exceeds supported size"))
            })?;
            if scanned.valid_len < file_len {
                truncate_existing(path, scanned.valid_len, durability)
                    .map_err(|error| io_error(context, error))?;
            }
            Ok(scanned)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(ScannedJournal {
            valid_len: 0,
            next_sequence: 1,
            record_offsets: Vec::new(),
        }),
        Err(error) => Err(io_error(context, error)),
    }
}

/// JSONL-backed [`EventJournal`].
///
/// Appends serialize one [`JournalRecord`] per line with the caller-selected
/// durability policy; `Flush` survives process crashes, `SyncData` additionally
/// survives power loss. Open validates the complete file once and records byte
/// offsets; replay then seeks directly to the requested sequence suffix. A
/// canonical in-process authority shares sequencing across handles and holds a
/// process-lifetime exclusive lease against competing writers.
#[derive(Debug)]
pub struct FileEventJournal<E> {
    path: PathBuf,
    durability: FileDurability,
    shared: Arc<SharedFileJournalState>,
    #[cfg(test)]
    append_fault: Mutex<Option<AppendFault>>,
    _event: PhantomData<fn() -> E>,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy)]
enum AppendFault {
    FullWrite,
    PartialWrite { bytes: usize },
    UnrecognizedSuffix,
}

impl<E: JournalEvent> FileEventJournal<E> {
    /// Open (or create) the journal at `path`.
    ///
    /// A torn trailing record from an interrupted append is truncated; gaps
    /// and mid-file corruption are errors.
    pub fn open(path: impl Into<PathBuf>, durability: FileDurability) -> Result<Self> {
        let path = path.into();
        let context = format!("journal open {}", path.display());
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        create_dir_all_durable(parent).map_err(|error| io_error(&context, error))?;
        let canonical_parent =
            std::fs::canonicalize(parent).map_err(|error| io_error(&context, error))?;
        let file_name = path.file_name().ok_or_else(|| {
            ReactError::Other(format!("{context}: journal path has no file name"))
        })?;
        let path = canonical_parent.join(file_name);
        let context = format!("journal open {}", path.display());
        // Appends go through `append_existing`, which requires the target to
        // already be a regular file, so a fresh journal creates an empty one.
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => finish_new_journal_creation(&file, &canonical_parent, sync_directory)
                .map_err(|error| io_error(&context, error))?,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let metadata = std::fs::symlink_metadata(&path)
                    .map_err(|metadata_error| io_error(&context, metadata_error))?;
                if metadata.file_type().is_file() && metadata.len() == 0 {
                    append_existing(&path, b"", FileDurability::SyncData)
                        .map_err(|barrier_error| io_error(&context, barrier_error))?;
                    sync_directory(&canonical_parent)
                        .map_err(|barrier_error| io_error(&context, barrier_error))?;
                }
            }
            Err(error) => return Err(io_error(&context, error)),
        }
        let mut registry = file_journal_registry().lock().map_err(|error| {
            ReactError::Other(format!("journal registry lock poisoned: {error}"))
        })?;
        if let Some(shared) = registry.get(&path).and_then(Weak::upgrade) {
            if shared.event_type != TypeId::of::<E>() {
                return Err(ReactError::Other(format!(
                    "{context}: journal is already open with a different event type"
                )));
            }
            if shared.durability != durability {
                return Err(ReactError::Other(format!(
                    "{context}: journal is already open with a different durability configuration"
                )));
            }
            // Reopening is also an integrity boundary. Scan while holding the
            // shared append lock so a second handle cannot miss or race a torn
            // write, including one caused outside this process.
            let mut state = shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let scanned = scan_and_repair_journal::<E>(&path, &context, durability)?;
            state.next_sequence = scanned.next_sequence;
            state.valid_len = scanned.valid_len;
            state.record_offsets = scanned.record_offsets;
            state.poison = None;
            drop(state);
            return Ok(Self {
                path,
                durability,
                shared,
                #[cfg(test)]
                append_fault: Mutex::new(None),
                _event: PhantomData,
            });
        }
        let lease = try_exclusive_file_lease(&path).map_err(|error| io_error(&context, error))?;
        let scanned = scan_and_repair_journal::<E>(&path, &context, durability)?;
        let shared = Arc::new(SharedFileJournalState {
            event_type: TypeId::of::<E>(),
            durability,
            state: Mutex::new(FileJournalState {
                next_sequence: scanned.next_sequence,
                valid_len: scanned.valid_len,
                record_offsets: scanned.record_offsets,
                poison: None,
            }),
            _lease: lease,
        });
        registry.insert(path.clone(), Arc::downgrade(&shared));
        Ok(Self {
            path,
            durability,
            shared,
            #[cfg(test)]
            append_fault: Mutex::new(None),
            _event: PhantomData,
        })
    }

    /// Journal file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn append_line(&self, line: &[u8]) -> std::io::Result<()> {
        #[cfg(test)]
        if let Some(fault) = self
            .append_fault
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        {
            use std::io::Write;

            let mut file = std::fs::OpenOptions::new().append(true).open(&self.path)?;
            match fault {
                AppendFault::FullWrite => file.write_all(line)?,
                AppendFault::PartialWrite { bytes } => {
                    file.write_all(line.get(..bytes.min(line.len())).unwrap_or(line))?;
                }
                AppendFault::UnrecognizedSuffix => file.write_all(b"x")?,
            }
            file.flush()?;
            return Err(std::io::Error::other("injected append durability failure"));
        }

        append_existing(&self.path, line, self.durability)
    }

    /// Reconcile the suffix after an append error. `Some` means the complete
    /// record committed despite the durability error; `None` means no record
    /// committed and any partial suffix was removed.
    fn reconcile_failed_append(
        &self,
        state: &mut FileJournalState,
        line: &[u8],
        context: &str,
        append_error: &std::io::Error,
    ) -> Result<Option<JournalDurabilityStatus>> {
        let suffix = read_existing_from(&self.path, state.valid_len)
            .map_err(|error| io_error(context, error))?;
        if suffix == line {
            return Ok(Some(JournalDurabilityStatus::Degraded {
                error: append_error.to_string(),
            }));
        }
        if line.starts_with(&suffix) {
            if !suffix.is_empty() {
                truncate_existing(&self.path, state.valid_len, self.durability)
                    .map_err(|error| io_error(context, error))?;
            }
            return Ok(None);
        }
        Err(ReactError::Other(format!(
            "{context}: append error left an unrecognized {}-byte suffix",
            suffix.len()
        )))
    }
}

impl<E: JournalEvent> EventJournal<E> for FileEventJournal<E> {
    fn append(&self, event: E) -> Result<JournalAppendReceipt<E>> {
        // The lock spans encoding + write + advance so concurrent appends
        // cannot observe or reuse an in-flight sequence.
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(reason) = &state.poison {
            return Err(ReactError::Other(format!(
                "journal append {} refused because the handle is poisoned: {reason}; reopen the journal to recover",
                self.path.display()
            )));
        }
        let next_sequence = state.next_sequence.checked_add(1).ok_or_else(|| {
            ReactError::Other("journal sequence exhausted before append".to_string())
        })?;
        let record = JournalRecord {
            sequence: state.next_sequence,
            event: Arc::new(event),
        };
        let mut line = serde_json::to_vec(&record).map_err(|error| {
            ReactError::Other(format!("failed to encode journal record: {error}"))
        })?;
        line.push(b'\n');
        let line_len = u64::try_from(line.len())
            .map_err(|_| ReactError::Other("journal record exceeds supported size".to_string()))?;
        let valid_len = state.valid_len.checked_add(line_len).ok_or_else(|| {
            ReactError::Other("journal byte length exhausted before append".to_string())
        })?;
        let context = format!("journal append {}", self.path.display());
        let durability = match self.append_line(&line) {
            Ok(()) => JournalDurabilityStatus::Confirmed,
            Err(error) => match self.reconcile_failed_append(&mut state, &line, &context, &error) {
                Ok(Some(status)) => status,
                Ok(None) => return Err(io_error(&context, error)),
                Err(repair_error) => {
                    let reason =
                        format!("append failed ({error}); reconciliation failed ({repair_error})");
                    state.poison = Some(reason.clone());
                    return Err(ReactError::Other(format!(
                        "{context}: {reason}; handle poisoned until reopen"
                    )));
                }
            },
        };
        let record_offset = state.valid_len;
        state.record_offsets.push(record_offset);
        state.valid_len = valid_len;
        state.next_sequence = next_sequence;
        Ok(JournalAppendReceipt { record, durability })
    }

    fn next_sequence(&self) -> u64 {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .next_sequence
    }

    fn last_sequence(&self) -> u64 {
        self.next_sequence().saturating_sub(1)
    }

    fn replay_after(&self, after_sequence: u64, limit: usize) -> Result<Vec<JournalRecord<E>>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let context = format!("journal replay {}", self.path.display());
        // The state lock serializes replay with append and gives O(1) access to
        // the first byte after `after_sequence`. Open already validated the
        // complete prefix, so replay only decodes the requested suffix.
        let state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let last_sequence = state.next_sequence.saturating_sub(1);
        if after_sequence >= last_sequence {
            return Ok(Vec::new());
        }
        let offset_index = usize::try_from(after_sequence).map_err(|_| {
            ReactError::Other(format!("{context}: sequence exceeds supported index"))
        })?;
        let start_offset = state
            .record_offsets
            .get(offset_index)
            .copied()
            .ok_or_else(|| {
                ReactError::Other(format!(
                    "{context}: missing byte offset after sequence {after_sequence}"
                ))
            })?;
        let bytes = read_existing_lines_from(&self.path, start_offset, limit)
            .map_err(|error| io_error(&context, error))?;
        let mut expected = after_sequence
            .checked_add(1)
            .ok_or_else(|| ReactError::Other(format!("{context}: journal sequence exhausted")))?;
        let mut records = Vec::new();
        for line in bytes
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
        {
            let record: JournalRecord<E> = serde_json::from_slice(line).map_err(|error| {
                ReactError::Other(format!("{context}: corrupt journal record: {error}"))
            })?;
            if record.sequence != expected {
                return Err(ReactError::Other(format!(
                    "{context}: journal sequence gap, expected {expected} but found {}",
                    record.sequence
                )));
            }
            expected = expected.checked_add(1).ok_or_else(|| {
                ReactError::Other(format!("{context}: journal sequence exhausted"))
            })?;
            records.push(record);
            if records.len() >= limit {
                break;
            }
        }
        Ok(records)
    }
}

const CHECKPOINT_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Serialize)]
struct CheckpointIntegrity<'a> {
    schema_version: u16,
    sequence: u64,
    state: &'a serde_json::Value,
}

#[derive(Debug, Serialize)]
struct StoredCheckpointRef<'a> {
    schema_version: u16,
    sequence: u64,
    state: &'a serde_json::Value,
    digest: &'a str,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredCheckpoint {
    schema_version: u16,
    sequence: u64,
    state: serde_json::Value,
    digest: String,
}

fn checkpoint_digest(
    schema_version: u16,
    sequence: u64,
    state: &serde_json::Value,
) -> Result<String> {
    let bytes = canonical_json_bytes(&CheckpointIntegrity {
        schema_version,
        sequence,
        state,
    })
    .map_err(|error| {
        ReactError::Other(format!(
            "failed to encode checkpoint integrity input: {error}"
        ))
    })?;
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(digest.len().saturating_mul(2));
    for byte in digest {
        encoded.push(lower_hex_digit(byte >> 4));
        encoded.push(lower_hex_digit(byte & 0x0f));
    }
    Ok(encoded)
}

fn lower_hex_digit(nibble: u8) -> char {
    match nibble {
        0..=9 => char::from(b'0'.saturating_add(nibble)),
        _ => char::from(b'a'.saturating_add(nibble.saturating_sub(10))),
    }
}

fn save_checkpoint_with<S: Serialize>(
    path: &Path,
    state: &S,
    through_sequence: u64,
    create_parent: impl FnOnce(&Path) -> std::io::Result<()>,
    write: impl FnOnce(&Path, &[u8]) -> std::io::Result<()>,
) -> Result<()> {
    let state = serde_json::to_value(state).map_err(|error| {
        ReactError::Other(format!("failed to encode checkpoint state: {error}"))
    })?;
    let digest = checkpoint_digest(CHECKPOINT_SCHEMA_VERSION, through_sequence, &state)?;
    let frame = StoredCheckpointRef {
        schema_version: CHECKPOINT_SCHEMA_VERSION,
        sequence: through_sequence,
        state: &state,
        digest: &digest,
    };
    let bytes = serde_json::to_vec(&frame)
        .map_err(|error| ReactError::Other(format!("failed to encode checkpoint: {error}")))?;
    let context = format!("checkpoint save {}", path.display());
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    create_parent(parent).map_err(|error| io_error(&context, error))?;
    write(path, &bytes).map_err(|error| io_error(&context, error))
}

/// Single-file [`CheckpointStore`] written with atomic replace.
///
/// The snapshot is written to a temporary sibling and renamed, so readers
/// observe either the previous or the new checkpoint, never a partial file. A
/// missing file loads as `None`; a corrupt file is an error. The private disk
/// frame is schema-versioned and protected by a SHA-256 checksum so valid JSON
/// mutations cannot be mistaken for a trustworthy replay prefix. Sequence `0`
/// is valid and represents a snapshot taken before the first journal event.
#[derive(Debug)]
pub struct FileCheckpointStore<S> {
    path: PathBuf,
    _state: PhantomData<S>,
}

impl<S> FileCheckpointStore<S> {
    pub fn open(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            _state: PhantomData,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl<S: Serialize + DeserializeOwned + Send + Sync + 'static> CheckpointStore<S>
    for FileCheckpointStore<S>
{
    fn save(&self, state: &S, through_sequence: u64) -> Result<()> {
        save_checkpoint_with(
            &self.path,
            state,
            through_sequence,
            create_dir_all_durable,
            atomic_write,
        )
    }

    fn load(&self) -> Result<Option<CheckpointFrame<S>>> {
        let context = format!("checkpoint load {}", self.path.display());
        let bytes = match read_existing(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io_error(&context, error)),
        };
        let frame: StoredCheckpoint = serde_json::from_slice(&bytes).map_err(|error| {
            ReactError::Other(format!("{context}: corrupt checkpoint: {error}"))
        })?;
        if frame.schema_version != CHECKPOINT_SCHEMA_VERSION {
            return Err(ReactError::Other(format!(
                "{context}: unsupported checkpoint schema version {}; expected {CHECKPOINT_SCHEMA_VERSION}",
                frame.schema_version
            )));
        }
        let expected_digest =
            checkpoint_digest(frame.schema_version, frame.sequence, &frame.state)?;
        if frame.digest != expected_digest {
            return Err(ReactError::Other(format!(
                "{context}: checkpoint integrity digest mismatch"
            )));
        }
        let state = serde_json::from_value(frame.state).map_err(|error| {
            ReactError::Other(format!("{context}: corrupt checkpoint state: {error}"))
        })?;
        Ok(Some(CheckpointFrame {
            sequence: frame.sequence,
            state,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::super::{CheckpointApplyStatus, CheckpointedReducer, MemoryEventJournal};
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Default, Serialize, Deserialize, Debug, PartialEq, Eq)]
    struct LensReducer {
        applied: u64,
    }

    #[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
    struct WideIntegerState {
        min: i128,
        max: u128,
    }

    struct VisibleFailureCheckpointStore {
        inner: FileCheckpointStore<LensReducer>,
        fail_once: std::sync::atomic::AtomicBool,
    }

    impl VisibleFailureCheckpointStore {
        fn new(path: impl Into<PathBuf>) -> Self {
            Self {
                inner: FileCheckpointStore::open(path),
                fail_once: std::sync::atomic::AtomicBool::new(true),
            }
        }
    }

    impl CheckpointStore<LensReducer> for VisibleFailureCheckpointStore {
        fn save(&self, state: &LensReducer, through_sequence: u64) -> Result<()> {
            if self
                .fail_once
                .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                save_checkpoint_with(
                    self.inner.path(),
                    state,
                    through_sequence,
                    create_dir_all_durable,
                    |path, bytes| {
                        atomic_write(path, bytes)?;
                        Err(std::io::Error::other(
                            "injected parent sync failure after visible checkpoint rename",
                        ))
                    },
                )
            } else {
                self.inner.save(state, through_sequence)
            }
        }

        fn load(&self) -> Result<Option<CheckpointFrame<LensReducer>>> {
            self.inner.load()
        }
    }

    impl super::super::EventReducer for LensReducer {
        type Event = String;

        fn apply(&mut self, _event: &String) {
            self.applied = self.applied.saturating_add(1);
        }
    }

    fn temp_root() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "echo-journal-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn mutate_checkpoint(
        path: &Path,
        mutate: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>),
    ) {
        let bytes = std::fs::read(path).expect("read checkpoint fixture");
        let mut frame: serde_json::Value =
            serde_json::from_slice(&bytes).expect("decode checkpoint fixture");
        mutate(frame.as_object_mut().expect("checkpoint object"));
        let bytes = serde_json::to_vec(&frame).expect("encode mutated checkpoint");
        std::fs::write(path, bytes).expect("write mutated checkpoint");
    }

    #[test]
    fn file_journal_round_trips_and_resumes() {
        let root = temp_root();
        let path = root.join("events.jsonl");
        let journal =
            FileEventJournal::<String>::open(&path, FileDurability::Flush).expect("open journal");
        assert_eq!(journal.last_sequence(), 0);
        journal.append("one".to_string()).expect("append");
        journal.append("two".to_string()).expect("append");

        let reopened =
            FileEventJournal::<String>::open(&path, FileDurability::Flush).expect("reopen journal");
        assert_eq!(reopened.next_sequence(), 3);
        let records = reopened.replay_after(0, usize::MAX).expect("replay");
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].event.as_str(), "one");
        assert_eq!(records[1].sequence, 2);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn independent_handles_share_canonical_sequence_state_under_concurrent_append() {
        const APPENDS_PER_HANDLE: usize = 32;
        let root = temp_root();
        let path = root.join("events.jsonl");
        let first = std::sync::Arc::new(
            FileEventJournal::<String>::open(&path, FileDurability::Flush)
                .expect("open first handle"),
        );
        let second_path = root.join(".").join("events.jsonl");
        let second = std::sync::Arc::new(
            FileEventJournal::<String>::open(second_path, FileDurability::Flush)
                .expect("open second handle"),
        );
        assert!(std::sync::Arc::ptr_eq(&first.shared, &second.shared));
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let mut handles = Vec::new();
        for (label, journal) in [("a", first.clone()), ("b", second.clone())] {
            let barrier = barrier.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                let mut sequences = Vec::new();
                for index in 0..APPENDS_PER_HANDLE {
                    let receipt = journal.append(format!("{label}-{index}"))?;
                    sequences.push(receipt.record.sequence);
                }
                Ok::<Vec<u64>, ReactError>(sequences)
            }));
        }
        let mut assigned = Vec::new();
        for handle in handles {
            assigned.extend(
                handle
                    .join()
                    .expect("append thread")
                    .expect("concurrent append"),
            );
        }
        assigned.sort_unstable();
        let expected_count = APPENDS_PER_HANDLE.saturating_mul(2);
        let expected = (1..=u64::try_from(expected_count).unwrap_or(u64::MAX)).collect::<Vec<_>>();
        assert_eq!(assigned, expected);

        drop(first);
        drop(second);
        let reopened =
            FileEventJournal::<String>::open(&path, FileDurability::Flush).expect("reopen journal");
        assert_eq!(
            reopened.last_sequence(),
            u64::try_from(expected_count).unwrap_or(u64::MAX)
        );
        let records = reopened
            .replay_after(0, usize::MAX)
            .expect("replay reopened journal");
        assert_eq!(records.len(), expected_count);
        drop(reopened);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn full_record_with_durability_error_advances_sequence_as_degraded() {
        let root = temp_root();
        let path = root.join("events.jsonl");
        let journal =
            FileEventJournal::<String>::open(&path, FileDurability::SyncData).expect("open");
        *journal
            .append_fault
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(AppendFault::FullWrite);

        let first = journal.append("one".to_string()).expect("degraded append");
        assert_eq!(first.record.sequence, 1);
        assert!(matches!(
            first.durability,
            JournalDurabilityStatus::Degraded { .. }
        ));
        assert_eq!(journal.next_sequence(), 2);
        let second = journal.append("two".to_string()).expect("second append");
        assert_eq!(second.record.sequence, 2);
        assert_eq!(second.durability, JournalDurabilityStatus::Confirmed);

        drop(journal);
        let reopened =
            FileEventJournal::<String>::open(&path, FileDurability::Flush).expect("reopen");
        let records = reopened.replay_after(0, usize::MAX).expect("replay");
        assert_eq!(
            records
                .iter()
                .map(|record| record.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        drop(reopened);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn partial_append_error_truncates_suffix_before_sequence_reuse() {
        let root = temp_root();
        let path = root.join("events.jsonl");
        let journal = FileEventJournal::<String>::open(&path, FileDurability::Flush).expect("open");
        *journal
            .append_fault
            .lock()
            .unwrap_or_else(|error| error.into_inner()) =
            Some(AppendFault::PartialWrite { bytes: 9 });

        assert!(journal.append("partial".to_string()).is_err());
        assert_eq!(journal.next_sequence(), 1);
        assert!(read_existing(&path).expect("read repaired file").is_empty());
        let committed = journal.append("committed".to_string()).expect("retry");
        assert_eq!(committed.record.sequence, 1);

        drop(journal);
        let reopened =
            FileEventJournal::<String>::open(&path, FileDurability::Flush).expect("reopen");
        let records = reopened.replay_after(0, usize::MAX).expect("replay");
        assert_eq!(records.len(), 1);
        assert_eq!(
            records.first().map(|record| record.event.as_str()),
            Some("committed")
        );
        drop(reopened);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn unreconciled_append_error_poisons_handle_until_reopen() {
        let root = temp_root();
        let path = root.join("events.jsonl");
        let journal = FileEventJournal::<String>::open(&path, FileDurability::Flush).expect("open");
        *journal
            .append_fault
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(AppendFault::UnrecognizedSuffix);

        let first_error = journal
            .append("first".to_string())
            .expect_err("unrecognized suffix must fail");
        assert!(first_error.to_string().contains("handle poisoned"));
        let second_error = journal
            .append("second".to_string())
            .expect_err("poisoned handle must reject append");
        assert!(second_error.to_string().contains("handle is poisoned"));

        drop(journal);
        let reopened = FileEventJournal::<String>::open(&path, FileDurability::Flush)
            .expect("repair on reopen");
        let committed = reopened.append("recovered".to_string()).expect("append");
        assert_eq!(committed.record.sequence, 1);
        drop(reopened);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn torn_trailing_record_is_truncated_on_open() {
        let root = temp_root();
        let path = root.join("events.jsonl");
        let journal =
            FileEventJournal::<String>::open(&path, FileDurability::Flush).expect("open journal");
        journal.append("one".to_string()).expect("append");
        // Simulate a torn append: a partial line without the newline.
        let good = read_existing(&path).expect("read");
        let mut torn = good.clone();
        torn.extend_from_slice(b"{\"sequence\":2,\"event\":\"par");
        std::fs::write(&path, &torn).expect("write torn");

        let reopened =
            FileEventJournal::<String>::open(&path, FileDurability::Flush).expect("reopen journal");
        assert_eq!(reopened.next_sequence(), 2);
        assert_eq!(read_existing(&path).expect("read").len(), good.len());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn mid_file_corruption_is_an_error() {
        let root = temp_root();
        let path = root.join("events.jsonl");
        let journal =
            FileEventJournal::<String>::open(&path, FileDurability::Flush).expect("open journal");
        journal.append("one".to_string()).expect("append");
        journal.append("two".to_string()).expect("append");
        journal.append("three".to_string()).expect("append");
        let contents = read_existing(&path).expect("read");
        let lines: Vec<&[u8]> = contents
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .collect();
        let mut corrupted = Vec::new();
        let corrupt: &[u8] = b"not json at all";
        for line in [lines.first(), Some(&corrupt), lines.get(2)]
            .into_iter()
            .flatten()
        {
            corrupted.extend_from_slice(line);
            corrupted.push(b'\n');
        }
        std::fs::write(&path, corrupted).expect("write corrupted");

        let error = FileEventJournal::<String>::open(&path, FileDurability::Flush)
            .expect_err("corruption must fail");
        assert!(error.to_string().contains("corrupt journal record"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn sequence_gap_is_an_error() {
        let root = temp_root();
        let path = root.join("events.jsonl");
        std::fs::write(
            &path,
            b"{\"sequence\":1,\"event\":\"a\"}\n{\"sequence\":3,\"event\":\"c\"}\n",
        )
        .expect("write gap");
        let error = FileEventJournal::<String>::open(&path, FileDurability::Flush)
            .expect_err("gap must fail");
        assert!(error.to_string().contains("sequence gap"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn checkpointed_reducer_recovers_from_files() {
        let root = temp_root();
        let journal = std::sync::Arc::new(
            FileEventJournal::<String>::open(root.join("events.jsonl"), FileDurability::Flush)
                .expect("open journal"),
        );
        let checkpoints = std::sync::Arc::new(FileCheckpointStore::<LensReducer>::open(
            root.join("checkpoint.json"),
        ));
        let reducer = CheckpointedReducer::new(std::sync::Arc::clone(&journal), checkpoints, 2);
        for index in 0..5 {
            reducer.apply(format!("event-{index}")).expect("apply");
        }

        let recovered = CheckpointedReducer::new(
            journal,
            std::sync::Arc::new(FileCheckpointStore::<LensReducer>::open(
                root.join("checkpoint.json"),
            )),
            2,
        );
        assert_eq!(
            recovered.recover().expect("recover").last_applied_sequence,
            5
        );
        recovered.with_state(|state| assert_eq!(state.applied, 5));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn checkpoint_integrity_round_trips_state_and_empty_sequence() {
        let root = temp_root();
        let checkpoint_path = root.join("checkpoint.json");
        let store = FileCheckpointStore::<LensReducer>::open(&checkpoint_path);
        let expected = LensReducer { applied: 7 };
        store.save(&expected, 0).expect("save checkpoint");

        let loaded = store
            .load()
            .expect("load checkpoint")
            .expect("checkpoint exists");
        assert_eq!(loaded.sequence, 0);
        assert_eq!(loaded.state, expected);

        let bytes = std::fs::read(&checkpoint_path).expect("read checkpoint");
        let stored: serde_json::Value =
            serde_json::from_slice(&bytes).expect("decode stored checkpoint");
        assert_eq!(
            stored
                .get("schema_version")
                .and_then(serde_json::Value::as_u64),
            Some(u64::from(CHECKPOINT_SCHEMA_VERSION))
        );
        assert_eq!(
            stored
                .get("digest")
                .and_then(serde_json::Value::as_str)
                .map(str::len),
            Some(64)
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn checkpoint_round_trips_wide_integers_with_fixed_canonical_digest() {
        let root = temp_root();
        let checkpoint_path = root.join("checkpoint.json");
        let store = FileCheckpointStore::<WideIntegerState>::open(&checkpoint_path);
        let expected = WideIntegerState {
            min: i128::MIN,
            max: u128::MAX,
        };
        store
            .save(&expected, u64::MAX)
            .expect("save wide integer checkpoint");

        let loaded = store
            .load()
            .expect("load wide integer checkpoint")
            .expect("wide integer checkpoint exists");
        assert_eq!(loaded.sequence, u64::MAX);
        assert_eq!(loaded.state, expected);

        let state = serde_json::to_value(&expected).expect("encode wide integer state");
        assert_eq!(
            checkpoint_digest(CHECKPOINT_SCHEMA_VERSION, u64::MAX, &state)
                .expect("compute checkpoint digest"),
            "e4ef5435fac64f6da1c37dc31e4118b3927ccd0cd8ca1d2512e3225e436bf447"
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn checkpoint_nested_parent_failure_is_retryable() {
        let root = temp_root();
        let checkpoint_path = root
            .join("missing-a")
            .join("missing-b")
            .join("checkpoint.json");
        let parent = checkpoint_path.parent().expect("checkpoint parent");
        let state = LensReducer { applied: 4 };
        let error = save_checkpoint_with(
            &checkpoint_path,
            &state,
            4,
            |directory| {
                std::fs::create_dir_all(directory)?;
                Err(std::io::Error::other(
                    "injected checkpoint parent-directory barrier failure",
                ))
            },
            atomic_write,
        )
        .expect_err("parent durability failure must surface");
        assert!(
            error
                .to_string()
                .contains("parent-directory barrier failure")
        );
        assert!(parent.is_dir());
        assert!(!checkpoint_path.exists());

        let store = FileCheckpointStore::<LensReducer>::open(&checkpoint_path);
        store.save(&state, 4).expect("retry checkpoint save");
        let loaded = store
            .load()
            .expect("load retried checkpoint")
            .expect("retried checkpoint exists");
        assert_eq!(loaded.sequence, 4);
        assert_eq!(loaded.state, state);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn visible_checkpoint_with_parent_sync_error_is_reported_as_degraded() {
        let root = temp_root();
        let checkpoints = std::sync::Arc::new(VisibleFailureCheckpointStore::new(
            root.join("checkpoint.json"),
        ));
        let reducer = CheckpointedReducer::new(
            std::sync::Arc::new(MemoryEventJournal::<String>::new()),
            std::sync::Arc::clone(&checkpoints) as std::sync::Arc<dyn CheckpointStore<LensReducer>>,
            1,
        );

        let first = reducer.apply("one".to_string()).expect("apply first event");
        assert!(matches!(
            first.checkpoint,
            CheckpointApplyStatus::Degraded { .. }
        ));
        let visible = checkpoints
            .load()
            .expect("load visible checkpoint")
            .expect("visible checkpoint exists");
        assert_eq!(visible.sequence, 1);
        assert_eq!(visible.state, LensReducer { applied: 1 });

        let second = reducer
            .apply("two".to_string())
            .expect("apply second event");
        assert_eq!(second.checkpoint, CheckpointApplyStatus::Saved);
        let repaired = checkpoints
            .load()
            .expect("load confirmed checkpoint")
            .expect("confirmed checkpoint exists");
        assert_eq!(repaired.sequence, 2);
        assert_eq!(repaired.state, LensReducer { applied: 2 });
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn checkpoint_rejects_state_sequence_digest_unknown_field_and_schema_tamper() {
        enum Tamper {
            State,
            Sequence,
            Digest,
            UnknownField,
            Schema,
        }

        for (label, tamper, expected_error) in [
            ("state", Tamper::State, "digest mismatch"),
            ("sequence", Tamper::Sequence, "digest mismatch"),
            ("digest", Tamper::Digest, "digest mismatch"),
            ("unknown", Tamper::UnknownField, "unknown field"),
            ("schema", Tamper::Schema, "unsupported checkpoint schema"),
        ] {
            let root = temp_root();
            let checkpoint_path = root.join(format!("{label}.json"));
            let store = FileCheckpointStore::<LensReducer>::open(&checkpoint_path);
            store
                .save(&LensReducer { applied: 2 }, 2)
                .expect("seed valid checkpoint");
            mutate_checkpoint(&checkpoint_path, |frame| match tamper {
                Tamper::State => {
                    if let Some(state) = frame
                        .get_mut("state")
                        .and_then(serde_json::Value::as_object_mut)
                    {
                        state.insert("applied".to_string(), serde_json::Value::from(99));
                    }
                }
                Tamper::Sequence => {
                    frame.insert("sequence".to_string(), serde_json::Value::from(99));
                }
                Tamper::Digest => {
                    frame.insert(
                        "digest".to_string(),
                        serde_json::Value::String("0".repeat(64)),
                    );
                }
                Tamper::UnknownField => {
                    frame.insert("unexpected".to_string(), serde_json::Value::Bool(true));
                }
                Tamper::Schema => {
                    frame.insert("schema_version".to_string(), serde_json::Value::from(99));
                }
            });

            let error = store.load().expect_err("tampered checkpoint must fail");
            assert!(
                error.to_string().contains(expected_error),
                "{label} tamper produced unexpected error: {error}"
            );
            std::fs::remove_dir_all(root).ok();
        }
    }

    #[test]
    fn corrupt_checkpoint_is_an_error() {
        let root = temp_root();
        let store = FileCheckpointStore::<LensReducer>::open(root.join("checkpoint.json"));
        std::fs::write(root.join("checkpoint.json"), b"{partial").expect("write corrupt");
        let error = store.load().expect_err("corrupt checkpoint must fail");
        assert!(error.to_string().contains("corrupt checkpoint"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn reducer_discards_and_repairs_corrupt_checkpoint() {
        let root = temp_root();
        let journal = std::sync::Arc::new(
            FileEventJournal::<String>::open(root.join("events.jsonl"), FileDurability::Flush)
                .expect("open journal"),
        );
        for value in ["one", "two", "three"] {
            journal.append(value.to_string()).expect("append");
        }
        let checkpoint_path = root.join("checkpoint.json");
        std::fs::write(&checkpoint_path, b"{partial").expect("write corrupt checkpoint");
        let reducer = CheckpointedReducer::new(
            journal,
            std::sync::Arc::new(FileCheckpointStore::<LensReducer>::open(&checkpoint_path)),
            2,
        );

        let receipt = reducer.recover().expect("journal recovery");
        assert_eq!(receipt.last_applied_sequence, 3);
        assert!(matches!(
            receipt.checkpoint,
            super::super::CheckpointRecoveryStatus::Rebuilt { .. }
        ));
        reducer.with_state(|state| assert_eq!(state.applied, 3));
        let repaired = FileCheckpointStore::<LensReducer>::open(&checkpoint_path)
            .load()
            .expect("load repaired checkpoint")
            .expect("repaired checkpoint exists");
        assert_eq!(repaired.sequence, 3);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn reducer_replays_full_journal_and_repairs_valid_json_state_mutation() {
        let root = temp_root();
        let journal = std::sync::Arc::new(
            FileEventJournal::<String>::open(root.join("events.jsonl"), FileDurability::Flush)
                .expect("open journal"),
        );
        for value in ["one", "two", "three"] {
            journal.append(value.to_string()).expect("append");
        }
        let checkpoint_path = root.join("checkpoint.json");
        let store = FileCheckpointStore::<LensReducer>::open(&checkpoint_path);
        store
            .save(&LensReducer { applied: 2 }, 2)
            .expect("save prefix checkpoint");
        mutate_checkpoint(&checkpoint_path, |frame| {
            if let Some(state) = frame
                .get_mut("state")
                .and_then(serde_json::Value::as_object_mut)
            {
                state.insert("applied".to_string(), serde_json::Value::from(200));
            }
        });
        let mutated_bytes = std::fs::read(&checkpoint_path).expect("read mutated checkpoint");
        assert!(serde_json::from_slice::<serde_json::Value>(&mutated_bytes).is_ok());

        let reducer = CheckpointedReducer::new(
            journal,
            std::sync::Arc::new(FileCheckpointStore::<LensReducer>::open(&checkpoint_path)),
            2,
        );
        let receipt = reducer
            .recover()
            .expect("recover from authoritative journal");
        assert_eq!(receipt.last_applied_sequence, 3);
        assert!(matches!(
            receipt.checkpoint,
            super::super::CheckpointRecoveryStatus::Rebuilt { .. }
        ));
        reducer.with_state(|state| assert_eq!(state.applied, 3));

        let repaired = store
            .load()
            .expect("load repaired checkpoint")
            .expect("repaired checkpoint exists");
        assert_eq!(repaired.sequence, 3);
        assert_eq!(repaired.state, LensReducer { applied: 3 });
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn duplicate_open_shares_lease_and_mismatched_durability_rejects() {
        let root = temp_root();
        let path = root.join("events.jsonl");
        let first = FileEventJournal::<String>::open(&path, FileDurability::Flush)
            .expect("open first journal");
        let second = FileEventJournal::<String>::open(
            root.join(".").join("events.jsonl"),
            FileDurability::Flush,
        )
        .expect("open shared journal");
        assert!(Arc::ptr_eq(&first.shared, &second.shared));
        let error = FileEventJournal::<String>::open(&path, FileDurability::SyncData)
            .expect_err("mismatched durability must reject");
        assert!(
            error
                .to_string()
                .contains("different durability configuration")
        );
        drop(first);
        drop(second);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn bare_and_dot_relative_paths_share_one_authority() {
        let name = format!("echo-file-journal-bare-{}.jsonl", uuid::Uuid::new_v4());
        let bare = PathBuf::from(&name);
        let dotted = PathBuf::from(".").join(&name);
        let first = FileEventJournal::<String>::open(&bare, FileDurability::Flush)
            .expect("open bare journal path");
        let second = FileEventJournal::<String>::open(&dotted, FileDurability::Flush)
            .expect("open dotted journal path");
        assert!(Arc::ptr_eq(&first.shared, &second.shared));
        assert_eq!(first.path(), second.path());
        drop(first);
        drop(second);
        std::fs::remove_file(&bare).ok();
        std::fs::remove_file(format!(".{name}.lease")).ok();
    }

    #[test]
    fn fresh_file_creation_requires_file_and_parent_barrier() {
        let root = temp_root();
        let path = root.join("events.jsonl");
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .expect("create fresh journal fixture");
        let called = std::cell::Cell::new(false);
        let error = finish_new_journal_creation(&file, &root, |_| {
            called.set(true);
            Err(std::io::Error::other("injected parent barrier failure"))
        })
        .expect_err("parent barrier failure must surface");
        assert!(called.get());
        assert!(error.to_string().contains("parent barrier failure"));
        finish_new_journal_creation(&file, &root, sync_directory)
            .expect("retry file and parent barrier");
        drop(file);

        let retry_path = root.join("retry.jsonl");
        std::fs::File::create(&retry_path).expect("leave ambiguous empty journal");
        let reopened = FileEventJournal::<String>::open(&retry_path, FileDurability::SyncData)
            .expect("production open reconciles empty existing journal");
        assert_eq!(reopened.last_sequence(), 0);
        reopened
            .append("durable".to_string())
            .expect("append after reconciled creation");
        drop(reopened);
        std::fs::remove_dir_all(root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn file_journal_rejects_symlink_authority_without_touching_target() {
        use std::os::unix::fs::symlink;

        let root = temp_root();
        let outside = root.join("outside.jsonl");
        let link = root.join("events.jsonl");
        std::fs::write(&outside, b"outside").expect("write outside target");
        symlink(&outside, &link).expect("create journal symlink");
        let error = FileEventJournal::<String>::open(&link, FileDurability::SyncData)
            .expect_err("symlink journal authority must reject");
        assert!(
            error.to_string().contains("symlink") || error.to_string().contains("Too many levels")
        );
        assert_eq!(std::fs::read(&outside).expect("read outside"), b"outside");
        std::fs::remove_dir_all(root).ok();
    }

    const FILE_LEASE_PROBE_ENV: &str = "ECHO_FILE_JOURNAL_LEASE_PROBE";

    #[test]
    fn file_process_lease_probe() {
        let Ok(path) = std::env::var(FILE_LEASE_PROBE_ENV) else {
            return;
        };
        let error = FileEventJournal::<String>::open(path, FileDurability::Flush)
            .expect_err("competing process must fail open");
        assert!(
            error
                .to_string()
                .contains("already open in another process")
        );
    }

    #[test]
    fn file_journal_rejects_competing_process_while_lease_is_alive() {
        let root = temp_root();
        let path = root.join("events.jsonl");
        let journal = FileEventJournal::<String>::open(&path, FileDurability::Flush)
            .expect("open leased journal");
        let output =
            std::process::Command::new(std::env::current_exe().expect("current test binary"))
                .arg("journal::file::tests::file_process_lease_probe")
                .arg("--exact")
                .arg("--nocapture")
                .env(FILE_LEASE_PROBE_ENV, &path)
                .output()
                .expect("run file lease probe");
        assert!(
            output.status.success(),
            "file lease probe failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        drop(journal);
        std::fs::remove_dir_all(root).ok();
    }
}
