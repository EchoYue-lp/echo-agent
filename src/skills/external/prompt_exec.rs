//! Inline shell execution and variable substitution for SKILL.md content.
//!
//! When a skill is activated, its Markdown body may contain:
//!
//! - **Inline commands**: `` !`pwd` `` → replaced by the command's stdout
//! - **Block commands**: `` ```! \n git status \n ``` `` → replaced by stdout
//! - **Variables**: `${SKILL_DIR}`, `${SESSION_ID}`, `${ARGUMENTS}`, `${1}`, `${2}`, etc.
//!
//! This mirrors the Claude Code `promptShellExecution.ts` behavior:
//! commands are executed **at activation time** and their output is substituted
//! inline, making skill instructions dynamic.
//!
//! ## Security
//!
//! - MCP-sourced skills **never** execute inline commands (untrusted remote content)
//! - Each command respects the configured timeout (default 10s per command)
//! - Commands run in the skill's directory as working directory

use std::path::Path;
use std::time::Duration;

use regex::Regex;
use tracing::{debug, warn};

const DEFAULT_CMD_TIMEOUT: Duration = Duration::from_secs(10);

/// Source of a skill (affects security policy).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillSource {
    /// Local filesystem skill (trusted).
    Local,
    /// Remote/MCP skill (untrusted — no shell execution).
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
        }
    }
}

/// Process a skill's Markdown body: substitute variables, then execute inline commands.
///
/// Returns the processed content with all substitutions and command outputs applied.
pub async fn process_skill_content(content: &str, ctx: &PromptContext) -> String {
    let mut result = substitute_variables(content, ctx);

    if ctx.source == SkillSource::Mcp {
        debug!("Skipping inline command execution for MCP skill (untrusted)");
        return result;
    }

    result = execute_block_commands(&result, ctx).await;
    result = execute_inline_commands(&result, ctx).await;
    result
}

/// Substitute template variables in skill content.
///
/// Supported variables:
/// - `${SKILL_DIR}` — absolute path to the skill directory
/// - `${SESSION_ID}` — current session identifier
/// - `${ARGUMENTS}` — all arguments joined by space
/// - `${1}`, `${2}`, ... — positional arguments
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

/// Execute block commands: ` ```! \n command \n ``` ` → stdout
async fn execute_block_commands(content: &str, ctx: &PromptContext) -> String {
    // Pattern: ```! followed by optional newline, then command(s), then ```
    let re = Regex::new(r"```!\s*\n?([\s\S]*?)\n?```").expect("valid regex");

    let mut result = content.to_string();
    let matches: Vec<_> = re.captures_iter(content).collect();

    // Process in reverse order so byte offsets remain valid
    for cap in matches.into_iter().rev() {
        let full_match = cap.get(0).unwrap();
        let command = cap.get(1).map(|m| m.as_str().trim()).unwrap_or("");

        if command.is_empty() {
            continue;
        }

        let output = run_command(command, ctx).await;

        result.replace_range(full_match.range(), &output);
    }

    result
}

/// Execute inline commands: `` !`command` `` → stdout
///
/// The pattern requires whitespace or line start before `!`` to avoid
/// matching Markdown image syntax (`![alt](url)`).
async fn execute_inline_commands(content: &str, ctx: &PromptContext) -> String {
    let re = Regex::new(r"(?:^|\s)!`([^`]+)`").expect("valid regex");

    let mut result = content.to_string();
    let matches: Vec<_> = re.captures_iter(content).collect();

    for cap in matches.into_iter().rev() {
        let full_match = cap.get(0).unwrap();
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
async fn run_command(command: &str, ctx: &PromptContext) -> String {
    let shell_cmd = build_shell_command(command, ctx);

    let mut cmd = tokio::process::Command::new(&shell_cmd.program);
    for arg in &shell_cmd.args {
        cmd.arg(arg);
    }

    if !ctx.skill_dir.is_empty() && Path::new(&ctx.skill_dir).exists() {
        cmd.current_dir(&ctx.skill_dir);
    }

    propagate_env(&mut cmd);

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
fn find_git_bash_path() -> Option<String> {
    None
}

fn propagate_env(cmd: &mut tokio::process::Command) {
    if let Ok(path) = std::env::var("PATH") {
        cmd.env("PATH", path);
    }
    if let Ok(home) = std::env::var("HOME") {
        cmd.env("HOME", home);
    }
    #[cfg(target_os = "windows")]
    {
        for key in &[
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "SYSTEMROOT",
            "COMSPEC",
            "PATHEXT",
        ] {
            if let Ok(val) = std::env::var(key) {
                cmd.env(key, val);
            }
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

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
}
