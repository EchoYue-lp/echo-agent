use echo_agent::config::FrameworkConfig;
use echo_agent::paths::DataRoot;
use echo_agent::runtime::{AgentTurnDriver, TurnMode};
use echo_agent::state::journal::{
    EventJournal, JournalAppendError, JournalBatchAppendError, JournalBatchCommitStatus,
    JournalBatchLookup, JournalDurabilityStatus, MemoryEventJournal, PreparedJournalBatch,
    SegmentedFileEventJournal,
};
use echo_agent::tasks::{
    RuntimeDagController, RuntimeInterruptionDisposition, RuntimeInterruptionReceipt,
    RuntimeInterruptionSettlementOutcome, RuntimePlanSnapshot, RuntimeTaskClaimOutcome,
    RuntimeTaskMutationError, RuntimeTaskRequeueOutcome, RuntimeTaskResumeOutcome,
    RuntimeTaskRetryOutcome, RuntimeTaskService, RuntimeTaskServiceConfig,
    RuntimeTaskSettlementOutcome, Task, TaskClaim, cancel_unfinished_runtime_tasks,
    resume_runtime_task, retry_runtime_task,
};
use echo_agent::tools::{StandardToolPack, ToolPack};

#[test]
fn public_facade_composes_without_split_crates() {
    let config = FrameworkConfig::default();
    let root = DataRoot::new("/tmp/echo-agent-smoke");
    let pack = StandardToolPack::new();

    assert!(config.model.name.is_empty());
    assert_eq!(
        root.path("state.json"),
        std::path::PathBuf::from("/tmp/echo-agent-smoke/state.json")
    );
    assert_eq!(pack.name(), "standard");
}

#[test]
fn runtime_state_and_task_primitives_are_available_from_the_facade() {
    let journal = MemoryEventJournal::new();
    let receipt = journal.append("facade-event".to_string()).expect("append");
    assert_eq!(receipt.record.sequence, 1);
    assert_eq!(receipt.durability, JournalDurabilityStatus::Confirmed);

    let batch = journal
        .append_batch(
            PreparedJournalBatch::new(vec!["second".to_string(), "third".to_string()])
                .expect("prepare facade batch"),
        )
        .expect("append facade batch");
    assert_eq!(batch.records().len(), 2);
    assert_eq!(batch.commit_status(), JournalBatchCommitStatus::Committed);
    assert_eq!(
        batch.records().first().map(|record| record.sequence),
        Some(2)
    );
    let _typed_batch_error: Option<JournalBatchAppendError<String>> = None;
    let _typed_single_error: Option<JournalAppendError<String>> = None;
    let prepared =
        PreparedJournalBatch::new(vec!["absent".to_string()]).expect("prepare facade lookup");
    assert!(matches!(
        journal.lookup_batch(&prepared).expect("facade lookup"),
        JournalBatchLookup::Absent
    ));

    let _driver = AgentTurnDriver;
    let _mode = TurnMode::Chat;
    let _mutation_error: Option<RuntimeTaskMutationError> = None;
    let _requeue = RuntimeTaskRequeueOutcome::Superseded;
    let _retry = RuntimeTaskRetryOutcome::Superseded;
    let _resume = RuntimeTaskResumeOutcome::Superseded;
    let _settlement = RuntimeTaskSettlementOutcome::Superseded;
    let _interruption: Option<RuntimeInterruptionReceipt> = None;
    let _claim: Option<TaskClaim> = None;
    let _claim_outcome: Option<RuntimeTaskClaimOutcome> = None;
    let service = RuntimeTaskService::new(
        std::sync::Arc::new(FacadeController),
        RuntimeTaskServiceConfig::default(),
    );
    let _execution = service.execute("compile-only", tokio_util::sync::CancellationToken::new());
    let _retry_mutation: fn(
        &mut RuntimePlanSnapshot,
        &Task,
        u64,
    ) -> Result<RuntimeTaskRetryOutcome, RuntimeTaskMutationError> = retry_runtime_task;
    let _resume_mutation: fn(
        &mut RuntimePlanSnapshot,
        &Task,
        u64,
    ) -> Result<RuntimeTaskResumeOutcome, RuntimeTaskMutationError> = resume_runtime_task;
    let _cancel_mutation: fn(
        &mut RuntimePlanSnapshot,
        u64,
    ) -> Result<
        RuntimeInterruptionSettlementOutcome,
        RuntimeTaskMutationError,
    > = cancel_unfinished_runtime_tasks;
    assert_eq!(
        RuntimeInterruptionDisposition::default(),
        RuntimeInterruptionDisposition::Cancelled
    );
}

struct FacadeController;

#[async_trait::async_trait]
impl RuntimeDagController for FacadeController {
    type DispatchOutput = ();

    async fn load_snapshot(&self, _run_id: &str) -> echo_agent::error::Result<RuntimePlanSnapshot> {
        Err(echo_agent::error::ReactError::Other(
            "compile-only facade controller".to_string(),
        ))
    }

    async fn claim_task(
        &self,
        _run_id: &str,
        _task: &Task,
        _expected_revision: u64,
    ) -> echo_agent::error::Result<RuntimeTaskClaimOutcome> {
        Ok(RuntimeTaskClaimOutcome::ReloadSnapshot)
    }

    async fn claim_is_current(
        &self,
        _run_id: &str,
        _task_id: &str,
        _claim: &TaskClaim,
    ) -> echo_agent::error::Result<bool> {
        Ok(false)
    }

    async fn dispatch_task(
        &self,
        _context: echo_agent::tasks::TaskSubagentContext,
        _claim: TaskClaim,
        _task: Task,
    ) -> echo_agent::error::Result<Self::DispatchOutput> {
        Ok(())
    }

    async fn resolve_dispatch(
        &self,
        _run_id: &str,
        _claim: TaskClaim,
        _task: Task,
        _dispatch: echo_agent::error::Result<Self::DispatchOutput>,
    ) -> echo_agent::error::Result<echo_agent::tasks::RuntimeTaskResolutionRequest> {
        Ok(echo_agent::tasks::RuntimeTaskResolutionRequest::Completed)
    }

    async fn settle_resolution(
        &self,
        _run_id: &str,
        _claim: &TaskClaim,
        _task: &Task,
        _request: echo_agent::tasks::RuntimeTaskResolutionRequest,
    ) -> echo_agent::error::Result<echo_agent::tasks::RuntimeTaskResolution> {
        Ok(echo_agent::tasks::RuntimeTaskResolution::Superseded)
    }

    async fn abandon_claim(
        &self,
        _run_id: &str,
        _claim: &TaskClaim,
        _task: &Task,
        _abandonment: echo_agent::tasks::RuntimeClaimAbandonment,
    ) -> echo_agent::error::Result<RuntimeTaskSettlementOutcome> {
        Ok(RuntimeTaskSettlementOutcome::Superseded)
    }

    async fn settle_interruption(
        &self,
        _run_id: &str,
        _expected_revision: u64,
        _disposition: RuntimeInterruptionDisposition,
    ) -> echo_agent::error::Result<RuntimeInterruptionSettlementOutcome> {
        Ok(RuntimeInterruptionSettlementOutcome::ReloadSnapshot)
    }
}

#[test]
fn segmented_journal_is_available_from_the_public_facade() {
    let root = tempfile::tempdir().expect("segmented facade tempdir");
    let journal = SegmentedFileEventJournal::<String>::open(
        root.path().join("events"),
        1024,
        echo_agent::utils::fs::FileDurability::Flush,
    )
    .expect("open segmented journal through facade");
    let receipt = journal
        .append_with_durability(
            "facade-segmented-event".to_string(),
            echo_agent::utils::fs::FileDurability::SyncData,
        )
        .expect("append segmented facade event");
    assert_eq!(receipt.record.sequence, 1);
    assert_eq!(journal.segments().len(), 1);
}

#[cfg(feature = "mcp")]
#[test]
fn mcp_reconcile_receipts_are_available_from_the_facade() {
    fn public_type<T>() {}

    public_type::<echo_agent::mcp::McpTargetReceipt>();
    assert_eq!(
        echo_agent::mcp::McpTargetChange::Connected,
        echo_agent::advanced::McpTargetChange::Connected
    );
}
