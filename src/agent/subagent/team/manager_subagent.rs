//! Manager-Subagent orchestration strategy.
//!
//! The manager decomposes a task into sub-tasks, fans them out to subagents,
//! collects results, and synthesizes the final answer.
//!
//! Sprint 11: single-pass plan → fan-out → synthesize, with **checkpoint/resume**
//! when a `run_id` + `RuntimeStateStore` are supplied. Three checkpoint nodes
//! (`team_{run_id}_plan`, `team_{run_id}_subagent_{idx}`, `team_{run_id}_synthesis`)
//! mirror the DAG skip-completed-on-retry pattern (`task_runtime/executor.rs:456`).
//! `store = None` → pure in-memory single-pass (today's behavior, backward-compat).

use super::{Team, TeamDispatchFn, TeamMember};
use crate::state::{TaskNode, TaskNodeStatus};
use futures::stream::{self, StreamExt};
use std::collections::HashMap;
use tracing::{debug, info, warn};

// ── Checkpoint node id helpers ───────────────────────────────────────────────

fn plan_node_id(run_id: &str) -> String {
    format!("team_{run_id}_plan")
}
fn synth_node_id(run_id: &str) -> String {
    format!("team_{run_id}_synthesis")
}
fn subagent_node_id(run_id: &str, idx: usize) -> String {
    format!("team_{run_id}_subagent_{idx}")
}

/// Orchestrates a team using the Manager-Subagent pattern.
///
/// Stateless except for the checkpoint store passed into `run()`. Sprint 11
/// removed the dead `max_retries`/`subagent_timeout_secs` fields (declared but
/// never read; timeouts come from the outer `TeamAgent::execute` wrapper and
/// the `SubagentExecutor` dispatch timeout).
pub struct ManagerSubagentOrchestrator;

struct SubTaskExecutionContext<'a> {
    max_concurrent: usize,
    run_id: Option<&'a str>,
    store: Option<&'a dyn crate::state::RuntimeStateStore>,
    prior_nodes: &'a HashMap<String, TaskNode>,
    member_dispatch: Option<&'a TeamDispatchFn>,
}

impl Default for ManagerSubagentOrchestrator {
    fn default() -> Self {
        Self
    }
}

impl ManagerSubagentOrchestrator {
    pub fn new() -> Self {
        Self
    }

    /// Run a task through the Manager-Subagent team.
    ///
    /// Phase 1: Manager decomposes the task into sub-tasks.
    /// Phase 2: Subagents execute sub-tasks in parallel (round-robin assignment).
    /// Phase 3: Manager synthesizes results into a final answer.
    ///
    /// Sprint 11 checkpoint/resume (both `run_id` + `store` required to activate):
    /// - **Fast-path**: if a prior `synthesis` node is `Success`, return its
    ///   stored answer immediately (zero agent calls).
    /// - **plan**: if prior `plan` node is `Success`, reuse its stored ordered
    ///   sub-task array (deterministic idx binding — user review patch #1);
    ///   else run planning + checkpoint the plan.
    /// - **subagents**: per `subagent_{idx}` node, skip if prior `Success` (reuse
    ///   stored output); if prior non-Success terminal (`Failed`) or non-terminal
    ///   (`Running`/`Blocked` — e.g. crash-stale), reset to `Pending` first then
    ///   re-run (state-reset defense, user review patch #3). Checkpoint each.
    /// - **synthesis**: always runs unless fast-pathed; checkpoint on completion.
    ///   `store = None` → skip all read/write, pure in-memory.
    pub async fn run(
        &self,
        team: &Team,
        task: &str,
        run_id: Option<&str>,
        store: Option<&dyn crate::state::RuntimeStateStore>,
    ) -> Result<String, String> {
        self.run_with_dispatch(team, task, run_id, store, None)
            .await
    }

    pub(crate) async fn run_with_dispatch(
        &self,
        team: &Team,
        task: &str,
        run_id: Option<&str>,
        store: Option<&dyn crate::state::RuntimeStateStore>,
        member_dispatch: Option<&TeamDispatchFn>,
    ) -> Result<String, String> {
        let manager_name = team.leader_name().ok_or("No leader in team")?;
        let subagents: Vec<&TeamMember> = team.subagents().collect();
        if subagents.is_empty() {
            return Err("No subagents in team".into());
        }

        // ── Load prior checkpoint nodes (DAG skip-on-resume pattern) ──
        let prior_nodes: HashMap<String, TaskNode> = if let (Some(rid), Some(st)) = (run_id, store)
        {
            st.load_nodes(rid)
                .await
                .map_err(|error| format!("Failed to load Team checkpoints: {error}"))?
                .into_iter()
                .map(|n| (n.id.clone(), n))
                .collect()
        } else {
            HashMap::new()
        };

        // Fast-path: synthesis already Success → return stored answer.
        // (Edition 2024 let-chains collapse the nested if-let guards.)
        if let Some(rid) = run_id
            && let Some(node) = prior_nodes.get(&synth_node_id(rid))
            && node.status == TaskNodeStatus::Success
            && let Some(ans) = node.outputs.as_str()
        {
            debug!("Team fast-path: returning stored synthesis (zero agent calls)");
            return Ok(ans.to_string());
        }

        info!(
            team = %team.name,
            manager = %manager_name,
            subagent_count = subagents.len(),
            has_checkpoint = prior_nodes.len() > prior_nodes.is_empty() as usize,
            "Starting Manager-Subagent execution"
        );

        // ── Phase 1: plan (skip if prior Success; else run + checkpoint) ──
        // Edition 2024 let-chains collapse the nested guards.
        let sub_tasks: Vec<String> = if let Some(rid) = run_id
            && let Some(node) = prior_nodes.get(&plan_node_id(rid))
            && node.status == TaskNodeStatus::Success
            && let Some(arr) = node.outputs.as_array()
        {
            // Reuse stored plan: ordered [{idx, task}] array (patch #1).
            let reused: Vec<String> = arr
                .iter()
                .filter_map(|v| v.get("task").and_then(|t| t.as_str()).map(String::from))
                .collect();
            if reused.is_empty() {
                self.plan_sub_tasks(team, manager_name, task, member_dispatch)
                    .await?
            } else {
                debug!(count = reused.len(), "Reusing stored plan");
                reused
            }
        } else {
            self.plan_sub_tasks(team, manager_name, task, member_dispatch)
                .await?
        };

        // Checkpoint: write plan node (ordered [{idx, task}] array for idx binding).
        if let (Some(rid), Some(st)) = (run_id, store) {
            let plan_outputs = serde_json::Value::Array(
                sub_tasks
                    .iter()
                    .enumerate()
                    .map(|(idx, t)| serde_json::json!({"idx": idx, "task": t}))
                    .collect(),
            );
            let node = TaskNode::new(plan_node_id(rid), "team_plan")
                .with_status(TaskNodeStatus::Success)
                .with_outputs(plan_outputs);
            st.save_node(rid, &node)
                .await
                .map_err(|error| format!("Failed to save Team plan checkpoint: {error}"))?;
        }
        debug!(sub_task_count = sub_tasks.len(), "Plan ready");

        // ── Phase 2: fan-out subagents (skip Success; reset+rerun Running/Failed) ──
        let results = self
            .execute_sub_tasks(
                &sub_tasks,
                subagents,
                SubTaskExecutionContext {
                    max_concurrent: team.config.max_concurrent,
                    run_id,
                    store,
                    prior_nodes: &prior_nodes,
                    member_dispatch,
                },
            )
            .await?;

        let failures: Vec<String> = results
            .iter()
            .filter_map(|(sub_task, result)| {
                result
                    .as_ref()
                    .err()
                    .map(|error| format!("{sub_task}: {error}"))
            })
            .collect();
        if !failures.is_empty() {
            return Err(format!(
                "Team execution failed for {} sub-task(s): {}",
                failures.len(),
                failures.join("; ")
            ));
        }

        // ── Phase 3: synthesize (runs unless fast-pathed above) ──
        let synthesis = self
            .synthesize(team, manager_name, task, &results, member_dispatch)
            .await?;

        // Checkpoint: write synthesis node.
        if let (Some(rid), Some(st)) = (run_id, store) {
            let node = TaskNode::new(synth_node_id(rid), "team_synthesis")
                .with_status(TaskNodeStatus::Success)
                .with_outputs(serde_json::Value::String(synthesis.clone()));
            st.save_node(rid, &node)
                .await
                .map_err(|error| format!("Failed to save Team synthesis checkpoint: {error}"))?;
        }
        Ok(synthesis)
    }

    /// Phase 1: The manager decomposes the task into sub-tasks.
    async fn plan_sub_tasks(
        &self,
        team: &Team,
        manager_name: &str,
        task: &str,
        member_dispatch: Option<&TeamDispatchFn>,
    ) -> Result<Vec<String>, String> {
        let manager = team.get_member(manager_name).ok_or("Manager not found")?;

        let planning_prompt = format!(
            "You are a team manager. Your team has these subagents:\n\
             {}\n\n\
             Decompose this task into 2-5 sub-tasks, one per line:\n\
             {}\n\n\
             Rules:\n\
             - Plan only — you do not execute the sub-tasks yourself.\n\
             - Each sub-task must be independently executable by one subagent, with a clear deliverable.\n\
             - Assign disjoint scope (files/modules/questions) so parallel subagents do not overlap or conflict.\n\
             - Do not create dependencies between lines: every line starts concurrently. If work requires ordering, keep those dependent steps together in one sub-task.\n\n\
             Output only the sub-tasks, one per line. No numbering, no extra text.",
            team.subagent_descriptions(),
            task
        );

        let output = execute_member(manager, &planning_prompt, member_dispatch)
            .await
            .map_err(|error| format!("Manager planning failed: {error}"))?;

        let sub_tasks: Vec<String> = output
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .map(|l| l.to_string())
            .collect();

        if sub_tasks.is_empty() {
            return Ok(vec![task.to_string()]);
        }
        Ok(sub_tasks)
    }

    /// Phase 2: Fan out sub-tasks to subagents in round-robin fashion.
    ///
    /// Sprint 11 checkpoint-aware: skip subagents whose prior `subagent_{idx}` node
    /// is `Success` (reuse stored output); reset+rerun those that are
    /// `Running`/`Failed`/`Blocked` (state-reset defense). Checkpoint each on
    /// completion.
    async fn execute_sub_tasks(
        &self,
        sub_tasks: &[String],
        subagents: Vec<&TeamMember>,
        context: SubTaskExecutionContext<'_>,
    ) -> Result<Vec<(String, Result<String, String>)>, String> {
        let subagent_count = subagents.len();
        if subagent_count == 0 {
            return Err("Manager-subagent team has no executable subagents".to_string());
        }
        type SubagentOutcome = (usize, String, Result<String, String>);
        let mut reused = Vec::new();
        let mut pending = Vec::new();

        for (i, sub_task) in sub_tasks.iter().enumerate() {
            let Some(subagent) = subagents.get(i % subagent_count).copied() else {
                continue;
            };
            let subagent_name = subagent.name.clone();
            let task = sub_task.clone();
            let idx = i;

            // Skip-on-resume: if this subagent_idx already Success, reuse its output.
            if let Some(rid) = context.run_id {
                let wid = subagent_node_id(rid, i);
                if let Some(node) = context.prior_nodes.get(&wid) {
                    if node.status == TaskNodeStatus::Success {
                        if let Some(out) = node.outputs.as_str() {
                            info!(subagent = %subagent_name, idx = i, "Reusing stored subagent result");
                            let stored: Result<String, String> = Ok(out.to_string());
                            reused.push((idx, task, stored));
                            continue;
                        }
                    } else {
                        // State-reset defense (patch #3): Running/Failed/Blocked
                        // → reset to Pending before re-running, overwriting stale
                        // state. Only when a store is configured.
                        if let Some(st) = context.store {
                            let reset = TaskNode::new(wid.clone(), format!("team_subagent_{i}"))
                                .with_status(TaskNodeStatus::Pending);
                            st.save_node(rid, &reset).await.map_err(|error| {
                                format!("Failed to reset Team subagent checkpoint: {error}")
                            })?;
                        }
                    }
                }
            }

            pending.push(async move {
                let result = execute_member(subagent, &task, context.member_dispatch)
                    .await
                    .map_err(|error| format!("Subagent {subagent_name} failed: {error}"));
                (idx, task, result)
            });
        }

        let mut results: Vec<(String, Result<String, String>)> =
            vec![(String::new(), Err("uninitialized".to_string())); sub_tasks.len()];
        let max_concurrent = context.max_concurrent.max(1);
        let outcomes = stream::iter(pending)
            .buffer_unordered(max_concurrent)
            .collect::<Vec<SubagentOutcome>>()
            .await;
        for (idx, task, result) in reused.into_iter().chain(outcomes) {
            let subagent_name = subagents
                .get(idx % subagent_count)
                .map(|subagent| subagent.name.clone())
                .unwrap_or_else(|| "unknown-subagent".to_string());
            match &result {
                Ok(_) => {
                    info!(subagent = %subagent_name, idx, "Subagent completed sub-task")
                }
                Err(e) => {
                    warn!(subagent = %subagent_name, idx, error = %e, "Subagent failed")
                }
            }
            // Checkpoint per-subagent (Success or Failed).
            if let (Some(rid), Some(st)) = (context.run_id, context.store) {
                let status = match &result {
                    Ok(_) => TaskNodeStatus::Success,
                    Err(_) => TaskNodeStatus::Failed,
                };
                let outputs = match &result {
                    Ok(o) => serde_json::Value::String(o.clone()),
                    Err(_) => serde_json::Value::Null,
                };
                let node =
                    TaskNode::new(subagent_node_id(rid, idx), format!("team_subagent_{idx}"))
                        .with_status(status)
                        .with_outputs(outputs);
                st.save_node(rid, &node)
                    .await
                    .map_err(|error| format!("Failed to save Team subagent checkpoint: {error}"))?;
            }
            if let Some(slot) = results.get_mut(idx) {
                *slot = (task, result);
            } else {
                warn!(idx, "Subagent result index exceeded planned sub-task count");
            }
        }
        if let Some(missing_idx) = results.iter().position(|(task, _)| task.is_empty()) {
            return Err(format!(
                "Team subagent result missing for planned sub-task index {missing_idx}"
            ));
        }
        Ok(results)
    }

    /// Phase 3: The manager synthesizes the final answer.
    async fn synthesize(
        &self,
        team: &Team,
        manager_name: &str,
        original_task: &str,
        results: &[(String, Result<String, String>)],
        member_dispatch: Option<&TeamDispatchFn>,
    ) -> Result<String, String> {
        let manager = team.get_member(manager_name).ok_or("Manager not found")?;

        let mut results_text = String::new();
        for (i, (sub_task, result)) in results.iter().enumerate() {
            results_text.push_str(&format!("Sub-task {}: {}\n", i + 1, sub_task));
            match result {
                Ok(output) => {
                    let truncated: String = output.chars().take(500).collect();
                    results_text.push_str(&format!("Result: {truncated}\n\n"));
                }
                Err(e) => {
                    results_text.push_str(&format!("Error: {e}\n\n"));
                }
            }
        }

        let synthesis_prompt = format!(
            "You are a team manager. Your subagents have completed their sub-tasks.\n\n\
             Original task: {original_task}\n\n\
             Subagent results:\n{results_text}\n\
             Synthesize these results into a single, coherent answer.\n\
             - Reconcile conflicting results explicitly by weighing evidence quality; do not silently drop either side.\n\
             - Report failed or blocked sub-tasks truthfully with suggested next steps; never claim they succeeded.\n\
             - Base the answer only on the subagent results provided — do not invent findings."
        );

        execute_member(manager, &synthesis_prompt, member_dispatch)
            .await
            .map_err(|error| format!("Manager synthesis failed: {error}"))
    }
}

async fn execute_member(
    member: &TeamMember,
    task: &str,
    dispatch: Option<&TeamDispatchFn>,
) -> Result<String, String> {
    if let Some(dispatch) = dispatch {
        let result = dispatch(member.name.clone(), task.to_string()).await?;
        if result.outcome.status != crate::agent::subagent::SubagentStatus::Completed {
            return Err(format!(
                "subagent '{}' ended with status {:?}: {}",
                member.name, result.outcome.status, result.output
            ));
        }
        Ok(result.output)
    } else {
        member
            .agent
            .execute(task)
            .await
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::RuntimeStateStore;
    use crate::testing::MockAgent;
    use futures::future::BoxFuture;
    use std::collections::HashMap as StdHashMap;
    use std::sync::{Arc, Mutex};

    #[test]
    fn test_team_strategy_default() {
        let strategy = crate::agent::subagent::team::strategy::TeamStrategy::default();
        assert_eq!(
            strategy,
            crate::agent::subagent::team::strategy::TeamStrategy::ManagerSubagent
        );
        assert_eq!(strategy.name(), "manager-subagent");
    }

    // ── Stub in-memory RuntimeStateStore for checkpoint tests ──────────────────
    /// An in-memory RuntimeStateStore: conversation_id → Vec<TaskNode>. Single
    /// Mutex; fine for tests. Lets us pre-seed nodes and assert on writes.
    struct InMemStore {
        nodes: Mutex<StdHashMap<String, Vec<TaskNode>>>,
    }
    impl InMemStore {
        fn new() -> Self {
            Self {
                nodes: Mutex::new(StdHashMap::new()),
            }
        }
        /// Pre-seed a node (test setup).
        fn seed(&self, conv_id: &str, node: TaskNode) {
            self.nodes
                .lock()
                .unwrap()
                .entry(conv_id.to_string())
                .or_default()
                .push(node);
        }
        /// Snapshot of nodes for a conversation (test assertion).
        fn snapshot(&self, conv_id: &str) -> Vec<TaskNode> {
            self.nodes
                .lock()
                .unwrap()
                .get(conv_id)
                .cloned()
                .unwrap_or_default()
        }
    }
    impl RuntimeStateStore for InMemStore {
        fn save_node<'a>(
            &'a self,
            conv_id: &'a str,
            node: &'a TaskNode,
        ) -> BoxFuture<'a, crate::error::Result<()>> {
            Box::pin(async move {
                let mut guard = self.nodes.lock().unwrap();
                let vec = guard.entry(conv_id.to_string()).or_default();
                // Upsert by id (replace if present).
                if let Some(existing) = vec.iter_mut().find(|n| n.id == node.id) {
                    *existing = node.clone();
                } else {
                    vec.push(node.clone());
                }
                Ok(())
            })
        }
        fn load_nodes<'a>(
            &'a self,
            conv_id: &'a str,
        ) -> BoxFuture<'a, crate::error::Result<Vec<TaskNode>>> {
            Box::pin(async move {
                Ok(self
                    .nodes
                    .lock()
                    .unwrap()
                    .get(conv_id)
                    .cloned()
                    .unwrap_or_default())
            })
        }
        fn update_status<'a>(
            &'a self,
            _conv_id: &'a str,
            _node_id: &'a str,
            _status: TaskNodeStatus,
        ) -> BoxFuture<'a, crate::error::Result<()>> {
            Box::pin(async { Ok(()) })
        }
        fn get_checkpoint<'a>(
            &'a self,
            _conv_id: &'a str,
        ) -> BoxFuture<'a, crate::error::Result<Option<crate::state::AgentCheckpoint>>> {
            Box::pin(async { Ok(None) })
        }
        fn save_checkpoint<'a>(
            &'a self,
            _checkpoint: &'a crate::state::AgentCheckpoint,
        ) -> BoxFuture<'a, crate::error::Result<()>> {
            Box::pin(async { Ok(()) })
        }
        fn clear_conversation<'a>(
            &'a self,
            _conv_id: &'a str,
        ) -> BoxFuture<'a, crate::error::Result<()>> {
            Box::pin(async { Ok(()) })
        }
    }

    /// Build a minimal 1-manager + 1-subagent team using MockAgents.
    fn build_team(
        plan_response: &str,
        subagent_responses: &[&str],
    ) -> (
        crate::agent::subagent::team::Team,
        Arc<MockAgent>,      // manager (for inspection)
        Vec<Arc<MockAgent>>, // subagents (for inspection)
    ) {
        use crate::agent::subagent::SubagentDefinition;
        use crate::agent::subagent::team::{Team, TeamRole};
        let mut manager = MockAgent::new("manager").with_response(plan_response);
        // Synthesis is the second execute() call on the manager — give it a canned response.
        manager = manager.with_response("SYNTHESIS");
        let manager = Arc::new(manager);
        let subagents: Vec<Arc<MockAgent>> = subagent_responses
            .iter()
            .map(|r| Arc::new(MockAgent::new("subagent").with_response(*r)))
            .collect();
        // Box clones of the Arc-wrapped mock agents into the team.
        // MockAgent is Clone (shares call history via internal Arc<Mutex>).
        let mut team = Team::new("team_test".to_string(), "test", Default::default());
        team.add_member(
            "manager",
            TeamRole::Leader,
            Box::new((*manager).clone()),
            SubagentDefinition::simple_sync("manager"),
        );
        for (i, w) in subagents.iter().enumerate() {
            team.add_member(
                &format!("subagent_{i}"),
                TeamRole::Subagent,
                Box::new((**w).clone()),
                SubagentDefinition::simple_sync(format!("subagent_{i}")),
            );
        }
        (team, manager, subagents)
    }

    #[tokio::test]
    async fn run_with_store_writes_three_checkpoints() {
        // Plan + 1 subagent + synthesis all checkpoint Success.
        let (team, _mgr, _ws) = build_team("subtask A\nsubtask B", &["w-out-0", "w-out-1"]);
        let store = Arc::new(InMemStore::new());
        let orch = ManagerSubagentOrchestrator::new();
        let result = orch
            .run(&team, "do thing", Some("run-1"), Some(store.as_ref()))
            .await
            .unwrap();
        assert_eq!(result, "SYNTHESIS");
        let snap = store.snapshot("run-1");
        let ids: Vec<&str> = snap.iter().map(|n| n.id.as_str()).collect();
        assert!(
            ids.contains(&"team_run-1_plan"),
            "plan node missing: {:?}",
            ids
        );
        assert!(
            ids.contains(&"team_run-1_subagent_0"),
            "subagent_0 node missing: {:?}",
            ids
        );
        assert!(
            ids.contains(&"team_run-1_subagent_1"),
            "subagent_1 node missing: {:?}",
            ids
        );
        assert!(
            ids.contains(&"team_run-1_synthesis"),
            "synthesis node missing: {:?}",
            ids
        );
        // All should be Success.
        for n in &snap {
            assert_eq!(
                n.status,
                TaskNodeStatus::Success,
                "node {} not Success",
                n.id
            );
        }
    }

    #[tokio::test]
    async fn member_failure_is_not_projected_as_successful_synthesis()
    -> std::result::Result<(), String> {
        use crate::agent::subagent::SubagentDefinition;
        use crate::agent::subagent::team::{Team, TeamRole};
        use crate::testing::FailingMockAgent;

        let manager = MockAgent::new("manager")
            .with_response("subtask A")
            .with_response("SHOULD NOT SYNTHESIZE");
        let mut team = Team::new("team-failure", "failure test", Default::default());
        team.add_member(
            "manager",
            TeamRole::Leader,
            Box::new(manager.clone()),
            SubagentDefinition::simple_sync("manager"),
        );
        team.add_member(
            "failing",
            TeamRole::Subagent,
            Box::new(FailingMockAgent::new("failing", "member failed")),
            SubagentDefinition::simple_sync("failing"),
        );

        let error = ManagerSubagentOrchestrator::new()
            .run(&team, "do thing", None, None)
            .await
            .err()
            .ok_or_else(|| "team unexpectedly reported success".to_string())?;
        assert!(error.contains("member failed"), "unexpected error: {error}");
        assert_eq!(
            manager.call_count(),
            1,
            "manager synthesis must not run after member failure"
        );
        Ok::<(), String>(())
    }

    #[tokio::test]
    async fn run_fast_path_returns_stored_synthesis() {
        // Pre-seed synthesis Success → zero agent calls, return stored answer.
        let (team, _mgr, _ws) = build_team("should-not-be-used", &["x"]);
        let store = Arc::new(InMemStore::new());
        store.seed(
            "run-fast",
            TaskNode::new("team_run-fast_synthesis", "team_synthesis")
                .with_status(TaskNodeStatus::Success)
                .with_outputs(serde_json::Value::String("PRECOMPUTED".to_string())),
        );
        let orch = ManagerSubagentOrchestrator::new();
        let result = orch
            .run(&team, "do thing", Some("run-fast"), Some(store.as_ref()))
            .await
            .unwrap();
        assert_eq!(result, "PRECOMPUTED");
    }

    #[tokio::test]
    async fn run_resumes_skipping_completed_plan_and_subagent() {
        // Pre-seed plan Success + subagent_0 Success → plan reused, subagent_0 reused,
        // only subagent_1 spawned fresh, synthesis merges stored + new.
        let (_team, _mgr, subagents) = build_team("fresh-plan", &["w-fresh-1"]);
        // subagent_0 won't actually be called (its stored output reused); give the
        // team's subagent_0 a different response that should NOT appear in results.
        let _ = subagents; // (the team was built with one subagent slot; this test uses 2)
        let store = Arc::new(InMemStore::new());
        store.seed(
            "run-resume",
            TaskNode::new("team_run-resume_plan", "team_plan")
                .with_status(TaskNodeStatus::Success)
                .with_outputs(serde_json::json!([
                    {"idx": 0, "task": "task-0"},
                    {"idx": 1, "task": "task-1"}
                ])),
        );
        store.seed(
            "run-resume",
            TaskNode::new("team_run-resume_subagent_0", "team_subagent_0")
                .with_status(TaskNodeStatus::Success)
                .with_outputs(serde_json::Value::String("STORED-W0".to_string())),
        );
        // Rebuild team with 2 subagents so subagent_1 exists.
        let (team2, _m, _ws) = build_team("fresh-plan", &["fresh-w0", "fresh-w1"]);
        let orch = ManagerSubagentOrchestrator::new();
        let _result = orch
            .run(&team2, "do thing", Some("run-resume"), Some(store.as_ref()))
            .await
            .unwrap();
        // Subagent_0's stored result should be reused; subagent_1 ran fresh. The
        // synthesis node should now be written.
        let snap = store.snapshot("run-resume");
        let w0 = snap
            .iter()
            .find(|n| n.id == "team_run-resume_subagent_0")
            .unwrap();
        assert_eq!(w0.status, TaskNodeStatus::Success);
        assert!(snap.iter().any(|n| n.id == "team_run-resume_synthesis"));
    }

    #[tokio::test]
    async fn run_resets_stale_running_or_failed_subagents() {
        // Pre-seed subagent_0 Running (crash-stale) + subagent_1 Failed → both must
        // be reset to Pending then re-run (state-reset defense, patch #3).
        let (team, _mgr, _ws) = build_team("task-A\ntask-B", &["rerun-0", "rerun-1"]);
        let store = Arc::new(InMemStore::new());
        store.seed(
            "run-reset",
            TaskNode::new("team_run-reset_subagent_0", "team_subagent_0")
                .with_status(TaskNodeStatus::Running), // stale
        );
        store.seed(
            "run-reset",
            TaskNode::new("team_run-reset_subagent_1", "team_subagent_1")
                .with_status(TaskNodeStatus::Failed),
        );
        let orch = ManagerSubagentOrchestrator::new();
        let _result = orch
            .run(&team, "do thing", Some("run-reset"), Some(store.as_ref()))
            .await
            .unwrap();
        let snap = store.snapshot("run-reset");
        // Both subagents should now be Success (reran, not stuck).
        let w0 = snap
            .iter()
            .find(|n| n.id == "team_run-reset_subagent_0")
            .unwrap();
        let w1 = snap
            .iter()
            .find(|n| n.id == "team_run-reset_subagent_1")
            .unwrap();
        assert_eq!(
            w0.status,
            TaskNodeStatus::Success,
            "stale Running subagent must rerun"
        );
        assert_eq!(
            w1.status,
            TaskNodeStatus::Success,
            "Failed subagent must rerun"
        );
    }

    #[tokio::test]
    async fn run_synthesis_missing_reruns_only_synthesis() {
        // Pre-seed plan + all subagents Success but NO synthesis → plan + subagents
        // skipped, only synthesis runs (patch #4 edge case). The plan being
        // reused means the manager's FIRST execute() call is the synthesis,
        // so it returns build_team's first response ("task-A").
        let (team, _mgr, _ws) = build_team("task-A", &["stored-w0"]);
        let store = Arc::new(InMemStore::new());
        store.seed(
            "run-synmiss",
            TaskNode::new("team_run-synmiss_plan", "team_plan")
                .with_status(TaskNodeStatus::Success)
                .with_outputs(serde_json::json!([{"idx": 0, "task": "task-A"}])),
        );
        store.seed(
            "run-synmiss",
            TaskNode::new("team_run-synmiss_subagent_0", "team_subagent_0")
                .with_status(TaskNodeStatus::Success)
                .with_outputs(serde_json::Value::String("W0-RESULT".to_string())),
        );
        let orch = ManagerSubagentOrchestrator::new();
        let result = orch
            .run(&team, "do thing", Some("run-synmiss"), Some(store.as_ref()))
            .await
            .unwrap();
        // Synthesis ran (manager's only execute call, since plan was reused) →
        // returns the first queued response "task-A". Not "SYNTHESIS" because
        // MockAgent consumes responses in order and the plan-skip means
        // synthesis is now the first call.
        assert_eq!(result, "task-A");
        // Core assertion: synthesis node written Success (plan + subagents reused).
        let snap = store.snapshot("run-synmiss");
        assert!(
            snap.iter().any(
                |n| n.id == "team_run-synmiss_synthesis" && n.status == TaskNodeStatus::Success
            )
        );
        // plan + subagent_0 nodes unchanged (still Success, not re-written).
        let plan = snap
            .iter()
            .find(|n| n.id == "team_run-synmiss_plan")
            .unwrap();
        assert_eq!(plan.status, TaskNodeStatus::Success);
    }
}
