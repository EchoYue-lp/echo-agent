//! Team intent compiled onto the canonical revisioned task runtime.
//!
//! This module owns no Agent instances, relationship store, ready-frontier
//! loop, or terminal classifier. A [`TeamSpec`] becomes one revisioned task
//! graph, and [`RuntimeDagExecutor`] is the only scheduler.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use echo_core::error::{ReactError, Result};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use echo_orchestration::tasks::{
    DefaultTaskToolPolicy, InMemoryRevisionedTaskStore, RevisionedTaskStore,
    RuntimeClaimAbandonment, RuntimeDagController, RuntimeDagExecutor, RuntimeDagExecutorConfig,
    RuntimeDagOutcome, RuntimePlanSnapshot, RuntimeTaskClaimOutcome, RuntimeTaskResolution, Task,
    TaskClaim, TaskExecution, TaskGraphContext, TaskGraphExecutionMode, TaskKind,
    TaskRevisionService, TaskSpec, TaskStatus, TaskSubagentContext,
};

use super::types::{SubagentResult, SubagentStatus};
use super::usage::LlmUsageStats;

/// How registered Subagents collaborate inside one canonical task graph.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamStrategy {
    /// Manager plans, members execute, then the manager synthesizes.
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
}

/// Runtime limits for a Team graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamConfig {
    /// Maximum concurrent Subagent dispatches in one ready wave.
    pub max_concurrent: usize,
}

impl Default for TeamConfig {
    fn default() -> Self {
        Self { max_concurrent: 5 }
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

/// Canonical member execution adapter supplied by [`super::SubagentExecutor`].
pub type TeamDispatchFn = Arc<
    dyn Fn(String, String) -> BoxFuture<'static, std::result::Result<SubagentResult, String>>
        + Send
        + Sync,
>;

/// Terminal output of one Team graph execution.
pub struct TeamExecutionResult {
    pub output: String,
    pub usage: Option<LlmUsageStats>,
}

struct CompiledTeamGraph {
    tasks: Vec<Task>,
    terminal_task_id: String,
}

/// Execute Team intent through the framework's single revisioned DAG runtime.
pub async fn execute_team(
    spec: &TeamSpec,
    objective: &str,
    run_id: &str,
    cancel: CancellationToken,
    dispatch: TeamDispatchFn,
) -> Result<TeamExecutionResult> {
    let compiled = compile_team_graph(spec, objective)?;
    let store = Arc::new(InMemoryRevisionedTaskStore::new());
    let service =
        TaskRevisionService::new(store.clone(), Arc::new(DefaultTaskToolPolicy::new(run_id)));
    service
        .create_prepared(
            run_id,
            TaskGraphContext {
                goal: objective.to_string(),
                assumptions: Vec::new(),
                risks: Vec::new(),
                execution_mode: TaskGraphExecutionMode::Parallel,
                metadata: serde_json::json!({ "team_strategy": spec.strategy.name() }),
            },
            compiled.tasks,
            "compile Team intent".to_string(),
        )
        .await
        .map_err(|error| ReactError::Other(error.to_string()))?;

    let controller = Arc::new(TeamRuntimeController {
        store,
        dispatch,
        outputs: Mutex::new(HashMap::new()),
    });
    let executor = RuntimeDagExecutor::new(
        controller.clone(),
        RuntimeDagExecutorConfig {
            max_concurrent_subagents: spec.config.max_concurrent.max(1),
            ..RuntimeDagExecutorConfig::default()
        },
    );
    match executor.execute(run_id, cancel).await? {
        RuntimeDagOutcome::Completed => {}
        RuntimeDagOutcome::Failed { error, .. } | RuntimeDagOutcome::Paused { error, .. } => {
            return Err(ReactError::Other(format!("Team graph failed: {error}")));
        }
        RuntimeDagOutcome::Cancelled => {
            return Err(ReactError::Other("Team graph cancelled".to_string()));
        }
    }

    let outputs = controller.outputs.lock().await;
    let terminal = outputs.get(&compiled.terminal_task_id).ok_or_else(|| {
        ReactError::Other(format!(
            "Team graph completed without terminal output '{}'",
            compiled.terminal_task_id
        ))
    })?;
    Ok(TeamExecutionResult {
        output: terminal.output.clone(),
        usage: aggregate_usage(outputs.values()),
    })
}

fn compile_team_graph(spec: &TeamSpec, objective: &str) -> Result<CompiledTeamGraph> {
    validate_team_spec(spec)?;
    let mut tasks = Vec::new();
    let terminal_task_id = match &spec.strategy {
        TeamStrategy::ManagerSubagent => {
            let plan_id = "team-plan".to_string();
            tasks.push(team_task(
                &plan_id,
                &spec.manager,
                format!(
                    "Analyze this objective and give concrete guidance to every Team member:\n{objective}"
                ),
                Vec::new(),
            ));
            let member_ids = spec
                .subagents
                .iter()
                .enumerate()
                .map(|(index, member)| {
                    let id = format!("team-member-{index}");
                    tasks.push(team_task(
                        &id,
                        member,
                        format!("Execute your part of this Team objective:\n{objective}"),
                        vec![plan_id.clone()],
                    ));
                    id
                })
                .collect::<Vec<_>>();
            let dependencies = if member_ids.is_empty() {
                vec![plan_id]
            } else {
                member_ids
            };
            let id = "team-synthesis".to_string();
            tasks.push(team_task(
                &id,
                &spec.manager,
                format!("Synthesize the Team's completed work for:\n{objective}"),
                dependencies,
            ));
            id
        }
        TeamStrategy::Pipeline(members) => {
            let mut previous = None;
            let mut terminal = String::new();
            for (index, member) in members.iter().enumerate() {
                let id = format!("team-pipeline-{index}");
                let dependencies = previous.iter().cloned().collect();
                tasks.push(team_task(
                    &id,
                    member,
                    format!("Advance this pipeline objective:\n{objective}"),
                    dependencies,
                ));
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
    dispatch: TeamDispatchFn,
    outputs: Mutex<HashMap<String, SubagentResult>>,
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
        _context: TaskSubagentContext,
        _claim: TaskClaim,
        task: Task,
    ) -> Result<Self::DispatchOutput> {
        let mut prompt = task.spec.description.clone();
        let outputs = self.outputs.lock().await;
        for dependency in &task.spec.depends_on {
            if let Some(output) = outputs.get(dependency) {
                prompt.push_str("\n\nCompleted dependency ");
                prompt.push_str(dependency);
                prompt.push_str(":\n");
                prompt.push_str(&output.output);
            }
        }
        drop(outputs);
        (self.dispatch)(task.spec.agent_role, prompt)
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
        let applied = self
            .store
            .settle_runtime_claim(run_id, &task.spec.id, &claim, status)
            .await
            .map_err(|error| ReactError::Other(error.to_string()))?;
        if !applied {
            return Ok(RuntimeTaskResolution::Superseded);
        }
        if let Some(output) = output {
            self.outputs.lock().await.insert(task.spec.id, output);
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

fn aggregate_usage<'a>(outputs: impl Iterator<Item = &'a SubagentResult>) -> Option<LlmUsageStats> {
    let mut total = LlmUsageStats::default();
    let mut has_usage = false;
    for result in outputs {
        let Some(usage) = &result.usage else {
            continue;
        };
        has_usage = true;
        if total.model.is_empty() {
            total.model = usage.model.clone();
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
    has_usage.then_some(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn successful_result(agent_name: String, task: &str) -> SubagentResult {
        SubagentResult::sync_result(&agent_name, format!("done: {task}"), Duration::ZERO)
    }

    #[tokio::test]
    async fn manager_team_uses_canonical_graph_and_dependency_outputs() -> Result<()> {
        let calls = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
        let observed = calls.clone();
        let dispatch: TeamDispatchFn = Arc::new(move |name, task| {
            let observed = observed.clone();
            Box::pin(async move {
                observed.lock().await.push((name.clone(), task.clone()));
                Ok(successful_result(name, &task))
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
        let calls = calls.lock().await;
        assert_eq!(calls.len(), 4);
        assert_eq!(calls.first().map(|call| call.0.as_str()), Some("manager"));
        assert_eq!(calls.last().map(|call| call.0.as_str()), Some("manager"));
        Ok(())
    }

    #[tokio::test]
    async fn pipeline_passes_previous_output_to_next_prompt() -> Result<()> {
        let calls = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
        let observed = calls.clone();
        let dispatch: TeamDispatchFn = Arc::new(move |name, task| {
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
        assert!(second_prompt.contains("Completed dependency team-pipeline-0"));
        assert!(second_prompt.contains("done: Advance this pipeline objective"));
        Ok(())
    }

    #[tokio::test]
    async fn failed_member_blocks_synthesis() {
        let calls = Arc::new(Mutex::new(Vec::<String>::new()));
        let observed = calls.clone();
        let dispatch: TeamDispatchFn = Arc::new(move |name, task| {
            let observed = observed.clone();
            Box::pin(async move {
                observed.lock().await.push(name.clone());
                if name == "broken" {
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
        let dispatch: TeamDispatchFn = Arc::new(move |name, task| {
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
        let dispatch: TeamDispatchFn = Arc::new(|name, _task| {
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
}
