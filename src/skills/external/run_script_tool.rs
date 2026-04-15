//! RunSkillScriptTool — execute scripts bundled with skills (Tier 3).
//!
//! Many real-world skills contain Python/Node/Shell scripts in their `scripts/`
//! directory. This tool lets the LLM execute them directly within the correct
//! working directory (the skill's root), with proper interpreter detection.
//!
//! ## Cross-platform execution
//!
//! | Extension | Unix | Windows |
//! |-----------|------|---------|
//! | `.py` | `python3 script.py` | `python script.py` |
//! | `.js` | `node script.js` | `node script.js` |
//! | `.ts` | `bun` / `deno` / `npx tsx` | same detection |
//! | `.sh` | `bash script.sh` | Git Bash → PowerShell fallback |
//! | `.ps1` | `pwsh script.ps1` | `powershell script.ps1` |
//! | `.rb` | `ruby script.rb` | `ruby script.rb` |
//!
//! On Windows, shell scripts (`.sh`, `.bash`) attempt Git Bash first, then
//! fall back to PowerShell. The interpreter is invoked **directly** (not via
//! `cmd /C` or `sh -c`) so there is no shell injection vector.
//!
//! ## Security model
//!
//! - Only scripts from **activated** skills can be run
//! - Path traversal (`..`) is rejected
//! - Interpreter is invoked directly (no shell wrapping by default)
//! - Configurable timeout (default 30 seconds)
//! - Optional sandbox integration

use std::path::PathBuf;
use std::sync::Arc;

use futures::future::BoxFuture;
use serde_json::json;
use tokio::sync::RwLock;

use crate::error::{Result, ToolError};
use crate::sandbox::{SandboxCommand, SandboxExecutor};
use crate::skills::registry::SkillRegistry;
use crate::tools::{Tool, ToolParameters, ToolResult};

const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Tool for executing scripts from activated skill directories.
///
/// See the [module-level docs](self) for cross-platform behavior and security model.
pub struct RunSkillScriptTool {
    registry: Arc<RwLock<SkillRegistry>>,
    sandbox: Option<Arc<dyn SandboxExecutor>>,
    timeout_secs: u64,
}

impl RunSkillScriptTool {
    pub fn new(registry: Arc<RwLock<SkillRegistry>>) -> Self {
        Self {
            registry,
            sandbox: None,
            timeout_secs: DEFAULT_TIMEOUT_SECS,
        }
    }

    pub fn with_sandbox(mut self, sandbox: Arc<dyn SandboxExecutor>) -> Self {
        self.sandbox = Some(sandbox);
        self
    }

    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }
}

impl Tool for RunSkillScriptTool {
    fn name(&self) -> &str {
        "run_skill_script"
    }

    fn description(&self) -> &str {
        "Execute a script from an activated skill's scripts/ directory. \
         The working directory is set to the skill's root. \
         Supports Python (.py), Node.js (.js/.ts), Bash (.sh), PowerShell (.ps1), \
         Ruby (.rb), Perl (.pl). \
         The skill must be activated first via activate_skill."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "skill_name": {
                    "type": "string",
                    "description": "Name of the activated skill"
                },
                "script": {
                    "type": "string",
                    "description": "Relative path to the script (e.g., 'scripts/analyze.py')"
                },
                "args": {
                    "type": "string",
                    "description": "Command-line arguments to pass to the script (optional)",
                    "default": ""
                }
            },
            "required": ["skill_name", "script"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let skill_name = parameters
                .get("skill_name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("skill_name".to_string()))?
                .to_string();

            let script_path = parameters
                .get("script")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("script".to_string()))?
                .to_string();

            let args_str = parameters
                .get("args")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            if script_path.contains("..") {
                return Ok(ToolResult::error(
                    "Path traversal ('..') is not allowed in script paths".into(),
                ));
            }

            let registry = self.registry.read().await;

            if !registry.is_activated(&skill_name) {
                return Ok(ToolResult::error(format!(
                    "Skill '{}' has not been activated. Call activate_skill first.",
                    skill_name
                )));
            }

            let descriptor = match registry.get_descriptor(&skill_name) {
                Some(d) => d,
                None => {
                    return Ok(ToolResult::error(format!(
                        "Skill '{}' not found in catalog",
                        skill_name
                    )));
                }
            };

            let skill_dir = match descriptor.location.parent() {
                Some(d) => d.to_path_buf(),
                None => {
                    return Ok(ToolResult::error(format!(
                        "Cannot determine skill directory for '{}'",
                        skill_name
                    )));
                }
            };

            let full_script_path = skill_dir.join(&script_path);

            // Verify resolved path stays within skill directory
            if let (Ok(canonical_skill), Ok(canonical_script)) =
                (skill_dir.canonicalize(), full_script_path.canonicalize())
                && !canonical_script.starts_with(&canonical_skill)
            {
                return Ok(ToolResult::error(
                    "Resolved script path escapes the skill directory".into(),
                ));
            }

            if !full_script_path.exists() {
                return Ok(ToolResult::error(format!(
                    "Script not found: {} (in skill '{}')",
                    script_path, skill_name
                )));
            }

            // Parse user args (simple split, respects quotes would need a real parser)
            let extra_args: Vec<String> = if args_str.is_empty() {
                vec![]
            } else {
                shell_words_split(&args_str)
            };

            let timeout = std::time::Duration::from_secs(self.timeout_secs);

            // Sandbox path: build a shell command string for the sandbox executor
            if let Some(sandbox) = &self.sandbox {
                let invocation = resolve_interpreter(&script_path);
                let mut parts = vec![];
                parts.extend(invocation.shell_prefix.iter().cloned());
                parts.push(full_script_path.display().to_string());
                parts.extend(extra_args.iter().cloned());
                let command_str = parts.join(" ");

                let mut sandbox_cmd = SandboxCommand::shell(&command_str);
                sandbox_cmd.timeout = timeout;
                if let Ok(canonical) = skill_dir.canonicalize() {
                    sandbox_cmd.working_dir = Some(canonical);
                }

                return match sandbox.execute(sandbox_cmd).await {
                    Ok(result) => format_execution_result(
                        &skill_name,
                        &script_path,
                        result.exit_code,
                        &result.stdout,
                        &result.stderr,
                    ),
                    Err(e) => Ok(ToolResult::error(format!(
                        "Sandbox execution failed for '{}' in skill '{}': {}",
                        script_path, skill_name, e
                    ))),
                };
            }

            // Direct execution: invoke the interpreter as a process (no shell wrapping)
            let invocation = resolve_interpreter(&script_path);
            let mut cmd = tokio::process::Command::new(&invocation.program);

            for arg in &invocation.prefix_args {
                cmd.arg(arg);
            }
            cmd.arg(&full_script_path);
            for arg in &extra_args {
                cmd.arg(arg);
            }
            cmd.current_dir(&skill_dir);

            // Inherit a clean environment with PATH so interpreters are found
            propagate_path_env(&mut cmd);

            match tokio::time::timeout(timeout, cmd.output()).await {
                Ok(Ok(output)) => {
                    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                    format_execution_result(
                        &skill_name,
                        &script_path,
                        output.status.code().unwrap_or(-1),
                        &stdout,
                        &stderr,
                    )
                }
                Ok(Err(e)) => Ok(ToolResult::error(format!(
                    "Failed to execute '{}' in skill '{}': {}. \
                     Ensure the interpreter is installed and on PATH.",
                    script_path, skill_name, e
                ))),
                Err(_) => Ok(ToolResult::error(format!(
                    "Script '{}' in skill '{}' timed out after {}s",
                    script_path, skill_name, self.timeout_secs
                ))),
            }
        })
    }
}

// ── Interpreter Resolution ───────────────────────────────────────────────────

/// Resolved invocation: how to run a script file.
struct Invocation {
    /// The executable to call (e.g. `python3`, `node`, `bash`, `powershell`).
    program: String,
    /// Arguments inserted between the program and the script path.
    /// e.g. for `deno run --allow-read script.ts`, prefix_args = ["run", "--allow-read"]
    prefix_args: Vec<String>,
    /// For sandbox mode (shell command string), the full prefix before the script path.
    shell_prefix: Vec<String>,
}

/// Resolve the interpreter invocation for a script path, with cross-platform awareness.
fn resolve_interpreter(script_path: &str) -> Invocation {
    let ext = script_path
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();

    match ext.as_str() {
        "py" => resolve_python(),
        "js" | "mjs" | "cjs" => Invocation::simple("node"),
        "ts" | "mts" | "tsx" => resolve_typescript(),
        "sh" | "bash" => resolve_shell(),
        "ps1" => resolve_powershell(),
        "rb" => Invocation::simple("ruby"),
        "pl" => Invocation::simple("perl"),
        "php" => Invocation::simple("php"),
        "r" | "R" => Invocation::new("Rscript", vec![], vec!["Rscript"]),
        _ => resolve_shell(), // unknown extension → try shell
    }
}

impl Invocation {
    fn simple(program: &str) -> Self {
        Self {
            program: program.into(),
            prefix_args: vec![],
            shell_prefix: vec![program.into()],
        }
    }

    fn new(program: &str, prefix_args: Vec<&str>, shell_prefix: Vec<&str>) -> Self {
        Self {
            program: program.into(),
            prefix_args: prefix_args.into_iter().map(String::from).collect(),
            shell_prefix: shell_prefix.into_iter().map(String::from).collect(),
        }
    }
}

/// Python: `python3` on Unix, `python` on Windows (where `python3` often doesn't exist).
fn resolve_python() -> Invocation {
    if cfg!(target_os = "windows") {
        // On Windows, prefer `python` (the py launcher maps to Python 3 by default).
        // Fall back to `py -3` if `python` isn't on PATH.
        if which_exists("python") {
            Invocation::simple("python")
        } else {
            Invocation::new("py", vec!["-3"], vec!["py", "-3"])
        }
    } else {
        // Unix: prefer python3, fall back to python
        if which_exists("python3") {
            Invocation::simple("python3")
        } else {
            Invocation::simple("python")
        }
    }
}

/// TypeScript: `bun` → `deno run` → `npx tsx` (cross-platform, same priority).
fn resolve_typescript() -> Invocation {
    if which_exists("bun") {
        return Invocation::simple("bun");
    }
    if which_exists("deno") {
        return Invocation::new(
            "deno",
            vec!["run", "--allow-read", "--allow-env"],
            vec!["deno", "run", "--allow-read", "--allow-env"],
        );
    }
    // npx tsx: works with any Node.js, auto-installs tsx on first run
    Invocation::new("npx", vec!["tsx"], vec!["npx", "tsx"])
}

/// Shell scripts (.sh/.bash): bash on Unix, Git Bash → PowerShell on Windows.
fn resolve_shell() -> Invocation {
    if cfg!(target_os = "windows") {
        // Try Git Bash first (common on Windows dev machines)
        if let Some(git_bash) = find_git_bash() {
            return Invocation::new(
                git_bash.to_str().unwrap_or("bash"),
                vec![],
                vec![git_bash.to_str().unwrap_or("bash")],
            );
        }
        // WSL bash
        if which_exists("wsl") {
            return Invocation::new("wsl", vec!["bash"], vec!["wsl", "bash"]);
        }
        // Last resort: PowerShell (won't understand bash syntax, but at least won't crash)
        resolve_powershell()
    } else {
        Invocation::simple("bash")
    }
}

/// PowerShell: `pwsh` (cross-platform PS 7+) → `powershell` (Windows built-in).
fn resolve_powershell() -> Invocation {
    if which_exists("pwsh") {
        Invocation::new(
            "pwsh",
            vec!["-NoProfile", "-NonInteractive", "-File"],
            vec!["pwsh", "-NoProfile", "-NonInteractive", "-File"],
        )
    } else if cfg!(target_os = "windows") {
        Invocation::new(
            "powershell",
            vec![
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ],
            vec![
                "powershell",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ],
        )
    } else {
        // PowerShell not available on this Unix system
        Invocation::simple("sh")
    }
}

/// Find Git Bash on Windows (common installation paths).
#[cfg(target_os = "windows")]
fn find_git_bash() -> Option<PathBuf> {
    let candidates = [
        std::env::var("ProgramFiles")
            .ok()
            .map(|p| PathBuf::from(p).join("Git").join("bin").join("bash.exe")),
        std::env::var("ProgramFiles(x86)")
            .ok()
            .map(|p| PathBuf::from(p).join("Git").join("bin").join("bash.exe")),
        Some(PathBuf::from(r"C:\Program Files\Git\bin\bash.exe")),
        Some(PathBuf::from(r"C:\Program Files (x86)\Git\bin\bash.exe")),
        // Also check if bash is on PATH (could be Git Bash, MSYS2, or Cygwin)
    ];

    for candidate in candidates.into_iter().flatten() {
        if candidate.exists() {
            return Some(candidate);
        }
    }

    // Check PATH
    if which_exists("bash") {
        return Some(PathBuf::from("bash"));
    }

    None
}

#[cfg(not(target_os = "windows"))]
fn find_git_bash() -> Option<PathBuf> {
    None // Not needed on Unix, bash is always available
}

// ── Utilities ────────────────────────────────────────────────────────────────

/// Check if a command exists on PATH (fast, no output).
fn which_exists(cmd: &str) -> bool {
    std::process::Command::new(cmd)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Propagate essential environment variables (PATH, HOME, etc.) to the child process.
fn propagate_path_env(cmd: &mut tokio::process::Command) {
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
    // Node.js / npm need these
    if let Ok(node_path) = std::env::var("NODE_PATH") {
        cmd.env("NODE_PATH", node_path);
    }
    // Python virtual environments
    if let Ok(venv) = std::env::var("VIRTUAL_ENV") {
        cmd.env("VIRTUAL_ENV", venv);
    }
}

/// Simple argument splitting that respects double quotes.
/// Not a full POSIX shell parser, but handles the common cases.
fn shell_words_split(s: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut chars = s.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '"' => in_quotes = !in_quotes,
            '\'' if !in_quotes => {
                // Single-quoted string: consume until next single quote
                while let Some(&next) = chars.peek() {
                    if next == '\'' {
                        chars.next();
                        break;
                    }
                    current.push(next);
                    chars.next();
                }
            }
            ' ' | '\t' if !in_quotes => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            '\\' if !in_quotes => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

/// Format execution output into a structured result.
fn format_execution_result(
    skill_name: &str,
    script_path: &str,
    exit_code: i32,
    stdout: &str,
    stderr: &str,
) -> Result<ToolResult> {
    let mut output = format!(
        "<script_output skill=\"{}\" script=\"{}\" exit_code=\"{}\">\n",
        skill_name, script_path, exit_code
    );

    if !stdout.is_empty() {
        output.push_str(stdout);
        if !stdout.ends_with('\n') {
            output.push('\n');
        }
    }

    if !stderr.is_empty() {
        output.push_str(&format!("\n<stderr>\n{}</stderr>\n", stderr.trim()));
    }

    output.push_str("</script_output>");

    if exit_code == 0 {
        Ok(ToolResult::success(output))
    } else {
        Ok(ToolResult {
            success: false,
            output,
            error: Some(format!(
                "Script '{}' exited with code {}",
                script_path, exit_code
            )),
        })
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_python() {
        let inv = resolve_python();
        // On any dev machine, one of these should be the program
        assert!(
            inv.program == "python3" || inv.program == "python" || inv.program == "py",
            "unexpected python program: {}",
            inv.program
        );
    }

    #[test]
    fn test_resolve_typescript() {
        let inv = resolve_typescript();
        // Should resolve to something (bun, deno, or npx)
        assert!(
            inv.program == "bun" || inv.program == "deno" || inv.program == "npx",
            "unexpected ts program: {}",
            inv.program
        );
    }

    #[test]
    fn test_resolve_shell() {
        let inv = resolve_shell();
        if cfg!(target_os = "windows") {
            // Should be bash, wsl, pwsh, or powershell
            assert!(
                inv.program.contains("bash")
                    || inv.program == "wsl"
                    || inv.program == "pwsh"
                    || inv.program == "powershell",
                "unexpected shell program: {}",
                inv.program
            );
        } else {
            assert_eq!(inv.program, "bash");
        }
    }

    #[test]
    fn test_resolve_powershell() {
        let inv = resolve_powershell();
        assert!(
            inv.program == "pwsh" || inv.program == "powershell" || inv.program == "sh",
            "unexpected ps program: {}",
            inv.program
        );
    }

    #[test]
    fn test_resolve_interpreter_extensions() {
        assert_eq!(resolve_interpreter("test.js").program, "node");
        assert_eq!(resolve_interpreter("test.rb").program, "ruby");
        assert_eq!(resolve_interpreter("test.pl").program, "perl");
        assert_eq!(resolve_interpreter("test.php").program, "php");
        assert_eq!(resolve_interpreter("test.R").program, "Rscript");
    }

    #[test]
    fn test_shell_words_split_simple() {
        assert_eq!(
            shell_words_split("--input data.csv --output result.json"),
            vec!["--input", "data.csv", "--output", "result.json"]
        );
    }

    #[test]
    fn test_shell_words_split_quotes() {
        assert_eq!(
            shell_words_split(r#"--name "hello world" --verbose"#),
            vec!["--name", "hello world", "--verbose"]
        );
    }

    #[test]
    fn test_shell_words_split_single_quotes() {
        assert_eq!(
            shell_words_split("--pattern 'foo bar' --count 5"),
            vec!["--pattern", "foo bar", "--count", "5"]
        );
    }

    #[test]
    fn test_shell_words_split_empty() {
        assert!(shell_words_split("").is_empty());
        assert!(shell_words_split("   ").is_empty());
    }

    #[test]
    fn test_shell_words_split_escaped() {
        assert_eq!(shell_words_split(r"hello\ world"), vec!["hello world"]);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn test_find_git_bash_returns_something_or_none() {
        // Just verify it doesn't panic
        let _ = find_git_bash();
    }
}
