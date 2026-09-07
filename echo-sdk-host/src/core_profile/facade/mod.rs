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
//! 6. family dispatch — todos 3–5 route to the real framework services;
//!    until a family lands, admitted operations answer with a typed
//!    `feature_unavailable` (never a partial or simulated result).

pub(crate) mod integrations;
pub(crate) mod memory;
pub(crate) mod observability;
pub(crate) mod registry;
pub(crate) mod state_delivery;
pub(crate) mod stream;
pub(crate) mod structured_output;
#[cfg(feature = "framework-subagent")]
pub(crate) mod subagent;
pub(crate) mod task;
#[cfg(feature = "framework-subagent")]
pub(crate) mod task_runtime;
pub(crate) mod tools;
pub(crate) mod workflow;

use agent_client_protocol::{Client, ConnectionTo, Dispatch, Error, HandleDispatchFrom, Handled};
use echo_sdk_protocol::capability::ExtensionCapability;
use echo_sdk_protocol::error::{
    EchoSdkError, ErrorDetails, ExtensionErrorCode, FacadeFailureDetail, Retryability,
};
use echo_sdk_protocol::methods::{FeatureOperationRequest, FeatureOperationResponse};
use echo_sdk_protocol::scalar::WireValue;
use std::sync::Arc;

/// One family handler outcome: the typed wire result or typed failure.
type WireResult = Result<WireValue, EchoSdkError>;

use super::state::CoreProfileState;
use super::wire;
use crate::config::SdkProfileLimits;
use registry::CompiledOperationCatalog;
use stream::{FacadeStreamBookkeeping, FacadeStreamLimits};

/// Connection-level facade runtime: stream bookkeeping, the resource handle
/// surface in [`super::handles::HandleRegistry`] and the RPC subagent
/// dispatch records. The Host manages addressing and lifecycle only;
/// business state stays with the Rust framework services.
pub(crate) struct SessionFacadeRuntime {
    // Family handlers (todos 3-5) open, advance and close their streams
    // through this bookkeeping; the admission ladder itself never touches
    // stream state.
    #[allow(dead_code)]
    pub streams: FacadeStreamBookkeeping,
    #[cfg(feature = "framework-subagent")]
    subagents:
        std::sync::Mutex<std::collections::HashMap<String, Arc<subagent::SubagentDispatchRecord>>>,
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
    // Integration family resources (todo 5): MCP managers, A2A clients,
    // LSP managers and topology trackers, owner-checked per session.
    pub integrations: integrations::IntegrationResources,
}

impl SessionFacadeRuntime {
    pub fn new(limits: &SdkProfileLimits) -> Self {
        Self {
            streams: FacadeStreamBookkeeping::new(FacadeStreamLimits {
                max_open_streams: limits.max_facade_streams,
                max_page_items: limits.max_facade_page_items,
            }),
            #[cfg(feature = "framework-subagent")]
            subagents: std::sync::Mutex::new(std::collections::HashMap::new()),
            #[cfg(feature = "framework-subagent")]
            task_executions: std::sync::Mutex::new(std::collections::HashMap::new()),
            workflow_graphs: std::sync::Mutex::new(std::collections::HashMap::new()),
            workflow_states: std::sync::Mutex::new(std::collections::HashMap::new()),
            delivery_ledgers: std::sync::Mutex::new(std::collections::HashMap::new()),
            trace_stores: std::sync::Mutex::new(std::collections::HashMap::new()),
            integrations: integrations::IntegrationResources::new(),
        }
    }

    /// Register one RPC-dispatched subagent record by execution id.
    #[cfg(feature = "framework-subagent")]
    pub fn register_subagent(
        &self,
        execution_id: String,
        record: Arc<subagent::SubagentDispatchRecord>,
    ) {
        let mut subagents = self
            .subagents
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if subagents.len() >= 512 {
            // Retire settled records first; the bound keeps a chatty client
            // from growing the map without end.
            subagents.retain(|_, record| record.settled.try_lock().is_ok_and(|set| set.is_none()));
        }
        subagents.insert(execution_id, record);
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
    /// run-level pause/cancel through the shared cancel token.
    #[cfg(feature = "framework-subagent")]
    pub fn register_task_execution(&self, scope: String, execution: TaskRunExecution) {
        self.task_executions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(scope, execution);
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

    /// Drop and cancel every facade resource owned by one session; called
    /// when the session closes so family resources never outlive their
    /// owner.
    pub fn drop_workflow_resources_of(&self, owner: &str) {
        workflow::drop_session_resources(&self.workflow_graphs, &self.workflow_states, owner);
        state_delivery::drop_session_ledgers(&self.delivery_ledgers, owner);
        observability::drop_session_stores(&self.trace_stores, owner);
        integrations::drop_session_resources(&self.integrations, owner);
    }
}

/// A live task-graph execution (cheap clone: token + controller Arcs).
#[cfg(feature = "framework-subagent")]
#[derive(Clone)]
pub(crate) struct TaskRunExecution {
    pub cancel: tokio_util::sync::CancellationToken,
    pub controller: Arc<task_runtime::FacadeTaskController>,
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
        _connection: ConnectionTo<Client>,
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
        // Never block the dispatch loop: the ladder itself is fast, but the
        // spawn keeps the official single-task rule uniform with the typed
        // handlers and leaves room for family work (todos 3–5).
        tokio::spawn(async move {
            let dispatcher = FacadeDispatcher { state };
            let respond = |result: Result<FeatureOperationResponse, Error>| match result {
                Ok(response) => match serde_json::to_value(response) {
                    Ok(value) => responder.respond(value),
                    Err(error) => responder.respond_with_error(
                        Error::internal_error()
                            .data(format!("facade response encoding failed: {error}")),
                    ),
                },
                Err(error) => responder.respond_with_error(error),
            };
            if let Err(error) = dispatcher
                .dispatch(&method_for_task, &params, respond)
                .await
            {
                tracing::warn!("facade dispatch for {method_for_task} failed: {error}");
            }
        });
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
    let request: FeatureOperationRequest =
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
    let catalog = CompiledOperationCatalog::global().map_err(|reason| {
        EchoSdkError::new(
            ExtensionErrorCode::InvalidConfig,
            reason,
            Retryability::Never,
        )
    })?;
    let (family, required_feature) = if method == "_echo_agent/facade/invoke" {
        let route = catalog.invoke_route(&request.operation).ok_or_else(|| {
            facade_error(
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
            )
        })?;
        (route.family.clone(), route.required_feature.clone())
    } else {
        let summary = catalog.family_method(method).ok_or_else(|| {
            EchoSdkError::new(
                ExtensionErrorCode::InvalidRequest,
                format!("method {method} is not a facade family surface"),
                Retryability::Never,
            )
        })?;
        (summary.family.clone(), summary.required_feature.clone())
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
    let page = state.limits.max_facade_page_items;
    match family.as_str() {
        "memory" => {
            let (authorities, _owner) = session.ok_or_else(missing_session)?;
            response(memory::dispatch(&authorities, &request, page).await)
        }
        "workflow" => {
            let (authorities, owner) = session.ok_or_else(missing_session)?;
            response(
                workflow::dispatch(
                    &state.facade.workflow_graphs,
                    &state.facade.workflow_states,
                    &authorities,
                    &owner,
                    &request,
                    page,
                )
                .await,
            )
        }
        "state" => response(state_delivery::dispatch_state(&state.state_store, &request).await),
        "delivery" => {
            let (_authorities, owner) = session.ok_or_else(missing_session)?;
            response(
                state_delivery::dispatch_delivery(
                    &state.facade.delivery_ledgers,
                    &owner,
                    &request,
                    page,
                )
                .await,
            )
        }
        "trace" => {
            let (_authorities, owner) = session.ok_or_else(missing_session)?;
            response(
                observability::dispatch_trace(&state.facade.trace_stores, &owner, &request, page)
                    .await,
            )
        }
        #[cfg(feature = "framework-eval")]
        "eval" => response(observability::dispatch_eval(&request).await),
        #[cfg(feature = "framework-improve")]
        "improve" => response(observability::dispatch_improve(&request).await),
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
                integrations::dispatch(&family, &state.facade.integrations, &owner, &request).await,
            )
        }
        tool_family if tools::TOOL_FAMILIES.contains(&tool_family) => {
            let (authorities, _owner) = session.ok_or_else(missing_session)?;
            response(tools::dispatch_tool(tool_family, &authorities.working_dir, &request).await)
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
    if handle.kind != echo_sdk_protocol::handle::HandleKind::Session {
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
        assert!(!owns_method("_echo_agent/task/create"));
        assert!(!owns_method("session/prompt"));
        assert!(!owns_method("_echo_agent/agent/create"));
    }
}
