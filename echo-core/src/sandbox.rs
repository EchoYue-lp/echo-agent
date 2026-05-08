//! Sandbox executor core trait and types
//!
//! Defines the [`SandboxExecutor`] trait and its parameter/result types.
//! The trait is implemented by `LocalSandbox`, `DockerSandbox`, and `K8sSandbox`
//! in the `echo-execution` crate.

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use crate::error::Result;

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

    /// Execute a command with resource limits
    fn execute_with_limits(
        &self,
        command: SandboxCommand,
        limits: ResourceLimits,
    ) -> BoxFuture<'_, Result<ExecutionResult>> {
        let _ = limits;
        self.execute(command)
    }

    /// Clean up sandbox resources (containers, temp files, etc.)
    fn cleanup(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async { Ok(()) })
    }
}

/// Isolation level
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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
}

impl ExecutionResult {
    /// Whether execution was successful
    pub fn success(&self) -> bool {
        self.exit_code == 0 && !self.timed_out
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
