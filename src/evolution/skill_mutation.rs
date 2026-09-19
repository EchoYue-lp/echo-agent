//! Canonical durable mutation owner for Skill lifecycle resources.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use chrono::{DateTime, Utc};
use echo_core::error::{MemoryError, ReactError};
use echo_core::utils::fs::FileDurability;
use echo_state::journal::file::FileEventJournal;
use echo_state::journal::{EventJournal, JournalDurabilityStatus, PreparedJournalBatch};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::audit::{
    ChangeEntry, ChangeEntryBuilder, ChangeFilter, ChangeLog, ChangeType, EntityType,
};
use super::curator::{Curator, CuratorState};
use super::security::{PromptInjectionDetector, SecretScanner};
use crate::error::Result;

const SKILL_AUTHORITY_AUDIT_KEY: &str = "__skill_lifecycle_authority_binding__";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct SkillAuditBinding {
    authority_id: String,
    destination_identity: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillMutationKind {
    Draft,
    Promote,
    Touch,
    Deprecate,
    Merge,
    Patch,
    Rollback,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SkillFileMutation {
    pub path: PathBuf,
    pub before: Option<Vec<u8>>,
    pub after: Option<Vec<u8>>,
}

impl SkillFileMutation {
    pub fn canonical_path(path: impl AsRef<Path>) -> Result<PathBuf> {
        canonical_resource_path(path.as_ref())
    }

    pub fn new(
        path: impl AsRef<Path>,
        before: Option<Vec<u8>>,
        after: Option<Vec<u8>>,
    ) -> Result<Self> {
        Ok(Self {
            path: Self::canonical_path(path)?,
            before,
            after,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SkillMutationRequest {
    pub request_id: String,
    pub entity_key: String,
    pub kind: SkillMutationKind,
    pub reason: String,
    pub files: Vec<SkillFileMutation>,
    pub curator_before: CuratorState,
    pub curator_after: CuratorState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollback_of: Option<SkillRollbackLineage>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillRollbackLineage {
    pub target_batch_id: String,
    pub target_change_id: Option<String>,
    pub target_change_ids: Vec<String>,
    pub target_generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillApprovalArtifact {
    pub approval_id: String,
    pub operation_digest: String,
    pub approver: String,
    pub approved_at: DateTime<Utc>,
}

impl SkillApprovalArtifact {
    pub fn new(
        approval_id: impl Into<String>,
        operation_digest: impl Into<String>,
        approver: impl Into<String>,
        approved_at: DateTime<Utc>,
    ) -> Self {
        Self {
            approval_id: approval_id.into(),
            operation_digest: operation_digest.into(),
            approver: approver.into(),
            approved_at,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillMutationPreview {
    pub request_id: String,
    pub operation_digest: String,
    pub affected_paths: Vec<PathBuf>,
    pub affected_skills: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillMutationReceipt {
    pub request_id: String,
    pub batch_id: String,
    pub generation: u64,
    pub change_ids: Vec<String>,
    pub approval_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkillMutationOutcome {
    Applied(SkillMutationReceipt),
    AlreadyApplied(SkillMutationReceipt),
    Conflict { reason: String },
    HistoryUnavailable { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkillRollbackTarget {
    ChangeId(String),
    BatchId(String),
    Rule(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkillRollbackSupport {
    Supported,
    HostOwned { target: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkillRollbackPreviewOutcome {
    Ready(SkillMutationPreview),
    Conflict { reason: String },
    HistoryUnavailable { reason: String },
    HostOwned { target: String },
}

pub trait SkillMutationObserver: Send + Sync {
    fn on_skill_mutation(&self, receipt: &SkillMutationReceipt);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkillUsageOutcome {
    Updated,
    UnknownSkill,
}

/// Narrow runtime handle for recording usage of an existing Skill.
/// It cannot create, promote, or otherwise change lifecycle state.
#[derive(Clone)]
pub struct SkillUsageHandle {
    authority: Arc<SkillMutationAuthority>,
}

impl SkillUsageHandle {
    pub fn new(authority: Arc<SkillMutationAuthority>) -> Self {
        Self { authority }
    }

    pub async fn record_usage(&self, skill_name: &str) -> Result<SkillUsageOutcome> {
        let mut last_error = None;
        for _ in 0..3 {
            let before = self.authority.curator.load_state()?;
            let Some(_) = before.skills.get(skill_name) else {
                return Ok(SkillUsageOutcome::UnknownSkill);
            };
            let mut after = before.clone();
            let meta = after.skills.get_mut(skill_name).ok_or_else(|| {
                ReactError::Other("skill disappeared while planning usage mutation".into())
            })?;
            meta.last_used_at = Utc::now();
            let request = SkillMutationRequest {
                request_id: uuid::Uuid::new_v4().to_string(),
                entity_key: skill_name.to_owned(),
                kind: SkillMutationKind::Touch,
                reason: "runtime skill usage".into(),
                files: Vec::new(),
                curator_before: before,
                curator_after: after,
                rollback_of: None,
            };
            let preview = match self.authority.preview(&request) {
                Ok(preview) => preview,
                Err(error) => {
                    last_error = Some(error);
                    continue;
                }
            };
            let approval = SkillApprovalArtifact::new(
                uuid::Uuid::new_v4().to_string(),
                &preview.operation_digest,
                "framework:skill-usage",
                Utc::now(),
            );
            match self.authority.apply(request, approval).await {
                Ok(SkillMutationOutcome::Applied(_))
                | Ok(SkillMutationOutcome::AlreadyApplied(_)) => {
                    return Ok(SkillUsageOutcome::Updated);
                }
                Ok(SkillMutationOutcome::Conflict { reason }) => {
                    last_error = Some(ReactError::Other(reason));
                    continue;
                }
                Ok(outcome) => {
                    return Err(ReactError::Other(format!(
                        "skill usage mutation did not settle: {outcome:?}"
                    )));
                }
                Err(error) => return Err(error),
            }
        }
        Err(last_error
            .unwrap_or_else(|| ReactError::Other("skill usage mutation exhausted retries".into())))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SkillOperationBatch {
    id: String,
    request: SkillMutationRequest,
    approval: SkillApprovalArtifact,
    audits: Vec<ChangeEntry>,
    #[serde(default)]
    rollback_of: Option<SkillRollbackLineage>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
enum SkillOperationEvent {
    Bound { audit_authority_id: String },
    Prepared(Box<SkillOperationBatch>),
    Settled { id: String },
}

#[derive(Clone)]
struct SkillHistoryItem {
    batch: SkillOperationBatch,
    settled: bool,
    generation: u64,
}

struct SkillJournalHistory {
    audit_authority_id: Option<String>,
    operations: Vec<SkillHistoryItem>,
}

struct SkillOperationJournal {
    journal: Arc<FileEventJournal<SkillOperationEvent>>,
    serial: Arc<tokio::sync::Mutex<()>>,
}

impl SkillOperationJournal {
    fn open(path: &Path) -> Result<Self> {
        let journal = Arc::new(FileEventJournal::open(path, FileDurability::SyncData)?);
        static SERIALS: OnceLock<Mutex<HashMap<PathBuf, Weak<tokio::sync::Mutex<()>>>>> =
            OnceLock::new();
        let mut serials = SERIALS
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .map_err(|error| ReactError::Other(format!("skill journal lock poisoned: {error}")))?;
        let serial = serials
            .get(journal.path())
            .and_then(Weak::upgrade)
            .unwrap_or_else(|| {
                let created = Arc::new(tokio::sync::Mutex::new(()));
                serials.insert(journal.path().to_path_buf(), Arc::downgrade(&created));
                created
            });
        Ok(Self { journal, serial })
    }

    fn append(&self, event: SkillOperationEvent) -> Result<()> {
        let receipt = self
            .journal
            .append_batch(
                PreparedJournalBatch::new(vec![event])
                    .map_err(|error| ReactError::Other(error.to_string()))?,
            )
            .map_err(|error| ReactError::Other(error.to_string()))?;
        match receipt.durability() {
            JournalDurabilityStatus::Confirmed => Ok(()),
            JournalDurabilityStatus::Degraded { .. } => self.journal.sync_data(),
            JournalDurabilityStatus::Unconfirmed => Err(ReactError::Other(
                "skill operation journal durability is unconfirmed".into(),
            )),
        }
    }

    fn prepare(&self, batch: SkillOperationBatch) -> Result<()> {
        self.append(SkillOperationEvent::Prepared(Box::new(batch)))
    }

    fn settle(&self, id: &str) -> Result<()> {
        self.append(SkillOperationEvent::Settled { id: id.to_owned() })
    }

    fn bind(&self, audit_authority_id: &str) -> Result<()> {
        if audit_authority_id.is_empty() {
            return Err(ReactError::Other(
                "skill audit authority identity must not be empty".into(),
            ));
        }
        let history = self.history_state()?;
        match history.audit_authority_id {
            Some(existing) if existing == audit_authority_id => Ok(()),
            Some(existing) => Err(ReactError::Other(format!(
                "skill journal is bound to audit authority {existing}, not {audit_authority_id}"
            ))),
            None if history.operations.is_empty() => self.append(SkillOperationEvent::Bound {
                audit_authority_id: audit_authority_id.to_owned(),
            }),
            None => Err(ReactError::Other(
                "skill journal has operations without an audit authority binding".into(),
            )),
        }
    }

    fn history(&self) -> Result<Vec<SkillHistoryItem>> {
        let history = self.history_state()?;
        if history.audit_authority_id.is_none() && !history.operations.is_empty() {
            return Err(ReactError::Other(
                "skill journal operation history is unbound".into(),
            ));
        }
        Ok(history.operations)
    }

    fn history_state(&self) -> Result<SkillJournalHistory> {
        self.journal.sync_data()?;
        let mut audit_authority_id = None;
        let mut pending = HashMap::<String, usize>::new();
        let mut known = HashSet::<String>::new();
        let mut history = Vec::<SkillHistoryItem>::new();
        let mut sequence = 0_u64;
        loop {
            let records = self.journal.replay_after(sequence, 512)?;
            if records.is_empty() {
                break;
            }
            for record in records {
                match record.event.as_ref() {
                    SkillOperationEvent::Bound {
                        audit_authority_id: observed,
                    } => {
                        if observed.is_empty()
                            || audit_authority_id.is_some()
                            || !history.is_empty()
                        {
                            return Err(ReactError::Other(
                                "skill journal has an invalid audit authority binding".into(),
                            ));
                        }
                        audit_authority_id = Some(observed.clone());
                    }
                    SkillOperationEvent::Prepared(batch) => {
                        if audit_authority_id.is_none() {
                            return Err(ReactError::Other(
                                "skill prepare precedes audit authority binding".into(),
                            ));
                        }
                        if !known.insert(batch.id.clone()) {
                            return Err(ReactError::Other(format!(
                                "duplicate skill operation batch {}",
                                batch.id
                            )));
                        }
                        let index = history.len();
                        pending.insert(batch.id.clone(), index);
                        history.push(SkillHistoryItem {
                            batch: batch.as_ref().clone(),
                            settled: false,
                            generation: record.sequence,
                        });
                    }
                    SkillOperationEvent::Settled { id } => {
                        let index = pending.remove(id).ok_or_else(|| {
                            ReactError::Other(format!("skill settlement {id} has no prepare"))
                        })?;
                        let item = history.get_mut(index).ok_or_else(|| {
                            ReactError::Other(format!("skill history index missing for {id}"))
                        })?;
                        item.settled = true;
                    }
                }
                sequence = record.sequence;
            }
        }
        Ok(SkillJournalHistory {
            audit_authority_id,
            operations: history,
        })
    }
}

pub struct SkillMutationAuthority {
    curator: Curator,
    journal: SkillOperationJournal,
    change_log: Arc<dyn ChangeLog>,
    audit_authority_id: String,
    scanner: SecretScanner,
    injector: PromptInjectionDetector,
    observer: Option<Arc<dyn SkillMutationObserver>>,
    #[cfg(test)]
    fail_point: Mutex<Option<SkillMutationFailPoint>>,
    #[cfg(test)]
    after_files_hook: Option<Arc<dyn Fn() -> Result<()> + Send + Sync>>,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SkillMutationFailPoint {
    Prepared,
    FilesProjected,
    CuratorProjected,
    AuditRecorded,
}

impl SkillMutationAuthority {
    pub fn open(curator: Curator, change_log: Arc<dyn ChangeLog>) -> Result<Self> {
        let journal = SkillOperationJournal::open(&curator.skill_operation_journal_path())?;
        let _serial = journal.serial.try_lock().map_err(|_| {
            ReactError::Other("skill authority is busy during destination binding".into())
        })?;
        let history = journal.history_state()?;
        let destination_identity = change_log.durable_destination_identity()?.ok_or_else(|| {
            ReactError::from(MemoryError::Unsupported(
                "Skill mutation authority requires a durable ChangeLog destination identity".into(),
            ))
        })?;
        let audit_marker = Self::read_audit_binding(change_log.as_ref())?;
        if let Some(marker) = audit_marker.as_ref()
            && marker.destination_identity != destination_identity
        {
            return Err(ReactError::Other(format!(
                "Skill ChangeLog marker destination {:?} does not match actual destination {:?}",
                marker.destination_identity, destination_identity
            )));
        }
        let audit_authority_id = match (history.audit_authority_id, audit_marker) {
            (Some(journal_id), Some(binding)) if journal_id == binding.authority_id => journal_id,
            (Some(journal_id), Some(binding)) => {
                return Err(ReactError::Other(format!(
                    "skill journal authority {journal_id} does not match ChangeLog authority {}",
                    binding.authority_id
                )));
            }
            (Some(_), None) => {
                return Err(ReactError::Other(
                    "skill journal is bound but ChangeLog has no matching authority marker".into(),
                ));
            }
            (None, Some(binding)) if history.operations.is_empty() => {
                journal.bind(&binding.authority_id)?;
                binding.authority_id
            }
            (None, Some(_)) => {
                return Err(ReactError::Other(
                    "unbound skill journal already contains operations".into(),
                ));
            }
            (None, None) if history.operations.is_empty() && change_log.is_empty() => {
                let created = uuid::Uuid::new_v4().to_string();
                let binding = SkillAuditBinding {
                    authority_id: created.clone(),
                    destination_identity,
                };
                let marker = ChangeEntryBuilder::new(
                    EntityType::Skill,
                    SKILL_AUTHORITY_AUDIT_KEY,
                    ChangeType::Create,
                )
                .after(serde_json::to_value(&binding)?)
                .reason("bind Skill lifecycle journal to one ChangeLog authority")
                .trigger("skill_mutation_authority_binding")
                .build_with(format!("skill_authority_{created}"), Utc::now());
                change_log.record_idempotent(marker)?;
                if Self::read_audit_binding(change_log.as_ref())?.as_ref() != Some(&binding) {
                    return Err(ReactError::Other(
                        "Skill ChangeLog authority marker is not durably observable".into(),
                    ));
                }
                if change_log.durable_destination_identity()?.as_ref()
                    != Some(&binding.destination_identity)
                {
                    return Err(ReactError::Other(
                        "Skill ChangeLog destination changed during authority binding".into(),
                    ));
                }
                journal.bind(&created)?;
                created
            }
            (None, None) => {
                return Err(ReactError::Other(
                    "unbound Skill authority has existing journal operations or ChangeLog records"
                        .into(),
                ));
            }
        };
        drop(_serial);
        Ok(Self {
            curator,
            journal,
            change_log,
            audit_authority_id,
            scanner: SecretScanner::new(),
            injector: PromptInjectionDetector,
            observer: None,
            #[cfg(test)]
            fail_point: Mutex::new(None),
            #[cfg(test)]
            after_files_hook: None,
        })
    }

    pub fn with_observer(mut self, observer: Arc<dyn SkillMutationObserver>) -> Self {
        self.observer = Some(observer);
        self
    }

    #[cfg(test)]
    fn with_fail_point(self, point: SkillMutationFailPoint) -> Self {
        if let Ok(mut configured) = self.fail_point.lock() {
            *configured = Some(point);
        }
        self
    }

    #[cfg(test)]
    fn with_after_files_hook(mut self, hook: Arc<dyn Fn() -> Result<()> + Send + Sync>) -> Self {
        self.after_files_hook = Some(hook);
        self
    }

    #[cfg(test)]
    fn fail_if(&self, point: SkillMutationFailPoint) -> Result<()> {
        let mut configured = self.fail_point.lock().map_err(|error| {
            ReactError::Other(format!("skill failpoint lock poisoned: {error}"))
        })?;
        if configured.as_ref() == Some(&point) {
            *configured = None;
            return Err(ReactError::Other(format!(
                "injected skill mutation failure at {point:?}"
            )));
        }
        Ok(())
    }

    pub fn curator(&self) -> &Curator {
        &self.curator
    }

    pub fn audit_authority_id(&self) -> &str {
        &self.audit_authority_id
    }

    fn read_audit_binding(change_log: &dyn ChangeLog) -> Result<Option<SkillAuditBinding>> {
        let markers = change_log
            .query(&ChangeFilter::new().with_key_prefix(SKILL_AUTHORITY_AUDIT_KEY))?
            .into_iter()
            .filter(|entry| {
                entry.entity_type == EntityType::Skill
                    && entry.entity_key == SKILL_AUTHORITY_AUDIT_KEY
            })
            .collect::<Vec<_>>();
        match markers.as_slice() {
            [] => Ok(None),
            [marker] if marker.change_type == ChangeType::Create => {
                let binding = marker
                    .after
                    .as_ref()
                    .cloned()
                    .ok_or_else(|| {
                        ReactError::Other("Skill ChangeLog authority marker is invalid".into())
                    })
                    .and_then(|value| {
                        serde_json::from_value::<SkillAuditBinding>(value).map_err(|error| {
                            ReactError::Other(format!(
                                "Skill ChangeLog authority marker is invalid: {error}"
                            ))
                        })
                    })?;
                if binding.authority_id.is_empty() || binding.destination_identity.is_empty() {
                    return Err(ReactError::Other(
                        "Skill ChangeLog authority marker is invalid".into(),
                    ));
                }
                Ok(Some(binding))
            }
            _ => Err(ReactError::Other(
                "Skill ChangeLog has duplicate or invalid authority markers".into(),
            )),
        }
    }

    pub fn rule_rollback_support(target: impl Into<String>) -> SkillRollbackSupport {
        SkillRollbackSupport::HostOwned {
            target: target.into(),
        }
    }

    pub async fn preview_rollback(
        &self,
        request_id: impl Into<String>,
        target: &SkillRollbackTarget,
    ) -> Result<SkillRollbackPreviewOutcome> {
        let _serial = self.journal.serial.lock().await;
        self.reconcile_locked()?;
        let request_id = request_id.into();
        if let SkillRollbackTarget::Rule(target) = target {
            return Ok(SkillRollbackPreviewOutcome::HostOwned {
                target: target.clone(),
            });
        }
        let history = self.journal.history()?;
        let Some(target_item) = resolve_target(target, &history, self.change_log.as_ref())? else {
            return Ok(SkillRollbackPreviewOutcome::HistoryUnavailable {
                reason: "rollback target is not present in retained skill history".into(),
            });
        };
        if !target_item.settled {
            return Ok(SkillRollbackPreviewOutcome::HistoryUnavailable {
                reason: "rollback target is not settled".into(),
            });
        }
        if let Some(reason) = non_tip_reason(target_item, &history) {
            return Ok(SkillRollbackPreviewOutcome::Conflict { reason });
        }
        let request = inverse_request(request_id, target_item, target_change_id(target));
        Ok(SkillRollbackPreviewOutcome::Ready(self.preview(&request)?))
    }

    pub async fn rollback(
        &self,
        request_id: impl Into<String>,
        target: SkillRollbackTarget,
        approval: SkillApprovalArtifact,
    ) -> Result<SkillMutationOutcome> {
        let _serial = self.journal.serial.lock().await;
        self.reconcile_locked()?;
        let request_id = request_id.into();
        let history = self.journal.history()?;
        if let Some(existing) = history
            .iter()
            .find(|item| item.batch.request.request_id == request_id)
        {
            let requested_target = match &target {
                SkillRollbackTarget::BatchId(batch_id) => Some(batch_id.clone()),
                SkillRollbackTarget::ChangeId(_) => {
                    resolve_target(&target, &history, self.change_log.as_ref())?
                        .map(|item| item.batch.id.clone())
                }
                SkillRollbackTarget::Rule(_) => None,
            };
            if existing
                .batch
                .rollback_of
                .as_ref()
                .map(|lineage| lineage.target_batch_id.as_str())
                == requested_target.as_deref()
            {
                return Ok(SkillMutationOutcome::AlreadyApplied(receipt(existing)));
            }
            return Ok(SkillMutationOutcome::Conflict {
                reason: "rollback request ID was reused for a different target".into(),
            });
        }
        let Some(target_item) = resolve_target(&target, &history, self.change_log.as_ref())? else {
            return match target {
                SkillRollbackTarget::Rule(target) => Ok(SkillMutationOutcome::Conflict {
                    reason: format!("rule rollback is host-owned: {target}"),
                }),
                _ => Ok(SkillMutationOutcome::HistoryUnavailable {
                    reason: "rollback target is not present in retained skill history".into(),
                }),
            };
        };
        if let Some(reason) = non_tip_reason(target_item, &history) {
            return Ok(SkillMutationOutcome::Conflict { reason });
        }
        self.apply_locked(
            inverse_request(request_id, target_item, target_change_id(&target)),
            approval,
        )
    }

    pub fn digest(request: &SkillMutationRequest) -> Result<String> {
        let bytes = serde_json::to_vec(request)?;
        Ok(hex_digest(&bytes))
    }

    pub fn preview(&self, request: &SkillMutationRequest) -> Result<SkillMutationPreview> {
        self.validate_request(request)?;
        self.validate_current(request)?;
        Ok(SkillMutationPreview {
            request_id: request.request_id.clone(),
            operation_digest: Self::digest(request)?,
            affected_paths: request.files.iter().map(|file| file.path.clone()).collect(),
            affected_skills: changed_skill_names(request),
        })
    }

    pub async fn apply(
        &self,
        request: SkillMutationRequest,
        approval: SkillApprovalArtifact,
    ) -> Result<SkillMutationOutcome> {
        let _serial = self.journal.serial.lock().await;
        self.apply_locked(request, approval)
    }

    fn apply_locked(
        &self,
        request: SkillMutationRequest,
        approval: SkillApprovalArtifact,
    ) -> Result<SkillMutationOutcome> {
        self.reconcile_locked()?;
        let history = self.journal.history()?;
        if let Some(existing) = history
            .iter()
            .find(|item| item.batch.request.request_id == request.request_id)
        {
            if existing.batch.request != request || existing.batch.approval != approval {
                return Ok(SkillMutationOutcome::Conflict {
                    reason: "request ID was reused with different content or approval".into(),
                });
            }
            return Ok(SkillMutationOutcome::AlreadyApplied(receipt(existing)));
        }
        if history.iter().any(|item| {
            item.batch.approval.approval_id == approval.approval_id
                && item.batch.request.request_id != request.request_id
        }) {
            return Ok(SkillMutationOutcome::Conflict {
                reason: "approval artifact was already consumed by another request".into(),
            });
        }
        let preview = match self.preview(&request) {
            Ok(preview) => preview,
            Err(error) => {
                return Ok(SkillMutationOutcome::Conflict {
                    reason: error.to_string(),
                });
            }
        };
        validate_approval(&approval, &preview)?;
        let batch_id = uuid::Uuid::new_v4().to_string();
        let audits = build_audits(&batch_id, &request, &approval);
        let rollback_of = request.rollback_of.clone();
        let batch = SkillOperationBatch {
            id: batch_id,
            request,
            approval,
            audits,
            rollback_of,
        };
        self.journal.prepare(batch.clone())?;
        #[cfg(test)]
        self.fail_if(SkillMutationFailPoint::Prepared)?;
        self.apply_batch(&batch)?;
        let item = self
            .journal
            .history()?
            .into_iter()
            .find(|item| item.batch.id == batch.id)
            .ok_or_else(|| ReactError::Other("skill receipt batch disappeared".into()))?;
        let receipt = receipt(&item);
        if let Some(observer) = &self.observer {
            observer.on_skill_mutation(&receipt);
        }
        Ok(SkillMutationOutcome::Applied(receipt))
    }

    pub fn reconcile(&self) -> Result<()> {
        let guard = self.journal.serial.try_lock().map_err(|_| {
            ReactError::Other("skill mutation is active; retry reconciliation".into())
        })?;
        let result = self.reconcile_locked();
        drop(guard);
        result
    }

    fn reconcile_locked(&self) -> Result<()> {
        for item in self.journal.history()? {
            if !item.settled {
                self.apply_batch(&item.batch)?;
            }
        }
        Ok(())
    }

    fn apply_batch(&self, batch: &SkillOperationBatch) -> Result<()> {
        self.validate_current_or_after(&batch.request)?;
        for file in &batch.request.files {
            let current = read_optional(&file.path)?;
            if current == file.after {
                continue;
            }
            if current != file.before {
                return Err(ReactError::Other(format!(
                    "skill file {} changed after prepare",
                    file.path.display()
                )));
            }
            write_optional(&file.path, file.after.as_deref())?;
        }
        #[cfg(test)]
        {
            if let Some(hook) = &self.after_files_hook {
                hook()?;
            }
            self.fail_if(SkillMutationFailPoint::FilesProjected)?;
        }
        let affected_names = changed_skill_names(&batch.request);
        self.curator.replace_skill_entries_if_current(
            &batch.request.curator_before,
            &batch.request.curator_after,
            &affected_names,
        )?;
        #[cfg(test)]
        self.fail_if(SkillMutationFailPoint::CuratorProjected)?;
        for audit in &batch.audits {
            self.change_log.record_idempotent(audit.clone())?;
        }
        #[cfg(test)]
        self.fail_if(SkillMutationFailPoint::AuditRecorded)?;
        self.journal.settle(&batch.id)
    }

    fn validate_request(&self, request: &SkillMutationRequest) -> Result<()> {
        if request.request_id.is_empty()
            || request.entity_key.is_empty()
            || request.reason.is_empty()
        {
            return Err(ReactError::Other(
                "skill mutation identity, entity key, and reason are required".into(),
            ));
        }
        if request.entity_key == SKILL_AUTHORITY_AUDIT_KEY {
            return Err(ReactError::Other(
                "skill mutation entity key is reserved for authority binding".into(),
            ));
        }
        let mut paths = HashSet::new();
        for file in &request.files {
            let canonical = canonical_resource_path(&file.path)?;
            if canonical != file.path {
                return Err(ReactError::Other(format!(
                    "skill resource path is not canonical: {}",
                    file.path.display()
                )));
            }
            if !paths.insert(canonical) || file.before == file.after {
                return Err(ReactError::Other(
                    "skill mutation has duplicate or unchanged file projections".into(),
                ));
            }
            if let Some(after) = &file.after {
                let content = std::str::from_utf8(after).map_err(|error| {
                    ReactError::Other(format!(
                        "SKILL.md projection is not valid UTF-8 at {}: {error}",
                        file.path.display()
                    ))
                })?;
                if request.kind != SkillMutationKind::Rollback
                    && (self.injector.detect(content) || self.scanner.contains_secrets(content))
                {
                    return Err(ReactError::Other(
                        "skill security check rejected content before prepare".into(),
                    ));
                }
            }
        }
        if request.files.is_empty() && request.curator_before == request.curator_after {
            return Err(ReactError::Other("skill mutation has no effect".into()));
        }
        if request.curator_before.last_run_at != request.curator_after.last_run_at {
            return Err(ReactError::Other(
                "skill lifecycle request cannot mutate Curator run metadata".into(),
            ));
        }
        if request.kind != SkillMutationKind::Rollback {
            for name in changed_skill_names(request) {
                if let Some(path) = request
                    .curator_after
                    .skills
                    .get(&name)
                    .and_then(|meta| meta.path.as_ref())
                    && SkillFileMutation::canonical_path(path)? != *path
                {
                    return Err(ReactError::Other(format!(
                        "skill lifecycle {name} has a non-canonical path"
                    )));
                }
            }
        }
        Ok(())
    }

    fn validate_current(&self, request: &SkillMutationRequest) -> Result<()> {
        for file in &request.files {
            if read_optional(&file.path)? != file.before {
                return Err(ReactError::Other(format!(
                    "skill file {} does not match reviewed bytes",
                    file.path.display()
                )));
            }
        }
        let current = self.curator.load_state()?;
        for name in changed_skill_names(request) {
            if current.skills.get(&name) != request.curator_before.skills.get(&name) {
                return Err(ReactError::Other(format!(
                    "skill lifecycle {name} does not match reviewed generation"
                )));
            }
        }
        Ok(())
    }

    fn validate_current_or_after(&self, request: &SkillMutationRequest) -> Result<()> {
        for file in &request.files {
            let current = read_optional(&file.path)?;
            if current != file.before && current != file.after {
                return Err(ReactError::Other(format!(
                    "skill file {} has an external edit",
                    file.path.display()
                )));
            }
        }
        let state = self.curator.load_state()?;
        for name in changed_skill_names(request) {
            let current = state.skills.get(&name);
            let before = request.curator_before.skills.get(&name);
            let after = request.curator_after.skills.get(&name);
            if current != before && current != after {
                return Err(ReactError::Other(format!(
                    "skill lifecycle {name} has an external edit"
                )));
            }
        }
        Ok(())
    }
}

fn validate_approval(
    approval: &SkillApprovalArtifact,
    preview: &SkillMutationPreview,
) -> Result<()> {
    if approval.approval_id.is_empty()
        || approval.approver.is_empty()
        || approval.operation_digest != preview.operation_digest
    {
        return Err(ReactError::Other(
            "skill approval is missing or does not match the exact mutation digest".into(),
        ));
    }
    Ok(())
}

fn resolve_target<'a>(
    target: &SkillRollbackTarget,
    history: &'a [SkillHistoryItem],
    change_log: &dyn ChangeLog,
) -> Result<Option<&'a SkillHistoryItem>> {
    match target {
        SkillRollbackTarget::BatchId(batch_id) => {
            Ok(history.iter().find(|item| item.batch.id == *batch_id))
        }
        SkillRollbackTarget::ChangeId(change_id) => {
            let audit_exists = change_log
                .query(&ChangeFilter::new())?
                .into_iter()
                .any(|entry| entry.change_id == *change_id);
            if !audit_exists {
                return Ok(None);
            }
            Ok(history.iter().find(|item| {
                item.batch
                    .audits
                    .iter()
                    .any(|entry| entry.change_id == *change_id)
            }))
        }
        SkillRollbackTarget::Rule(_) => Ok(None),
    }
}

fn non_tip_reason(target: &SkillHistoryItem, history: &[SkillHistoryItem]) -> Option<String> {
    for file in &target.batch.request.files {
        let latest = history
            .iter()
            .filter(|item| {
                item.batch
                    .request
                    .files
                    .iter()
                    .any(|candidate| candidate.path == file.path)
            })
            .max_by_key(|item| item.generation);
        if latest.map(|item| item.batch.id.as_str()) != Some(target.batch.id.as_str()) {
            return Some(format!(
                "skill file {} is no longer at target generation",
                file.path.display()
            ));
        }
    }
    for name in changed_skill_names(&target.batch.request) {
        let latest = history
            .iter()
            .filter(|item| changed_skill_names(&item.batch.request).contains(&name))
            .max_by_key(|item| item.generation);
        if latest.map(|item| item.batch.id.as_str()) != Some(target.batch.id.as_str()) {
            return Some(format!(
                "skill lifecycle {name} is no longer at target generation"
            ));
        }
    }
    None
}

fn target_change_id(target: &SkillRollbackTarget) -> Option<String> {
    match target {
        SkillRollbackTarget::ChangeId(change_id) => Some(change_id.clone()),
        SkillRollbackTarget::BatchId(_) | SkillRollbackTarget::Rule(_) => None,
    }
}

fn inverse_request(
    request_id: String,
    target: &SkillHistoryItem,
    target_change_id: Option<String>,
) -> SkillMutationRequest {
    SkillMutationRequest {
        request_id,
        entity_key: target.batch.request.entity_key.clone(),
        kind: SkillMutationKind::Rollback,
        reason: format!("rollback skill batch {}", target.batch.id),
        files: target
            .batch
            .request
            .files
            .iter()
            .map(|file| SkillFileMutation {
                path: file.path.clone(),
                before: file.after.clone(),
                after: file.before.clone(),
            })
            .collect(),
        curator_before: target.batch.request.curator_after.clone(),
        curator_after: target.batch.request.curator_before.clone(),
        rollback_of: Some(SkillRollbackLineage {
            target_batch_id: target.batch.id.clone(),
            target_change_id,
            target_change_ids: target
                .batch
                .audits
                .iter()
                .map(|audit| audit.change_id.clone())
                .collect(),
            target_generation: target.generation,
        }),
    }
}

fn changed_skill_names(request: &SkillMutationRequest) -> Vec<String> {
    let mut names = HashSet::new();
    for name in request
        .curator_before
        .skills
        .keys()
        .chain(request.curator_after.skills.keys())
    {
        if request.curator_before.skills.get(name) != request.curator_after.skills.get(name) {
            names.insert(name.clone());
        }
    }
    let mut names = names.into_iter().collect::<Vec<_>>();
    names.sort();
    names
}

fn build_audits(
    batch_id: &str,
    request: &SkillMutationRequest,
    approval: &SkillApprovalArtifact,
) -> Vec<ChangeEntry> {
    let change_type = match request.kind {
        SkillMutationKind::Draft => ChangeType::Create,
        SkillMutationKind::Promote => ChangeType::Promote,
        SkillMutationKind::Touch | SkillMutationKind::Patch | SkillMutationKind::Rollback => {
            ChangeType::Update
        }
        SkillMutationKind::Deprecate => ChangeType::Demote,
        SkillMutationKind::Merge => ChangeType::Merge,
    };
    let operation = serde_json::json!({
        "mutation_kind": request.kind.clone(),
        "request_id": request.request_id.clone(),
        "batch_id": batch_id,
        "approval": approval,
        "rollback_of": request.rollback_of.clone(),
    });
    vec![
        ChangeEntryBuilder::new(EntityType::Skill, &request.entity_key, change_type)
            .before(serde_json::json!({
                "files": request.files.iter().map(file_summary_before).collect::<Vec<_>>(),
                "curator": curator_summary(request, true),
                "operation": operation.clone(),
            }))
            .after(serde_json::json!({
                "files": request.files.iter().map(file_summary_after).collect::<Vec<_>>(),
                "curator": curator_summary(request, false),
                "operation": operation,
            }))
            .reason(request.reason.clone())
            .trigger("skill_mutation_authority")
            .build_with(uuid::Uuid::new_v4().to_string(), Utc::now()),
    ]
}

fn curator_summary(
    request: &SkillMutationRequest,
    before: bool,
) -> BTreeMap<String, Option<super::curator::SkillMeta>> {
    let state = if before {
        &request.curator_before
    } else {
        &request.curator_after
    };
    changed_skill_names(request)
        .into_iter()
        .map(|name| {
            let meta = state.skills.get(&name).cloned();
            (name, meta)
        })
        .collect()
}

fn file_summary_before(file: &SkillFileMutation) -> serde_json::Value {
    file_summary(&file.path, file.before.as_deref())
}

fn file_summary_after(file: &SkillFileMutation) -> serde_json::Value {
    file_summary(&file.path, file.after.as_deref())
}

fn file_summary(path: &Path, bytes: Option<&[u8]>) -> serde_json::Value {
    let scanner = SecretScanner::new();
    let (summary, redacted) = bytes
        .map(|value| {
            let scanned = scanner.scan(&String::from_utf8_lossy(value));
            (
                Some(scanned.content.chars().take(160).collect::<String>()),
                scanned.has_secrets,
            )
        })
        .unwrap_or((None, false));
    serde_json::json!({
        "path": path,
        "sha256": bytes.map(hex_digest),
        "length": bytes.map(<[u8]>::len),
        "summary": summary,
        "redacted": redacted,
    })
}

fn receipt(item: &SkillHistoryItem) -> SkillMutationReceipt {
    SkillMutationReceipt {
        request_id: item.batch.request.request_id.clone(),
        batch_id: item.batch.id.clone(),
        generation: item.generation,
        change_ids: item
            .batch
            .audits
            .iter()
            .map(|audit| audit.change_id.clone())
            .collect(),
        approval_id: item.batch.approval.approval_id.clone(),
    }
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn canonical_resource_path(path: &Path) -> Result<PathBuf> {
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(ReactError::Other(format!(
            "skill resource path contains parent traversal: {}",
            path.display()
        )));
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(Path::new(std::path::MAIN_SEPARATOR_STR)),
            Component::Normal(value) => normalized.push(value),
            Component::ParentDir => {
                return Err(ReactError::Other(
                    "normalized skill resource path contains parent traversal".into(),
                ));
            }
        }
    }
    if normalized.exists() {
        return std::fs::canonicalize(normalized).map_err(Into::into);
    }
    let mut probe = normalized.as_path();
    let mut suffix = Vec::<OsString>::new();
    while !probe.exists() {
        let name = probe.file_name().ok_or_else(|| {
            ReactError::Other(format!(
                "skill resource path has no existing ancestor: {}",
                normalized.display()
            ))
        })?;
        suffix.push(name.to_os_string());
        probe = probe.parent().ok_or_else(|| {
            ReactError::Other(format!(
                "skill resource path has no parent: {}",
                normalized.display()
            ))
        })?;
    }
    let mut canonical = std::fs::canonicalize(probe)?;
    for segment in suffix.into_iter().rev() {
        canonical.push(segment);
    }
    Ok(canonical)
}

fn write_optional(path: &Path, bytes: Option<&[u8]>) -> Result<()> {
    match bytes {
        Some(bytes) => echo_core::utils::fs::atomic_write(path, bytes).map_err(Into::into),
        None => match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        },
    }
}

fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evolution::audit::JsonlChangeLog;
    use crate::evolution::audit::{ChangeRecordOutcome, EntityType};
    use crate::evolution::curator::{CuratorConfig, SkillLifecycle, SkillMeta};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FailOnceLog {
        inner: JsonlChangeLog,
        fail_next: std::sync::atomic::AtomicBool,
    }

    impl FailOnceLog {
        fn new(path: PathBuf) -> Result<Self> {
            Ok(Self {
                inner: JsonlChangeLog::new(path)?,
                fail_next: std::sync::atomic::AtomicBool::new(false),
            })
        }

        fn fail_next(&self) {
            self.fail_next.store(true, Ordering::SeqCst);
        }
    }

    impl ChangeLog for FailOnceLog {
        fn durable_destination_identity(&self) -> Result<Option<String>> {
            self.inner.durable_destination_identity()
        }

        fn record(&self, entry: ChangeEntry) -> Result<()> {
            self.record_idempotent(entry).map(|_| ())
        }

        fn record_idempotent(&self, entry: ChangeEntry) -> Result<ChangeRecordOutcome> {
            if self.fail_next.swap(false, Ordering::SeqCst) {
                return Err(ReactError::Other("injected audit failure".into()));
            }
            self.inner.record_idempotent(entry)
        }

        fn query(&self, filter: &ChangeFilter) -> Result<Vec<ChangeEntry>> {
            self.inner.query(filter)
        }

        fn latest_for(
            &self,
            entity_type: EntityType,
            entity_key: &str,
        ) -> Result<Option<ChangeEntry>> {
            self.inner.latest_for(entity_type, entity_key)
        }

        fn len(&self) -> usize {
            self.inner.len()
        }
    }

    struct UnsupportedIdentityLog {
        inner: JsonlChangeLog,
    }

    impl UnsupportedIdentityLog {
        fn new(path: PathBuf) -> Result<Self> {
            Ok(Self {
                inner: JsonlChangeLog::new(path)?,
            })
        }
    }

    impl ChangeLog for UnsupportedIdentityLog {
        fn record(&self, entry: ChangeEntry) -> Result<()> {
            self.inner.record(entry)
        }

        fn record_idempotent(&self, entry: ChangeEntry) -> Result<ChangeRecordOutcome> {
            self.inner.record_idempotent(entry)
        }

        fn query(&self, filter: &ChangeFilter) -> Result<Vec<ChangeEntry>> {
            self.inner.query(filter)
        }

        fn latest_for(
            &self,
            entity_type: EntityType,
            entity_key: &str,
        ) -> Result<Option<ChangeEntry>> {
            self.inner.latest_for(entity_type, entity_key)
        }

        fn len(&self) -> usize {
            self.inner.len()
        }
    }

    #[derive(Default)]
    struct CountingObserver(AtomicUsize);

    impl SkillMutationObserver for CountingObserver {
        fn on_skill_mutation(&self, _receipt: &SkillMutationReceipt) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn skill_meta(name: &str, path: &Path, lifecycle: SkillLifecycle) -> SkillMeta {
        let now = Utc::now();
        let canonical_path =
            SkillFileMutation::canonical_path(path).unwrap_or_else(|_| path.to_path_buf());
        SkillMeta {
            name: name.to_owned(),
            path: Some(canonical_path),
            lifecycle,
            created_at: now,
            last_used_at: now,
            last_modified_at: now,
            pinned: false,
            agent_created: true,
            superseded_by: None,
        }
    }

    fn approval(preview: &SkillMutationPreview, id: &str) -> SkillApprovalArtifact {
        SkillApprovalArtifact::new(id, &preview.operation_digest, "reviewer", Utc::now())
    }

    fn open_json_authority(
        curator: Curator,
        path: impl AsRef<Path>,
    ) -> Result<(SkillMutationAuthority, Arc<JsonlChangeLog>)> {
        let log = Arc::new(JsonlChangeLog::new(path.as_ref().to_path_buf())?);
        let authority = SkillMutationAuthority::open(curator, log.clone())?;
        Ok((authority, log))
    }

    #[tokio::test]
    async fn approved_skill_mutation_is_durable_idempotent_and_rollbackable() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let state_path = dir.path().join("curator.json");
        let skill_path = dir.path().join("skills/a/SKILL.md");
        let curator = Curator::new(CuratorConfig::default(), &state_path);
        let observer = Arc::new(CountingObserver::default());
        let (authority, _log) =
            open_json_authority(curator.clone(), dir.path().join("changes.jsonl"))?;
        let authority = authority.with_observer(observer.clone());
        let before = curator.load_state()?;
        let mut after = before.clone();
        after.skills.insert(
            "a".into(),
            skill_meta("a", &skill_path, SkillLifecycle::Draft),
        );
        let request = SkillMutationRequest {
            request_id: "draft-a".into(),
            entity_key: "a".into(),
            kind: SkillMutationKind::Draft,
            reason: "approved draft".into(),
            files: vec![SkillFileMutation::new(
                &skill_path,
                None,
                Some(b"---\nname: a\n---\nbody\n".to_vec()),
            )?],
            curator_before: before,
            curator_after: after,
            rollback_of: None,
        };
        let preview = authority.preview(&request)?;
        let artifact = approval(&preview, "approval-a");
        let applied = authority.apply(request.clone(), artifact.clone()).await?;
        let receipt = match applied {
            SkillMutationOutcome::Applied(receipt) => receipt,
            other => return Err(ReactError::Other(format!("unexpected outcome: {other:?}"))),
        };
        assert_eq!(std::fs::read(&skill_path)?, b"---\nname: a\n---\nbody\n");
        assert_eq!(
            curator
                .load_state()?
                .skills
                .get("a")
                .map(|meta| meta.lifecycle),
            Some(SkillLifecycle::Draft)
        );
        assert_eq!(observer.0.load(Ordering::SeqCst), 1);
        assert!(matches!(
            authority.apply(request, artifact).await?,
            SkillMutationOutcome::AlreadyApplied(_)
        ));
        assert_eq!(observer.0.load(Ordering::SeqCst), 1);

        let rollback_target = SkillRollbackTarget::BatchId(receipt.batch_id);
        let rollback_preview = authority
            .preview_rollback("rollback-a", &rollback_target)
            .await?;
        let SkillRollbackPreviewOutcome::Ready(rollback_preview) = rollback_preview else {
            return Err(ReactError::Other("rollback preview was not ready".into()));
        };
        let rollback_approval = approval(&rollback_preview, "approval-rollback-a");
        let rollback = authority
            .rollback(
                "rollback-a",
                rollback_target.clone(),
                rollback_approval.clone(),
            )
            .await?;
        let rollback_receipt = match rollback {
            SkillMutationOutcome::Applied(receipt) => receipt,
            other => return Err(ReactError::Other(format!("unexpected rollback: {other:?}"))),
        };
        assert!(!skill_path.exists());
        assert!(!curator.load_state()?.skills.contains_key("a"));
        assert_eq!(observer.0.load(Ordering::SeqCst), 2);
        assert!(matches!(
            authority
                .rollback("rollback-a", rollback_target, rollback_approval)
                .await?,
            SkillMutationOutcome::AlreadyApplied(_)
        ));

        let redo_target = SkillRollbackTarget::BatchId(rollback_receipt.batch_id);
        let SkillRollbackPreviewOutcome::Ready(redo_preview) =
            authority.preview_rollback("redo-a", &redo_target).await?
        else {
            return Err(ReactError::Other(
                "rollback-of-rollback was not ready".into(),
            ));
        };
        assert!(matches!(
            authority
                .rollback(
                    "rollback-a",
                    redo_target.clone(),
                    approval(&redo_preview, "wrong-target-approval"),
                )
                .await?,
            SkillMutationOutcome::Conflict { .. }
        ));
        assert!(matches!(
            authority
                .rollback(
                    "redo-a",
                    redo_target,
                    approval(&redo_preview, "approval-redo-a"),
                )
                .await?,
            SkillMutationOutcome::Applied(_)
        ));
        assert_eq!(std::fs::read(&skill_path)?, b"---\nname: a\n---\nbody\n");
        Ok(())
    }

    #[tokio::test]
    async fn approval_mismatch_replay_and_external_edit_fail_closed() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let curator = Curator::new(CuratorConfig::default(), dir.path().join("curator.json"));
        let (authority, _log) =
            open_json_authority(curator.clone(), dir.path().join("changes.jsonl"))?;
        let path = dir.path().join("SKILL.md");
        std::fs::write(&path, "old")?;
        let state = curator.load_state()?;
        let request = SkillMutationRequest {
            request_id: "patch-a".into(),
            entity_key: "a".into(),
            kind: SkillMutationKind::Patch,
            reason: "patch".into(),
            files: vec![SkillFileMutation::new(
                &path,
                Some(b"old".to_vec()),
                Some(b"new".to_vec()),
            )?],
            curator_before: state.clone(),
            curator_after: state,
            rollback_of: None,
        };
        let preview = authority.preview(&request)?;
        let wrong = SkillApprovalArtifact::new("approval", "wrong", "reviewer", Utc::now());
        assert!(authority.apply(request.clone(), wrong).await.is_err());
        assert_eq!(std::fs::read(&path)?, b"old");
        std::fs::write(&path, "external")?;
        assert!(matches!(
            authority
                .apply(request, approval(&preview, "approval"))
                .await?,
            SkillMutationOutcome::Conflict { .. }
        ));
        assert_eq!(std::fs::read(&path)?, b"external");
        assert!(matches!(
            SkillMutationAuthority::rule_rollback_support("rules/policy"),
            SkillRollbackSupport::HostOwned { .. }
        ));
        Ok(())
    }

    #[test]
    fn security_rejection_happens_before_prepare() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let curator = Curator::new(CuratorConfig::default(), dir.path().join("curator.json"));
        let (authority, _log) =
            open_json_authority(curator.clone(), dir.path().join("changes.jsonl"))?;
        let state = curator.load_state()?;
        let request = SkillMutationRequest {
            request_id: "secret".into(),
            entity_key: "secret".into(),
            kind: SkillMutationKind::Patch,
            reason: "secret rejection".into(),
            files: vec![SkillFileMutation::new(
                dir.path().join("SKILL.md"),
                None,
                Some(b"API_KEY='abcdefghijklmnop'".to_vec()),
            )?],
            curator_before: state.clone(),
            curator_after: state,
            rollback_of: None,
        };
        assert!(authority.preview(&request).is_err());
        assert!(authority.journal.history()?.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn reserved_authority_entity_key_cannot_poison_marker_or_journal() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let state_path = dir.path().join("curator.json");
        let skill_path = dir.path().join("malicious/SKILL.md");
        let log_path = dir.path().join("changes.jsonl");
        let curator = Curator::new(CuratorConfig::default(), &state_path);
        let (authority, log) = open_json_authority(curator.clone(), &log_path)?;
        let state = curator.load_state()?;
        let request = SkillMutationRequest {
            request_id: "reserved-entity-key".into(),
            entity_key: SKILL_AUTHORITY_AUDIT_KEY.into(),
            kind: SkillMutationKind::Patch,
            reason: "attempt to collide with authority marker".into(),
            files: vec![SkillFileMutation::new(
                &skill_path,
                None,
                Some(b"---\nname: malicious\n---\nbody\n".to_vec()),
            )?],
            curator_before: state.clone(),
            curator_after: state,
            rollback_of: None,
        };
        let approval = SkillApprovalArtifact::new(
            "reserved-key-approval",
            SkillMutationAuthority::digest(&request)?,
            "reviewer",
            Utc::now(),
        );

        assert!(matches!(
            authority.apply(request, approval).await?,
            SkillMutationOutcome::Conflict { .. }
        ));
        assert!(!skill_path.exists());
        assert!(authority.journal.history()?.is_empty());
        assert_eq!(log.len(), 1);
        drop(authority);

        let reopened = SkillMutationAuthority::open(curator, log.clone())?;
        assert!(reopened.journal.history()?.is_empty());
        let markers = log
            .query(&ChangeFilter::new().with_key_prefix(SKILL_AUTHORITY_AUDIT_KEY))?
            .into_iter()
            .filter(|entry| entry.entity_key == SKILL_AUTHORITY_AUDIT_KEY)
            .count();
        assert_eq!(markers, 1);
        assert_eq!(log.len(), 1);
        Ok(())
    }

    #[test]
    fn path_alias_and_invalid_utf8_requests_fail_before_prepare() -> Result<()> {
        let dir = tempfile::tempdir()?;
        assert!(
            SkillFileMutation::new(
                dir.path().join("nested/../SKILL.md"),
                None,
                Some(b"valid".to_vec()),
            )
            .is_err()
        );
        let curator = Curator::new(CuratorConfig::default(), dir.path().join("curator.json"));
        let (authority, _log) =
            open_json_authority(curator.clone(), dir.path().join("changes.jsonl"))?;
        let state = curator.load_state()?;
        let invalid = SkillMutationRequest {
            request_id: "invalid-utf8".into(),
            entity_key: "invalid".into(),
            kind: SkillMutationKind::Patch,
            reason: "invalid utf8".into(),
            files: vec![SkillFileMutation::new(
                dir.path().join("SKILL.md"),
                None,
                Some(vec![0xff, 0xfe]),
            )?],
            curator_before: state.clone(),
            curator_after: state,
            rollback_of: None,
        };
        assert!(authority.preview(&invalid).is_err());
        let state = curator.load_state()?;
        let relative = SkillMutationRequest {
            request_id: "relative".into(),
            entity_key: "relative".into(),
            kind: SkillMutationKind::Patch,
            reason: "relative".into(),
            files: vec![SkillFileMutation {
                path: PathBuf::from("SKILL.md"),
                before: None,
                after: Some(b"valid".to_vec()),
            }],
            curator_before: state.clone(),
            curator_after: state,
            rollback_of: None,
        };
        assert!(authority.preview(&relative).is_err());

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let real = dir.path().join("real.md");
            let alias = dir.path().join("alias.md");
            std::fs::write(&real, "old")?;
            symlink(&real, &alias)?;
            let first =
                SkillFileMutation::new(&real, Some(b"old".to_vec()), Some(b"one".to_vec()))?;
            let second =
                SkillFileMutation::new(&alias, Some(b"old".to_vec()), Some(b"two".to_vec()))?;
            assert_eq!(first.path, second.path);
            let state = curator.load_state()?;
            let duplicate = SkillMutationRequest {
                request_id: "alias".into(),
                entity_key: "alias".into(),
                kind: SkillMutationKind::Patch,
                reason: "alias".into(),
                files: vec![first, second],
                curator_before: state.clone(),
                curator_after: state,
                rollback_of: None,
            };
            assert!(authority.preview(&duplicate).is_err());
        }
        assert!(authority.journal.history()?.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn secret_removal_audit_is_redacted_but_private_rollback_is_exact() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("SKILL.md");
        let secret = "API_KEY='abcdefghijklmnop'\n";
        std::fs::write(&path, secret)?;
        let curator = Curator::new(CuratorConfig::default(), dir.path().join("curator.json"));
        let log_path = dir.path().join("changes.jsonl");
        let (authority, _log) = open_json_authority(curator.clone(), &log_path)?;
        let state = curator.load_state()?;
        let request = SkillMutationRequest {
            request_id: "remove-secret".into(),
            entity_key: "secret".into(),
            kind: SkillMutationKind::Patch,
            reason: "remove secret".into(),
            files: vec![SkillFileMutation::new(
                &path,
                Some(secret.as_bytes().to_vec()),
                Some(b"clean\n".to_vec()),
            )?],
            curator_before: state.clone(),
            curator_after: state,
            rollback_of: None,
        };
        let preview = authority.preview(&request)?;
        let receipt = match authority
            .apply(request, approval(&preview, "remove-secret-approval"))
            .await?
        {
            SkillMutationOutcome::Applied(receipt) => receipt,
            other => return Err(ReactError::Other(format!("unexpected outcome: {other:?}"))),
        };
        let audit = std::fs::read_to_string(&log_path)?;
        assert!(!audit.contains("abcdefghijklmnop"));
        assert!(audit.contains("[REDACTED"));
        let target = SkillRollbackTarget::BatchId(receipt.batch_id);
        let SkillRollbackPreviewOutcome::Ready(rollback_preview) = authority
            .preview_rollback("restore-secret", &target)
            .await?
        else {
            return Err(ReactError::Other("secret rollback was not ready".into()));
        };
        authority
            .rollback(
                "restore-secret",
                target,
                approval(&rollback_preview, "restore-secret-approval"),
            )
            .await?;
        assert_eq!(std::fs::read_to_string(&path)?, secret);
        assert!(!std::fs::read_to_string(log_path)?.contains("abcdefghijklmnop"));
        Ok(())
    }

    #[tokio::test]
    async fn business_audit_exposes_approval_and_rollback_lineage_without_payload() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let curator = Curator::new(CuratorConfig::default(), dir.path().join("curator.json"));
        let log = Arc::new(JsonlChangeLog::new(dir.path().join("changes.jsonl"))?);
        let authority = SkillMutationAuthority::open(curator.clone(), log.clone())?;
        let mut receipts = Vec::new();
        for (index, kind) in [
            SkillMutationKind::Promote,
            SkillMutationKind::Merge,
            SkillMutationKind::Patch,
        ]
        .into_iter()
        .enumerate()
        {
            let key = format!("audit-{index}");
            let path = dir.path().join(format!("{key}.md"));
            std::fs::write(&path, "before")?;
            let before = curator.load_state()?;
            let mut after = before.clone();
            if kind == SkillMutationKind::Promote {
                after
                    .skills
                    .insert(key.clone(), skill_meta(&key, &path, SkillLifecycle::Active));
            }
            let request = SkillMutationRequest {
                request_id: format!("request-{index}"),
                entity_key: key.clone(),
                kind: kind.clone(),
                reason: format!("audit {kind:?}"),
                files: if kind == SkillMutationKind::Promote {
                    Vec::new()
                } else {
                    vec![SkillFileMutation::new(
                        &path,
                        Some(b"before".to_vec()),
                        Some(format!("after-{index}").into_bytes()),
                    )?]
                },
                curator_before: before,
                curator_after: after,
                rollback_of: None,
            };
            let preview = authority.preview(&request)?;
            let approval_id = format!("approval-{index}");
            let receipt = match authority
                .apply(request, approval(&preview, &approval_id))
                .await?
            {
                SkillMutationOutcome::Applied(receipt) => receipt,
                other => return Err(ReactError::Other(format!("unexpected outcome: {other:?}"))),
            };
            assert_eq!(receipt.approval_id, approval_id);
            let audit = log
                .latest_for(EntityType::Skill, &key)?
                .ok_or_else(|| ReactError::Other("missing business audit".into()))?;
            let operation = audit
                .after
                .as_ref()
                .and_then(|value| value.get("operation"))
                .ok_or_else(|| ReactError::Other("missing operation envelope".into()))?;
            assert_eq!(
                operation
                    .get("mutation_kind")
                    .and_then(serde_json::Value::as_str),
                Some(match kind {
                    SkillMutationKind::Promote => "promote",
                    SkillMutationKind::Merge => "merge",
                    SkillMutationKind::Patch => "patch",
                    _ => return Err(ReactError::Other("unexpected test mutation kind".into())),
                })
            );
            assert_eq!(
                operation
                    .get("approval")
                    .and_then(|value| value.get("approval_id"))
                    .and_then(serde_json::Value::as_str),
                Some(approval_id.as_str())
            );
            assert_eq!(
                operation
                    .get("approval")
                    .and_then(|value| value.get("operation_digest"))
                    .and_then(serde_json::Value::as_str),
                Some(preview.operation_digest.as_str())
            );
            assert_eq!(
                operation
                    .get("approval")
                    .and_then(|value| value.get("approver"))
                    .and_then(serde_json::Value::as_str),
                Some("reviewer")
            );
            assert!(
                operation
                    .get("approval")
                    .and_then(|value| value.get("approved_at"))
                    .and_then(serde_json::Value::as_str)
                    .is_some()
            );
            assert!(operation.get("request_id").is_some());
            assert!(operation.get("batch_id").is_some());
            receipts.push((key, receipt));
        }

        let (patch_key, patch_receipt) = receipts
            .pop()
            .ok_or_else(|| ReactError::Other("missing patch receipt".into()))?;
        let target_change_id = patch_receipt
            .change_ids
            .first()
            .cloned()
            .ok_or_else(|| ReactError::Other("missing patch change id".into()))?;
        let target = SkillRollbackTarget::ChangeId(target_change_id.clone());
        let SkillRollbackPreviewOutcome::Ready(preview) = authority
            .preview_rollback("audit-rollback", &target)
            .await?
        else {
            return Err(ReactError::Other("audit rollback was not ready".into()));
        };
        authority
            .rollback(
                "audit-rollback",
                target,
                approval(&preview, "audit-rollback-approval"),
            )
            .await?;
        let rollback_audit = log
            .latest_for(EntityType::Skill, &patch_key)?
            .ok_or_else(|| ReactError::Other("missing rollback business audit".into()))?;
        let lineage = rollback_audit
            .after
            .as_ref()
            .and_then(|value| value.get("operation"))
            .and_then(|value| value.get("rollback_of"))
            .ok_or_else(|| ReactError::Other("missing rollback lineage".into()))?;
        assert_eq!(
            lineage
                .get("target_batch_id")
                .and_then(serde_json::Value::as_str),
            Some(patch_receipt.batch_id.as_str())
        );
        assert_eq!(
            lineage
                .get("target_change_id")
                .and_then(serde_json::Value::as_str),
            Some(target_change_id.as_str())
        );
        assert_eq!(
            lineage
                .get("target_generation")
                .and_then(serde_json::Value::as_u64),
            Some(patch_receipt.generation)
        );
        Ok(())
    }

    #[tokio::test]
    async fn audit_failure_reconciles_exact_projection_once_after_restart() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let state_path = dir.path().join("curator.json");
        let skill_path = dir.path().join("SKILL.md");
        let curator = Curator::new(CuratorConfig::default(), &state_path);
        let state = curator.load_state()?;
        let request = SkillMutationRequest {
            request_id: "crash-request".into(),
            entity_key: "crash-skill".into(),
            kind: SkillMutationKind::Patch,
            reason: "crash recovery".into(),
            files: vec![SkillFileMutation::new(
                &skill_path,
                None,
                Some(b"durable skill".to_vec()),
            )?],
            curator_before: state.clone(),
            curator_after: state,
            rollback_of: None,
        };
        {
            let log = Arc::new(FailOnceLog::new(dir.path().join("changes-a.jsonl"))?);
            let authority = SkillMutationAuthority::open(curator.clone(), log.clone())?;
            let preview = authority.preview(&request)?;
            log.fail_next();
            assert!(
                authority
                    .apply(request, approval(&preview, "crash-approval"))
                    .await
                    .is_err()
            );
            assert_eq!(std::fs::read(&skill_path)?, b"durable skill");
        }
        let log_b_path = dir.path().join("changes-b.jsonl");
        let log_b = Arc::new(JsonlChangeLog::new(log_b_path.clone())?);
        assert!(SkillMutationAuthority::open(curator.clone(), log_b).is_err());
        assert_eq!(JsonlChangeLog::new(log_b_path)?.len(), 0);

        let log_path = dir.path().join("changes-a.jsonl");
        let (authority, _log) = open_json_authority(curator, &log_path)?;
        authority.reconcile()?;
        assert_eq!(std::fs::read(&skill_path)?, b"durable skill");
        assert_eq!(JsonlChangeLog::new(log_path)?.len(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn every_prepared_stage_retries_to_one_settled_receipt() -> Result<()> {
        for point in [
            SkillMutationFailPoint::Prepared,
            SkillMutationFailPoint::FilesProjected,
            SkillMutationFailPoint::CuratorProjected,
            SkillMutationFailPoint::AuditRecorded,
        ] {
            let dir = tempfile::tempdir()?;
            let path = dir.path().join("SKILL.md");
            let curator = Curator::new(CuratorConfig::default(), dir.path().join("curator.json"));
            let log = Arc::new(JsonlChangeLog::new(dir.path().join("changes.jsonl"))?);
            let before = curator.load_state()?;
            let mut after = before.clone();
            after.skills.insert(
                "stage".into(),
                skill_meta("stage", &path, SkillLifecycle::Draft),
            );
            let request = SkillMutationRequest {
                request_id: "stage-request".into(),
                entity_key: "stage".into(),
                kind: SkillMutationKind::Draft,
                reason: "stage recovery".into(),
                files: vec![SkillFileMutation::new(
                    &path,
                    None,
                    Some(b"stage-content".to_vec()),
                )?],
                curator_before: before,
                curator_after: after,
                rollback_of: None,
            };
            let artifact;
            {
                let authority = SkillMutationAuthority::open(curator.clone(), log.clone())?
                    .with_fail_point(point);
                let preview = authority.preview(&request)?;
                artifact = approval(&preview, "stage-approval");
                assert!(
                    authority
                        .apply(request.clone(), artifact.clone())
                        .await
                        .is_err()
                );
            }
            let authority = SkillMutationAuthority::open(curator.clone(), log.clone())?;
            assert!(matches!(
                authority.apply(request, artifact).await?,
                SkillMutationOutcome::AlreadyApplied(_)
            ));
            assert_eq!(std::fs::read(&path)?, b"stage-content");
            assert_eq!(
                curator
                    .load_state()?
                    .skills
                    .get("stage")
                    .map(|meta| meta.lifecycle),
                Some(SkillLifecycle::Draft)
            );
            assert_eq!(log.len(), 2);
        }
        Ok(())
    }

    #[tokio::test]
    async fn unrelated_candidate_insert_between_file_and_curator_is_preserved() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("SKILL.md");
        let curator = Curator::new(CuratorConfig::default(), dir.path().join("curator.json"));
        let log = Arc::new(JsonlChangeLog::new(dir.path().join("changes.jsonl"))?);
        let before = curator.load_state()?;
        let mut after = before.clone();
        after.skills.insert(
            "skill-a".into(),
            skill_meta("skill-a", &path, SkillLifecycle::Draft),
        );
        let request = SkillMutationRequest {
            request_id: "skill-a-draft".into(),
            entity_key: "skill-a".into(),
            kind: SkillMutationKind::Draft,
            reason: "draft with candidate interleave".into(),
            files: vec![SkillFileMutation::new(
                &path,
                None,
                Some(b"skill-a".to_vec()),
            )?],
            curator_before: before,
            curator_after: after,
            rollback_of: None,
        };
        let hook_curator = curator.clone();
        let authority = SkillMutationAuthority::open(curator.clone(), log)?.with_after_files_hook(
            Arc::new(move || {
                hook_curator.register_candidate_with_authority("candidate-b", "candidate-authority")
            }),
        );
        let preview = authority.preview(&request)?;
        authority
            .apply(request, approval(&preview, "interleave-approval"))
            .await?;
        let state = curator.load_state()?;
        assert!(state.skills.contains_key("skill-a"));
        assert!(state.skills.contains_key("candidate-b"));
        Ok(())
    }

    #[tokio::test]
    async fn unsettled_inverse_retry_reconciles_before_already_applied() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("SKILL.md");
        std::fs::write(&path, "before")?;
        let curator = Curator::new(CuratorConfig::default(), dir.path().join("curator.json"));
        let log = Arc::new(FailOnceLog::new(dir.path().join("changes.jsonl"))?);
        let state = curator.load_state()?;
        let request = SkillMutationRequest {
            request_id: "forward".into(),
            entity_key: "inverse".into(),
            kind: SkillMutationKind::Patch,
            reason: "forward".into(),
            files: vec![SkillFileMutation::new(
                &path,
                Some(b"before".to_vec()),
                Some(b"after".to_vec()),
            )?],
            curator_before: state.clone(),
            curator_after: state,
            rollback_of: None,
        };
        let receipt;
        let rollback_approval;
        let target;
        {
            let authority = SkillMutationAuthority::open(curator.clone(), log.clone())?;
            let preview = authority.preview(&request)?;
            receipt = match authority
                .apply(request, approval(&preview, "forward-approval"))
                .await?
            {
                SkillMutationOutcome::Applied(receipt) => receipt,
                other => return Err(ReactError::Other(format!("unexpected outcome: {other:?}"))),
            };
            target = SkillRollbackTarget::BatchId(receipt.batch_id.clone());
            let SkillRollbackPreviewOutcome::Ready(preview) = authority
                .preview_rollback("inverse-request", &target)
                .await?
            else {
                return Err(ReactError::Other("inverse preview was not ready".into()));
            };
            rollback_approval = approval(&preview, "inverse-approval");
            log.fail_next();
            assert!(
                authority
                    .rollback("inverse-request", target.clone(), rollback_approval.clone(),)
                    .await
                    .is_err()
            );
        }
        let authority = SkillMutationAuthority::open(curator, log.clone())?;
        assert!(matches!(
            authority
                .rollback("inverse-request", target, rollback_approval)
                .await?,
            SkillMutationOutcome::AlreadyApplied(_)
        ));
        assert_eq!(std::fs::read(path)?, b"before");
        assert_eq!(log.len(), 3);
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rollback_holds_serial_across_revalidation_prepare_and_projection() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("SKILL.md");
        std::fs::write(&path, "zero")?;
        let curator = Curator::new(CuratorConfig::default(), dir.path().join("curator.json"));
        let log = Arc::new(JsonlChangeLog::new(dir.path().join("changes.jsonl"))?);
        let calls = Arc::new(AtomicUsize::new(0));
        let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let release_rx = Arc::new(Mutex::new(release_rx));
        let hook_calls = calls.clone();
        let hook_release = release_rx.clone();
        let hook = Arc::new(move || {
            if hook_calls.fetch_add(1, Ordering::SeqCst) == 1 {
                entered_tx.send(()).map_err(|error| {
                    ReactError::Other(format!("rollback hook signal failed: {error}"))
                })?;
                hook_release
                    .lock()
                    .map_err(|error| {
                        ReactError::Other(format!("rollback hook lock failed: {error}"))
                    })?
                    .recv()
                    .map_err(|error| {
                        ReactError::Other(format!("rollback hook release failed: {error}"))
                    })?;
            }
            Ok(())
        });
        let authority = Arc::new(
            SkillMutationAuthority::open(curator.clone(), log)?.with_after_files_hook(hook),
        );
        let state = curator.load_state()?;
        let first = SkillMutationRequest {
            request_id: "serial-forward".into(),
            entity_key: "serial".into(),
            kind: SkillMutationKind::Patch,
            reason: "forward".into(),
            files: vec![SkillFileMutation::new(
                &path,
                Some(b"zero".to_vec()),
                Some(b"one".to_vec()),
            )?],
            curator_before: state.clone(),
            curator_after: state.clone(),
            rollback_of: None,
        };
        let preview = authority.preview(&first)?;
        let receipt = match authority
            .apply(first, approval(&preview, "serial-forward-approval"))
            .await?
        {
            SkillMutationOutcome::Applied(receipt) => receipt,
            other => return Err(ReactError::Other(format!("unexpected outcome: {other:?}"))),
        };
        let target = SkillRollbackTarget::BatchId(receipt.batch_id);
        let SkillRollbackPreviewOutcome::Ready(rollback_preview) = authority
            .preview_rollback("serial-rollback", &target)
            .await?
        else {
            return Err(ReactError::Other("serial rollback was not ready".into()));
        };
        let rollback_approval = approval(&rollback_preview, "serial-rollback-approval");
        let newer = SkillMutationRequest {
            request_id: "serial-newer".into(),
            entity_key: "serial".into(),
            kind: SkillMutationKind::Patch,
            reason: "newer".into(),
            files: vec![SkillFileMutation::new(
                &path,
                Some(b"one".to_vec()),
                Some(b"two".to_vec()),
            )?],
            curator_before: state.clone(),
            curator_after: state,
            rollback_of: None,
        };
        let newer_preview = authority.preview(&newer)?;
        let newer_approval = approval(&newer_preview, "serial-newer-approval");
        let rollback_task = {
            let authority = authority.clone();
            tokio::spawn(async move {
                authority
                    .rollback("serial-rollback", target, rollback_approval)
                    .await
            })
        };
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .map_err(|error| {
                ReactError::Other(format!("rollback did not enter projection hook: {error}"))
            })?;
        let newer_task = {
            let authority = authority.clone();
            tokio::spawn(async move { authority.apply(newer, newer_approval).await })
        };
        tokio::task::yield_now().await;
        release_tx.send(()).map_err(|error| {
            ReactError::Other(format!("rollback release signal failed: {error}"))
        })?;
        let rollback_outcome = rollback_task
            .await
            .map_err(|error| ReactError::Other(format!("rollback task failed: {error}")))??;
        assert!(matches!(rollback_outcome, SkillMutationOutcome::Applied(_)));
        let newer_outcome = newer_task
            .await
            .map_err(|error| ReactError::Other(format!("newer task failed: {error}")))??;
        assert!(matches!(
            newer_outcome,
            SkillMutationOutcome::Conflict { .. }
        ));
        assert_eq!(std::fs::read(path)?, b"zero");
        Ok(())
    }

    #[tokio::test]
    async fn non_tip_aba_rollback_is_rejected_by_generation() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("SKILL.md");
        std::fs::write(&path, "same")?;
        let curator = Curator::new(CuratorConfig::default(), dir.path().join("curator.json"));
        let (authority, _log) =
            open_json_authority(curator.clone(), dir.path().join("changes.jsonl"))?;
        let state = curator.load_state()?;
        let first = SkillMutationRequest {
            request_id: "aba-1".into(),
            entity_key: "aba".into(),
            kind: SkillMutationKind::Patch,
            reason: "first".into(),
            files: vec![SkillFileMutation::new(
                &path,
                Some(b"same".to_vec()),
                Some(b"different".to_vec()),
            )?],
            curator_before: state.clone(),
            curator_after: state.clone(),
            rollback_of: None,
        };
        let first_preview = authority.preview(&first)?;
        let first_receipt = match authority
            .apply(first, approval(&first_preview, "aba-approval-1"))
            .await?
        {
            SkillMutationOutcome::Applied(receipt) => receipt,
            other => return Err(ReactError::Other(format!("unexpected outcome: {other:?}"))),
        };
        let second = SkillMutationRequest {
            request_id: "aba-2".into(),
            entity_key: "aba".into(),
            kind: SkillMutationKind::Patch,
            reason: "second".into(),
            files: vec![SkillFileMutation::new(
                path,
                Some(b"different".to_vec()),
                Some(b"same".to_vec()),
            )?],
            curator_before: state.clone(),
            curator_after: state,
            rollback_of: None,
        };
        let second_preview = authority.preview(&second)?;
        authority
            .apply(second, approval(&second_preview, "aba-approval-2"))
            .await?;
        assert!(matches!(
            authority
                .preview_rollback(
                    "rollback-old",
                    &SkillRollbackTarget::BatchId(first_receipt.batch_id),
                )
                .await?,
            SkillRollbackPreviewOutcome::Conflict { .. }
        ));
        Ok(())
    }

    #[test]
    fn same_stem_curator_paths_have_isolated_journals() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let json = Curator::new(CuratorConfig::default(), dir.path().join("state.json"));
        let toml = Curator::new(CuratorConfig::default(), dir.path().join("state.toml"));
        assert_ne!(
            json.skill_operation_journal_path(),
            toml.skill_operation_journal_path()
        );
        let (_json, _) = open_json_authority(json, dir.path().join("json.jsonl"))?;
        let (_toml, _) = open_json_authority(toml, dir.path().join("toml.jsonl"))?;
        Ok(())
    }

    #[test]
    fn authorities_for_same_root_share_one_in_process_serial() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let curator = Curator::new(CuratorConfig::default(), dir.path().join("state.json"));
        let log = Arc::new(JsonlChangeLog::new(dir.path().join("changes.jsonl"))?);
        let owner = SkillMutationAuthority::open(curator.clone(), log.clone())?;
        let peer = SkillMutationAuthority::open(curator, log)?;
        assert!(Arc::ptr_eq(&owner.journal.serial, &peer.journal.serial));
        Ok(())
    }

    #[test]
    fn concurrent_open_binds_exactly_one_destination_marker() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let state_path = dir.path().join("state.json");
        let curator = Curator::new(CuratorConfig::default(), &state_path);
        let log_a = Arc::new(JsonlChangeLog::new(dir.path().join("a.jsonl"))?);
        let log_b = Arc::new(JsonlChangeLog::new(dir.path().join("b.jsonl"))?);
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let first = {
            let curator = curator.clone();
            let log = log_a.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                SkillMutationAuthority::open(curator, log).is_ok()
            })
        };
        let second = {
            let curator = curator.clone();
            let log = log_b.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                SkillMutationAuthority::open(curator, log).is_ok()
            })
        };
        barrier.wait();
        let first_ok = first
            .join()
            .map_err(|_| ReactError::Other("first authority open panicked".into()))?;
        let second_ok = second
            .join()
            .map_err(|_| ReactError::Other("second authority open panicked".into()))?;
        assert_ne!(first_ok, second_ok);
        assert_eq!(log_a.len() + log_b.len(), 1);

        let same_dir = tempfile::tempdir()?;
        let same_curator =
            Curator::new(CuratorConfig::default(), same_dir.path().join("state.json"));
        let same_log = Arc::new(JsonlChangeLog::new(same_dir.path().join("changes.jsonl"))?);
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let mut joins = Vec::new();
        for _ in 0..2 {
            let curator = same_curator.clone();
            let log = same_log.clone();
            let barrier = barrier.clone();
            joins.push(std::thread::spawn(move || {
                barrier.wait();
                SkillMutationAuthority::open(curator, log).is_ok()
            }));
        }
        barrier.wait();
        let mut successes = 0;
        for join in joins {
            if join
                .join()
                .map_err(|_| ReactError::Other("same-log authority open panicked".into()))?
            {
                successes += 1;
            }
        }
        assert!(successes >= 1);
        assert_eq!(same_log.len(), 1);
        let reopened = SkillMutationAuthority::open(same_curator.clone(), same_log.clone())?;
        assert!(!reopened.audit_authority_id().is_empty());

        let copied_path = same_dir.path().join("copied.jsonl");
        std::fs::copy(same_dir.path().join("changes.jsonl"), &copied_path)?;
        let copied_log = Arc::new(JsonlChangeLog::new(copied_path)?);
        let copied_result = SkillMutationAuthority::open(same_curator.clone(), copied_log);
        assert!(matches!(copied_result, Err(ReactError::Other(_))));
        let same_path_reopened = SkillMutationAuthority::open(same_curator, same_log)?;
        assert_eq!(
            same_path_reopened.audit_authority_id(),
            reopened.audit_authority_id()
        );
        Ok(())
    }

    #[test]
    fn authority_rejects_changelog_without_durable_destination_identity() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let curator = Curator::new(CuratorConfig::default(), dir.path().join("state.json"));
        let log = Arc::new(UnsupportedIdentityLog::new(
            dir.path().join("changes.jsonl"),
        )?);
        assert!(matches!(
            SkillMutationAuthority::open(curator, log),
            Err(ReactError::Memory(error))
                if matches!(error.as_ref(), MemoryError::Unsupported(_))
        ));
        Ok(())
    }

    #[test]
    fn authority_rejects_unmarked_existing_business_log() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let curator = Curator::new(CuratorConfig::default(), dir.path().join("state.json"));
        let log = Arc::new(JsonlChangeLog::new(dir.path().join("changes.jsonl"))?);
        log.record(
            ChangeEntryBuilder::new(EntityType::Skill, "existing", ChangeType::Update)
                .reason("existing business record")
                .trigger("test")
                .build(log.as_ref()),
        )?;
        assert!(matches!(
            SkillMutationAuthority::open(curator, log),
            Err(ReactError::Other(_))
        ));
        Ok(())
    }

    #[test]
    fn skill_journal_lease_child_probe() -> Result<()> {
        let Some(path) = std::env::var_os("ECHO_TEST_SKILL_JOURNAL_STATE") else {
            return Ok(());
        };
        let curator = Curator::new(CuratorConfig::default(), PathBuf::from(&path));
        let log = Arc::new(JsonlChangeLog::new(
            PathBuf::from(&path).with_extension("changes.jsonl"),
        )?);
        if SkillMutationAuthority::open(curator, log).is_ok() {
            return Err(ReactError::Other(
                "competing process opened the live skill journal".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn second_process_fails_closed_while_skill_authority_is_live() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let state_path = dir.path().join("state.json");
        let curator = Curator::new(CuratorConfig::default(), &state_path);
        let log = Arc::new(JsonlChangeLog::new(dir.path().join("changes.jsonl"))?);
        let _owner = SkillMutationAuthority::open(curator, log)?;
        let executable = std::env::current_exe()?;
        let child = std::process::Command::new(executable)
            .arg("--exact")
            .arg("evolution::skill_mutation::tests::skill_journal_lease_child_probe")
            .env("ECHO_TEST_SKILL_JOURNAL_STATE", state_path)
            .output()?;
        if !child.status.success() {
            return Err(ReactError::Other(format!(
                "competing skill journal process probe failed: {}",
                String::from_utf8_lossy(&child.stdout)
            )));
        }
        Ok(())
    }

    #[tokio::test]
    async fn usage_handle_never_creates_unknown_and_audits_existing_skill() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let curator = Curator::new(CuratorConfig::default(), dir.path().join("curator.json"));
        let (authority, log) =
            open_json_authority(curator.clone(), dir.path().join("changes.jsonl"))?;
        let authority = Arc::new(authority);
        let usage = SkillUsageHandle::new(authority.clone());
        assert_eq!(
            usage.record_usage("unknown").await?,
            SkillUsageOutcome::UnknownSkill
        );
        assert!(curator.load_state()?.skills.is_empty());

        let before = curator.load_state()?;
        let mut after = before.clone();
        let skill_path = dir.path().join("known/SKILL.md");
        after.skills.insert(
            "known".into(),
            skill_meta("known", &skill_path, SkillLifecycle::Active),
        );
        let request = SkillMutationRequest {
            request_id: "register-known".into(),
            entity_key: "known".into(),
            kind: SkillMutationKind::Promote,
            reason: "register known skill".into(),
            files: Vec::new(),
            curator_before: before,
            curator_after: after,
            rollback_of: None,
        };
        let preview = authority.preview(&request)?;
        authority
            .apply(request, approval(&preview, "register-known-approval"))
            .await?;
        let before_usage = curator
            .load_state()?
            .skills
            .get("known")
            .map(|meta| meta.last_used_at)
            .ok_or_else(|| ReactError::Other("known skill missing".into()))?;
        assert_eq!(
            usage.record_usage("known").await?,
            SkillUsageOutcome::Updated
        );
        let after_usage = curator
            .load_state()?
            .skills
            .get("known")
            .map(|meta| meta.last_used_at)
            .ok_or_else(|| ReactError::Other("known skill missing after usage".into()))?;
        assert!(after_usage >= before_usage);
        assert_eq!(log.len(), 3);
        Ok(())
    }

    #[tokio::test]
    async fn concurrent_usage_updates_serialize_without_creating_parallel_authority() -> Result<()>
    {
        let dir = tempfile::tempdir()?;
        let curator = Curator::new(CuratorConfig::default(), dir.path().join("curator.json"));
        let (authority, log) =
            open_json_authority(curator.clone(), dir.path().join("changes.jsonl"))?;
        let authority = Arc::new(authority);
        let before = curator.load_state()?;
        let mut after = before.clone();
        after.skills.insert(
            "known".into(),
            skill_meta(
                "known",
                &dir.path().join("SKILL.md"),
                SkillLifecycle::Active,
            ),
        );
        let request = SkillMutationRequest {
            request_id: "register-concurrent".into(),
            entity_key: "known".into(),
            kind: SkillMutationKind::Promote,
            reason: "register concurrent skill".into(),
            files: Vec::new(),
            curator_before: before,
            curator_after: after,
            rollback_of: None,
        };
        let preview = authority.preview(&request)?;
        authority
            .apply(request, approval(&preview, "register-concurrent-approval"))
            .await?;
        let first = SkillUsageHandle::new(authority.clone());
        let second = SkillUsageHandle::new(authority);
        let (first, second) =
            tokio::join!(first.record_usage("known"), second.record_usage("known"));
        assert_eq!(first?, SkillUsageOutcome::Updated);
        assert_eq!(second?, SkillUsageOutcome::Updated);
        assert_eq!(log.len(), 4);
        Ok(())
    }

    #[tokio::test]
    async fn consumed_approval_cannot_authorize_another_request() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("SKILL.md");
        std::fs::write(&path, "one")?;
        let curator = Curator::new(CuratorConfig::default(), dir.path().join("curator.json"));
        let (authority, _log) =
            open_json_authority(curator.clone(), dir.path().join("changes.jsonl"))?;
        let state = curator.load_state()?;
        let first = SkillMutationRequest {
            request_id: "approval-request-1".into(),
            entity_key: "approval".into(),
            kind: SkillMutationKind::Patch,
            reason: "first".into(),
            files: vec![SkillFileMutation::new(
                &path,
                Some(b"one".to_vec()),
                Some(b"two".to_vec()),
            )?],
            curator_before: state.clone(),
            curator_after: state.clone(),
            rollback_of: None,
        };
        let first_preview = authority.preview(&first)?;
        let artifact = approval(&first_preview, "single-use-approval");
        authority.apply(first, artifact.clone()).await?;
        let second = SkillMutationRequest {
            request_id: "approval-request-2".into(),
            entity_key: "approval".into(),
            kind: SkillMutationKind::Patch,
            reason: "second".into(),
            files: vec![SkillFileMutation::new(
                &path,
                Some(b"two".to_vec()),
                Some(b"three".to_vec()),
            )?],
            curator_before: state.clone(),
            curator_after: state,
            rollback_of: None,
        };
        assert!(matches!(
            authority.apply(second, artifact).await?,
            SkillMutationOutcome::Conflict { .. }
        ));
        assert_eq!(std::fs::read(path)?, b"two");
        Ok(())
    }

    #[tokio::test]
    async fn cancellation_while_waiting_for_authority_prepares_nothing() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let curator = Curator::new(CuratorConfig::default(), dir.path().join("curator.json"));
        let log = Arc::new(JsonlChangeLog::new(dir.path().join("changes.jsonl"))?);
        let authority = Arc::new(SkillMutationAuthority::open(curator.clone(), log)?);
        let state = curator.load_state()?;
        let request = SkillMutationRequest {
            request_id: "cancelled".into(),
            entity_key: "cancelled".into(),
            kind: SkillMutationKind::Patch,
            reason: "cancelled".into(),
            files: vec![SkillFileMutation::new(
                dir.path().join("SKILL.md"),
                None,
                Some(b"content".to_vec()),
            )?],
            curator_before: state.clone(),
            curator_after: state,
            rollback_of: None,
        };
        let preview = authority.preview(&request)?;
        let artifact = approval(&preview, "cancelled-approval");
        let held = authority.journal.serial.lock().await;
        let task = {
            let authority = authority.clone();
            tokio::spawn(async move { authority.apply(request, artifact).await })
        };
        tokio::task::yield_now().await;
        task.abort();
        let _ = task.await;
        drop(held);
        assert!(authority.journal.history()?.is_empty());
        Ok(())
    }
}
