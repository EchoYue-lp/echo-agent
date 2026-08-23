//! Directory-backed segmented event journal.
//!
//! Kafka's log model uses immutable segments while offsets remain global. This
//! implementation applies that storage shape to the framework journal: every
//! segment name is its first global sequence, closed segments are immutable,
//! and only the active segment may repair a crash-torn tail. LangGraph-style
//! checkpoints remain derived state above this authoritative event history;
//! callers choose retention and pin policy by passing a keep cursor to prune.
//! Product stream identities, retention counts, and UI projections do not live
//! in this module.

use super::{
    EventJournal, JournalAppendReceipt, JournalDurabilityStatus, JournalEvent, JournalRecord,
};
use echo_core::error::{ReactError, Result};
use echo_core::utils::canonical_json::canonical_json_bytes;
use echo_core::utils::fs::{
    ExclusiveFileLease, FileDurability, append_existing, read_existing, read_existing_from,
    read_existing_lines_from, truncate_existing, try_exclusive_file_lease,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::any::TypeId;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

const SEGMENT_SUFFIX: &str = ".jsonl";
const SEGMENT_DIGITS: usize = 20;
const LEASE_AUTHORITY: &str = "segmented-event-journal";

fn journal_error(message: impl Into<String>) -> ReactError {
    ReactError::Other(message.into())
}

fn io_error(context: &str, error: std::io::Error) -> ReactError {
    journal_error(format!("{context}: {error}"))
}

#[derive(Debug, Serialize)]
struct IntegrityPayload<'a, E> {
    sequence: u64,
    event: &'a E,
}

#[derive(Debug, Serialize)]
struct StoredRecordRef<'a, E> {
    sequence: u64,
    event: &'a E,
    digest: &'a str,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredRecord<E> {
    sequence: u64,
    event: E,
    digest: String,
}

fn record_digest<E: Serialize>(sequence: u64, event: &E) -> Result<String> {
    let bytes = canonical_json_bytes(&IntegrityPayload { sequence, event }).map_err(|error| {
        journal_error(format!("failed to encode journal integrity input: {error}"))
    })?;
    let digest = Sha256::digest(bytes);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn verify_record<E: JournalEvent>(
    context: &str,
    expected_sequence: u64,
    record: &StoredRecord<E>,
) -> Result<()> {
    if record.sequence != expected_sequence {
        return Err(journal_error(format!(
            "{context}: journal sequence gap, expected {expected_sequence} but found {}",
            record.sequence
        )));
    }
    let expected_digest = record_digest(record.sequence, &record.event)?;
    if record.digest != expected_digest {
        return Err(journal_error(format!(
            "{context}: integrity digest mismatch at sequence {}",
            record.sequence
        )));
    }
    Ok(())
}

fn segment_name(start_sequence: u64) -> String {
    format!("{start_sequence:020}{SEGMENT_SUFFIX}")
}

fn segment_path(directory: &Path, start_sequence: u64) -> PathBuf {
    directory.join(segment_name(start_sequence))
}

fn parse_segment_name(name: &str) -> Option<u64> {
    let digits = name.strip_suffix(SEGMENT_SUFFIX)?;
    if digits.len() != SEGMENT_DIGITS || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

fn list_segment_paths(directory: &Path, context: &str) -> Result<Vec<(u64, PathBuf)>> {
    let mut segments = Vec::new();
    let entries = std::fs::read_dir(directory).map_err(|error| io_error(context, error))?;
    for entry in entries {
        let entry = entry.map_err(|error| io_error(context, error))?;
        let file_type = entry
            .file_type()
            .map_err(|error| io_error(context, error))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.ends_with(SEGMENT_SUFFIX) {
            continue;
        }
        if !file_type.is_file() {
            return Err(journal_error(format!(
                "{context}: segmented journal entry {name:?} is not a regular file"
            )));
        }
        let start = parse_segment_name(&name).ok_or_else(|| {
            journal_error(format!(
                "{context}: invalid segmented journal file name {name:?}"
            ))
        })?;
        if start == 0 {
            return Err(journal_error(format!(
                "{context}: segment start sequence must be positive"
            )));
        }
        segments.push((start, entry.path()));
    }
    segments.sort_by_key(|(start, _)| *start);
    Ok(segments)
}

#[derive(Debug, Clone)]
struct SegmentState {
    path: PathBuf,
    start_sequence: u64,
    end_sequence: u64,
    bytes: u64,
    record_offsets: Vec<u64>,
}

impl SegmentState {
    fn has_records(&self) -> bool {
        !self.record_offsets.is_empty()
    }

    fn metadata(&self, active: bool) -> JournalSegmentMetadata {
        JournalSegmentMetadata {
            path: self.path.clone(),
            start_sequence: self.start_sequence,
            end_sequence: self.end_sequence,
            bytes: self.bytes,
            active,
        }
    }
}

fn scan_segment<E: JournalEvent>(
    path: &Path,
    start_sequence: u64,
    active: bool,
    durability: FileDurability,
    context: &str,
) -> Result<SegmentState> {
    let bytes = read_existing(path).map_err(|error| io_error(context, error))?;
    let mut expected = start_sequence;
    let mut valid_len = 0_u64;
    let mut offset = 0_usize;
    let mut record_offsets = Vec::new();
    while offset < bytes.len() {
        let Some(newline) = bytes
            .get(offset..)
            .and_then(|suffix| suffix.iter().position(|byte| *byte == b'\n'))
        else {
            if active {
                break;
            }
            return Err(journal_error(format!(
                "{context}: closed segment has an incomplete trailing record"
            )));
        };
        let line_end = offset
            .checked_add(newline)
            .ok_or_else(|| journal_error(format!("{context}: segment byte offset overflow")))?;
        let line = bytes
            .get(offset..line_end)
            .ok_or_else(|| journal_error(format!("{context}: invalid segment byte range")))?;
        let record: StoredRecord<E> = serde_json::from_slice(line).map_err(|error| {
            journal_error(format!(
                "{context}: corrupt journal record at sequence {expected}: {error}"
            ))
        })?;
        verify_record(context, expected, &record)?;
        record_offsets
            .push(u64::try_from(offset).map_err(|_| {
                journal_error(format!("{context}: segment exceeds supported size"))
            })?);
        expected = expected
            .checked_add(1)
            .ok_or_else(|| journal_error(format!("{context}: journal sequence exhausted")))?;
        offset = line_end
            .checked_add(1)
            .ok_or_else(|| journal_error(format!("{context}: segment byte offset overflow")))?;
        valid_len = u64::try_from(offset)
            .map_err(|_| journal_error(format!("{context}: segment exceeds supported size")))?;
    }
    let file_len = u64::try_from(bytes.len())
        .map_err(|_| journal_error(format!("{context}: segment exceeds supported size")))?;
    if valid_len < file_len {
        if !active {
            return Err(journal_error(format!(
                "{context}: closed segment has a torn tail"
            )));
        }
        truncate_existing(path, valid_len, durability).map_err(|error| io_error(context, error))?;
    }
    if !active && record_offsets.is_empty() {
        return Err(journal_error(format!("{context}: closed segment is empty")));
    }
    Ok(SegmentState {
        path: path.to_path_buf(),
        start_sequence,
        end_sequence: expected.saturating_sub(1),
        bytes: valid_len,
        record_offsets,
    })
}

#[cfg(unix)]
fn sync_directory(directory: &Path) -> std::io::Result<()> {
    std::fs::File::open(directory)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_directory: &Path) -> std::io::Result<()> {
    Ok(())
}

fn create_segment(directory: &Path, start_sequence: u64, context: &str) -> Result<SegmentState> {
    let path = segment_path(directory, start_sequence);
    let file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let metadata = std::fs::symlink_metadata(&path)
                .map_err(|metadata_error| io_error(context, metadata_error))?;
            if !metadata.file_type().is_file() || metadata.len() != 0 {
                return Err(journal_error(format!(
                    "{context}: expected rollover segment already exists and is not an empty regular file: {}",
                    path.display()
                )));
            }
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .map_err(|open_error| io_error(context, open_error))?
        }
        Err(error) => return Err(io_error(context, error)),
    };
    file.sync_data().map_err(|error| io_error(context, error))?;
    sync_directory(directory).map_err(|error| io_error(context, error))?;
    Ok(SegmentState {
        path,
        start_sequence,
        end_sequence: start_sequence.saturating_sub(1),
        bytes: 0,
        record_offsets: Vec::new(),
    })
}

fn scan_directory<E: JournalEvent>(
    directory: &Path,
    durability: FileDurability,
    context: &str,
) -> Result<SegmentedJournalState> {
    let listed = list_segment_paths(directory, context)?;
    if listed.is_empty() {
        let active = create_segment(directory, 1, context)?;
        return Ok(SegmentedJournalState {
            next_sequence: 1,
            segments: vec![active],
            poison: None,
        });
    }
    let mut segments = Vec::with_capacity(listed.len());
    let last_index = listed.len().saturating_sub(1);
    let mut expected_start = None;
    for (index, (start, path)) in listed.into_iter().enumerate() {
        if let Some(expected) = expected_start
            && start != expected
        {
            return Err(journal_error(format!(
                "{context}: segment gap, expected start {expected} but found {start}"
            )));
        }
        let active = index == last_index;
        let segment_context = format!("{context} segment {}", path.display());
        let segment = scan_segment::<E>(&path, start, active, durability, &segment_context)?;
        expected_start = segment.end_sequence.checked_add(1);
        segments.push(segment);
    }
    let next_sequence = segments
        .last()
        .and_then(|segment| segment.end_sequence.checked_add(1))
        .ok_or_else(|| journal_error(format!("{context}: journal sequence exhausted")))?;
    Ok(SegmentedJournalState {
        next_sequence,
        segments,
        poison: None,
    })
}

/// Stable metadata for one retained journal segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalSegmentMetadata {
    /// Canonical segment path.
    pub path: PathBuf,
    /// First global sequence encoded by the file name.
    pub start_sequence: u64,
    /// Last global sequence in the segment. An empty active segment reports
    /// `start_sequence - 1`.
    pub end_sequence: u64,
    /// Valid bytes currently stored in the segment.
    pub bytes: u64,
    /// Whether this is the only mutable segment.
    pub active: bool,
}

#[derive(Debug)]
struct SegmentedJournalState {
    next_sequence: u64,
    segments: Vec<SegmentState>,
    poison: Option<String>,
}

#[derive(Debug)]
struct SharedSegmentedJournalState {
    event_type: TypeId,
    max_active_segment_bytes: u64,
    durability: FileDurability,
    state: Mutex<SegmentedJournalState>,
    _lease: ExclusiveFileLease,
}

fn segmented_registry() -> &'static Mutex<HashMap<PathBuf, Weak<SharedSegmentedJournalState>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, Weak<SharedSegmentedJournalState>>>> =
        OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Directory-backed segmented [`EventJournal`].
///
/// Segment files are named with their 20-digit global start sequence. Appends
/// roll before the next record once the active file reaches
/// `max_active_segment_bytes`; a single indivisible record may exceed the
/// threshold. Repeated opens in one process share one canonical authority.
/// Another process fails to open while that authority is alive.
#[derive(Debug)]
pub struct SegmentedFileEventJournal<E> {
    directory: PathBuf,
    shared: Arc<SharedSegmentedJournalState>,
    #[cfg(test)]
    append_fault: Mutex<Option<AppendFault>>,
    #[cfg(test)]
    sync_fault: Mutex<bool>,
    #[cfg(test)]
    create_fault: Mutex<bool>,
    _event: PhantomData<fn() -> E>,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy)]
enum AppendFault {
    FullWrite,
    PartialWrite { bytes: usize },
    UnrecognizedSuffix,
}

impl<E: JournalEvent> SegmentedFileEventJournal<E> {
    /// Open or create a segmented journal directory.
    pub fn open(
        directory: impl Into<PathBuf>,
        max_active_segment_bytes: u64,
        durability: FileDurability,
    ) -> Result<Self> {
        if max_active_segment_bytes == 0 {
            return Err(journal_error(
                "segmented journal max active segment bytes must be positive",
            ));
        }
        let directory = directory.into();
        let context = format!("segmented journal open {}", directory.display());
        std::fs::create_dir_all(&directory).map_err(|error| io_error(&context, error))?;
        let metadata = std::fs::metadata(&directory).map_err(|error| io_error(&context, error))?;
        if !metadata.is_dir() {
            return Err(journal_error(format!(
                "{context}: journal root is not a directory"
            )));
        }
        let directory =
            std::fs::canonicalize(&directory).map_err(|error| io_error(&context, error))?;
        let context = format!("segmented journal open {}", directory.display());
        let mut registry = segmented_registry().lock().map_err(|error| {
            journal_error(format!("segmented journal registry lock poisoned: {error}"))
        })?;
        if let Some(shared) = registry.get(&directory).and_then(Weak::upgrade) {
            if shared.event_type != TypeId::of::<E>() {
                return Err(journal_error(format!(
                    "{context}: journal is already open with a different event type"
                )));
            }
            if shared.max_active_segment_bytes != max_active_segment_bytes
                || shared.durability != durability
            {
                return Err(journal_error(format!(
                    "{context}: journal is already open with a different configuration"
                )));
            }
            let mut state = shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            *state = scan_directory::<E>(&directory, durability, &context)?;
            drop(state);
            return Ok(Self {
                directory,
                shared,
                #[cfg(test)]
                append_fault: Mutex::new(None),
                #[cfg(test)]
                sync_fault: Mutex::new(false),
                #[cfg(test)]
                create_fault: Mutex::new(false),
                _event: PhantomData,
            });
        }
        let lease = try_exclusive_file_lease(&directory.join(LEASE_AUTHORITY))
            .map_err(|error| io_error(&context, error))?;
        let state = scan_directory::<E>(&directory, durability, &context)?;
        let shared = Arc::new(SharedSegmentedJournalState {
            event_type: TypeId::of::<E>(),
            max_active_segment_bytes,
            durability,
            state: Mutex::new(state),
            _lease: lease,
        });
        registry.insert(directory.clone(), Arc::downgrade(&shared));
        Ok(Self {
            directory,
            shared,
            #[cfg(test)]
            append_fault: Mutex::new(None),
            #[cfg(test)]
            sync_fault: Mutex::new(false),
            #[cfg(test)]
            create_fault: Mutex::new(false),
            _event: PhantomData,
        })
    }

    /// Canonical journal directory.
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Snapshot retained segment ranges and sizes.
    pub fn segments(&self) -> Vec<JournalSegmentMetadata> {
        let state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let last = state.segments.len().saturating_sub(1);
        state
            .segments
            .iter()
            .enumerate()
            .map(|(index, segment)| segment.metadata(index == last))
            .collect()
    }

    /// Force a data durability barrier on the active segment.
    pub fn sync_data(&self) -> Result<()> {
        let state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let active = state
            .segments
            .last()
            .ok_or_else(|| journal_error("segmented journal has no active segment"))?;
        let context = format!("segmented journal sync_data {}", active.path.display());
        self.sync_active(&active.path)
            .map_err(|error| io_error(&context, error))
    }

    /// Remove complete closed segments strictly before the caller's keep
    /// cursor. The active segment is never removed and no retention policy is
    /// inferred by the framework.
    pub fn prune_closed_segments_before(
        &self,
        keep_from_sequence: u64,
    ) -> Result<Vec<JournalSegmentMetadata>> {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let active_index = state.segments.len().saturating_sub(1);
        let candidates = state
            .segments
            .iter()
            .enumerate()
            .filter(|(index, segment)| {
                *index < active_index && segment.end_sequence < keep_from_sequence
            })
            .map(|(_, segment)| segment.clone())
            .collect::<Vec<_>>();
        let mut removed = Vec::new();
        for candidate in candidates {
            let context = format!("segmented journal prune {}", candidate.path.display());
            if let Err(error) = std::fs::remove_file(&candidate.path) {
                state.segments.retain(|segment| {
                    !removed
                        .iter()
                        .any(|metadata: &JournalSegmentMetadata| metadata.path == segment.path)
                });
                return Err(io_error(&context, error));
            }
            removed.push(candidate.metadata(false));
        }
        state
            .segments
            .retain(|segment| !removed.iter().any(|metadata| metadata.path == segment.path));
        if !removed.is_empty() {
            sync_directory(&self.directory)
                .map_err(|error| io_error("segmented journal prune directory sync", error))?;
        }
        Ok(removed)
    }

    fn roll_segment(&self, state: &mut SegmentedJournalState) -> Result<()> {
        let active = state
            .segments
            .last()
            .ok_or_else(|| journal_error("segmented journal has no active segment"))?;
        let context = format!("segmented journal rollover {}", active.path.display());
        self.sync_active(&active.path)
            .map_err(|error| io_error(&context, error))?;
        #[cfg(test)]
        {
            let mut fail = self
                .create_fault
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if std::mem::take(&mut *fail) {
                let path = segment_path(&self.directory, state.next_sequence);
                let file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .map_err(|error| io_error(&context, error))?;
                file.sync_data()
                    .map_err(|error| io_error(&context, error))?;
                return Err(journal_error(format!(
                    "{context}: injected failure after creating rollover segment"
                )));
            }
        }
        let next = create_segment(&self.directory, state.next_sequence, &context)?;
        state.segments.push(next);
        Ok(())
    }

    fn sync_active(&self, path: &Path) -> std::io::Result<()> {
        #[cfg(test)]
        {
            let mut fail = self
                .sync_fault
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if std::mem::take(&mut *fail) {
                return Err(std::io::Error::other(
                    "injected segmented sync_data failure",
                ));
            }
        }
        append_existing(path, b"", FileDurability::SyncData)
    }

    fn append_line(&self, path: &Path, line: &[u8]) -> std::io::Result<()> {
        #[cfg(test)]
        if let Some(fault) = self
            .append_fault
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        {
            use std::io::Write;

            let mut file = std::fs::OpenOptions::new().append(true).open(path)?;
            match fault {
                AppendFault::FullWrite => file.write_all(line)?,
                AppendFault::PartialWrite { bytes } => {
                    let prefix = line.get(..bytes.min(line.len())).unwrap_or(line);
                    file.write_all(prefix)?;
                }
                AppendFault::UnrecognizedSuffix => file.write_all(b"x")?,
            }
            file.flush()?;
            return Err(std::io::Error::other(
                "injected segmented append durability failure",
            ));
        }
        append_existing(path, line, self.shared.durability)
    }

    fn reconcile_failed_append(
        &self,
        active: &SegmentState,
        line: &[u8],
        context: &str,
        append_error: &std::io::Error,
    ) -> Result<Option<JournalDurabilityStatus>> {
        let suffix = read_existing_from(&active.path, active.bytes)
            .map_err(|error| io_error(context, error))?;
        if suffix == line {
            return Ok(Some(JournalDurabilityStatus::Degraded {
                error: append_error.to_string(),
            }));
        }
        if line.starts_with(&suffix) {
            if !suffix.is_empty() {
                truncate_existing(&active.path, active.bytes, self.shared.durability)
                    .map_err(|error| io_error(context, error))?;
            }
            return Ok(None);
        }
        Err(journal_error(format!(
            "{context}: append error left an unrecognized {}-byte suffix",
            suffix.len()
        )))
    }
}

impl<E: JournalEvent> EventJournal<E> for SegmentedFileEventJournal<E> {
    fn append(&self, event: E) -> Result<JournalAppendReceipt<E>> {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(reason) = &state.poison {
            return Err(journal_error(format!(
                "segmented journal append {} refused because the handle is poisoned: {reason}; reopen the journal to recover",
                self.directory.display()
            )));
        }
        let should_roll = state.segments.last().is_some_and(|active| {
            active.has_records() && active.bytes >= self.shared.max_active_segment_bytes
        });
        if should_roll {
            self.roll_segment(&mut state)?;
        }
        let next_sequence = state
            .next_sequence
            .checked_add(1)
            .ok_or_else(|| journal_error("segmented journal sequence exhausted before append"))?;
        let record = JournalRecord {
            sequence: state.next_sequence,
            event: Arc::new(event),
        };
        let digest = record_digest(record.sequence, record.event.as_ref())?;
        let stored = StoredRecordRef {
            sequence: record.sequence,
            event: record.event.as_ref(),
            digest: &digest,
        };
        let mut line = serde_json::to_vec(&stored).map_err(|error| {
            journal_error(format!(
                "failed to encode segmented journal record: {error}"
            ))
        })?;
        line.push(b'\n');
        let line_len = u64::try_from(line.len())
            .map_err(|_| journal_error("segmented journal record exceeds supported size"))?;
        let active = state
            .segments
            .last()
            .cloned()
            .ok_or_else(|| journal_error("segmented journal has no active segment"))?;
        let new_bytes = active.bytes.checked_add(line_len).ok_or_else(|| {
            journal_error("segmented journal byte length exhausted before append")
        })?;
        let context = format!("segmented journal append {}", active.path.display());
        let durability = match self.append_line(&active.path, &line) {
            Ok(()) => JournalDurabilityStatus::Confirmed,
            Err(error) => match self.reconcile_failed_append(&active, &line, &context, &error) {
                Ok(Some(status)) => status,
                Ok(None) => return Err(io_error(&context, error)),
                Err(repair_error) => {
                    let reason =
                        format!("append failed ({error}); reconciliation failed ({repair_error})");
                    state.poison = Some(reason.clone());
                    return Err(journal_error(format!(
                        "{context}: {reason}; handle poisoned until reopen"
                    )));
                }
            },
        };
        let active = state
            .segments
            .last_mut()
            .ok_or_else(|| journal_error("segmented journal has no active segment"))?;
        active.record_offsets.push(active.bytes);
        active.bytes = new_bytes;
        active.end_sequence = record.sequence;
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
        let context = format!("segmented journal replay {}", self.directory.display());
        let state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if after_sequence >= state.next_sequence.saturating_sub(1) {
            return Ok(Vec::new());
        }
        let mut records = Vec::new();
        for segment in &state.segments {
            if !segment.has_records() || segment.end_sequence <= after_sequence {
                continue;
            }
            let first_sequence = after_sequence.saturating_add(1).max(segment.start_sequence);
            let offset_index = usize::try_from(
                first_sequence.saturating_sub(segment.start_sequence),
            )
            .map_err(|_| journal_error(format!("{context}: sequence exceeds supported index")))?;
            let start_offset = segment
                .record_offsets
                .get(offset_index)
                .copied()
                .ok_or_else(|| {
                    journal_error(format!(
                        "{context}: missing byte offset for sequence {first_sequence}"
                    ))
                })?;
            let remaining = limit.saturating_sub(records.len());
            let bytes = read_existing_lines_from(&segment.path, start_offset, remaining)
                .map_err(|error| io_error(&context, error))?;
            let mut expected = first_sequence;
            for line in bytes
                .split(|byte| *byte == b'\n')
                .filter(|line| !line.is_empty())
            {
                let stored: StoredRecord<E> = serde_json::from_slice(line).map_err(|error| {
                    journal_error(format!("{context}: corrupt journal record: {error}"))
                })?;
                verify_record(&context, expected, &stored)?;
                expected = expected.checked_add(1).ok_or_else(|| {
                    journal_error(format!("{context}: journal sequence exhausted"))
                })?;
                records.push(JournalRecord {
                    sequence: stored.sequence,
                    event: Arc::new(stored.event),
                });
                if records.len() >= limit {
                    return Ok(records);
                }
            }
        }
        Ok(records)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;

    fn temp_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "echo-segmented-journal-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).expect("create segmented journal temp root");
        root
    }

    fn open_strings(
        root: &Path,
        max_bytes: u64,
        durability: FileDurability,
    ) -> SegmentedFileEventJournal<String> {
        SegmentedFileEventJournal::open(root, max_bytes, durability)
            .expect("open segmented journal")
    }

    fn mutate_json_line(
        path: &Path,
        mutate: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>),
    ) {
        let bytes = std::fs::read(path).expect("read segment");
        let line = bytes
            .split(|byte| *byte == b'\n')
            .find(|line| !line.is_empty())
            .expect("segment record");
        let mut value: serde_json::Value =
            serde_json::from_slice(line).expect("decode segment record");
        mutate(value.as_object_mut().expect("record object"));
        let mut encoded = serde_json::to_vec(&value).expect("encode mutated record");
        encoded.push(b'\n');
        std::fs::write(path, encoded).expect("write mutated segment");
    }

    #[test]
    fn rollover_preserves_one_global_cursor() {
        let root = temp_root("rollover");
        let journal = open_strings(&root, 1, FileDurability::Flush);
        for value in ["one", "two", "three"] {
            journal.append(value.to_string()).expect("append");
        }
        let metadata = journal.segments();
        assert_eq!(metadata.len(), 3);
        assert_eq!(
            metadata
                .iter()
                .map(|segment| (segment.start_sequence, segment.end_sequence))
                .collect::<Vec<_>>(),
            vec![(1, 1), (2, 2), (3, 3)]
        );
        let replay = journal.replay_after(0, usize::MAX).expect("replay");
        assert_eq!(
            replay
                .iter()
                .map(|record| record.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        drop(journal);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn thirty_two_concurrent_appends_are_contiguous() {
        const APPENDS: usize = 32;
        let root = temp_root("concurrent");
        let journal = Arc::new(open_strings(&root, 4096, FileDurability::Flush));
        let barrier = Arc::new(Barrier::new(APPENDS));
        let mut handles = Vec::new();
        for index in 0..APPENDS {
            let journal = Arc::clone(&journal);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                journal
                    .append(format!("event-{index}"))
                    .map(|receipt| receipt.record.sequence)
            }));
        }
        let mut sequences = handles
            .into_iter()
            .map(|handle| handle.join().expect("append thread").expect("append"))
            .collect::<Vec<_>>();
        sequences.sort_unstable();
        assert_eq!(sequences, (1..=32).collect::<Vec<_>>());
        assert_eq!(journal.last_sequence(), 32);
        drop(journal);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn active_torn_tail_is_repaired_on_reopen() {
        let root = temp_root("active-torn");
        let journal = open_strings(&root, 1, FileDurability::Flush);
        journal.append("one".to_string()).expect("append one");
        journal.append("two".to_string()).expect("append two");
        let active = journal
            .segments()
            .into_iter()
            .find(|segment| segment.active)
            .expect("active segment");
        let good_len = active.bytes;
        drop(journal);
        append_existing(&active.path, b"{\"sequence\":3", FileDurability::Flush)
            .expect("write torn active tail");

        let reopened = open_strings(&root, 1, FileDurability::Flush);
        assert_eq!(
            std::fs::metadata(&active.path)
                .expect("active metadata")
                .len(),
            good_len
        );
        assert_eq!(
            reopened
                .append("three".to_string())
                .expect("append after repair")
                .record
                .sequence,
            3
        );
        drop(reopened);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn closed_torn_tail_fails_open() {
        let root = temp_root("closed-torn");
        let journal = open_strings(&root, 1, FileDurability::Flush);
        journal.append("one".to_string()).expect("append one");
        journal.append("two".to_string()).expect("append two");
        let closed = journal
            .segments()
            .into_iter()
            .find(|segment| !segment.active)
            .expect("closed segment");
        drop(journal);
        let len = std::fs::metadata(&closed.path)
            .expect("closed metadata")
            .len();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&closed.path)
            .expect("open closed segment")
            .set_len(len.saturating_sub(1))
            .expect("truncate closed segment");
        let error = SegmentedFileEventJournal::<String>::open(&root, 1, FileDurability::Flush)
            .expect_err("closed torn segment must fail");
        assert!(error.to_string().contains("closed segment"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn closed_segment_corruption_fails_open() {
        let root = temp_root("closed-corrupt");
        let journal = open_strings(&root, 1, FileDurability::Flush);
        journal.append("one".to_string()).expect("append one");
        journal.append("two".to_string()).expect("append two");
        let closed = journal
            .segments()
            .into_iter()
            .find(|segment| !segment.active)
            .expect("closed segment");
        drop(journal);
        std::fs::write(&closed.path, b"not-json\n").expect("corrupt closed segment");
        let error = SegmentedFileEventJournal::<String>::open(&root, 1, FileDurability::Flush)
            .expect_err("closed corruption must fail");
        assert!(error.to_string().contains("corrupt journal record"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn segment_gap_fails_open() {
        let root = temp_root("segment-gap");
        let journal = open_strings(&root, 1, FileDurability::Flush);
        journal.append("one".to_string()).expect("append one");
        journal.append("two".to_string()).expect("append two");
        let active = journal
            .segments()
            .into_iter()
            .find(|segment| segment.active)
            .expect("active segment");
        drop(journal);
        std::fs::rename(&active.path, segment_path(&root, 3)).expect("rename segment");
        let error = SegmentedFileEventJournal::<String>::open(&root, 1, FileDurability::Flush)
            .expect_err("segment gap must fail");
        assert!(error.to_string().contains("segment gap"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn digest_mismatch_fails_open_and_replay() {
        let root = temp_root("digest");
        let journal = open_strings(&root, 4096, FileDurability::Flush);
        journal.append("one".to_string()).expect("append");
        let active = journal.segments().pop().expect("active segment");
        mutate_json_line(&active.path, |record| {
            record.insert(
                "digest".to_string(),
                serde_json::Value::String("0".repeat(64)),
            );
        });
        let replay_error = journal
            .replay_after(0, 1)
            .expect_err("replay must verify digest");
        assert!(replay_error.to_string().contains("digest mismatch"));
        drop(journal);
        let open_error =
            SegmentedFileEventJournal::<String>::open(&root, 4096, FileDurability::Flush)
                .expect_err("open must verify digest");
        assert!(open_error.to_string().contains("digest mismatch"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn unknown_record_field_fails_closed() {
        let root = temp_root("unknown-field");
        let journal = open_strings(&root, 4096, FileDurability::Flush);
        journal.append("one".to_string()).expect("append");
        let active = journal.segments().pop().expect("active segment");
        drop(journal);
        mutate_json_line(&active.path, |record| {
            record.insert("unexpected".to_string(), serde_json::Value::Bool(true));
        });
        let error = SegmentedFileEventJournal::<String>::open(&root, 4096, FileDurability::Flush)
            .expect_err("unknown field must fail");
        assert!(error.to_string().contains("unknown field"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn per_event_durability_and_explicit_barrier_succeed() {
        for durability in [FileDurability::Flush, FileDurability::SyncData] {
            let root = temp_root("durability");
            let journal = open_strings(&root, 4096, durability);
            let receipt = journal.append("one".to_string()).expect("append");
            assert_eq!(receipt.durability, JournalDurabilityStatus::Confirmed);
            journal.sync_data().expect("explicit durability barrier");
            drop(journal);
            let reopened = open_strings(&root, 4096, durability);
            assert_eq!(reopened.last_sequence(), 1);
            drop(reopened);
            std::fs::remove_dir_all(root).ok();
        }
    }

    #[test]
    fn rollover_does_not_open_next_segment_before_sync_barrier() {
        let root = temp_root("rollover-barrier");
        let journal = open_strings(&root, 1, FileDurability::Flush);
        journal.append("one".to_string()).expect("append first");
        *journal
            .sync_fault
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = true;
        let error = journal
            .append("two".to_string())
            .expect_err("rollover barrier must fail append");
        assert!(error.to_string().contains("sync_data failure"));
        assert_eq!(journal.next_sequence(), 2);
        assert_eq!(journal.segments().len(), 1);
        assert_eq!(
            journal
                .append("two".to_string())
                .expect("retry after barrier recovery")
                .record
                .sequence,
            2
        );
        assert_eq!(journal.segments().len(), 2);
        drop(journal);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn ambiguous_segment_creation_is_adopted_on_retry() {
        let root = temp_root("create-ambiguity");
        let journal = open_strings(&root, 1, FileDurability::Flush);
        journal.append("one".to_string()).expect("append first");
        *journal
            .create_fault
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = true;
        let error = journal
            .append("two".to_string())
            .expect_err("injected create ambiguity must fail append");
        assert!(
            error
                .to_string()
                .contains("after creating rollover segment")
        );
        assert_eq!(journal.next_sequence(), 2);
        assert_eq!(journal.segments().len(), 1);
        let ambiguous_path = segment_path(&root, 2);
        assert_eq!(
            std::fs::metadata(&ambiguous_path)
                .expect("ambiguous segment metadata")
                .len(),
            0
        );

        let receipt = journal
            .append("two".to_string())
            .expect("retry adopts expected empty segment");
        assert_eq!(receipt.record.sequence, 2);
        assert_eq!(journal.segments().len(), 2);
        assert_eq!(
            journal.replay_after(0, usize::MAX).expect("replay").len(),
            2
        );
        drop(journal);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn full_write_error_returns_degraded_receipt() {
        let root = temp_root("full-write");
        let journal = open_strings(&root, 4096, FileDurability::SyncData);
        *journal
            .append_fault
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(AppendFault::FullWrite);
        let receipt = journal
            .append("one".to_string())
            .expect("full record owns sequence");
        assert_eq!(receipt.record.sequence, 1);
        assert!(matches!(
            receipt.durability,
            JournalDurabilityStatus::Degraded { .. }
        ));
        assert_eq!(journal.last_sequence(), 1);
        drop(journal);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn partial_write_is_repaired_before_sequence_reuse() {
        let root = temp_root("partial-write");
        let journal = open_strings(&root, 4096, FileDurability::Flush);
        *journal
            .append_fault
            .lock()
            .unwrap_or_else(|error| error.into_inner()) =
            Some(AppendFault::PartialWrite { bytes: 11 });
        assert!(journal.append("partial".to_string()).is_err());
        assert_eq!(journal.next_sequence(), 1);
        let receipt = journal
            .append("committed".to_string())
            .expect("retry after repair");
        assert_eq!(receipt.record.sequence, 1);
        drop(journal);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn unrecognized_append_suffix_poisons_until_reopen() {
        let root = temp_root("poison");
        let journal = open_strings(&root, 4096, FileDurability::Flush);
        *journal
            .append_fault
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(AppendFault::UnrecognizedSuffix);
        let error = journal
            .append("first".to_string())
            .expect_err("unrecognized suffix must fail");
        assert!(error.to_string().contains("handle poisoned"));
        let poisoned = journal
            .append("second".to_string())
            .expect_err("poisoned handle must reject append");
        assert!(poisoned.to_string().contains("handle is poisoned"));
        let reopened = open_strings(&root, 4096, FileDurability::Flush);
        assert!(Arc::ptr_eq(&journal.shared, &reopened.shared));
        assert_eq!(
            reopened
                .append("recovered".to_string())
                .expect("append after reopen")
                .record
                .sequence,
            1
        );
        drop(journal);
        drop(reopened);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn bounded_replay_crosses_segments() {
        let root = temp_root("bounded-replay");
        let journal = open_strings(&root, 1, FileDurability::Flush);
        for value in 1..=6 {
            journal.append(format!("event-{value}")).expect("append");
        }
        let replay = journal.replay_after(1, 3).expect("bounded replay");
        assert_eq!(
            replay
                .iter()
                .map(|record| record.sequence)
                .collect::<Vec<_>>(),
            vec![2, 3, 4]
        );
        drop(journal);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn prune_removes_only_whole_closed_segments() {
        let root = temp_root("prune");
        let journal = open_strings(&root, 1, FileDurability::Flush);
        for value in ["one", "two", "three"] {
            journal.append(value.to_string()).expect("append");
        }
        let active_path = journal
            .segments()
            .into_iter()
            .find(|segment| segment.active)
            .expect("active segment")
            .path;
        let removed = journal
            .prune_closed_segments_before(3)
            .expect("prune closed segments");
        assert_eq!(removed.len(), 2);
        assert!(removed.iter().all(|segment| !segment.active));
        assert!(active_path.exists());
        let remaining = journal.segments();
        assert_eq!(remaining.len(), 1);
        assert!(remaining.first().is_some_and(|segment| segment.active));
        drop(journal);
        let reopened = open_strings(&root, 1, FileDurability::Flush);
        assert_eq!(reopened.last_sequence(), 3);
        assert_eq!(
            reopened
                .replay_after(0, usize::MAX)
                .expect("replay retained")
                .len(),
            1
        );
        drop(reopened);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn duplicate_open_shares_authority_and_mismatch_rejects() {
        let root = temp_root("duplicate");
        let first = open_strings(&root, 4096, FileDurability::Flush);
        let alias = root.join(".");
        let second = open_strings(&alias, 4096, FileDurability::Flush);
        assert!(Arc::ptr_eq(&first.shared, &second.shared));
        let max_error =
            SegmentedFileEventJournal::<String>::open(&root, 8192, FileDurability::Flush)
                .expect_err("mismatched max bytes must reject");
        assert!(max_error.to_string().contains("different configuration"));
        let durability_error =
            SegmentedFileEventJournal::<String>::open(&root, 4096, FileDurability::SyncData)
                .expect_err("mismatched durability must reject");
        assert!(
            durability_error
                .to_string()
                .contains("different configuration")
        );
        drop(first);
        drop(second);
        std::fs::remove_dir_all(root).ok();
    }

    const LEASE_PROBE_ENV: &str = "ECHO_SEGMENTED_JOURNAL_LEASE_PROBE";

    #[test]
    fn process_lease_probe() {
        let Ok(root) = std::env::var(LEASE_PROBE_ENV) else {
            return;
        };
        let error = SegmentedFileEventJournal::<String>::open(root, 4096, FileDurability::Flush)
            .expect_err("competing process must fail open");
        assert!(
            error
                .to_string()
                .contains("already open in another process")
        );
    }

    #[test]
    fn competing_process_fails_open_while_lease_is_alive() {
        let root = temp_root("process-lease");
        let journal = open_strings(&root, 4096, FileDurability::Flush);
        let output =
            std::process::Command::new(std::env::current_exe().expect("current test binary"))
                .arg("journal::segmented::tests::process_lease_probe")
                .arg("--exact")
                .arg("--nocapture")
                .env(LEASE_PROBE_ENV, &root)
                .output()
                .expect("run lease probe");
        assert!(
            output.status.success(),
            "lease probe failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        drop(journal);
        std::fs::remove_dir_all(root).ok();
    }

    #[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
    struct NonCloneEvent {
        value: String,
    }

    #[test]
    fn non_clone_events_append_and_replay_without_serde_copy() {
        let memory = super::super::MemoryEventJournal::new();
        let receipt = memory
            .append(NonCloneEvent {
                value: "memory".to_string(),
            })
            .expect("memory append");
        assert_eq!(receipt.record.event.value, "memory");
        assert_eq!(
            memory
                .replay_after(0, 1)
                .expect("memory replay")
                .first()
                .map(|record| record.event.value.as_str()),
            Some("memory")
        );

        let root = temp_root("non-clone");
        let journal = SegmentedFileEventJournal::open(&root, 4096, FileDurability::Flush)
            .expect("open non-clone journal");
        journal
            .append(NonCloneEvent {
                value: "file".to_string(),
            })
            .expect("file append");
        assert_eq!(
            journal
                .replay_after(0, 1)
                .expect("file replay")
                .first()
                .map(|record| record.event.value.as_str()),
            Some("file")
        );
        drop(journal);
        std::fs::remove_dir_all(root).ok();
    }
}
