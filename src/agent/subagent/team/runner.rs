//! TeamRunner — parallel fan-out execution for team members.
//!
//! Runs multiple team members concurrently with a semaphore for concurrency control.

use super::{Team, TeamMember, TeamRole};
use std::sync::Arc;

/// Result from a single team member execution.
pub struct MemberResult {
    pub name: String,
    pub role: TeamRole,
    pub output: String,
    pub success: bool,
    pub duration_ms: u64,
}

/// Parallel fan-out runner for team execution.
pub struct TeamRunner {
    pub max_concurrent: usize,
    pub timeout_secs: u64,
}

impl Default for TeamRunner {
    fn default() -> Self {
        Self {
            max_concurrent: 5,
            timeout_secs: 120,
        }
    }
}

impl TeamRunner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fan out a task to all team workers in parallel with concurrency control.
    pub async fn fan_out(&self, team: &Team, task: &str) -> Vec<MemberResult> {
        let workers: Vec<&TeamMember> = team.workers().collect();
        if workers.is_empty() {
            return vec![];
        }

        let sem = Arc::new(tokio::sync::Semaphore::new(self.max_concurrent));
        let mut handles = Vec::new();

        for worker in workers {
            let agent = Arc::clone(&worker.agent);
            let name = worker.name.clone();
            let role = worker.role.clone();
            let task = task.to_string();
            let sem = sem.clone();
            let timeout = self.timeout_secs;

            handles.push(tokio::spawn(async move {
                let _permit = sem.acquire().await.expect("semaphore closed");
                let start = std::time::Instant::now();
                let result = tokio::time::timeout(
                    std::time::Duration::from_secs(timeout),
                    agent.execute(&task),
                )
                .await;
                let duration_ms = start.elapsed().as_millis() as u64;

                match result {
                    Ok(Ok(output)) => MemberResult {
                        name,
                        role,
                        output,
                        success: true,
                        duration_ms,
                    },
                    Ok(Err(e)) => MemberResult {
                        name,
                        role,
                        output: format!("Error: {e}"),
                        success: false,
                        duration_ms,
                    },
                    Err(_) => MemberResult {
                        name,
                        role,
                        output: format!("Timeout after {timeout}s"),
                        success: false,
                        duration_ms,
                    },
                }
            }));
        }

        let mut results = Vec::new();
        for h in handles {
            if let Ok(r) = h.await {
                results.push(r);
            }
        }
        results.sort_by_key(|r| r.duration_ms);
        results
    }

    /// Fan out to specific members by name.
    pub async fn fan_out_to(&self, team: &Team, names: &[&str], task: &str) -> Vec<MemberResult> {
        let members: Vec<&TeamMember> = names.iter().filter_map(|n| team.get_member(n)).collect();
        if members.is_empty() {
            return vec![];
        }

        let sem = Arc::new(tokio::sync::Semaphore::new(self.max_concurrent));
        let mut handles = Vec::new();

        for member in members {
            let agent = Arc::clone(&member.agent);
            let name = member.name.clone();
            let role = member.role.clone();
            let task = task.to_string();
            let sem = sem.clone();
            let timeout = self.timeout_secs;

            handles.push(tokio::spawn(async move {
                let _permit = sem.acquire().await.expect("semaphore closed");
                let start = std::time::Instant::now();
                let result = tokio::time::timeout(
                    std::time::Duration::from_secs(timeout),
                    agent.execute(&task),
                )
                .await;
                let (output, success) = match result {
                    Ok(Ok(o)) => (o, true),
                    Ok(Err(e)) => (format!("Error: {e}"), false),
                    Err(_) => ("Timeout".into(), false),
                };
                MemberResult {
                    name,
                    role,
                    output,
                    success,
                    duration_ms: start.elapsed().as_millis() as u64,
                }
            }));
        }

        let mut results = Vec::new();
        for h in handles {
            if let Ok(r) = h.await {
                results.push(r);
            }
        }
        results
    }
}
