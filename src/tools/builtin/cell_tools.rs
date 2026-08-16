//! Background command cell tools — `wait` / `stop_cell` / `list_cells`.
//!
//! Companion surface to `shell(background=true)`: the shell tool launches a
//! long-running command as a cell and returns a `cell_id` immediately; these
//! tools long-poll, stop, and list cells through a shared
//! [`CommandCellRegistry`]. Registered whenever a registry is injected into
//! the agent (one process-wide registry shared by the main agent and its
//! subagents).

use echo_core::tools::cell::{CommandCellDelta, CommandCellRegistry};
use echo_core::tools::{Tool, ToolParameters, ToolResult};
use futures::future::BoxFuture;
use serde_json::{Value, json};
use std::sync::Arc;
use tracing::debug;

/// Default long-poll yield budget (30s), matching the Codex awaiter's start.
const DEFAULT_YIELD_MS: u64 = 30_000;
/// Upper bound for one wait round (1h), matching Codex
/// `background_terminal_max_timeout = 3600000`.
const MAX_YIELD_MS: u64 = 3_600_000;

/// Long-poll a background command cell.
///
/// Returns when the cell reaches a terminal phase, when new output appears
/// after the caller's cursor, or when `yield_time_ms` elapses — whichever
/// comes first. Retry-safe: re-call with the returned `next_cursor`.
pub struct WaitCellTool {
    registry: Arc<dyn CommandCellRegistry>,
}

impl WaitCellTool {
    pub fn new(registry: Arc<dyn CommandCellRegistry>) -> Self {
        Self { registry }
    }
}

impl Tool for WaitCellTool {
    fn name(&self) -> &str {
        "wait"
    }

    fn description(&self) -> &str {
        "Long-poll a background command cell (from shell with background=true). Returns when the command \
         finishes, produces new output, or the yield budget expires. Pass the previously returned \
         next_cursor as cursor to continue reading incremental output. A terminal cell can still have \
         unread capped output, so keep draining while next_cursor is below output bytes. \
         yield_time_ms=0 is a non-blocking status check."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "cell_id": {
                    "type": "string",
                    "description": "The cell ID returned by the background shell launch"
                },
                "cursor": {
                    "type": "integer",
                    "description": "Byte cursor from the previous wait's next_cursor (0 for the first call)"
                },
                "yield_time_ms": {
                    "type": "integer",
                    "description": "How long to wait for progress before returning (default 30000, max 3600000)"
                }
            },
            "required": ["cell_id"]
        })
    }

    fn execute(
        &self,
        parameters: ToolParameters,
    ) -> BoxFuture<'_, crate::error::Result<ToolResult>> {
        let registry = self.registry.clone();
        Box::pin(async move {
            let cell_id = parameters
                .get("cell_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| crate::error::ToolError::MissingParameter("cell_id".to_string()))?
                .to_string();
            let cursor = parameters
                .get("cursor")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let yield_ms = parameters
                .get("yield_time_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(DEFAULT_YIELD_MS)
                .min(MAX_YIELD_MS);

            debug!(cell_id = %cell_id, cursor, yield_ms, "Waiting on command cell");

            match registry.wait(&cell_id, cursor, yield_ms).await {
                Ok(delta) => Ok(ToolResult::success(format_wait_result(
                    &cell_id, yield_ms, &delta,
                ))),
                Err(error) => Ok(ToolResult::error(error)),
            }
        })
    }

    /// Hour-scale long-polls are the whole point of this tool: opt out of the
    /// per-tool/batch timeout (same exemption as `agent_tool`/`task_execute`)
    /// and let the explicit `yield_time_ms` be the only deadline.
    fn exempt_from_batch_timeout(&self) -> bool {
        true
    }
}

/// Render one wait round for the model: phase, exit code, incremental output,
/// and the cursor to re-pass next time.
fn format_wait_result(cell_id: &str, yield_ms: u64, delta: &CommandCellDelta) -> String {
    use echo_core::tools::cell::CommandCellPhase;

    let snapshot = &delta.snapshot;
    let mut lines = Vec::new();
    lines.push(format!(
        "cell {cell_id} [{}]: {}",
        snapshot.name,
        snapshot.phase.as_str()
    ));
    if snapshot.phase.is_terminal() {
        if let Some(code) = snapshot.exit_code {
            lines.push(format!("exit_code: {code}"));
        } else {
            // 超时/取消/启动失败没有退出码。
            lines.push("exit_code: none (timed out, cancelled, or failed to launch)".to_string());
        }
    }
    if matches!(snapshot.phase, CommandCellPhase::LaunchFailed) {
        lines.push(
            "the command could not be spawned; check the command and working directory".to_string(),
        );
    }
    lines.push(format!(
        "output bytes: {}{}",
        snapshot.total_output_bytes,
        if snapshot.output_truncated {
            " (early output discarded, buffer retention cap)"
        } else {
            ""
        }
    ));
    if let Some(artifact) = &snapshot.output_artifact {
        lines.push(format!(
            "complete output artifact: {} ({} payload bytes, sha256 {})",
            artifact.path.display(),
            artifact.payload_bytes,
            artifact.sha256
        ));
    }

    if delta.new_output.is_empty() {
        if snapshot.phase.is_terminal() {
            lines.push("no new output since the last cursor.".to_string());
        } else {
            lines.push(format!(
                "still running, no new output within the {yield_ms} ms yield budget — call wait again."
            ));
        }
    } else {
        lines.push("--- new output ---".to_string());
        lines.push(delta.new_output.clone());
        if delta.output_elided {
            lines.push("(output elided: continue with next_cursor, or early bytes were discarded from memory)".to_string());
        }
    }

    lines.push(format!("next_cursor: {}", delta.next_cursor));
    if delta.next_cursor < snapshot.total_output_bytes {
        lines.push(format!(
            "unread output remains; call wait again with cursor={} even though the cell may already be terminal.",
            delta.next_cursor
        ));
    } else {
        lines.push(format!(
            "re-pass cursor={} on your next wait call.",
            delta.next_cursor
        ));
    }
    lines.join("\n")
}

/// Stop (kill) a background command cell by ID.
pub struct StopCellTool {
    registry: Arc<dyn CommandCellRegistry>,
}

impl StopCellTool {
    pub fn new(registry: Arc<dyn CommandCellRegistry>) -> Self {
        Self { registry }
    }
}

impl Tool for StopCellTool {
    fn name(&self) -> &str {
        "stop_cell"
    }

    fn description(&self) -> &str {
        "Stop a background command cell: kills its process group. The cell transitions to the \
         cancelled phase and its captured output stays readable via wait."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "cell_id": {
                    "type": "string",
                    "description": "The cell ID to stop"
                }
            },
            "required": ["cell_id"]
        })
    }

    fn execute(
        &self,
        parameters: ToolParameters,
    ) -> BoxFuture<'_, crate::error::Result<ToolResult>> {
        let registry = self.registry.clone();
        Box::pin(async move {
            let cell_id = parameters
                .get("cell_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| crate::error::ToolError::MissingParameter("cell_id".to_string()))?
                .to_string();

            if registry.stop(&cell_id) {
                Ok(ToolResult::success(format!(
                    "cell {cell_id}: stop requested; the process group is being killed. \
                     Use wait(cell_id) to observe the cancelled phase."
                )))
            } else {
                Ok(ToolResult::error(format!(
                    "cell '{cell_id}' not found (it may have been pruned from terminal history)"
                )))
            }
        })
    }
}

/// List every tracked background command cell (running and terminal).
pub struct ListCellsTool {
    registry: Arc<dyn CommandCellRegistry>,
}

impl ListCellsTool {
    pub fn new(registry: Arc<dyn CommandCellRegistry>) -> Self {
        Self { registry }
    }
}

impl Tool for ListCellsTool {
    fn name(&self) -> &str {
        "list_cells"
    }

    fn description(&self) -> &str {
        "List all background command cells with their phase, exit code, and output size."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
        })
    }

    fn execute(
        &self,
        _parameters: ToolParameters,
    ) -> BoxFuture<'_, crate::error::Result<ToolResult>> {
        let registry = self.registry.clone();
        Box::pin(async move {
            let cells = registry.list().await;
            if cells.is_empty() {
                return Ok(ToolResult::success("No background command cells."));
            }
            let mut lines = Vec::new();
            lines.push(format!("Background command cells ({}):", cells.len()));
            for cell in &cells {
                let exit = cell
                    .exit_code
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "-".to_string());
                lines.push(format!(
                    "  - {} [{}] {}: {} (exit {}, {} bytes{})",
                    cell.cell_id,
                    cell.name,
                    cell.phase.as_str(),
                    if cell.phase.is_terminal() {
                        "finished"
                    } else {
                        "in progress"
                    },
                    exit,
                    cell.total_output_bytes,
                    if cell.output_truncated {
                        ", truncated"
                    } else {
                        ""
                    }
                ));
            }
            Ok(ToolResult::success(lines.join("\n")))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::tools::cell::{
        CommandCellPhase, CommandCellRegistry, CommandCellRequest, CommandCellSnapshot,
    };
    use echo_orchestration::tasks::BackgroundCommandManager;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Registry wrapper that counts wait calls so tests can assert the wait
    /// tool actually consulted the shared registry.
    struct CountingRegistry {
        inner: BackgroundCommandManager,
        wait_calls: AtomicU64,
    }

    impl CommandCellRegistry for CountingRegistry {
        fn launch(&self, request: CommandCellRequest) -> Result<String, String> {
            self.inner.launch(request)
        }

        fn wait(
            &self,
            cell_id: &str,
            cursor: u64,
            yield_ms: u64,
        ) -> BoxFuture<'_, Result<CommandCellDelta, String>> {
            self.wait_calls.fetch_add(1, Ordering::Relaxed);
            self.inner.wait(cell_id, cursor, yield_ms)
        }

        fn stop(&self, cell_id: &str) -> bool {
            self.inner.stop(cell_id)
        }

        fn list(&self) -> BoxFuture<'_, Vec<CommandCellSnapshot>> {
            self.inner.list()
        }
    }

    fn params(json: Value) -> ToolParameters {
        match json {
            Value::Object(map) => map.into_iter().collect(),
            _ => ToolParameters::new(),
        }
    }

    #[tokio::test]
    async fn wait_tool_round_trips_cell_output_and_cursor() {
        let registry = Arc::new(CountingRegistry {
            inner: BackgroundCommandManager::default(),
            wait_calls: AtomicU64::new(0),
        });
        let cell_id = registry
            .inner
            .launch(CommandCellRequest {
                command: "echo wait-tool-ok".to_string(),
                working_dir: None,
                timeout_secs: None,
                ..Default::default()
            })
            .unwrap_or_default();
        assert!(!cell_id.is_empty());

        let tool = WaitCellTool::new(registry.clone());
        // 排空到终态: wait 工具的返回文本必须带输出与 next_cursor。
        let mut cursor = 0_u64;
        let mut saw_output = false;
        for _ in 0..20 {
            let result = tool
                .execute(params(json!({
                    "cell_id": cell_id,
                    "cursor": cursor,
                    "yield_time_ms": 5_000
                })))
                .await
                .unwrap();
            assert!(result.success);
            if result.output.contains("wait-tool-ok") {
                saw_output = true;
            }
            if result.output.contains("succeeded") {
                break;
            }
            let next = result
                .output
                .lines()
                .find_map(|line| line.strip_prefix("next_cursor: "))
                .and_then(|value| value.parse::<u64>().ok());
            cursor = next.unwrap_or(cursor);
        }
        assert!(saw_output, "wait tool output must include cell output");
        assert!(registry.wait_calls.load(Ordering::Relaxed) >= 1);
    }

    #[tokio::test]
    async fn wait_tool_reports_unknown_cell_as_error() {
        let tool = WaitCellTool::new(Arc::new(BackgroundCommandManager::default()));
        let result = tool
            .execute(params(json!({ "cell_id": "missing" })))
            .await
            .unwrap();
        assert!(!result.success);
    }

    #[tokio::test]
    async fn list_and_stop_tools_reflect_registry_state() {
        let registry = Arc::new(BackgroundCommandManager::default());
        let cell_id = registry
            .launch(CommandCellRequest {
                command: "sleep 30".to_string(),
                working_dir: None,
                timeout_secs: None,
                ..Default::default()
            })
            .unwrap_or_default();

        let list = ListCellsTool::new(registry.clone());
        let listed = list.execute(params(json!({}))).await.unwrap();
        assert!(listed.success);
        assert!(listed.output.contains("running"));

        let stop = StopCellTool::new(registry.clone());
        let stopped = stop
            .execute(params(json!({ "cell_id": cell_id })))
            .await
            .unwrap();
        assert!(stopped.success);

        // 停止后 wait 观察到 cancelled 终态。
        let wait = WaitCellTool::new(registry.clone());
        let observed = wait
            .execute(params(
                json!({ "cell_id": cell_id, "yield_time_ms": 10_000 }),
            ))
            .await
            .unwrap();
        assert!(observed.output.contains("cancelled"));

        let unknown = stop
            .execute(params(json!({ "cell_id": "no-such" })))
            .await
            .unwrap();
        assert!(!unknown.success);
    }

    #[test]
    fn wait_tool_is_exempt_from_batch_timeout() {
        let tool = WaitCellTool::new(Arc::new(BackgroundCommandManager::default()));
        assert!(tool.exempt_from_batch_timeout());
        assert!(tool.manages_own_timeout());
        let _phase = CommandCellPhase::Running;
    }
}
