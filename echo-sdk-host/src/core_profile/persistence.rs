//! Durable state of the core profile under the explicit state root.
//!
//! Layout (all paths below `sdk_profile.state_root`):
//!
//! ```text
//! runtime_state/     framework Agent checkpoints (FileRuntimeStateStore)
//! journals/<run>/    one SegmentedFileEventJournal per run (full events)
//! host/generation    monotonic Host generation counter (atomic rewrite)
//! host/run-index.json  run metadata: identity, status, terminal, receipt
//! ```
//!
//! The Host restart never revives an interrupted driver: index entries still
//! marked `running` at startup are surfaced as `RunStatus::Interrupted` with
//! no terminal and no receipt, while settled runs keep their single
//! authoritative terminal. Journals preserve every committed envelope, so
//! recovered streams can replay exactly what the previous process accepted.

use echo_agent::error::{ReactError, Result};
use echo_agent::state::FileRuntimeStateStore;
use echo_agent::state::journal::SegmentedFileEventJournal;
use echo_agent::utils::fs::{FileDurability, atomic_write};
use echo_sdk_protocol::methods::{RunReceiptWire, RunStatus, RunTerminal};
use echo_sdk_protocol::scalar::WirePath;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Maximum bytes of one active journal segment before rotation.
const MAX_JOURNAL_SEGMENT_BYTES: u64 = 4 * 1024 * 1024;
/// Maximum run index entries retained; the oldest settled runs are pruned
/// first so the index stays bounded across long-lived state roots.
const MAX_RUN_INDEX_ENTRIES: usize = 1024;
const MAX_RUN_INDEX_BYTES: u64 = 4 * 1024 * 1024;
const MAX_SESSION_INDEX_ENTRIES: usize = 1024;
const MAX_SESSION_INDEX_BYTES: u64 = 4 * 1024 * 1024;

/// One persisted run record. `status` is the *durable* status: `running`
/// means the previous process was still driving it when it exited, which the
/// next process reports as [`RunStatus::Interrupted`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RunIndexRecord {
    pub run_id: String,
    pub stream_id: String,
    pub session_id: String,
    pub agent_handle_id: String,
    /// `running` or `settled`; interrupted is derived (a record still
    /// `running` after a restart means the process died mid-run).
    pub status: String,
    #[serde(default)]
    pub last_sequence: u64,
    #[serde(default)]
    pub input_kind: String,
    /// Canonical Agent construction fingerprint used to reject recovery into
    /// a different definition after a Host restart.
    #[serde(default)]
    pub agent_config_fingerprint: String,
    /// Session working directory captured at start, never reconstructed from
    /// the recovering process cwd.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<WirePath>,
    /// The single authoritative terminal, stored verbatim at settle time.
    /// Interrupted runs carry none — the index never fabricates outcomes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<RunTerminal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<RunReceiptWire>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionIndexRecord {
    pub session_id: String,
    pub agent_handle_id: String,
    #[serde(default)]
    pub agent_config_fingerprint: String,
    pub cwd: WirePath,
}

/// One historical run recovered from the index and its journal.
pub(crate) struct RecoveredRun {
    pub record: RunIndexRecord,
    /// `Interrupted` for runs whose previous process never settled them.
    pub status: RunStatus,
    pub last_sequence: u64,
    pub terminal: Option<RunTerminal>,
    pub receipt: Option<RunReceiptWire>,
}

/// Durable stores of one Host generation.
pub(crate) struct CorePersistence {
    state_root: PathBuf,
    generation: u64,
    state_store: Arc<FileRuntimeStateStore>,
    index_lock: Mutex<()>,
}

impl CorePersistence {
    /// Open (or create) the durable state for a new Host generation. Takes
    /// the process-exclusive state-store lease, so a second live Host on the
    /// same state root fails here instead of corrupting checkpoints.
    pub fn open(state_root: &Path) -> Result<Self> {
        let journals = state_root.join("journals");
        let host_dir = state_root.join("host");
        std::fs::create_dir_all(&journals).map_err(ReactError::Io)?;
        std::fs::create_dir_all(&host_dir).map_err(ReactError::Io)?;
        let state_store = Arc::new(FileRuntimeStateStore::new(state_root)?);
        let generation = advance_generation(&host_dir)?;
        Ok(Self {
            state_root: state_root.to_path_buf(),
            generation,
            state_store,
            index_lock: Mutex::new(()),
        })
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn state_store(&self) -> Arc<FileRuntimeStateStore> {
        self.state_store.clone()
    }

    pub fn journal_directory(&self, run_id: &str) -> PathBuf {
        self.state_root.join("journals").join(sanitize(run_id))
    }

    /// Open the per-run segmented journal. Each run owns a directory, so
    /// journal sequence 1..N aligns with that run's envelope sequence.
    pub fn open_run_journal(
        &self,
        run_id: &str,
    ) -> Result<Arc<SegmentedFileEventJournal<echo_agent::agent::EventEnvelope>>> {
        SegmentedFileEventJournal::open(
            self.journal_directory(run_id),
            MAX_JOURNAL_SEGMENT_BYTES,
            FileDurability::Flush,
        )
        .map(Arc::new)
        .map_err(|error| ReactError::Other(format!("failed to open run journal: {error}")))
    }

    fn index_path(&self) -> PathBuf {
        self.state_root.join("host").join("run-index.json")
    }

    fn sessions_path(&self) -> PathBuf {
        self.state_root.join("host").join("session-index.json")
    }

    pub fn record_session(
        &self,
        session_id: &str,
        agent_handle_id: &str,
        agent_config_fingerprint: &str,
        cwd: WirePath,
    ) -> Result<()> {
        cwd.validate()
            .map_err(|error| ReactError::Other(format!("invalid session cwd: {error}")))?;
        let _guard = self
            .index_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let path = self.sessions_path();
        if let Ok(metadata) = std::fs::metadata(&path)
            && metadata.len() > MAX_SESSION_INDEX_BYTES
        {
            return Err(ReactError::Other(format!(
                "session index exceeds the {MAX_SESSION_INDEX_BYTES} byte bound"
            )));
        }
        let mut records: Vec<SessionIndexRecord> = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|error| ReactError::Other(format!("session index is corrupt: {error}")))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(ReactError::Io(error)),
        };
        if records.len() >= MAX_SESSION_INDEX_ENTRIES
            && !records.iter().any(|record| record.session_id == session_id)
        {
            return Err(ReactError::Other(format!(
                "session index limit {MAX_SESSION_INDEX_ENTRIES} reached"
            )));
        }
        records.retain(|record| record.session_id != session_id);
        records.push(SessionIndexRecord {
            session_id: session_id.to_string(),
            agent_handle_id: agent_handle_id.to_string(),
            agent_config_fingerprint: agent_config_fingerprint.to_string(),
            cwd,
        });
        let bytes = serde_json::to_vec_pretty(&records).map_err(|error| {
            ReactError::Other(format!("failed to encode session index: {error}"))
        })?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_SESSION_INDEX_BYTES {
            return Err(ReactError::Other(format!(
                "session index exceeds the {MAX_SESSION_INDEX_BYTES} byte bound"
            )));
        }
        atomic_write(&path, &bytes).map_err(ReactError::Io)
    }

    pub fn load_session(&self, session_id: &str) -> Result<Option<SessionIndexRecord>> {
        let path = self.sessions_path();
        if let Ok(metadata) = std::fs::metadata(&path)
            && metadata.len() > MAX_SESSION_INDEX_BYTES
        {
            return Err(ReactError::Other(format!(
                "session index exceeds the {MAX_SESSION_INDEX_BYTES} byte bound"
            )));
        }
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(ReactError::Io(error)),
        };
        let records: Vec<SessionIndexRecord> = serde_json::from_slice(&bytes)
            .map_err(|error| ReactError::Other(format!("session index is corrupt: {error}")))?;
        if records.len() > MAX_SESSION_INDEX_ENTRIES {
            return Err(ReactError::Other(format!(
                "session index contains more than {MAX_SESSION_INDEX_ENTRIES} entries"
            )));
        }
        Ok(records
            .into_iter()
            .find(|record| record.session_id == session_id))
    }

    /// Load the run index and classify every record. Runs whose durable
    /// status is `running` become `Interrupted` — never completed, never
    /// auto-resumed from an arbitrary LLM/tool instruction point.
    pub fn load_recovered_runs(&self) -> Result<Vec<RecoveredRun>> {
        let path = self.index_path();
        if let Ok(metadata) = std::fs::metadata(&path)
            && metadata.len() > MAX_RUN_INDEX_BYTES
        {
            return Err(ReactError::Other(format!(
                "run index exceeds the {MAX_RUN_INDEX_BYTES} byte bound"
            )));
        }
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(ReactError::Io(error)),
        };
        let records: Vec<RunIndexRecord> = serde_json::from_slice(&bytes)
            .map_err(|error| ReactError::Other(format!("run index is corrupt: {error}")))?;
        if records.len() > MAX_RUN_INDEX_ENTRIES {
            return Err(ReactError::Other(format!(
                "run index contains more than {MAX_RUN_INDEX_ENTRIES} entries"
            )));
        }
        let mut runs = Vec::with_capacity(records.len());
        for record in records {
            // `running` (or any unknown class) after a process restart means
            // the previous Host died mid-run: the run is Interrupted with no
            // terminal and no receipt — never completed, never resumed.
            let (status, terminal) = if record.status == "running" {
                (RunStatus::Interrupted, None)
            } else {
                let status = match record.terminal.as_ref() {
                    Some(RunTerminal::Completed { .. }) => RunStatus::Completed,
                    Some(RunTerminal::Cancelled) => RunStatus::Cancelled,
                    Some(RunTerminal::Failed { .. }) => RunStatus::Failed,
                    None => RunStatus::Interrupted,
                };
                (status, record.terminal.clone())
            };
            let receipt = record.receipt.clone();
            runs.push(RecoveredRun {
                record,
                status,
                last_sequence: 0,
                terminal,
                receipt,
            });
        }
        Ok(runs)
    }

    /// Mark a run as durably started.
    #[allow(clippy::too_many_arguments)]
    pub fn record_run_started(
        &self,
        run_id: &str,
        stream_id: &str,
        session_id: &str,
        agent_handle_id: &str,
        agent_config_fingerprint: &str,
        cwd: Option<WirePath>,
        input_kind: &str,
    ) -> Result<()> {
        self.upsert_record(RunIndexRecord {
            run_id: run_id.to_string(),
            stream_id: stream_id.to_string(),
            session_id: session_id.to_string(),
            agent_handle_id: agent_handle_id.to_string(),
            status: "running".to_string(),
            last_sequence: 0,
            input_kind: input_kind.to_string(),
            agent_config_fingerprint: agent_config_fingerprint.to_string(),
            cwd,
            terminal: None,
            receipt: None,
        })
    }

    /// Mark a run as durably settled, storing the single authoritative
    /// terminal verbatim so restarts can answer `run/get` for historical
    /// runs without re-deriving framework state.
    pub fn record_run_settled(
        &self,
        run_id: &str,
        terminal: RunTerminal,
        receipt: RunReceiptWire,
        last_sequence: u64,
    ) -> Result<()> {
        terminal
            .validate()
            .map_err(|error| ReactError::Other(format!("invalid run terminal: {error}")))?;
        receipt
            .validate()
            .map_err(|error| ReactError::Other(format!("invalid run receipt: {error}")))?;
        let _guard = self
            .index_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut records = self.read_index()?;
        let record = records
            .iter_mut()
            .find(|record| record.run_id == run_id)
            .ok_or_else(|| {
                ReactError::Other(format!("run {run_id} is missing from durable index"))
            })?;
        record.status = "settled".to_string();
        record.last_sequence = last_sequence;
        record.terminal = Some(terminal);
        record.receipt = Some(receipt);
        self.write_index(records)
    }

    /// Remove a start record when preparing or spawning a run fails before the
    /// framework driver can own its lifecycle.
    pub fn remove_run(&self, run_id: &str) -> Result<()> {
        let _guard = self
            .index_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut records = self.read_index()?;
        records.retain(|record| record.run_id != run_id);
        self.write_index(records)
    }

    fn upsert_record(&self, record: RunIndexRecord) -> Result<()> {
        let _guard = self
            .index_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut records = self.read_index()?;
        records.retain(|existing| existing.run_id != record.run_id);
        records.push(record);
        self.write_index(records)
    }

    fn read_index(&self) -> Result<Vec<RunIndexRecord>> {
        let path = self.index_path();
        if let Ok(metadata) = std::fs::metadata(&path)
            && metadata.len() > MAX_RUN_INDEX_BYTES
        {
            return Err(ReactError::Other(format!(
                "run index exceeds the {MAX_RUN_INDEX_BYTES} byte bound"
            )));
        }
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|error| ReactError::Other(format!("run index is corrupt: {error}"))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(ReactError::Io(error)),
        }
    }

    fn write_index(&self, mut records: Vec<RunIndexRecord>) -> Result<()> {
        // Bound the index: prune the oldest settled entries first; a run
        // still marked `running` is never pruned (it must stay visible as
        // Interrupted after restart).
        while records.len() > MAX_RUN_INDEX_ENTRIES {
            let oldest_settled = records.iter().position(|record| record.status != "running");
            match oldest_settled {
                Some(position) => {
                    records.remove(position);
                }
                None => break,
            }
        }
        if records.len() > MAX_RUN_INDEX_ENTRIES {
            return Err(ReactError::Other(format!(
                "run index limit {MAX_RUN_INDEX_ENTRIES} reached by active runs"
            )));
        }
        let mut bytes = serde_json::to_vec_pretty(&records)
            .map_err(|error| ReactError::Other(format!("failed to encode run index: {error}")))?;
        while u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_RUN_INDEX_BYTES {
            let Some(position) = records.iter().position(|record| record.status != "running")
            else {
                return Err(ReactError::Other(format!(
                    "run index exceeds the {MAX_RUN_INDEX_BYTES} byte bound"
                )));
            };
            records.remove(position);
            bytes = serde_json::to_vec_pretty(&records).map_err(|error| {
                ReactError::Other(format!("failed to encode run index: {error}"))
            })?;
        }
        atomic_write(&self.index_path(), &bytes).map_err(ReactError::Io)
    }
}

fn sanitize(id: &str) -> String {
    let safe: String = id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect();
    let truncated: String = safe.chars().take(128).collect();
    if truncated.is_empty() {
        "unnamed".to_string()
    } else {
        truncated
    }
}

/// Read `{ generation: N }` and persist `N + 1` atomically. The counter is
/// the Host incarnation: every restart mints a new generation so pre-restart
/// handles fail as stale instead of rebinding to newer objects.
fn advance_generation(host_dir: &Path) -> Result<u64> {
    let path = host_dir.join("generation");
    let previous = match std::fs::read_to_string(&path) {
        Ok(content) => content
            .trim()
            .parse::<u64>()
            .map_err(|error| ReactError::Other(format!("generation file is corrupt: {error}")))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => return Err(ReactError::Io(error)),
    };
    let next = previous
        .checked_add(1)
        .ok_or_else(|| ReactError::Other("Host generation exhausted".to_string()))?;
    atomic_write(&path, next.to_string().as_bytes()).map_err(ReactError::Io)?;
    Ok(next)
}

/// Receipt facts for recovered runs. Only persisted receipts come back:
/// historical runs never receive synthesized counters.
pub(crate) fn recovered_receipt_wire(recovered: &RecoveredRun) -> Option<RunReceiptWire> {
    recovered.receipt.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_advances_monotonically_within_one_state_root() -> Result<()> {
        let directory = tempfile::tempdir().map_err(ReactError::Io)?;
        let host_dir = directory.path().join("host");
        std::fs::create_dir_all(&host_dir).map_err(ReactError::Io)?;
        let first = advance_generation(&host_dir)?;
        let second = advance_generation(&host_dir)?;
        assert_eq!(first, 1);
        assert_eq!(second, 2);
        Ok(())
    }

    #[test]
    fn interrupted_runs_are_never_reported_settled() -> Result<()> {
        let directory = tempfile::tempdir().map_err(ReactError::Io)?;
        let persistence = CorePersistence::open(directory.path())?;
        persistence.record_run_started(
            "run-1",
            "stream-1",
            "sess-1",
            "agent-1",
            "host_default",
            Some(WirePath::Utf8 {
                path: "/tmp".to_string(),
            }),
            "chat",
        )?;
        let recovered = persistence.load_recovered_runs()?;
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].status, RunStatus::Interrupted);
        assert!(recovered[0].terminal.is_none());
        Ok(())
    }

    #[test]
    fn run_ids_are_sanitized_into_journal_directories() {
        assert_eq!(sanitize("run/../etc"), "run____etc");
        assert_eq!(sanitize(""), "unnamed");
        let long: String = "a".repeat(300);
        assert!(sanitize(&long).chars().count() <= 128);
    }

    #[test]
    fn concurrent_settlement_updates_do_not_drop_run_index_entries() -> Result<()> {
        let directory = tempfile::tempdir().map_err(ReactError::Io)?;
        let persistence = Arc::new(CorePersistence::open(directory.path())?);
        for (run, stream) in [("run-a", "stream-a"), ("run-b", "stream-b")] {
            persistence.record_run_started(
                run,
                stream,
                "sess-1",
                "agent-1",
                "host_default",
                Some(WirePath::Utf8 {
                    path: "/tmp".to_string(),
                }),
                "chat",
            )?;
        }
        let first = persistence.clone();
        let second = persistence.clone();
        let left = std::thread::spawn(move || {
            first.record_run_settled(
                "run-a",
                RunTerminal::Cancelled,
                RunReceiptWire {
                    turn_id: "run-a".to_string(),
                    outcome: "cancelled".to_string(),
                    final_answer: None,
                    final_message_id: None,
                    prompt_tokens: echo_sdk_protocol::scalar::WireU64::from_u64(0),
                    completion_tokens: echo_sdk_protocol::scalar::WireU64::from_u64(0),
                    llm_calls: echo_sdk_protocol::scalar::WireU64::from_u64(0),
                    compaction_count: echo_sdk_protocol::scalar::WireU64::from_u64(0),
                    last_event_sequence: echo_sdk_protocol::scalar::WireU64::from_u64(1),
                    elapsed_ms: echo_sdk_protocol::scalar::WireU64::from_u64(1),
                },
                1,
            )
        });
        let right = std::thread::spawn(move || {
            second.record_run_settled(
                "run-b",
                RunTerminal::Cancelled,
                RunReceiptWire {
                    turn_id: "run-b".to_string(),
                    outcome: "cancelled".to_string(),
                    final_answer: None,
                    final_message_id: None,
                    prompt_tokens: echo_sdk_protocol::scalar::WireU64::from_u64(0),
                    completion_tokens: echo_sdk_protocol::scalar::WireU64::from_u64(0),
                    llm_calls: echo_sdk_protocol::scalar::WireU64::from_u64(0),
                    compaction_count: echo_sdk_protocol::scalar::WireU64::from_u64(0),
                    last_event_sequence: echo_sdk_protocol::scalar::WireU64::from_u64(1),
                    elapsed_ms: echo_sdk_protocol::scalar::WireU64::from_u64(1),
                },
                1,
            )
        });
        left.join()
            .map_err(|_| ReactError::Other("left settlement panicked".to_string()))??;
        right
            .join()
            .map_err(|_| ReactError::Other("right settlement panicked".to_string()))??;
        let recovered = persistence.load_recovered_runs()?;
        assert_eq!(recovered.len(), 2);
        assert!(recovered.iter().all(|run| run.receipt.is_some()));
        Ok(())
    }
}
