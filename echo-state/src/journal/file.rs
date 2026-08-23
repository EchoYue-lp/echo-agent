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
    FileDurability, append_existing, atomic_write, read_existing, truncate_existing,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

fn io_error(context: &str, error: std::io::Error) -> ReactError {
    ReactError::Other(format!("{context}: {error}"))
}

/// Scanned state of a journal file on open.
struct ScannedJournal {
    /// Byte length of the valid prefix (records plus their newlines).
    valid_len: u64,
    /// Next sequence a fresh append should assign.
    next_sequence: u64,
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
    while offset < bytes.len() {
        let Some(newline) = bytes[offset..].iter().position(|byte| *byte == b'\n') else {
            // Trailing chunk without a newline: a torn write. It is only
            // tolerated as the final record; drop it.
            break;
        };
        let line_end = offset + newline;
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
        next_sequence = record.sequence + 1;
        valid_len = u64::try_from(line_end + 1)
            .map_err(|_| ReactError::Other(format!("{context}: journal exceeds supported size")))?;
        offset = line_end + 1;
    }
    Ok(ScannedJournal {
        valid_len,
        next_sequence,
    })
}

/// JSONL-backed [`EventJournal`].
///
/// Appends serialize one [`JournalRecord`] per line with the caller-selected
/// durability policy; `Flush` survives process crashes, `SyncData` additionally
/// survives power loss. Replay re-reads the file, so pair it with a
/// [`super::CheckpointedReducer`] to bound hot-path reads.
#[derive(Debug)]
pub struct FileEventJournal<E> {
    path: PathBuf,
    durability: FileDurability,
    next_sequence: Mutex<u64>,
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
        let scanned = match read_existing(&path) {
            Ok(bytes) => {
                let scanned = scan_journal::<E>(&context, &bytes)?;
                let file_len = u64::try_from(bytes.len()).map_err(|_| {
                    ReactError::Other(format!("{context}: journal exceeds supported size"))
                })?;
                if scanned.valid_len < file_len {
                    truncate_existing(&path, scanned.valid_len, durability)
                        .map_err(|error| io_error(&context, error))?;
                }
                scanned
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => ScannedJournal {
                valid_len: 0,
                next_sequence: 1,
            },
            Err(error) => return Err(io_error(&context, error)),
        };
        Ok(Self {
            path,
            durability,
            next_sequence: Mutex::new(scanned.next_sequence),
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
        let mut next = self
            .next_sequence
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let record = JournalRecord {
            sequence: *next,
            event: event.clone(),
        };
        let mut line = serde_json::to_vec(&record).map_err(|error| {
            ReactError::Other(format!("failed to encode journal record: {error}"))
        })?;
        line.push(b'\n');
        let context = format!("journal append {}", self.path.display());
        append_existing(&self.path, &line, self.durability)
            .map_err(|error| io_error(&context, error))?;
        *next = record.sequence + 1;
        Ok(record)
    }

    fn next_sequence(&self) -> u64 {
        *self
            .next_sequence
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    fn last_sequence(&self) -> u64 {
        self.next_sequence().saturating_sub(1)
    }

    fn replay_after(&self, after_sequence: u64, limit: usize) -> Result<Vec<JournalRecord<E>>> {
        let context = format!("journal replay {}", self.path.display());
        let bytes = read_existing(&self.path).map_err(|error| io_error(&context, error))?;
        // A concurrent append may have exposed a torn final line; scanning the
        // complete prefix is still sound because sequences are validated.
        let scanned = scan_journal::<E>(&context, &bytes)?;
        if scanned.valid_len == 0 {
            return Ok(Vec::new());
        }
        let valid_prefix = usize::try_from(scanned.valid_len)
            .map_err(|_| ReactError::Other(format!("{context}: journal exceeds supported size")))?;
        let records: Vec<JournalRecord<E>> = bytes[..valid_prefix]
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(serde_json::from_slice)
            .collect::<std::result::Result<_, _>>()
            .map_err(|error| {
                ReactError::Other(format!("{context}: corrupt journal record: {error}"))
            })?;
        Ok(records
            .into_iter()
            .filter(|record| record.sequence > after_sequence)
            .take(limit)
            .collect())
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

        fn apply(&mut self, _event: &String) -> Result<()> {
            self.applied += 1;
            Ok(())
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
        assert_eq!(recovered.recover().expect("recover"), 5);
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
}
