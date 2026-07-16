//! Sandbox executor core trait and types
//!
//! Defines the [`SandboxExecutor`] trait and its parameter/result types.
//! The trait is implemented by `LocalSandbox`, `DockerSandbox`, and `K8sSandbox`
//! in the `echo-execution` crate.

use futures::future::BoxFuture;
use futures::stream::{self, Stream};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use crate::error::{ReactError, Result, SandboxError};

/// Sandbox executor unified interface.
///
/// All three layers (Local / Docker / K8s) implement this trait.
pub trait SandboxExecutor: Send + Sync {
    /// Executor name
    fn name(&self) -> &str;

    /// Current isolation level
    fn isolation_level(&self) -> IsolationLevel;

    /// Check if the executor is available
    fn is_available(&self) -> BoxFuture<'_, bool>;

    /// Execute a command
    fn execute(&self, command: SandboxCommand) -> BoxFuture<'_, Result<ExecutionResult>>;

    /// Execute a command as an event stream.
    ///
    /// Executors without live pipe support degrade explicitly to one buffered
    /// [`SandboxStreamEvent::Complete`] event. Callers can inspect
    /// [`Self::supports_streaming`] before describing output as live.
    fn execute_stream<'a>(
        &'a self,
        command: SandboxCommand,
    ) -> BoxFuture<'a, Result<Pin<Box<dyn Stream<Item = SandboxStreamEvent> + Send + 'a>>>> {
        Box::pin(async move {
            let result = self.execute(command).await?;
            let events: Pin<Box<dyn Stream<Item = SandboxStreamEvent> + Send + 'a>> = Box::pin(
                stream::once(async move { SandboxStreamEvent::Complete(result) }),
            );
            Ok(events)
        })
    }

    /// Whether [`Self::execute_stream`] emits output before completion.
    fn supports_streaming(&self) -> bool {
        false
    }

    /// Execute a command with resource limits
    fn execute_with_limits(
        &self,
        command: SandboxCommand,
        limits: ResourceLimits,
    ) -> BoxFuture<'_, Result<ExecutionResult>> {
        let _ = limits;
        self.execute(command)
    }

    /// Execute with resource limits and an optional owning-run cancellation token.
    ///
    /// Concrete backends should override this when cancellation needs backend-
    /// specific cleanup. The default drops the execution future and reports a
    /// typed cancellation instead of silently treating it as a timeout.
    fn execute_with_limits_and_cancel(
        &self,
        command: SandboxCommand,
        limits: ResourceLimits,
        cancel: Option<Arc<CancellationToken>>,
    ) -> BoxFuture<'_, Result<ExecutionResult>> {
        Box::pin(async move {
            let Some(cancel) = cancel else {
                return self.execute_with_limits(command, limits).await;
            };
            tokio::select! {
                _ = cancel.cancelled() => Err(ReactError::Sandbox(Box::new(
                    SandboxError::Cancelled("owning run was cancelled".to_string()),
                ))),
                result = self.execute_with_limits(command, limits) => result,
            }
        })
    }

    /// Clean up sandbox resources (containers, temp files, etc.)
    fn cleanup(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async { Ok(()) })
    }
}

/// Pipe channel for sandbox output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxOutputChannel {
    Stdout,
    Stderr,
}

/// Incremental sandbox execution event.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event_type", rename_all = "snake_case")]
pub enum SandboxStreamEvent {
    Output {
        channel: SandboxOutputChannel,
        chunk: String,
    },
    Complete(ExecutionResult),
}

/// Isolation level
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IsolationLevel {
    /// No isolation (direct host execution)
    None = 0,
    /// Process-level isolation (resource limits, path restrictions)
    Process = 1,
    /// OS sandbox isolation (bubblewrap / sandbox-exec / seccomp)
    OsSandbox = 2,
    /// Container isolation (Docker / Podman)
    Container = 3,
    /// Orchestration workload isolation (e.g. ephemeral K8s Pod)
    Orchestrated = 4,
}

impl std::fmt::Display for IsolationLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IsolationLevel::None => write!(f, "none"),
            IsolationLevel::Process => write!(f, "process"),
            IsolationLevel::OsSandbox => write!(f, "os-sandbox"),
            IsolationLevel::Container => write!(f, "container"),
            IsolationLevel::Orchestrated => write!(f, "orchestrated"),
        }
    }
}

/// Sandbox execution command
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxCommand {
    /// Command type
    pub kind: CommandKind,
    /// Minimum isolation required by the caller for this command.
    #[serde(default)]
    pub minimum_isolation: Option<IsolationLevel>,
    /// Working directory
    pub working_dir: Option<PathBuf>,
    /// Environment variables
    pub env: HashMap<String, String>,
    /// Execution timeout
    pub timeout: Duration,
    /// Standard input
    pub stdin: Option<String>,
}

/// Command type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CommandKind {
    /// Shell command (executed via sh -c / cmd /C)
    Shell(String),
    /// Direct program execution
    Program { program: String, args: Vec<String> },
    /// Execute code snippet (requires language runtime)
    Code { language: String, code: String },
}

impl SandboxCommand {
    /// Create a shell command
    pub fn shell(cmd: impl Into<String>) -> Self {
        Self {
            kind: CommandKind::Shell(cmd.into()),
            minimum_isolation: None,
            working_dir: None,
            env: HashMap::new(),
            timeout: Duration::from_secs(30),
            stdin: None,
        }
    }

    /// Create a program execution command
    pub fn program(program: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            kind: CommandKind::Program {
                program: program.into(),
                args,
            },
            minimum_isolation: None,
            working_dir: None,
            env: HashMap::new(),
            timeout: Duration::from_secs(30),
            stdin: None,
        }
    }

    /// Create a code execution command
    pub fn code(language: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            kind: CommandKind::Code {
                language: language.into(),
                code: code.into(),
            },
            minimum_isolation: None,
            working_dir: None,
            env: HashMap::new(),
            timeout: Duration::from_secs(30),
            stdin: None,
        }
    }

    /// Set working directory
    pub fn with_working_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.working_dir = Some(dir.into());
        self
    }

    /// Set timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Require at least this isolation level regardless of manager policy.
    pub fn with_minimum_isolation(mut self, isolation: IsolationLevel) -> Self {
        self.minimum_isolation = Some(isolation);
        self
    }

    /// Add environment variable
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    /// Set standard input
    pub fn with_stdin(mut self, stdin: impl Into<String>) -> Self {
        self.stdin = Some(stdin.into());
        self
    }
}

/// Execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    /// Exit code (0 = success)
    pub exit_code: i32,
    /// Standard output
    pub stdout: String,
    /// Standard error
    pub stderr: String,
    /// Execution duration
    pub duration: Duration,
    /// Sandbox type used
    pub sandbox_type: String,
    /// Whether the execution was terminated due to timeout
    pub timed_out: bool,
    /// Whether the execution was terminated by an owning-run cancellation.
    #[serde(default)]
    pub cancelled: bool,
    /// Whether retained output exceeded the configured memory cap.
    #[serde(default)]
    pub output_truncated: bool,
    /// Total stdout bytes observed before truncation.
    #[serde(default)]
    pub stdout_bytes: u64,
    /// Total stderr bytes observed before truncation.
    #[serde(default)]
    pub stderr_bytes: u64,
}

impl ExecutionResult {
    /// Whether execution was successful
    pub fn success(&self) -> bool {
        self.exit_code == 0 && !self.timed_out && !self.cancelled
    }

    /// Combined stdout + stderr output
    pub fn combined_output(&self) -> String {
        if self.stderr.is_empty() {
            self.stdout.clone()
        } else if self.stdout.is_empty() {
            self.stderr.clone()
        } else {
            format!("{}\n{}", self.stdout, self.stderr)
        }
    }

    /// Bound retained stdout + stderr to a shared UTF-8-safe byte budget.
    pub fn enforce_output_limit(&mut self, max_output_bytes: u64) {
        let max_output_bytes = usize::try_from(max_output_bytes).unwrap_or(usize::MAX);
        let original_stdout_bytes = self.stdout.len();
        let original_stderr_bytes = self.stderr.len();

        self.stdout = retain_utf8_prefix(&self.stdout, max_output_bytes);
        let remaining = max_output_bytes.saturating_sub(self.stdout.len());
        self.stderr = retain_utf8_prefix(&self.stderr, remaining);

        self.output_truncated |=
            self.stdout.len() < original_stdout_bytes || self.stderr.len() < original_stderr_bytes;
    }
}

fn retain_utf8_prefix(value: &str, max_bytes: usize) -> String {
    let mut retained_bytes = 0_usize;
    value
        .chars()
        .take_while(|character| {
            let next = retained_bytes.saturating_add(character.len_utf8());
            if next > max_bytes {
                return false;
            }
            retained_bytes = next;
            true
        })
        .collect()
}

/// Resource limits
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    /// Maximum execution time (seconds, treated as wall-clock timeout)
    pub cpu_time_secs: Option<u64>,
    /// Maximum memory (bytes)
    pub memory_bytes: Option<u64>,
    /// Maximum output size (bytes)
    pub max_output_bytes: Option<u64>,
    /// Maximum number of processes
    pub max_processes: Option<u32>,
    /// Whether networking is allowed
    pub network: bool,
    /// Allowed mount paths (read-only)
    pub read_only_paths: Vec<PathBuf>,
    /// Allowed mount paths (read-write)
    pub writable_paths: Vec<PathBuf>,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            cpu_time_secs: Some(30),
            memory_bytes: Some(256 * 1024 * 1024), // 256 MB
            max_output_bytes: Some(1024 * 1024),   // 1 MB
            max_processes: Some(64),
            network: false,
            read_only_paths: vec![],
            writable_paths: vec![],
        }
    }
}

impl ResourceLimits {
    /// No limits (trusted environments only)
    pub fn unrestricted() -> Self {
        Self {
            cpu_time_secs: None,
            memory_bytes: None,
            max_output_bytes: None,
            max_processes: None,
            network: true,
            read_only_paths: vec![],
            writable_paths: vec![],
        }
    }

    /// Strict limits (suitable for untrusted code)
    pub fn strict() -> Self {
        Self {
            cpu_time_secs: Some(10),
            memory_bytes: Some(64 * 1024 * 1024), // 64 MB
            max_output_bytes: Some(256 * 1024),   // 256 KB
            max_processes: Some(8),
            network: false,
            read_only_paths: vec![],
            writable_paths: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct PendingExecutor;

    impl SandboxExecutor for PendingExecutor {
        fn name(&self) -> &str {
            "pending"
        }

        fn isolation_level(&self) -> IsolationLevel {
            IsolationLevel::OsSandbox
        }

        fn is_available(&self) -> BoxFuture<'_, bool> {
            Box::pin(async { true })
        }

        fn execute(&self, _command: SandboxCommand) -> BoxFuture<'_, Result<ExecutionResult>> {
            Box::pin(std::future::pending())
        }
    }

    fn execution_result(stdout: &str, stderr: &str) -> ExecutionResult {
        ExecutionResult {
            exit_code: 0,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            duration: Duration::ZERO,
            sandbox_type: "test".to_string(),
            timed_out: false,
            cancelled: false,
            output_truncated: false,
            stdout_bytes: u64::try_from(stdout.len()).unwrap_or(u64::MAX),
            stderr_bytes: u64::try_from(stderr.len()).unwrap_or(u64::MAX),
        }
    }

    #[test]
    fn output_limit_is_shared_and_utf8_safe() {
        let mut result = execution_result("中文", "abc");
        result.enforce_output_limit(7);
        assert_eq!(result.stdout, "中文");
        assert_eq!(result.stderr, "a");
        assert!(result.output_truncated);
    }

    #[test]
    fn cancelled_execution_is_not_successful() {
        let mut result = execution_result("", "");
        result.cancelled = true;
        assert!(!result.success());
    }

    #[tokio::test]
    async fn default_controlled_execution_reports_cancellation() -> Result<()> {
        let cancel = Arc::new(CancellationToken::new());
        cancel.cancel();
        let result = PendingExecutor
            .execute_with_limits_and_cancel(
                SandboxCommand::shell("ignored"),
                ResourceLimits::default(),
                Some(cancel),
            )
            .await;
        match result {
            Err(ReactError::Sandbox(error)) if matches!(*error, SandboxError::Cancelled(_)) => {
                Ok(())
            }
            other => Err(ReactError::Other(format!(
                "expected sandbox cancellation, got {other:?}"
            ))),
        }
    }
}
