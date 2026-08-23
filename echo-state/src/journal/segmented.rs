//! Directory-backed segmented event journal.
//!
//! Kafka's log model uses immutable segments while offsets remain global. This
//! implementation applies that storage shape to the framework journal: every
//! segment name is its first global sequence, closed segments are immutable,
//! and only the active segment may repair a crash-torn tail. LangGraph-style
//! checkpoints remain derived state above this authoritative event history;
//! callers choose retention and pin policy by passing a keep cursor to prune.
//! Each append may select `Flush` or `SyncData` without opening a second
//! authority; event classification remains a caller policy.
//! Product stream identities, retention counts, and UI projections do not live
//! in this module.

use super::{
    EventJournal, JournalAppendReceipt, JournalDurabilityStatus, JournalEvent, JournalRecord,
    WeakRegistry,
};
use echo_core::error::{ReactError, Result};
use echo_core::utils::canonical_json::canonical_json_bytes;
use echo_core::utils::fs::{
    ExclusiveFileLease, ExistingDirectoryGuard, ExistingRegularFileGuard, FileDurability,
    append_existing_matching, atomic_write, create_dir_all_durable, matching_existing_regular_len,
    open_existing_directory_guard, open_existing_regular_guard, read_existing,
    read_existing_from_matching, read_existing_lines_from, read_existing_lines_from_matching,
    read_existing_matching, sync_existing_directory_matching, truncate_existing_matching,
    try_exclusive_file_lease, verify_existing_directory,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::any::TypeId;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

const SEGMENT_SUFFIX: &str = ".jsonl";
const SEGMENT_DIGITS: usize = 20;
const LEASE_AUTHORITY: &str = "segmented-event-journal";
const RETENTION_MARKER: &str = ".retained-floor.json";
const RETENTION_SCHEMA_VERSION: u16 = 1;

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

#[derive(Debug, Serialize)]
struct RetentionIntegrity {
    schema_version: u16,
    retained_floor: u64,
    cleanup_pending: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetentionMarker {
    schema_version: u16,
    retained_floor: u64,
    cleanup_pending: bool,
    digest: String,
}

enum MarkerWriteStatus {
    Confirmed,
    Degraded { error: String },
}

fn integrity_digest<T: Serialize>(value: &T) -> Result<String> {
    let bytes = canonical_json_bytes(value).map_err(|error| {
        journal_error(format!("failed to encode journal integrity input: {error}"))
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

fn record_digest<E: Serialize>(sequence: u64, event: &E) -> Result<String> {
    integrity_digest(&IntegrityPayload { sequence, event })
}

fn retention_digest(
    schema_version: u16,
    retained_floor: u64,
    cleanup_pending: bool,
) -> Result<String> {
    integrity_digest(&RetentionIntegrity {
        schema_version,
        retained_floor,
        cleanup_pending,
    })
}

fn load_retention_marker(directory: &Path, context: &str) -> Result<Option<RetentionMarker>> {
    let path = directory.join(RETENTION_MARKER);
    let bytes = match read_existing(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error(context, error)),
    };
    let marker: RetentionMarker = serde_json::from_slice(&bytes).map_err(|error| {
        journal_error(format!(
            "{context}: corrupt retained-floor marker {}: {error}",
            path.display()
        ))
    })?;
    if marker.schema_version != RETENTION_SCHEMA_VERSION {
        return Err(journal_error(format!(
            "{context}: unsupported retained-floor marker schema {}",
            marker.schema_version
        )));
    }
    if marker.retained_floor == 0 {
        return Err(journal_error(format!(
            "{context}: retained-floor marker must be positive"
        )));
    }
    let expected = retention_digest(
        marker.schema_version,
        marker.retained_floor,
        marker.cleanup_pending,
    )?;
    if marker.digest != expected {
        return Err(journal_error(format!(
            "{context}: retained-floor marker digest mismatch"
        )));
    }
    Ok(Some(marker))
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
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.ends_with(SEGMENT_SUFFIX) {
            continue;
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
    active_file_guard: Option<Arc<ExistingRegularFileGuard>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OpaqueSegmentState {
    path: PathBuf,
    start_sequence: u64,
    end_sequence: Option<u64>,
    bytes: u64,
}

impl OpaqueSegmentState {
    fn metadata(&self) -> JournalPhysicalSegmentMetadata {
        JournalPhysicalSegmentMetadata {
            path: self.path.clone(),
            start_sequence: self.start_sequence,
            end_sequence: self.end_sequence,
            observed_bytes: self.bytes,
        }
    }
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
    let active_file_guard = if active {
        Some(Arc::new(
            open_existing_regular_guard(path).map_err(|error| io_error(context, error))?,
        ))
    } else {
        None
    };
    let bytes = match &active_file_guard {
        Some(file_guard) => read_existing_matching(path, file_guard),
        None => read_existing(path),
    }
    .map_err(|error| io_error(context, error))?;
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
        let file_guard = active_file_guard.as_ref().ok_or_else(|| {
            journal_error(format!(
                "{context}: active segment identity guard is missing"
            ))
        })?;
        truncate_existing_matching(path, file_guard, file_len, valid_len, durability)
            .map_err(|error| io_error(context, error))?;
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
        active_file_guard,
    })
}

fn ensure_journal_directory(directory: &Path, context: &str) -> Result<()> {
    create_dir_all_durable(directory).map_err(|error| io_error(context, error))?;
    let metadata =
        std::fs::symlink_metadata(directory).map_err(|error| io_error(context, error))?;
    if !metadata.file_type().is_dir() {
        return Err(journal_error(format!(
            "{context}: journal root is not a directory"
        )));
    }
    Ok(())
}

fn create_segment(
    directory: &Path,
    directory_guard: &ExistingDirectoryGuard,
    start_sequence: u64,
    context: &str,
) -> Result<SegmentState> {
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
    sync_existing_directory_matching(directory, directory_guard)
        .map_err(|error| io_error(context, error))?;
    let active_file_guard =
        Arc::new(open_existing_regular_guard(&path).map_err(|error| io_error(context, error))?);
    Ok(SegmentState {
        path,
        start_sequence,
        end_sequence: start_sequence.saturating_sub(1),
        bytes: 0,
        record_offsets: Vec::new(),
        active_file_guard: Some(active_file_guard),
    })
}

fn scan_directory<E: JournalEvent>(
    directory: &Path,
    directory_guard: &ExistingDirectoryGuard,
    durability: FileDurability,
    context: &str,
    create_if_empty: bool,
) -> Result<SegmentedJournalState> {
    let listed = list_segment_paths(directory, context)?;
    let marker = load_retention_marker(directory, context)?;
    if listed.is_empty() {
        if marker.is_some() {
            return Err(journal_error(format!(
                "{context}: retained-floor marker exists but every journal segment is missing"
            )));
        }
        if !create_if_empty {
            return Err(journal_error(format!(
                "{context}: live journal has no segment files; close the authority before verified repair"
            )));
        }
        let active = create_segment(directory, directory_guard, 1, context)?;
        return Ok(SegmentedJournalState {
            next_sequence: 1,
            segments: vec![active],
            obsolete_segments: Vec::new(),
            retained_floor: 1,
            cleanup_pending: false,
            marker_barrier_pending: false,
            poison: None,
        });
    }
    let (retained_floor, cleanup_pending) = marker
        .as_ref()
        .map(|marker| (marker.retained_floor, marker.cleanup_pending))
        .unwrap_or((1, false));
    let mut obsolete_paths = Vec::new();
    let mut logical_paths = Vec::new();
    for (start, path) in listed {
        if start < retained_floor {
            obsolete_paths.push((start, path));
        } else {
            logical_paths.push((start, path));
        }
    }
    if !cleanup_pending && !obsolete_paths.is_empty() {
        let first_start = obsolete_paths.first().map(|(start, _)| *start).unwrap_or(0);
        return Err(journal_error(format!(
            "{context}: retained-floor cleanup is confirmed but prefix segment {first_start} remains"
        )));
    }
    let first_logical = logical_paths
        .first()
        .map(|(start, _)| *start)
        .ok_or_else(|| {
            journal_error(format!(
                "{context}: no segment starts at retained floor {retained_floor}"
            ))
        })?;
    if first_logical != retained_floor {
        let marker_context = if marker.is_some() {
            format!("missing segment at retained floor {retained_floor}, found {first_logical}")
        } else {
            format!(
                "journal prefix starts at {first_logical} without an authorized retained-floor marker"
            )
        };
        return Err(journal_error(format!("{context}: {marker_context}")));
    }
    let mut obsolete_segments = Vec::with_capacity(obsolete_paths.len());
    for (start, path) in &obsolete_paths {
        obsolete_segments.push(OpaqueSegmentState {
            path: path.clone(),
            start_sequence: *start,
            end_sequence: None,
            bytes: std::fs::symlink_metadata(path)
                .map(|metadata| metadata.len())
                .unwrap_or(0),
        });
    }
    let mut segments = Vec::with_capacity(logical_paths.len());
    let last_index = logical_paths.len().saturating_sub(1);
    let mut expected_start = None;
    for (index, (start, path)) in logical_paths.into_iter().enumerate() {
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
        obsolete_segments,
        retained_floor,
        cleanup_pending,
        marker_barrier_pending: false,
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

/// Observed filesystem metadata for one physically removed opaque segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalPhysicalSegmentMetadata {
    /// Removed path.
    pub path: PathBuf,
    /// Start sequence encoded by the validated file name.
    pub start_sequence: u64,
    /// Exact end only when this process previously validated the segment.
    pub end_sequence: Option<u64>,
    /// File size observed before physical deletion.
    pub observed_bytes: u64,
}

/// Durable logical retention state for a segmented journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JournalRetentionMetadata {
    /// Earliest sequence callers may request from replay.
    pub retained_floor: u64,
    /// Physical prefix deletion or its directory barrier still needs retry.
    pub cleanup_pending: bool,
}

/// Outcome of physical cleanup after a logical prune commits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalPhysicalCleanupStatus {
    /// Every logically pruned file was removed and the directory was synced.
    Confirmed,
    /// The logical floor committed, but physical cleanup remains retryable.
    Degraded { error: String },
}

/// Durability of the logical retained-floor marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalPruneCommitStatus {
    /// The logical retained floor and its parent-directory barrier completed.
    Confirmed,
    /// The complete marker is visible, but its directory barrier needs retry.
    Degraded { error: String },
}

/// Typed receipt for whole-segment pruning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalPruneReceipt {
    /// Durable earliest logically retained sequence.
    pub retained_floor: u64,
    /// Durability of the logical retained-floor transition.
    pub commit: JournalPruneCommitStatus,
    /// Segments newly excluded from logical replay by this call.
    pub logically_pruned: Vec<JournalSegmentMetadata>,
    /// Segment files physically removed by this call.
    pub physically_removed: Vec<JournalPhysicalSegmentMetadata>,
    /// Whether physical deletion and its directory barrier completed.
    pub cleanup: JournalPhysicalCleanupStatus,
}

#[derive(Debug)]
struct SegmentedJournalState {
    next_sequence: u64,
    segments: Vec<SegmentState>,
    obsolete_segments: Vec<OpaqueSegmentState>,
    retained_floor: u64,
    cleanup_pending: bool,
    marker_barrier_pending: bool,
    poison: Option<String>,
}

#[derive(Debug)]
struct SharedSegmentedJournalState {
    event_type: TypeId,
    max_active_segment_bytes: u64,
    durability: FileDurability,
    state: Mutex<SegmentedJournalState>,
    directory_guard: ExistingDirectoryGuard,
    _lease: ExclusiveFileLease,
}

fn segment_layout_matches(left: &[SegmentState], right: &[SegmentState]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.path == right.path
                && left.start_sequence == right.start_sequence
                && left.end_sequence == right.end_sequence
                && left.bytes == right.bytes
                && left.record_offsets == right.record_offsets
        })
}

fn opaque_layout_matches(left: &[OpaqueSegmentState], right: &[OpaqueSegmentState]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.path == right.path
                && left.start_sequence == right.start_sequence
                && left.bytes == right.bytes
        })
}

fn verify_active_segment_identity(state: &SegmentedJournalState, context: &str) -> Result<()> {
    let active = state
        .segments
        .last()
        .ok_or_else(|| journal_error(format!("{context}: active segment is missing")))?;
    let file_guard = active.active_file_guard.as_ref().ok_or_else(|| {
        journal_error(format!(
            "{context}: active segment identity guard is missing"
        ))
    })?;
    let len = matching_existing_regular_len(&active.path, file_guard)
        .map_err(|error| io_error(context, error))?;
    if len != active.bytes {
        return Err(journal_error(format!(
            "{context}: active segment length changed from {} to {len}; verified reopen required",
            active.bytes
        )));
    }
    Ok(())
}

fn verify_live_active_segment_for_repair(
    state: &SegmentedJournalState,
    context: &str,
) -> Result<()> {
    let active = state
        .segments
        .last()
        .ok_or_else(|| journal_error(format!("{context}: active segment is missing")))?;
    let file_guard = active.active_file_guard.as_ref().ok_or_else(|| {
        journal_error(format!(
            "{context}: active segment identity guard is missing"
        ))
    })?;
    let len = matching_existing_regular_len(&active.path, file_guard)
        .map_err(|error| io_error(context, error))?;
    if len < active.bytes {
        return Err(journal_error(format!(
            "{context}: active segment shrank from {} to {len}; close the authority before verified repair",
            active.bytes
        )));
    }
    Ok(())
}

fn segmented_registry() -> &'static Mutex<WeakRegistry<SharedSegmentedJournalState>> {
    static REGISTRY: OnceLock<Mutex<WeakRegistry<SharedSegmentedJournalState>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(WeakRegistry::new()))
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
    shared: Option<Arc<SharedSegmentedJournalState>>,
    #[cfg(test)]
    append_fault: Mutex<Option<AppendFault>>,
    #[cfg(test)]
    append_durabilities: Mutex<Vec<FileDurability>>,
    #[cfg(test)]
    sync_fault: Mutex<bool>,
    #[cfg(test)]
    create_fault: Mutex<bool>,
    #[cfg(test)]
    marker_write_fault_after: Mutex<Option<usize>>,
    #[cfg(test)]
    marker_full_write_fault: Mutex<bool>,
    #[cfg(test)]
    delete_fault_after: Mutex<Option<usize>>,
    #[cfg(test)]
    directory_sync_fault: Mutex<bool>,
    #[cfg(test)]
    directory_sync_attempts: std::sync::atomic::AtomicUsize,
    _event: PhantomData<fn() -> E>,
}

impl<E> Drop for SegmentedFileEventJournal<E> {
    fn drop(&mut self) {
        let Some(shared) = self.shared.take() else {
            return;
        };
        let mut registry = segmented_registry()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        registry.release_handle(&self.directory, &shared);
        drop(shared);
        drop(registry);
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Copy)]
enum AppendFault {
    FullWrite,
    PartialWrite { bytes: usize },
    UnrecognizedSuffix,
}

impl<E: JournalEvent> SegmentedFileEventJournal<E> {
    fn from_shared(directory: PathBuf, shared: Arc<SharedSegmentedJournalState>) -> Self {
        Self {
            directory,
            shared: Some(shared),
            #[cfg(test)]
            append_fault: Mutex::new(None),
            #[cfg(test)]
            append_durabilities: Mutex::new(Vec::new()),
            #[cfg(test)]
            sync_fault: Mutex::new(false),
            #[cfg(test)]
            create_fault: Mutex::new(false),
            #[cfg(test)]
            marker_write_fault_after: Mutex::new(None),
            #[cfg(test)]
            marker_full_write_fault: Mutex::new(false),
            #[cfg(test)]
            delete_fault_after: Mutex::new(None),
            #[cfg(test)]
            directory_sync_fault: Mutex::new(false),
            #[cfg(test)]
            directory_sync_attempts: std::sync::atomic::AtomicUsize::new(0),
            _event: PhantomData,
        }
    }

    fn shared(&self) -> Result<&Arc<SharedSegmentedJournalState>> {
        self.shared
            .as_ref()
            .ok_or_else(|| journal_error("segmented journal handle is already closing"))
    }

    fn shared_io(&self) -> std::io::Result<&Arc<SharedSegmentedJournalState>> {
        self.shared
            .as_ref()
            .ok_or_else(|| std::io::Error::other("segmented journal handle is already closing"))
    }

    fn verify_directory_identity(&self, context: &str) -> Result<()> {
        verify_existing_directory(&self.directory, &self.shared()?.directory_guard)
            .map_err(|error| io_error(context, error))
    }

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
        let requested = directory.into();
        let context = format!("segmented journal open {}", requested.display());
        let parent = requested
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        create_dir_all_durable(parent).map_err(|error| io_error(&context, error))?;
        let canonical_parent =
            std::fs::canonicalize(parent).map_err(|error| io_error(&context, error))?;
        let name = requested.file_name().ok_or_else(|| {
            journal_error(format!("{context}: journal directory has no final name"))
        })?;
        let directory = canonical_parent.join(name);
        let context = format!("segmented journal open {}", directory.display());
        let mut registry = segmented_registry().lock().map_err(|error| {
            journal_error(format!("segmented journal registry lock poisoned: {error}"))
        })?;
        registry.prune_dead_if_due();
        if let Some(shared) = registry.upgrade(&directory) {
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
            verify_existing_directory(&directory, &shared.directory_guard)
                .map_err(|error| io_error(&context, error))?;
            verify_live_active_segment_for_repair(&state, &context)?;
            let marker_barrier_pending = state.marker_barrier_pending;
            let mut rescanned = scan_directory::<E>(
                &directory,
                &shared.directory_guard,
                durability,
                &context,
                false,
            )?;
            verify_existing_directory(&directory, &shared.directory_guard)
                .map_err(|error| io_error(&context, error))?;
            rescanned.marker_barrier_pending |= marker_barrier_pending;
            rescanned.cleanup_pending |= marker_barrier_pending;
            if rescanned.next_sequence != state.next_sequence
                || rescanned.retained_floor != state.retained_floor
                || rescanned.cleanup_pending != state.cleanup_pending
                || !segment_layout_matches(&rescanned.segments, &state.segments)
                || !opaque_layout_matches(&rescanned.obsolete_segments, &state.obsolete_segments)
            {
                return Err(journal_error(format!(
                    "{context}: disk layout diverged from the live authority; close it before verified reopen"
                )));
            }
            if !registry.add_handle(&directory, &shared) {
                return Err(journal_error(format!(
                    "{context}: failed to register shared journal handle"
                )));
            }
            state.poison = None;
            drop(state);
            return Ok(Self::from_shared(directory, shared));
        }
        ensure_journal_directory(&directory, &context)?;
        let directory_guard =
            open_existing_directory_guard(&directory).map_err(|error| io_error(&context, error))?;
        let lease = try_exclusive_file_lease(&directory.join(LEASE_AUTHORITY))
            .map_err(|error| io_error(&context, error))?;
        verify_existing_directory(&directory, &directory_guard)
            .map_err(|error| io_error(&context, error))?;
        let state = scan_directory::<E>(&directory, &directory_guard, durability, &context, true)?;
        verify_existing_directory(&directory, &directory_guard)
            .map_err(|error| io_error(&context, error))?;
        let shared = Arc::new(SharedSegmentedJournalState {
            event_type: TypeId::of::<E>(),
            max_active_segment_bytes,
            durability,
            state: Mutex::new(state),
            directory_guard,
            _lease: lease,
        });
        registry.insert(directory.clone(), &shared);
        Ok(Self::from_shared(directory, shared))
    }

    /// Canonical journal directory.
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Snapshot retained segment ranges and sizes.
    pub fn segments(&self) -> Vec<JournalSegmentMetadata> {
        let Some(shared) = self.shared.as_ref() else {
            return Vec::new();
        };
        let state = shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let last = state.segments.len().saturating_sub(1);
        state
            .segments
            .iter()
            .enumerate()
            .filter(|(_, segment)| segment.start_sequence >= state.retained_floor)
            .map(|(index, segment)| segment.metadata(index == last))
            .collect()
    }

    /// Read the durable logical replay floor and pending cleanup state.
    pub fn retention_metadata(&self) -> JournalRetentionMetadata {
        let Some(shared) = self.shared.as_ref() else {
            return JournalRetentionMetadata {
                retained_floor: 1,
                cleanup_pending: false,
            };
        };
        let state = shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        JournalRetentionMetadata {
            retained_floor: state.retained_floor,
            cleanup_pending: state.cleanup_pending,
        }
    }

    /// Force a data durability barrier and finish pending prune cleanup.
    pub fn sync_data(&self) -> Result<()> {
        let shared = self.shared()?;
        let mut state = shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.verify_directory_identity("segmented journal sync_data")?;
        verify_active_segment_identity(&state, "segmented journal sync_data")?;
        let active = state
            .segments
            .last()
            .ok_or_else(|| journal_error("segmented journal has no active segment"))?;
        let context = format!("segmented journal sync_data {}", active.path.display());
        self.sync_active(active)
            .map_err(|error| io_error(&context, error))?;
        if state.marker_barrier_pending {
            self.confirm_marker_barrier(&mut state)?;
        }
        if state.cleanup_pending {
            let (_, cleanup) = self.finish_pending_cleanup(&mut state);
            if let JournalPhysicalCleanupStatus::Degraded { error } = cleanup {
                return Err(journal_error(format!(
                    "segmented journal pending cleanup remains degraded: {error}"
                )));
            }
        }
        Ok(())
    }

    /// Remove complete closed segments strictly before the caller's keep
    /// cursor. The active segment is never removed and no retention policy is
    /// inferred by the framework. The caller must first persist a checkpoint
    /// through `retained_floor - 1` and preserve any product-level pins.
    ///
    /// Once the retained-floor marker commits, this returns a typed receipt
    /// even if physical deletion is degraded. Retry this method or call
    /// [`Self::sync_data`] to finish pending cleanup.
    pub fn prune_closed_segments_before(
        &self,
        keep_from_sequence: u64,
    ) -> Result<JournalPruneReceipt> {
        let shared = self.shared()?;
        let mut state = shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.verify_directory_identity("segmented journal prune")?;
        verify_active_segment_identity(&state, "segmented journal prune")?;
        if state.marker_barrier_pending
            && let Err(error) = self.confirm_marker_barrier(&mut state)
        {
            let message = error.to_string();
            return Ok(JournalPruneReceipt {
                retained_floor: state.retained_floor,
                commit: JournalPruneCommitStatus::Degraded {
                    error: message.clone(),
                },
                logically_pruned: Vec::new(),
                physically_removed: Vec::new(),
                cleanup: JournalPhysicalCleanupStatus::Degraded { error: message },
            });
        }
        let active_index = state.segments.len().saturating_sub(1);
        let candidate_count = state
            .segments
            .iter()
            .enumerate()
            .filter(|(index, segment)| {
                *index < active_index && segment.end_sequence < keep_from_sequence
            })
            .count();
        let requested_floor = state
            .segments
            .get(candidate_count)
            .map(|segment| segment.start_sequence)
            .unwrap_or(state.retained_floor);
        let target_floor = requested_floor.max(state.retained_floor);
        let logically_pruned = state
            .segments
            .iter()
            .filter(|segment| {
                segment.start_sequence >= state.retained_floor
                    && segment.start_sequence < target_floor
            })
            .map(|segment| segment.metadata(false))
            .collect::<Vec<_>>();
        let commit = if target_floor > state.retained_floor {
            match self.write_retention_marker(target_floor, true)? {
                MarkerWriteStatus::Confirmed => {
                    state.retained_floor = target_floor;
                    state.cleanup_pending = true;
                    Self::move_logical_prefix_to_obsolete(&mut state);
                    JournalPruneCommitStatus::Confirmed
                }
                MarkerWriteStatus::Degraded { error } => {
                    state.retained_floor = target_floor;
                    state.cleanup_pending = true;
                    state.marker_barrier_pending = true;
                    Self::move_logical_prefix_to_obsolete(&mut state);
                    return Ok(JournalPruneReceipt {
                        retained_floor: target_floor,
                        commit: JournalPruneCommitStatus::Degraded {
                            error: error.clone(),
                        },
                        logically_pruned,
                        physically_removed: Vec::new(),
                        cleanup: JournalPhysicalCleanupStatus::Degraded {
                            error: format!(
                                "physical cleanup blocked on retained-floor barrier: {error}"
                            ),
                        },
                    });
                }
            }
        } else {
            JournalPruneCommitStatus::Confirmed
        };
        let (physically_removed, cleanup) = if state.cleanup_pending {
            self.finish_pending_cleanup(&mut state)
        } else {
            (Vec::new(), JournalPhysicalCleanupStatus::Confirmed)
        };
        Ok(JournalPruneReceipt {
            retained_floor: state.retained_floor,
            commit,
            logically_pruned,
            physically_removed,
            cleanup,
        })
    }

    fn move_logical_prefix_to_obsolete(state: &mut SegmentedJournalState) {
        let mut retained = Vec::with_capacity(state.segments.len());
        for segment in std::mem::take(&mut state.segments) {
            if segment.start_sequence < state.retained_floor {
                state.obsolete_segments.push(OpaqueSegmentState {
                    path: segment.path,
                    start_sequence: segment.start_sequence,
                    end_sequence: Some(segment.end_sequence),
                    bytes: segment.bytes,
                });
            } else {
                retained.push(segment);
            }
        }
        state
            .obsolete_segments
            .sort_by_key(|segment| segment.start_sequence);
        state.segments = retained;
    }

    fn write_retention_marker(
        &self,
        retained_floor: u64,
        cleanup_pending: bool,
    ) -> Result<MarkerWriteStatus> {
        #[cfg(test)]
        {
            let mut fault = self
                .marker_write_fault_after
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(remaining) = fault.as_mut() {
                if *remaining == 0 {
                    *fault = None;
                    return Err(journal_error(
                        "injected retained-floor marker write failure",
                    ));
                }
                *remaining = remaining.saturating_sub(1);
            }
        }
        let marker = RetentionMarker {
            schema_version: RETENTION_SCHEMA_VERSION,
            retained_floor,
            cleanup_pending,
            digest: retention_digest(RETENTION_SCHEMA_VERSION, retained_floor, cleanup_pending)?,
        };
        let bytes = serde_json::to_vec(&marker).map_err(|error| {
            journal_error(format!("failed to encode retained-floor marker: {error}"))
        })?;
        let path = self.directory.join(RETENTION_MARKER);
        let write_result = atomic_write(&path, &bytes);
        #[cfg(test)]
        if write_result.is_ok()
            && std::mem::take(
                &mut *self
                    .marker_full_write_fault
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()),
            )
        {
            return Ok(MarkerWriteStatus::Degraded {
                error: "injected visible marker parent barrier failure".to_string(),
            });
        }
        match write_result {
            Ok(()) => Ok(MarkerWriteStatus::Confirmed),
            Err(error) => {
                let reconciled = load_retention_marker(
                    &self.directory,
                    "segmented journal marker write reconciliation",
                );
                if reconciled.is_ok_and(|marker| {
                    marker.is_some_and(|marker| {
                        marker.schema_version == RETENTION_SCHEMA_VERSION
                            && marker.retained_floor == retained_floor
                            && marker.cleanup_pending == cleanup_pending
                    })
                }) {
                    Ok(MarkerWriteStatus::Degraded {
                        error: error.to_string(),
                    })
                } else {
                    Err(io_error(
                        &format!("segmented journal retention marker {}", path.display()),
                        error,
                    ))
                }
            }
        }
    }

    fn confirm_marker_barrier(&self, state: &mut SegmentedJournalState) -> Result<()> {
        self.sync_segment_directory()
            .map_err(|error| io_error("segmented journal retained-floor barrier retry", error))?;
        let marker = load_retention_marker(
            &self.directory,
            "segmented journal retained-floor barrier retry",
        )?
        .ok_or_else(|| journal_error("retained-floor marker disappeared before barrier retry"))?;
        state.marker_barrier_pending = false;
        state.cleanup_pending = marker.cleanup_pending;
        Ok(())
    }

    fn finish_pending_cleanup(
        &self,
        state: &mut SegmentedJournalState,
    ) -> (
        Vec<JournalPhysicalSegmentMetadata>,
        JournalPhysicalCleanupStatus,
    ) {
        let candidates = state.obsolete_segments.clone();
        let mut removed = Vec::new();
        let mut deletion_error = None;
        for candidate in candidates {
            if let Err(error) = self.remove_pruned_segment(&candidate.path) {
                deletion_error = Some(format!(
                    "failed to remove {}: {error}",
                    candidate.path.display()
                ));
                break;
            }
            state
                .obsolete_segments
                .retain(|segment| segment.path != candidate.path);
            removed.push(candidate.metadata());
        }
        let sync_error = self
            .sync_segment_directory()
            .err()
            .map(|error| format!("directory sync failed: {error}"));
        if deletion_error.is_some() || sync_error.is_some() {
            state.cleanup_pending = true;
            let error = [deletion_error, sync_error]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join("; ");
            return (removed, JournalPhysicalCleanupStatus::Degraded { error });
        }
        match self.write_retention_marker(state.retained_floor, false) {
            Ok(MarkerWriteStatus::Confirmed) => {}
            Ok(MarkerWriteStatus::Degraded { error }) => {
                state.cleanup_pending = true;
                state.marker_barrier_pending = true;
                return (removed, JournalPhysicalCleanupStatus::Degraded { error });
            }
            Err(error) => {
                state.cleanup_pending = true;
                return (
                    removed,
                    JournalPhysicalCleanupStatus::Degraded {
                        error: error.to_string(),
                    },
                );
            }
        }
        state.cleanup_pending = false;
        (removed, JournalPhysicalCleanupStatus::Confirmed)
    }

    fn remove_pruned_segment(&self, path: &Path) -> std::io::Result<()> {
        #[cfg(test)]
        {
            let mut fault = self
                .delete_fault_after
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(remaining) = fault.as_mut() {
                if *remaining == 0 {
                    *fault = None;
                    return Err(std::io::Error::other(
                        "injected segmented prune delete failure",
                    ));
                }
                *remaining = remaining.saturating_sub(1);
            }
        }
        std::fs::remove_file(path)
    }

    fn sync_segment_directory(&self) -> std::io::Result<()> {
        let shared = self.shared_io()?;
        #[cfg(test)]
        {
            use std::sync::atomic::Ordering;

            self.directory_sync_attempts.fetch_add(1, Ordering::SeqCst);
            let mut fail = self
                .directory_sync_fault
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if std::mem::take(&mut *fail) {
                return Err(std::io::Error::other(
                    "injected segmented directory sync failure",
                ));
            }
        }
        sync_existing_directory_matching(&self.directory, &shared.directory_guard)
    }

    fn roll_segment(&self, state: &mut SegmentedJournalState) -> Result<()> {
        let active = state
            .segments
            .last()
            .cloned()
            .ok_or_else(|| journal_error("segmented journal has no active segment"))?;
        let context = format!("segmented journal rollover {}", active.path.display());
        self.sync_active(&active)
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
        let next = create_segment(
            &self.directory,
            &self.shared()?.directory_guard,
            state.next_sequence,
            &context,
        )?;
        if let Some(previous) = state.segments.last_mut() {
            previous.active_file_guard = None;
        }
        state.segments.push(next);
        Ok(())
    }

    fn sync_active(&self, active: &SegmentState) -> std::io::Result<()> {
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
        let file_guard = active
            .active_file_guard
            .as_ref()
            .ok_or_else(|| std::io::Error::other("active segment identity guard is missing"))?;
        append_existing_matching(
            &active.path,
            file_guard,
            active.bytes,
            b"",
            FileDurability::SyncData,
        )
    }

    fn append_line(
        &self,
        active: &SegmentState,
        line: &[u8],
        durability: FileDurability,
    ) -> std::io::Result<()> {
        let file_guard = active
            .active_file_guard
            .as_ref()
            .ok_or_else(|| std::io::Error::other("active segment identity guard is missing"))?;
        #[cfg(test)]
        self.append_durabilities
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(durability);
        #[cfg(test)]
        if let Some(fault) = self
            .append_fault
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        {
            let bytes = match fault {
                AppendFault::FullWrite => line,
                AppendFault::PartialWrite { bytes } => {
                    line.get(..bytes.min(line.len())).unwrap_or(line)
                }
                AppendFault::UnrecognizedSuffix => b"x",
            };
            append_existing_matching(
                &active.path,
                file_guard,
                active.bytes,
                bytes,
                FileDurability::Flush,
            )?;
            return Err(std::io::Error::other(
                "injected segmented append durability failure",
            ));
        }
        append_existing_matching(&active.path, file_guard, active.bytes, line, durability)
    }

    fn reconcile_failed_append(
        &self,
        active: &SegmentState,
        line: &[u8],
        context: &str,
        append_error: &std::io::Error,
        durability: FileDurability,
    ) -> Result<Option<JournalDurabilityStatus>> {
        let file_guard = active.active_file_guard.as_ref().ok_or_else(|| {
            journal_error(format!(
                "{context}: active segment identity guard is missing"
            ))
        })?;
        let suffix = read_existing_from_matching(&active.path, file_guard, active.bytes)
            .map_err(|error| io_error(context, error))?;
        if suffix == line {
            return Ok(Some(JournalDurabilityStatus::Degraded {
                error: append_error.to_string(),
            }));
        }
        if line.starts_with(&suffix) {
            if !suffix.is_empty() {
                let suffix_len = u64::try_from(suffix.len()).map_err(|_| {
                    journal_error(format!("{context}: segment suffix exceeds supported size"))
                })?;
                let current_len = active.bytes.checked_add(suffix_len).ok_or_else(|| {
                    journal_error(format!("{context}: segment byte length exhausted"))
                })?;
                truncate_existing_matching(
                    &active.path,
                    file_guard,
                    current_len,
                    active.bytes,
                    durability,
                )
                .map_err(|error| io_error(context, error))?;
            }
            return Ok(None);
        }
        Err(journal_error(format!(
            "{context}: append error left an unrecognized {}-byte suffix",
            suffix.len()
        )))
    }
    /// Append one event using an event-specific durability policy.
    ///
    /// This uses the same serialized sequence authority as [`EventJournal::append`].
    /// A full write whose requested barrier fails returns a degraded committed
    /// receipt and must not be retried. Partial writes are truncated using the
    /// requested durability before the sequence can be reused.
    pub fn append_with_durability(
        &self,
        event: E,
        durability: FileDurability,
    ) -> Result<JournalAppendReceipt<E>> {
        let shared = self.shared()?;
        let mut state = shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(reason) = &state.poison {
            return Err(journal_error(format!(
                "segmented journal append {} refused because the handle is poisoned: {reason}; reopen the journal to recover",
                self.directory.display()
            )));
        }
        self.verify_directory_identity("segmented journal append")?;
        verify_active_segment_identity(&state, "segmented journal append")?;
        let should_roll = state.segments.last().is_some_and(|active| {
            active.has_records() && active.bytes >= shared.max_active_segment_bytes
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
        let durability_status = match self.append_line(&active, &line, durability) {
            Ok(()) => JournalDurabilityStatus::Confirmed,
            Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
                return Err(io_error(&context, error));
            }
            Err(error) => match self
                .reconcile_failed_append(&active, &line, &context, &error, durability)
            {
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
        Ok(JournalAppendReceipt {
            record,
            durability: durability_status,
        })
    }
}

impl<E: JournalEvent> EventJournal<E> for SegmentedFileEventJournal<E> {
    fn append(&self, event: E) -> Result<JournalAppendReceipt<E>> {
        self.append_with_durability(event, self.shared()?.durability)
    }

    fn next_sequence(&self) -> u64 {
        self.shared
            .as_ref()
            .map(|shared| {
                shared
                    .state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .next_sequence
            })
            .unwrap_or(1)
    }

    fn last_sequence(&self) -> u64 {
        self.next_sequence().saturating_sub(1)
    }

    fn retained_floor(&self) -> u64 {
        self.shared
            .as_ref()
            .map(|shared| {
                shared
                    .state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .retained_floor
            })
            .unwrap_or(1)
    }

    fn replay_after(&self, after_sequence: u64, limit: usize) -> Result<Vec<JournalRecord<E>>> {
        let context = format!("segmented journal replay {}", self.directory.display());
        let shared = self.shared()?;
        let state = shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.verify_directory_identity(&context)?;
        verify_active_segment_identity(&state, &context)?;
        let requested_sequence = after_sequence.saturating_add(1);
        if requested_sequence < state.retained_floor {
            return Err(journal_error(format!(
                "{context}: requested sequence {requested_sequence} is below retained floor {}; restart replay at cursor {}",
                state.retained_floor,
                state.retained_floor.saturating_sub(1)
            )));
        }
        if limit == 0 {
            return Ok(Vec::new());
        }
        if after_sequence >= state.next_sequence.saturating_sub(1) {
            return Ok(Vec::new());
        }
        let mut records = Vec::new();
        for segment in &state.segments {
            if !segment.has_records()
                || segment.start_sequence < state.retained_floor
                || segment.end_sequence <= after_sequence
            {
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
            let bytes = match &segment.active_file_guard {
                Some(file_guard) => read_existing_lines_from_matching(
                    &segment.path,
                    file_guard,
                    segment.bytes,
                    start_offset,
                    remaining,
                ),
                None => read_existing_lines_from(&segment.path, start_offset, remaining),
            }
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
    use super::super::{CheckpointStore, CheckpointedReducer, EventReducer, FileCheckpointStore};
    use super::*;
    use std::sync::Barrier;

    fn shared<E>(journal: &SegmentedFileEventJournal<E>) -> &Arc<SharedSegmentedJournalState> {
        journal
            .shared
            .as_ref()
            .expect("live segmented journal handle")
    }

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

    #[derive(Default, Debug, Serialize, Deserialize)]
    struct RetainedReducer {
        events: Vec<String>,
    }

    impl EventReducer for RetainedReducer {
        type Event = String;

        fn apply(&mut self, event: &String) {
            self.events.push(event.clone());
        }
    }

    fn write_checkpoint(path: &Path, sequence: u64, events: &[&str]) {
        let state = RetainedReducer {
            events: events.iter().map(|event| (*event).to_string()).collect(),
        };
        FileCheckpointStore::open(path)
            .save(&state, sequence)
            .expect("write checkpoint fixture");
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
    fn integrity_digest_is_fixed_width_lowercase_hex() {
        let digest = record_digest(42, &"digest-event").expect("compute digest");
        assert_eq!(
            digest,
            "210912c51b7695a5b542fd2a506eaf15b0cbc34627d40bb0349f40a26fa578b6"
        );
        assert_eq!(digest.len(), 64);
        assert!(
            digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
    }

    #[test]
    fn fresh_nested_directory_uses_durable_creation() {
        let root = temp_root("fresh-parent-sync");
        let directory = root.join("missing-a").join("missing-b").join("journal");
        let journal = open_strings(&directory, 4096, FileDurability::SyncData);
        assert!(directory.is_dir());
        journal
            .append("durable nested".to_string())
            .expect("append nested journal");
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
        echo_core::utils::fs::append_existing(
            &active.path,
            b"{\"sequence\":3",
            FileDurability::Flush,
        )
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
    fn one_handle_alternates_event_durability_across_rollovers() {
        let root = temp_root("mixed-durability");
        let journal = open_strings(&root, 1, FileDurability::Flush);
        let first = journal
            .append_with_durability("delta-1".to_string(), FileDurability::Flush)
            .expect("append flush delta");
        let second = journal
            .append_with_durability("safe-point".to_string(), FileDurability::SyncData)
            .expect("append sync safe point");
        let third = journal
            .append("delta-2".to_string())
            .expect("append default flush");
        assert_eq!(first.record.sequence, 1);
        assert_eq!(second.record.sequence, 2);
        assert_eq!(third.record.sequence, 3);
        assert_eq!(journal.segments().len(), 3);
        assert_eq!(
            *journal
                .append_durabilities
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
            vec![
                FileDurability::Flush,
                FileDurability::SyncData,
                FileDurability::Flush,
            ]
        );
        drop(journal);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn per_event_full_write_degraded_receipt_owns_sequence_without_retry() {
        let root = temp_root("mixed-full-write");
        let journal = open_strings(&root, 4096, FileDurability::Flush);
        *journal
            .append_fault
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(AppendFault::FullWrite);
        let committed = journal
            .append_with_durability("terminal".to_string(), FileDurability::SyncData)
            .expect("full write returns committed receipt");
        assert_eq!(committed.record.sequence, 1);
        assert!(matches!(
            committed.durability,
            JournalDurabilityStatus::Degraded { .. }
        ));
        let next = journal
            .append_with_durability("next-delta".to_string(), FileDurability::Flush)
            .expect("next event does not retry terminal");
        assert_eq!(next.record.sequence, 2);
        assert_eq!(
            journal.replay_after(0, usize::MAX).expect("replay").len(),
            2
        );
        drop(journal);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn per_event_partial_write_repairs_with_requested_durability() {
        let root = temp_root("mixed-partial-write");
        let journal = open_strings(&root, 4096, FileDurability::Flush);
        *journal
            .append_fault
            .lock()
            .unwrap_or_else(|error| error.into_inner()) =
            Some(AppendFault::PartialWrite { bytes: 13 });
        assert!(
            journal
                .append_with_durability("safe-point".to_string(), FileDurability::SyncData)
                .is_err()
        );
        assert_eq!(journal.next_sequence(), 1);
        assert_eq!(
            journal
                .append_with_durability("retry".to_string(), FileDurability::Flush)
                .expect("sequence can be reused after durable repair")
                .record
                .sequence,
            1
        );
        assert_eq!(
            *journal
                .append_durabilities
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
            vec![FileDurability::SyncData, FileDurability::Flush]
        );
        drop(journal);
        std::fs::remove_dir_all(root).ok();
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
        assert!(Arc::ptr_eq(shared(&journal), shared(&reopened)));
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
        let receipt = journal
            .prune_closed_segments_before(3)
            .expect("prune closed segments");
        assert_eq!(receipt.retained_floor, 3);
        assert_eq!(receipt.commit, JournalPruneCommitStatus::Confirmed);
        assert_eq!(receipt.logically_pruned.len(), 2);
        assert_eq!(receipt.physically_removed.len(), 2);
        assert!(
            receipt
                .physically_removed
                .iter()
                .all(|segment| segment.end_sequence.is_some())
        );
        assert_eq!(receipt.cleanup, JournalPhysicalCleanupStatus::Confirmed);
        assert!(active_path.exists());
        let remaining = journal.segments();
        assert_eq!(remaining.len(), 1);
        assert!(remaining.first().is_some_and(|segment| segment.active));
        drop(journal);
        let reopened = open_strings(&root, 1, FileDurability::Flush);
        assert_eq!(reopened.last_sequence(), 3);
        assert_eq!(
            reopened.retention_metadata(),
            JournalRetentionMetadata {
                retained_floor: 3,
                cleanup_pending: false,
            }
        );
        let too_old = reopened
            .replay_after(0, usize::MAX)
            .expect_err("replay below retained floor must fail");
        assert!(too_old.to_string().contains("below retained floor"));
        assert_eq!(
            reopened
                .replay_after(2, usize::MAX)
                .expect("replay retained")
                .len(),
            1
        );
        drop(reopened);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn zero_limit_still_rejects_a_cursor_below_retained_floor() {
        let root = temp_root("zero-limit-floor");
        let journal = open_strings(&root, 1, FileDurability::Flush);
        for value in ["one", "two", "three"] {
            journal.append(value.to_string()).expect("append");
        }
        journal
            .prune_closed_segments_before(3)
            .expect("prune prefix");
        let error = journal
            .replay_after(0, 0)
            .expect_err("zero limit must still validate cursor");
        assert!(error.to_string().contains("below retained floor"));
        assert!(journal.replay_after(2, 0).expect("floor cursor").is_empty());
        drop(journal);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn missing_prefix_without_marker_fails_open() {
        let root = temp_root("missing-prefix-marker");
        let journal = open_strings(&root, 1, FileDurability::Flush);
        journal.append("one".to_string()).expect("append one");
        journal.append("two".to_string()).expect("append two");
        let first = journal
            .segments()
            .into_iter()
            .find(|segment| segment.start_sequence == 1)
            .expect("first segment");
        drop(journal);
        std::fs::remove_file(first.path).expect("remove prefix without marker");
        let error = SegmentedFileEventJournal::<String>::open(&root, 1, FileDurability::Flush)
            .expect_err("missing unmarked prefix must fail");
        assert!(error.to_string().contains("without an authorized"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn corrupt_retained_floor_marker_fails_open() {
        let root = temp_root("corrupt-floor-marker");
        let journal = open_strings(&root, 1, FileDurability::Flush);
        for value in ["one", "two", "three"] {
            journal.append(value.to_string()).expect("append");
        }
        journal
            .prune_closed_segments_before(3)
            .expect("prune prefix");
        drop(journal);
        let marker = root.join(RETENTION_MARKER);
        mutate_json_line(&marker, |record| {
            record.insert(
                "digest".to_string(),
                serde_json::Value::String("0".repeat(64)),
            );
        });
        let error = SegmentedFileEventJournal::<String>::open(&root, 1, FileDurability::Flush)
            .expect_err("corrupt marker must fail");
        assert!(error.to_string().contains("marker digest mismatch"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn initial_marker_write_failure_does_not_commit_logical_prune() {
        let root = temp_root("marker-first-fault");
        let journal = open_strings(&root, 1, FileDurability::Flush);
        for value in ["one", "two", "three"] {
            journal.append(value.to_string()).expect("append");
        }
        *journal
            .marker_write_fault_after
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(0);
        let error = journal
            .prune_closed_segments_before(3)
            .expect_err("marker failure precedes logical commit");
        assert!(error.to_string().contains("marker write failure"));
        assert_eq!(journal.retention_metadata().retained_floor, 1);
        assert_eq!(journal.segments().len(), 3);
        assert_eq!(
            journal.replay_after(0, usize::MAX).expect("replay").len(),
            3
        );
        drop(journal);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn partial_delete_is_degraded_synced_and_retryable_after_reopen() {
        use std::sync::atomic::Ordering;

        let root = temp_root("partial-prune-delete");
        let journal = open_strings(&root, 1, FileDurability::Flush);
        for value in ["one", "two", "three", "four"] {
            journal.append(value.to_string()).expect("append");
        }
        *journal
            .delete_fault_after
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(1);
        let before = journal.directory_sync_attempts.load(Ordering::SeqCst);
        let receipt = journal
            .prune_closed_segments_before(4)
            .expect("logical prune commits before delete failure");
        assert_eq!(receipt.retained_floor, 4);
        assert_eq!(receipt.logically_pruned.len(), 3);
        assert_eq!(receipt.physically_removed.len(), 1);
        assert!(matches!(
            receipt.cleanup,
            JournalPhysicalCleanupStatus::Degraded { .. }
        ));
        assert!(journal.directory_sync_attempts.load(Ordering::SeqCst) > before);
        assert!(journal.retention_metadata().cleanup_pending);
        assert_eq!(
            journal
                .segments()
                .first()
                .map(|segment| segment.start_sequence),
            Some(4)
        );
        drop(journal);

        let reopened = open_strings(&root, 1, FileDurability::Flush);
        assert_eq!(
            reopened.retention_metadata(),
            JournalRetentionMetadata {
                retained_floor: 4,
                cleanup_pending: true,
            }
        );
        assert_eq!(
            reopened
                .segments()
                .first()
                .map(|segment| segment.start_sequence),
            Some(4)
        );
        assert_eq!(
            reopened
                .replay_after(3, usize::MAX)
                .expect("floor replay")
                .len(),
            1
        );
        let retry = reopened
            .prune_closed_segments_before(4)
            .expect("retry pending cleanup");
        assert_eq!(retry.cleanup, JournalPhysicalCleanupStatus::Confirmed);
        assert_eq!(retry.physically_removed.len(), 2);
        assert!(!reopened.retention_metadata().cleanup_pending);
        drop(reopened);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn directory_sync_failure_stays_pending_until_explicit_barrier() {
        let root = temp_root("prune-dir-sync");
        let journal = open_strings(&root, 1, FileDurability::Flush);
        for value in ["one", "two", "three"] {
            journal.append(value.to_string()).expect("append");
        }
        *journal
            .directory_sync_fault
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = true;
        let receipt = journal
            .prune_closed_segments_before(3)
            .expect("logical prune commits before sync failure");
        assert!(matches!(
            receipt.cleanup,
            JournalPhysicalCleanupStatus::Degraded { .. }
        ));
        assert!(journal.retention_metadata().cleanup_pending);
        drop(journal);

        let reopened = open_strings(&root, 1, FileDurability::Flush);
        assert!(reopened.retention_metadata().cleanup_pending);
        reopened
            .sync_data()
            .expect("retry cleanup and directory barrier");
        assert!(!reopened.retention_metadata().cleanup_pending);
        drop(reopened);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn final_marker_failure_returns_degraded_receipt_and_survives_reopen() {
        let root = temp_root("marker-final-fault");
        let journal = open_strings(&root, 1, FileDurability::Flush);
        for value in ["one", "two", "three"] {
            journal.append(value.to_string()).expect("append");
        }
        *journal
            .marker_write_fault_after
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(1);
        let receipt = journal
            .prune_closed_segments_before(3)
            .expect("logical prune remains committed");
        assert!(matches!(
            receipt.cleanup,
            JournalPhysicalCleanupStatus::Degraded { .. }
        ));
        assert_eq!(receipt.physically_removed.len(), 2);
        assert!(journal.retention_metadata().cleanup_pending);
        drop(journal);

        let reopened = open_strings(&root, 1, FileDurability::Flush);
        assert!(reopened.retention_metadata().cleanup_pending);
        reopened.sync_data().expect("finish marker transition");
        assert!(!reopened.retention_metadata().cleanup_pending);
        drop(reopened);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn visible_marker_barrier_failure_is_typed_and_deletes_nothing() {
        let root = temp_root("visible-marker-barrier");
        let journal = open_strings(&root, 1, FileDurability::Flush);
        for value in ["one", "two", "three"] {
            journal.append(value.to_string()).expect("append");
        }
        *journal
            .marker_full_write_fault
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = true;
        let receipt = journal
            .prune_closed_segments_before(3)
            .expect("visible marker failure returns receipt");
        assert!(matches!(
            receipt.commit,
            JournalPruneCommitStatus::Degraded { .. }
        ));
        assert!(receipt.physically_removed.is_empty());
        assert_eq!(
            list_segment_paths(&root, "test list")
                .expect("physical segments")
                .len(),
            3
        );
        assert!(journal.retention_metadata().cleanup_pending);

        let reopened = open_strings(&root, 1, FileDurability::Flush);
        assert!(Arc::ptr_eq(shared(&journal), shared(&reopened)));
        assert_eq!(
            list_segment_paths(&root, "test reopen list")
                .expect("physical segments after reopen")
                .len(),
            3
        );
        let retry = reopened
            .prune_closed_segments_before(3)
            .expect("retry marker barrier and cleanup");
        assert_eq!(retry.commit, JournalPruneCommitStatus::Confirmed);
        assert_eq!(retry.physically_removed.len(), 2);
        assert_eq!(retry.cleanup, JournalPhysicalCleanupStatus::Confirmed);
        assert!(!reopened.retention_metadata().cleanup_pending);
        drop(journal);
        drop(reopened);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn pending_obsolete_prefix_is_opaque_across_crash_recovery() {
        let root = temp_root("opaque-obsolete-prefix");
        let journal = open_strings(&root, 1, FileDurability::Flush);
        for value in ["one", "two", "three", "four"] {
            journal.append(value.to_string()).expect("append");
        }
        *journal
            .marker_full_write_fault
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = true;
        let receipt = journal
            .prune_closed_segments_before(4)
            .expect("logical marker visible");
        assert!(matches!(
            receipt.commit,
            JournalPruneCommitStatus::Degraded { .. }
        ));
        drop(journal);

        std::fs::remove_file(segment_path(&root, 2)).expect("obsolete segment 2 missing");
        std::fs::write(segment_path(&root, 3), b"corrupt obsolete bytes")
            .expect("obsolete segment 3 corrupt");
        let reopened = open_strings(&root, 1, FileDurability::Flush);
        assert_eq!(
            reopened
                .segments()
                .iter()
                .map(|segment| segment.start_sequence)
                .collect::<Vec<_>>(),
            vec![4]
        );
        assert_eq!(
            reopened
                .replay_after(3, usize::MAX)
                .expect("floor replay")
                .len(),
            1
        );
        let cleanup = reopened
            .prune_closed_segments_before(4)
            .expect("remove opaque leftovers");
        assert_eq!(cleanup.physically_removed.len(), 2);
        assert!(
            cleanup
                .physically_removed
                .iter()
                .all(|segment| segment.end_sequence.is_none())
        );
        assert_eq!(cleanup.cleanup, JournalPhysicalCleanupStatus::Confirmed);
        assert!(!segment_path(&root, 1).exists());
        assert!(!segment_path(&root, 3).exists());
        drop(reopened);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn confirmed_cleanup_rejects_reappearing_obsolete_prefix() {
        let root = temp_root("confirmed-obsolete-prefix");
        let journal = open_strings(&root, 1, FileDurability::Flush);
        for value in ["one", "two", "three"] {
            journal.append(value.to_string()).expect("append");
        }
        journal
            .prune_closed_segments_before(3)
            .expect("confirmed prune");
        drop(journal);
        std::fs::write(segment_path(&root, 1), b"reappeared obsolete prefix")
            .expect("restore obsolete prefix");
        let error = SegmentedFileEventJournal::<String>::open(&root, 1, FileDurability::Flush)
            .expect_err("confirmed marker rejects obsolete prefix");
        assert!(error.to_string().contains("cleanup is confirmed"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn pruned_recovery_rejects_missing_corrupt_behind_and_ahead_checkpoints() {
        let root = temp_root("pruned-checkpoints");
        let journal = Arc::new(open_strings(&root, 1, FileDurability::Flush));
        for value in ["one", "two", "three"] {
            journal.append(value.to_string()).expect("append");
        }
        journal
            .prune_closed_segments_before(3)
            .expect("prune prefix");
        let checkpoint_path = root.join("checkpoint.json");

        for (label, expected_error) in [
            ("corrupt", "checkpoint load failed"),
            ("behind", "behind retained journal floor"),
            ("ahead", "ahead of journal sequence"),
        ] {
            match label {
                "corrupt" => {
                    std::fs::write(&checkpoint_path, b"{partial")
                        .expect("write corrupt checkpoint");
                }
                "behind" => write_checkpoint(&checkpoint_path, 1, &["one"]),
                "ahead" => write_checkpoint(&checkpoint_path, 99, &[]),
                _ => {}
            }
            let reducer = CheckpointedReducer::<_, RetainedReducer>::new(
                Arc::clone(&journal),
                Arc::new(FileCheckpointStore::open(&checkpoint_path)),
                0,
            );
            let error = reducer.recover().expect_err(label);
            assert!(
                error.to_string().contains(expected_error),
                "{label} checkpoint produced unexpected error: {error}"
            );
        }
        std::fs::remove_file(&checkpoint_path).expect("remove checkpoint");
        let missing = CheckpointedReducer::<_, RetainedReducer>::new(
            Arc::clone(&journal),
            Arc::new(FileCheckpointStore::open(&checkpoint_path)),
            0,
        );
        assert!(missing.recover().is_err());

        write_checkpoint(&checkpoint_path, 2, &["one", "two"]);
        let valid = CheckpointedReducer::<_, RetainedReducer>::new(
            Arc::clone(&journal),
            Arc::new(FileCheckpointStore::open(&checkpoint_path)),
            0,
        );
        assert_eq!(
            valid
                .recover()
                .expect("valid floor checkpoint")
                .last_applied_sequence,
            3
        );
        valid.with_state(|state| assert_eq!(state.events, vec!["one", "two", "three"]));
        drop(valid);
        drop(journal);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn duplicate_open_shares_authority_and_mismatch_rejects() {
        let root = temp_root("duplicate");
        let first = open_strings(&root, 4096, FileDurability::Flush);
        let alias = root.join(".");
        let second = open_strings(&alias, 4096, FileDurability::Flush);
        assert!(Arc::ptr_eq(shared(&first), shared(&second)));
        let type_error = SegmentedFileEventJournal::<u64>::open(&root, 4096, FileDurability::Flush)
            .expect_err("mismatched event type must reject");
        assert!(type_error.to_string().contains("different event type"));
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

    #[test]
    fn live_directory_missing_and_replacement_fail_without_reset() {
        let parent = temp_root("directory-replacement-parent");
        let root = parent.join("journal");
        let displaced = parent.join("displaced");
        let journal = open_strings(&root, 4096, FileDurability::Flush);
        journal.append("one".to_string()).expect("append original");
        let active = journal
            .segments()
            .into_iter()
            .find(|segment| segment.active)
            .expect("active segment");
        std::fs::rename(&root, &displaced).expect("displace live directory");

        let missing = SegmentedFileEventJournal::<String>::open(&root, 4096, FileDurability::Flush)
            .expect_err("live missing directory must not be recreated");
        assert!(missing.to_string().contains("segmented journal open"));
        assert!(!root.exists());
        assert_eq!(journal.next_sequence(), 2);

        std::fs::create_dir(&root).expect("create replacement directory");
        let segment_name = active.path.file_name().expect("active segment name");
        std::fs::copy(displaced.join(segment_name), root.join(segment_name))
            .expect("copy valid replacement segment");
        let replay_error = journal
            .replay_after(0, usize::MAX)
            .expect_err("replacement directory must reject replay");
        assert!(
            replay_error
                .to_string()
                .contains("directory identity changed")
        );
        let append_error = journal
            .append("blocked".to_string())
            .expect_err("replacement directory must reject append");
        assert!(
            append_error
                .to_string()
                .contains("directory identity changed")
        );
        let sync_error = journal
            .sync_data()
            .expect_err("replacement directory must reject sync");
        assert!(
            sync_error
                .to_string()
                .contains("directory identity changed")
        );
        let second_open =
            SegmentedFileEventJournal::<String>::open(&root, 4096, FileDurability::Flush)
                .expect_err("live replacement directory must reject second open");
        assert!(
            second_open
                .to_string()
                .contains("directory identity changed")
        );
        assert_eq!(journal.next_sequence(), 2);

        drop(journal);
        let reopened = open_strings(&root, 4096, FileDurability::Flush);
        assert_eq!(reopened.last_sequence(), 1);
        assert_eq!(
            reopened
                .replay_after(0, 1)
                .expect("replay verified replacement")
                .first()
                .map(|record| record.event.as_str()),
            Some("one")
        );
        drop(reopened);
        std::fs::remove_dir_all(parent).ok();
    }

    #[test]
    fn active_segment_same_length_replacement_fails_all_live_io() {
        let root = temp_root("active-replacement");
        let journal = open_strings(&root, 4096, FileDurability::Flush);
        journal.append("one".to_string()).expect("append original");
        let active = journal
            .segments()
            .into_iter()
            .find(|segment| segment.active)
            .expect("active segment");
        let bytes = std::fs::read(&active.path).expect("read active segment");
        assert_eq!(u64::try_from(bytes.len()).unwrap_or(u64::MAX), active.bytes);
        std::fs::remove_file(&active.path).expect("remove active segment");
        std::fs::write(&active.path, &bytes).expect("write same-length replacement");

        assert!(journal.replay_after(0, usize::MAX).is_err());
        assert!(journal.append("blocked".to_string()).is_err());
        assert!(journal.sync_data().is_err());
        assert!(
            SegmentedFileEventJournal::<String>::open(&root, 4096, FileDurability::Flush,).is_err()
        );
        assert_eq!(journal.next_sequence(), 2);

        drop(journal);
        let reopened = open_strings(&root, 4096, FileDurability::Flush);
        assert_eq!(reopened.last_sequence(), 1);
        drop(reopened);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn closed_segment_same_length_replacement_still_verifies_digest() {
        let root = temp_root("closed-replacement");
        let journal = open_strings(&root, 1, FileDurability::Flush);
        journal.append("one".to_string()).expect("append one");
        journal.append("two".to_string()).expect("append two");
        let closed = journal
            .segments()
            .into_iter()
            .find(|segment| !segment.active)
            .expect("closed segment");
        let bytes = std::fs::read(&closed.path).expect("read closed segment");
        let line = bytes
            .split(|byte| *byte == b'\n')
            .find(|line| !line.is_empty())
            .expect("closed record");
        let mut value: serde_json::Value =
            serde_json::from_slice(line).expect("decode closed record");
        value.as_object_mut().expect("closed record object").insert(
            "digest".to_string(),
            serde_json::Value::String("0".repeat(64)),
        );
        let mut replacement = serde_json::to_vec(&value).expect("encode replacement");
        replacement.push(b'\n');
        assert_eq!(replacement.len(), bytes.len());
        std::fs::remove_file(&closed.path).expect("remove closed segment");
        std::fs::write(&closed.path, replacement).expect("write closed replacement");

        let error = journal
            .replay_after(0, usize::MAX)
            .expect_err("closed replacement must fail digest validation");
        assert!(error.to_string().contains("digest mismatch"));
        drop(journal);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn registry_mass_live_drop_is_exact_across_multiple_waves() {
        let root = temp_root("weak-registry");
        let canonical_root = std::fs::canonicalize(&root).expect("canonical test root");
        let live_directory = canonical_root.join("live");
        let live = open_strings(&live_directory, 4096, FileDurability::Flush);
        let live_directory = live.directory().to_path_buf();
        let per_wave = super::super::WEAK_REGISTRY_HARD_LIMIT.saturating_add(8);
        for wave in 0..2 {
            let mut journals = Vec::with_capacity(per_wave);
            for index in 0..per_wave {
                let directory = canonical_root.join(format!("wave-{wave}-{index}"));
                journals.push(open_strings(&directory, 4096, FileDurability::Flush));
            }
            let registry = segmented_registry()
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            assert_eq!(registry.paths_beneath(&canonical_root), per_wave + 1);
            let retained = registry
                .upgrade(&live_directory)
                .expect("live registry entry");
            assert!(Arc::ptr_eq(&retained, shared(&live)));
            drop(retained);
            drop(registry);
            drop(journals);
            assert_eq!(
                segmented_registry()
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .paths_beneath(&canonical_root),
                1
            );
        }

        let alias = open_strings(&live_directory, 4096, FileDurability::Flush);
        assert!(Arc::ptr_eq(shared(&live), shared(&alias)));
        drop(live);
        drop(alias);
        assert_eq!(
            segmented_registry()
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .paths_beneath(&canonical_root),
            0
        );

        let reopened = open_strings(&live_directory, 4096, FileDurability::Flush);
        assert_eq!(
            reopened
                .append("after-reopen".to_string())
                .expect("append through reacquired authority")
                .record
                .sequence,
            1
        );
        drop(reopened);
        assert_eq!(
            segmented_registry()
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .paths_beneath(&canonical_root),
            0
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn concurrent_two_alias_drop_and_immediate_reopen_remains_available() {
        let root = temp_root("concurrent-close-reopen");
        let canonical_root = std::fs::canonicalize(&root).expect("canonical test root");
        for _ in 0..64 {
            let first = open_strings(&canonical_root, 4096, FileDurability::Flush);
            let second = open_strings(&canonical_root, 4096, FileDurability::Flush);
            let barrier = Arc::new(Barrier::new(3));
            let first_barrier = Arc::clone(&barrier);
            let first_drop = std::thread::spawn(move || {
                first_barrier.wait();
                drop(first);
            });
            let second_barrier = Arc::clone(&barrier);
            let second_drop = std::thread::spawn(move || {
                second_barrier.wait();
                drop(second);
            });
            barrier.wait();
            let reopened = open_strings(&canonical_root, 4096, FileDurability::Flush);
            first_drop.join().expect("join first dropping handle");
            second_drop.join().expect("join second dropping handle");
            assert_eq!(
                segmented_registry()
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .paths_beneath(&canonical_root),
                1
            );
            drop(reopened);
            assert_eq!(
                segmented_registry()
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .paths_beneath(&canonical_root),
                0
            );
        }
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
