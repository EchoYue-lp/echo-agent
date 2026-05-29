//! Manager-Worker orchestration strategy.
//!
//! The manager decomposes a task into sub-tasks, fans them out to workers,
//! collects results, and synthesizes the final answer.

use super::{Team, TeamMember};
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Orchestrates a team using the Manager-Worker pattern.
pub struct ManagerWorkerOrchestrator {
    /// Maximum number of retries for failed sub-tasks.
    pub max_retries: u32,
    /// Timeout per worker sub-task in seconds.
    pub worker_timeout_secs: u64,
}

impl Default for ManagerWorkerOrchestrator {
    fn default() -> Self {
        Self {
            max_retries: 2,
            worker_timeout_secs: 300,
        }
    }
}

impl ManagerWorkerOrchestrator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Run a task through the Manager-Worker team.
    ///
    /// Phase 1: Manager decomposes the task into sub-tasks.
    /// Phase 2: Workers execute sub-tasks in parallel (round-robin assignment).
    /// Phase 3: Manager synthesizes results into a final answer.
    pub async fn run(&self, team: &Team, task: &str) -> Result<String, String> {
        let manager_name = team.leader_name().ok_or("No leader in team")?;
        let workers: Vec<&TeamMember> = team.workers().collect();

        if workers.is_empty() {
            return Err("No workers in team".into());
        }

        info!(
            team = %team.name,
            manager = %manager_name,
            worker_count = workers.len(),
            "Starting Manager-Worker execution"
        );

        // Phase 1: Manager plans
        let sub_tasks = self.plan_sub_tasks(team, manager_name, task).await?;
        debug!(
            sub_task_count = sub_tasks.len(),
            "Manager created sub-tasks"
        );

        // Phase 2: Fan out to workers
        let results = self.execute_sub_tasks(&sub_tasks, workers).await;

        // Phase 3: Manager synthesizes
        self.synthesize(team, manager_name, task, &results).await
    }

    /// Phase 1: The manager decomposes the task into sub-tasks.
    async fn plan_sub_tasks(
        &self,
        team: &Team,
        manager_name: &str,
        task: &str,
    ) -> Result<Vec<String>, String> {
        let manager = team.get_member(manager_name).ok_or("Manager not found")?;

        let planning_prompt = format!(
            "You are a team manager. Your team has these workers:\n\
             {}\n\n\
             Decompose this task into 2-5 sub-tasks, one per line:\n\
             {}\n\n\
             Output only the sub-tasks, one per line. No numbering, no extra text.",
            team.worker_descriptions(),
            task
        );

        let output = manager
            .agent
            .execute(&planning_prompt)
            .await
            .map_err(|e| format!("Manager planning failed: {e}"))?;

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

    /// Phase 2: Fan out sub-tasks to workers in round-robin fashion.
    async fn execute_sub_tasks(
        &self,
        sub_tasks: &[String],
        workers: Vec<&TeamMember>,
    ) -> Vec<(String, Result<String, String>)> {
        let worker_count = workers.len();
        let mut handles = Vec::new();

        for (i, sub_task) in sub_tasks.iter().enumerate() {
            let worker = &workers[i % worker_count];
            let worker_name = worker.name.clone();
            let agent = Arc::clone(&worker.agent);
            let task = sub_task.clone();

            handles.push(tokio::spawn(async move {
                let result = agent
                    .execute(&task)
                    .await
                    .map_err(|e| format!("Worker {worker_name} failed: {e}"));
                (worker_name, task, result)
            }));
        }

        let mut results = Vec::new();
        for handle in handles {
            match handle.await {
                Ok((name, task, result)) => {
                    match &result {
                        Ok(_) => info!(worker = %name, "Worker completed sub-task"),
                        Err(e) => warn!(worker = %name, error = %e, "Worker failed"),
                    }
                    results.push((task, result));
                }
                Err(e) => {
                    warn!("Worker spawned task panicked: {e}");
                }
            }
        }
        results
    }

    /// Phase 3: The manager synthesizes the final answer.
    async fn synthesize(
        &self,
        team: &Team,
        manager_name: &str,
        original_task: &str,
        results: &[(String, Result<String, String>)],
    ) -> Result<String, String> {
        let manager = team.get_member(manager_name).ok_or("Manager not found")?;

        let mut results_text = String::new();
        for (i, (sub_task, result)) in results.iter().enumerate() {
            results_text.push_str(&format!("Sub-task {}: {}\n", i + 1, sub_task));
            match result {
                Ok(output) => {
                    results_text
                        .push_str(&format!("Result: {}\n\n", &output[..output.len().min(500)]));
                }
                Err(e) => {
                    results_text.push_str(&format!("Error: {e}\n\n"));
                }
            }
        }

        let synthesis_prompt = format!(
            "You are a team manager. Your workers have completed their sub-tasks.\n\n\
             Original task: {original_task}\n\n\
             Worker results:\n{results_text}\n\
             Synthesize these results into a single, coherent answer.\n\
             If any sub-tasks failed, note the failures and suggest next steps."
        );

        manager
            .agent
            .execute(&synthesis_prompt)
            .await
            .map_err(|e| format!("Manager synthesis failed: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_orchestrator_defaults() {
        let orch = ManagerWorkerOrchestrator::new();
        assert_eq!(orch.max_retries, 2);
        assert_eq!(orch.worker_timeout_secs, 300);
    }

    #[test]
    fn test_team_strategy_default() {
        let strategy = crate::agent::subagent::team::strategy::TeamStrategy::default();
        assert_eq!(
            strategy,
            crate::agent::subagent::team::strategy::TeamStrategy::ManagerWorker
        );
        assert_eq!(strategy.name(), "manager-worker");
    }
}
