//! Team intent compiled onto the canonical revisioned task runtime.
//!
//! Declarative [`TeamSpec`] values and programmatic [`Team`] values both become
//! one revisioned task graph. Programmatic composition may own Agent handles,
//! but dependency state, ready-frontier selection, cancellation, and terminal
//! settlement remain exclusively owned by [`RuntimeTaskService`].

mod manager_subagent;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use echo_core::agent::Agent;
use echo_core::error::{AgentError, AgentFailure, AgentTerminalKind, ReactError, Result};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, OnceCell};
use tokio_util::sync::CancellationToken;

use echo_orchestration::tasks::{
    DefaultTaskToolPolicy, InMemoryRevisionedTaskStore, NestedDelegationPolicy,
    RevisionedTaskGraph, RevisionedTaskStore, RuntimeAttemptControlCleanupReceipt,
    RuntimeClaimAbandonment, RuntimeDagController, RuntimeDagOutcome,
    RuntimeInterruptionDisposition, RuntimeInterruptionSettlementOutcome, RuntimePlanSnapshot,
    RuntimeTaskAttemptInterruptDisposition, RuntimeTaskAttemptInterruptProjectionError,
    RuntimeTaskClaimOutcome, RuntimeTaskResolution, RuntimeTaskResolutionRequest,
    RuntimeTaskService, RuntimeTaskServiceConfig, Task, TaskClaim, TaskExecution, TaskGraphContext,
    TaskGraphExecutionMode, TaskPlanPatch, TaskPlanPatchOp, TaskRevisionError, TaskRevisionService,
    TaskSpec, TaskStatus, TaskSubagentContext,
};

use super::control::{
    SubagentAttemptIdentity, SubagentControlError, SubagentInterruptRequestDisposition,
};
use super::executor::{DispatchRequest, SubagentExecutor, SubagentExecutorConfig};
use super::registry::SubagentRegistry;
use super::types::{ExecutionMode, SubagentDefinition, SubagentResult, SubagentStatus};
use super::usage::LlmUsageStats;

/// How a Team of registered or programmatically supplied Subagents collaborates.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamStrategy {
    /// A manager produces a typed task plan, members execute it, and the manager
    /// synthesizes the completed outputs.
    #[default]
    ManagerSubagent,
    /// Registered Subagents execute in the specified order.
    Pipeline(Vec<String>),
    /// Debaters execute independently, then the judge synthesizes.
    Debate {
        judge: String,
        debaters: Vec<String>,
    },
    /// Declared members execute independently, then the reducer synthesizes.
    Swarm { reducer: String },
}

impl TeamStrategy {
    pub fn name(&self) -> &'static str {
        match self {
            Self::ManagerSubagent => "manager_subagent",
            Self::Pipeline(_) => "pipeline",
            Self::Debate { .. } => "debate",
            Self::Swarm { .. } => "swarm",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            Self::ManagerSubagent => {
                "Manager plans typed tasks, Subagents execute them, and the manager synthesizes"
            }
            Self::Pipeline(_) => "Subagents execute in sequence",
            Self::Debate { .. } => "Debaters propose independently and a judge synthesizes",
            Self::Swarm { .. } => "Subagents inspect independently and a reducer synthesizes",
        }
    }
}

/// Runtime limits for a Team graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamConfig {
    /// Maximum concurrent Subagent dispatches in one ready wave.
    pub max_concurrent: usize,
    /// Whole-Team execution timeout in seconds. Zero disables the timeout.
    #[serde(default = "default_team_timeout_secs")]
    pub default_timeout_secs: u64,
}

const fn default_team_timeout_secs() -> u64 {
    600
}

impl Default for TeamConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 5,
            default_timeout_secs: default_team_timeout_secs(),
        }
    }
}

/// Declarative Team intent. All names resolve through the shared Subagent registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamSpec {
    pub strategy: TeamStrategy,
    pub manager: String,
    pub subagents: Vec<String>,
    pub config: TeamConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TeamTaskExtension {
    member: String,
    #[serde(default)]
    phase: Option<String>,
}

/// Role of a programmatically supplied Team member.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamRole {
    Leader,
    Subagent,
    Reviewer,
}

/// One programmatically supplied Team member.
#[derive(Clone)]
pub struct TeamMember {
    pub name: String,
    pub role: TeamRole,
    pub agent: Arc<dyn Agent>,
    pub definition: SubagentDefinition,
    execution_gate: Arc<Mutex<()>>,
}

/// A reusable set of concrete Agent instances compiled through the canonical
/// Team graph runtime when executed.
#[derive(Clone)]
pub struct Team {
    pub id: String,
    pub name: String,
    pub config: TeamConfig,
    members: HashMap<String, TeamMember>,
}

impl Team {
    pub fn new(id: impl Into<String>, name: impl Into<String>, config: TeamConfig) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            config,
            members: HashMap::new(),
        }
    }

    pub fn add_member(
        &mut self,
        name: &str,
        role: TeamRole,
        agent: Box<dyn Agent>,
        definition: SubagentDefinition,
    ) -> std::result::Result<(), String> {
        self.add_shared_member(name, role, Arc::from(agent), definition)
    }

    pub fn add_shared_member(
        &mut self,
        name: &str,
        role: TeamRole,
        agent: Arc<dyn Agent>,
        mut definition: SubagentDefinition,
    ) -> std::result::Result<(), String> {
        if name.trim().is_empty() {
            return Err("Team member name cannot be empty".to_string());
        }
        if self.members.contains_key(name) {
            return Err(format!("Team member '{name}' is already registered"));
        }
        definition.name = name.to_string();
        self.members.insert(
            name.to_string(),
            TeamMember {
                name: name.to_string(),
                role,
                agent,
                definition,
                execution_gate: Arc::new(Mutex::new(())),
            },
        );
        Ok(())
    }

    pub fn get_member(&self, name: &str) -> Option<&TeamMember> {
        self.members.get(name)
    }

    pub fn member_names(&self) -> Vec<String> {
        let mut names = self.members.keys().cloned().collect::<Vec<_>>();
        names.sort();
        names
    }

    pub fn subagent_names(&self) -> Vec<String> {
        let mut names = self
            .members
            .values()
            .filter(|member| member.role == TeamRole::Subagent)
            .map(|member| member.name.clone())
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    pub fn len(&self) -> usize {
        self.members.len()
    }

    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    pub fn members(&self) -> impl Iterator<Item = &TeamMember> {
        self.members.values()
    }

    pub fn leader_name(&self) -> Option<&str> {
        self.members()
            .find(|member| member.role == TeamRole::Leader)
            .map(|member| member.name.as_str())
    }

    pub fn subagents(&self) -> impl Iterator<Item = &TeamMember> {
        self.members()
            .filter(|member| member.role == TeamRole::Subagent)
    }

    pub fn subagent_descriptions(&self) -> String {
        let mut descriptions = self
            .subagents()
            .map(|member| format!("- {}: {}", member.name, member.definition.description))
            .collect::<Vec<_>>();
        descriptions.sort();
        descriptions.join("\n")
    }

    fn to_spec(&self, strategy: TeamStrategy) -> std::result::Result<TeamSpec, String> {
        let mut leaders = self
            .members()
            .filter(|member| member.role == TeamRole::Leader)
            .map(|member| member.name.as_str())
            .collect::<Vec<_>>();
        leaders.sort_unstable();
        if strategy == TeamStrategy::ManagerSubagent && leaders.len() != 1 {
            return Err(format!(
                "Manager-Subagent Team requires exactly one manager; found {}",
                leaders.len()
            ));
        }
        let manager = leaders.first().copied().unwrap_or_default().to_string();
        let spec = TeamSpec {
            strategy,
            manager,
            subagents: self.subagent_names(),
            config: self.config.clone(),
        };
        validate_team_spec(&spec).map_err(|error| error.to_string())?;
        for name in referenced_member_names(&spec) {
            if !self.members.contains_key(name) {
                return Err(format!("Team member '{name}' is not registered"));
            }
        }
        Ok(spec)
    }

    async fn dispatch_controller(&self) -> Arc<dyn TeamDispatchController> {
        let members = self.members.clone();
        let registry = Arc::new(SubagentRegistry::new());
        for member in members.values() {
            registry
                .register_shared(member.definition.clone(), member.agent.clone())
                .await;
        }
        let executor = Arc::new(SubagentExecutor::new(
            registry,
            SubagentExecutorConfig {
                max_concurrent_forks: self.config.max_concurrent.max(1),
                default_timeout_secs: self.config.default_timeout_secs,
                ..SubagentExecutorConfig::default()
            },
        ));
        let control_scope_id = format!("team-control-{}", uuid::Uuid::new_v4().as_simple());
        let parent_agent = self.name.clone();
        let dispatch_executor = Arc::clone(&executor);
        let dispatch_scope_id = control_scope_id.clone();
        let dispatch: TeamDispatchFn = Arc::new(move |request| {
            let TeamDispatchRequest {
                member: name,
                task,
                context,
            } = request;
            let member = members.get(&name).cloned();
            let executor = Arc::clone(&dispatch_executor);
            let parent_agent = parent_agent.clone();
            let control_scope_id = dispatch_scope_id.clone();
            Box::pin(async move {
                let member = member
                    .ok_or_else(|| ReactError::Other(format!("Team member '{name}' not found")))?;
                let _execution = member.execution_gate.lock().await;
                let claim = context.claim().ok_or_else(|| {
                    ReactError::Other("Team member dispatch requires an exact claim".to_string())
                })?;
                let task_id = context.task_id().ok_or_else(|| {
                    ReactError::Other("Team member dispatch requires an exact task id".to_string())
                })?;
                let identity = SubagentAttemptIdentity::for_runtime_scope(
                    control_scope_id,
                    context.run_id().to_string(),
                    task_id.to_string(),
                    claim.execution_id(context.run_id(), task_id),
                    claim.attempt,
                )
                .map_err(|error| ReactError::Other(error.to_string()))?;
                executor
                    .dispatch_attempt(
                        DispatchRequest {
                            agent_name: name,
                            task,
                            mode_override: Some(ExecutionMode::Sync),
                            cancel: context.cancellation_token().clone(),
                            parent_agent,
                            parent_context: None,
                            delegation_policy: NestedDelegationPolicy::default(),
                            runtime_context: Some(context.runtime_context()),
                            message: None,
                            prompt_payload: None,
                            prompt_context: None,
                            constraints: Vec::new(),
                            background: false,
                        },
                        identity,
                    )
                    .await
            })
        });
        let reserve_executor = Arc::clone(&executor);
        let reserve_scope_id = control_scope_id.clone();
        let reserve_attempt: TeamReserveAttemptFn = Arc::new(move |context| {
            let identity = identity_from_context(&context, &reserve_scope_id, "Team reservation")?;
            reserve_executor
                .reserve_attempt(identity, context.cancellation_token().clone())
                .map_err(|error| error.to_string())
        });
        let interrupt_executor = Arc::clone(&executor);
        let interrupt_scope_id = control_scope_id.clone();
        let request_interrupt: TeamRequestInterruptFn = Arc::new(move |run_id, task_id, claim| {
            let identity = SubagentAttemptIdentity::for_runtime_scope(
                interrupt_scope_id.clone(),
                run_id.clone(),
                task_id.clone(),
                claim.execution_id(&run_id, &task_id),
                claim.attempt,
            )
            .map_err(runtime_interrupt_projection_error)?;
            interrupt_executor
                .request_interrupt(identity)
                .map(|receipt| Some(runtime_interrupt_disposition(receipt.disposition)))
                .map_err(runtime_interrupt_projection_error)
        });
        let reconcile_executor = Arc::clone(&executor);
        let reconcile_scope_id = control_scope_id.clone();
        let reconcile_attempts: TeamReconcileAttemptFn =
            Arc::new(move |_run_id, current_execution_ids| {
                reconcile_executor
                    .reconcile_attempt_control(&reconcile_scope_id, &current_execution_ids)
                    .map_err(|error| error.to_string())
            });
        let cleanup_executor = executor;
        let cleanup_scope_id = control_scope_id;
        let retire_attempt: TeamRetireAttemptFn = Arc::new(move |run_id, task, claim| {
            let cleanup_executor = Arc::clone(&cleanup_executor);
            let control_scope_id = cleanup_scope_id.clone();
            Box::pin(async move {
                let task_id = task.spec.id.clone();
                let identity = match SubagentAttemptIdentity::for_runtime_scope(
                    control_scope_id,
                    run_id.clone(),
                    task_id.clone(),
                    claim.execution_id(&run_id, &task_id),
                    claim.attempt,
                ) {
                    Ok(identity) => identity,
                    Err(error) => {
                        return RuntimeAttemptControlCleanupReceipt::RetryableFailure {
                            error: error.to_string(),
                        };
                    }
                };
                match cleanup_executor.retire_attempt_control(&identity) {
                    Ok(true) => RuntimeAttemptControlCleanupReceipt::Retired,
                    Ok(false) => RuntimeAttemptControlCleanupReceipt::AlreadyConsumed,
                    Err(error) => RuntimeAttemptControlCleanupReceipt::RetryableFailure {
                        error: error.to_string(),
                    },
                }
            })
        });
        closure_dispatch_controller(
            dispatch,
            TeamDispatchControl {
                reserve_attempt,
                request_interrupt,
                retire_attempt,
                reconcile_attempts,
            },
        )
    }
}

/// Inseparable binding between one Team runtime authority and the canonical
/// task service built from that exact `Arc<R>`.
pub struct TeamRuntimeServiceHandle<R: TeamRuntime> {
    runtime: Arc<R>,
    runtime_tasks: Arc<RuntimeTaskService<R>>,
}

impl<R: TeamRuntime> Clone for TeamRuntimeServiceHandle<R> {
    fn clone(&self) -> Self {
        Self {
            runtime: self.runtime.clone(),
            runtime_tasks: self.runtime_tasks.clone(),
        }
    }
}

impl<R: TeamRuntime> TeamRuntimeServiceHandle<R> {
    pub fn new(runtime: Arc<R>, config: RuntimeTaskServiceConfig) -> Self {
        let runtime_tasks = Arc::new(RuntimeTaskService::new(runtime.clone(), config));
        Self {
            runtime,
            runtime_tasks,
        }
    }

    pub fn runtime(&self) -> &Arc<R> {
        &self.runtime
    }

    pub fn runtime_tasks(&self) -> &Arc<RuntimeTaskService<R>> {
        &self.runtime_tasks
    }
}

/// Stable execution-time authority for one Team run.
///
/// The handle keeps the revisioned task store, canonical runtime service, and
/// live Subagent control registry together. Exact control therefore cannot be
/// accidentally issued through a fresh service that lacks the active attempt.
#[derive(Clone)]
pub struct TeamRuntimeHandle {
    handle_id: String,
    run_id: String,
    service: TeamRuntimeServiceHandle<TeamRuntimeController>,
    active_executions: Arc<AtomicUsize>,
}

pub(super) struct TeamRuntimeExecutionGuard {
    active_executions: Arc<AtomicUsize>,
}

impl Drop for TeamRuntimeExecutionGuard {
    fn drop(&mut self) {
        self.active_executions.fetch_sub(1, Ordering::SeqCst);
    }
}

impl TeamRuntimeHandle {
    fn new(
        run_id: String,
        runtime: Arc<TeamRuntimeController>,
        config: RuntimeTaskServiceConfig,
    ) -> Self {
        Self {
            handle_id: uuid::Uuid::new_v4().to_string(),
            run_id,
            service: TeamRuntimeServiceHandle::new(runtime, config),
            active_executions: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn handle_id(&self) -> &str {
        &self.handle_id
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    pub fn is_active(&self) -> bool {
        self.active_executions.load(Ordering::SeqCst) > 0
    }

    pub(super) fn start_execution(&self) -> TeamRuntimeExecutionGuard {
        self.active_executions.fetch_add(1, Ordering::SeqCst);
        TeamRuntimeExecutionGuard {
            active_executions: Arc::clone(&self.active_executions),
        }
    }

    pub async fn snapshot(&self) -> Result<RuntimePlanSnapshot> {
        self.service.runtime().load_snapshot(&self.run_id).await
    }

    pub async fn request_attempt_interrupt(
        &self,
        task_id: &str,
        claim: &TaskClaim,
    ) -> std::result::Result<
        echo_orchestration::tasks::RuntimeTaskAttemptInterruptReceipt,
        echo_orchestration::tasks::RuntimeTaskAttemptInterruptError,
    > {
        self.service
            .runtime_tasks()
            .request_attempt_interrupt(&self.run_id, task_id, claim)
            .await
    }

    pub(super) async fn execute(
        &self,
        spec: &TeamSpec,
        objective: &str,
        cancel: CancellationToken,
    ) -> Result<TeamExecutionResult> {
        let active = self.start_execution();
        self.execute_started(spec, objective, cancel, active).await
    }

    pub(super) async fn execute_started(
        &self,
        spec: &TeamSpec,
        objective: &str,
        cancel: CancellationToken,
        _active: TeamRuntimeExecutionGuard,
    ) -> Result<TeamExecutionResult> {
        execute_team_on_runtime_service(spec, objective, &self.run_id, cancel, &self.service).await
    }
}

pub(super) fn runtime_handle_for_controller(
    run_id: String,
    controller: Arc<dyn TeamDispatchController>,
    config: RuntimeTaskServiceConfig,
) -> Arc<TeamRuntimeHandle> {
    let runtime = Arc::new(TeamRuntimeController::with_controller(controller));
    Arc::new(TeamRuntimeHandle::new(run_id, runtime, config))
}

/// Programmatic Team facade. It owns only concrete Agent handles and delegates
/// all graph semantics to [`TeamRuntimeHandle`].
pub struct TeamAgent {
    team: Team,
    strategy: TeamStrategy,
    run_id: Option<String>,
    cancel: CancellationToken,
    member_controller: Option<Arc<dyn TeamDispatchController>>,
    runtime: OnceCell<Arc<TeamRuntimeHandle>>,
}

impl TeamAgent {
    pub fn new(team: Team, strategy: TeamStrategy) -> Self {
        Self {
            team,
            strategy,
            run_id: None,
            cancel: CancellationToken::new(),
            member_controller: None,
            runtime: OnceCell::new(),
        }
    }

    pub fn builder() -> TeamAgentBuilder {
        TeamAgentBuilder::new()
    }

    pub fn team(&self) -> &Team {
        &self.team
    }

    pub fn strategy(&self) -> &TeamStrategy {
        &self.strategy
    }

    pub fn run_id(&self) -> Option<&str> {
        self.runtime
            .get()
            .map(|handle| handle.run_id())
            .or(self.run_id.as_deref())
    }

    pub fn cancellation_token(&self) -> &CancellationToken {
        &self.cancel
    }

    pub async fn execute(&self, task: &str) -> std::result::Result<String, String> {
        self.execute_with_usage(task)
            .await
            .map(|result| result.output)
    }

    pub async fn execute_with_usage(
        &self,
        task: &str,
    ) -> std::result::Result<TeamExecutionResult, String> {
        let spec = self.team.to_spec(self.strategy.clone())?;
        let runtime = self.runtime_handle().await?;
        runtime
            .execute(&spec, task, self.cancel.child_token())
            .await
            .map_err(|error| error.to_string())
    }

    /// Return the stable runtime/control handle used by every execution of
    /// this TeamAgent. Calling this before `execute` prepares the same handle
    /// without starting task work.
    pub async fn runtime_handle(&self) -> std::result::Result<Arc<TeamRuntimeHandle>, String> {
        self.runtime
            .get_or_try_init(|| async {
                let run_id = self
                    .run_id
                    .clone()
                    .unwrap_or_else(|| format!("team-{}", uuid::Uuid::new_v4().as_simple()));
                let dispatch_controller = match self.member_controller.clone() {
                    Some(controller) => controller,
                    None => self.team.dispatch_controller().await,
                };
                Ok::<Arc<TeamRuntimeHandle>, String>(runtime_handle_for_controller(
                    run_id,
                    dispatch_controller,
                    RuntimeTaskServiceConfig {
                        max_concurrent_subagents: self.team.config.max_concurrent.max(1),
                        ..RuntimeTaskServiceConfig::default()
                    },
                ))
            })
            .await
            .cloned()
    }
}

/// Fluent programmatic Team builder retained as a framework composition API.
pub struct TeamAgentBuilder {
    name: String,
    members: Vec<(String, TeamRole, Arc<dyn Agent>, SubagentDefinition)>,
    strategy: TeamStrategy,
    config: TeamConfig,
    run_id: Option<String>,
    cancel: CancellationToken,
    member_controller: Option<Arc<dyn TeamDispatchController>>,
}

impl Default for TeamAgentBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl TeamAgentBuilder {
    pub fn new() -> Self {
        Self {
            name: "team".to_string(),
            members: Vec::new(),
            strategy: TeamStrategy::default(),
            config: TeamConfig::default(),
            run_id: None,
            cancel: CancellationToken::new(),
            member_controller: None,
        }
    }

    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    pub fn manager(
        self,
        name: &str,
        agent: Box<dyn Agent>,
        definition: SubagentDefinition,
    ) -> Self {
        self.manager_shared(name, Arc::from(agent), definition)
    }

    pub fn manager_shared(
        mut self,
        name: &str,
        agent: Arc<dyn Agent>,
        definition: SubagentDefinition,
    ) -> Self {
        self.members
            .push((name.to_string(), TeamRole::Leader, agent, definition));
        self
    }

    pub fn subagent(
        self,
        name: &str,
        agent: Box<dyn Agent>,
        definition: SubagentDefinition,
    ) -> Self {
        self.subagent_shared(name, Arc::from(agent), definition)
    }

    pub fn subagent_shared(
        mut self,
        name: &str,
        agent: Arc<dyn Agent>,
        definition: SubagentDefinition,
    ) -> Self {
        self.members
            .push((name.to_string(), TeamRole::Subagent, agent, definition));
        self
    }

    pub fn reviewer(
        self,
        name: &str,
        agent: Box<dyn Agent>,
        definition: SubagentDefinition,
    ) -> Self {
        self.reviewer_shared(name, Arc::from(agent), definition)
    }

    pub fn reviewer_shared(
        mut self,
        name: &str,
        agent: Arc<dyn Agent>,
        definition: SubagentDefinition,
    ) -> Self {
        self.members
            .push((name.to_string(), TeamRole::Reviewer, agent, definition));
        self
    }

    pub fn strategy(mut self, strategy: TeamStrategy) -> Self {
        self.strategy = strategy;
        self
    }

    pub fn timeout_secs(mut self, timeout_secs: u64) -> Self {
        self.config.default_timeout_secs = timeout_secs;
        self
    }

    pub fn config(mut self, config: TeamConfig) -> Self {
        self.config = config;
        self
    }

    pub fn cancel(mut self, cancel: CancellationToken) -> Self {
        self.cancel = cancel;
        self
    }

    pub fn member_dispatch_controller(
        mut self,
        controller: Arc<dyn TeamDispatchController>,
    ) -> Self {
        self.member_controller = Some(controller);
        self
    }

    pub fn run_id(mut self, run_id: Option<String>) -> Self {
        self.run_id = run_id;
        self
    }

    pub fn build(self) -> std::result::Result<TeamAgent, String> {
        let mut team = Team::new(
            format!("team-{}", uuid::Uuid::new_v4().as_simple()),
            self.name,
            self.config,
        );
        for (name, role, agent, definition) in self.members {
            team.add_shared_member(&name, role, agent, definition)?;
        }
        let mut team_agent = TeamAgent::new(team, self.strategy);
        team_agent.run_id = self.run_id;
        team_agent.cancel = self.cancel;
        team_agent.member_controller = self.member_controller;
        Ok(team_agent)
    }
}

/// Structured request sent to one exact Team member attempt.
#[derive(Debug, Clone)]
pub struct TeamDispatchRequest {
    pub member: String,
    pub task: String,
    pub context: TaskSubagentContext,
}

fn identity_from_context(
    context: &TaskSubagentContext,
    control_scope_id: &str,
    operation: &str,
) -> std::result::Result<SubagentAttemptIdentity, String> {
    let claim = context
        .claim()
        .ok_or_else(|| format!("{operation} requires an exact TaskClaim"))?;
    let task_id = context
        .task_id()
        .ok_or_else(|| format!("{operation} requires an exact task id"))?;
    SubagentAttemptIdentity::for_runtime_scope(
        control_scope_id.to_string(),
        context.run_id().to_string(),
        task_id.to_string(),
        claim.execution_id(context.run_id(), task_id),
        claim.attempt,
    )
    .map_err(|error| error.to_string())
}

pub(super) fn runtime_interrupt_disposition(
    disposition: SubagentInterruptRequestDisposition,
) -> RuntimeTaskAttemptInterruptDisposition {
    match disposition {
        SubagentInterruptRequestDisposition::QueuedBeforeAdmission => {
            RuntimeTaskAttemptInterruptDisposition::QueuedBeforeReservation
        }
        SubagentInterruptRequestDisposition::ReservedRequested => {
            RuntimeTaskAttemptInterruptDisposition::ReservedRequested
        }
        SubagentInterruptRequestDisposition::ActiveRequested => {
            RuntimeTaskAttemptInterruptDisposition::ActiveRequested
        }
        SubagentInterruptRequestDisposition::ActiveAlreadyRequested => {
            RuntimeTaskAttemptInterruptDisposition::ActiveAlreadyRequested
        }
        SubagentInterruptRequestDisposition::AlreadySettled(status) => {
            RuntimeTaskAttemptInterruptDisposition::AlreadySettled {
                status: status.as_str().to_string(),
            }
        }
    }
}

pub(super) fn runtime_interrupt_projection_error(
    error: SubagentControlError,
) -> RuntimeTaskAttemptInterruptProjectionError {
    match error {
        SubagentControlError::PendingCapacityExceeded { limit } => {
            RuntimeTaskAttemptInterruptProjectionError::PendingCapacityExceeded { limit }
        }
        SubagentControlError::IdentityConflict {
            task_id,
            attempt,
            expected_execution_id,
            actual_execution_id,
        } => RuntimeTaskAttemptInterruptProjectionError::IdentityConflict {
            task_id,
            attempt,
            expected_execution_id,
            actual_execution_id,
        },
        error => RuntimeTaskAttemptInterruptProjectionError::Unavailable {
            message: error.to_string(),
        },
    }
}

/// Canonical member execution adapter supplied by [`super::SubagentExecutor`].
pub type TeamDispatchFn =
    Arc<dyn Fn(TeamDispatchRequest) -> BoxFuture<'static, Result<SubagentResult>> + Send + Sync>;

/// Process-local reservation hook paired with a Team dispatch callback.
pub type TeamReserveAttemptFn =
    Arc<dyn Fn(TaskSubagentContext) -> std::result::Result<(), String> + Send + Sync>;

/// Live exact-interrupt projection hook paired with a Team runtime.
pub type TeamRequestInterruptFn = Arc<
    dyn Fn(
            String,
            String,
            TaskClaim,
        ) -> std::result::Result<
            Option<RuntimeTaskAttemptInterruptDisposition>,
            RuntimeTaskAttemptInterruptProjectionError,
        > + Send
        + Sync,
>;

/// Post-settlement cleanup hook for a Team attempt control projection.
pub type TeamRetireAttemptFn = Arc<
    dyn Fn(String, Task, TaskClaim) -> BoxFuture<'static, RuntimeAttemptControlCleanupReceipt>
        + Send
        + Sync,
>;

pub type TeamReconcileAttemptFn =
    Arc<dyn Fn(String, HashSet<String>) -> std::result::Result<(), String> + Send + Sync>;

#[derive(Clone)]
pub(super) struct TeamDispatchControl {
    pub reserve_attempt: TeamReserveAttemptFn,
    pub request_interrupt: TeamRequestInterruptFn,
    pub retire_attempt: TeamRetireAttemptFn,
    pub reconcile_attempts: TeamReconcileAttemptFn,
}

impl Default for TeamDispatchControl {
    fn default() -> Self {
        Self {
            reserve_attempt: Arc::new(|_| Ok(())),
            request_interrupt: Arc::new(|_, _, _| Ok(None)),
            retire_attempt: Arc::new(|_, _, _| {
                Box::pin(async { RuntimeAttemptControlCleanupReceipt::Retired })
            }),
            reconcile_attempts: Arc::new(|_, _| Ok(())),
        }
    }
}

/// Complete execution and exact-control contract for Team member attempts.
/// Implementations must keep dispatch and control on the same live registry.
#[async_trait]
pub trait TeamDispatchController: Send + Sync {
    async fn dispatch(&self, request: TeamDispatchRequest) -> Result<SubagentResult>;

    fn reserve_attempt(&self, context: TaskSubagentContext) -> std::result::Result<(), String>;

    fn request_interrupt(
        &self,
        run_id: String,
        task_id: String,
        claim: TaskClaim,
    ) -> std::result::Result<
        Option<RuntimeTaskAttemptInterruptDisposition>,
        RuntimeTaskAttemptInterruptProjectionError,
    >;

    async fn retire_attempt(
        &self,
        run_id: String,
        task: Task,
        claim: TaskClaim,
    ) -> RuntimeAttemptControlCleanupReceipt;

    fn reconcile_attempts(
        &self,
        run_id: String,
        current_execution_ids: HashSet<String>,
    ) -> std::result::Result<(), String>;
}

struct ClosureTeamDispatchController {
    dispatch: TeamDispatchFn,
    control: TeamDispatchControl,
}

#[async_trait]
impl TeamDispatchController for ClosureTeamDispatchController {
    async fn dispatch(&self, request: TeamDispatchRequest) -> Result<SubagentResult> {
        (self.dispatch)(request).await
    }

    fn reserve_attempt(&self, context: TaskSubagentContext) -> std::result::Result<(), String> {
        (self.control.reserve_attempt)(context)
    }

    fn request_interrupt(
        &self,
        run_id: String,
        task_id: String,
        claim: TaskClaim,
    ) -> std::result::Result<
        Option<RuntimeTaskAttemptInterruptDisposition>,
        RuntimeTaskAttemptInterruptProjectionError,
    > {
        (self.control.request_interrupt)(run_id, task_id, claim)
    }

    async fn retire_attempt(
        &self,
        run_id: String,
        task: Task,
        claim: TaskClaim,
    ) -> RuntimeAttemptControlCleanupReceipt {
        (self.control.retire_attempt)(run_id, task, claim).await
    }

    fn reconcile_attempts(
        &self,
        run_id: String,
        current_execution_ids: HashSet<String>,
    ) -> std::result::Result<(), String> {
        (self.control.reconcile_attempts)(run_id, current_execution_ids)
    }
}

pub(super) fn closure_dispatch_controller(
    dispatch: TeamDispatchFn,
    control: TeamDispatchControl,
) -> Arc<dyn TeamDispatchController> {
    Arc::new(ClosureTeamDispatchController { dispatch, control })
}

/// Terminal output of one Team graph execution.
#[derive(Debug, Clone)]
pub struct TeamExecutionResult {
    pub output: String,
    pub usage: Option<LlmUsageStats>,
    /// Last task-graph revision observed after execution.
    pub final_revision: u64,
}

pub(super) struct CompiledTeamGraph {
    tasks: Vec<Task>,
    terminal_task_id: String,
}

/// Canonical persistence and dispatch boundary for resumable Team execution.
///
/// Implementations own storage and product-specific dispatch, while
/// [`RuntimeTaskService`] remains the only dependency, claim, cancellation,
/// and settlement engine. A runtime must durably persist a successful
/// [`SubagentResult`] before it exposes the corresponding task as Completed.
#[async_trait]
pub trait TeamRuntime: RuntimeDagController<DispatchOutput = SubagentResult> {
    /// Revision service backed by the same graph authority as this controller.
    fn revisions(&self) -> &TaskRevisionService;

    /// Load one reusable result for a completed task.
    async fn task_result(&self, run_id: &str, task_id: &str) -> Result<Option<SubagentResult>>;

    /// Load all reusable results for usage aggregation and recovery checks.
    async fn task_results(&self, run_id: &str) -> Result<Vec<SubagentResult>>;
}

/// Execute Team intent through the framework's single revisioned DAG runtime.
pub async fn execute_team(
    spec: &TeamSpec,
    objective: &str,
    run_id: &str,
    cancel: CancellationToken,
    dispatch: TeamDispatchFn,
) -> Result<TeamExecutionResult> {
    execute_team_with_runtime_dispatch(spec, objective, run_id, cancel, dispatch).await
}

pub(super) async fn execute_team_with_runtime_dispatch(
    spec: &TeamSpec,
    objective: &str,
    run_id: &str,
    cancel: CancellationToken,
    dispatch: TeamDispatchFn,
) -> Result<TeamExecutionResult> {
    let runtime = Arc::new(TeamRuntimeController::new(dispatch));
    execute_team_on_runtime(spec, objective, run_id, cancel, runtime).await
}

/// Execute or resume Team intent on a caller-supplied canonical runtime.
///
/// Reusing `run_id` resumes the existing revisioned graph. The stored objective
/// and Team specification must match exactly; a mismatched identity fails
/// closed instead of dispatching into an unrelated run.
pub async fn execute_team_on_runtime<R>(
    spec: &TeamSpec,
    objective: &str,
    run_id: &str,
    cancel: CancellationToken,
    runtime: Arc<R>,
) -> Result<TeamExecutionResult>
where
    R: TeamRuntime,
{
    let service = TeamRuntimeServiceHandle::new(
        runtime.clone(),
        RuntimeTaskServiceConfig {
            max_concurrent_subagents: spec.config.max_concurrent.max(1),
            ..RuntimeTaskServiceConfig::default()
        },
    );
    execute_team_on_runtime_service(spec, objective, run_id, cancel, &service).await
}

/// Execute or resume a Team with a caller-retained runtime service.
///
/// Use this entry point when exact control must run concurrently with a
/// caller-supplied [`TeamRuntime`]; the same service owns execution, supervisor
/// handles, reconciliation, and interrupt requests.
pub async fn execute_team_on_runtime_service<R>(
    spec: &TeamSpec,
    objective: &str,
    run_id: &str,
    cancel: CancellationToken,
    service: &TeamRuntimeServiceHandle<R>,
) -> Result<TeamExecutionResult>
where
    R: TeamRuntime,
{
    validate_team_spec(spec)?;
    let timeout = spec.config.default_timeout_secs;
    let timeout_cancel = cancel.clone();
    let execution = execute_team_inner(
        spec,
        objective,
        run_id,
        cancel,
        service.runtime().clone(),
        service.runtime_tasks().clone(),
    );
    if timeout == 0 {
        execution.await
    } else {
        let mut execution = Box::pin(execution);
        tokio::select! {
            result = &mut execution => result,
            _ = tokio::time::sleep(Duration::from_secs(timeout)) => {
                timeout_cancel.cancel();
                let settlement = execution.await;
                let detail = match settlement {
                    Err(error)
                        if AgentFailure::from(&error).terminal_kind
                            != AgentTerminalKind::Cancelled =>
                    {
                        format!(
                            "Team execution timed out after {timeout}s; cancellation settlement failed: {error}"
                        )
                    }
                    _ => format!("Team execution timed out after {timeout}s"),
                };
                Err(ReactError::Agent(Box::new(AgentError::Timeout(detail))))
            }
        }
    }
}

async fn execute_team_inner<R>(
    spec: &TeamSpec,
    objective: &str,
    run_id: &str,
    cancel: CancellationToken,
    runtime: Arc<R>,
    runtime_tasks: Arc<RuntimeTaskService<R>>,
) -> Result<TeamExecutionResult>
where
    R: TeamRuntime,
{
    let service = runtime.revisions();
    let graph_context = team_graph_context(spec, objective)?;

    let terminal_task_id = if spec.strategy == TeamStrategy::ManagerSubagent {
        let initial = manager_subagent::initial_graph(spec, objective);
        let graph = ensure_team_graph(
            service,
            run_id,
            &graph_context,
            initial.tasks.clone(),
            "compile Team manager plan",
        )
        .await?;
        let synthesis_task_id = manager_subagent::synthesis_task_id();
        let already_expanded = graph
            .snapshot
            .tasks
            .iter()
            .any(|task| task.spec.id == synthesis_task_id);
        if already_expanded {
            let plan =
                manager_plan_output(runtime.as_ref(), run_id, &initial.terminal_task_id).await?;
            let expanded = manager_subagent::expand_graph(spec, objective, &plan)?;
            validate_manager_graph(run_id, &graph, &initial.tasks, &expanded.tasks)?;
        } else {
            validate_team_graph_specs(run_id, &graph, &initial.tasks)?;
            drive_team_graph(runtime_tasks.as_ref(), run_id, cancel.child_token()).await?;
            let plan =
                manager_plan_output(runtime.as_ref(), run_id, &initial.terminal_task_id).await?;
            let expanded = manager_subagent::expand_graph(spec, objective, &plan)?;
            let current = service
                .load(run_id)
                .await
                .map_err(|error| ReactError::Other(error.to_string()))?
                .ok_or_else(|| ReactError::Other("Team manager graph disappeared".to_string()))?;
            let committed = if current
                .snapshot
                .tasks
                .iter()
                .any(|task| task.spec.id == synthesis_task_id)
            {
                current
            } else {
                validate_team_graph_specs(run_id, &current, &initial.tasks)?;
                commit_manager_expansion(service, run_id, current, &expanded.tasks).await?
            };
            validate_manager_graph(run_id, &committed, &initial.tasks, &expanded.tasks)?;
        }
        drive_team_graph(runtime_tasks.as_ref(), run_id, cancel.child_token()).await?;
        synthesis_task_id.to_string()
    } else {
        let compiled = compile_team_graph(spec, objective)?;
        let graph = ensure_team_graph(
            service,
            run_id,
            &graph_context,
            compiled.tasks.clone(),
            "compile Team intent",
        )
        .await?;
        validate_team_graph_specs(run_id, &graph, &compiled.tasks)?;
        drive_team_graph(runtime_tasks.as_ref(), run_id, cancel).await?;
        compiled.terminal_task_id
    };

    let terminal = runtime
        .task_result(run_id, &terminal_task_id)
        .await?
        .ok_or_else(|| {
            ReactError::Other(format!(
                "Team graph completed without terminal output '{terminal_task_id}'"
            ))
        })?;
    let outputs = runtime.task_results(run_id).await?;
    let final_revision = service
        .load(run_id)
        .await
        .map_err(|error| ReactError::Other(error.to_string()))?
        .map(|graph| graph.snapshot.revision)
        .ok_or_else(|| ReactError::Other(format!("Team graph '{run_id}' not found")))?;
    Ok(TeamExecutionResult {
        output: terminal.output,
        usage: aggregate_usage(outputs.iter()),
        final_revision,
    })
}

async fn ensure_team_graph(
    service: &TaskRevisionService,
    run_id: &str,
    graph_context: &TaskGraphContext,
    tasks: Vec<Task>,
    reason: &str,
) -> Result<RevisionedTaskGraph> {
    if let Some(graph) = service
        .load(run_id)
        .await
        .map_err(|error| ReactError::Other(error.to_string()))?
    {
        validate_team_graph_identity(run_id, &graph.context, graph_context)?;
        return Ok(graph);
    }

    match service
        .create_prepared(run_id, graph_context.clone(), tasks, reason.to_string())
        .await
    {
        Ok(graph) => Ok(graph),
        Err(TaskRevisionError::RevisionConflict { .. }) => {
            let graph = service
                .load(run_id)
                .await
                .map_err(|error| ReactError::Other(error.to_string()))?
                .ok_or_else(|| {
                    ReactError::Other(format!(
                        "Team graph '{run_id}' conflicted during creation and then disappeared"
                    ))
                })?;
            validate_team_graph_identity(run_id, &graph.context, graph_context)?;
            Ok(graph)
        }
        Err(error) => Err(ReactError::Other(error.to_string())),
    }
}

async fn manager_plan_output<R>(runtime: &R, run_id: &str, plan_task_id: &str) -> Result<String>
where
    R: TeamRuntime,
{
    runtime
        .task_result(run_id, plan_task_id)
        .await?
        .map(|result| result.output)
        .ok_or_else(|| {
            ReactError::Other("Team manager completed without a plan output".to_string())
        })
}

async fn commit_manager_expansion(
    service: &TaskRevisionService,
    run_id: &str,
    current: RevisionedTaskGraph,
    expanded_tasks: &[Task],
) -> Result<RevisionedTaskGraph> {
    let patch = TaskPlanPatch {
        base_revision: current.snapshot.revision,
        reason: "expand Team manager plan".to_string(),
        operations: expanded_tasks
            .iter()
            .map(|task| TaskPlanPatchOp::Insert {
                after_task_id: None,
                task: task.spec.clone(),
            })
            .collect(),
    };
    match service.apply_patch(run_id, patch).await {
        Ok(graph) => Ok(graph),
        Err(TaskRevisionError::RevisionConflict { .. }) => service
            .load(run_id)
            .await
            .map_err(|error| ReactError::Other(error.to_string()))?
            .ok_or_else(|| {
                ReactError::Other(format!(
                    "Team graph '{run_id}' conflicted during manager expansion and then disappeared"
                ))
            }),
        Err(error) => Err(ReactError::Other(error.to_string())),
    }
}

fn validate_manager_graph(
    run_id: &str,
    graph: &RevisionedTaskGraph,
    initial_tasks: &[Task],
    expanded_tasks: &[Task],
) -> Result<()> {
    let expected = initial_tasks
        .iter()
        .chain(expanded_tasks)
        .cloned()
        .collect::<Vec<_>>();
    validate_team_graph_specs(run_id, graph, &expected)
}

fn validate_team_graph_specs(
    run_id: &str,
    graph: &RevisionedTaskGraph,
    expected_tasks: &[Task],
) -> Result<()> {
    if graph.snapshot.tasks.len() != expected_tasks.len() {
        return Err(ReactError::Other(format!(
            "Team run '{run_id}' contains a different task graph than the requested Team"
        )));
    }
    for expected in expected_tasks {
        let Some(stored) = graph
            .snapshot
            .tasks
            .iter()
            .find(|task| task.spec.id == expected.spec.id)
        else {
            return Err(ReactError::Other(format!(
                "Team run '{run_id}' is missing task '{}'",
                expected.spec.id
            )));
        };
        if !team_task_specs_match(&stored.spec, &expected.spec) {
            return Err(ReactError::Other(format!(
                "Team run '{run_id}' task '{}' differs from the requested Team",
                expected.spec.id
            )));
        }
    }
    Ok(())
}

fn team_task_specs_match(stored: &TaskSpec, expected: &TaskSpec) -> bool {
    let mut stored_without_extension = stored.clone();
    stored_without_extension.extension = serde_json::Value::Null;
    let mut expected_without_extension = expected.clone();
    expected_without_extension.extension = serde_json::Value::Null;
    if stored_without_extension != expected_without_extension {
        return false;
    }
    match &expected.extension {
        serde_json::Value::Null => true,
        serde_json::Value::Object(expected_fields) => {
            stored.extension.as_object().is_some_and(|stored_fields| {
                expected_fields
                    .iter()
                    .all(|(key, value)| stored_fields.get(key) == Some(value))
            })
        }
        expected_extension => &stored.extension == expected_extension,
    }
}

fn team_graph_context(spec: &TeamSpec, objective: &str) -> Result<TaskGraphContext> {
    let team_spec = serde_json::to_value(spec)
        .map_err(|error| ReactError::Other(format!("Failed to encode Team identity: {error}")))?;
    Ok(TaskGraphContext {
        goal: objective.to_string(),
        assumptions: Vec::new(),
        risks: Vec::new(),
        execution_mode: TaskGraphExecutionMode::Parallel,
        metadata: serde_json::json!({
            "team_strategy": spec.strategy.name(),
            "team_spec": team_spec,
        }),
    })
}

fn validate_team_graph_identity(
    run_id: &str,
    stored: &TaskGraphContext,
    requested: &TaskGraphContext,
) -> Result<()> {
    if stored == requested {
        return Ok(());
    }
    Err(ReactError::Other(format!(
        "Team run '{run_id}' already belongs to a different objective or Team specification"
    )))
}

async fn drive_team_graph<R>(
    runtime_tasks: &RuntimeTaskService<R>,
    run_id: &str,
    cancel: CancellationToken,
) -> Result<()>
where
    R: TeamRuntime,
{
    match runtime_tasks.execute(run_id, cancel).await? {
        RuntimeDagOutcome::Completed => {}
        RuntimeDagOutcome::Failed { error, .. } => {
            return Err(ReactError::Other(format!("Team graph failed: {error}")));
        }
        RuntimeDagOutcome::Paused { reason, .. } => {
            return Err(ReactError::Other(format!("Team graph paused: {reason}")));
        }
        RuntimeDagOutcome::Stalled { reason } => {
            return Err(ReactError::Other(format!("Team graph stalled: {reason}")));
        }
        RuntimeDagOutcome::Cancelled => {
            return Err(ReactError::Agent(Box::new(AgentError::Cancelled(
                "Team graph cancelled".to_string(),
            ))));
        }
    }
    Ok(())
}

fn compile_team_graph(spec: &TeamSpec, objective: &str) -> Result<CompiledTeamGraph> {
    validate_team_spec(spec)?;
    let mut tasks = Vec::new();
    let terminal_task_id = match &spec.strategy {
        TeamStrategy::ManagerSubagent => {
            return Err(ReactError::Other(
                "Manager-Subagent graphs require the two-phase compiler".to_string(),
            ));
        }
        TeamStrategy::Pipeline(members) => {
            let mut previous = None;
            let mut terminal = String::new();
            for (index, member) in members.iter().enumerate() {
                let id = format!("team-pipeline-{index}");
                let dependencies = previous.iter().cloned().collect();
                let mut task = team_task(
                    &id,
                    member,
                    format!("Advance this pipeline objective:\n{objective}"),
                    dependencies,
                );
                set_team_task_phase(&mut task, "pipeline")?;
                tasks.push(task);
                previous = Some(id.clone());
                terminal = id;
            }
            terminal
        }
        TeamStrategy::Debate { judge, debaters } => {
            let proposal_ids = debaters
                .iter()
                .enumerate()
                .map(|(index, member)| {
                    let id = format!("team-proposal-{index}");
                    tasks.push(team_task(
                        &id,
                        member,
                        format!("Propose an independent solution for:\n{objective}"),
                        Vec::new(),
                    ));
                    id
                })
                .collect();
            let id = "team-judge".to_string();
            tasks.push(team_task(
                &id,
                judge,
                format!("Judge and synthesize the proposals for:\n{objective}"),
                proposal_ids,
            ));
            id
        }
        TeamStrategy::Swarm { reducer } => {
            let shard_ids = spec
                .subagents
                .iter()
                .enumerate()
                .map(|(index, member)| {
                    let id = format!("team-shard-{index}");
                    tasks.push(team_task(
                        &id,
                        member,
                        format!("Inspect your assigned portion of:\n{objective}"),
                        Vec::new(),
                    ));
                    id
                })
                .collect();
            let id = "team-reducer".to_string();
            tasks.push(team_task(
                &id,
                reducer,
                format!("Merge the Team findings for:\n{objective}"),
                shard_ids,
            ));
            id
        }
    };
    Ok(CompiledTeamGraph {
        tasks,
        terminal_task_id,
    })
}

fn validate_team_spec(spec: &TeamSpec) -> Result<()> {
    let mut names = HashSet::new();
    let mut validate_name = |name: &str| {
        if name.trim().is_empty() {
            return Err(ReactError::Other(
                "Team member names cannot be empty".to_string(),
            ));
        }
        if !names.insert(name.to_string()) {
            return Err(ReactError::Other(format!(
                "Team member '{name}' is declared more than once"
            )));
        }
        Ok(())
    };
    match &spec.strategy {
        TeamStrategy::ManagerSubagent => {
            validate_name(&spec.manager)?;
            if spec.subagents.is_empty() {
                return Err(ReactError::Other(
                    "Manager-Subagent Team requires at least one executable Subagent".to_string(),
                ));
            }
            for member in &spec.subagents {
                validate_name(member)?;
            }
        }
        TeamStrategy::Pipeline(members) => {
            if members.is_empty() {
                return Err(ReactError::Other(
                    "Team pipeline requires at least one Subagent".to_string(),
                ));
            }
            for member in members {
                validate_name(member)?;
            }
        }
        TeamStrategy::Debate { judge, debaters } => {
            validate_name(judge)?;
            if debaters.is_empty() {
                return Err(ReactError::Other(
                    "Team debate requires at least one debater".to_string(),
                ));
            }
            for member in debaters {
                validate_name(member)?;
            }
        }
        TeamStrategy::Swarm { reducer } => {
            validate_name(reducer)?;
            if spec.subagents.is_empty() {
                return Err(ReactError::Other(
                    "Team swarm requires at least one Subagent".to_string(),
                ));
            }
            for member in &spec.subagents {
                validate_name(member)?;
            }
        }
    }
    Ok(())
}

fn referenced_member_names(spec: &TeamSpec) -> Vec<&str> {
    match &spec.strategy {
        TeamStrategy::ManagerSubagent => std::iter::once(spec.manager.as_str())
            .chain(spec.subagents.iter().map(String::as_str))
            .collect(),
        TeamStrategy::Pipeline(members) => members.iter().map(String::as_str).collect(),
        TeamStrategy::Debate { judge, debaters } => std::iter::once(judge.as_str())
            .chain(debaters.iter().map(String::as_str))
            .collect(),
        TeamStrategy::Swarm { reducer } => spec
            .subagents
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(reducer.as_str()))
            .collect(),
    }
}

fn team_task(id: &str, member: &str, description: String, depends_on: Vec<String>) -> Task {
    let extension = serde_json::to_value(TeamTaskExtension {
        member: member.to_string(),
        phase: None,
    })
    .unwrap_or(serde_json::Value::Null);
    let spec = TaskSpec {
        id: id.to_string(),
        title: description.clone(),
        description,
        depends_on,
        max_retries: 0,
        extension,
    };
    Task {
        execution: TaskExecution::pending(id),
        spec,
    }
}

fn set_team_task_phase(task: &mut Task, phase: &str) -> Result<()> {
    let mut extension: TeamTaskExtension = serde_json::from_value(task.spec.extension.clone())
        .map_err(|error| ReactError::Other(format!("Invalid Team task extension: {error}")))?;
    extension.phase = Some(phase.to_string());
    task.spec.extension = serde_json::to_value(extension).map_err(|error| {
        ReactError::Other(format!("Failed to encode Team task extension: {error}"))
    })?;
    Ok(())
}

#[derive(Clone)]
struct StagedTeamOutput {
    run_id: String,
    task_id: String,
    output: SubagentResult,
}

struct TeamRuntimeController {
    store: Arc<InMemoryRevisionedTaskStore>,
    revisions: TaskRevisionService,
    dispatch_controller: Arc<dyn TeamDispatchController>,
    outputs: Mutex<HashMap<String, HashMap<String, SubagentResult>>>,
    staged_outputs: Mutex<HashMap<String, StagedTeamOutput>>,
    settlement: Mutex<()>,
}

impl TeamRuntimeController {
    fn new(dispatch: TeamDispatchFn) -> Self {
        Self::with_controller(closure_dispatch_controller(
            dispatch,
            TeamDispatchControl::default(),
        ))
    }

    fn with_controller(dispatch_controller: Arc<dyn TeamDispatchController>) -> Self {
        let store = Arc::new(InMemoryRevisionedTaskStore::new());
        let revisions = TaskRevisionService::new(
            store.clone(),
            Arc::new(DefaultTaskToolPolicy::new("team-runtime")),
        );
        Self {
            store,
            revisions,
            dispatch_controller,
            outputs: Mutex::new(HashMap::new()),
            staged_outputs: Mutex::new(HashMap::new()),
            settlement: Mutex::new(()),
        }
    }
}

#[async_trait]
impl RuntimeDagController for TeamRuntimeController {
    type DispatchOutput = SubagentResult;

    async fn load_snapshot(&self, run_id: &str) -> Result<RuntimePlanSnapshot> {
        self.store
            .load(run_id)
            .await
            .map_err(|error| ReactError::Other(error.to_string()))?
            .map(|graph| graph.snapshot)
            .ok_or_else(|| ReactError::Other(format!("Team graph '{run_id}' not found")))
    }

    async fn claim_task(
        &self,
        run_id: &str,
        task: &Task,
        expected_revision: u64,
    ) -> Result<RuntimeTaskClaimOutcome> {
        self.store
            .claim_runtime_task(run_id, task, expected_revision)
            .await
            .map_err(|error| ReactError::Other(error.to_string()))
    }

    async fn claim_is_current(
        &self,
        run_id: &str,
        task_id: &str,
        claim: &TaskClaim,
    ) -> Result<bool> {
        self.store
            .runtime_claim_is_current(run_id, task_id, claim)
            .await
            .map_err(|error| ReactError::Other(error.to_string()))
    }

    async fn reserve_attempt_control(&self, context: &TaskSubagentContext) -> Result<()> {
        self.dispatch_controller
            .reserve_attempt(context.clone())
            .map_err(ReactError::Other)
    }

    async fn request_live_interrupt(
        &self,
        run_id: &str,
        task_id: &str,
        claim: &TaskClaim,
    ) -> std::result::Result<
        Option<RuntimeTaskAttemptInterruptDisposition>,
        RuntimeTaskAttemptInterruptProjectionError,
    > {
        self.dispatch_controller.request_interrupt(
            run_id.to_string(),
            task_id.to_string(),
            claim.clone(),
        )
    }

    async fn retire_attempt_control(
        &self,
        run_id: &str,
        task: &Task,
        claim: &TaskClaim,
    ) -> RuntimeAttemptControlCleanupReceipt {
        self.dispatch_controller
            .retire_attempt(run_id.to_string(), task.clone(), claim.clone())
            .await
    }

    async fn reconcile_attempt_control(
        &self,
        run_id: &str,
        current_execution_ids: &HashSet<String>,
    ) -> Result<()> {
        self.dispatch_controller
            .reconcile_attempts(run_id.to_string(), current_execution_ids.clone())
            .map_err(ReactError::Other)
    }

    async fn dispatch_task(
        &self,
        context: TaskSubagentContext,
        task: Task,
    ) -> Result<Self::DispatchOutput> {
        let outputs = self.outputs.lock().await;
        let run_outputs = outputs.get(context.run_id());
        let mut dependency_outputs = Vec::with_capacity(task.spec.depends_on.len());
        let waived_dependencies: HashSet<&str> = context
            .waived_dependency_ids()
            .iter()
            .map(String::as_str)
            .collect();
        for dependency in &task.spec.depends_on {
            if waived_dependencies.contains(dependency.as_str()) {
                continue;
            }
            let output = run_outputs
                .and_then(|values| values.get(dependency))
                .ok_or_else(|| {
                    ReactError::Other(format!(
                        "Team dependency '{dependency}' completed without a reusable result"
                    ))
                })?;
            dependency_outputs.push((dependency, output));
        }
        let extension: TeamTaskExtension = serde_json::from_value(task.spec.extension.clone())
            .map_err(|error| ReactError::Other(format!("Invalid Team task extension: {error}")))?;
        let pipeline_phase = extension.phase.as_deref() == Some("pipeline");
        let mut prompt = if pipeline_phase && dependency_outputs.len() == 1 {
            dependency_outputs
                .first()
                .map(|(_, output)| output.output.clone())
                .ok_or_else(|| {
                    ReactError::Other("Pipeline dependency output is unavailable".to_string())
                })?
        } else {
            let mut prompt = task.spec.description.clone();
            for (dependency, output) in dependency_outputs {
                prompt.push_str("\n\nCompleted dependency ");
                prompt.push_str(dependency);
                prompt.push_str(":\n");
                prompt.push_str(&output.output);
            }
            prompt
        };
        drop(outputs);
        if pipeline_phase && task.spec.depends_on.is_empty() {
            prompt = task.spec.description.clone();
        }
        for dependency in context.waived_dependency_ids() {
            prompt.push_str("\n\nDependency '");
            prompt.push_str(dependency);
            prompt.push_str("' was explicitly skipped; no reusable output is available.");
        }
        if context.claim().is_none() {
            return Err(ReactError::Other(
                "Team runtime dispatch requires an exact TaskClaim context".to_string(),
            ));
        }
        self.dispatch_controller
            .dispatch(TeamDispatchRequest {
                member: extension.member,
                task: prompt,
                context,
            })
            .await
    }

    async fn resolve_dispatch(
        &self,
        run_id: &str,
        claim: TaskClaim,
        task: Task,
        dispatch: Result<Self::DispatchOutput>,
    ) -> Result<RuntimeTaskResolutionRequest> {
        let (request, output) = match dispatch {
            Ok(output) if output.outcome.status == SubagentStatus::Completed => {
                (RuntimeTaskResolutionRequest::Completed, Some(output))
            }
            Ok(output) if output.outcome.status == SubagentStatus::Cancelled => {
                (RuntimeTaskResolutionRequest::Cancelled, None)
            }
            Ok(output) => {
                let error = if output.outcome.summary.is_empty() {
                    output.output.clone()
                } else {
                    output.outcome.summary.clone()
                };
                (RuntimeTaskResolutionRequest::Failed { error }, None)
            }
            Err(error) => {
                let message = error.to_string();
                let request = match AgentFailure::from(&error).terminal_kind {
                    AgentTerminalKind::Cancelled => RuntimeTaskResolutionRequest::Cancelled,
                    AgentTerminalKind::TimedOut => {
                        RuntimeTaskResolutionRequest::TimedOut { error: message }
                    }
                    AgentTerminalKind::Failed | AgentTerminalKind::PermissionDenied => {
                        RuntimeTaskResolutionRequest::Failed { error: message }
                    }
                };
                (request, None)
            }
        };
        if let Some(output) = output {
            let mut staged_outputs = self.staged_outputs.lock().await;
            if staged_outputs.contains_key(&claim.claim_id) {
                return Err(ReactError::Other(format!(
                    "Team result for claim '{}' was staged more than once",
                    claim.claim_id
                )));
            }
            staged_outputs.insert(
                claim.claim_id.clone(),
                StagedTeamOutput {
                    run_id: run_id.to_string(),
                    task_id: task.spec.id,
                    output,
                },
            );
        }
        Ok(request)
    }

    async fn settle_resolution(
        &self,
        run_id: &str,
        claim: &TaskClaim,
        task: &Task,
        request: RuntimeTaskResolutionRequest,
    ) -> Result<RuntimeTaskResolution> {
        let _settlement = self.settlement.lock().await;
        let staged_candidate = self.staged_outputs.lock().await.remove(&claim.claim_id);
        if request == RuntimeTaskResolutionRequest::Completed
            && !staged_candidate
                .as_ref()
                .is_some_and(|staged| staged.run_id == run_id && staged.task_id == task.spec.id)
        {
            return Err(ReactError::Other(format!(
                "Team result for claim '{}' was not staged before completion",
                claim.claim_id
            )));
        }
        let mut outputs = if staged_candidate.is_some() {
            Some(self.outputs.lock().await)
        } else {
            None
        };
        let settlement = self
            .store
            .settle_runtime_resolution(run_id, &task.spec.id, claim, request)
            .await
            .map_err(|error| ReactError::Other(error.to_string()));
        if matches!(&settlement, Ok(RuntimeTaskResolution::Completed)) {
            // Candidate ownership and the output lock precede CAS. Keep the
            // commit-to-publication section below free of cancellation points.
            let staged = staged_candidate.ok_or_else(|| {
                ReactError::Other(format!(
                    "Team result for settled claim '{}' disappeared",
                    claim.claim_id
                ))
            })?;
            let outputs = outputs.as_mut().ok_or_else(|| {
                ReactError::Other("Team output publication lock is unavailable".to_string())
            })?;
            outputs
                .entry(staged.run_id)
                .or_default()
                .insert(staged.task_id, staged.output);
        }
        settlement
    }

    async fn abandon_claim(
        &self,
        run_id: &str,
        claim: &TaskClaim,
        task: &Task,
        abandonment: RuntimeClaimAbandonment,
    ) -> Result<echo_orchestration::tasks::RuntimeTaskSettlementOutcome> {
        let status = match abandonment {
            RuntimeClaimAbandonment::Interrupted { disposition } => match disposition {
                RuntimeInterruptionDisposition::Cancelled => TaskStatus::Cancelled,
                RuntimeInterruptionDisposition::Paused { reason } => TaskStatus::Paused(reason),
            },
            RuntimeClaimAbandonment::Failed { error } => TaskStatus::Failed(error),
        };
        self.store
            .settle_runtime_claim(run_id, &task.spec.id, claim, status)
            .await
            .map_err(|error| ReactError::Other(error.to_string()))
    }

    async fn settle_interruption(
        &self,
        run_id: &str,
        expected_revision: u64,
        disposition: RuntimeInterruptionDisposition,
    ) -> Result<RuntimeInterruptionSettlementOutcome> {
        self.store
            .settle_runtime_interruption(run_id, expected_revision, disposition)
            .await
            .map_err(|error| ReactError::Other(error.to_string()))
    }
}

#[async_trait]
impl TeamRuntime for TeamRuntimeController {
    fn revisions(&self) -> &TaskRevisionService {
        &self.revisions
    }

    async fn task_result(&self, run_id: &str, task_id: &str) -> Result<Option<SubagentResult>> {
        Ok(self
            .outputs
            .lock()
            .await
            .get(run_id)
            .and_then(|outputs| outputs.get(task_id))
            .cloned())
    }

    async fn task_results(&self, run_id: &str) -> Result<Vec<SubagentResult>> {
        Ok(self
            .outputs
            .lock()
            .await
            .get(run_id)
            .map(|outputs| outputs.values().cloned().collect())
            .unwrap_or_default())
    }
}

fn aggregate_usage<'a>(outputs: impl Iterator<Item = &'a SubagentResult>) -> Option<LlmUsageStats> {
    let mut total = LlmUsageStats::default();
    let mut has_usage = false;
    let mut models = BTreeSet::new();
    for result in outputs {
        let Some(usage) = &result.llm_usage else {
            continue;
        };
        has_usage = true;
        if !usage.model.is_empty() {
            models.insert(usage.model.clone());
        }
        total.prompt_tokens = total.prompt_tokens.saturating_add(usage.prompt_tokens);
        total.completion_tokens = total
            .completion_tokens
            .saturating_add(usage.completion_tokens);
        total.total_tokens = total.total_tokens.saturating_add(usage.total_tokens);
        total.cached_prompt_tokens = total
            .cached_prompt_tokens
            .saturating_add(usage.cached_prompt_tokens);
        total.cache_creation_prompt_tokens = total
            .cache_creation_prompt_tokens
            .saturating_add(usage.cache_creation_prompt_tokens);
        total.call_count = total.call_count.saturating_add(usage.call_count);
        total.usage_reported |= usage.usage_reported;
    }
    total.model = models.into_iter().collect::<Vec<_>>().join(",");
    has_usage.then_some(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::agent::AgentEvent;
    use echo_core::error::Result as CoreResult;
    use echo_orchestration::tasks::PlanValidator;
    use futures::stream::{self, BoxStream};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    fn successful_result(agent_name: String, task: &str) -> SubagentResult {
        SubagentResult::sync_result(&agent_name, format!("done: {task}"), Duration::ZERO)
    }

    fn manager_plan_result(agent_name: String, plan: &str) -> SubagentResult {
        SubagentResult::sync_result(&agent_name, plan.to_string(), Duration::ZERO)
    }

    #[tokio::test]
    async fn manager_team_uses_canonical_graph_and_dependency_outputs() -> Result<()> {
        let calls = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
        let observed = calls.clone();
        let dispatch: TeamDispatchFn = Arc::new(move |request| {
            let TeamDispatchRequest {
                member: name,
                task,
                context,
            } = request;
            let _cancel = context.cancellation_token().clone();
            let observed = observed.clone();
            Box::pin(async move {
                observed.lock().await.push((name.clone(), task.clone()));
                if name == "manager" && !task.contains("Completed dependency") {
                    Ok(manager_plan_result(
                        name,
                        r#"{"tasks":[
                            {"id":"implementation","subagent":"researcher","description":"inspect implementation","depends_on":[]},
                            {"id":"tests","subagent":"reviewer","description":"inspect tests","depends_on":["implementation"]},
                            {"id":"documentation","subagent":"researcher","description":"inspect documentation","depends_on":[]}
                        ]}"#,
                    ))
                } else {
                    Ok(successful_result(name, &task))
                }
            })
        });
        let result = execute_team(
            &TeamSpec {
                strategy: TeamStrategy::ManagerSubagent,
                manager: "manager".to_string(),
                subagents: vec!["researcher".to_string(), "reviewer".to_string()],
                config: TeamConfig::default(),
            },
            "review the repository",
            "team-test",
            CancellationToken::new(),
            dispatch,
        )
        .await?;
        assert!(result.output.contains("Completed dependency team-member-0"));
        assert_eq!(result.final_revision, 2);
        let calls = calls.lock().await;
        assert_eq!(calls.len(), 5);
        assert_eq!(calls.first().map(|call| call.0.as_str()), Some("manager"));
        assert_eq!(calls.last().map(|call| call.0.as_str()), Some("manager"));
        let mut assigned = calls
            .iter()
            .skip(1)
            .take(3)
            .map(|call| call.0.as_str())
            .collect::<Vec<_>>();
        assigned.sort_unstable();
        assert_eq!(assigned, vec!["researcher", "researcher", "reviewer"]);
        assert!(calls.iter().any(|(name, prompt)| {
            name == "reviewer" && prompt.contains("Completed dependency team-member-0")
        }));
        Ok(())
    }

    #[tokio::test]
    async fn concurrent_manager_expansion_converges_on_one_exact_revision() -> Result<()> {
        let dispatch: TeamDispatchFn = Arc::new(|request| {
            let TeamDispatchRequest {
                member: name,
                task,
                context,
            } = request;
            let _cancel = context.cancellation_token().clone();
            Box::pin(async move { Ok(successful_result(name, &task)) })
        });
        let runtime = TeamRuntimeController::new(dispatch);
        let service = runtime.revisions();
        let run_id = "concurrent-manager-expansion";
        let objective = "review the repository";
        let spec = TeamSpec {
            strategy: TeamStrategy::ManagerSubagent,
            manager: "manager".to_string(),
            subagents: vec!["researcher".to_string()],
            config: TeamConfig::default(),
        };
        let context = team_graph_context(&spec, objective)?;
        let initial = manager_subagent::initial_graph(&spec, objective);
        let (left_created, right_created) = tokio::join!(
            ensure_team_graph(
                service,
                run_id,
                &context,
                initial.tasks.clone(),
                "create concurrent Team graph",
            ),
            ensure_team_graph(
                service,
                run_id,
                &context,
                initial.tasks.clone(),
                "create concurrent Team graph",
            ),
        );
        validate_team_graph_specs(run_id, &left_created?, &initial.tasks)?;
        validate_team_graph_specs(run_id, &right_created?, &initial.tasks)?;

        let expanded = manager_subagent::expand_graph(
            &spec,
            objective,
            r#"{"tasks":[{"id":"implementation","subagent":"researcher","description":"inspect implementation","depends_on":[]}]}"#,
        )?;
        let stale = service
            .load(run_id)
            .await
            .map_err(|error| ReactError::Other(error.to_string()))?
            .ok_or_else(|| ReactError::Other("concurrent Team graph missing".to_string()))?;
        let (left_expanded, right_expanded) = tokio::join!(
            commit_manager_expansion(service, run_id, stale.clone(), &expanded.tasks),
            commit_manager_expansion(service, run_id, stale, &expanded.tasks),
        );
        let left_expanded = left_expanded?;
        let right_expanded = right_expanded?;
        validate_manager_graph(run_id, &left_expanded, &initial.tasks, &expanded.tasks)?;
        validate_manager_graph(run_id, &right_expanded, &initial.tasks, &expanded.tasks)?;
        assert_eq!(left_expanded.snapshot.revision, 2);
        assert_eq!(right_expanded.snapshot.revision, 2);
        Ok(())
    }

    #[tokio::test]
    async fn concurrent_callers_resume_one_manager_run_without_duplicate_dispatch() -> Result<()> {
        let calls = Arc::new(Mutex::new(Vec::<String>::new()));
        let observed = calls.clone();
        let dispatch: TeamDispatchFn = Arc::new(move |request| {
            let TeamDispatchRequest {
                member: name,
                task,
                context,
            } = request;
            let _cancel = context.cancellation_token().clone();
            let observed = observed.clone();
            Box::pin(async move {
                observed.lock().await.push(name.clone());
                if name == "manager" && !task.contains("Completed dependency") {
                    Ok(manager_plan_result(
                        name,
                        r#"{"tasks":[{"id":"implementation","subagent":"researcher","description":"inspect implementation","depends_on":[]}]}"#,
                    ))
                } else {
                    Ok(successful_result(name, &task))
                }
            })
        });
        let runtime = Arc::new(TeamRuntimeController::new(dispatch));
        let spec = TeamSpec {
            strategy: TeamStrategy::ManagerSubagent,
            manager: "manager".to_string(),
            subagents: vec!["researcher".to_string()],
            config: TeamConfig::default(),
        };
        let (left, right) = tokio::join!(
            execute_team_on_runtime(
                &spec,
                "review the repository",
                "concurrent-manager-run",
                CancellationToken::new(),
                runtime.clone(),
            ),
            execute_team_on_runtime(
                &spec,
                "review the repository",
                "concurrent-manager-run",
                CancellationToken::new(),
                runtime,
            ),
        );
        let left = left?;
        let right = right?;
        assert_eq!(left.output, right.output);
        assert_eq!(left.final_revision, 2);
        assert_eq!(right.final_revision, 2);
        assert_eq!(calls.lock().await.len(), 3);
        Ok(())
    }

    #[tokio::test]
    async fn caller_owned_runtime_resumes_manager_graph_without_redispatch() -> Result<()> {
        let calls = Arc::new(Mutex::new(0usize));
        let observed = calls.clone();
        let dispatch: TeamDispatchFn = Arc::new(move |request| {
            let TeamDispatchRequest {
                member: name,
                task,
                context,
            } = request;
            let _cancel = context.cancellation_token().clone();
            let observed = observed.clone();
            Box::pin(async move {
                let mut count = observed.lock().await;
                *count = count.saturating_add(1);
                drop(count);
                if name == "manager" && !task.contains("Completed dependency") {
                    Ok(manager_plan_result(
                        name,
                        r#"{"tasks":[{"id":"implementation","subagent":"researcher","description":"inspect implementation","depends_on":[]}]}"#,
                    ))
                } else {
                    Ok(successful_result(name, &task))
                }
            })
        });
        let runtime = Arc::new(TeamRuntimeController::new(dispatch));
        let spec = TeamSpec {
            strategy: TeamStrategy::ManagerSubagent,
            manager: "manager".to_string(),
            subagents: vec!["researcher".to_string()],
            config: TeamConfig::default(),
        };
        let service =
            TeamRuntimeServiceHandle::new(runtime.clone(), RuntimeTaskServiceConfig::default());
        assert!(Arc::ptr_eq(service.runtime(), &runtime));

        let first = execute_team_on_runtime_service(
            &spec,
            "review the repository",
            "resumable-team",
            CancellationToken::new(),
            &service,
        )
        .await?;
        let first_call_count = *calls.lock().await;
        let resumed = execute_team_on_runtime_service(
            &spec,
            "review the repository",
            "resumable-team",
            CancellationToken::new(),
            &service,
        )
        .await?;

        assert_eq!(first.final_revision, 2);
        assert_eq!(resumed.final_revision, 2);
        assert_eq!(resumed.output, first.output);
        assert_eq!(*calls.lock().await, first_call_count);
        Ok(())
    }

    #[tokio::test]
    async fn reused_run_id_rejects_a_different_team_identity() -> Result<()> {
        let calls = Arc::new(Mutex::new(0usize));
        let observed = calls.clone();
        let dispatch: TeamDispatchFn = Arc::new(move |request| {
            let TeamDispatchRequest {
                member: name,
                task,
                context,
            } = request;
            let _cancel = context.cancellation_token().clone();
            let observed = observed.clone();
            Box::pin(async move {
                let mut count = observed.lock().await;
                *count = count.saturating_add(1);
                drop(count);
                Ok(successful_result(name, &task))
            })
        });
        let runtime = Arc::new(TeamRuntimeController::new(dispatch));
        let spec = TeamSpec {
            strategy: TeamStrategy::Pipeline(vec!["first".to_string()]),
            manager: String::new(),
            subagents: Vec::new(),
            config: TeamConfig::default(),
        };
        execute_team_on_runtime(
            &spec,
            "first objective",
            "identity-bound-team",
            CancellationToken::new(),
            runtime.clone(),
        )
        .await?;
        let first_call_count = *calls.lock().await;
        let result = execute_team_on_runtime(
            &spec,
            "different objective",
            "identity-bound-team",
            CancellationToken::new(),
            runtime,
        )
        .await;

        assert!(result.is_err_and(|error| error.to_string().contains("different objective")));
        assert_eq!(*calls.lock().await, first_call_count);
        Ok(())
    }

    #[tokio::test]
    async fn superseded_claim_does_not_publish_a_team_result() -> Result<()> {
        let dispatch: TeamDispatchFn = Arc::new(|request| {
            let TeamDispatchRequest {
                member: name,
                task,
                context,
            } = request;
            let _cancel = context.cancellation_token().clone();
            Box::pin(async move { Ok(successful_result(name, &task)) })
        });
        let runtime = TeamRuntimeController::new(dispatch);
        let task = team_task(
            "superseded-task",
            "member",
            "test superseded settlement".to_string(),
            Vec::new(),
        );
        runtime
            .revisions()
            .create_prepared(
                "superseded-team",
                team_graph_context(
                    &TeamSpec {
                        strategy: TeamStrategy::Pipeline(vec!["member".to_string()]),
                        manager: String::new(),
                        subagents: Vec::new(),
                        config: TeamConfig::default(),
                    },
                    "test superseded settlement",
                )?,
                vec![task.clone()],
                "prepare superseded claim test".to_string(),
            )
            .await
            .map_err(|error| ReactError::Other(error.to_string()))?;
        let active_claim = match runtime.claim_task("superseded-team", &task, 1).await? {
            RuntimeTaskClaimOutcome::Claimed(claim) => claim,
            RuntimeTaskClaimOutcome::ReloadSnapshot => {
                return Err(ReactError::Other(
                    "claim unexpectedly requested a snapshot reload".to_string(),
                ));
            }
        };
        let stale_claim = TaskClaim::new(
            active_claim.revision,
            active_claim.attempt,
            active_claim.spec_hash,
        );
        let request = runtime
            .resolve_dispatch(
                "superseded-team",
                stale_claim.clone(),
                task.clone(),
                Ok(successful_result("member".to_string(), "stale output")),
            )
            .await?;
        let resolution = runtime
            .settle_resolution("superseded-team", &stale_claim, &task, request)
            .await?;

        assert_eq!(resolution, RuntimeTaskResolution::Superseded);
        assert!(
            runtime
                .task_result("superseded-team", "superseded-task")
                .await?
                .is_none()
        );
        Ok(())
    }

    #[tokio::test]
    async fn superseded_settlement_preserves_the_committed_team_result() -> Result<()> {
        for current_first in [true, false] {
            let dispatch: TeamDispatchFn = Arc::new(|request| {
                let TeamDispatchRequest {
                    member: name,
                    task,
                    context,
                } = request;
                let _cancel = context.cancellation_token().clone();
                Box::pin(async move { Ok(successful_result(name, &task)) })
            });
            let runtime = TeamRuntimeController::new(dispatch);
            let scope_id = if current_first {
                "settlement-current-first"
            } else {
                "settlement-stale-first"
            };
            let task = team_task(
                "settlement-task",
                "member",
                "test result settlement".to_string(),
                Vec::new(),
            );
            runtime
                .revisions()
                .create_prepared(
                    scope_id,
                    team_graph_context(
                        &TeamSpec {
                            strategy: TeamStrategy::Pipeline(vec!["member".to_string()]),
                            manager: String::new(),
                            subagents: Vec::new(),
                            config: TeamConfig::default(),
                        },
                        "test result settlement",
                    )?,
                    vec![task.clone()],
                    "prepare result settlement test".to_string(),
                )
                .await
                .map_err(|error| ReactError::Other(error.to_string()))?;
            let active_claim = match runtime.claim_task(scope_id, &task, 1).await? {
                RuntimeTaskClaimOutcome::Claimed(claim) => claim,
                RuntimeTaskClaimOutcome::ReloadSnapshot => {
                    return Err(ReactError::Other(
                        "claim unexpectedly requested a snapshot reload".to_string(),
                    ));
                }
            };
            let stale_claim = TaskClaim::new(
                active_claim.revision,
                active_claim.attempt,
                active_claim.spec_hash.clone(),
            );
            let committed_request = runtime
                .resolve_dispatch(
                    scope_id,
                    active_claim.clone(),
                    task.clone(),
                    Ok(successful_result("member".to_string(), "committed")),
                )
                .await?;
            let stale_request = runtime
                .resolve_dispatch(
                    scope_id,
                    stale_claim.clone(),
                    task.clone(),
                    Ok(successful_result("member".to_string(), "stale")),
                )
                .await?;

            let (committed, stale) = if current_first {
                let committed = runtime
                    .settle_resolution(scope_id, &active_claim, &task, committed_request)
                    .await?;
                let stale = runtime
                    .settle_resolution(scope_id, &stale_claim, &task, stale_request)
                    .await?;
                (committed, stale)
            } else {
                let stale = runtime
                    .settle_resolution(scope_id, &stale_claim, &task, stale_request)
                    .await?;
                let committed = runtime
                    .settle_resolution(scope_id, &active_claim, &task, committed_request)
                    .await?;
                (committed, stale)
            };
            assert_eq!(committed, RuntimeTaskResolution::Completed);
            assert_eq!(stale, RuntimeTaskResolution::Superseded);
            let output = runtime
                .task_result(scope_id, "settlement-task")
                .await?
                .ok_or_else(|| ReactError::Other("committed Team result missing".to_string()))?;
            assert_eq!(output.output, "done: committed");
        }
        Ok(())
    }

    #[tokio::test]
    async fn pipeline_passes_previous_output_to_next_prompt() -> Result<()> {
        let calls = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
        let observed = calls.clone();
        let dispatch: TeamDispatchFn = Arc::new(move |request| {
            let TeamDispatchRequest {
                member: name,
                task,
                context,
            } = request;
            let _cancel = context.cancellation_token().clone();
            let observed = observed.clone();
            Box::pin(async move {
                observed.lock().await.push((name.clone(), task.clone()));
                Ok(successful_result(name, &task))
            })
        });
        execute_team(
            &TeamSpec {
                strategy: TeamStrategy::Pipeline(vec!["first".into(), "second".into()]),
                manager: String::new(),
                subagents: Vec::new(),
                config: TeamConfig::default(),
            },
            "pipeline objective",
            "pipeline-test",
            CancellationToken::new(),
            dispatch,
        )
        .await?;

        let calls = calls.lock().await;
        let second_prompt = calls
            .get(1)
            .map(|call| call.1.as_str())
            .ok_or_else(|| ReactError::Other("second pipeline dispatch missing".to_string()))?;
        assert_eq!(
            second_prompt,
            "done: Advance this pipeline objective:\npipeline objective"
        );
        Ok(())
    }

    #[tokio::test]
    async fn skipped_team_dependency_is_a_typed_waiver_without_required_output() -> Result<()> {
        let calls = Arc::new(Mutex::new(Vec::<String>::new()));
        let observed = calls.clone();
        let dispatch: TeamDispatchFn = Arc::new(move |request| {
            let TeamDispatchRequest {
                member: name,
                task,
                context,
            } = request;
            let _cancel = context.cancellation_token().clone();
            let observed = observed.clone();
            Box::pin(async move {
                observed.lock().await.push(task.clone());
                Ok(successful_result(name, &task))
            })
        });
        let runtime = Arc::new(TeamRuntimeController::new(dispatch));
        let mut waived = team_task(
            "waived",
            "first",
            "waived pipeline stage".to_string(),
            Vec::new(),
        );
        waived.execution.status = TaskStatus::Skipped;
        let dependent = team_task(
            "dependent",
            "second",
            "continue after waiver".to_string(),
            vec!["waived".to_string()],
        );
        let spec = TeamSpec {
            strategy: TeamStrategy::Pipeline(vec!["first".to_string(), "second".to_string()]),
            manager: String::new(),
            subagents: Vec::new(),
            config: TeamConfig::default(),
        };
        runtime
            .revisions()
            .create_prepared(
                "skip-waiver-team",
                team_graph_context(&spec, "continue after waiver")?,
                vec![waived, dependent],
                "prepare skipped dependency waiver".to_string(),
            )
            .await
            .map_err(|error| ReactError::Other(error.to_string()))?;
        let service = RuntimeTaskService::new(runtime, RuntimeTaskServiceConfig::default());

        assert_eq!(
            service
                .execute("skip-waiver-team", CancellationToken::new())
                .await?,
            RuntimeDagOutcome::Completed
        );
        let prompts = calls.lock().await;
        assert_eq!(prompts.len(), 1);
        let prompt = prompts
            .first()
            .ok_or_else(|| ReactError::Other("waiver prompt is missing".to_string()))?;
        assert!(prompt.contains("Dependency 'waived' was explicitly skipped"));
        Ok(())
    }

    #[tokio::test]
    async fn failed_member_blocks_synthesis() {
        let calls = Arc::new(Mutex::new(Vec::<String>::new()));
        let observed = calls.clone();
        let dispatch: TeamDispatchFn = Arc::new(move |request| {
            let TeamDispatchRequest {
                member: name,
                task,
                context,
            } = request;
            let _cancel = context.cancellation_token().clone();
            let observed = observed.clone();
            Box::pin(async move {
                observed.lock().await.push(name.clone());
                if name == "manager" && !task.contains("Completed dependency") {
                    Ok(manager_plan_result(
                        name,
                        r#"{"tasks":[{"id":"broken-task","subagent":"broken","description":"one concrete task","depends_on":[]}]}"#,
                    ))
                } else if name == "broken" {
                    Err(ReactError::Other("scripted member failure".to_string()))
                } else {
                    Ok(successful_result(name, &task))
                }
            })
        });
        let result = execute_team(
            &TeamSpec {
                strategy: TeamStrategy::ManagerSubagent,
                manager: "manager".to_string(),
                subagents: vec!["broken".to_string()],
                config: TeamConfig::default(),
            },
            "objective",
            "failure-test",
            CancellationToken::new(),
            dispatch,
        )
        .await;
        assert!(result.is_err());
        assert_eq!(
            calls.lock().await.as_slice(),
            &["manager".to_string(), "broken".to_string()]
        );
    }

    #[tokio::test]
    async fn pre_cancelled_team_dispatches_nothing() {
        let calls = Arc::new(Mutex::new(0usize));
        let observed = calls.clone();
        let dispatch: TeamDispatchFn = Arc::new(move |request| {
            let TeamDispatchRequest {
                member: name,
                task,
                context,
            } = request;
            let _cancel = context.cancellation_token().clone();
            let observed = observed.clone();
            Box::pin(async move {
                let mut count = observed.lock().await;
                *count = count.saturating_add(1);
                drop(count);
                Ok(successful_result(name, &task))
            })
        });
        let cancel = CancellationToken::new();
        cancel.cancel();
        let result = execute_team(
            &TeamSpec {
                strategy: TeamStrategy::Pipeline(vec!["first".to_string()]),
                manager: String::new(),
                subagents: Vec::new(),
                config: TeamConfig::default(),
            },
            "objective",
            "cancel-test",
            cancel,
            dispatch,
        )
        .await;
        assert!(result.is_err_and(|error| error.to_string().contains("cancelled")));
        assert_eq!(*calls.lock().await, 0);
    }

    #[tokio::test]
    async fn unresolved_member_is_a_graph_failure() {
        let dispatch: TeamDispatchFn = Arc::new(|request| {
            let TeamDispatchRequest {
                member: name,
                task: _task,
                context,
            } = request;
            let _cancel = context.cancellation_token().clone();
            Box::pin(async move {
                Err(ReactError::Other(format!(
                    "Team Subagent '{name}' not registered"
                )))
            })
        });
        let result = execute_team(
            &TeamSpec {
                strategy: TeamStrategy::Pipeline(vec!["missing".to_string()]),
                manager: String::new(),
                subagents: Vec::new(),
                config: TeamConfig::default(),
            },
            "objective",
            "missing-test",
            CancellationToken::new(),
            dispatch,
        )
        .await;
        assert!(result.is_err_and(|error| error.to_string().contains("not registered")));
    }

    #[test]
    fn empty_pipeline_is_rejected() {
        let error = compile_team_graph(
            &TeamSpec {
                strategy: TeamStrategy::Pipeline(Vec::new()),
                manager: String::new(),
                subagents: Vec::new(),
                config: TeamConfig::default(),
            },
            "task",
        )
        .err();
        assert!(error.is_some_and(|error| error.to_string().contains("at least one")));
    }

    #[test]
    fn interrupt_projection_preserves_capacity_and_identity_conflict() {
        assert_eq!(
            runtime_interrupt_projection_error(SubagentControlError::PendingCapacityExceeded {
                limit: 17
            }),
            RuntimeTaskAttemptInterruptProjectionError::PendingCapacityExceeded { limit: 17 }
        );
        assert_eq!(
            runtime_interrupt_projection_error(SubagentControlError::IdentityConflict {
                task_id: "task".to_string(),
                attempt: 2,
                expected_execution_id: "expected".to_string(),
                actual_execution_id: "actual".to_string(),
            }),
            RuntimeTaskAttemptInterruptProjectionError::IdentityConflict {
                task_id: "task".to_string(),
                attempt: 2,
                expected_execution_id: "expected".to_string(),
                actual_execution_id: "actual".to_string(),
            }
        );
    }

    #[test]
    fn manager_without_executable_subagents_is_rejected() {
        let error = validate_team_spec(&TeamSpec {
            strategy: TeamStrategy::ManagerSubagent,
            manager: "manager".to_string(),
            subagents: Vec::new(),
            config: TeamConfig::default(),
        })
        .err();
        assert!(error.is_some_and(|error| error.to_string().contains("at least one")));
    }

    #[test]
    fn manager_plan_rejects_unstructured_text() {
        let spec = TeamSpec {
            strategy: TeamStrategy::ManagerSubagent,
            manager: "manager".to_string(),
            subagents: vec!["researcher".to_string()],
            config: TeamConfig::default(),
        };
        let error = manager_subagent::expand_graph(
            &spec,
            "review the repository",
            "inspect implementation\ninspect tests",
        )
        .err();
        assert!(error.is_some_and(|error| error.to_string().contains("typed JSON")));
    }

    #[test]
    fn manager_plan_rejects_unknown_subagent() {
        let spec = TeamSpec {
            strategy: TeamStrategy::ManagerSubagent,
            manager: "manager".to_string(),
            subagents: vec!["researcher".to_string()],
            config: TeamConfig::default(),
        };
        let error = manager_subagent::expand_graph(
            &spec,
            "review the repository",
            r#"{"tasks":[{"id":"review","subagent":"writer","description":"review code","depends_on":[]}]}"#,
        )
        .err();
        assert!(error.is_some_and(|error| error.to_string().contains("unknown Subagent 'writer'")));
    }

    #[test]
    fn manager_plan_rejects_unknown_dependency() {
        let spec = TeamSpec {
            strategy: TeamStrategy::ManagerSubagent,
            manager: "manager".to_string(),
            subagents: vec!["researcher".to_string()],
            config: TeamConfig::default(),
        };
        let error = manager_subagent::expand_graph(
            &spec,
            "review the repository",
            r#"{"tasks":[{"id":"review","subagent":"researcher","description":"review code","depends_on":["missing"]}]}"#,
        )
        .err();
        assert!(
            error.is_some_and(|error| error.to_string().contains("unknown dependency 'missing'"))
        );
    }

    #[test]
    fn manager_plan_precedes_and_ignores_the_framework_result_contract() -> Result<()> {
        let spec = TeamSpec {
            strategy: TeamStrategy::ManagerSubagent,
            manager: "manager".to_string(),
            subagents: vec!["researcher".to_string()],
            config: TeamConfig::default(),
        };
        let initial = manager_subagent::initial_graph(&spec, "review the repository");
        let prompt = initial
            .tasks
            .first()
            .map(|task| task.spec.description.as_str())
            .ok_or_else(|| ReactError::Other("Team manager plan task missing".to_string()))?;
        assert!(prompt.contains("before any framework-owned final `## Result` section"));

        let output = r#"```json
{"tasks":[{"id":"review","subagent":"researcher","description":"review code","depends_on":[]}]}
```
## Result
```json
{"contract_version":2,"status":"completed","summary":"Created a typed Team plan","artifacts":[],"evidence":[],"remaining_work":[]}
```"#;
        let expanded = manager_subagent::expand_graph(&spec, "review the repository", output)?;
        assert_eq!(expanded.tasks.len(), 2);
        assert_eq!(
            expanded.tasks.first().map(|task| task.spec.id.as_str()),
            Some("team-member-0")
        );
        assert_eq!(
            expanded.tasks.last().map(|task| task.spec.id.as_str()),
            Some(manager_subagent::synthesis_task_id())
        );
        Ok(())
    }

    #[test]
    fn manager_plan_uses_the_canonical_validator_task_limit() {
        let spec = TeamSpec {
            strategy: TeamStrategy::ManagerSubagent,
            manager: "manager".to_string(),
            subagents: vec!["researcher".to_string()],
            config: TeamConfig::default(),
        };
        let validator_limit = PlanValidator::default().max_tasks;
        let team_limit = validator_limit.saturating_sub(2);
        let tasks = (0..validator_limit.saturating_sub(1))
            .map(|index| {
                serde_json::json!({
                    "id": format!("task-{index}"),
                    "subagent": "researcher",
                    "description": format!("inspect item {index}"),
                    "depends_on": [],
                })
            })
            .collect::<Vec<_>>();
        let plan = serde_json::json!({ "tasks": tasks }).to_string();
        let error = manager_subagent::expand_graph(&spec, "review the repository", &plan).err();
        assert!(error.is_some_and(|error| {
            error
                .to_string()
                .contains(&format!("maximum is {team_limit}"))
        }));
    }

    #[test]
    fn team_identity_allows_product_metadata_extensions_but_not_rewrites()
    -> std::result::Result<(), String> {
        let mut expected = team_task(
            "pipeline-task",
            "researcher",
            "inspect implementation".to_string(),
            Vec::new(),
        );
        set_team_task_phase(&mut expected, "pipeline").map_err(|error| error.to_string())?;
        let mut extended = expected.spec.clone();
        if let Some(fields) = extended.extension.as_object_mut() {
            fields.insert(
                "product_projection".to_string(),
                serde_json::json!({ "card": "compact" }),
            );
        }
        assert!(team_task_specs_match(&extended, &expected.spec));

        if let Some(fields) = extended.extension.as_object_mut() {
            fields.insert("phase".to_string(), serde_json::json!("manager_member"));
        }
        assert!(!team_task_specs_match(&extended, &expected.spec));
        Ok(())
    }

    #[tokio::test]
    async fn team_timeout_cancels_the_root_token() {
        let settled = Arc::new(AtomicBool::new(false));
        let observed_settlement = settled.clone();
        let dispatch: TeamDispatchFn = Arc::new(move |request| {
            let TeamDispatchRequest {
                member: _name,
                task: _task,
                context,
            } = request;
            let cancel = context.cancellation_token().clone();
            let observed_settlement = observed_settlement.clone();
            Box::pin(async move {
                cancel.cancelled().await;
                observed_settlement.store(true, Ordering::SeqCst);
                Ok(SubagentResult::cancelled(
                    "slow",
                    "cancelled",
                    ExecutionMode::Sync,
                ))
            })
        });
        let cancel = CancellationToken::new();
        let result = execute_team_with_runtime_dispatch(
            &TeamSpec {
                strategy: TeamStrategy::Pipeline(vec!["slow".to_string()]),
                manager: String::new(),
                subagents: Vec::new(),
                config: TeamConfig {
                    max_concurrent: 1,
                    default_timeout_secs: 1,
                },
            },
            "wait",
            "timeout-test",
            cancel.clone(),
            dispatch,
        )
        .await;
        assert!(result.is_err_and(|error| error.to_string().contains("timed out")));
        assert!(cancel.is_cancelled());
        assert!(settled.load(Ordering::SeqCst));
    }

    struct RecordingAgent {
        name: String,
        response: String,
        inputs: Arc<Mutex<Vec<String>>>,
        executed: Arc<AtomicBool>,
    }

    impl Agent for RecordingAgent {
        fn name(&self) -> &str {
            &self.name
        }

        fn model_name(&self) -> &str {
            "recording"
        }

        fn system_prompt(&self) -> &str {
            ""
        }

        fn execute<'a>(&'a self, task: &'a str) -> BoxFuture<'a, CoreResult<String>> {
            Box::pin(async move {
                self.inputs.lock().await.push(task.to_string());
                self.executed.store(true, Ordering::SeqCst);
                Ok(self.response.clone())
            })
        }

        fn execute_stream<'a>(
            &'a self,
            task: &'a str,
        ) -> BoxFuture<'a, CoreResult<BoxStream<'a, CoreResult<AgentEvent>>>> {
            Box::pin(async move {
                let output = self.execute(task).await?;
                Ok(Box::pin(stream::once(
                    async move { Ok(AgentEvent::FinalAnswer(output)) },
                )) as BoxStream<'a, CoreResult<AgentEvent>>)
            })
        }
    }

    #[tokio::test]
    async fn programmatic_team_uses_shared_subagent_executor_and_exact_pipeline_input() -> Result<()>
    {
        let first_inputs = Arc::new(Mutex::new(Vec::new()));
        let second_inputs = Arc::new(Mutex::new(Vec::new()));
        let first_executed = Arc::new(AtomicBool::new(false));
        let second_executed = Arc::new(AtomicBool::new(false));
        let team = TeamAgent::builder()
            .name("object-team")
            .subagent(
                "first",
                Box::new(RecordingAgent {
                    name: "first".to_string(),
                    response: "first-output".to_string(),
                    inputs: first_inputs.clone(),
                    executed: first_executed.clone(),
                }),
                SubagentDefinition::simple_sync("placeholder-first"),
            )
            .subagent(
                "second",
                Box::new(RecordingAgent {
                    name: "second".to_string(),
                    response: "second-output".to_string(),
                    inputs: second_inputs.clone(),
                    executed: second_executed.clone(),
                }),
                SubagentDefinition::simple_sync("placeholder-second"),
            )
            .strategy(TeamStrategy::Pipeline(vec![
                "first".to_string(),
                "second".to_string(),
            ]))
            .build()
            .map_err(ReactError::Other)?;

        let output = team
            .execute("pipeline-objective")
            .await
            .map_err(ReactError::Other)?;
        assert_eq!(output, "second-output");
        assert!(first_executed.load(Ordering::SeqCst));
        assert!(second_executed.load(Ordering::SeqCst));
        assert!(
            second_inputs
                .lock()
                .await
                .first()
                .is_some_and(|input| input.contains("first-output"))
        );
        Ok(())
    }

    #[tokio::test]
    async fn team_runtime_handle_shares_execution_and_exact_control_authority() -> Result<()> {
        let dispatch: TeamDispatchFn = Arc::new(|request| {
            let TeamDispatchRequest {
                member,
                task: _,
                context,
            } = request;
            Box::pin(async move {
                context.cancellation_token().cancelled().await;
                Ok(SubagentResult::cancelled(
                    member,
                    "cancelled by exact Team runtime control",
                    ExecutionMode::Sync,
                ))
            })
        });
        let team = Arc::new(
            TeamAgent::builder()
                .name("controlled-team")
                .subagent(
                    "slow",
                    Box::new(RecordingAgent {
                        name: "slow".to_string(),
                        response: "unused".to_string(),
                        inputs: Arc::new(Mutex::new(Vec::new())),
                        executed: Arc::new(AtomicBool::new(false)),
                    }),
                    SubagentDefinition::simple_sync("slow"),
                )
                .strategy(TeamStrategy::Pipeline(vec!["slow".to_string()]))
                .member_dispatch_controller(closure_dispatch_controller(
                    dispatch,
                    TeamDispatchControl::default(),
                ))
                .build()
                .map_err(ReactError::Other)?,
        );
        let handle = team.runtime_handle().await.map_err(ReactError::Other)?;
        let same_handle = team.runtime_handle().await.map_err(ReactError::Other)?;
        assert!(Arc::ptr_eq(&handle, &same_handle));
        assert_eq!(team.run_id(), Some(handle.run_id()));

        let execution = tokio::spawn({
            let team = Arc::clone(&team);
            async move { team.execute("wait for control").await }
        });
        let (task_id, claim) = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Ok(snapshot) = handle.snapshot().await
                    && let Some((task_id, claim)) = snapshot.tasks.iter().find_map(|task| {
                        task.execution
                            .claim
                            .clone()
                            .map(|claim| (task.spec.id.clone(), claim))
                    })
                {
                    break (task_id, claim);
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| ReactError::Other("Team claim was not observable".to_string()))?;
        let receipt = handle.request_attempt_interrupt(&task_id, &claim).await?;
        assert!(receipt.requested);
        let error = tokio::time::timeout(Duration::from_secs(2), execution)
            .await
            .map_err(|_| ReactError::Other("Team exact interrupt did not settle".to_string()))?
            .map_err(|error| ReactError::Other(format!("Team execution panicked: {error}")))?
            .err()
            .ok_or_else(|| {
                ReactError::Other("cancelled Team unexpectedly succeeded".to_string())
            })?;
        assert!(error.contains("cancelled"));
        assert_eq!(
            handle
                .snapshot()
                .await?
                .tasks
                .iter()
                .find(|task| task.spec.id == task_id)
                .map(|task| &task.execution.status),
            Some(&TaskStatus::Cancelled)
        );
        Ok(())
    }

    #[tokio::test]
    async fn programmatic_team_preserves_shared_agent_identity() -> Result<()> {
        let shared: Arc<dyn Agent> = Arc::new(RecordingAgent {
            name: "shared".to_string(),
            response: "shared-output".to_string(),
            inputs: Arc::new(Mutex::new(Vec::new())),
            executed: Arc::new(AtomicBool::new(false)),
        });
        let team = TeamAgent::builder()
            .subagent_shared(
                "shared",
                shared.clone(),
                SubagentDefinition::simple_sync("placeholder"),
            )
            .strategy(TeamStrategy::Pipeline(vec!["shared".to_string()]))
            .build()
            .map_err(ReactError::Other)?;
        let stored = team
            .team()
            .get_member("shared")
            .map(|member| member.agent.clone())
            .ok_or_else(|| ReactError::Other("shared Team member missing".to_string()))?;
        assert!(Arc::ptr_eq(&shared, &stored));
        Ok(())
    }

    #[test]
    fn programmatic_team_rejects_duplicate_member_names() {
        let first: Arc<dyn Agent> = Arc::new(RecordingAgent {
            name: "duplicate".to_string(),
            response: "first".to_string(),
            inputs: Arc::new(Mutex::new(Vec::new())),
            executed: Arc::new(AtomicBool::new(false)),
        });
        let second: Arc<dyn Agent> = Arc::new(RecordingAgent {
            name: "duplicate".to_string(),
            response: "second".to_string(),
            inputs: Arc::new(Mutex::new(Vec::new())),
            executed: Arc::new(AtomicBool::new(false)),
        });
        let result = TeamAgent::builder()
            .subagent_shared("duplicate", first, SubagentDefinition::simple_sync("first"))
            .subagent_shared(
                "duplicate",
                second,
                SubagentDefinition::simple_sync("second"),
            )
            .strategy(TeamStrategy::Pipeline(vec!["duplicate".to_string()]))
            .build();
        assert!(result.is_err_and(|error| error.contains("already registered")));
    }
}
