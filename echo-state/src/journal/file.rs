//! File-backed journal and checkpoint implementations.
//!
//! [`FileEventJournal`] persists one JSON line per atomic batch frame. On open
//! it scans the file, validates 1-based contiguous sequences, tolerates one
//! torn trailing frame by truncating the entire batch, and rejects gaps or
//! mid-file corruption loudly. [`FileCheckpointStore`] writes one atomic
//! snapshot file that pairs the reducer state with its applied sequence.

use super::{
    BatchIdentity, CheckpointFrame, CheckpointStore, EventJournal, JournalBatchAppendError,
    JournalBatchAppendReceipt, JournalBatchAppendResult, JournalBatchCommitStatus,
    JournalBatchLookup, JournalDurabilityStatus, JournalEvent, JournalRecord, PreparedJournalBatch,
    WeakRegistry, decode_journal_batch, prepare_journal_frame, verify_journal_batch_sequence,
};
use echo_core::error::{ReactError, Result};
use echo_core::utils::canonical_json::canonical_json_bytes;
use echo_core::utils::fs::{
    ExclusiveFileLease, ExistingRegularFileGuard, FileDurability, append_existing,
    append_existing_matching, atomic_write, create_dir_all_durable, matching_existing_regular_len,
    open_existing_regular_guard, read_existing, read_existing_from_matching,
    read_existing_lines_from_matching, read_existing_matching, truncate_existing_matching,
    try_exclusive_file_lease,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::any::TypeId;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

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
    /// Byte length of the valid prefix (batch frames plus their newlines).
    valid_len: u64,
    /// Next sequence a fresh append should assign.
    next_sequence: u64,
    /// Byte offset of every record's batch frame, indexed by `sequence - 1`.
    record_offsets: Vec<u64>,
    batches: HashMap<String, FileBatchIndex>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileBatchIndex {
    identity: BatchIdentity,
    frame_offset: u64,
}

/// Parse the journal batch frames and validate sequencing.
///
/// Returns the valid prefix length, the next sequence, and whether the file
/// ended with a torn frame that should be truncated. A missing trailing newline
/// invalidates the complete final batch; corruption in a complete frame fails
/// closed.
fn scan_journal<E: JournalEvent>(context: &str, bytes: &[u8]) -> Result<ScannedJournal> {
    let mut valid_len: u64 = 0;
    let mut next_sequence: u64 = 1;
    let mut offset: usize = 0;
    let mut record_offsets = Vec::new();
    let mut batches = HashMap::new();
    while offset < bytes.len() {
        let suffix = bytes
            .get(offset..)
            .ok_or_else(|| ReactError::Other(format!("{context}: invalid journal byte offset")))?;
        let Some(newline) = suffix.iter().position(|byte| *byte == b'\n') else {
            // A partial physical frame makes none of its records visible.
            break;
        };
        let line_end = offset
            .checked_add(newline)
            .ok_or_else(|| ReactError::Other(format!("{context}: journal byte offset overflow")))?;
        let line = bytes.get(offset..line_end).ok_or_else(|| {
            ReactError::Other(format!("{context}: invalid journal batch byte range"))
        })?;
        let frame = decode_journal_batch::<E>(context, line)?;
        if batches.contains_key(frame.batch_id()) {
            return Err(ReactError::Other(format!(
                "{context}: duplicate physical batch identity {}",
                frame.batch_id()
            )));
        }
        verify_journal_batch_sequence(context, next_sequence, &frame)?;
        let frame_offset = u64::try_from(offset)
            .map_err(|_| ReactError::Other(format!("{context}: journal exceeds supported size")))?;
        record_offsets.extend(std::iter::repeat_n(frame_offset, frame.records().len()));
        let record_count = u64::try_from(frame.records().len()).map_err(|_| {
            ReactError::Other(format!(
                "{context}: journal batch count exceeds supported range"
            ))
        })?;
        batches.insert(
            frame.batch_id().to_string(),
            FileBatchIndex {
                identity: frame.identity()?,
                frame_offset,
            },
        );
        next_sequence = next_sequence
            .checked_add(record_count)
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
        batches,
    })
}

#[derive(Debug)]
struct FileJournalState {
    next_sequence: u64,
    valid_len: u64,
    record_offsets: Vec<u64>,
    batches: HashMap<String, FileBatchIndex>,
    poison: Option<String>,
}

#[derive(Debug)]
struct SharedFileJournalState {
    event_type: TypeId,
    durability: FileDurability,
    state: Mutex<FileJournalState>,
    file_guard: ExistingRegularFileGuard,
    _lease: ExclusiveFileLease,
}

fn file_journal_registry() -> &'static Mutex<WeakRegistry<SharedFileJournalState>> {
    static REGISTRY: OnceLock<Mutex<WeakRegistry<SharedFileJournalState>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(WeakRegistry::new()))
}

fn scan_and_repair_journal<E: JournalEvent>(
    path: &Path,
    file_guard: &ExistingRegularFileGuard,
    context: &str,
    durability: FileDurability,
) -> Result<ScannedJournal> {
    let bytes =
        read_existing_matching(path, file_guard).map_err(|error| io_error(context, error))?;
    let scanned = scan_journal::<E>(context, &bytes)?;
    let file_len = u64::try_from(bytes.len())
        .map_err(|_| ReactError::Other(format!("{context}: journal exceeds supported size")))?;
    if scanned.valid_len < file_len {
        truncate_existing_matching(path, file_guard, file_len, scanned.valid_len, durability)
            .map_err(|error| io_error(context, error))?;
    }
    let final_len = matching_existing_regular_len(path, file_guard)
        .map_err(|error| io_error(context, error))?;
    if final_len != scanned.valid_len {
        return Err(ReactError::Other(format!(
            "{context}: journal length changed during verified scan; reopen required"
        )));
    }
    Ok(scanned)
}

fn ensure_journal_file(path: &Path, parent: &Path, context: &str) -> Result<()> {
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(file) => finish_new_journal_creation(&file, parent, sync_directory)
            .map_err(|error| io_error(context, error)),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let metadata = std::fs::symlink_metadata(path)
                .map_err(|metadata_error| io_error(context, metadata_error))?;
            if metadata.file_type().is_file() && metadata.len() == 0 {
                append_existing(path, b"", FileDurability::SyncData)
                    .map_err(|barrier_error| io_error(context, barrier_error))?;
                sync_directory(parent).map_err(|barrier_error| io_error(context, barrier_error))?;
            }
            Ok(())
        }
        Err(error) => Err(io_error(context, error)),
    }
}

/// JSONL-backed atomic-batch [`EventJournal`].
///
/// Appends serialize one digest-protected batch frame per line with the
/// caller-selected durability policy; `Flush` survives process crashes, `SyncData` additionally
/// survives power loss. Open validates the complete file once and records byte
/// offsets; replay then seeks directly to the requested sequence suffix. A
/// canonical in-process authority shares sequencing across handles and holds a
/// process-lifetime exclusive lease against competing writers.
#[derive(Debug)]
pub struct FileEventJournal<E> {
    path: PathBuf,
    durability: FileDurability,
    shared: Option<Arc<SharedFileJournalState>>,
    #[cfg(test)]
    append_fault: Mutex<Option<AppendFault>>,
    #[cfg(test)]
    sync_fault: Mutex<bool>,
    #[cfg(test)]
    truncate_barrier_fault: Mutex<bool>,
    #[cfg(test)]
    reconcile_read_fault: Mutex<bool>,
    _event: PhantomData<fn() -> E>,
}

impl<E> Drop for FileEventJournal<E> {
    fn drop(&mut self) {
        let Some(shared) = self.shared.take() else {
            return;
        };
        let mut registry = file_journal_registry()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        registry.release_handle(&self.path, &shared);
        // Drop every handle Arc, including the last lease owner, while opens
        // are excluded by the registry lock.
        drop(shared);
        drop(registry);
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Copy)]
enum AppendFault {
    ZeroWriteInvalidData,
    FullWrite,
    FullWriteInvalidData,
    PartialWrite { bytes: usize },
    PartialWriteInvalidData { bytes: usize },
    MissingLastByteInvalidData,
    UnrecognizedSuffix,
    UnrecognizedSuffixInvalidData,
}

impl<E: JournalEvent> FileEventJournal<E> {
    fn shared(&self) -> Result<&Arc<SharedFileJournalState>> {
        self.shared
            .as_ref()
            .ok_or_else(|| ReactError::Other("file journal handle is already closing".to_string()))
    }

    fn shared_io(&self) -> std::io::Result<&Arc<SharedFileJournalState>> {
        self.shared
            .as_ref()
            .ok_or_else(|| std::io::Error::other("file journal handle is already closing"))
    }

    /// Open (or create) the journal at `path`.
    ///
    /// A torn trailing batch frame from an interrupted append is truncated as
    /// one unit; gaps and complete-frame corruption are errors.
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
        let mut registry = file_journal_registry().lock().map_err(|error| {
            ReactError::Other(format!("journal registry lock poisoned: {error}"))
        })?;
        registry.prune_dead_if_due();
        if let Some(shared) = registry.upgrade(&path) {
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
            let observed_len = matching_existing_regular_len(&path, &shared.file_guard)
                .map_err(|error| io_error(&context, error))?;
            if observed_len < state.valid_len {
                return Err(ReactError::Other(format!(
                    "{context}: live journal shrank from {} to {observed_len}; close the existing authority before verified repair",
                    state.valid_len
                )));
            }
            let scanned =
                scan_and_repair_journal::<E>(&path, &shared.file_guard, &context, durability)?;
            if scanned.next_sequence != state.next_sequence
                || scanned.valid_len != state.valid_len
                || scanned.record_offsets != state.record_offsets
                || scanned.batches != state.batches
            {
                return Err(ReactError::Other(format!(
                    "{context}: disk prefix diverged from the live authority; close it before verified reopen"
                )));
            }
            if !registry.add_handle(&path, &shared) {
                return Err(ReactError::Other(format!(
                    "{context}: failed to register shared journal handle"
                )));
            }
            state.poison = None;
            drop(state);
            return Ok(Self {
                path,
                durability,
                shared: Some(shared),
                #[cfg(test)]
                append_fault: Mutex::new(None),
                #[cfg(test)]
                sync_fault: Mutex::new(false),
                #[cfg(test)]
                truncate_barrier_fault: Mutex::new(false),
                #[cfg(test)]
                reconcile_read_fault: Mutex::new(false),
                _event: PhantomData,
            });
        }
        let lease = try_exclusive_file_lease(&path).map_err(|error| io_error(&context, error))?;
        // Only a lease-owning new authority may create the data file. A live
        // in-process authority was resolved above without touching the path.
        ensure_journal_file(&path, &canonical_parent, &context)?;
        let file_guard =
            open_existing_regular_guard(&path).map_err(|error| io_error(&context, error))?;
        let scanned = scan_and_repair_journal::<E>(&path, &file_guard, &context, durability)?;
        let shared = Arc::new(SharedFileJournalState {
            event_type: TypeId::of::<E>(),
            durability,
            state: Mutex::new(FileJournalState {
                next_sequence: scanned.next_sequence,
                valid_len: scanned.valid_len,
                record_offsets: scanned.record_offsets,
                batches: scanned.batches,
                poison: None,
            }),
            file_guard,
            _lease: lease,
        });
        registry.insert(path.clone(), &shared);
        Ok(Self {
            path,
            durability,
            shared: Some(shared),
            #[cfg(test)]
            append_fault: Mutex::new(None),
            #[cfg(test)]
            sync_fault: Mutex::new(false),
            #[cfg(test)]
            truncate_barrier_fault: Mutex::new(false),
            #[cfg(test)]
            reconcile_read_fault: Mutex::new(false),
            _event: PhantomData,
        })
    }

    /// Journal file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Force an idempotent data durability barrier on the current journal.
    ///
    /// Use this after an append returns
    /// [`JournalDurabilityStatus::Degraded`]. The record in that receipt
    /// already owns all of its sequences and must not be appended again; retry this
    /// barrier instead. The operation is serialized with append and replay,
    /// does not write an event or advance the sequence, and refuses a poisoned
    /// authority until it is reopened and repaired.
    pub fn sync_data(&self) -> Result<()> {
        let shared = self.shared()?;
        let state = shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(reason) = &state.poison {
            return Err(ReactError::Other(format!(
                "journal sync_data {} refused because the handle is poisoned: {reason}; reopen the journal to recover",
                self.path.display()
            )));
        }
        let context = format!("journal sync_data {}", self.path.display());
        self.verify_current_file(&state, &context)?;
        self.sync_journal_data(state.valid_len)
            .map_err(|error| io_error(&context, error))
    }

    fn verify_current_file(&self, state: &FileJournalState, context: &str) -> Result<()> {
        let observed_len = matching_existing_regular_len(&self.path, &self.shared()?.file_guard)
            .map_err(|error| io_error(context, error))?;
        if observed_len != state.valid_len {
            return Err(ReactError::Other(format!(
                "{context}: journal length changed from {} to {observed_len}; verified reopen required",
                state.valid_len
            )));
        }
        Ok(())
    }

    fn sync_journal_data(&self, expected_len: u64) -> std::io::Result<()> {
        let shared = self.shared_io()?;
        #[cfg(test)]
        {
            let mut fail = self
                .sync_fault
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if std::mem::take(&mut *fail) {
                return Err(std::io::Error::other("injected journal sync_data failure"));
            }
        }
        append_existing_matching(
            &self.path,
            &shared.file_guard,
            expected_len,
            b"",
            FileDurability::SyncData,
        )
    }

    fn append_line(&self, expected_len: u64, line: &[u8]) -> std::io::Result<()> {
        let shared = self.shared_io()?;
        #[cfg(test)]
        if let Some(fault) = self
            .append_fault
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        {
            let (bytes, error_kind) = match fault {
                AppendFault::ZeroWriteInvalidData => {
                    (b"".as_slice(), std::io::ErrorKind::InvalidData)
                }
                AppendFault::FullWrite => (line, std::io::ErrorKind::Other),
                AppendFault::FullWriteInvalidData => (line, std::io::ErrorKind::InvalidData),
                AppendFault::PartialWrite { bytes } => (
                    line.get(..bytes.min(line.len())).unwrap_or(line),
                    std::io::ErrorKind::Other,
                ),
                AppendFault::PartialWriteInvalidData { bytes } => (
                    line.get(..bytes.min(line.len())).unwrap_or(line),
                    std::io::ErrorKind::InvalidData,
                ),
                AppendFault::MissingLastByteInvalidData => (
                    line.get(..line.len().saturating_sub(1)).unwrap_or(line),
                    std::io::ErrorKind::InvalidData,
                ),
                AppendFault::UnrecognizedSuffix => (b"x".as_slice(), std::io::ErrorKind::Other),
                AppendFault::UnrecognizedSuffixInvalidData => {
                    (b"x".as_slice(), std::io::ErrorKind::InvalidData)
                }
            };
            append_existing_matching(
                &self.path,
                &shared.file_guard,
                expected_len,
                bytes,
                FileDurability::Flush,
            )?;
            return Err(std::io::Error::new(
                error_kind,
                "injected append durability failure",
            ));
        }

        append_existing_matching(
            &self.path,
            &shared.file_guard,
            expected_len,
            line,
            self.durability,
        )
    }

    /// Reconcile the suffix after an append error. `Some` means the complete
    /// batch committed despite the durability error; `None` means no record
    /// committed and the entire partial frame was removed durably.
    fn reconcile_failed_append(
        &self,
        state: &mut FileJournalState,
        line: &[u8],
        context: &str,
        append_error: &std::io::Error,
    ) -> Result<Option<JournalDurabilityStatus>> {
        let shared = self.shared()?;
        #[cfg(test)]
        {
            let mut fail = self
                .reconcile_read_fault
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if std::mem::take(&mut *fail) {
                return Err(io_error(
                    context,
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "injected reconciliation read failure",
                    ),
                ));
            }
        }
        let suffix = read_existing_from_matching(&self.path, &shared.file_guard, state.valid_len)
            .map_err(|error| io_error(context, error))?;
        if suffix == line {
            return Ok(Some(JournalDurabilityStatus::Degraded {
                error: append_error.to_string(),
            }));
        }
        if line.starts_with(&suffix) {
            if !suffix.is_empty() {
                let suffix_len = u64::try_from(suffix.len()).map_err(|_| {
                    ReactError::Other(format!("{context}: journal suffix exceeds supported size"))
                })?;
                let current_len = state.valid_len.checked_add(suffix_len).ok_or_else(|| {
                    ReactError::Other(format!("{context}: journal byte length exhausted"))
                })?;
                self.truncate_partial_suffix(
                    &shared.file_guard,
                    current_len,
                    state.valid_len,
                    self.durability,
                )
                .map_err(|error| io_error(context, error))?;
            }
            return Ok(None);
        }
        Err(ReactError::Other(format!(
            "{context}: append error left an unrecognized {}-byte suffix",
            suffix.len()
        )))
    }

    fn truncate_partial_suffix(
        &self,
        file_guard: &ExistingRegularFileGuard,
        current_len: u64,
        valid_len: u64,
        durability: FileDurability,
    ) -> std::io::Result<()> {
        #[cfg(test)]
        {
            let mut fail = self
                .truncate_barrier_fault
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if std::mem::take(&mut *fail) {
                truncate_existing_matching(
                    &self.path,
                    file_guard,
                    current_len,
                    valid_len,
                    FileDurability::Flush,
                )?;
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "injected truncate durability barrier failure",
                ));
            }
        }
        truncate_existing_matching(&self.path, file_guard, current_len, valid_len, durability)
    }

    fn read_indexed_batch(
        &self,
        state: &FileJournalState,
        index: &FileBatchIndex,
        context: &str,
        confirm_durability: bool,
    ) -> Result<JournalBatchAppendReceipt<E>> {
        let shared = self.shared()?;
        let bytes = read_existing_lines_from_matching(
            &self.path,
            &shared.file_guard,
            state.valid_len,
            index.frame_offset,
            1,
        )
        .map_err(|error| io_error(context, error))?;
        let line = bytes
            .split(|byte| *byte == b'\n')
            .find(|line| !line.is_empty())
            .ok_or_else(|| {
                ReactError::Other(format!("{context}: indexed batch frame is missing"))
            })?;
        let frame = decode_journal_batch::<E>(context, line)?;
        verify_journal_batch_sequence(context, index.identity.first_sequence, &frame)?;
        if frame.identity()? != index.identity {
            return Err(ReactError::Other(format!(
                "{context}: indexed batch identity changed"
            )));
        }
        let durability = if confirm_durability {
            match self.sync_journal_data(state.valid_len) {
                Ok(()) => JournalDurabilityStatus::Confirmed,
                Err(error) => JournalDurabilityStatus::Degraded {
                    error: error.to_string(),
                },
            }
        } else {
            JournalDurabilityStatus::Unconfirmed
        };
        Ok(JournalBatchAppendReceipt {
            batch_id: frame.batch_id().to_string(),
            records: frame.into_records().into(),
            durability,
            commit: JournalBatchCommitStatus::AlreadyCommitted,
        })
    }
}

impl<E: JournalEvent> EventJournal<E> for FileEventJournal<E> {
    fn append_batch(&self, batch: PreparedJournalBatch<E>) -> JournalBatchAppendResult<E> {
        if let Err(error) = batch.validate_payload_integrity() {
            return Err(JournalBatchAppendError::prepared_mutation(
                batch,
                error.to_string(),
            ));
        }
        let shared = match self.shared() {
            Ok(shared) => shared,
            Err(error) => {
                return Err(JournalBatchAppendError::not_committed(
                    batch,
                    error.to_string(),
                ));
            }
        };
        let mut state = shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(reason) = &state.poison {
            return Err(JournalBatchAppendError::authority_poisoned(
                batch,
                format!(
                    "journal append {} refused because the handle is poisoned: {reason}; reopen the journal to recover",
                    self.path.display()
                ),
            ));
        }
        let context = format!("journal append {}", self.path.display());
        if let Err(error) = self.verify_current_file(&state, &context) {
            return Err(JournalBatchAppendError::not_committed(
                batch,
                error.to_string(),
            ));
        }
        if let Some(existing) = state.batches.get(batch.batch_id()).cloned() {
            if existing.identity.payload_digest != batch.payload_digest()
                || existing.identity.record_count != u64::try_from(batch.len()).unwrap_or(u64::MAX)
            {
                let reason = format!(
                    "batch identity {} conflicts with an existing committed payload",
                    batch.batch_id()
                );
                state.poison = Some(reason.clone());
                return Err(JournalBatchAppendError::identity_conflict(
                    batch,
                    existing.identity.first_sequence,
                    existing.identity.record_count,
                    reason,
                ));
            }
            return match self.read_indexed_batch(&state, &existing, &context, true) {
                Ok(receipt) => Ok(receipt),
                Err(error) => {
                    let reason = format!("failed to verify already committed batch: {error}");
                    state.poison = Some(reason.clone());
                    Err(JournalBatchAppendError::outcome_unknown(batch, reason))
                }
            };
        }
        let prepared = match prepare_journal_frame(&batch, state.next_sequence) {
            Ok(prepared) => prepared,
            Err(error) => {
                return Err(JournalBatchAppendError::not_committed(
                    batch,
                    error.to_string(),
                ));
            }
        };
        let line_len = match u64::try_from(prepared.line.len()) {
            Ok(line_len) => line_len,
            Err(_) => {
                return Err(JournalBatchAppendError::not_committed(
                    batch,
                    "journal batch frame exceeds supported size",
                ));
            }
        };
        let valid_len = match state.valid_len.checked_add(line_len) {
            Some(valid_len) => valid_len,
            None => {
                return Err(JournalBatchAppendError::not_committed(
                    batch,
                    "journal byte length exhausted before append",
                ));
            }
        };
        let durability = match self.append_line(state.valid_len, &prepared.line) {
            Ok(()) => JournalDurabilityStatus::Confirmed,
            Err(error) => {
                match self.reconcile_failed_append(&mut state, &prepared.line, &context, &error) {
                    Ok(Some(status)) => status,
                    Ok(None) => {
                        return Err(JournalBatchAppendError::not_committed(
                            batch,
                            io_error(&context, error).to_string(),
                        ));
                    }
                    Err(repair_error) => {
                        let reason = format!(
                            "append failed ({error}); reconciliation failed ({repair_error})"
                        );
                        state.poison = Some(reason.clone());
                        return Err(JournalBatchAppendError::outcome_unknown(
                            batch,
                            format!("{context}: {reason}; handle poisoned until reopen"),
                        ));
                    }
                }
            }
        };
        let record_offset = state.valid_len;
        state
            .record_offsets
            .extend(std::iter::repeat_n(record_offset, prepared.records.len()));
        state.valid_len = valid_len;
        state.next_sequence = prepared.next_sequence;
        let batch_id = batch.batch_id;
        state.batches.insert(
            batch_id.clone(),
            FileBatchIndex {
                identity: prepared.identity,
                frame_offset: record_offset,
            },
        );
        Ok(JournalBatchAppendReceipt {
            batch_id,
            records: prepared.records,
            durability,
            commit: JournalBatchCommitStatus::Committed,
        })
    }

    fn lookup_batch(&self, batch: &PreparedJournalBatch<E>) -> Result<JournalBatchLookup<E>> {
        if let Err(error) = batch.validate_payload_integrity() {
            return Ok(JournalBatchLookup::Conflict {
                error: error.to_string(),
            });
        }
        let context = format!("journal batch lookup {}", self.path.display());
        let shared = self.shared()?;
        let mut state = shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(reason) = &state.poison {
            return Err(ReactError::Other(format!(
                "{context}: lookup refused because the handle is poisoned: {reason}; reopen required"
            )));
        }
        self.verify_current_file(&state, &context)?;
        let Some(existing) = state.batches.get(batch.batch_id()).cloned() else {
            return Ok(JournalBatchLookup::Absent);
        };
        if existing.identity.payload_digest != batch.payload_digest()
            || existing.identity.record_count != u64::try_from(batch.len()).unwrap_or(u64::MAX)
        {
            let reason = format!(
                "batch identity {} conflicts with an existing committed payload",
                batch.batch_id()
            );
            state.poison = Some(reason.clone());
            return Ok(JournalBatchLookup::Conflict { error: reason });
        }
        match self.read_indexed_batch(&state, &existing, &context, false) {
            Ok(receipt) => Ok(JournalBatchLookup::AlreadyCommitted(receipt)),
            Err(error) => {
                state.poison = Some(error.to_string());
                Err(error)
            }
        }
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

    fn replay_after(&self, after_sequence: u64, limit: usize) -> Result<Vec<JournalRecord<E>>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let context = format!("journal replay {}", self.path.display());
        // The state lock serializes replay with append and gives O(1) access to
        // the batch frame containing the first requested record.
        let shared = self.shared()?;
        let state = shared
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
        let mut batch_start_index = offset_index;
        while batch_start_index > 0 {
            let previous_index = batch_start_index.saturating_sub(1);
            if state.record_offsets.get(previous_index).copied() != Some(start_offset) {
                break;
            }
            batch_start_index = previous_index;
        }
        let mut expected = u64::try_from(batch_start_index)
            .map_err(|_| ReactError::Other(format!("{context}: sequence exceeds supported index")))?
            .checked_add(1)
            .ok_or_else(|| ReactError::Other(format!("{context}: journal sequence exhausted")))?;
        let bytes = read_existing_lines_from_matching(
            &self.path,
            &shared.file_guard,
            state.valid_len,
            start_offset,
            limit,
        )
        .map_err(|error| io_error(&context, error))?;
        let mut records = Vec::new();
        for line in bytes
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
        {
            let frame = decode_journal_batch::<E>(&context, line)?;
            verify_journal_batch_sequence(&context, expected, &frame)?;
            let indexed = state.batches.get(frame.batch_id()).ok_or_else(|| {
                ReactError::Other(format!(
                    "{context}: replayed batch {} is absent from the authority index",
                    frame.batch_id()
                ))
            })?;
            if frame.identity()? != indexed.identity {
                return Err(ReactError::Other(format!(
                    "{context}: replayed batch {} conflicts with the authority index",
                    frame.batch_id()
                )));
            }
            let frame_count = u64::try_from(frame.records().len()).map_err(|_| {
                ReactError::Other(format!(
                    "{context}: journal batch count exceeds supported range"
                ))
            })?;
            expected = expected.checked_add(frame_count).ok_or_else(|| {
                ReactError::Other(format!("{context}: journal sequence exhausted"))
            })?;
            for record in frame.into_records() {
                if record.sequence > after_sequence {
                    records.push(record);
                    if records.len() >= limit {
                        return Ok(records);
                    }
                }
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

    fn shared<E>(journal: &FileEventJournal<E>) -> &Arc<SharedFileJournalState> {
        journal.shared.as_ref().expect("live file journal handle")
    }

    fn batch<E: JournalEvent>(events: Vec<E>) -> PreparedJournalBatch<E> {
        PreparedJournalBatch::new(events).expect("prepare test batch")
    }

    #[derive(Default, Serialize, Deserialize, Debug, PartialEq, Eq)]
    struct LensReducer {
        applied: u64,
    }

    #[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
    struct WideIntegerState {
        min: i128,
        max: u128,
    }

    #[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
    struct NonCloneBatchEvent {
        value: String,
    }

    #[derive(Debug)]
    struct MutableBatchEvent {
        value: std::sync::atomic::AtomicUsize,
    }

    impl Serialize for MutableBatchEvent {
        fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            serializer.serialize_u64(
                u64::try_from(self.value.load(std::sync::atomic::Ordering::SeqCst))
                    .unwrap_or(u64::MAX),
            )
        }
    }

    impl<'de> Deserialize<'de> for MutableBatchEvent {
        fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            let value = usize::deserialize(deserializer)?;
            Ok(Self {
                value: std::sync::atomic::AtomicUsize::new(value),
            })
        }
    }

    #[derive(Clone, Copy)]
    enum FrameTamper {
        Schema,
        BatchId,
        FirstSequence,
        RecordBatchId,
        RecordSequence,
        Payload,
        Digest,
    }

    fn tampered_frame(tamper: FrameTamper) -> Vec<u8> {
        let prepared = prepare_journal_frame(&batch(vec!["original".to_string()]), 1)
            .expect("prepare tamper frame");
        let mut frame: serde_json::Value =
            serde_json::from_slice(&prepared.line).expect("decode tamper frame");
        match tamper {
            FrameTamper::Schema => {
                if let Some(value) = frame.get_mut("schema_version") {
                    *value = serde_json::json!(99);
                }
            }
            FrameTamper::BatchId => {
                if let Some(value) = frame.get_mut("batch_id") {
                    *value = serde_json::json!(uuid::Uuid::new_v4().to_string());
                }
            }
            FrameTamper::FirstSequence => {
                if let Some(value) = frame.get_mut("first_sequence") {
                    *value = serde_json::json!(2);
                }
            }
            FrameTamper::RecordBatchId => {
                if let Some(value) = frame
                    .get_mut("records")
                    .and_then(serde_json::Value::as_array_mut)
                    .and_then(|records| records.first_mut())
                    .and_then(|record| record.get_mut("batch_id"))
                {
                    *value = serde_json::json!(uuid::Uuid::new_v4().to_string());
                }
            }
            FrameTamper::RecordSequence => {
                if let Some(value) = frame
                    .get_mut("records")
                    .and_then(serde_json::Value::as_array_mut)
                    .and_then(|records| records.first_mut())
                    .and_then(|record| record.get_mut("sequence"))
                {
                    *value = serde_json::json!(2);
                }
            }
            FrameTamper::Payload => {
                if let Some(value) = frame
                    .get_mut("records")
                    .and_then(serde_json::Value::as_array_mut)
                    .and_then(|records| records.first_mut())
                    .and_then(|record| record.get_mut("event"))
                {
                    *value = serde_json::json!("changed");
                }
            }
            FrameTamper::Digest => {
                if let Some(value) = frame.get_mut("digest") {
                    *value = serde_json::json!("0".repeat(64));
                }
            }
        }
        let mut bytes = serde_json::to_vec(&frame).expect("encode tamper frame");
        bytes.push(b'\n');
        bytes
    }

    struct VisibleFailureCheckpointStore {
        inner: FileCheckpointStore<LensReducer>,
        fail_once: std::sync::atomic::AtomicBool,
    }

    struct CountingCheckpointStore {
        saves: std::sync::atomic::AtomicUsize,
        inner: super::super::MemoryCheckpointStore<LensReducer>,
    }

    impl CountingCheckpointStore {
        fn new() -> Self {
            Self {
                saves: std::sync::atomic::AtomicUsize::new(0),
                inner: super::super::MemoryCheckpointStore::new(),
            }
        }
    }

    impl CheckpointStore<LensReducer> for CountingCheckpointStore {
        fn save(&self, state: &LensReducer, through_sequence: u64) -> Result<()> {
            self.saves.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.inner.save(state, through_sequence)
        }

        fn load(&self) -> Result<Option<CheckpointFrame<LensReducer>>> {
            self.inner.load()
        }
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
        assert!(std::sync::Arc::ptr_eq(shared(&first), shared(&second)));
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let mut handles = Vec::new();
        for (label, journal) in [("a", first.clone()), ("b", second.clone())] {
            let barrier = barrier.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                let mut sequences = Vec::new();
                for index in 0..APPENDS_PER_HANDLE {
                    let receipt = journal
                        .append(format!("{label}-{index}"))
                        .map_err(|error| ReactError::Other(error.to_string()))?;
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
    fn concurrent_file_batches_commit_as_contiguous_non_interleaved_ranges() {
        const BATCHES: usize = 12;
        let root = temp_root();
        let path = root.join("events.jsonl");
        let journal = Arc::new(
            FileEventJournal::<i32>::open(&path, FileDurability::Flush).expect("open journal"),
        );
        let barrier = Arc::new(std::sync::Barrier::new(BATCHES));
        let mut handles = Vec::new();
        for index in 0..BATCHES {
            let journal = Arc::clone(&journal);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                let base = i32::try_from(index).unwrap_or(i32::MAX).saturating_mul(10);
                journal.append_batch(batch(vec![
                    base,
                    base.saturating_add(1),
                    base.saturating_add(2),
                ]))
            }));
        }
        let mut receipts = Vec::new();
        for handle in handles {
            receipts.push(
                handle
                    .join()
                    .ok()
                    .and_then(std::result::Result::ok)
                    .expect("concurrent file batch"),
            );
        }
        let replay = journal.replay_after(0, usize::MAX).expect("replay batches");
        for receipt in receipts {
            let first = receipt
                .records
                .first()
                .map(|record| record.sequence)
                .expect("first batch record");
            let record_count = u64::try_from(receipt.records.len()).unwrap_or(u64::MAX);
            let values = replay
                .iter()
                .filter(|record| {
                    record.sequence >= first && record.sequence < first.saturating_add(record_count)
                })
                .map(|record| *record.event)
                .collect::<Vec<_>>();
            assert!(values.windows(2).all(|pair| {
                pair.first()
                    .zip(pair.get(1))
                    .is_some_and(|(left, right)| left.saturating_add(1) == *right)
            }));
        }
        drop(journal);
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
        assert_eq!(
            journal.replay_after(0, usize::MAX).expect("replay").len(),
            1
        );
        journal
            .sync_data()
            .expect("confirm degraded append without retrying event");
        journal.sync_data().expect("barrier is idempotent");
        assert_eq!(journal.next_sequence(), 2);
        assert_eq!(
            journal.replay_after(0, usize::MAX).expect("replay").len(),
            1
        );
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
        journal
            .sync_data()
            .expect("partial repair remains a valid empty journal");
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
        assert!(!second_error.is_retry_safe());
        assert!(second_error.requires_reopen());
        let barrier_error = journal
            .sync_data()
            .expect_err("poisoned handle must reject durability barrier");
        assert!(barrier_error.to_string().contains("handle is poisoned"));

        drop(journal);
        let reopened = FileEventJournal::<String>::open(&path, FileDurability::Flush)
            .expect("repair on reopen");
        let committed = reopened.append("recovered".to_string()).expect("append");
        assert_eq!(committed.record.sequence, 1);
        drop(reopened);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn file_batch_fault_matrix_is_typed_and_never_partially_visible() {
        let full_root = temp_root();
        let full_path = full_root.join("events.jsonl");
        let full = FileEventJournal::<String>::open(&full_path, FileDurability::SyncData)
            .expect("open full fault journal");
        let empty = PreparedJournalBatch::new(Vec::<String>::new())
            .expect_err("empty file batch must fail preflight");
        assert!(empty.error.contains("at least one"));
        assert!(read_existing(&full_path).expect("empty journal").is_empty());
        *full
            .append_fault
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(AppendFault::FullWriteInvalidData);
        let committed = full
            .append_batch(batch(vec![
                "a".to_string(),
                "b".to_string(),
                "c".to_string(),
            ]))
            .expect("full frame owns the whole batch");
        assert_eq!(committed.records.len(), 3);
        assert!(matches!(
            committed.durability,
            JournalDurabilityStatus::Degraded { .. }
        ));
        assert_eq!(full.next_sequence(), 4);
        drop(full);
        let full_reopened = FileEventJournal::<String>::open(&full_path, FileDurability::SyncData)
            .expect("reopen committed batch");
        assert_eq!(
            full_reopened
                .replay_after(0, usize::MAX)
                .expect("cold replay committed batch")
                .len(),
            3
        );
        assert_eq!(
            full_reopened
                .replay_after(1, 1)
                .expect("replay inside committed batch")
                .first()
                .map(|record| record.event.as_str()),
            Some("b")
        );
        assert_eq!(
            full_reopened
                .replay_after(0, usize::MAX)
                .expect("lookup committed batch")
                .iter()
                .filter(|record| record.batch_id == committed.batch_id)
                .count(),
            3
        );
        drop(full_reopened);

        let full_unknown_root = temp_root();
        let full_unknown_path = full_unknown_root.join("events.jsonl");
        let full_unknown =
            FileEventJournal::<String>::open(&full_unknown_path, FileDurability::SyncData)
                .expect("open full unknown journal");
        *full_unknown
            .append_fault
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(AppendFault::FullWriteInvalidData);
        *full_unknown
            .reconcile_read_fault
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = true;
        let full_unknown_prepared = batch(vec!["a".to_string(), "b".to_string()]);
        let full_unknown_id = full_unknown_prepared.batch_id().to_string();
        let full_unknown_error = full_unknown
            .append_batch(full_unknown_prepared)
            .expect_err("full write without reconciliation proof is unknown");
        assert!(matches!(
            full_unknown_error,
            JournalBatchAppendError::OutcomeUnknown { .. }
        ));
        let full_unknown_returned = full_unknown_error
            .into_prepared()
            .expect("unknown outcome retains prepared batch");
        drop(full_unknown);
        let full_unknown_reopened =
            FileEventJournal::<String>::open(&full_unknown_path, FileDurability::SyncData)
                .expect("reopen full unknown journal");
        *full_unknown_reopened
            .sync_fault
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = true;
        let lookup = full_unknown_reopened
            .lookup_batch(&full_unknown_returned)
            .expect("lookup full unknown batch");
        let (reconciled_id, reconciled_len, lookup_durability) = match lookup {
            JournalBatchLookup::AlreadyCommitted(receipt) => (
                receipt.batch_id().to_string(),
                receipt.records().len(),
                receipt.durability().clone(),
            ),
            JournalBatchLookup::Absent | JournalBatchLookup::Conflict { .. } => {
                (String::new(), 0, JournalDurabilityStatus::Confirmed)
            }
        };
        assert_eq!(reconciled_id, full_unknown_id);
        assert_eq!(reconciled_len, 2);
        assert_eq!(lookup_durability, JournalDurabilityStatus::Unconfirmed);
        drop(full_unknown_reopened);

        let partial_root = temp_root();
        let partial_path = partial_root.join("events.jsonl");
        let partial = FileEventJournal::<String>::open(&partial_path, FileDurability::SyncData)
            .expect("open partial fault journal");
        *partial
            .append_fault
            .lock()
            .unwrap_or_else(|error| error.into_inner()) =
            Some(AppendFault::PartialWriteInvalidData { bytes: 17 });
        let not_committed = partial
            .append_batch(batch(vec![
                "a".to_string(),
                "b".to_string(),
                "c".to_string(),
            ]))
            .expect_err("partial frame must be removed as one batch");
        assert!(matches!(
            not_committed,
            JournalBatchAppendError::NotCommitted { .. }
        ));
        assert!(not_committed.is_retry_safe());
        assert_eq!(partial.next_sequence(), 1);
        assert!(
            read_existing(&partial_path)
                .expect("repaired file")
                .is_empty()
        );
        drop(partial);

        let short_root = temp_root();
        let short_path = short_root.join("events.jsonl");
        let short = FileEventJournal::<String>::open(&short_path, FileDurability::SyncData)
            .expect("open missing-last-byte journal");
        *short
            .append_fault
            .lock()
            .unwrap_or_else(|error| error.into_inner()) =
            Some(AppendFault::MissingLastByteInvalidData);
        let short_prepared = batch(vec!["one".to_string(), "two".to_string()]);
        let short_id = short_prepared.batch_id().to_string();
        let short_error = short
            .append_batch(short_prepared)
            .expect_err("line len minus one must repair the full batch");
        assert!(short_error.is_retry_safe());
        assert!(
            read_existing(&short_path)
                .expect("short repaired")
                .is_empty()
        );
        let short_retry = short
            .append_batch(short_error.into_prepared().expect("retryable short batch"))
            .expect("retry short batch");
        assert_eq!(short_retry.batch_id, short_id);
        drop(short);

        let barrier_root = temp_root();
        let barrier_path = barrier_root.join("events.jsonl");
        let barrier = FileEventJournal::<String>::open(&barrier_path, FileDurability::SyncData)
            .expect("open truncate barrier journal");
        *barrier
            .append_fault
            .lock()
            .unwrap_or_else(|error| error.into_inner()) =
            Some(AppendFault::PartialWriteInvalidData { bytes: 23 });
        *barrier
            .truncate_barrier_fault
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = true;
        let barrier_prepared = batch(vec!["one".to_string(), "two".to_string()]);
        let barrier_id = barrier_prepared.batch_id().to_string();
        let barrier_error = barrier
            .append_batch(barrier_prepared)
            .expect_err("truncate barrier ambiguity must poison");
        assert!(matches!(
            barrier_error,
            JournalBatchAppendError::OutcomeUnknown { .. }
        ));
        assert_eq!(barrier_error.batch_id(), barrier_id);
        let barrier_returned = barrier_error
            .into_prepared()
            .expect("truncate ambiguity retains prepared batch");
        drop(barrier);
        let barrier_reopened =
            FileEventJournal::<String>::open(&barrier_path, FileDurability::SyncData)
                .expect("reopen truncate ambiguity");
        assert!(matches!(
            barrier_reopened
                .lookup_batch(&barrier_returned)
                .expect("lookup absent ambiguous batch"),
            JournalBatchLookup::Absent
        ));
        let barrier_retry = barrier_reopened
            .append_batch(barrier_returned)
            .expect("retry proven absent batch");
        assert_eq!(barrier_retry.batch_id(), barrier_id);
        drop(barrier_reopened);

        let unknown_root = temp_root();
        let unknown_path = unknown_root.join("events.jsonl");
        let unknown = FileEventJournal::<String>::open(&unknown_path, FileDurability::Flush)
            .expect("open unknown fault journal");
        *unknown
            .append_fault
            .lock()
            .unwrap_or_else(|error| error.into_inner()) =
            Some(AppendFault::UnrecognizedSuffixInvalidData);
        let outcome = unknown
            .append_batch(batch(vec!["a".to_string(), "b".to_string()]))
            .expect_err("unrecognized suffix has unknown outcome");
        assert!(matches!(
            outcome,
            JournalBatchAppendError::OutcomeUnknown { .. }
        ));
        assert!(!outcome.is_retry_safe());
        assert!(outcome.requires_reopen());
        let refused = unknown
            .append_batch(batch(vec!["retry".to_string()]))
            .expect_err("poisoned authority forbids blind retry");
        assert!(matches!(
            &refused,
            JournalBatchAppendError::AuthorityPoisoned { .. }
        ));
        assert!(!refused.is_retry_safe());
        assert!(refused.requires_reopen());
        drop(unknown);

        std::fs::remove_dir_all(full_root).ok();
        std::fs::remove_dir_all(full_unknown_root).ok();
        std::fs::remove_dir_all(partial_root).ok();
        std::fs::remove_dir_all(short_root).ok();
        std::fs::remove_dir_all(barrier_root).ok();
        std::fs::remove_dir_all(unknown_root).ok();
    }

    #[test]
    fn zero_write_not_committed_returns_same_non_clone_prepared_batch() {
        let root = temp_root();
        let path = root.join("events.jsonl");
        let journal = FileEventJournal::<NonCloneBatchEvent>::open(&path, FileDurability::SyncData)
            .expect("open non-clone batch journal");
        *journal
            .append_fault
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(AppendFault::ZeroWriteInvalidData);
        let not_committed = journal
            .append(NonCloneBatchEvent {
                value: "owned-once".to_string(),
            })
            .expect_err("zero-byte write error must be retryable");
        assert!(not_committed.is_retry_safe());
        assert!(read_existing(&path).expect("zero-write file").is_empty());
        let returned = not_committed
            .into_prepared()
            .expect("returned prepared batch");
        let batch_id = returned.batch_id().to_string();
        let payload = returned
            .events()
            .first()
            .cloned()
            .expect("returned payload");
        assert_eq!(returned.batch_id(), batch_id);
        assert!(
            returned
                .events()
                .first()
                .is_some_and(|event| Arc::ptr_eq(event, &payload))
        );
        let committed = journal.append_batch(returned).expect("retry same batch");
        assert_eq!(committed.batch_id(), batch_id);
        assert_eq!(
            committed.records().first().map(JournalRecord::batch_id),
            Some(batch_id.as_str())
        );
        assert!(
            committed
                .records()
                .first()
                .is_some_and(|record| Arc::ptr_eq(&record.event, &payload))
        );

        let duplicate = PreparedJournalBatch::with_test_identity(
            batch_id.clone(),
            vec![NonCloneBatchEvent {
                value: "owned-once".to_string(),
            }],
        )
        .expect("same identity duplicate");
        let idempotent = journal
            .append_batch(duplicate)
            .expect("idempotent duplicate");
        assert_eq!(
            idempotent.commit_status(),
            JournalBatchCommitStatus::AlreadyCommitted
        );
        assert_eq!(journal.last_sequence(), 1);

        let conflict = PreparedJournalBatch::with_test_identity(
            batch_id,
            vec![NonCloneBatchEvent {
                value: "different".to_string(),
            }],
        )
        .expect("conflicting identity");
        let conflict_error = journal
            .append_batch(conflict)
            .expect_err("same id different payload must poison");
        assert!(conflict_error.to_string().contains("conflicts"));
        assert!(!conflict_error.is_retry_safe());
        drop(journal);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn single_non_clone_unknown_outcome_retains_identity_for_cold_lookup() {
        let root = temp_root();
        let path = root.join("events.jsonl");
        let journal = FileEventJournal::<NonCloneBatchEvent>::open(&path, FileDurability::SyncData)
            .expect("open single unknown journal");
        *journal
            .append_fault
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(AppendFault::FullWriteInvalidData);
        *journal
            .reconcile_read_fault
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = true;
        let error = journal
            .append(NonCloneBatchEvent {
                value: "unknown-once".to_string(),
            })
            .expect_err("single full-write ambiguity must remain typed");
        assert!(!error.is_retry_safe());
        let batch_id = error.batch_id().to_string();
        let returned = error
            .into_prepared()
            .expect("unknown single retains prepared payload");
        drop(journal);

        let reopened =
            FileEventJournal::<NonCloneBatchEvent>::open(&path, FileDurability::SyncData)
                .expect("reopen single unknown journal");
        let lookup = reopened
            .lookup_batch(&returned)
            .expect("lookup unknown single");
        let (resolved_id, resolved_sequence) = match lookup {
            JournalBatchLookup::AlreadyCommitted(receipt) => (
                receipt.batch_id().to_string(),
                receipt.records().first().map(|record| record.sequence),
            ),
            JournalBatchLookup::Absent | JournalBatchLookup::Conflict { .. } => {
                (String::new(), None)
            }
        };
        assert_eq!(resolved_id, batch_id);
        assert_eq!(resolved_sequence, Some(1));
        drop(reopened);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn file_rejects_mutated_unknown_payload_before_cold_idempotent_match() {
        let root = temp_root();
        let path = root.join("events.jsonl");
        let journal = FileEventJournal::<MutableBatchEvent>::open(&path, FileDurability::SyncData)
            .expect("open mutable unknown journal");
        *journal
            .append_fault
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(AppendFault::FullWriteInvalidData);
        *journal
            .reconcile_read_fault
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = true;
        let prepared = batch(vec![MutableBatchEvent {
            value: std::sync::atomic::AtomicUsize::new(1),
        }]);
        let batch_id = prepared.batch_id().to_string();
        let unknown = journal
            .append_batch(prepared)
            .expect_err("full write ambiguity retains mutable payload");
        let mutated = unknown.into_prepared().expect("returned mutable payload");
        if let Some(event) = mutated.events().first() {
            event.value.store(2, std::sync::atomic::Ordering::SeqCst);
        }
        drop(journal);

        let reopened = FileEventJournal::<MutableBatchEvent>::open(&path, FileDurability::SyncData)
            .expect("reopen mutable unknown journal");
        assert!(matches!(
            reopened
                .lookup_batch(&mutated)
                .expect("lookup mutated payload"),
            JournalBatchLookup::Conflict { .. }
        ));
        let mutation = reopened
            .append_batch(mutated)
            .expect_err("mutated payload must not match old commit");
        assert!(matches!(
            mutation,
            JournalBatchAppendError::PreparedMutation { .. }
        ));
        assert!(!mutation.is_retry_safe());
        assert_eq!(reopened.last_sequence(), 1);

        let original = PreparedJournalBatch::with_test_identity(
            batch_id,
            vec![MutableBatchEvent {
                value: std::sync::atomic::AtomicUsize::new(1),
            }],
        )
        .expect("prepare original payload lookup");
        assert!(matches!(
            reopened.lookup_batch(&original).expect("lookup old digest"),
            JournalBatchLookup::AlreadyCommitted(_)
        ));
        drop(reopened);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn reducer_does_not_refold_already_committed_batch_after_cold_recovery() {
        let root = temp_root();
        let path = root.join("events.jsonl");
        let journal = Arc::new(
            FileEventJournal::<String>::open(&path, FileDurability::SyncData)
                .expect("open reducer unknown journal"),
        );
        let checkpoints = Arc::new(CountingCheckpointStore::new());
        let writer = CheckpointedReducer::<_, LensReducer>::new(
            Arc::clone(&journal),
            Arc::clone(&checkpoints) as Arc<dyn CheckpointStore<LensReducer>>,
            1,
        );
        *journal
            .append_fault
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(AppendFault::FullWriteInvalidData);
        *journal
            .reconcile_read_fault
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = true;
        let error = writer
            .apply_batch(batch(vec!["one".to_string(), "two".to_string()]))
            .expect_err("full write ambiguity must stay typed through reducer");
        assert_eq!(writer.last_applied_sequence(), 0);
        let returned = error
            .into_prepared()
            .expect("reducer error retains prepared batch");
        drop(writer);
        drop(journal);

        let reopened = Arc::new(
            FileEventJournal::<String>::open(&path, FileDurability::SyncData)
                .expect("reopen reducer journal"),
        );
        let recovered = CheckpointedReducer::<_, LensReducer>::new(
            reopened,
            Arc::clone(&checkpoints) as Arc<dyn CheckpointStore<LensReducer>>,
            1,
        );
        assert_eq!(
            recovered
                .recover()
                .expect("recover committed batch")
                .last_applied_sequence,
            2
        );
        let saves_before = checkpoints.saves.load(std::sync::atomic::Ordering::SeqCst);
        let applied_before = recovered.with_state(|state| state.applied);
        let receipt = recovered
            .apply_batch(returned)
            .expect("idempotent reducer apply");
        assert_eq!(receipt.commit, JournalBatchCommitStatus::AlreadyCommitted);
        assert_eq!(receipt.checkpoint, CheckpointApplyStatus::NotDue);
        assert_eq!(recovered.last_applied_sequence(), 2);
        assert_eq!(recovered.with_state(|state| state.applied), applied_before);
        assert_eq!(
            checkpoints.saves.load(std::sync::atomic::Ordering::SeqCst),
            saves_before
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn sync_data_failure_is_retryable_without_advancing_sequence() {
        let root = temp_root();
        let path = root.join("events.jsonl");
        let journal =
            FileEventJournal::<String>::open(&path, FileDurability::Flush).expect("open journal");
        journal.append("one".to_string()).expect("append");
        *journal
            .sync_fault
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = true;

        let error = journal
            .sync_data()
            .expect_err("injected barrier failure must surface");
        assert!(
            error
                .to_string()
                .contains("injected journal sync_data failure")
        );
        assert_eq!(journal.next_sequence(), 2);
        journal.sync_data().expect("retry durability barrier");
        assert_eq!(journal.next_sequence(), 2);
        assert_eq!(
            journal.replay_after(0, usize::MAX).expect("replay").len(),
            1
        );

        drop(journal);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn replacement_requires_last_handle_close_and_verified_reopen() {
        let root = temp_root();
        let path = root.join("events.jsonl");
        let journal =
            FileEventJournal::<String>::open(&path, FileDurability::Flush).expect("open journal");
        journal.append("one".to_string()).expect("append original");
        let original = read_existing(&path).expect("read original record");
        let replacement = prepare_journal_frame(&batch(vec!["two".to_string()]), 1)
            .expect("encode replacement batch frame")
            .line;
        assert_eq!(replacement.len(), original.len());
        std::fs::remove_file(&path).expect("remove journal fixture");
        let missing_open = FileEventJournal::<String>::open(&path, FileDurability::Flush)
            .expect_err("live missing file must not be recreated");
        assert!(missing_open.to_string().contains("journal open"));
        assert!(!path.exists());
        assert_eq!(journal.next_sequence(), 2);
        std::fs::write(&path, &replacement).expect("write same-length replacement");

        let replay_error = journal
            .replay_after(0, usize::MAX)
            .expect_err("replacement must reject replay");
        assert!(replay_error.to_string().contains("identity changed"));
        let append_error = journal
            .append("must-not-commit".to_string())
            .expect_err("replacement must reject append");
        assert!(append_error.to_string().contains("identity changed"));
        let sync_error = journal
            .sync_data()
            .expect_err("replacement must reject barrier");
        assert!(sync_error.to_string().contains("identity changed"));
        let second_open = FileEventJournal::<String>::open(&path, FileDurability::Flush)
            .expect_err("live replacement must not reset shared state");
        assert!(second_open.to_string().contains("identity changed"));
        assert_eq!(journal.next_sequence(), 2);
        assert!(
            shared(&journal)
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .poison
                .is_none()
        );

        drop(journal);
        let reopened = FileEventJournal::<String>::open(&path, FileDurability::Flush)
            .expect("verified new authority opens replacement");
        let records = reopened
            .replay_after(0, usize::MAX)
            .expect("replay replacement");
        assert_eq!(
            records.first().map(|record| record.event.as_str()),
            Some("two")
        );
        assert_eq!(
            reopened
                .append("after-reopen".to_string())
                .expect("append after verified reopen")
                .record
                .sequence,
            2
        );
        drop(reopened);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn live_reopen_repairs_only_a_torn_extra_suffix() {
        let root = temp_root();
        let path = root.join("events.jsonl");
        let journal =
            FileEventJournal::<String>::open(&path, FileDurability::Flush).expect("open journal");
        journal.append("one".to_string()).expect("append original");
        let committed_len = read_existing(&path).expect("read original").len();
        append_existing(&path, b"{\"sequence\":2", FileDurability::Flush)
            .expect("append torn external suffix");

        assert!(journal.append("blocked".to_string()).is_err());
        assert!(journal.sync_data().is_err());
        assert_eq!(journal.next_sequence(), 2);
        let repaired = FileEventJournal::<String>::open(&path, FileDurability::Flush)
            .expect("verified live reopen repairs torn suffix");
        assert!(Arc::ptr_eq(shared(&journal), shared(&repaired)));
        assert_eq!(
            read_existing(&path).expect("read repaired journal").len(),
            committed_len
        );
        assert_eq!(
            repaired
                .append("two".to_string())
                .expect("append after repair")
                .record
                .sequence,
            2
        );

        drop(journal);
        drop(repaired);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn truncated_live_journal_requires_authority_close_before_repair() {
        let root = temp_root();
        let path = root.join("events.jsonl");
        let journal =
            FileEventJournal::<String>::open(&path, FileDurability::Flush).expect("open journal");
        journal.append("one".to_string()).expect("append original");
        let len = std::fs::metadata(&path).expect("journal metadata").len();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("open journal fixture")
            .set_len(len.saturating_sub(1))
            .expect("truncate journal fixture");

        assert!(journal.append("blocked".to_string()).is_err());
        assert!(journal.sync_data().is_err());
        let second_open = FileEventJournal::<String>::open(&path, FileDurability::Flush)
            .expect_err("live truncated authority must not reset");
        assert!(second_open.to_string().contains("live journal shrank"));
        assert_eq!(journal.next_sequence(), 2);

        drop(journal);
        let repaired = FileEventJournal::<String>::open(&path, FileDurability::Flush)
            .expect("new authority repairs torn record");
        assert_eq!(repaired.next_sequence(), 1);
        assert_eq!(
            repaired
                .append("replacement".to_string())
                .expect("append after verified repair")
                .record
                .sequence,
            1
        );
        drop(repaired);
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
    fn cold_recovery_discards_every_record_in_a_partial_batch_frame() {
        let root = temp_root();
        let path = root.join("events.jsonl");
        let journal = FileEventJournal::<String>::open(&path, FileDurability::SyncData)
            .expect("open journal");
        drop(journal);
        let prepared = prepare_journal_frame(
            &batch(vec![
                "one".to_string(),
                "two".to_string(),
                "three".to_string(),
            ]),
            1,
        )
        .expect("prepare batch frame");
        let partial_len = prepared.line.len().saturating_sub(1).max(1) / 2;
        let partial = prepared
            .line
            .get(..partial_len)
            .expect("partial frame range");
        append_existing(&path, partial, FileDurability::Flush).expect("write partial frame");

        let reopened =
            FileEventJournal::<String>::open(&path, FileDurability::SyncData).expect("repair open");
        assert_eq!(reopened.next_sequence(), 1);
        assert!(
            reopened
                .replay_after(0, usize::MAX)
                .expect("replay")
                .is_empty()
        );
        assert!(read_existing(&path).expect("read repaired").is_empty());
        drop(reopened);
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
        drop(journal);
        std::fs::write(&path, corrupted).expect("write corrupted");

        let error = FileEventJournal::<String>::open(&path, FileDurability::Flush)
            .expect_err("corruption must fail");
        assert!(error.to_string().contains("corrupt journal batch"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn file_batch_frame_rejects_every_integrity_field_tamper() {
        for tamper in [
            FrameTamper::Schema,
            FrameTamper::BatchId,
            FrameTamper::FirstSequence,
            FrameTamper::RecordBatchId,
            FrameTamper::RecordSequence,
            FrameTamper::Payload,
            FrameTamper::Digest,
        ] {
            let root = temp_root();
            let path = root.join("events.jsonl");
            std::fs::write(&path, tampered_frame(tamper)).expect("write tampered frame");
            let result = FileEventJournal::<String>::open(&path, FileDurability::SyncData);
            assert!(result.is_err());
            std::fs::remove_dir_all(root).ok();
        }
    }

    #[test]
    fn file_cold_scan_rejects_a_duplicated_complete_batch_frame() {
        let root = temp_root();
        let path = root.join("events.jsonl");
        let frame = prepare_journal_frame(&batch(vec!["one".to_string()]), 1)
            .expect("prepare duplicate frame");
        let mut duplicated = frame.line.clone();
        duplicated.extend_from_slice(&frame.line);
        std::fs::write(&path, duplicated).expect("write duplicated frame");
        let error = FileEventJournal::<String>::open(&path, FileDurability::SyncData)
            .expect_err("duplicate physical identity must fail closed");
        assert!(
            error
                .to_string()
                .contains("duplicate physical batch identity")
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn sequence_gap_is_an_error() {
        let root = temp_root();
        let path = root.join("events.jsonl");
        let first =
            prepare_journal_frame(&batch(vec!["a".to_string()]), 1).expect("first batch frame");
        let third =
            prepare_journal_frame(&batch(vec!["c".to_string()]), 3).expect("third batch frame");
        let mut gap = first.line;
        gap.extend_from_slice(&third.line);
        std::fs::write(&path, gap).expect("write gap");
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
        assert!(Arc::ptr_eq(shared(&first), shared(&second)));
        let type_error = FileEventJournal::<u64>::open(&path, FileDurability::Flush)
            .expect_err("mismatched event type must reject");
        assert!(type_error.to_string().contains("different event type"));
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
    fn registry_mass_live_drop_is_exact_across_multiple_waves() {
        let root = temp_root();
        let canonical_root = std::fs::canonicalize(&root).expect("canonical test root");
        let live_path = canonical_root.join("live.jsonl");
        let live = FileEventJournal::<String>::open(&live_path, FileDurability::Flush)
            .expect("open live journal");
        let per_wave = super::super::WEAK_REGISTRY_HARD_LIMIT.saturating_add(8);
        for wave in 0..2 {
            let mut journals = Vec::with_capacity(per_wave);
            for index in 0..per_wave {
                let path = canonical_root.join(format!("wave-{wave}-{index}.jsonl"));
                journals.push(
                    FileEventJournal::<String>::open(path, FileDurability::Flush)
                        .expect("open live wave journal"),
                );
            }
            let registry = file_journal_registry()
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            assert_eq!(registry.paths_beneath(&canonical_root), per_wave + 1);
            let retained = registry.upgrade(&live_path).expect("live registry entry");
            assert!(Arc::ptr_eq(&retained, shared(&live)));
            drop(retained);
            drop(registry);
            drop(journals);
            assert_eq!(
                file_journal_registry()
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .paths_beneath(&canonical_root),
                1
            );
        }

        let alias = FileEventJournal::<String>::open(&live_path, FileDurability::Flush)
            .expect("reopen live authority");
        assert!(Arc::ptr_eq(shared(&live), shared(&alias)));
        drop(live);
        drop(alias);
        assert_eq!(
            file_journal_registry()
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .paths_beneath(&canonical_root),
            0
        );

        let reopened = FileEventJournal::<String>::open(&live_path, FileDurability::Flush)
            .expect("reacquire authority and lease");
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
            file_journal_registry()
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .paths_beneath(&canonical_root),
            0
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn concurrent_two_alias_drop_and_immediate_reopen_remains_available() {
        let root = temp_root();
        let canonical_root = std::fs::canonicalize(&root).expect("canonical test root");
        let path = canonical_root.join("events.jsonl");
        for _ in 0..64 {
            let first = FileEventJournal::<String>::open(&path, FileDurability::Flush)
                .expect("open first alias");
            let second = FileEventJournal::<String>::open(&path, FileDurability::Flush)
                .expect("open second alias");
            let barrier = Arc::new(std::sync::Barrier::new(3));
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
            let reopened = FileEventJournal::<String>::open(&path, FileDurability::Flush)
                .expect("immediate reopen during alias drops");
            first_drop.join().expect("join first dropping handle");
            second_drop.join().expect("join second dropping handle");
            assert_eq!(
                file_journal_registry()
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .paths_beneath(&canonical_root),
                1
            );
            drop(reopened);
            assert_eq!(
                file_journal_registry()
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .paths_beneath(&canonical_root),
                0
            );
        }
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
        assert!(Arc::ptr_eq(shared(&first), shared(&second)));
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
