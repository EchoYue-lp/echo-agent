//! Team intent compiled onto the canonical revisioned task runtime.
//!
//! Declarative [`TeamSpec`] values and programmatic [`Team`] values both become
//! one revisioned task graph. Programmatic composition may own Agent handles,
//! but dependency state, ready-frontier selection, cancellation, and terminal
//! settlement remain exclusively owned by [`RuntimeDagExecutor`].

mod manager_subagent;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
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
    RevisionedTaskGraph, RevisionedTaskStore, RuntimeClaimAbandonment, RuntimeDagController,
    RuntimeDagExecutor, RuntimeDagExecutorConfig, RuntimeDagOutcome, RuntimePlanSnapshot,
    RuntimeTaskClaimOutcome, RuntimeTaskResolution, Task, TaskClaim, TaskExecution,
    TaskGraphContext, TaskGraphExecutionMode, TaskKind, TaskPlanPatch, TaskPlanPatchOp,
    TaskRevisionError, TaskRevisionService, TaskSpec, TaskStatus, TaskSubagentContext,
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

    fn dispatch(&self) -> TeamDispatchFn {
        let members = self.members.clone();
        let runtime = Arc::new(tokio::sync::OnceCell::<Arc<SubagentExecutor>>::new());
        let parent_agent = self.name.clone();
        let config = self.config.clone();
        Arc::new(move |name, task, cancel| {
            let member = members.get(&name).cloned();
            let members = members.clone();
            let runtime = runtime.clone();
            let parent_agent = parent_agent.clone();
            let config = config.clone();
            Box::pin(async move {
                let member = member.ok_or_else(|| format!("Team member '{name}' not found"))?;
                let _execution = member.execution_gate.lock().await;
                let executor = runtime
                    .get_or_init(|| async move {
                        let registry = Arc::new(SubagentRegistry::new());
                        for member in members.values() {
                            registry
                                .register_shared(member.definition.clone(), member.agent.clone())
                                .await;
                        }
                        Arc::new(SubagentExecutor::new(
                            registry,
                            SubagentExecutorConfig {
                                max_concurrent_forks: config.max_concurrent.max(1),
                                default_timeout_secs: config.default_timeout_secs,
                                ..SubagentExecutorConfig::default()
                            },
                        ))
                    })
                    .await;
                executor
                    .dispatch(DispatchRequest {
                        agent_name: name,
                        task,
                        mode_override: Some(ExecutionMode::Sync),
                        cancel,
                        parent_agent,
                        parent_context: None,
                        delegation_policy: NestedDelegationPolicy::default(),
                        runtime_context: None,
                        message: None,
                        prompt_payload: None,
                        constraints: Vec::new(),
                        background: false,
                    })
                    .await
                    .map_err(|error| error.to_string())
            })
        })
    }
}

/// Programmatic Team facade. It owns only concrete Agent handles and delegates
/// all graph semantics to [`execute_team`].
pub struct TeamAgent {
    team: Team,
    strategy: TeamStrategy,
    run_id: Option<String>,
    cancel: CancellationToken,
    member_dispatch: Option<TeamDispatchFn>,
    runtime: OnceCell<Arc<TeamRuntimeController>>,
}

impl TeamAgent {
    pub fn new(team: Team, strategy: TeamStrategy) -> Self {
        Self {
            team,
            strategy,
            run_id: None,
            cancel: CancellationToken::new(),
            member_dispatch: None,
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
        self.run_id.as_deref()
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
        let stable_run_id = self.run_id.clone();
        let run_id = stable_run_id
            .clone()
            .unwrap_or_else(|| format!("team-{}", uuid::Uuid::new_v4().as_simple()));
        let dispatch = self
            .member_dispatch
            .clone()
            .unwrap_or_else(|| self.team.dispatch());
        let runtime = if stable_run_id.is_some() {
            self.runtime
                .get_or_init(|| async move { Arc::new(TeamRuntimeController::new(dispatch)) })
                .await
                .clone()
        } else {
            Arc::new(TeamRuntimeController::new(dispatch))
        };
        execute_team_on_runtime(&spec, task, &run_id, self.cancel.child_token(), runtime)
            .await
            .map_err(|error| error.to_string())
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
    member_dispatch: Option<TeamDispatchFn>,
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
            member_dispatch: None,
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

    pub fn member_dispatch(mut self, dispatch: TeamDispatchFn) -> Self {
        self.member_dispatch = Some(dispatch);
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
        team_agent.member_dispatch = self.member_dispatch;
        Ok(team_agent)
    }
}

/// Canonical member execution adapter supplied by [`super::SubagentExecutor`].
pub type TeamDispatchFn = Arc<
    dyn Fn(
            String,
            String,
            CancellationToken,
        ) -> BoxFuture<'static, std::result::Result<SubagentResult, String>>
        + Send
        + Sync,
>;

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
/// [`RuntimeDagExecutor`] remains the only dependency, claim, cancellation,
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
    validate_team_spec(spec)?;
    let timeout = spec.config.default_timeout_secs;
    let timeout_cancel = cancel.clone();
    let execution = execute_team_inner(spec, objective, run_id, cancel, runtime);
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
                        if AgentFailure::from_react_error(&error).terminal_kind
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
) -> Result<TeamExecutionResult>
where
    R: TeamRuntime,
{
    let service = runtime.revisions();
    let executor = RuntimeDagExecutor::new(
        runtime.clone(),
        RuntimeDagExecutorConfig {
            max_concurrent_subagents: spec.config.max_concurrent.max(1),
            ..RuntimeDagExecutorConfig::default()
        },
    );
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
            drive_team_graph(&executor, run_id, cancel.child_token()).await?;
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
        drive_team_graph(&executor, run_id, cancel.child_token()).await?;
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
        drive_team_graph(&executor, run_id, cancel).await?;
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
    let mut stored_without_metadata = stored.clone();
    stored_without_metadata.metadata = serde_json::Value::Null;
    let mut expected_without_metadata = expected.clone();
    expected_without_metadata.metadata = serde_json::Value::Null;
    if stored_without_metadata != expected_without_metadata {
        return false;
    }
    match &expected.metadata {
        serde_json::Value::Null => true,
        serde_json::Value::Object(expected_fields) => {
            stored.metadata.as_object().is_some_and(|stored_fields| {
                expected_fields
                    .iter()
                    .all(|(key, value)| stored_fields.get(key) == Some(value))
            })
        }
        expected_metadata => &stored.metadata == expected_metadata,
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
    executor: &RuntimeDagExecutor<R>,
    run_id: &str,
    cancel: CancellationToken,
) -> Result<()>
where
    R: TeamRuntime,
{
    match executor.execute(run_id, cancel).await? {
        RuntimeDagOutcome::Completed => {}
        RuntimeDagOutcome::Failed { error, .. } | RuntimeDagOutcome::Paused { error, .. } => {
            return Err(ReactError::Other(format!("Team graph failed: {error}")));
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
                task.spec.metadata = serde_json::json!({ "team_phase": "pipeline" });
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
    let spec = TaskSpec {
        id: id.to_string(),
        title: description.clone(),
        description,
        kind: TaskKind::Investigation,
        agent_role: member.to_string(),
        depends_on,
        files: Vec::new(),
        allowed_tools: Vec::new(),
        required_artifacts: Vec::new(),
        execution_checks: Vec::new(),
        acceptance_criteria: Vec::new(),
        max_retries: 0,
        metadata: serde_json::Value::Null,
    };
    Task {
        execution: TaskExecution::pending(id),
        spec,
    }
}

struct TeamRuntimeController {
    store: Arc<InMemoryRevisionedTaskStore>,
    revisions: TaskRevisionService,
    dispatch: TeamDispatchFn,
    outputs: Mutex<HashMap<String, HashMap<String, SubagentResult>>>,
    settlement: Mutex<()>,
}

impl TeamRuntimeController {
    fn new(dispatch: TeamDispatchFn) -> Self {
        let store = Arc::new(InMemoryRevisionedTaskStore::new());
        let revisions = TaskRevisionService::new(
            store.clone(),
            Arc::new(DefaultTaskToolPolicy::new("team-runtime")),
        );
        Self {
            store,
            revisions,
            dispatch,
            outputs: Mutex::new(HashMap::new()),
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

    async fn dispatch_task(
        &self,
        context: TaskSubagentContext,
        _claim: TaskClaim,
        task: Task,
    ) -> Result<Self::DispatchOutput> {
        let outputs = self.outputs.lock().await;
        let run_outputs = outputs.get(&context.run_id);
        let mut dependency_outputs = Vec::with_capacity(task.spec.depends_on.len());
        for dependency in &task.spec.depends_on {
            let output = run_outputs
                .and_then(|values| values.get(dependency))
                .ok_or_else(|| {
                    ReactError::Other(format!(
                        "Team dependency '{dependency}' completed without a reusable result"
                    ))
                })?;
            dependency_outputs.push((dependency, output));
        }
        let pipeline_phase = task
            .spec
            .metadata
            .get("team_phase")
            .and_then(serde_json::Value::as_str)
            == Some("pipeline");
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
        (self.dispatch)(task.spec.agent_role, prompt, context.cancel)
            .await
            .map_err(ReactError::Other)
    }

    async fn resolve_dispatch(
        &self,
        run_id: &str,
        claim: TaskClaim,
        task: Task,
        dispatch: Result<Self::DispatchOutput>,
    ) -> Result<RuntimeTaskResolution> {
        let _settlement = self.settlement.lock().await;
        let (status, resolution, output) = match dispatch {
            Ok(output) if output.outcome.status == SubagentStatus::Completed => (
                TaskStatus::Completed,
                RuntimeTaskResolution::Completed,
                Some(output),
            ),
            Ok(output) if output.outcome.status == SubagentStatus::Cancelled => (
                TaskStatus::Cancelled,
                RuntimeTaskResolution::Cancelled,
                None,
            ),
            Ok(output) => {
                let error = if output.outcome.summary.is_empty() {
                    output.output.clone()
                } else {
                    output.outcome.summary.clone()
                };
                (
                    TaskStatus::Failed(error.clone()),
                    RuntimeTaskResolution::Failed { error },
                    None,
                )
            }
            Err(error) => {
                let message = error.to_string();
                (
                    TaskStatus::Failed(message.clone()),
                    RuntimeTaskResolution::Failed { error: message },
                    None,
                )
            }
        };
        let staged_output = if let Some(output) = output {
            Some(
                self.outputs
                    .lock()
                    .await
                    .entry(run_id.to_string())
                    .or_default()
                    .insert(task.spec.id.clone(), output),
            )
        } else {
            None
        };
        let applied = match self
            .store
            .settle_runtime_claim(run_id, &task.spec.id, &claim, status)
            .await
        {
            Ok(applied) => applied,
            Err(error) => {
                if let Some(previous) = staged_output {
                    self.restore_output(run_id, &task.spec.id, previous).await;
                }
                return Err(ReactError::Other(error.to_string()));
            }
        };
        if !applied {
            if let Some(previous) = staged_output {
                self.restore_output(run_id, &task.spec.id, previous).await;
            }
            return Ok(RuntimeTaskResolution::Superseded);
        }
        Ok(resolution)
    }

    async fn abandon_claim(
        &self,
        run_id: &str,
        claim: &TaskClaim,
        task: &Task,
        abandonment: RuntimeClaimAbandonment,
    ) -> Result<()> {
        let status = match abandonment {
            RuntimeClaimAbandonment::Cancelled => TaskStatus::Cancelled,
            RuntimeClaimAbandonment::Failed { error } => TaskStatus::Failed(error),
        };
        self.store
            .settle_runtime_claim(run_id, &task.spec.id, claim, status)
            .await
            .map_err(|error| ReactError::Other(error.to_string()))?;
        Ok(())
    }

    async fn block_task(&self, run_id: &str, task: &Task, reason: &str) -> Result<()> {
        self.store
            .block_runtime_task(run_id, &task.spec.id, reason)
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

impl TeamRuntimeController {
    async fn restore_output(&self, run_id: &str, task_id: &str, previous: Option<SubagentResult>) {
        let mut outputs = self.outputs.lock().await;
        let run_outputs = outputs.entry(run_id.to_string()).or_default();
        match previous {
            Some(output) => {
                run_outputs.insert(task_id.to_string(), output);
            }
            None => {
                run_outputs.remove(task_id);
            }
        }
        if run_outputs.is_empty() {
            outputs.remove(run_id);
        }
    }
}

fn aggregate_usage<'a>(outputs: impl Iterator<Item = &'a SubagentResult>) -> Option<LlmUsageStats> {
    let mut total = LlmUsageStats::default();
    let mut has_usage = false;
    let mut models = BTreeSet::new();
    for result in outputs {
        let Some(usage) = &result.usage else {
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
        let dispatch: TeamDispatchFn = Arc::new(move |name, task, _cancel| {
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
        let dispatch: TeamDispatchFn = Arc::new(|name, task, _cancel| {
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
        let dispatch: TeamDispatchFn = Arc::new(move |name, task, _cancel| {
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
        let dispatch: TeamDispatchFn = Arc::new(move |name, task, _cancel| {
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

        let first = execute_team_on_runtime(
            &spec,
            "review the repository",
            "resumable-team",
            CancellationToken::new(),
            runtime.clone(),
        )
        .await?;
        let first_call_count = *calls.lock().await;
        let resumed = execute_team_on_runtime(
            &spec,
            "review the repository",
            "resumable-team",
            CancellationToken::new(),
            runtime,
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
        let dispatch: TeamDispatchFn = Arc::new(move |name, task, _cancel| {
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
        let dispatch: TeamDispatchFn = Arc::new(|name, task, _cancel| {
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
        let resolution = runtime
            .resolve_dispatch(
                "superseded-team",
                stale_claim,
                task,
                Ok(successful_result("member".to_string(), "stale output")),
            )
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
        let dispatch: TeamDispatchFn = Arc::new(|name, task, _cancel| {
            Box::pin(async move { Ok(successful_result(name, &task)) })
        });
        let runtime = TeamRuntimeController::new(dispatch);
        let task = team_task(
            "settlement-task",
            "member",
            "test result settlement".to_string(),
            Vec::new(),
        );
        runtime
            .revisions()
            .create_prepared(
                "settlement-team",
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
        let active_claim = match runtime.claim_task("settlement-team", &task, 1).await? {
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
        let committed = runtime
            .resolve_dispatch(
                "settlement-team",
                active_claim,
                task.clone(),
                Ok(successful_result("member".to_string(), "committed")),
            )
            .await?;
        assert_eq!(committed, RuntimeTaskResolution::Completed);
        let stale = runtime
            .resolve_dispatch(
                "settlement-team",
                stale_claim,
                task,
                Ok(successful_result("member".to_string(), "stale")),
            )
            .await?;
        assert_eq!(stale, RuntimeTaskResolution::Superseded);
        let output = runtime
            .task_result("settlement-team", "settlement-task")
            .await?
            .ok_or_else(|| ReactError::Other("committed Team result missing".to_string()))?;
        assert_eq!(output.output, "done: committed");
        Ok(())
    }

    #[tokio::test]
    async fn pipeline_passes_previous_output_to_next_prompt() -> Result<()> {
        let calls = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
        let observed = calls.clone();
        let dispatch: TeamDispatchFn = Arc::new(move |name, task, _cancel| {
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
    async fn failed_member_blocks_synthesis() {
        let calls = Arc::new(Mutex::new(Vec::<String>::new()));
        let observed = calls.clone();
        let dispatch: TeamDispatchFn = Arc::new(move |name, task, _cancel| {
            let observed = observed.clone();
            Box::pin(async move {
                observed.lock().await.push(name.clone());
                if name == "manager" && !task.contains("Completed dependency") {
                    Ok(manager_plan_result(
                        name,
                        r#"{"tasks":[{"id":"broken-task","subagent":"broken","description":"one concrete task","depends_on":[]}]}"#,
                    ))
                } else if name == "broken" {
                    Err("scripted member failure".to_string())
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
        let dispatch: TeamDispatchFn = Arc::new(move |name, task, _cancel| {
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
        let dispatch: TeamDispatchFn = Arc::new(|name, _task, _cancel| {
            Box::pin(async move { Err(format!("Team Subagent '{name}' not registered")) })
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
{"contract_version":1,"status":"completed","summary":"Created a typed Team plan","artifacts":[],"verification":[],"remaining_work":[],"touched_files":{"read":[],"written":[]}}
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
    fn team_identity_allows_product_metadata_extensions_but_not_rewrites() {
        let mut expected = team_task(
            "pipeline-task",
            "researcher",
            "inspect implementation".to_string(),
            Vec::new(),
        );
        expected.spec.metadata = serde_json::json!({ "team_phase": "pipeline" });
        let mut extended = expected.spec.clone();
        extended.metadata = serde_json::json!({
            "team_phase": "pipeline",
            "product_projection": { "card": "compact" }
        });
        assert!(team_task_specs_match(&extended, &expected.spec));

        extended.metadata = serde_json::json!({
            "team_phase": "manager_member",
            "product_projection": { "card": "compact" }
        });
        assert!(!team_task_specs_match(&extended, &expected.spec));
    }

    #[tokio::test]
    async fn team_timeout_cancels_the_root_token() {
        let settled = Arc::new(AtomicBool::new(false));
        let observed_settlement = settled.clone();
        let dispatch: TeamDispatchFn = Arc::new(move |_name, _task, cancel| {
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
