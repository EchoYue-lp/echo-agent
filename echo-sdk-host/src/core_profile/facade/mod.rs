//! Facade adapter runtime (plan 07, todo 2).
//!
//! One raw dispatcher owns every facade wire method that is compiled into
//! this build: the generic `_echo_agent/facade/invoke` and the family
//! methods whose handler families have landed (`features::
//! compiled_facade_families`). The dispatcher attaches to the same
//! official builder chain as the typed core handlers
//! ([`super::SdkCoreProfile::attach`]), so all profiles share one
//! transport, writer, Session/Run and close chain.
//!
//! Methods the dispatcher does not own fall through
//! (`Handled::No`) and the official runtime answers method-not-found —
//! exactly the fail-closed contract for plain clients and for families
//! this build did not compile.
//!
//! The admission ladder (mirroring the core handlers):
//!
//! 1. Extended-mode gate — Standard connections get the official
//!    method-not-found (the dispatcher does not claim at all);
//! 2. capability gate — `feature_surfaces` must be advertised;
//! 3. request validation — exact operation identity, sha256 signature
//!    digest, no wildcards, bounded typed arguments;
//! 4. route resolution — family method binding or exact invoke identity
//!    through the embedded canonical catalog; unknown operations fail
//!    closed as `invalid_value` with typed detail;
//! 5. feature gate — the family's root leaf feature must be advertised;
//! 6. family dispatch — compiled family handlers route to the real
//!    framework services; families this build did not compile answer with
//!    a typed `feature_unavailable` (never a partial or simulated result).

pub(crate) mod integrations;
pub(crate) mod memory;
pub(crate) mod observability;
#[cfg(feature = "framework-human-loop")]
pub(crate) mod permission;
pub(crate) mod registry;
pub(crate) mod source_operations;
pub(crate) mod state_delivery;
pub(crate) mod stream;
pub(crate) mod structured_output;
#[cfg(feature = "framework-subagent")]
pub(crate) mod subagent;
pub(crate) mod task;
#[cfg(feature = "framework-subagent")]
pub(crate) mod task_runtime;
#[cfg(feature = "framework-telemetry")]
pub(crate) mod telemetry;
pub(crate) mod tools;
pub(crate) mod workflow;

use agent_client_protocol::{Client, ConnectionTo, Dispatch, Error, HandleDispatchFrom, Handled};
use echo_sdk_protocol::capability::ExtensionCapability;
use echo_sdk_protocol::error::{
    EchoSdkError, ErrorDetails, ExtensionErrorCode, FacadeFailureDetail, Retryability,
};
use echo_sdk_protocol::handle::{HandleKind, WireHandle};
use echo_sdk_protocol::methods::{FeatureOperationRequest, FeatureOperationResponse};
use echo_sdk_protocol::scalar::WireValue;
use std::sync::Arc;
#[cfg(feature = "framework-subagent")]
use std::sync::atomic::{AtomicBool, Ordering};

/// One family handler outcome: the typed wire result or typed failure.
type WireResult = Result<WireValue, EchoSdkError>;

/// Parse one [`WireHandle`] argument addressing an open facade resource.
/// Family operations address their resources through Host-issued handles
/// only (design §8): shape, kind, generation and issuance are checked by
/// the unified authority on every access.
pub(crate) fn resource_handle_at(
    arguments: &[serde_json::Value],
    position: usize,
    what: &str,
    method: &str,
) -> Result<WireHandle, EchoSdkError> {
    let value = arguments.get(position).cloned().ok_or_else(|| {
        wire::sdk_error(
            ExtensionErrorCode::InvalidValue,
            format!("operation requires {what} at argument {position}"),
            Retryability::Never,
            method,
        )
    })?;
    serde_json::from_value::<WireHandle>(value).map_err(|error| {
        wire::sdk_error(
            ExtensionErrorCode::InvalidValue,
            format!("{what} at argument {position} is not a resource handle: {error}"),
            Retryability::Never,
            method,
        )
    })
}

/// Resolve one facade resource through the unified authority and prove it
/// belongs to the requesting session.
pub(crate) fn owned_resource(
    handles: &HandleRegistry,
    resource: &WireHandle,
    owner: &str,
    method: &str,
) -> Result<Arc<FacadeResourceRecord>, EchoSdkError> {
    let record = handles.facade_resource(resource, method)?;
    if record.owner_session.as_deref() != Some(owner) {
        return Err(wire::sdk_error(
            ExtensionErrorCode::InvalidValue,
            "facade resource belongs to another session",
            Retryability::Never,
            method,
        ));
    }
    Ok(record)
}

use super::handles::{FacadeResourceRecord, HandleRegistry};
use super::state::CoreProfileState;
use super::wire;
use crate::config::SdkProfileLimits;
use registry::CompiledOperationCatalog;

/// Connection-level facade runtime: the resource handle surface in
/// [`super::handles::HandleRegistry`] and the RPC subagent dispatch records.
/// Stream handles are also owned by that registry; this runtime only keeps
/// family business maps and bounded execution records.
pub(crate) struct SessionFacadeRuntime {
    /// File leases and identity guards issued by the source-operation
    /// adapter. Their Rust values stay alive until the unified facade
    /// resource handle is closed, preserving the framework's descriptor and
    /// lease lifetime semantics without introducing another handle registry.
    file_authorities: std::sync::Mutex<
        std::collections::HashMap<String, Arc<source_operations::FileAuthorityRecord>>,
    >,
    pub(crate) tokenizer_authorities: std::sync::Mutex<
        std::collections::HashMap<String, Arc<dyn echo_agent::tokenizer::Tokenizer>>,
    >,
    #[cfg(feature = "framework-content-guard")]
    pub(crate) content_guard_authorities: std::sync::Mutex<
        std::collections::HashMap<String, source_operations::ContentGuardAuthorityRecord>,
    >,
    #[cfg(feature = "framework-project-rules")]
    pub(crate) instruction_resolver_authorities: std::sync::Mutex<
        std::collections::HashMap<String, source_operations::InstructionResolverAuthorityRecord>,
    >,
    streams: stream::FacadeStreamRuntime,
    #[cfg(feature = "framework-subagent")]
    subagents:
        std::sync::Mutex<std::collections::HashMap<String, Arc<subagent::SubagentDispatchRecord>>>,
    /// Advertised live-subagent bound (`max_open_handles`): the map never
    /// grows past it, so a chatty client cannot exhaust the Host.
    #[cfg(feature = "framework-subagent")]
    max_subagents: usize,
    #[cfg(feature = "framework-subagent")]
    task_executions: std::sync::Mutex<std::collections::HashMap<String, TaskRunExecution>>,
    // Workflow family resources (todo 4): compiled graphs and standalone
    // shared states, owner-checked per session. The maps hold addressing
    // only; the graph engine keeps its own state.
    workflow_graphs:
        std::sync::Mutex<std::collections::HashMap<String, Arc<workflow::WorkflowGraphRecord>>>,
    workflow_states:
        std::sync::Mutex<std::collections::HashMap<String, Arc<workflow::WorkflowStateRecord>>>,
    // Delivery ledgers and trace stores (todo 4 step 3): framework
    // services held as owner-checked resources.
    delivery_ledgers: std::sync::Mutex<
        std::collections::HashMap<String, Arc<state_delivery::DeliveryLedgerRecord>>,
    >,
    trace_stores:
        std::sync::Mutex<std::collections::HashMap<String, Arc<observability::TraceStoreRecord>>>,
    #[cfg(feature = "framework-eval")]
    pub(crate) eval_runners:
        std::sync::Mutex<std::collections::HashMap<String, Arc<echo_agent::eval::EvalRunner>>>,
    #[cfg(feature = "framework-eval")]
    pub(crate) llm_graders:
        std::sync::Mutex<std::collections::HashMap<String, Arc<echo_agent::eval::LlmGrader>>>,
    #[cfg(feature = "framework-improve")]
    pub(crate) trajectory_savers: std::sync::Mutex<
        std::collections::HashMap<String, Arc<echo_agent::improve::TrajectorySaver>>,
    >,
    #[cfg(feature = "framework-improve")]
    pub(crate) improvement_loops: std::sync::Mutex<
        std::collections::HashMap<String, Arc<echo_agent::improve::ImprovementLoop>>,
    >,
    pub(crate) plugin_registries: std::sync::Mutex<
        std::collections::HashMap<
            String,
            Arc<tokio::sync::Mutex<echo_agent::plugin::PluginRegistry>>,
        >,
    >,
    pub(crate) memory_store_resources: std::sync::Mutex<
        std::collections::HashMap<String, Arc<source_operations::MemoryStoreAuthority>>,
    >,
    pub(crate) prompt_contexts: std::sync::Mutex<
        std::collections::HashMap<String, Arc<echo_agent::skills::external::PromptContext>>,
    >,
    /// RAG index/search dependencies scoped to one ACP session. The map is
    /// deliberately connection-owned and never process-global: closing a
    /// session drops both its in-memory index and embedder client.
    #[cfg(feature = "framework-rag")]
    rag_tools: std::sync::Mutex<std::collections::HashMap<String, Arc<tools::RagToolSet>>>,
    // Integration family resources (todo 5): MCP managers, A2A clients,
    // LSP managers and topology trackers, owner-checked per session.
    pub integrations: integrations::IntegrationResources,
}

impl SessionFacadeRuntime {
    pub fn new(limits: &SdkProfileLimits) -> Self {
        Self {
            file_authorities: std::sync::Mutex::new(std::collections::HashMap::new()),
            tokenizer_authorities: std::sync::Mutex::new(std::collections::HashMap::new()),
            #[cfg(feature = "framework-content-guard")]
            content_guard_authorities: std::sync::Mutex::new(std::collections::HashMap::new()),
            #[cfg(feature = "framework-project-rules")]
            instruction_resolver_authorities: std::sync::Mutex::new(
                std::collections::HashMap::new(),
            ),
            streams: stream::FacadeStreamRuntime::new(
                limits.max_facade_streams,
                limits.shutdown_timeout_secs,
            ),
            #[cfg(feature = "framework-subagent")]
            subagents: std::sync::Mutex::new(std::collections::HashMap::new()),
            #[cfg(feature = "framework-subagent")]
            max_subagents: limits.max_open_handles,
            #[cfg(feature = "framework-subagent")]
            task_executions: std::sync::Mutex::new(std::collections::HashMap::new()),
            workflow_graphs: std::sync::Mutex::new(std::collections::HashMap::new()),
            workflow_states: std::sync::Mutex::new(std::collections::HashMap::new()),
            delivery_ledgers: std::sync::Mutex::new(std::collections::HashMap::new()),
            trace_stores: std::sync::Mutex::new(std::collections::HashMap::new()),
            #[cfg(feature = "framework-eval")]
            eval_runners: std::sync::Mutex::new(std::collections::HashMap::new()),
            #[cfg(feature = "framework-eval")]
            llm_graders: std::sync::Mutex::new(std::collections::HashMap::new()),
            #[cfg(feature = "framework-improve")]
            trajectory_savers: std::sync::Mutex::new(std::collections::HashMap::new()),
            #[cfg(feature = "framework-improve")]
            improvement_loops: std::sync::Mutex::new(std::collections::HashMap::new()),
            plugin_registries: std::sync::Mutex::new(std::collections::HashMap::new()),
            memory_store_resources: std::sync::Mutex::new(std::collections::HashMap::new()),
            prompt_contexts: std::sync::Mutex::new(std::collections::HashMap::new()),
            #[cfg(feature = "framework-rag")]
            rag_tools: std::sync::Mutex::new(std::collections::HashMap::new()),
            integrations: integrations::IntegrationResources::new(),
        }
    }

    /// Register one RPC-dispatched subagent record by execution id. The
    /// advertised live-subagent bound is a hard limit: settled records are
    /// retired first, and a full map rejects the dispatch with a typed
    /// error instead of growing without end.
    #[cfg(feature = "framework-subagent")]
    pub fn register_subagent(
        &self,
        execution_id: String,
        record: Arc<subagent::SubagentDispatchRecord>,
    ) -> Result<(), EchoSdkError> {
        let mut subagents = self
            .subagents
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if subagents.len() >= self.max_subagents {
            // Drop settled records first; unsettled (or currently-joining)
            // attempts stay, and a still-full map rejects the new dispatch.
            subagents.retain(|_, record| {
                !record
                    .settled
                    .try_lock()
                    .is_ok_and(|settled| settled.is_some())
            });
            if subagents.len() >= self.max_subagents {
                return Err(wire::sdk_error(
                    ExtensionErrorCode::PayloadTooLarge,
                    format!(
                        "live subagent dispatch limit {} reached",
                        self.max_subagents
                    ),
                    Retryability::AfterDelay,
                    "_echo_agent/subagent/dispatch",
                ));
            }
        }
        subagents.insert(execution_id, record);
        Ok(())
    }

    /// Resolve one subagent record by execution id.
    #[cfg(feature = "framework-subagent")]
    pub fn subagent_of(&self, execution_id: &str) -> Option<Arc<subagent::SubagentDispatchRecord>> {
        self.subagents
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(execution_id)
            .cloned()
    }

    /// One live task-graph execution per TaskRun scope, addressable for
    /// run-level pause/cancel through the shared cancel token. Insertion is
    /// atomic: a second execute of a live scope fails instead of silently
    /// replacing the tracked execution (the old DAG would otherwise keep
    /// running untracked and its cleanup would delete the new record).
    #[cfg(feature = "framework-subagent")]
    pub fn try_register_task_execution(
        &self,
        scope: String,
        execution: TaskRunExecution,
    ) -> Result<(), EchoSdkError> {
        let mut executions = self
            .task_executions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if executions.contains_key(&scope) {
            return Err(wire::sdk_error(
                ExtensionErrorCode::InvalidValue,
                "task run is already executing; cancel it before re-executing",
                Retryability::Never,
                "_echo_agent/task/execute",
            ));
        }
        executions.insert(scope, execution);
        Ok(())
    }

    #[cfg(feature = "framework-subagent")]
    pub fn task_execution_of(&self, scope: &str) -> Option<TaskRunExecution> {
        self.task_executions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(scope)
            .cloned()
    }

    #[cfg(feature = "framework-subagent")]
    pub fn remove_task_execution(&self, scope: &str) {
        self.task_executions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(scope);
    }

    /// Lazily create the RAG tool set for one ACP session. Configuration is
    /// checked before insertion, so an unavailable embedder never appears as
    /// a successful or shared process-global capability.
    #[cfg(feature = "framework-rag")]
    pub fn rag_tools_for_session(
        &self,
        owner: &str,
        operation: &str,
    ) -> Result<Arc<tools::RagToolSet>, EchoSdkError> {
        let mut rag_tools = self
            .rag_tools
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(existing) = rag_tools.get(owner) {
            return Ok(existing.clone());
        }
        let created = Arc::new(
            tools::RagToolSet::from_env().map_err(|error| error.into_sdk_error(operation))?,
        );
        rag_tools.insert(owner.to_string(), created.clone());
        Ok(created)
    }

    /// Cancel and remove every task execution owned by one session scope;
    /// session close must never leave a DAG running untracked.
    #[cfg(feature = "framework-subagent")]
    pub fn drop_task_executions_of(&self, scope: &str) {
        let removed = self
            .task_executions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(scope);
        if let Some(execution) = removed {
            execution.cancel.cancel();
        }
    }

    /// Cancel every live task execution and wait for each spawned service
    /// task to settle (connection teardown; the caller's shutdown timeout
    /// bounds the whole wait).
    #[cfg(feature = "framework-subagent")]
    pub async fn cancel_all_task_executions_and_wait(&self) {
        let executions = std::mem::take(
            &mut *self
                .task_executions
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
        for (_, execution) in executions {
            execution.cancel.cancel();
            while !execution.completion.settled.load(Ordering::Acquire) {
                let notified = execution.completion.notify.notified();
                if execution.completion.settled.load(Ordering::Acquire) {
                    break;
                }
                notified.await;
            }
        }
    }

    /// Cancel every RPC subagent dispatch attempt owned by one session.
    #[cfg(feature = "framework-subagent")]
    pub fn cancel_subagents_of(&self, owner: &str) {
        let mut subagents = self
            .subagents
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let owned: Vec<String> = subagents
            .iter()
            .filter(|(_, record)| record.owner_session == owner)
            .map(|(execution_id, _)| execution_id.clone())
            .collect();
        for execution_id in owned {
            if let Some(record) = subagents.remove(&execution_id) {
                record.background.cancel();
            }
        }
    }

    /// Cancel every live RPC subagent dispatch attempt and wait for each
    /// background attempt to settle (connection teardown; the caller's
    /// shutdown timeout bounds the whole wait).
    #[cfg(feature = "framework-subagent")]
    pub async fn cancel_all_subagents_and_wait(&self) {
        let records = std::mem::take(
            &mut *self
                .subagents
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
        for (_, record) in records {
            record.background.cancel();
            let _ = record.background.join().await;
        }
    }

    /// Drop and cancel every facade resource whose handle the unified
    /// authority just closed for this session; called when the session
    /// closes so family resources never outlive their owner. Task
    /// executions of the session's graph scope and its live subagent
    /// dispatches are cancelled too.
    pub async fn drop_session_resources_of(&self, owner: &str, closed: &[String]) {
        self.streams.close_owner(owner).await;
        source_operations::drop_file_authorities(&self.file_authorities, closed);
        source_operations::drop_tokenizer_authorities(&self.tokenizer_authorities, closed);
        #[cfg(feature = "framework-content-guard")]
        source_operations::drop_value_authorities(&self.content_guard_authorities, closed);
        #[cfg(feature = "framework-project-rules")]
        source_operations::drop_value_authorities(&self.instruction_resolver_authorities, closed);
        workflow::drop_session_resources(&self.workflow_graphs, &self.workflow_states, closed);
        state_delivery::drop_session_ledgers(&self.delivery_ledgers, closed);
        observability::drop_session_stores(&self.trace_stores, closed);
        #[cfg(feature = "framework-eval")]
        self.eval_runners
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .retain(|id, _| !closed.iter().any(|closed_id| closed_id == id));
        #[cfg(feature = "framework-eval")]
        self.llm_graders
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .retain(|id, _| !closed.iter().any(|closed_id| closed_id == id));
        #[cfg(feature = "framework-improve")]
        self.trajectory_savers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .retain(|id, _| !closed.iter().any(|closed_id| closed_id == id));
        #[cfg(feature = "framework-improve")]
        self.improvement_loops
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .retain(|id, _| !closed.iter().any(|closed_id| closed_id == id));
        self.plugin_registries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .retain(|id, _| !closed.iter().any(|closed_id| closed_id == id));
        self.memory_store_resources
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .retain(|id, _| !closed.iter().any(|closed_id| closed_id == id));
        self.prompt_contexts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .retain(|id, _| !closed.iter().any(|closed_id| closed_id == id));
        #[cfg(feature = "framework-rag")]
        self.rag_tools
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(owner);
        integrations::close_session_resources(&self.integrations, closed).await;
        #[cfg(feature = "framework-subagent")]
        {
            self.drop_task_executions_of(owner);
            self.cancel_subagents_of(owner);
        }
    }

    /// Close one owner-checked facade resource and release its concrete Rust
    /// business object. The unified registry is closed first so concurrent
    /// requests cannot resolve the handle while its lease/manager/graph is
    /// being torn down; business cleanup then follows the same path as
    /// Session teardown.
    pub async fn close_resource(
        &self,
        handles: &HandleRegistry,
        owner: &str,
        resource: &WireHandle,
        operation: &str,
    ) -> Result<bool, EchoSdkError> {
        if handles.is_closed(resource) {
            return Ok(false);
        }
        let _record = owned_resource(handles, resource, owner, operation)?;
        let closed = handles.close_facade_resource(resource, operation)?;
        if !closed {
            return Ok(false);
        }
        self.streams.close_resource(&resource.id).await;
        let ids = [resource.id.clone()];
        source_operations::drop_file_authorities(&self.file_authorities, &ids);
        source_operations::drop_tokenizer_authorities(&self.tokenizer_authorities, &ids);
        #[cfg(feature = "framework-content-guard")]
        source_operations::drop_value_authorities(&self.content_guard_authorities, &ids);
        #[cfg(feature = "framework-project-rules")]
        source_operations::drop_value_authorities(&self.instruction_resolver_authorities, &ids);
        workflow::drop_session_resources(&self.workflow_graphs, &self.workflow_states, &ids);
        state_delivery::drop_session_ledgers(&self.delivery_ledgers, &ids);
        observability::drop_session_stores(&self.trace_stores, &ids);
        #[cfg(feature = "framework-eval")]
        self.eval_runners
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&resource.id);
        #[cfg(feature = "framework-eval")]
        self.llm_graders
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&resource.id);
        #[cfg(feature = "framework-improve")]
        self.trajectory_savers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&resource.id);
        #[cfg(feature = "framework-improve")]
        self.improvement_loops
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&resource.id);
        self.plugin_registries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&resource.id);
        self.memory_store_resources
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&resource.id);
        self.prompt_contexts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&resource.id);
        #[cfg(feature = "framework-rag")]
        if _record.family == "rag" {
            self.rag_tools
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .remove(owner);
        }
        integrations::close_session_resources(&self.integrations, &ids).await;
        Ok(true)
    }

    /// Connection teardown: cancel task executions and subagent dispatches
    /// and wait for their bounded settlement, close every resource handle
    /// in the unified authority, await integration close (MCP child
    /// processes included) and release the remaining business maps. The
    /// caller runs this inside the bounded shutdown chain.
    pub async fn close_all(&self, handles: &HandleRegistry) {
        #[cfg(feature = "framework-subagent")]
        {
            self.cancel_all_task_executions_and_wait().await;
            self.cancel_all_subagents_and_wait().await;
        }
        self.streams.close_all().await;
        handles.close_all_facade_resources();
        std::mem::take(
            &mut *self
                .file_authorities
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
        std::mem::take(
            &mut *self
                .memory_store_resources
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
        std::mem::take(
            &mut *self
                .prompt_contexts
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
        std::mem::take(
            &mut *self
                .plugin_registries
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
        #[cfg(feature = "framework-eval")]
        std::mem::take(
            &mut *self
                .llm_graders
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
        std::mem::take(
            &mut *self
                .tokenizer_authorities
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
        #[cfg(feature = "framework-content-guard")]
        std::mem::take(
            &mut *self
                .content_guard_authorities
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
        #[cfg(feature = "framework-project-rules")]
        std::mem::take(
            &mut *self
                .instruction_resolver_authorities
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
        self.integrations.close_all().await;
        std::mem::take(
            &mut *self
                .workflow_graphs
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
        std::mem::take(
            &mut *self
                .workflow_states
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
        std::mem::take(
            &mut *self
                .delivery_ledgers
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
        std::mem::take(
            &mut *self
                .trace_stores
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
        #[cfg(feature = "framework-eval")]
        std::mem::take(
            &mut *self
                .eval_runners
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
        #[cfg(feature = "framework-improve")]
        std::mem::take(
            &mut *self
                .trajectory_savers
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
        #[cfg(feature = "framework-improve")]
        std::mem::take(
            &mut *self
                .improvement_loops
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
        #[cfg(feature = "framework-rag")]
        {
            std::mem::take(
                &mut *self
                    .rag_tools
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()),
            );
        }
    }
}

/// Per-request bounds passed to every family handler: the advertised facade
/// resource limit and the page bound. Family handlers enforce these instead
/// of private magic constants so the real limit always matches the
/// `initialize` advertisement (design §15 — payload, queue and shutdown
/// bounds are all explicit).
#[derive(Debug, Clone, Copy)]
pub(crate) struct FacadeFamilyLimits {
    pub max_resources: usize,
    pub page: usize,
}

/// A live task-graph execution (cheap clone: token + controller Arcs).
/// `done` fires exactly once when the execution task settles, so bounded
/// teardown can wait for real settlement instead of abandoning the DAG.
#[cfg(feature = "framework-subagent")]
pub(crate) struct TaskRunCompletion {
    pub settled: AtomicBool,
    pub notify: tokio::sync::Notify,
}

#[cfg(feature = "framework-subagent")]
#[derive(Clone)]
pub(crate) struct TaskRunExecution {
    pub cancel: tokio_util::sync::CancellationToken,
    pub controller: Arc<task_runtime::FacadeTaskController>,
    pub completion: Arc<TaskRunCompletion>,
}

/// Typed facade admission error carrying [`FacadeFailureDetail`].
fn facade_error(
    code: ExtensionErrorCode,
    message: impl Into<String>,
    retryable: Retryability,
    operation: &str,
    detail: FacadeFailureDetail,
) -> EchoSdkError {
    let mut error = EchoSdkError::new(code, message, retryable).with_operation(operation);
    error.details = Some(ErrorDetails {
        fields: None,
        facade: Some(detail),
    });
    error
}

/// Whether this build's dispatcher owns one wire method. Family methods
/// are owned once their handler family compiled in; the generic invoke is
/// owned by the facade runtime itself.
fn owns_method(method: &str) -> bool {
    if method == "_echo_agent/facade/invoke" {
        return true;
    }
    if method == "_echo_agent/structured_output/validate" {
        return true;
    }
    let compiled = crate::features::compiled_facade_families();
    CompiledOperationCatalog::global()
        .ok()
        .and_then(|catalog| catalog.family_method(method))
        .is_some_and(|summary| compiled.contains(&summary.family.as_str()))
}

/// The raw facade dispatcher attached to the official builder chain.
pub(crate) struct FacadeDispatcher {
    state: Arc<CoreProfileState>,
}

impl FacadeDispatcher {
    pub fn new(state: Arc<CoreProfileState>) -> Self {
        Self { state }
    }

    async fn dispatch(
        &self,
        method: &str,
        params: &serde_json::Value,
        respond: impl FnOnce(Result<FeatureOperationResponse, Error>) -> Result<(), Error>,
    ) -> Result<(), Error> {
        match facade_admission(&self.state, method, params).await {
            Ok(response) => respond(Ok(response)),
            Err(error) => respond(Err(wire::into_jsonrpc_error(error))),
        }
    }
}

impl HandleDispatchFrom<Client> for FacadeDispatcher {
    async fn handle_dispatch_from(
        &mut self,
        message: Dispatch,
        connection: ConnectionTo<Client>,
    ) -> Result<Handled<Dispatch>, Error> {
        let method = message.method().to_string();
        let extended = match self.state.services() {
            Ok(services) => services.is_extended().await,
            Err(_) => false,
        };
        // Standard connections — and methods this build does not own —
        // fall through to the official method-not-found.
        if !extended || !owns_method(&method) {
            return Ok(Handled::No {
                message,
                retry: false,
            });
        }
        let Dispatch::Request(request, responder) = message else {
            return Ok(Handled::No {
                message,
                retry: false,
            });
        };
        let params = request.params().clone();
        let state = self.state.clone();
        let method_for_task = method.clone();
        let connection_for_task = connection.clone();
        // Never block the dispatch loop: the ladder itself is fast, but the
        // family work rides the official connection task so connection
        // teardown takes in-flight facade work down with the transport
        // instead of leaving detached tasks behind (plan 07 todo 2 step 3).
        // The responder lives in a shared cell: whoever settles first (the
        // spawned task or the spawn-failure path) answers exactly once, so
        // a failed spawn still delivers the typed host-shutting-down
        // failure while the request is alive.
        let spawn_method = method_for_task.clone();
        let responder_cell = Arc::new(std::sync::Mutex::new(Some(responder)));
        let cell_for_task = responder_cell.clone();
        if let Err(error) = connection.spawn(async move {
            // Direct Agent chat/execute source operations reuse the typed
            // RunStart handler, including EventEnvelope delivery, stream
            // registration, cancellation and exactly-one terminal semantics.
            match source_operations::run_start_request(&state, &spawn_method, &params) {
                Ok(Some(run_request)) => {
                    let responder = cell_for_task
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .take()
                        .ok_or_else(|| {
                            Error::internal_error().data("facade response was already answered")
                        })?;
                    let run_responder = responder.wrap_params(|_method, result| {
                        result.and_then(|run| {
                            let value = serde_json::to_value(FeatureOperationResponse {
                                value: WireValue::from_json(serde_json::to_value(run).map_err(
                                    |error| {
                                        Error::internal_error()
                                            .data(format!("run response encoding failed: {error}"))
                                    },
                                )?)
                                .map_err(|error| {
                                    Error::internal_error()
                                        .data(format!("run response wire encoding failed: {error}"))
                                })?,
                            })
                            .map_err(|error| {
                                Error::internal_error()
                                    .data(format!("facade response encoding failed: {error}"))
                            })?;
                            Ok(value)
                        })
                    });
                    crate::core_profile::handler::run_start(
                        state.clone(),
                        run_request,
                        run_responder,
                        connection_for_task.clone(),
                    )
                    .await?;
                    return Ok(());
                }
                Ok(None) => {}
                Err(error) => {
                    let responder = cell_for_task
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .take();
                    if let Some(responder) = responder {
                        responder.respond_with_error(wire::into_jsonrpc_error(error))?;
                    }
                    return Ok(());
                }
            }
            match source_operations::run_steer_request(&state, &spawn_method, &params) {
                Ok(Some(steer_request)) => {
                    let responder = cell_for_task
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .take()
                        .ok_or_else(|| {
                            Error::internal_error().data("facade response was already answered")
                        })?;
                    let steer_responder = responder.wrap_params(|_method, result| {
                        result.and_then(|steer| {
                            let value = serde_json::to_value(FeatureOperationResponse {
                                value: WireValue::from_json(serde_json::to_value(steer).map_err(
                                    |error| {
                                        Error::internal_error().data(format!(
                                            "steer response encoding failed: {error}"
                                        ))
                                    },
                                )?)
                                .map_err(|error| {
                                    Error::internal_error().data(format!(
                                        "steer response wire encoding failed: {error}"
                                    ))
                                })?,
                            })
                            .map_err(|error| {
                                Error::internal_error()
                                    .data(format!("facade response encoding failed: {error}"))
                            })?;
                            Ok(value)
                        })
                    });
                    crate::core_profile::handler::run_steer(
                        state.clone(),
                        steer_request,
                        steer_responder,
                        connection_for_task.clone(),
                    )
                    .await?;
                    return Ok(());
                }
                Ok(None) => {}
                Err(error) => {
                    let responder = cell_for_task
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .take();
                    if let Some(responder) = responder {
                        responder.respond_with_error(wire::into_jsonrpc_error(error))?;
                    }
                    return Ok(());
                }
            }
            match source_operations::agent_close_request(&state, &spawn_method, &params) {
                Ok(Some(close_request)) => {
                    let responder = cell_for_task
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .take()
                        .ok_or_else(|| {
                            Error::internal_error().data("facade response was already answered")
                        })?;
                    let close_responder = responder.wrap_params(|_method, result| {
                        result.and_then(|closed| {
                            let value = serde_json::to_value(FeatureOperationResponse {
                                value: WireValue::from_json(serde_json::to_value(closed).map_err(
                                    |error| {
                                        Error::internal_error().data(format!(
                                            "agent close response encoding failed: {error}"
                                        ))
                                    },
                                )?)
                                .map_err(|error| {
                                    Error::internal_error().data(format!(
                                        "agent close response wire encoding failed: {error}"
                                    ))
                                })?,
                            })
                            .map_err(|error| {
                                Error::internal_error()
                                    .data(format!("facade response encoding failed: {error}"))
                            })?;
                            Ok(value)
                        })
                    });
                    crate::core_profile::handler::agent_close(
                        state.clone(),
                        close_request,
                        close_responder,
                        connection_for_task.clone(),
                    )
                    .await?;
                    return Ok(());
                }
                Ok(None) => {}
                Err(error) => {
                    let responder = cell_for_task
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .take();
                    if let Some(responder) = responder {
                        responder.respond_with_error(wire::into_jsonrpc_error(error))?;
                    }
                    return Ok(());
                }
            }
            let dispatcher = FacadeDispatcher { state };
            let respond = |result: Result<FeatureOperationResponse, Error>| {
                let responder = cell_for_task
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .take();
                match responder {
                    Some(responder) => match result {
                        Ok(response) => match serde_json::to_value(response) {
                            Ok(value) => responder.respond(value),
                            Err(error) => responder.respond_with_error(
                                Error::internal_error()
                                    .data(format!("facade response encoding failed: {error}")),
                            ),
                        },
                        Err(error) => responder.respond_with_error(error),
                    },
                    None => {
                        Err(Error::internal_error().data("facade response was already answered"))
                    }
                }
            };
            if let Err(error) = dispatcher.dispatch(&spawn_method, &params, respond).await {
                tracing::warn!("facade dispatch for {spawn_method} failed: {error}");
            }
            Ok(())
        }) {
            tracing::warn!("facade dispatch for {method_for_task} could not be spawned: {error}");
            if let Some(responder) = responder_cell
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take()
            {
                let _ = responder.respond_with_error(wire::into_jsonrpc_error(EchoSdkError::new(
                    ExtensionErrorCode::HostShuttingDown,
                    format!("connection can no longer host facade dispatch: {error}"),
                    Retryability::Never,
                )));
            }
        }
        Ok(Handled::Yes)
    }

    fn describe_chain(&self) -> impl std::fmt::Debug {
        "FacadeDispatcher"
    }
}

/// The unified facade admission ladder. Returns the typed response or the
/// typed extension error; both are answered on the claimed request.
async fn facade_admission(
    state: &CoreProfileState,
    method: &str,
    params: &serde_json::Value,
) -> Result<FeatureOperationResponse, EchoSdkError> {
    let services = state.services().map_err(|error| {
        EchoSdkError::new(
            ExtensionErrorCode::HostShuttingDown,
            error.to_string(),
            Retryability::Never,
        )
    })?;
    if !services.is_extended().await {
        return Err(EchoSdkError::new(
            ExtensionErrorCode::InvalidRequest,
            "facade methods require a negotiated extension connection",
            Retryability::Never,
        ));
    }
    if services.ensure_admission().is_err() {
        return Err(EchoSdkError::new(
            ExtensionErrorCode::HostShuttingDown,
            "ACP Host is shutting down",
            Retryability::Never,
        ));
    }
    if !state
        .advertisement
        .declares(ExtensionCapability::FeatureSurfaces)
    {
        return Err(EchoSdkError::new(
            ExtensionErrorCode::ExtensionCapabilityMismatch,
            "capability feature_surfaces is not advertised",
            Retryability::Never,
        ));
    }
    let mut request: FeatureOperationRequest =
        serde_json::from_value(params.clone()).map_err(|error| {
            facade_error(
                ExtensionErrorCode::InvalidRequest,
                format!("facade request payload is malformed: {error}"),
                Retryability::Never,
                method,
                FacadeFailureDetail::default(),
            )
        })?;
    if let Err(reason) = request.validate() {
        return Err(facade_error(
            ExtensionErrorCode::InvalidValue,
            reason,
            Retryability::Never,
            method,
            FacadeFailureDetail {
                operation: Some(request.operation.clone()),
                signature_digest: Some(request.signature_digest.clone()),
                ..FacadeFailureDetail::default()
            },
        ));
    }
    if request.arguments.len() > state.limits.max_facade_operation_args {
        return Err(facade_error(
            ExtensionErrorCode::PayloadTooLarge,
            format!(
                "facade operation has {} arguments; limit is {}",
                request.arguments.len(),
                state.limits.max_facade_operation_args
            ),
            Retryability::Never,
            method,
            FacadeFailureDetail {
                operation: Some(request.operation.clone()),
                ..FacadeFailureDetail::default()
            },
        ));
    }
    let catalog = CompiledOperationCatalog::global().map_err(|reason| {
        EchoSdkError::new(
            ExtensionErrorCode::InvalidConfig,
            reason,
            Retryability::Never,
        )
    })?;
    let (family, required_feature, handler_operation) = if method == "_echo_agent/facade/invoke" {
        // Exact source identities resolve through the invoke table; closed
        // family-operation identities dispatch through the same family
        // handler as `<family>/op` — the generic invoke surface is
        // executable for every frozen family operation (plan 07 todo 1).
        let (route_family, route_feature, handler_operation, digests, required_features, semantics) =
            match catalog.invoke_route(&request.operation) {
                Some(route) => (
                    route.family.clone(),
                    route.required_feature.clone(),
                    route.handler_operation.clone(),
                    route.signature_digests.clone(),
                    route.required_features.clone(),
                    route.feature_semantics.clone(),
                ),
                None => match catalog.family_for_operation(&request.operation) {
                    Some(operation_family) => {
                        let family_method = if operation_family == "invoke" {
                            "_echo_agent/facade/invoke".to_string()
                        } else {
                            format!("_echo_agent/{operation_family}/op")
                        };
                        let summary = catalog.family_method(&family_method).ok_or_else(|| {
                            facade_error(
                                ExtensionErrorCode::InvalidValue,
                                format!(
                                    "operation {} has no executable family surface",
                                    request.operation
                                ),
                                Retryability::Never,
                                method,
                                FacadeFailureDetail {
                                    operation: Some(request.operation.clone()),
                                    ..FacadeFailureDetail::default()
                                },
                            )
                        })?;
                        (
                            operation_family.to_string(),
                            summary.required_feature.clone(),
                            None,
                            summary
                                .operation_signatures
                                .get(&request.operation)
                                .cloned()
                                .unwrap_or_default(),
                            summary.required_features.clone(),
                            summary.feature_semantics.clone(),
                        )
                    }
                    None => {
                        return Err(facade_error(
                            ExtensionErrorCode::InvalidValue,
                            format!(
                                "operation {} is not a canonical route of this contract",
                                request.operation
                            ),
                            Retryability::Never,
                            method,
                            FacadeFailureDetail {
                                operation: Some(request.operation.clone()),
                                ..FacadeFailureDetail::default()
                            },
                        ));
                    }
                },
            };
        if !digests.is_empty()
            && !digests
                .iter()
                .any(|digest| digest == &request.signature_digest)
        {
            return Err(facade_error(
                ExtensionErrorCode::InvalidValue,
                format!(
                    "signature digest for {} is not part of the canonical route",
                    request.operation
                ),
                Retryability::Never,
                method,
                FacadeFailureDetail {
                    operation: Some(request.operation.clone()),
                    signature_digest: Some(request.signature_digest.clone()),
                    ..FacadeFailureDetail::default()
                },
            ));
        }
        let compiled = state.advertisement.features.iter();
        let all_compiled = |required: &[String]| {
            required
                .iter()
                .all(|feature| compiled.clone().any(|f| f == feature))
        };
        let any_compiled = |required: &[String]| {
            required.is_empty()
                || required
                    .iter()
                    .any(|feature| state.advertisement.features.iter().any(|f| f == feature))
        };
        let requirements_met = match semantics.as_str() {
            "all_of" => all_compiled(&required_features),
            "any_of" => any_compiled(&required_features),
            _ => all_compiled(&required_features),
        };
        if !requirements_met {
            return Err(facade_error(
                ExtensionErrorCode::FeatureUnavailable,
                format!(
                    "required feature set for {} is not compiled",
                    request.operation
                ),
                Retryability::Never,
                method,
                FacadeFailureDetail {
                    operation: Some(request.operation.clone()),
                    required_feature: route_feature.clone(),
                    ..FacadeFailureDetail::default()
                },
            ));
        }
        (route_family, route_feature, handler_operation)
    } else {
        let summary = catalog.family_method(method).ok_or_else(|| {
            EchoSdkError::new(
                ExtensionErrorCode::InvalidRequest,
                format!("method {method} is not a facade family surface"),
                Retryability::Never,
            )
        })?;
        let family_qualified_tool =
            crate::core_profile::facade::tools::TOOL_FAMILIES.contains(&summary.family.as_str())
                && summary.operations.iter().any(|operation| {
                    request.operation == format!("{}.{operation}", summary.family)
                });
        let operation_key = if family_qualified_tool {
            request
                .operation
                .split_once('.')
                .map(|(_, operation)| operation)
                .unwrap_or(request.operation.as_str())
        } else {
            request.operation.as_str()
        };
        if !summary.operations.is_empty()
            && !summary.operation_signatures.contains_key(operation_key)
        {
            return Err(facade_error(
                ExtensionErrorCode::InvalidValue,
                format!(
                    "operation {} is not implemented by family {}",
                    request.operation, summary.family
                ),
                Retryability::Never,
                method,
                FacadeFailureDetail {
                    operation: Some(request.operation.clone()),
                    signature_digest: Some(request.signature_digest.clone()),
                    ..FacadeFailureDetail::default()
                },
            ));
        }
        // Operation-bearing families compare the request digest against the
        // frozen per-operation envelope digest; protocol-native families
        // (structured output validation) carry no family operations and
        // their typed DTO is the contract.
        if !summary.operations.is_empty() {
            let allowed_digests =
                summary
                    .operation_signatures
                    .get(operation_key)
                    .ok_or_else(|| {
                        facade_error(
                            ExtensionErrorCode::InvalidValue,
                            format!(
                                "operation {} has no executable signature",
                                request.operation
                            ),
                            Retryability::Never,
                            method,
                            FacadeFailureDetail {
                                operation: Some(request.operation.clone()),
                                signature_digest: Some(request.signature_digest.clone()),
                                ..FacadeFailureDetail::default()
                            },
                        )
                    })?;
            if !allowed_digests
                .iter()
                .any(|digest| digest == &request.signature_digest)
            {
                return Err(facade_error(
                    ExtensionErrorCode::InvalidValue,
                    format!(
                        "signature digest for {} is not canonical",
                        request.operation
                    ),
                    Retryability::Never,
                    method,
                    FacadeFailureDetail {
                        operation: Some(request.operation.clone()),
                        signature_digest: Some(request.signature_digest.clone()),
                        ..FacadeFailureDetail::default()
                    },
                ));
            }
        }
        // The family surface's frozen feature requirement uses the same
        // all-of/any-of rules as invoke routes (design §13): advertisement,
        // preflight and handler must agree on one semantics.
        let requirements_met = match summary.feature_semantics.as_str() {
            "all_of" => summary
                .required_features
                .iter()
                .all(|feature| state.advertisement.features.iter().any(|f| f == feature)),
            "any_of" => {
                summary.required_features.is_empty()
                    || summary
                        .required_features
                        .iter()
                        .any(|feature| state.advertisement.features.iter().any(|f| f == feature))
            }
            _ => summary
                .required_features
                .iter()
                .all(|feature| state.advertisement.features.iter().any(|f| f == feature)),
        };
        if !requirements_met {
            return Err(facade_error(
                ExtensionErrorCode::FeatureUnavailable,
                format!("required feature set for {method} is not compiled"),
                Retryability::Never,
                method,
                FacadeFailureDetail {
                    operation: Some(request.operation.clone()),
                    required_feature: summary.required_feature.clone(),
                    ..FacadeFailureDetail::default()
                },
            ));
        }
        (
            summary.family.clone(),
            summary.required_feature.clone(),
            None,
        )
    };
    if let Some(feature) = &required_feature
        && !state.advertisement.features.iter().any(|f| f == feature)
    {
        return Err(facade_error(
            ExtensionErrorCode::FeatureUnavailable,
            format!("feature {feature} is not compiled into this Host"),
            Retryability::Never,
            method,
            FacadeFailureDetail {
                required_feature: Some(feature.clone()),
                ..FacadeFailureDetail::default()
            },
        ));
    }
    if let Some(handler_operation) = handler_operation {
        if matches!(
            family.as_str(),
            "workflow"
                | "delivery"
                | "trace"
                | "permission"
                | "mcp"
                | "a2a"
                | "lsp"
                | "channels"
                | "topology"
                | "project_rules"
        ) {
            let agent = request
                .handle
                .as_ref()
                .ok_or_else(|| {
                    facade_error(
                        ExtensionErrorCode::InvalidValue,
                        "source family operation requires an Agent receiver",
                        Retryability::Never,
                        method,
                        FacadeFailureDetail {
                            operation: Some(request.operation.clone()),
                            ..FacadeFailureDetail::default()
                        },
                    )
                })?
                .clone();
            state
                .handles
                .check_shape_and_generation(&agent, HandleKind::Agent, method)?;
            state.handles.agent(&agent)?;
            let session = match request.arguments.first() {
                Some(WireValue::Handle(handle)) if handle.kind == HandleKind::Session => {
                    handle.clone()
                }
                _ => {
                    return Err(facade_error(
                        ExtensionErrorCode::InvalidValue,
                        "source family operation requires a Session receiver argument",
                        Retryability::Never,
                        method,
                        FacadeFailureDetail {
                            operation: Some(request.operation.clone()),
                            ..FacadeFailureDetail::default()
                        },
                    ));
                }
            };
            state
                .handles
                .check_shape_and_generation(&session, HandleKind::Session, method)?;
            let session_record = state.handles.session(&session)?;
            if session_record.agent_handle_id != agent.id {
                return Err(facade_error(
                    ExtensionErrorCode::InvalidValue,
                    "source family Session does not belong to the Agent receiver",
                    Retryability::Never,
                    method,
                    FacadeFailureDetail {
                        operation: Some(request.operation.clone()),
                        ..FacadeFailureDetail::default()
                    },
                ));
            }
            request.handle = Some(session);
            request.arguments.remove(0);
            for argument in &mut request.arguments {
                if let WireValue::Handle(handle) = argument {
                    let value = serde_json::to_value(handle.clone()).map_err(|error| {
                        facade_error(
                            ExtensionErrorCode::FrameworkError,
                            format!("source family resource handle projection failed: {error}"),
                            Retryability::Never,
                            method,
                            FacadeFailureDetail {
                                operation: Some(request.operation.clone()),
                                ..FacadeFailureDetail::default()
                            },
                        )
                    })?;
                    *argument = WireValue::from_json(value).map_err(|error| {
                        facade_error(
                            ExtensionErrorCode::FrameworkError,
                            format!("source family resource handle projection failed: {error}"),
                            Retryability::Never,
                            method,
                            FacadeFailureDetail {
                                operation: Some(request.operation.clone()),
                                ..FacadeFailureDetail::default()
                            },
                        )
                    })?;
                }
            }
        }
        request.operation = handler_operation;
    }
    // Structured-output validation is contract-level and self-contained:
    // it ships with the facade runtime itself (todo 3).
    if method == "_echo_agent/structured_output/validate" {
        return match structured_output::validate(&request) {
            Ok(value) => Ok(echo_sdk_protocol::methods::FeatureOperationResponse { value }),
            Err(error) => Err(error),
        };
    }
    // Family dispatch (todos 4–5): compiled families route to their real
    // framework services; the rest answer with the typed feature failure
    // instead of simulating a result. Families that need a session resolve
    // its authorities here; resource-free families (state, eval, improve)
    // dispatch without one.
    let session = session_authorities_of(state, &request).await;
    let response = move |value: WireResult| {
        value.map(|value| echo_sdk_protocol::methods::FeatureOperationResponse { value })
    };
    let missing_session = || {
        facade_error(
            ExtensionErrorCode::InvalidValue,
            "family operation requires a session handle",
            Retryability::Never,
            method,
            FacadeFailureDetail {
                operation: Some(request.operation.clone()),
                ..FacadeFailureDetail::default()
            },
        )
    };
    let limits = FacadeFamilyLimits {
        max_resources: state.limits.max_facade_resources,
        page: state.limits.max_facade_page_items,
    };
    match family.as_str() {
        "invoke" if request.operation == "facade.resource.close" => {
            let (_authorities, owner) = session.ok_or_else(missing_session)?;
            if request.arguments.len() != 1 {
                return Err(facade_error(
                    ExtensionErrorCode::InvalidValue,
                    "facade.resource.close accepts exactly one FacadeResource argument",
                    Retryability::Never,
                    method,
                    FacadeFailureDetail {
                        operation: Some(request.operation.clone()),
                        ..FacadeFailureDetail::default()
                    },
                ));
            }
            let resource = match request.arguments.first() {
                Some(WireValue::Handle(handle)) => handle,
                _ => {
                    return Err(facade_error(
                        ExtensionErrorCode::InvalidValue,
                        "facade.resource.close requires a FacadeResource handle",
                        Retryability::Never,
                        method,
                        FacadeFailureDetail {
                            operation: Some(request.operation.clone()),
                            ..FacadeFailureDetail::default()
                        },
                    ));
                }
            };
            response(
                state
                    .facade
                    .close_resource(&state.handles, &owner, resource, &request.operation)
                    .await
                    .map(WireValue::Bool),
            )
        }
        "workflow" | "a2a" | "shell"
            if request.operation.ends_with(".stream.next")
                || request.operation.ends_with(".stream.cancel")
                || request.operation.ends_with(".stream.close") =>
        {
            let (_authorities, owner) = session.ok_or_else(missing_session)?;
            response(
                stream::dispatch_control(
                    &state.facade.streams,
                    &state.handles,
                    &owner,
                    &family,
                    &request,
                )
                .await,
            )
        }
        "memory" => {
            let (authorities, owner) = session.ok_or_else(missing_session)?;
            response(
                memory::dispatch(
                    &authorities,
                    &state.facade.memory_store_resources,
                    &state.handles,
                    &owner,
                    &request,
                    limits.page,
                )
                .await,
            )
        }
        "workflow" => {
            let (authorities, owner) = session.ok_or_else(missing_session)?;
            #[cfg(feature = "sdk-extension-bridge")]
            if matches!(
                request.operation.as_str(),
                "workflow.extension.run" | "workflow.extension.run_stream"
            ) {
                use echo_agent::workflow::Workflow as _;
                let extension = match request.arguments.first() {
                    Some(WireValue::Handle(handle)) if handle.kind == HandleKind::Extension => {
                        handle.clone()
                    }
                    _ => {
                        return Err(facade_error(
                            ExtensionErrorCode::InvalidValue,
                            "workflow.extension.run requires a Workflow extension handle",
                            Retryability::Never,
                            method,
                            FacadeFailureDetail::default(),
                        ));
                    }
                };
                let input = match request.arguments.get(1) {
                    Some(WireValue::String(input)) => input.clone(),
                    _ => {
                        return Err(facade_error(
                            ExtensionErrorCode::InvalidValue,
                            "workflow.extension.run requires input text",
                            Retryability::Never,
                            method,
                            FacadeFailureDetail::default(),
                        ));
                    }
                };
                let mut proxy =
                    crate::core_profile::extension_bridge::ExtensionAgentComponentProxy::new(
                        state.extension_bridge.clone(),
                        extension,
                        owner.clone(),
                    )
                    .filter(|proxy| {
                        proxy.component()
                            == echo_sdk_protocol::methods::AgentComponentKindWire::Workflow
                    })
                    .ok_or_else(|| {
                        facade_error(
                            ExtensionErrorCode::InvalidValue,
                            "extension is not a Workflow component",
                            Retryability::Never,
                            method,
                            FacadeFailureDetail::default(),
                        )
                    })?;
                if request.operation == "workflow.extension.run_stream" {
                    use futures::StreamExt as _;
                    let (anchor, _) = state.handles.register_facade_resource(
                        state.limits.max_facade_resources,
                        "workflow",
                        "workflow.extension_stream",
                        Some(&owner),
                        method,
                    )?;
                    let producer = match state.facade.streams.open_ephemeral(
                        &state.handles,
                        &anchor,
                        &owner,
                        &request.operation,
                    ) {
                        Ok(producer) => producer,
                        Err(error) => {
                            let _ = state.handles.close_facade_resource(&anchor, method);
                            return Err(error);
                        }
                    };
                    let sender = producer.sender.clone();
                    let cancel = producer.cancel.clone();
                    let background = tokio::spawn(async move {
                        let source = proxy.run_stream(&input).await;
                        let mut source = match source {
                            Ok(source) => source,
                            Err(error) => {
                                let _ = sender
                                    .send(Err(wire::sdk_error(
                                        ExtensionErrorCode::FrameworkError,
                                        wire::bounded_framework_message(&error.to_string()),
                                        Retryability::Never,
                                        "_echo_agent/workflow/op",
                                    )))
                                    .await;
                                return;
                            }
                        };
                        loop {
                            let item = tokio::select! {
                                () = cancel.cancelled() => break,
                                item = source.next() => item,
                            };
                            let Some(item) = item else { break };
                            let item = item
                                .map_err(|error| {
                                    wire::sdk_error(
                                        ExtensionErrorCode::FrameworkError,
                                        wire::bounded_framework_message(&error.to_string()),
                                        Retryability::Never,
                                        "_echo_agent/workflow/op",
                                    )
                                })
                                .and_then(workflow::workflow_event_value);
                            if sender.send(item).await.is_err() {
                                break;
                            }
                        }
                    });
                    state
                        .facade
                        .streams
                        .attach_background(&producer, background);
                    return response(Ok(WireValue::Handle(producer.handle)));
                }
                let output = proxy.run(&input).await.map_err(|error| {
                    wire::sdk_error(
                        ExtensionErrorCode::FrameworkError,
                        wire::bounded_framework_message(&error.to_string()),
                        Retryability::Never,
                        method,
                    )
                })?;
                let output = serde_json::to_value(output).map_err(|error| {
                    wire::sdk_error(
                        ExtensionErrorCode::SerializationViolation,
                        error.to_string(),
                        Retryability::Never,
                        method,
                    )
                })?;
                return response(WireValue::from_json(output).map_err(|error| {
                    wire::sdk_error(
                        ExtensionErrorCode::SerializationViolation,
                        error.to_string(),
                        Retryability::Never,
                        method,
                    )
                }));
            }
            #[cfg(feature = "sdk-extension-bridge")]
            let checkpoint_store =
                crate::core_profile::extension_bridge::latest_agent_component_proxy(
                    state,
                    state.extension_bridge.clone(),
                    &owner,
                    echo_sdk_protocol::methods::AgentComponentKindWire::WorkflowCheckpointStore,
                )
                .map(|proxy| Arc::new(proxy) as Arc<dyn echo_agent::workflow::CheckpointStore>);
            #[cfg(not(feature = "sdk-extension-bridge"))]
            let checkpoint_store = None;
            response(
                workflow::dispatch(
                    &state.handles,
                    &state.facade.workflow_graphs,
                    &state.facade.workflow_states,
                    &state.facade.streams,
                    &authorities,
                    checkpoint_store,
                    &owner,
                    &request,
                    limits,
                )
                .await,
            )
        }
        "state" => response(state_delivery::dispatch_state(&state.state_store, &request).await),
        #[cfg(feature = "framework-telemetry")]
        "telemetry" => response(telemetry::dispatch(&request)),
        "delivery" => {
            let (_authorities, owner) = session.ok_or_else(missing_session)?;
            response(
                state_delivery::dispatch_delivery(
                    &state.handles,
                    &state.facade.delivery_ledgers,
                    &owner,
                    &request,
                    limits,
                )
                .await,
            )
        }
        "trace" => {
            let (_authorities, owner) = session.ok_or_else(missing_session)?;
            response(
                observability::dispatch_trace(
                    &state.handles,
                    &state.facade.trace_stores,
                    &owner,
                    &request,
                    limits,
                )
                .await,
            )
        }
        #[cfg(feature = "framework-eval")]
        "eval" => response(observability::dispatch_eval(&request).await),
        #[cfg(feature = "framework-improve")]
        "improve" => response(observability::dispatch_improve(&request).await),
        #[cfg(feature = "framework-human-loop")]
        "permission" => {
            let (_authorities, _owner) = session.ok_or_else(missing_session)?;
            response(permission::dispatch(state, &request).await)
        }
        #[cfg(feature = "framework-content-guard")]
        "content_guard" => response(tools::dispatch_content_guard(&request)),
        #[cfg(feature = "framework-project-rules")]
        "project_rules" => {
            let (authorities, _owner) = session.ok_or_else(missing_session)?;
            response(tools::dispatch_project_rules(
                &authorities.working_dir,
                &request,
            ))
        }
        "mcp" | "a2a" | "lsp" | "topology" => {
            let (_authorities, owner) = session.ok_or_else(missing_session)?;
            response(
                integrations::dispatch(
                    &state.handles,
                    &family,
                    &state.facade.integrations,
                    &state.facade.streams,
                    &owner,
                    &request,
                    limits.max_resources,
                )
                .await,
            )
        }
        #[cfg(all(feature = "framework-channels", feature = "sdk-extension-bridge"))]
        "channels" => {
            let (_authorities, owner) = session.ok_or_else(missing_session)?;
            response(
                integrations::dispatch_channels(
                    &state.handles,
                    &state.facade.integrations,
                    &state.extension_bridge,
                    &owner,
                    &request,
                    limits.max_resources,
                )
                .await,
            )
        }
        #[cfg(feature = "sdk-extension-bridge")]
        "shell" if request.operation == "shell.sandbox.extension.run_stream" => {
            use echo_agent::sandbox::SandboxExecutor as _;
            use futures::StreamExt as _;
            let (_authorities, owner) = session.ok_or_else(missing_session)?;
            if request.arguments.len() != 2 {
                return Err(facade_error(
                    ExtensionErrorCode::InvalidValue,
                    "sandbox.extension.run_stream requires [SandboxExecutor extension, command]",
                    Retryability::Never,
                    method,
                    FacadeFailureDetail::default(),
                ));
            }
            let extension = match request.arguments.first() {
                Some(WireValue::Handle(handle)) if handle.kind == HandleKind::Extension => {
                    handle.clone()
                }
                _ => {
                    return Err(facade_error(
                        ExtensionErrorCode::InvalidValue,
                        "sandbox.extension.run_stream requires an Extension handle",
                        Retryability::Never,
                        method,
                        FacadeFailureDetail::default(),
                    ));
                }
            };
            let command = request
                .arguments
                .get(1)
                .cloned()
                .ok_or_else(|| {
                    facade_error(
                        ExtensionErrorCode::InvalidValue,
                        "sandbox command is missing",
                        Retryability::Never,
                        method,
                        FacadeFailureDetail::default(),
                    )
                })?
                .into_json()
                .map_err(|error| {
                    facade_error(
                        ExtensionErrorCode::InvalidValue,
                        error.to_string(),
                        Retryability::Never,
                        method,
                        FacadeFailureDetail::default(),
                    )
                })
                .and_then(|value| {
                    serde_json::from_value::<echo_agent::sandbox::SandboxCommand>(value).map_err(
                        |error| {
                            facade_error(
                                ExtensionErrorCode::InvalidValue,
                                format!("sandbox command is malformed: {error}"),
                                Retryability::Never,
                                method,
                                FacadeFailureDetail::default(),
                            )
                        },
                    )
                })?;
            let proxy = crate::core_profile::extension_bridge::ExtensionAgentComponentProxy::new(
                state.extension_bridge.clone(),
                extension,
                owner.clone(),
            )
            .filter(|proxy| {
                proxy.component()
                    == echo_sdk_protocol::methods::AgentComponentKindWire::SandboxExecutor
            })
            .ok_or_else(|| {
                facade_error(
                    ExtensionErrorCode::InvalidValue,
                    "extension is not a SandboxExecutor component",
                    Retryability::Never,
                    method,
                    FacadeFailureDetail::default(),
                )
            })?;
            let (anchor, _) = state.handles.register_facade_resource(
                state.limits.max_facade_resources,
                "shell",
                "shell.sandbox_extension_stream",
                Some(&owner),
                method,
            )?;
            let producer = match state.facade.streams.open_ephemeral(
                &state.handles,
                &anchor,
                &owner,
                &request.operation,
            ) {
                Ok(producer) => producer,
                Err(error) => {
                    let _ = state.handles.close_facade_resource(&anchor, method);
                    return Err(error);
                }
            };
            let sender = producer.sender.clone();
            let cancel = producer.cancel.clone();
            let background = tokio::spawn(async move {
                let source = proxy.execute_stream(command).await;
                let mut source = match source {
                    Ok(source) => source,
                    Err(error) => {
                        let _ = sender
                            .send(Err(wire::sdk_error(
                                ExtensionErrorCode::FrameworkError,
                                wire::bounded_framework_message(&error.to_string()),
                                Retryability::Never,
                                "_echo_agent/shell/op",
                            )))
                            .await;
                        return;
                    }
                };
                loop {
                    let item = tokio::select! {
                        () = cancel.cancelled() => break,
                        item = source.next() => item,
                    };
                    let Some(event) = item else { break };
                    let item = serde_json::to_value(event)
                        .map_err(|error| {
                            wire::sdk_error(
                                ExtensionErrorCode::SerializationViolation,
                                error.to_string(),
                                Retryability::Never,
                                "_echo_agent/shell/op",
                            )
                        })
                        .and_then(|value| {
                            WireValue::from_json(value).map_err(|error| {
                                wire::sdk_error(
                                    ExtensionErrorCode::SerializationViolation,
                                    error.to_string(),
                                    Retryability::Never,
                                    "_echo_agent/shell/op",
                                )
                            })
                        });
                    if sender.send(item).await.is_err() {
                        break;
                    }
                }
            });
            state
                .facade
                .streams
                .attach_background(&producer, background);
            response(Ok(WireValue::Handle(producer.handle)))
        }
        "source_operation" => {
            response(source_operations::dispatch(state, &state.handles, &request).await)
        }
        tool_family if tools::TOOL_FAMILIES.contains(&tool_family) => {
            let (authorities, owner) = session.ok_or_else(missing_session)?;
            response(
                tools::dispatch_tool(tool_family, &authorities, &owner, &state.facade, &request)
                    .await,
            )
        }
        _ => Err(facade_error(
            ExtensionErrorCode::FeatureUnavailable,
            format!("facade family {family} is not available in this Host build"),
            Retryability::Never,
            method,
            FacadeFailureDetail {
                required_feature,
                ..FacadeFailureDetail::default()
            },
        )),
    }
}

/// Resolve the session authority services addressed by one family
/// operation, together with the owning ACP session id (workflow-family
/// resources are owner-checked against it). Family operations address
/// their session through the request handle (kind `session`); without it
/// the Host refuses to guess.
async fn session_authorities_of(
    state: &CoreProfileState,
    request: &echo_sdk_protocol::methods::FeatureOperationRequest,
) -> Option<(
    std::sync::Arc<crate::factory::SessionAuthorityServices>,
    String,
)> {
    let handle = request.handle.as_ref()?;
    if state
        .handles
        .check_shape_and_generation(
            handle,
            echo_sdk_protocol::handle::HandleKind::Session,
            "_echo_agent/facade/invoke",
        )
        .is_err()
    {
        return None;
    }
    let services = state.services().ok()?;
    let acp_session_id = state.handles.session(handle).ok()?.acp_session_id.clone();
    let _ = services;
    let authorities = state.session_factory.session_services(&acp_session_id)?;
    Some((authorities, acp_session_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_compiled_methods_are_owned() {
        // The dispatcher owns the generic invoke surface, structured-output
        // validation and every compiled family handler (todo 4 stateful
        // families; eval/improve follow their Cargo features).
        assert!(owns_method("_echo_agent/facade/invoke"));
        assert!(owns_method("_echo_agent/memory/op"));
        assert!(owns_method("_echo_agent/workflow/op"));
        assert!(owns_method("_echo_agent/state/op"));
        assert!(owns_method("_echo_agent/delivery/op"));
        assert!(owns_method("_echo_agent/trace/op"));
        assert_eq!(
            owns_method("_echo_agent/eval/op"),
            cfg!(feature = "framework-eval")
        );
        assert_eq!(
            owns_method("_echo_agent/improve/op"),
            cfg!(feature = "framework-improve")
        );
        // Family ownership follows the compiled adapter authority. Channels
        // additionally require the reverse bridge because their plugins and
        // handlers are consumer implementations; testing has no facade
        // adapter and remains method-not-found.
        assert_eq!(
            owns_method("_echo_agent/channels/op"),
            cfg!(all(
                feature = "framework-channels",
                feature = "sdk-extension-bridge"
            ))
        );
        assert_eq!(
            owns_method("_echo_agent/telemetry/op"),
            cfg!(feature = "framework-telemetry")
        );
        assert!(!owns_method("_echo_agent/testing/op"));
        assert!(!owns_method("_echo_agent/task/create"));
        assert!(!owns_method("session/prompt"));
        assert!(!owns_method("_echo_agent/agent/create"));
    }

    #[cfg(feature = "framework-rag")]
    #[tokio::test]
    async fn rag_tools_are_removed_with_session_and_connection_close() {
        let runtime = SessionFacadeRuntime::new(&SdkProfileLimits::default());
        runtime
            .rag_tools
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(
                "session-a".to_string(),
                Arc::new(tools::RagToolSet::for_test()),
            );
        runtime
            .rag_tools
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(
                "session-b".to_string(),
                Arc::new(tools::RagToolSet::for_test()),
            );

        runtime.drop_session_resources_of("session-a", &[]).await;
        {
            let rag_tools = runtime
                .rag_tools
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            assert!(!rag_tools.contains_key("session-a"));
            assert!(rag_tools.contains_key("session-b"));
        }

        runtime.close_all(&HandleRegistry::new(1, 8)).await;
        assert!(
            runtime
                .rag_tools
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .is_empty()
        );
    }
}
