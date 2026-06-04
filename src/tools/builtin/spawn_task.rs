//! Spawn background task tool — allows agents to offload work without blocking.
//!
//! The agent spawns a named background task and immediately receives a task ID.
//! The task runs asynchronously; the agent can check its status later using
//! the `check_task_status` tool.
//!
//! # Execution Modes
//!
//! The tool supports two modes:
//! 1. **Command mode** (`command` parameter): Runs a shell command asynchronously
//!    and captures stdout/stderr as the result.
//! 2. **Description mode** (no `command`): Registers a named placeholder task
//!    for tracking purposes (the description is returned as the result).

use futures::future::BoxFuture;

use crate::error::ToolError;
use crate::tasks::{TaskSpawner, TaskSpawnerConfig};
use crate::tools::{Tool, ToolParameters, ToolResult};
use serde_json::{Value, json};
use std::sync::Arc;
use tracing::info;

/// Maximum length of captured stdout/stderr output (bytes).
const MAX_OUTPUT_LEN: usize = 64 * 1024; // 64 KiB

/// Tool that spawns a named background task and returns a task ID.
pub struct SpawnBackgroundTaskTool {
    spawner: Arc<TaskSpawner>,
}

impl SpawnBackgroundTaskTool {
    /// Create a new tool with the given task spawner.
    pub fn new(spawner: Arc<TaskSpawner>) -> Self {
        Self { spawner }
    }

    /// Create a new tool with a default spawner.
    #[allow(dead_code)]
    pub fn with_default_spawner() -> Self {
        Self {
            spawner: Arc::new(TaskSpawner::new(TaskSpawnerConfig::default())),
        }
    }
}

impl Tool for SpawnBackgroundTaskTool {
    fn name(&self) -> &str {
        "spawn_background_task"
    }

    fn description(&self) -> &str {
        "Spawn a background task that runs asynchronously. Returns a task ID you can use to check status later with check_task_status. \
         Supports two modes: (1) pass `command` to run a shell command in the background, or (2) omit `command` to register a named tracking task."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Human-readable name for the task (e.g. 'analyze-data', 'fetch-reports')"
                },
                "description": {
                    "type": "string",
                    "description": "Detailed description of what the task will do"
                },
                "command": {
                    "type": "string",
                    "description": "Optional shell command to execute in the background. If provided, the command runs asynchronously and stdout/stderr are captured as the result."
                },
                "timeout_secs": {
                    "type": "number",
                    "description": "Optional timeout in seconds (default: 300)"
                }
            },
            "required": ["name", "description"]
        })
    }

    fn execute(
        &self,
        parameters: ToolParameters,
    ) -> BoxFuture<'_, crate::error::Result<ToolResult>> {
        let spawner = self.spawner.clone();
        Box::pin(async move {
            let name = parameters
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("name".to_string()))?
                .to_string();

            let description = parameters
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("(no description)")
                .to_string();

            let command = parameters
                .get("command")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let has_command = command.is_some();
            let name_for_task = name.clone();

            let handle = if let Some(cmd) = command {
                // ── Command mode: run a real shell command in the background ──
                let description_for_task = description.clone();
                let cmd_for_task = cmd.clone();
                spawner.spawn(&name, async move {
                    info!(
                        task_name = %name_for_task,
                        command = %cmd_for_task,
                        "Background task executing shell command"
                    );

                    // Safety: validate command is not empty
                    if cmd_for_task.trim().is_empty() {
                        return Err(crate::error::ReactError::Other(
                            "Background task command is empty".into(),
                        ));
                    }

                    // Use tokio::process::Command for safe async execution
                    let output = tokio::process::Command::new("sh")
                        .arg("-c")
                        .arg(&cmd_for_task)
                        .kill_on_drop(true)
                        .output()
                        .await
                        .map_err(|e| {
                            crate::error::ReactError::Other(format!(
                                "Failed to execute background command: {e}"
                            ))
                        })?;

                    let stdout = truncate_output(&output.stdout);
                    let stderr = truncate_output(&output.stderr);
                    let exit_code = output.status.code().unwrap_or(-1);

                    let mut result = format!(
                        "Background task '{}' ({})\nExit code: {exit_code}\n",
                        name_for_task, description_for_task
                    );

                    if !stdout.is_empty() {
                        result.push_str(&format!("\nstdout:\n{stdout}"));
                    }
                    if !stderr.is_empty() {
                        result.push_str(&format!("\nstderr:\n{stderr}"));
                    }

                    if output.status.success() {
                        Ok(result)
                    } else {
                        // Still return Ok with the output — the task "completed"
                        // even though the command failed. The exit code is in the output.
                        Ok(result)
                    }
                })
            } else {
                // ── Description mode: register a named tracking task ──
                let description_for_task = description.clone();
                spawner.spawn(&name, async move {
                    info!(task_name = %name_for_task, "Background task registered (tracking only)");
                    Ok(format!(
                        "Background task '{}' completed: {}",
                        name_for_task, description_for_task
                    ))
                })
            };

            let task_id = handle.id.clone();
            info!(task_id = %task_id, name = %name, "Background task spawned");

            let mode_info = if has_command {
                " (running shell command)"
            } else {
                " (tracking task)"
            };

            Ok(ToolResult::success(format!(
                "Background task '{}' spawned{mode_info} with ID: {}\nUse check_task_status(task_id='{}') to check progress.",
                name, task_id, task_id
            )))
        })
    }
}

/// Truncate output bytes to [`MAX_OUTPUT_LEN`], appending a notice if truncated.
fn truncate_output(bytes: &[u8]) -> String {
    if bytes.len() <= MAX_OUTPUT_LEN {
        String::from_utf8_lossy(bytes).into_owned()
    } else {
        let mut s = String::from_utf8_lossy(&bytes[..MAX_OUTPUT_LEN]).into_owned();
        s.push_str(&format!(
            "\n... (output truncated, {} bytes total)",
            bytes.len()
        ));
        s
    }
}
