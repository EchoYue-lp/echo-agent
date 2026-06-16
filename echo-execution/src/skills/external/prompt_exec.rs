//! Inline shell execution and variable substitution for SKILL.md content.
//!
//! When a skill is activated, its Markdown body may contain:
//!
//! - **Inline commands**: `` !`pwd` `` -> replaced by the command's stdout
//! - **Block commands**: `` ```! \n git status \n ``` `` -> replaced by stdout
//! - **Variables**: `${SKILL_DIR}`, `${SESSION_ID}`, `${ARGUMENTS}`, `${1}`, `${2}`, etc.
//!
//! This mirrors the Claude Code `promptShellExecution.ts` behavior:
//! commands are executed **at activation time** and their output is substituted
//! inline, making skill instructions dynamic.
//!
//! ## Security
//!
//! - MCP-sourced skills **never** execute inline commands (untrusted remote content)
//! - Local skills use at minimum Process isolation level via SandboxManager
//! - Each command respects the configured timeout (default 10s per command)
//! - After timeout, subprocess is killed via `Child::kill()`
//! - Commands run in the skill's directory as working directory

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use tracing::warn;

use crate::sandbox::{SandboxCommand, SandboxManager};
use crate::skills::minimal_env;

const DEFAULT_CMD_TIMEOUT: Duration = Duration::from_secs(10);

/// Pre-compiled regex for block command syntax: ```! ... ```
static BLOCK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"```!\s*\n?([\s\S]*?)\n?```").expect("valid block regex"));

/// Pre-compiled regex for inline command syntax: !`...`
static INLINE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|\s)!`([^`]+)`").expect("valid inline regex"));

/// Pre-compiled regex for detecting block command markers.
static BLOCK_MARKER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"```!\s*\n?").expect("valid block marker regex"));

/// Pre-compiled regex for detecting inline command markers.
static INLINE_MARKER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|\s)!`[^`]*`").expect("valid inline marker regex"));

/// Pre-compiled regex for SOH-delimited placeholders.
static PLACEHOLDER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x01CMD_OUT_\d+\x01").expect("valid placeholder regex"));

/// Source of a skill (affects security policy).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillSource {
    /// Local filesystem skill (trusted).
    Local,
    /// Remote/MCP skill (untrusted -- no shell execution).
    Mcp,
}

/// Context for variable substitution and command execution.
#[derive(Debug, Clone)]
pub struct PromptContext {
    /// Absolute path to the skill's directory.
    pub skill_dir: String,
    /// Session identifier (if available).
    pub session_id: String,
    /// User-provided arguments (positional).
    pub arguments: Vec<String>,
    /// Preferred shell from frontmatter ("bash" or "powershell").
    pub shell: Option<String>,
    /// Per-command timeout.
    pub timeout: Duration,
    /// Skill source (local vs MCP).
    pub source: SkillSource,
    /// Optional sandbox manager for command execution.
    /// When Some, commands are routed through the sandbox.
    pub sandbox: Option<Arc<SandboxManager>>,
}

impl Default for PromptContext {
    fn default() -> Self {
        Self {
            skill_dir: String::new(),
            session_id: String::new(),
            arguments: Vec::new(),
            shell: None,
            timeout: DEFAULT_CMD_TIMEOUT,
            source: SkillSource::Local,
            sandbox: None,
        }
    }
}

/// Process a skill's Markdown body: substitute variables, then execute inline commands.
///
/// # Security
///
/// To prevent user-supplied arguments from injecting new commands:
/// 1. All command regions are extracted FIRST from the original content.
/// 2. Variable substitution runs only on non-command text (placeholders protect command
///    regions from both substitution and injection).
/// 3. After substitution, the result is scanned for any NEW command markers. If found,
///    substitution is rejected and the content is returned without command execution.
/// 4. Original commands are executed and their output replaces the placeholders.
pub async fn process_skill_content(content: &str, ctx: &PromptContext) -> String {
    // MCP skills: completely forbid execution of inline commands
    if ctx.source == SkillSource::Mcp {
        return substitute_variables(content, ctx);
    }

    // Step 1: Extract all command regions from ORIGINAL content (before substitution)
    let regions = extract_command_regions(content);

    // Step 2: Build a safe version with unique placeholders instead of command regions
    let mut safe = String::with_capacity(content.len());
    let mut pos = 0;
    for (i, region) in regions.iter().enumerate() {
        safe.push_str(&content[pos..region.start]);
        safe.push_str(&format!("\x01CMD_OUT_{}\x01", i)); // SOH-delimited placeholder
        pos = region.end;
    }
    safe.push_str(&content[pos..]);

    // Step 3: Substitute variables only in the safe (non-command) content
    let substituted = substitute_variables(&safe, ctx);

    // Step 4: Detect injected command markers introduced by variable expansion
    if has_command_markers(&substituted) {
        warn!(
            "Possible command injection detected after variable substitution — \
               command execution disabled for this skill activation"
        );
        // Return substituted content with placeholders removed (no command execution)
        return strip_placeholders(&substituted);
    }

    // Step 5: Execute original commands and collect outputs
    let mut cmd_outputs: HashMap<usize, String> = HashMap::new();
    for (i, region) in regions.iter().enumerate() {
        let output = run_command(&region.command_text, ctx).await;
        cmd_outputs.insert(i, output);
    }

    // Step 6: Replace placeholders with command outputs
    let mut result = substituted;
    for i in (0..regions.len()).rev() {
        let placeholder = format!("\x01CMD_OUT_{}\x01", i);
        if let Some(output) = cmd_outputs.remove(&i) {
            if regions[i].is_block {
                // Block commands: output replaces the entire ```! ... ``` block
                result = result.replace(&placeholder, &output);
            } else {
                // Inline commands: preserve leading whitespace from the original match
                let matched = &content[regions[i].start..regions[i].end];
                let leading_ws: String =
                    matched.chars().take_while(|c| c.is_whitespace()).collect();
                result = result.replace(&placeholder, &format!("{}{}", leading_ws, output.trim()));
            }
        }
    }

    result
}

/// A detected command region in skill content.
struct CmdRegion {
    start: usize,
    end: usize,
    command_text: String,
    is_block: bool,
}

/// Extract all command regions (block and inline) from content, in order.
fn extract_command_regions(content: &str) -> Vec<CmdRegion> {
    let mut regions: Vec<CmdRegion> = Vec::new();

    for cap in BLOCK_RE.captures_iter(content) {
        let m = cap.get(0).unwrap();
        regions.push(CmdRegion {
            start: m.start(),
            end: m.end(),
            command_text: cap
                .get(1)
                .map(|m| m.as_str().trim())
                .unwrap_or("")
                .to_string(),
            is_block: true,
        });
    }
    for cap in INLINE_RE.captures_iter(content) {
        let m = cap.get(0).unwrap();
        regions.push(CmdRegion {
            start: m.start(),
            end: m.end(),
            command_text: cap
                .get(1)
                .map(|m| m.as_str().trim())
                .unwrap_or("")
                .to_string(),
            is_block: false,
        });
    }

    regions.sort_by_key(|r| r.start);
    regions
}

/// Check if content contains block or inline command markers.
fn has_command_markers(content: &str) -> bool {
    BLOCK_MARKER_RE.is_match(content) || INLINE_MARKER_RE.is_match(content)
}

/// Remove placeholder markers from content.
fn strip_placeholders(content: &str) -> String {
    PLACEHOLDER_RE.replace_all(content, "").to_string()
}

/// Substitute template variables in skill content.
///
/// Supported variables:
/// - `${SKILL_DIR}` -- absolute path to the skill directory
/// - `${SESSION_ID}` -- current session identifier
/// - `${ARGUMENTS}` -- all arguments joined by space
/// - `${1}`, `${2}`, ... -- positional arguments
fn substitute_variables(content: &str, ctx: &PromptContext) -> String {
    let mut result = content.to_string();

    result = result.replace("${SKILL_DIR}", &ctx.skill_dir);
    result = result.replace("${CLAUDE_SKILL_DIR}", &ctx.skill_dir);

    result = result.replace("${SESSION_ID}", &ctx.session_id);
    result = result.replace("${CLAUDE_SESSION_ID}", &ctx.session_id);

    let args_joined = ctx.arguments.join(" ");
    result = result.replace("${ARGUMENTS}", &args_joined);

    // Positional: ${1}, ${2}, ...
    for (i, arg) in ctx.arguments.iter().enumerate() {
        let placeholder = format!("${{{}}}", i + 1);
        result = result.replace(&placeholder, arg);
    }

    result
}

/// Execute block commands: ` ```! \n command \n ``` ` -> stdout
#[cfg(test)]
async fn execute_block_commands(content: &str, ctx: &PromptContext) -> String {
    // Pattern: ```! followed by optional newline, then command(s), then ```
    let mut result = content.to_string();
    let matches: Vec<_> = BLOCK_RE.captures_iter(content).collect();

    // Process in reverse order so byte offsets remain valid
    for cap in matches.into_iter().rev() {
        let full_match = match cap.get(0) {
            Some(m) => m,
            None => continue,
        };
        let command = cap.get(1).map(|m| m.as_str().trim()).unwrap_or("");

        if command.is_empty() {
            continue;
        }

        let output = run_command(command, ctx).await;

        result.replace_range(full_match.range(), &output);
    }

    result
}

/// Execute inline commands: `` !`command` `` -> stdout
///
/// The pattern requires whitespace or line start before `!`` to avoid
/// matching Markdown image syntax (`![alt](url)`).
#[cfg(test)]
async fn execute_inline_commands(content: &str, ctx: &PromptContext) -> String {
    let mut result = content.to_string();
    let matches: Vec<_> = INLINE_RE.captures_iter(content).collect();

    for cap in matches.into_iter().rev() {
        let full_match = match cap.get(0) {
            Some(m) => m,
            None => continue,
        };
        let command = cap.get(1).map(|m| m.as_str().trim()).unwrap_or("");

        if command.is_empty() {
            continue;
        }

        let output = run_command(command, ctx).await;

        // Preserve leading whitespace from the match
        let matched_str = full_match.as_str();
        let leading_ws: String = matched_str
            .chars()
            .take_while(|c| c.is_whitespace())
            .collect();
        let replacement = format!("{}{}", leading_ws, output.trim());

        result.replace_range(full_match.range(), &replacement);
    }

    result
}

/// Execute a shell command and return its stdout (or error message).
///
/// Sandbox routing:
/// - If a SandboxManager is configured, commands go through the sandbox
///   with at minimum Process isolation level.
/// - If no sandbox, falls back to bare process spawning with minimal env
///   and proper timeout + kill behavior.
async fn run_command(command: &str, ctx: &PromptContext) -> String {
    // Sandbox execution path
    if let Some(ref manager) = ctx.sandbox {
        return run_command_sandboxed(command, ctx, manager).await;
    }

    // Fallback: direct process execution with minimal env and proper kill
    run_command_direct(command, ctx).await
}

/// Execute a command through the SandboxManager.
async fn run_command_sandboxed(
    command: &str,
    ctx: &PromptContext,
    manager: &SandboxManager,
) -> String {
    // Use minimal environment
    let env = minimal_env(&ctx.skill_dir, &ctx.session_id, HashMap::new());

    let mut sandbox_cmd = SandboxCommand::shell(command).with_timeout(ctx.timeout);

    if !ctx.skill_dir.is_empty() && Path::new(&ctx.skill_dir).exists() {
        sandbox_cmd = sandbox_cmd.with_working_dir(&ctx.skill_dir);
    }

    for (k, v) in env {
        sandbox_cmd = sandbox_cmd.with_env(k, v);
    }

    match manager.execute(sandbox_cmd).await {
        Ok(result) => {
            if result.success() {
                result.stdout.trim().to_string()
            } else {
                let stderr = result.stderr.trim();
                if !stderr.is_empty() {
                    warn!(
                        command = command,
                        exit_code = result.exit_code,
                        stderr = %stderr,
                        "Inline skill command failed (sandboxed)"
                    );
                    format!("[error: {}]", stderr)
                } else {
                    format!("[error: exit code {}]", result.exit_code)
                }
            }
        }
        Err(e) => {
            warn!(command = command, error = %e, "Inline skill command sandbox error");
            format!("[sandbox error: {}]", e)
        }
    }
}

/// Execute a command directly with minimal environment.
///
/// # ⚠️ 警告：Fallback 路径的限制
///
/// 此函数是 `run_command` 的 fallback 路径，仅在无 `SandboxManager` 时使用。
/// 它有以下限制：
///
/// direct fallback 会使用 `kill_on_drop(true)` 配合 `timeout(...)` 尽力终止超时子进程，
/// 但生产环境仍建议优先配置 `SandboxManager`（通过 `PromptContext::sandbox` 字段），
/// 以获得更强的隔离、资源限制和统一清理。
///
/// **适用场景**：仅建议用于简单 demo、测试、或无沙箱依赖的开发环境。
///
/// **推荐做法**：在 `PromptContext` 中设置 `sandbox = Some(Arc::new(SandboxManager::auto_detect()))`
/// 以启用沙箱执行路径，确保超时后进程被正确终止。
async fn run_command_direct(command: &str, ctx: &PromptContext) -> String {
    // Security gate: a skill's shell block is arbitrary code sourced from a
    // SKILL.md file. When running without a sandbox (the default), validate it
    // against the same strict policy the ShellTool uses, so a local skill can't
    // use `!`-blocks to run `rm -rf` or other destructive commands. Skills
    // needing such commands should be routed through a sandbox instead.
    if let Err(reason) = check_skill_command_safety(command) {
        warn!(command = command, reason = %reason, "Skill shell command rejected by safety policy");
        return format!("[error: command rejected by safety policy: {reason}]");
    }

    let shell_cmd = build_shell_command(command, ctx);

    let mut cmd = tokio::process::Command::new(&shell_cmd.program);
    for arg in &shell_cmd.args {
        cmd.arg(arg);
    }
    cmd.kill_on_drop(true);

    if !ctx.skill_dir.is_empty() && Path::new(&ctx.skill_dir).exists() {
        cmd.current_dir(&ctx.skill_dir);
    }

    // Use minimal environment instead of inheriting everything
    set_minimal_cmd_env(&mut cmd, ctx);

    // Use output() to capture stdout/stderr
    match tokio::time::timeout(ctx.timeout, cmd.output()).await {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();

            if !output.status.success() {
                warn!(
                    command = command,
                    exit_code = output.status.code(),
                    stderr = %stderr.trim(),
                    "Inline skill command failed"
                );
                if !stderr.is_empty() {
                    format!("[error: {}]", stderr.trim())
                } else {
                    format!("[error: exit code {}]", output.status.code().unwrap_or(-1))
                }
            } else {
                stdout.trim().to_string()
            }
        }
        Ok(Err(e)) => {
            warn!(command = command, error = %e, "Inline skill command execution error");
            format!("[error: {}]", e)
        }
        Err(_) => {
            warn!(
                command = command,
                timeout_secs = ctx.timeout.as_secs(),
                "Inline skill command timed out"
            );
            format!("[timeout after {}s]", ctx.timeout.as_secs())
        }
    }
}

struct ShellCommand {
    program: String,
    args: Vec<String>,
}

fn build_shell_command(command: &str, ctx: &PromptContext) -> ShellCommand {
    let shell_pref = ctx.shell.as_deref().unwrap_or("bash");

    if shell_pref == "powershell" {
        let program = if which_exists("pwsh") {
            "pwsh"
        } else if cfg!(target_os = "windows") {
            "powershell"
        } else {
            "sh"
        };

        if program == "powershell" || program == "pwsh" {
            ShellCommand {
                program: program.to_string(),
                args: vec![
                    "-NoProfile".to_string(),
                    "-NonInteractive".to_string(),
                    "-Command".to_string(),
                    command.to_string(),
                ],
            }
        } else {
            ShellCommand {
                program: "sh".to_string(),
                args: vec!["-c".to_string(), command.to_string()],
            }
        }
    } else if cfg!(target_os = "windows") {
        // Windows: try Git Bash first
        if let Some(bash) = find_git_bash_path() {
            ShellCommand {
                program: bash,
                args: vec!["-c".to_string(), command.to_string()],
            }
        } else {
            ShellCommand {
                program: "cmd".to_string(),
                args: vec!["/C".to_string(), command.to_string()],
            }
        }
    } else {
        ShellCommand {
            program: "bash".to_string(),
            args: vec!["-c".to_string(), command.to_string()],
        }
    }
}

fn which_exists(cmd: &str) -> bool {
    std::process::Command::new(cmd)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(target_os = "windows")]
fn find_git_bash_path() -> Option<String> {
    let candidates = [
        std::env::var("ProgramFiles")
            .ok()
            .map(|p| format!("{}\\Git\\bin\\bash.exe", p)),
        Some(r"C:\Program Files\Git\bin\bash.exe".to_string()),
    ];
    for candidate in candidates.into_iter().flatten() {
        if std::path::Path::new(&candidate).exists() {
            return Some(candidate);
        }
    }
    if which_exists("bash") {
        return Some("bash".to_string());
    }
    None
}

#[cfg(not(target_os = "windows"))]
pub fn find_git_bash_path() -> Option<String> {
    None
}

/// Set minimal environment variables for a subprocess.
/// Only passes cleaned PATH, SKILL_DIR, and SESSION_ID.
fn set_minimal_cmd_env(cmd: &mut tokio::process::Command, ctx: &PromptContext) {
    cmd.env_clear();
    if let Ok(path) = std::env::var("PATH") {
        cmd.env("PATH", path);
    }
    if !ctx.skill_dir.is_empty() {
        cmd.env("SKILL_DIR", &ctx.skill_dir);
    }
    if !ctx.session_id.is_empty() {
        cmd.env("SESSION_ID", &ctx.session_id);
    }
}

/// Check whether a skill's shell command may run outside a sandbox.
///
/// Skill `!`-blocks are arbitrary code sourced from a SKILL.md file (local or,
/// in the future, fetched). When no sandbox is configured this validates the
/// command against the strict default policy — the same gate `ShellTool` and
/// `spawn_background_task` use — so a skill cannot use an inline shell block to
/// run destructive commands (`rm -rf`, arbitrary metacharacters, etc.). Skills
/// that genuinely need such commands must be routed through a sandbox.
///
/// Returns `Ok(())` if the command is safe, or `Err(reason)` describing the
/// rejection.
fn check_skill_command_safety(command: &str) -> std::result::Result<(), String> {
    #[cfg(feature = "shell")]
    {
        use echo_tools::shell::{CommandSafety, validate_command_safety};
        return match validate_command_safety(command) {
            CommandSafety::Safe => Ok(()),
            CommandSafety::RequiresApproval(reason) | CommandSafety::Dangerous(reason) => {
                Err(reason)
            }
        };
    }

    // Fallback when the `shell` feature is disabled: a conservative inline gate.
    #[allow(unreachable_code)]
    {
        const SHELL_META: &[char] = &['|', ';', '&', '$', '`', '>', '<', '(', ')', '\n', '\r'];
        if command.chars().any(|c| SHELL_META.contains(&c)) {
            return Err(
                "Skill command contains shell metacharacters; sandbox execution required. \
                 Enable the `shell` feature for the full safety classifier."
                    .to_string(),
            );
        }
        let base = command.split_whitespace().next().unwrap_or("");
        let base = base.rsplit(['/', '\\']).next().unwrap_or(base);
        if matches!(
            base,
            "rm" | "rmdir" | "mkfs" | "dd" | "shutdown" | "reboot" | "halt" | "poweroff"
        ) {
            return Err(format!(
                "Skill command '{base}' is destructive; sandbox execution required"
            ));
        }
        Ok(())
    }
}

// -- Tests --

#[cfg(test)]
mod tests {
    use super::*;

    fn test_ctx() -> PromptContext {
        PromptContext {
            skill_dir: "/tmp/test-skill".into(),
            session_id: "session-abc123".into(),
            arguments: vec!["arg1".into(), "arg2".into()],
            shell: None,
            timeout: Duration::from_secs(5),
            source: SkillSource::Local,
            sandbox: None,
        }
    }

    #[test]
    fn test_substitute_variables() {
        let ctx = test_ctx();
        let content = "Dir: ${SKILL_DIR}\nSession: ${SESSION_ID}\nArgs: ${ARGUMENTS}\nFirst: ${1}";
        let result = substitute_variables(content, &ctx);
        assert_eq!(
            result,
            "Dir: /tmp/test-skill\nSession: session-abc123\nArgs: arg1 arg2\nFirst: arg1"
        );
    }

    #[test]
    fn test_substitute_claude_compat_vars() {
        let ctx = test_ctx();
        let content = "${CLAUDE_SKILL_DIR}/scripts/run.py ${CLAUDE_SESSION_ID}";
        let result = substitute_variables(content, &ctx);
        assert_eq!(result, "/tmp/test-skill/scripts/run.py session-abc123");
    }

    #[test]
    fn test_substitute_no_args() {
        let ctx = PromptContext {
            arguments: vec![],
            ..test_ctx()
        };
        let content = "No args: ${ARGUMENTS} and ${1}";
        let result = substitute_variables(content, &ctx);
        assert_eq!(result, "No args:  and ${1}");
    }

    #[tokio::test]
    async fn test_block_command_execution() {
        if cfg!(target_os = "windows") {
            return;
        }
        let ctx = test_ctx();
        let content = "Before\n```!\necho hello-world\n```\nAfter";
        let result = execute_block_commands(content, &ctx).await;
        assert!(result.contains("hello-world"), "got: {}", result);
        assert!(result.contains("Before"));
        assert!(result.contains("After"));
        assert!(!result.contains("```!"));
    }

    #[tokio::test]
    async fn test_inline_command_execution() {
        if cfg!(target_os = "windows") {
            return;
        }
        let ctx = test_ctx();
        let content = "Current dir is !`echo /test/path`";
        let result = execute_inline_commands(content, &ctx).await;
        assert!(result.contains("/test/path"), "got: {}", result);
        assert!(!result.contains("!`"));
    }

    #[tokio::test]
    async fn test_mcp_source_skips_execution() {
        let ctx = PromptContext {
            source: SkillSource::Mcp,
            ..test_ctx()
        };
        let content = "Run !`echo dangerous` here";
        let result = process_skill_content(content, &ctx).await;
        assert!(
            result.contains("!`echo dangerous`"),
            "MCP skill should not execute commands: {}",
            result
        );
    }

    #[tokio::test]
    async fn test_full_processing() {
        if cfg!(target_os = "windows") {
            return;
        }
        let ctx = test_ctx();
        let content = "Skill at ${SKILL_DIR}\nVersion: !`echo 1.0.0`";
        let result = process_skill_content(content, &ctx).await;
        assert!(result.contains("/tmp/test-skill"));
        assert!(result.contains("1.0.0"));
    }

    #[tokio::test]
    async fn test_inline_command_does_not_inherit_unlisted_env() {
        if cfg!(target_os = "windows") {
            return;
        }

        let key = "ECHO_EXECUTION_TEST_SECRET";
        let secret = "super-secret-value";
        // Test-only process env mutation for validating env_clear() behavior.
        unsafe {
            std::env::set_var(key, secret);
        }

        let ctx = test_ctx();
        let content = format!("secret=!`printf %s \"${key}\"`");
        let result = execute_inline_commands(&content, &ctx).await;

        assert!(
            !result.contains(secret),
            "unexpected leaked env in: {}",
            result
        );

        unsafe {
            std::env::remove_var(key);
        }
    }

    // ── Injection prevention tests ──────────────────────────────────────────

    #[tokio::test]
    async fn test_injection_via_arguments_inline() {
        if cfg!(target_os = "windows") {
            return;
        }
        // User argument containing inline command syntax should NOT be executed
        let ctx = PromptContext {
            arguments: vec!["!`echo injected`".into()],
            ..test_ctx()
        };
        let content = "Args: ${ARGUMENTS}";
        let result = process_skill_content(content, &ctx).await;
        // The full command syntax must remain literal (not executed and replaced by output)
        assert!(
            result.contains("!`echo injected`"),
            "Injected inline command should remain as literal text, got: {}",
            result
        );
    }

    #[tokio::test]
    async fn test_injection_via_arguments_block() {
        if cfg!(target_os = "windows") {
            return;
        }
        // User argument containing block command syntax should NOT be executed
        let ctx = PromptContext {
            arguments: vec!["```!\necho injected\n```".into()],
            ..test_ctx()
        };
        let content = "Args: ${ARGUMENTS}";
        let result = process_skill_content(content, &ctx).await;
        // The block command markers must remain as literal text
        assert!(
            result.contains("```!"),
            "Injected block command marker should remain literal, got: {}",
            result
        );
    }

    #[tokio::test]
    async fn test_injection_via_positional_arg() {
        if cfg!(target_os = "windows") {
            return;
        }
        let ctx = PromptContext {
            arguments: vec!["!`touch /tmp/evil`".into()],
            ..test_ctx()
        };
        let content = "Arg1: ${1}";
        let result = process_skill_content(content, &ctx).await;
        assert!(
            result.contains("!`touch /tmp/evil`"),
            "Injected command should remain literal, got: {}",
            result
        );
    }

    #[tokio::test]
    async fn test_variables_still_expand_in_text_regions() {
        if cfg!(target_os = "windows") {
            return;
        }
        let ctx = PromptContext {
            arguments: vec!["world".into()],
            ..test_ctx()
        };
        // Command is in the original content, variables in text region
        let content = "Hello ${1}! Status: !`echo ok`";
        let result = process_skill_content(content, &ctx).await;
        assert!(
            result.contains("Hello world!"),
            "Variable should expand: {}",
            result
        );
        assert!(
            result.contains("ok"),
            "Command should still execute: {}",
            result
        );
    }

    #[tokio::test]
    async fn test_injection_safe_argument_no_command() {
        // Safe argument should work normally
        let ctx = PromptContext {
            arguments: vec!["safe-value".into()],
            ..test_ctx()
        };
        let content = "Arg: ${1}";
        let result = process_skill_content(content, &ctx).await;
        assert_eq!(result, "Arg: safe-value");
    }
}
