//! Sandboxed inline or persisted-script execution.
//!
//! Lets a subagent execute arbitrary Python/R/JS/... code snippets or a
//! reviewable script already saved inside `ctx.working_dir`. Persisted scripts
//! are executed directly by the language runtime rather than being read back
//! into another inline wrapper.
//!
//! `run_code` is an agent-controlled arbitrary-code primitive, so it always
//! requires an OS-level or stronger [`echo_core::sandbox::SandboxExecutor`]. Interactive terminals
//! remain a separate user-controlled capability and are not gated here.
//!
//! Modeled on `ShellTool` (`shell.rs`): holds `Option<Arc<dyn SandboxExecutor>>`
//! (the echo-core trait, no echo-execution dependency) and overrides
//! `execute_with_context` to honor `ctx.working_dir`.

use echo_core::error::{ReactError, Result, ToolError};
use echo_core::sandbox::{
    ExecutionResult, IsolationLevel, ResourceLimits, SandboxCommand, SandboxExecutor,
};
use echo_core::tools::permission::ToolPermission;
use echo_core::tools::{
    Tool, ToolContext, ToolFailure, ToolFailureCategory, ToolParameters, ToolResult,
    ToolResultKind, ToolRiskLevel, ToolSideEffect,
};
use futures::future::BoxFuture;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Languages supported by [`RunCodeTool`]. Inline code uses the existing
/// arg-based `Code` backend; persisted scripts use a direct interpreter
/// program invocation.
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
const DEFAULT_MAX_OUTPUT_BYTES: u64 = 1024 * 1024;

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

fn script_program(language: &str) -> Result<&'static str> {
    match language {
        "python" | "python3" => Ok("python3"),
        "r" => Ok("Rscript"),
        "javascript" | "js" | "node" => Ok("node"),
        "ruby" => Ok("ruby"),
        "perl" => Ok("perl"),
        "php" => Ok("php"),
        "bash" => Ok("bash"),
        "sh" => Ok("sh"),
        _ => Err(ReactError::from(ToolError::InvalidParameter {
            name: "language".to_string(),
            message: format!("No script runtime is configured for '{language}'"),
        })),
    }
}

fn validate_script_path(script_path: &str, working_dir: Option<&Path>) -> Result<PathBuf> {
    let trimmed = script_path.trim();
    if trimmed.is_empty() {
        return Err(ReactError::from(ToolError::InvalidParameter {
            name: "script_path".to_string(),
            message: "Script path cannot be empty".to_string(),
        }));
    }

    let relative = Path::new(trimmed);
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(ReactError::from(ToolError::InvalidParameter {
            name: "script_path".to_string(),
            message: "Script path must stay inside the execution working directory".to_string(),
        }));
    }

    let base = match working_dir {
        Some(path) => path.to_path_buf(),
        None => std::env::current_dir().map_err(|error| ToolError::ExecutionFailed {
            tool: "run_code".to_string(),
            message: format!("Cannot resolve the current working directory: {error}"),
        })?,
    };
    let canonical_base = base
        .canonicalize()
        .map_err(|error| ToolError::ExecutionFailed {
            tool: "run_code".to_string(),
            message: format!("Cannot access the execution working directory: {error}"),
        })?;
    let resolved = base.join(relative);
    let canonical_script = resolved
        .canonicalize()
        .map_err(|error| ToolError::ExecutionFailed {
            tool: "run_code".to_string(),
            message: format!("Cannot access script '{trimmed}': {error}"),
        })?;
    if !canonical_script.starts_with(&canonical_base) || !canonical_script.is_file() {
        return Err(ReactError::from(ToolError::InvalidParameter {
            name: "script_path".to_string(),
            message: "Script must be a file inside the execution working directory".to_string(),
        }));
    }

    Ok(relative.to_path_buf())
}

/// Sandboxed inline or persisted-script execution tool.
pub struct RunCodeTool {
    /// Interior-mutable so `Tool::set_sandbox` (a `&self` method) can inject
    /// the executor after construction (via `set_sandbox_manager` →
    /// `ToolManager::apply_sandbox`), without requiring `&mut self`.
    sandbox: Mutex<Option<Arc<dyn SandboxExecutor>>>,
    /// Per-call timeout in seconds (default 60, capped at 300 like ShellTool).
    timeout_secs: u64,
    /// Shared retained stdout + stderr budget.
    max_output_bytes: u64,
}

impl Default for RunCodeTool {
    fn default() -> Self {
        Self {
            sandbox: Mutex::new(None),
            timeout_secs: 60,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
        }
    }
}

impl RunCodeTool {
    pub fn new() -> Self {
        Self::default()
    }

    /// Inject an OS-level or stronger sandbox executor.
    pub fn with_sandbox(self, sandbox: Arc<dyn SandboxExecutor>) -> Self {
        *self
            .sandbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(sandbox);
        self
    }

    /// Per-call timeout (default 60s, capped at 300s like ShellTool).
    pub fn with_timeout_secs(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    /// Override the retained stdout + stderr byte budget.
    pub fn with_max_output_bytes(mut self, max_output_bytes: u64) -> Self {
        self.max_output_bytes = max_output_bytes;
        self
    }
}

impl Tool for RunCodeTool {
    fn name(&self) -> &str {
        "run_code"
    }

    fn description(&self) -> &str {
        "Execute Python/R/JavaScript/etc. code in the task working directory. For real analysis, save the script first and run it via `script_path` (a reviewed file); for quick exploration use `code`. Provide exactly one of the two. Returns stdout/stderr/exit code."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "language": {
                    "type": "string",
                    "enum": ["python", "r", "javascript", "ruby", "perl", "php", "bash"],
                    "description": "Code language (case-insensitive). Defaults to python."
                },
                "code": {
                    "type": "string",
                    "description": "Inline code snippet for quick execution; mutually exclusive with `script_path`."
                },
                "script_path": {
                    "type": "string",
                    "description": "Relative path of a saved, reviewed script inside `working_dir`; mutually exclusive with `code`."
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in seconds (optional, default 60, max 300)."
                }
            },
            "required": ["language"],
            "oneOf": [
                {"required": ["code"], "not": {"required": ["script_path"]}},
                {"required": ["script_path"], "not": {"required": ["code"]}}
            ]
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
        *self
            .sandbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(sandbox);
        true
    }

    fn execute_with_context<'a>(
        &'a self,
        parameters: ToolParameters,
        ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolResult>> {
        Box::pin(async move {
            let sandbox = self
                .sandbox
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
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
                .and_then(|value| value.as_str())
                .filter(|value| !value.trim().is_empty());
            let script_path = parameters
                .get("script_path")
                .and_then(|value| value.as_str())
                .filter(|value| !value.trim().is_empty());
            let (mut sandbox_cmd, execution_mode, persisted_script) = match (code, script_path) {
                (Some(code), None) => (SandboxCommand::code(&language, code), "inline", None),
                (None, Some(script_path)) => {
                    let relative = validate_script_path(script_path, ctx.working_dir.as_deref())?;
                    let program = script_program(&language)?;
                    (
                        SandboxCommand::program(
                            program,
                            vec![relative.to_string_lossy().to_string()],
                        ),
                        "script",
                        Some(relative),
                    )
                }
                (Some(_), Some(_)) => {
                    return Err(ReactError::from(ToolError::InvalidParameter {
                        name: "code/script_path".to_string(),
                        message: "Provide exactly one of code or script_path".to_string(),
                    }));
                }
                (None, None) => {
                    return Err(ReactError::from(ToolError::InvalidParameter {
                        name: "code/script_path".to_string(),
                        message: "Provide exactly one of code or script_path".to_string(),
                    }));
                }
            };

            let timeout_secs = parameters
                .get("timeout")
                .and_then(|v| v.as_u64())
                .unwrap_or(self.timeout_secs)
                .clamp(1, 300);
            let timeout_duration = Duration::from_secs(timeout_secs);

            // 2. Bind the Subagent working directory and minimum isolation.
            //    `with_timeout` takes a Duration (echo-core/src/sandbox.rs:148).
            sandbox_cmd = sandbox_cmd.with_minimum_isolation(IsolationLevel::OsSandbox);
            if let Some(dir) = &ctx.working_dir {
                sandbox_cmd = sandbox_cmd.with_working_dir(dir.clone());
            }
            sandbox_cmd = sandbox_cmd.with_timeout(timeout_duration);

            let Some(sandbox) = sandbox else {
                return Ok(sandbox_unavailable_result(
                    "run_code requires a configured SandboxExecutor; unsandboxed execution is disabled",
                ));
            };
            if sandbox.isolation_level() < IsolationLevel::OsSandbox {
                return Ok(sandbox_unavailable_result(format!(
                    "run_code requires OS-level isolation, but executor '{}' provides {}",
                    sandbox.name(),
                    sandbox.isolation_level()
                )));
            }
            if ctx
                .cancel
                .as_ref()
                .is_some_and(|cancel| cancel.is_cancelled())
            {
                return Ok(cancelled_result(
                    "run_code was cancelled before sandbox startup",
                ));
            }
            if !sandbox.is_available().await {
                return Ok(sandbox_unavailable_result(format!(
                    "run_code sandbox '{}' is not available on this host",
                    sandbox.name()
                )));
            }

            let mut limits = ResourceLimits {
                cpu_time_secs: Some(timeout_secs),
                max_output_bytes: Some(self.max_output_bytes),
                network: false,
                ..Default::default()
            };
            if let Some(working_dir) = &ctx.working_dir {
                limits.writable_paths.push(working_dir.clone());
            }

            match sandbox
                .execute_with_limits_and_cancel(sandbox_cmd, limits, ctx.cancel.clone())
                .await
            {
                Ok(result) => {
                    let mut tool_result = tool_result_from_execution(
                        result,
                        ctx.working_dir.as_ref(),
                        sandbox.isolation_level(),
                        self.max_output_bytes,
                    )
                    .with_meta("execution_mode", execution_mode);
                    if let Some(script) = persisted_script {
                        tool_result = tool_result
                            .with_meta("script_path", script.to_string_lossy().to_string());
                    }
                    Ok(tool_result)
                }
                Err(error) => Ok(sandbox_error_result(error)),
            }
        })
    }
}

fn sandbox_unavailable_result(message: impl Into<String>) -> ToolResult {
    ToolResult::error(message).with_failure(
        ToolFailure::new(ToolFailureCategory::Unavailable)
            .with_postcondition("configure an OS-level SandboxExecutor before retrying run_code"),
    )
}

fn cancelled_result(message: impl Into<String>) -> ToolResult {
    let mut failure = ToolFailure::new(ToolFailureCategory::Cancelled);
    failure.side_effect = ToolSideEffect::Possible;
    failure.postcondition = Some(
        "verify the sandbox process stopped and inspect working-directory outputs".to_string(),
    );
    ToolResult::error(message).with_failure(failure)
}

fn sandbox_error_result(error: ReactError) -> ToolResult {
    let mut failure = ToolFailure::from_error(&error, true);
    if failure.category == ToolFailureCategory::Cancelled {
        failure.side_effect = ToolSideEffect::Possible;
        failure.postcondition = Some(
            "verify the sandbox process stopped and inspect working-directory outputs".to_string(),
        );
    }
    ToolResult::error(format!("Sandbox execution failed: {error}")).with_failure(failure)
}

fn tool_result_from_execution(
    mut execution: ExecutionResult,
    working_dir: Option<&std::path::PathBuf>,
    isolation_level: IsolationLevel,
    max_output_bytes: u64,
) -> ToolResult {
    execution.enforce_output_limit(max_output_bytes);
    let stdout_bytes = if execution.stdout_bytes == 0 {
        u64::try_from(execution.stdout.len()).unwrap_or(u64::MAX)
    } else {
        execution.stdout_bytes
    };
    let stderr_bytes = if execution.stderr_bytes == 0 {
        u64::try_from(execution.stderr.len()).unwrap_or(u64::MAX)
    } else {
        execution.stderr_bytes
    };
    let combined_output = execution.combined_output();

    let mut result = if execution.success() {
        ToolResult::success_with_kind(
            ToolResultKind::CommandOutput {
                exit_code: Some(execution.exit_code),
            },
            combined_output,
        )
    } else {
        let failure = if execution.cancelled {
            let mut failure = ToolFailure::new(ToolFailureCategory::Cancelled);
            failure.side_effect = ToolSideEffect::Possible;
            failure.postcondition = Some(
                "verify the sandbox process stopped and inspect working-directory outputs"
                    .to_string(),
            );
            failure
        } else if execution.timed_out {
            ToolFailure::new(ToolFailureCategory::Timeout)
                .with_side_effect(ToolSideEffect::Possible)
                .with_postcondition(
                    "verify the sandbox process stopped and inspect working-directory outputs before retrying",
                )
        } else {
            let mut failure = ToolFailure::new(ToolFailureCategory::Permanent);
            failure.side_effect = ToolSideEffect::Possible;
            failure.postcondition = Some(
                "inspect stdout, stderr, and working-directory outputs before continuing"
                    .to_string(),
            );
            failure
        };
        let message = if execution.cancelled {
            "Code execution cancelled".to_string()
        } else if execution.timed_out {
            "Code execution timed out".to_string()
        } else {
            format!(
                "Code execution failed with exit code {}",
                execution.exit_code
            )
        };
        ToolResult::error(message)
            .with_failure(failure)
            .with_output(combined_output)
    };

    result.kind = ToolResultKind::CommandOutput {
        exit_code: Some(execution.exit_code),
    };
    result.truncated = execution.output_truncated;
    result
        .with_meta("duration_ms", execution.duration.as_millis().to_string())
        .with_meta("exit_code", execution.exit_code.to_string())
        .with_meta("sandbox_type", execution.sandbox_type)
        .with_meta("isolation_level", isolation_level.to_string())
        .with_meta(
            "working_dir",
            working_dir
                .map(|dir| dir.display().to_string())
                .unwrap_or_default(),
        )
        .with_meta("timed_out", execution.timed_out.to_string())
        .with_meta("cancelled", execution.cancelled.to_string())
        .with_meta("output_truncated", execution.output_truncated.to_string())
        .with_meta("stdout_bytes", stdout_bytes.to_string())
        .with_meta("stderr_bytes", stderr_bytes.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::agent::CancellationToken;
    use echo_core::sandbox::CommandKind;
    use std::collections::HashMap;

    #[derive(Clone)]
    struct RecordedCall {
        command: SandboxCommand,
        limits: ResourceLimits,
    }

    struct RecordingSandbox {
        isolation: IsolationLevel,
        available: bool,
        result: ExecutionResult,
        seen: std::sync::Mutex<Option<RecordedCall>>,
    }

    impl SandboxExecutor for RecordingSandbox {
        fn name(&self) -> &str {
            "recording"
        }

        fn isolation_level(&self) -> IsolationLevel {
            self.isolation
        }

        fn is_available(&self) -> BoxFuture<'_, bool> {
            let available = self.available;
            Box::pin(async move { available })
        }

        fn execute(
            &self,
            _command: SandboxCommand,
        ) -> BoxFuture<'_, std::result::Result<ExecutionResult, ReactError>> {
            let result = self.result.clone();
            Box::pin(async move { Ok(result) })
        }

        fn execute_with_limits_and_cancel(
            &self,
            command: SandboxCommand,
            limits: ResourceLimits,
            _cancel: Option<Arc<CancellationToken>>,
        ) -> BoxFuture<'_, std::result::Result<ExecutionResult, ReactError>> {
            *self
                .seen
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                Some(RecordedCall { command, limits });
            let result = self.result.clone();
            Box::pin(async move { Ok(result) })
        }
    }

    fn success_execution(stdout: &str) -> ExecutionResult {
        ExecutionResult {
            exit_code: 0,
            stdout: stdout.to_string(),
            stderr: String::new(),
            duration: Duration::from_millis(1),
            sandbox_type: "recording".to_string(),
            timed_out: false,
            cancelled: false,
            output_truncated: false,
            stdout_bytes: u64::try_from(stdout.len()).unwrap_or(u64::MAX),
            stderr_bytes: 0,
        }
    }

    fn recording_sandbox(
        isolation: IsolationLevel,
        available: bool,
        result: ExecutionResult,
    ) -> Arc<RecordingSandbox> {
        Arc::new(RecordingSandbox {
            isolation,
            available,
            result,
            seen: std::sync::Mutex::new(None),
        })
    }

    fn code_params(language: &str, code: &str) -> HashMap<String, serde_json::Value> {
        [
            ("language".to_string(), serde_json::json!(language)),
            ("code".to_string(), serde_json::json!(code)),
        ]
        .into_iter()
        .collect()
    }

    fn script_params(language: &str, script_path: &str) -> HashMap<String, serde_json::Value> {
        [
            ("language".to_string(), serde_json::json!(language)),
            ("script_path".to_string(), serde_json::json!(script_path)),
        ]
        .into_iter()
        .collect()
    }

    #[test]
    fn validate_language_lowercases_and_accepts_known() -> Result<()> {
        assert_eq!(validate_language("Python")?, "python");
        assert_eq!(validate_language("PYTHON")?, "python");
        assert_eq!(validate_language("R")?, "r");
        assert_eq!(validate_language("JavaScript")?, "javascript");
        Ok(())
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

    #[tokio::test]
    async fn run_code_without_sandbox_fails_closed() -> Result<()> {
        let result = RunCodeTool::new()
            .execute_with_context(
                code_params("python", "print('unsafe')"),
                &ToolContext::default(),
            )
            .await?;
        assert!(!result.success);
        assert_eq!(
            result.failure.as_ref().map(|failure| failure.category),
            Some(ToolFailureCategory::Unavailable)
        );
        assert!(
            result
                .error
                .as_deref()
                .is_some_and(|error| error.contains("unsandboxed"))
        );
        Ok(())
    }

    #[tokio::test]
    async fn run_code_rejects_process_only_executor() -> Result<()> {
        let sandbox = recording_sandbox(
            IsolationLevel::Process,
            true,
            success_execution("should not run"),
        );
        let result = RunCodeTool::new()
            .with_sandbox(sandbox.clone())
            .execute_with_context(code_params("python", "print(1)"), &ToolContext::default())
            .await?;
        assert_eq!(
            result.failure.as_ref().map(|failure| failure.category),
            Some(ToolFailureCategory::Unavailable)
        );
        assert!(
            sandbox
                .seen
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_none()
        );
        Ok(())
    }

    #[tokio::test]
    async fn run_code_binds_working_dir_and_resource_limits() -> Result<()> {
        let sandbox = recording_sandbox(IsolationLevel::OsSandbox, true, success_execution("ok"));
        let captured: Arc<RecordingSandbox> = sandbox.clone();
        let tool = RunCodeTool::new().with_sandbox(sandbox);
        let working_dir = std::path::PathBuf::from("/tmp/eko-data-subagent-xyz");
        let ctx = ToolContext {
            working_dir: Some(working_dir.clone()),
            ..ToolContext::default()
        };
        let result = tool
            .execute_with_context(code_params("Python", "print(1+1)"), &ctx)
            .await?;
        assert!(result.success);

        let seen = captured
            .seen
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .ok_or_else(|| ReactError::Other("sandbox call was not recorded".to_string()))?;
        assert_eq!(seen.command.working_dir, Some(working_dir.clone()));
        assert_eq!(seen.command.timeout, Duration::from_secs(60));
        assert_eq!(seen.limits.cpu_time_secs, Some(60));
        assert_eq!(seen.limits.max_output_bytes, Some(DEFAULT_MAX_OUTPUT_BYTES));
        assert!(!seen.limits.network);
        assert!(seen.limits.writable_paths.contains(&working_dir));
        match seen.command.kind {
            CommandKind::Code { language, code } => {
                assert_eq!(language, "python");
                assert_eq!(code, "print(1+1)");
            }
            _ => {
                return Err(ReactError::Other(
                    "expected code sandbox command".to_string(),
                ));
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn run_code_executes_persisted_script_directly() -> Result<()> {
        let workspace = tempfile::tempdir()?;
        std::fs::write(workspace.path().join("analysis.py"), "print('reviewed')\n")?;
        let sandbox = recording_sandbox(IsolationLevel::OsSandbox, true, success_execution("ok"));
        let captured = sandbox.clone();
        let ctx = ToolContext {
            working_dir: Some(workspace.path().to_path_buf()),
            ..ToolContext::default()
        };

        let result = RunCodeTool::new()
            .with_sandbox(sandbox)
            .execute_with_context(script_params("python", "analysis.py"), &ctx)
            .await?;
        assert!(result.success);
        assert_eq!(
            result.metadata.get("execution_mode").map(String::as_str),
            Some("script")
        );
        assert_eq!(
            result.metadata.get("script_path").map(String::as_str),
            Some("analysis.py")
        );

        let seen = captured
            .seen
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .ok_or_else(|| ReactError::Other("sandbox call was not recorded".to_string()))?;
        match seen.command.kind {
            CommandKind::Program { program, args } => {
                assert_eq!(program, "python3");
                assert_eq!(args, vec!["analysis.py".to_string()]);
            }
            _ => {
                return Err(ReactError::Other(
                    "expected direct program sandbox command".to_string(),
                ));
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn run_code_rejects_ambiguous_or_escaping_script_inputs() -> Result<()> {
        let workspace = tempfile::tempdir()?;
        std::fs::write(workspace.path().join("analysis.py"), "print(1)\n")?;
        let ctx = ToolContext {
            working_dir: Some(workspace.path().to_path_buf()),
            ..ToolContext::default()
        };
        let tool = RunCodeTool::new();

        let mut ambiguous = script_params("python", "analysis.py");
        ambiguous.insert("code".to_string(), serde_json::json!("print(2)"));
        assert!(tool.execute_with_context(ambiguous, &ctx).await.is_err());
        assert!(
            tool.execute_with_context(script_params("python", "../analysis.py"), &ctx)
                .await
                .is_err()
        );
        assert!(
            tool.execute_with_context(script_params("python", "missing.py"), &ctx)
                .await
                .is_err()
        );
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_code_rejects_script_symlink_outside_working_dir() -> Result<()> {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir()?;
        let outside = tempfile::NamedTempFile::new()?;
        symlink(outside.path(), workspace.path().join("analysis.py"))?;
        let ctx = ToolContext {
            working_dir: Some(workspace.path().to_path_buf()),
            ..ToolContext::default()
        };

        assert!(
            RunCodeTool::new()
                .execute_with_context(script_params("python", "analysis.py"), &ctx)
                .await
                .is_err()
        );
        Ok(())
    }

    #[tokio::test]
    async fn run_code_preserves_timeout_and_cancellation_categories() -> Result<()> {
        let mut timed_out = success_execution("");
        timed_out.exit_code = -1;
        timed_out.timed_out = true;
        let timeout_result = RunCodeTool::new()
            .with_sandbox(recording_sandbox(
                IsolationLevel::OsSandbox,
                true,
                timed_out,
            ))
            .execute_with_context(code_params("r", "print(1)"), &ToolContext::default())
            .await?;
        assert_eq!(
            timeout_result
                .failure
                .as_ref()
                .map(|failure| failure.category),
            Some(ToolFailureCategory::Timeout)
        );

        let mut cancelled = success_execution("");
        cancelled.exit_code = -1;
        cancelled.cancelled = true;
        let cancelled_result = RunCodeTool::new()
            .with_sandbox(recording_sandbox(
                IsolationLevel::OsSandbox,
                true,
                cancelled,
            ))
            .execute_with_context(code_params("python", "print(1)"), &ToolContext::default())
            .await?;
        assert_eq!(
            cancelled_result
                .failure
                .as_ref()
                .map(|failure| failure.category),
            Some(ToolFailureCategory::Cancelled)
        );
        Ok(())
    }

    #[tokio::test]
    async fn run_code_applies_shared_utf8_output_limit() -> Result<()> {
        let result = RunCodeTool::new()
            .with_max_output_bytes(5)
            .with_sandbox(recording_sandbox(
                IsolationLevel::OsSandbox,
                true,
                success_execution("中文abc"),
            ))
            .execute_with_context(
                code_params("python", "print('中文abc')"),
                &ToolContext::default(),
            )
            .await?;
        assert!(result.success);
        assert_eq!(result.output, "中");
        assert!(result.truncated);
        assert_eq!(
            result.metadata.get("output_truncated").map(String::as_str),
            Some("true")
        );
        Ok(())
    }
}
