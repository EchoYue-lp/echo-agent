//! File-backed journal and checkpoint implementations.
//!
//! [`FileEventJournal`] persists one JSON line per record. On open it scans the
//! file, validates 1-based contiguous sequences, tolerates one torn trailing
//! record (partial write after a crash) by truncating it, and rejects gaps or
//! mid-file corruption loudly. [`FileCheckpointStore`] writes one atomic
//! snapshot file that pairs the reducer state with its applied sequence.

use super::{CheckpointFrame, CheckpointStore, EventJournal, JournalEvent, JournalRecord};
use echo_core::error::{ReactError, Result};
use echo_core::utils::fs::{
    FileDurability, append_existing, atomic_write, read_existing, read_existing_from,
    truncate_existing,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::any::TypeId;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

fn io_error(context: &str, error: std::io::Error) -> ReactError {
    ReactError::Other(format!("{context}: {error}"))
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
}

#[derive(Debug)]
struct SharedFileJournalState {
    event_type: TypeId,
    state: Mutex<FileJournalState>,
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
/// offsets; replay then seeks directly to the requested sequence suffix.
#[derive(Debug)]
pub struct FileEventJournal<E> {
    path: PathBuf,
    durability: FileDurability,
    shared: Arc<SharedFileJournalState>,
    _event: PhantomData<fn() -> E>,
}

impl<E: JournalEvent> FileEventJournal<E> {
    /// Open (or create) the journal at `path`.
    ///
    /// A torn trailing record from an interrupted append is truncated; gaps
    /// and mid-file corruption are errors.
    pub fn open(path: impl Into<PathBuf>, durability: FileDurability) -> Result<Self> {
        let path = path.into();
        let context = format!("journal open {}", path.display());
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| io_error(&context, error))?;
        }
        // Appends go through `append_existing`, which requires the target to
        // already be a regular file, so a fresh journal creates an empty one.
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(io_error(&context, error)),
        }
        let path = path
            .parent()
            .and_then(|parent| std::fs::canonicalize(parent).ok())
            .and_then(|parent| path.file_name().map(|name| parent.join(name)))
            .unwrap_or(path);
        let context = format!("journal open {}", path.display());
        let mut registry = file_journal_registry().lock().map_err(|error| {
            ReactError::Other(format!("journal registry lock poisoned: {error}"))
        })?;
        if let Some(shared) = registry.get(&path).and_then(Weak::upgrade) {
            if shared.event_type != TypeId::of::<E>() {
                return Err(ReactError::Other(format!(
                    "{context}: journal is already open with a different event type"
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
            drop(state);
            return Ok(Self {
                path,
                durability,
                shared,
                _event: PhantomData,
            });
        }
        let scanned = scan_and_repair_journal::<E>(&path, &context, durability)?;
        let shared = Arc::new(SharedFileJournalState {
            event_type: TypeId::of::<E>(),
            state: Mutex::new(FileJournalState {
                next_sequence: scanned.next_sequence,
                valid_len: scanned.valid_len,
                record_offsets: scanned.record_offsets,
            }),
        });
        registry.insert(path.clone(), Arc::downgrade(&shared));
        Ok(Self {
            path,
            durability,
            shared,
            _event: PhantomData,
        })
    }

    /// Journal file path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl<E: JournalEvent> EventJournal<E> for FileEventJournal<E> {
    fn append(&self, event: &E) -> Result<JournalRecord<E>> {
        // The lock spans encoding + write + advance so concurrent appends
        // cannot observe or reuse an in-flight sequence.
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let next_sequence = state.next_sequence.checked_add(1).ok_or_else(|| {
            ReactError::Other("journal sequence exhausted before append".to_string())
        })?;
        let record = JournalRecord {
            sequence: state.next_sequence,
            event: event.clone(),
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
        append_existing(&self.path, &line, self.durability)
            .map_err(|error| io_error(&context, error))?;
        let record_offset = state.valid_len;
        state.record_offsets.push(record_offset);
        state.valid_len = valid_len;
        state.next_sequence = next_sequence;
        Ok(record)
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
        let bytes = read_existing_from(&self.path, start_offset)
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

#[derive(Debug, Serialize, Deserialize)]
struct CheckpointFile<S> {
    sequence: u64,
    state: S,
}

/// Single-file [`CheckpointStore`] written with atomic replace.
///
/// The snapshot is written to a temporary sibling and renamed, so readers
/// observe either the previous or the new checkpoint, never a partial file. A
/// missing file loads as `None`; a corrupt file is an error.
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
        let frame = CheckpointFile {
            sequence: through_sequence,
            state,
        };
        let bytes = serde_json::to_vec(&frame)
            .map_err(|error| ReactError::Other(format!("failed to encode checkpoint: {error}")))?;
        let context = format!("checkpoint save {}", self.path.display());
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| io_error(&context, error))?;
        }
        atomic_write(&self.path, &bytes).map_err(|error| io_error(&context, error))
    }

    fn load(&self) -> Result<Option<CheckpointFrame<S>>> {
        let context = format!("checkpoint load {}", self.path.display());
        let bytes = match read_existing(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io_error(&context, error)),
        };
        let frame: CheckpointFile<S> = serde_json::from_slice(&bytes).map_err(|error| {
            ReactError::Other(format!("{context}: corrupt checkpoint: {error}"))
        })?;
        Ok(Some(CheckpointFrame {
            sequence: frame.sequence,
            state: frame.state,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::super::CheckpointedReducer;
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Default, Serialize, Deserialize, Debug)]
    struct LensReducer {
        applied: u64,
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

    #[test]
    fn file_journal_round_trips_and_resumes() {
        let root = temp_root();
        let path = root.join("events.jsonl");
        let journal =
            FileEventJournal::<String>::open(&path, FileDurability::Flush).expect("open journal");
        assert_eq!(journal.last_sequence(), 0);
        journal.append(&"one".to_string()).expect("append");
        journal.append(&"two".to_string()).expect("append");

        let reopened =
            FileEventJournal::<String>::open(&path, FileDurability::Flush).expect("reopen journal");
        assert_eq!(reopened.next_sequence(), 3);
        let records = reopened.replay_after(0, usize::MAX).expect("replay");
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].event, "one");
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
                    let record = journal.append(&format!("{label}-{index}"))?;
                    sequences.push(record.sequence);
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
    fn torn_trailing_record_is_truncated_on_open() {
        let root = temp_root();
        let path = root.join("events.jsonl");
        let journal =
            FileEventJournal::<String>::open(&path, FileDurability::Flush).expect("open journal");
        journal.append(&"one".to_string()).expect("append");
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
        journal.append(&"one".to_string()).expect("append");
        journal.append(&"two".to_string()).expect("append");
        journal.append(&"three".to_string()).expect("append");
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
            reducer.apply(&format!("event-{index}")).expect("apply");
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
            journal.append(&value.to_string()).expect("append");
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
}
