//! Durable, forward-recoverable boundary between layered memory and its audit.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use echo_core::error::ReactError;
use echo_core::utils::fs::FileDurability;
use echo_state::journal::file::FileEventJournal;
use echo_state::journal::{EventJournal, JournalDurabilityStatus, PreparedJournalBatch};
use serde::{Deserialize, Serialize};

use super::audit::ChangeEntry;
use super::layer::HotEntryMeta;

type Result<T> = std::result::Result<T, ReactError>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct HotValue {
    pub meta: HotEntryMeta,
    pub content: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct MemoryOperation {
    pub id: String,
    pub key: String,
    pub warm_before: Option<serde_json::Value>,
    pub warm_after: Option<serde_json::Value>,
    pub hot_before: Option<HotValue>,
    pub hot_after: Option<HotValue>,
    pub audit: ChangeEntry,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct MemoryOperationBatch {
    pub id: String,
    pub operations: Vec<MemoryOperation>,
    /// Forward/rollback lineage is private journal metadata. `None` is the
    /// legacy forward form so old journal records remain decodable.
    #[serde(default)]
    pub origin: Option<MemoryOperationOrigin>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct MemoryOperationOrigin {
    pub request_id: String,
    pub target_batch_id: String,
    pub target_generation: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
enum MemoryOperationEvent {
    Prepared(MemoryOperationBatch),
    Settled { id: String },
}

#[derive(Clone, Debug)]
pub(super) struct MemoryOperationHistory {
    pub batch: MemoryOperationBatch,
    pub settled: bool,
    pub generation: u64,
}

pub(super) struct MemoryOperationJournal {
    journal: Arc<FileEventJournal<MemoryOperationEvent>>,
    serial: Arc<tokio::sync::Mutex<()>>,
}

impl MemoryOperationJournal {
    pub fn open(root: &Path) -> Result<Self> {
        let path = root.join("evolution").join("memory-operations.jsonl");
        let journal = Arc::new(FileEventJournal::open(path, FileDurability::SyncData)?);
        static SERIALS: OnceLock<Mutex<HashMap<PathBuf, Weak<tokio::sync::Mutex<()>>>>> =
            OnceLock::new();
        let mut serials = SERIALS
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .map_err(|error| {
                ReactError::Other(format!(
                    "memory operation lock registry is poisoned: {error}"
                ))
            })?;
        let serial = match serials.get(journal.path()).and_then(Weak::upgrade) {
            Some(existing) => existing,
            None => {
                let created = Arc::new(tokio::sync::Mutex::new(()));
                serials.insert(journal.path().to_path_buf(), Arc::downgrade(&created));
                created
            }
        };
        Ok(Self { journal, serial })
    }

    pub fn serial(&self) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(&self.serial)
    }

    fn append_confirmed(&self, event: MemoryOperationEvent) -> Result<()> {
        let batch = PreparedJournalBatch::new(vec![event])
            .map_err(|error| ReactError::Other(error.to_string()))?;
        let receipt = self
            .journal
            .append_batch(batch)
            .map_err(|error| ReactError::Other(error.to_string()))?;
        match receipt.durability() {
            JournalDurabilityStatus::Confirmed => Ok(()),
            JournalDurabilityStatus::Degraded { .. } => self.journal.sync_data(),
            JournalDurabilityStatus::Unconfirmed => Err(ReactError::Other(
                "memory operation journal append has unconfirmed durability".into(),
            )),
        }
    }

    pub fn prepare(&self, operation: MemoryOperation) -> Result<()> {
        self.prepare_batch(MemoryOperationBatch {
            id: operation.id.clone(),
            operations: vec![operation],
            origin: None,
        })
    }

    pub fn prepare_batch(&self, batch: MemoryOperationBatch) -> Result<()> {
        if batch.id.is_empty() || batch.operations.is_empty() {
            return Err(ReactError::Other(
                "memory operation batch must not be empty".into(),
            ));
        }
        let mut keys = std::collections::HashSet::new();
        let mut ids = std::collections::HashSet::new();
        for operation in &batch.operations {
            if operation.id.is_empty()
                || operation.id != operation.audit.change_id
                || !keys.insert(operation.key.as_str())
                || !ids.insert(operation.id.as_str())
            {
                return Err(ReactError::Other(
                    "memory operation batch has duplicate keys or invalid audit identity".into(),
                ));
            }
        }
        self.append_confirmed(MemoryOperationEvent::Prepared(batch))
    }

    pub fn settle(&self, id: &str) -> Result<()> {
        self.append_confirmed(MemoryOperationEvent::Settled { id: id.to_owned() })
    }

    pub fn history(&self) -> Result<Vec<(MemoryOperationBatch, bool)>> {
        Ok(self
            .history_with_generations()?
            .into_iter()
            .map(|item| (item.batch, item.settled))
            .collect())
    }

    pub fn history_with_generations(&self) -> Result<Vec<MemoryOperationHistory>> {
        self.journal.sync_data()?;
        let mut identities = HashMap::<String, usize>::new();
        let mut known_ids = HashSet::<String>::new();
        let mut history = Vec::<MemoryOperationHistory>::new();
        let mut sequence = 0_u64;
        loop {
            let records = self.journal.replay_after(sequence, 512)?;
            if records.is_empty() {
                break;
            }
            for record in &records {
                match record.event.as_ref() {
                    MemoryOperationEvent::Prepared(batch) => {
                        let index = history.len();
                        if !known_ids.insert(batch.id.clone()) {
                            return Err(ReactError::Other(format!(
                                "memory operation {} has duplicate prepare facts",
                                batch.id
                            )));
                        }
                        identities.insert(batch.id.clone(), index);
                        history.push(MemoryOperationHistory {
                            batch: batch.clone(),
                            settled: false,
                            generation: record.sequence,
                        });
                    }
                    MemoryOperationEvent::Settled { id } => {
                        let Some(index) = identities.remove(id) else {
                            return Err(ReactError::Other(format!(
                                "memory operation {id} has no pending prepare fact"
                            )));
                        };
                        let Some(item) = history.get_mut(index) else {
                            return Err(ReactError::Other(format!(
                                "memory operation {id} history index is invalid"
                            )));
                        };
                        item.settled = true;
                    }
                }
                sequence = record.sequence;
            }
        }
        Ok(history)
    }
}

#[cfg(test)]
mod tests {
    use super::MemoryOperationBatch;

    #[test]
    fn legacy_batch_without_lineage_decodes_as_forward_history() -> Result<(), serde_json::Error> {
        let legacy = serde_json::json!({
            "id": "legacy-batch",
            "operations": []
        });
        let decoded = serde_json::from_value::<MemoryOperationBatch>(legacy)?;
        assert!(decoded.origin.is_none());
        Ok(())
    }
}
