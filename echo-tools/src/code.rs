//! Inline code execution tool (Sprint 10b).
//!
//! Lets a subagent execute arbitrary Python/R/JS/... code snippets. The code
//! automatically runs inside `ctx.working_dir` (the worker's isolated tmpdir
//! for data/research workers — Sprint 10's `DataWorkspaceFactory` chain).
//!
//! Security model (AGENTS.md "local personal assistant"):
//! - With a configured `SandboxExecutor`: runs via the sandbox (Docker/OS/etc.).
//! - Without a sandbox: `tracing::warn!` + runs bare (local trusted machine —
//!   refusing would break out-of-box UX). This is the opposite of a web
//!   service's zero-trust deny.
//!
//! Modeled on `ShellTool` (`shell.rs`): holds `Option<Arc<dyn SandboxExecutor>>`
//! (the echo-core trait, no echo-execution dependency) and overrides
//! `execute_with_context` to honor `ctx.working_dir`.

use echo_core::error::{ReactError, Result, ToolError};
use echo_core::sandbox::{SandboxCommand, SandboxExecutor};
use echo_core::tools::permission::ToolPermission;
use echo_core::tools::{Tool, ToolContext, ToolParameters, ToolResult, ToolRiskLevel};
use futures::future::BoxFuture;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Languages supported by [`RunCodeTool`]. All use arg-based execution
/// (`-c`/`-e` flag) for consistency with the existing `Code` backend
/// (`local.rs`/`docker.rs`). (Switching all languages to stdin-based
/// execution to avoid ARG_MAX is a cross-cutting future follow-up, not
/// in scope for Sprint 10b — see D-10b-stdin-1.)
const SUPPORTED_LANGUAGES: &[&str] = &[
    "python",
    "python3",
    "r",
    "javascript",
    "js",
    "node",
    "ruby",
    "perl",
    "php",
    "bash",
    "sh",
];

/// Validate a language against the supported set (case-insensitive).
///
/// Decision D-10b-case-1: the LLM may emit `Python`/`PYTHON`/`R`, so we
/// normalize to lowercase before matching.
///
/// Returns the normalized lowercase language, or an error.
fn validate_language(language: &str) -> Result<String> {
    let normalized = language.to_lowercase();
    if SUPPORTED_LANGUAGES.iter().any(|&l| l == normalized) {
        Ok(normalized)
    } else {
        // ToolError::InvalidParameter is a struct variant {name, message}
        // (echo-core/src/error.rs:118). Wrap explicitly into ReactError to
        // avoid Into-ambiguity at the call site.
        Err(ReactError::from(ToolError::InvalidParameter {
            name: "language".to_string(),
            message: format!(
                "Unsupported language '{language}'. Supported: {:?}",
                SUPPORTED_LANGUAGES
            ),
        }))
    }
}

/// Inline code execution tool.
pub struct RunCodeTool {
    /// Interior-mutable so `Tool::set_sandbox` (a `&self` method) can inject
    /// the executor after construction (via `set_sandbox_manager` →
    /// `ToolManager::apply_sandbox`), without requiring `&mut self`.
    sandbox: Mutex<Option<Arc<dyn SandboxExecutor>>>,
    /// Per-call timeout in seconds (default 60, capped at 300 like ShellTool).
    timeout_secs: u64,
}

impl Default for RunCodeTool {
    fn default() -> Self {
        Self {
            sandbox: Mutex::new(None),
            timeout_secs: 60,
        }
    }
}

impl RunCodeTool {
    pub fn new() -> Self {
        Self::default()
    }

    /// Inject a sandbox executor (Docker / OS-sandbox / local). Without this,
    /// the tool falls back to a bare `tokio::process` run with a warning.
    pub fn with_sandbox(self, sandbox: Arc<dyn SandboxExecutor>) -> Self {
        *self.sandbox.lock().expect("sandbox mutex poisoned") = Some(sandbox);
        self
    }

    /// Per-call timeout (default 60s, capped at 300s like ShellTool).
    pub fn with_timeout_secs(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }
}

impl Tool for RunCodeTool {
    fn name(&self) -> &str {
        "run_code"
    }

    fn description(&self) -> &str {
        "执行一段代码(Python/R/JavaScript/...)。代码自动在当前任务工作目录(working_dir)中运行 — 无需创建新目录,直接读写当前目录文件即可。返回 stdout/stderr/exit code。"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "language": {
                    "type": "string",
                    "enum": ["python", "r", "javascript", "ruby", "perl", "php", "bash"],
                    "description": "代码语言(大小写不敏感)。默认 python。"
                },
                "code": {
                    "type": "string",
                    "description": "要执行的代码片段。"
                },
                "timeout": {
                    "type": "integer",
                    "description": "超时秒数(可选,默认 60,上限 300)。"
                }
            },
            "required": ["language", "code"]
        })
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Execute]
    }

    fn risk_level(&self) -> ToolRiskLevel {
        ToolRiskLevel::Dangerous
    }

    /// P2: receive sandbox at agent-setup time (via ToolManager::apply_sandbox).
    fn set_sandbox(&self, sandbox: Arc<dyn SandboxExecutor>) -> bool {
        *self.sandbox.lock().expect("sandbox mutex poisoned") = Some(sandbox);
        true
    }

    fn execute_with_context<'a>(
        &'a self,
        parameters: ToolParameters,
        ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolResult>> {
        Box::pin(async move {
            // Read sandbox (interior-mutable; clone the Arc out of the lock).
            let sandbox = self.sandbox.lock().expect("sandbox mutex poisoned").clone();
            // 1. Parse + circuit-break on unknown language (user review patch #1).
            let raw_lang = parameters
                .get("language")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    ReactError::from(ToolError::MissingParameter("language".to_string()))
                })?;
            let language = validate_language(raw_lang)?;

            let code = parameters
                .get("code")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ReactError::from(ToolError::MissingParameter("code".to_string())))?;

            let timeout_secs = parameters
                .get("timeout")
                .and_then(|v| v.as_u64())
                .unwrap_or(self.timeout_secs)
                .min(300);
            let timeout_duration = Duration::from_secs(timeout_secs);

            // 2. Build the sandbox command, binding the worker's working_dir.
            //    `with_timeout` takes a Duration (echo-core/src/sandbox.rs:148).
            let mut sandbox_cmd = SandboxCommand::code(&language, code);
            if let Some(dir) = &ctx.working_dir {
                sandbox_cmd = sandbox_cmd.with_working_dir(dir.clone());
            }
            sandbox_cmd = sandbox_cmd.with_timeout(timeout_duration);

            // 3. Execute via sandbox if configured, else warn + bare fallback.
            if let Some(sandbox) = &sandbox {
                match tokio::time::timeout(timeout_duration, sandbox.execute(sandbox_cmd)).await {
                    Ok(Ok(result)) => {
                        if result.success() {
                            Ok(ToolResult::success(result.combined_output()))
                        } else {
                            Ok(ToolResult::error(format!(
                                "Code execution failed (exit code {}).\n{}",
                                result.exit_code,
                                result.combined_output()
                            )))
                        }
                    }
                    Ok(Err(e)) => Ok(ToolResult::error(format!("Sandbox execution failed: {e}"))),
                    Err(_) => Ok(ToolResult::error(format!(
                        "⏱️ Code execution timed out after {timeout_secs}s"
                    ))),
                }
            } else {
                // Decision D-10b-RCE-1: warn-not-deny. EKO is a local personal
                // assistant; refusing here would break out-of-box UX.
                tracing::warn!(
                    language = %language,
                    "run_code: no SandboxExecutor configured — running unsandboxed. \
                     Ensure you trust the generated code."
                );
                run_bare(&language, code, ctx, timeout_duration).await
            }
        })
    }
}

/// Bare-process fallback when no `SandboxExecutor` is configured.
///
/// Passes code via the interpreter's `-c`/`-e` flag (mirroring the arg-based
/// convention of the `Code` backend), honoring `ctx.working_dir` as the
/// process's `current_dir`.
async fn run_bare(
    language: &str,
    code: &str,
    ctx: &ToolContext,
    timeout_duration: Duration,
) -> Result<ToolResult> {
    let (interpreter, flag) = match language {
        "python" | "python3" => ("python3", "-c"),
        "node" | "javascript" | "js" => ("node", "-e"),
        "r" => ("Rscript", "-e"),
        "ruby" => ("ruby", "-e"),
        "perl" => ("perl", "-e"),
        "php" => ("php", "-r"),
        "bash" | "sh" => ("sh", "-c"),
        other => {
            return Err(ReactError::from(ToolError::InvalidParameter {
                name: "language".to_string(),
                message: format!("Unsupported language '{other}'"),
            }));
        }
    };

    let mut command = tokio::process::Command::new(interpreter);
    command.arg(flag).arg(code);
    command.kill_on_drop(true);
    if let Some(dir) = &ctx.working_dir {
        command.current_dir(dir);
    }

    let output = tokio::time::timeout(timeout_duration, command.output())
        .await
        .map_err(|_| {
            echo_core::error::ReactError::Other(format!(
                "run_code timed out after {timeout_duration:?}"
            ))
        })?
        .map_err(|e| {
            ReactError::from(ToolError::ExecutionFailed {
                tool: "run_code".to_string(),
                message: format!("Failed to spawn interpreter {interpreter}: {e}"),
            })
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if output.status.success() {
        let combined = if stderr.is_empty() {
            stdout
        } else {
            format!("{stdout}\n{stderr}")
        };
        Ok(ToolResult::success(combined))
    } else {
        Ok(ToolResult::error(format!(
            "Code execution failed (exit code {}).\n{}\n{}",
            output.status.code().unwrap_or(-1),
            stdout,
            stderr
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::sandbox::ExecutionResult;

    #[test]
    fn validate_language_lowercases_and_accepts_known() {
        // Decision D-10b-case-1: LLM may emit "Python"/"PYTHON"/"R".
        assert_eq!(validate_language("Python").unwrap(), "python");
        assert_eq!(validate_language("PYTHON").unwrap(), "python");
        assert_eq!(validate_language("R").unwrap(), "r");
        assert_eq!(validate_language("JavaScript").unwrap(), "javascript");
    }

    #[test]
    fn validate_language_rejects_unknown() {
        // Circuit-breaker (user review patch #1): unknown language fails at the
        // tool layer, never reaches the sandbox.
        assert!(validate_language("haskell").is_err());
        assert!(validate_language("").is_err());
    }

    #[test]
    fn supported_languages_includes_r() {
        // Sprint 10b headline: R is a first-class language.
        assert!(SUPPORTED_LANGUAGES.contains(&"r"));
        assert!(SUPPORTED_LANGUAGES.contains(&"python"));
    }

    /// Sprint 10b headline: data worker's tmpdir (ctx.working_dir) must be
    /// propagated to the SandboxCommand so the code runs in the worker's
    /// isolated workspace, not the process cwd.
    #[tokio::test]
    async fn run_code_binds_ctx_working_dir() {
        use echo_core::sandbox::IsolationLevel;
        use std::collections::HashMap;

        /// A stub sandbox that records the working_dir it was asked to use.
        struct RecordingSandbox {
            seen_working_dir: std::sync::Mutex<Option<std::path::PathBuf>>,
        }

        impl SandboxExecutor for RecordingSandbox {
            fn name(&self) -> &str {
                "recording"
            }
            fn isolation_level(&self) -> IsolationLevel {
                IsolationLevel::None
            }
            fn is_available(&self) -> BoxFuture<'_, bool> {
                Box::pin(async { true })
            }
            fn execute<'a>(
                &'a self,
                command: SandboxCommand,
            ) -> BoxFuture<'a, std::result::Result<ExecutionResult, echo_core::error::ReactError>>
            {
                // Capture BEFORE the await (command is consumed).
                *self.seen_working_dir.lock().unwrap() = command.working_dir.clone();
                Box::pin(async move {
                    Ok(ExecutionResult {
                        exit_code: 0,
                        stdout: "ok".to_string(),
                        stderr: String::new(),
                        duration: Duration::from_millis(1),
                        sandbox_type: "recording".to_string(),
                        timed_out: false,
                        output_truncated: false,
                        stdout_bytes: 2,
                        stderr_bytes: 0,
                    })
                })
            }
        }

        let sandbox = Arc::new(RecordingSandbox {
            seen_working_dir: std::sync::Mutex::new(None),
        });
        let captured: Arc<RecordingSandbox> = sandbox.clone();
        let tool = RunCodeTool::new().with_sandbox(sandbox);

        // ToolParameters = HashMap<String, serde_json::Value> (echo-core:266).
        // Idiomatic construction (matches shell.rs:756, web/search.rs:350):
        let mut params: HashMap<String, serde_json::Value> = HashMap::new();
        params.insert("language".to_string(), serde_json::json!("python"));
        params.insert("code".to_string(), serde_json::json!("print(1+1)"));

        let ctx = ToolContext {
            working_dir: Some(std::path::PathBuf::from("/tmp/eko-data-worker-xyz")),
            conversation_id: None,
            run_id: None,
            turn_id: None,
            execution_id: None,
            cancel: None,
            trace_sink: None,
            delegation_policy: None,
        };
        let _ = tool.execute_with_context(params, &ctx).await.unwrap();

        let seen = captured.seen_working_dir.lock().unwrap().clone();
        assert_eq!(
            seen,
            Some(std::path::PathBuf::from("/tmp/eko-data-worker-xyz")),
            "ctx.working_dir must propagate to the sandbox command"
        );
    }
}
