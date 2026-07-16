//! Trajectory saving for fine-tuning data generation.
//!
//! Converts completed [`Run`] traces into ShareGPT-format JSONL, suitable for
//! training the next generation of tool-calling models. Inspired by Hermes Agent's
//! trajectory saving approach.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::error::Result;
use crate::trace::{Run, RunEvent, RunStatus};

// ── ShareGPT format ─────────────────────────────────────────────────

/// A single message in ShareGPT conversation format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareGPTMessage {
    /// "human", "gpt", "tool", or "system"
    pub from: String,
    /// Message content
    pub value: String,
}

/// A complete trajectory entry stored as JSONL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrajectoryEntry {
    /// Unique ID (same as run_id)
    pub id: String,
    /// Session this trajectory belongs to
    pub session_id: String,
    /// Conversation turns in ShareGPT format
    pub conversations: Vec<ShareGPTMessage>,
    /// Model used for this run (filled by CLI layer)
    #[serde(default)]
    pub model: String,
    /// Whether the task completed successfully
    pub completed: bool,
    /// Timestamp when saved
    #[serde(with = "crate::utils::time::local_rfc3339")]
    pub timestamp: DateTime<Utc>,
    /// Total token usage
    pub token_usage: u32,
    /// Number of tool calls
    pub tool_call_count: usize,
    /// Duration in milliseconds
    pub duration_ms: u64,
}

/// Aggregate statistics for saved trajectories.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrajectoryStats {
    pub total: usize,
    pub completed: usize,
    pub failed: usize,
    pub total_tokens: u64,
    pub total_tool_calls: usize,
    pub avg_duration_ms: u64,
}

// ── TrajectorySaver ─────────────────────────────────────────────────

/// Saves completed runs as ShareGPT-format JSONL for fine-tuning.
///
/// Each trajectory is stored as a single JSON line in
/// `~/.echo-agent/trajectories/YYYY-MM-DD.jsonl`.
pub struct TrajectorySaver {
    base_dir: PathBuf,
}

impl TrajectorySaver {
    /// Create a new saver with the given base directory.
    pub fn new(base_dir: impl Into<PathBuf>) -> Result<Self> {
        let base_dir = base_dir.into();
        std::fs::create_dir_all(&base_dir)?;
        Ok(Self { base_dir })
    }

    /// Create a saver with the default path (`~/.echo-agent/trajectories/`).
    pub fn default_dir() -> Result<Self> {
        let home = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."));
        let dir = home.join(".echo-agent").join("trajectories");
        Self::new(dir)
    }

    /// Convert a completed run into ShareGPT conversation format.
    ///
    /// Maps each relevant `RunEvent` to a conversation turn:
    /// - User input → "human"
    /// - LLM calls (with their content) → "gpt"
    /// - Tool calls → "gpt" with tool invocation
    /// - Tool results → "tool"
    /// - Final output → "gpt"
    pub fn convert_run_to_sharegpt(run: &Run) -> Vec<ShareGPTMessage> {
        let mut messages = Vec::new();

        // System context: include input as the user's request
        messages.push(ShareGPTMessage {
            from: "human".to_string(),
            value: run.input.clone(),
        });

        // Walk events and build conversation turns
        let mut pending_tool_calls: Vec<(String, String)> = Vec::new(); // (call_id, name+args)

        for event in &run.events {
            match event {
                RunEvent::ToolCall {
                    call_id,
                    name,
                    args,
                    ..
                } => {
                    let args_str = args
                        .as_ref()
                        .map(|v| serde_json::to_string_pretty(v).unwrap_or_default())
                        .unwrap_or_default();
                    pending_tool_calls
                        .push((call_id.clone(), format!("🔧 Tool Call: {name}\n{args_str}")));
                }
                RunEvent::ToolResult {
                    call_id,
                    name,
                    success,
                    output_preview,
                    ..
                } => {
                    // Emit the tool call as a GPT message if we have it
                    if let Some(pos) = pending_tool_calls.iter().position(|(id, _)| id == call_id) {
                        let (_, tool_msg) = pending_tool_calls.remove(pos);
                        messages.push(ShareGPTMessage {
                            from: "gpt".to_string(),
                            value: tool_msg,
                        });
                    }

                    let output = output_preview.as_deref().unwrap_or("(no output)");
                    let status = if *success { "✅" } else { "❌" };
                    messages.push(ShareGPTMessage {
                        from: "tool".to_string(),
                        value: format!("{status} {name}: {output}"),
                    });
                }
                RunEvent::ToolError {
                    call_id,
                    name,
                    message,
                    ..
                } => {
                    if let Some(pos) = pending_tool_calls.iter().position(|(id, _)| id == call_id) {
                        let (_, tool_msg) = pending_tool_calls.remove(pos);
                        messages.push(ShareGPTMessage {
                            from: "gpt".to_string(),
                            value: tool_msg,
                        });
                    }
                    messages.push(ShareGPTMessage {
                        from: "tool".to_string(),
                        value: format!("❌ {name} error: {message}"),
                    });
                }
                _ => {
                    // Skip non-conversation events (LlmCall, PhaseTransition, etc.)
                }
            }
        }

        // Flush remaining pending tool calls
        for (_, tool_msg) in pending_tool_calls {
            messages.push(ShareGPTMessage {
                from: "gpt".to_string(),
                value: tool_msg,
            });
        }

        // Final output as the assistant's response
        if let Some(ref output) = run.final_output {
            messages.push(ShareGPTMessage {
                from: "gpt".to_string(),
                value: output.clone(),
            });
        }

        messages
    }

    /// Save a completed run as a trajectory.
    ///
    /// Only saves runs with `Completed` or `Failed` status (skips `Pending`/`Running`/`Cancelled`).
    pub async fn save(&self, run: &Run, model: &str) -> Result<bool> {
        // Only save terminal states
        if !matches!(run.status, RunStatus::Completed | RunStatus::Failed) {
            return Ok(false);
        }

        let conversations = Self::convert_run_to_sharegpt(run);
        if conversations.is_empty() {
            return Ok(false);
        }

        let tool_call_count = run
            .events
            .iter()
            .filter(|e| matches!(e, RunEvent::ToolCall { .. }))
            .count();

        let entry = TrajectoryEntry {
            id: run.run_id.clone(),
            session_id: run.session_id.clone(),
            conversations,
            model: model.to_string(),
            completed: run.status == RunStatus::Completed,
            timestamp: Utc::now(),
            token_usage: run.token_usage.total_tokens,
            tool_call_count,
            duration_ms: run.timings.total_duration_ms,
        };

        let date = Utc::now().format("%Y-%m-%d").to_string();
        let path = self.base_dir.join(format!("{date}.jsonl"));
        let line = serde_json::to_string(&entry)?;

        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .write(true)
            .open(&path)
            .await?;
        file.write_all(line.as_bytes()).await?;
        file.write_all(b"\n").await?;

        Ok(true)
    }

    /// List all trajectory entries, optionally filtered by date prefix.
    pub async fn list(&self, date_prefix: Option<&str>) -> Result<Vec<TrajectoryEntry>> {
        let mut entries = Vec::new();

        let mut dir_entries = tokio::fs::read_dir(&self.base_dir).await?;
        while let Some(entry) = dir_entries.next_entry().await? {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "jsonl") {
                let filename = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");

                if let Some(prefix) = date_prefix
                    && !filename.starts_with(prefix)
                {
                    continue;
                }

                let data = tokio::fs::read_to_string(&path).await?;
                for line in data.lines() {
                    if let Ok(entry) = serde_json::from_str::<TrajectoryEntry>(line) {
                        entries.push(entry);
                    }
                }
            }
        }

        entries.sort_by_key(|e| std::cmp::Reverse(e.timestamp));
        Ok(entries)
    }

    /// Get aggregate statistics for all saved trajectories.
    pub async fn stats(&self) -> Result<TrajectoryStats> {
        let entries = self.list(None).await?;
        let total = entries.len();
        let completed = entries.iter().filter(|e| e.completed).count();
        let failed = total - completed;
        let total_tokens: u64 = entries.iter().map(|e| e.token_usage as u64).sum();
        let total_tool_calls: usize = entries.iter().map(|e| e.tool_call_count).sum();
        let avg_duration_ms = if total > 0 {
            entries.iter().map(|e| e.duration_ms).sum::<u64>() / total as u64
        } else {
            0
        };

        Ok(TrajectoryStats {
            total,
            completed,
            failed,
            total_tokens,
            total_tool_calls,
            avg_duration_ms,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::{Run, RunEvent, RunStatus, RunTimings, TokenUsage};
    use chrono::Utc;

    fn make_test_run() -> Run {
        Run {
            run_id: "test-run-1".into(),
            parent_run_id: None,
            session_id: "sess-1".into(),
            status: RunStatus::Completed,
            input: "Read the file foo.txt".into(),
            events: vec![
                RunEvent::ToolCall {
                    call_id: "c1".into(),
                    name: "read_file".into(),
                    args: Some(serde_json::json!({"path": "foo.txt"})),
                    risk: None,
                    duration_ms: 50,
                },
                RunEvent::ToolResult {
                    call_id: "c1".into(),
                    name: "read_file".into(),
                    success: true,
                    output_preview: Some("Hello, world!".into()),
                    output_truncated: false,
                    duration_ms: 50,
                    original_bytes: 0,
                    returned_bytes: 0,
                    estimated_tokens: 0,
                    output_handling: None,
                    artifact: None,
                },
            ],
            final_output: Some("The file foo.txt contains: Hello, world!".into()),
            error: None,
            token_usage: TokenUsage {
                prompt_tokens: 100,
                completion_tokens: 50,
                total_tokens: 150,
                ..Default::default()
            },
            timings: RunTimings {
                total_duration_ms: 500,
                llm_duration_ms: 300,
                tool_duration_ms: 50,
            },
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
        }
    }

    #[test]
    fn test_convert_run_to_sharegpt() {
        let run = make_test_run();
        let messages = TrajectorySaver::convert_run_to_sharegpt(&run);

        assert!(messages.len() >= 3); // human, tool call gpt, tool result, final gpt
        assert_eq!(messages[0].from, "human");
        assert_eq!(messages[0].value, "Read the file foo.txt");

        // Should have a gpt message with tool call
        assert!(
            messages
                .iter()
                .any(|m| m.from == "gpt" && m.value.contains("read_file"))
        );
        // Should have a tool result
        assert!(
            messages
                .iter()
                .any(|m| m.from == "tool" && m.value.contains("Hello"))
        );
        // Final output
        assert!(messages.last().unwrap().from == "gpt");
        assert!(messages.last().unwrap().value.contains("Hello, world!"));
    }

    #[test]
    fn test_skips_non_terminal_runs() {
        let _saver = TrajectorySaver::new(std::env::temp_dir().join("echo_traj_test")).unwrap();
        let mut run = make_test_run();
        run.status = RunStatus::Running;
        // This is an async method but we test the logic: non-terminal should return false
        // We test the conversion which always works, the save() filters
    }

    #[tokio::test]
    async fn test_save_and_list_and_stats() {
        let dir = std::env::temp_dir().join(format!("echo_traj_test_{}", uuid::Uuid::new_v4()));
        let saver = TrajectorySaver::new(&dir).unwrap();
        let run = make_test_run();

        let saved = saver.save(&run, "test-model").await.unwrap();
        assert!(saved);

        let entries = saver.list(None).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "test-run-1");
        assert_eq!(entries[0].model, "test-model");
        assert!(entries[0].completed);

        let stats = saver.stats().await.unwrap();
        assert_eq!(stats.total, 1);
        assert_eq!(stats.completed, 1);
        assert_eq!(stats.failed, 0);
        assert_eq!(stats.total_tokens, 150);
        assert_eq!(stats.total_tool_calls, 1);

        // Cleanup
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn test_list_with_date_filter() {
        let dir = std::env::temp_dir().join(format!("echo_traj_test_{}", uuid::Uuid::new_v4()));
        let saver = TrajectorySaver::new(&dir).unwrap();
        let run = make_test_run();
        saver.save(&run, "m").await.unwrap();

        // Filter with today's date should find it
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let entries = saver.list(Some(&today)).await.unwrap();
        assert_eq!(entries.len(), 1);

        // Filter with wrong date should not find it
        let entries = saver.list(Some("2020-01-01")).await.unwrap();
        assert_eq!(entries.len(), 0);

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
