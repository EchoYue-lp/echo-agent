use echo_agent::config::FrameworkConfig;
use echo_agent::paths::DataRoot;
use echo_agent::runtime::{AgentTurnDriver, TurnMode};
use echo_agent::state::journal::{EventJournal, JournalDurabilityStatus, MemoryEventJournal};
use echo_agent::tasks::{RuntimeTaskMutationError, RuntimeTaskRequeueOutcome};
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
    let receipt = journal.append(&"facade-event".to_string()).expect("append");
    assert_eq!(receipt.record.sequence, 1);
    assert_eq!(receipt.durability, JournalDurabilityStatus::Confirmed);

    let _driver = AgentTurnDriver;
    let _mode = TurnMode::Chat;
    let _mutation_error: Option<RuntimeTaskMutationError> = None;
    let _requeue = RuntimeTaskRequeueOutcome::Superseded;
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
