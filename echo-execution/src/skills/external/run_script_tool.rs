//! RunSkillScriptTool -- execute scripts bundled with skills (Tier 3).
//!
//! Many real-world skills contain Python/Node/Shell scripts in their `scripts/`
//! directory. This tool lets the LLM execute them directly within the correct
//! working directory (the skill's root), with proper interpreter detection.
//!
//! ## Sandbox integration
//!
//! All script execution goes through [`SandboxManager`], which automatically
//! selects the best isolation level based on security policy:
//!
//! | Command type | Typical isolation |
//! |-------------|-------------------|
//! | `ls`, `cat`, `echo` | None (trusted) |
//! | `python3 script.py` | OS sandbox |
//! | Unknown / dangerous | Docker container |
//!
//! If no `SandboxManager` is configured, falls back to bare process spawning
//! (for simple demos / tests).
//!
//! ## Cross-platform interpreter detection
//!
//! | Extension | Unix | Windows |
//! |-----------|------|---------|
//! | `.py` | `python3 script.py` | `python script.py` |
//! | `.js` | `node script.js` | `node script.js` |
//! | `.ts` | `bun` / `deno` / `npx tsx` | same detection |
//! | `.sh` | `bash script.sh` | Git Bash -> PowerShell fallback |
//! | `.ps1` | `pwsh script.ps1` | `powershell script.ps1` |
//! | `.rb` | `ruby script.rb` | `ruby script.rb` |
//!
//! ## Security model
//!
//! - Only scripts from **activated** skills can be run
//! - Path traversal (`..`) is rejected
//! - Interpreter is invoked directly (no shell wrapping)
//! - Configurable timeout (default 30 seconds)
//! - Full sandbox integration with policy-based routing

use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::future::BoxFuture;
use serde_json::json;
use tokio::sync::RwLock;

use crate::sandbox::{SandboxCommand, SandboxManager};
use crate::skills::registry::SkillRegistry;
use crate::skills::{is_path_safe, minimal_env};
use echo_core::error::{Result, ToolError};
use echo_core::tools::{Tool, ToolParameters, ToolResult};

const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Tool for executing scripts from activated skill directories.
///
/// See the [module-level docs](self) for cross-platform behavior and security model.
pub struct RunSkillScriptTool {
    registry: Arc<RwLock<SkillRegistry>>,
    sandbox: Option<Arc<SandboxManager>>,
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

    pub fn with_sandbox_manager(mut self, manager: Arc<SandboxManager>) -> Self {
        self.sandbox = Some(manager);
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

            if !is_path_safe(Path::new(""), Path::new(&script_path)) {
                return Ok(ToolResult::error(
                    "Path traversal ('..') is not allowed in script paths",
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
                    "Resolved script path escapes the skill directory",
                ));
            }

            if !full_script_path.exists() {
                return Ok(ToolResult::error(format!(
                    "Script not found: {} (in skill '{}')",
                    script_path, skill_name
                )));
            }

            // Resolve interpreter from script extension
            let invocation = resolve_interpreter(&script_path);

            // Build argument list: [prefix_args...] <script_path> [user_args...]
            let mut all_args: Vec<String> = invocation.prefix_args.iter().cloned().collect();
            all_args.push(full_script_path.display().to_string());
            if !args_str.is_empty() {
                if let Some(parsed) = shlex::split(&args_str) {
                    all_args.extend(parsed);
                } else {
                    // Fallback: treat the whole string as a single argument
                    all_args.push(args_str);
                }
            }

            let canonical_skill_dir = skill_dir
                .canonicalize()
                .unwrap_or_else(|_| skill_dir.clone());
            let timeout = std::time::Duration::from_secs(self.timeout_secs);

            // -- Sandbox execution path --
            if let Some(ref manager) = self.sandbox {
                let sandbox_cmd = SandboxCommand::program(&invocation.program, all_args)
                    .with_working_dir(&canonical_skill_dir)
                    .with_timeout(timeout);

                return match manager.execute(sandbox_cmd).await {
                    Ok(result) => format_execution_result(
                        &skill_name,
                        &script_path,
                        result.exit_code,
                        &result.stdout,
                        &result.stderr,
                        &result.sandbox_type,
                    ),
                    Err(e) => Ok(ToolResult::error(format!(
                        "Sandbox execution failed for '{}' in skill '{}': {}",
                        script_path, skill_name, e
                    ))),
                };
            }

            // -- Fallback: direct process execution (no sandbox) --
            let mut cmd = tokio::process::Command::new(&invocation.program);
            for arg in &all_args {
                cmd.arg(arg);
            }
            cmd.current_dir(&canonical_skill_dir);

            // Use minimal environment to avoid leaking sensitive variables
            let env = minimal_env(
                &canonical_skill_dir.display().to_string(),
                "", // no session_id needed for script execution
                std::collections::HashMap::new(),
            );
            for (k, v) in env {
                cmd.env(k, v);
            }

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
                        "direct",
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

// -- Interpreter Resolution --

/// Resolved invocation: how to run a script file.
struct Invocation {
    /// The executable to call (e.g. `python3`, `node`, `bash`).
    program: String,
    /// Arguments inserted between the program and the script path.
    prefix_args: Vec<String>,
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
        "r" | "R" => Invocation::new("Rscript", vec![]),
        _ => resolve_shell(), // unknown extension -> try shell
    }
}

impl Invocation {
    fn simple(program: &str) -> Self {
        Self {
            program: program.into(),
            prefix_args: vec![],
        }
    }

    fn new(program: &str, prefix_args: Vec<&str>) -> Self {
        Self {
            program: program.into(),
            prefix_args: prefix_args.into_iter().map(String::from).collect(),
        }
    }
}

/// Python: `python3` on Unix, `python` on Windows.
fn resolve_python() -> Invocation {
    if cfg!(target_os = "windows") {
        if which_exists("python") {
            Invocation::simple("python")
        } else {
            Invocation::new("py", vec!["-3"])
        }
    } else {
        if which_exists("python3") {
            Invocation::simple("python3")
        } else {
            Invocation::simple("python")
        }
    }
}

/// TypeScript: `bun` -> `deno run` -> `npx tsx`.
fn resolve_typescript() -> Invocation {
    if which_exists("bun") {
        return Invocation::simple("bun");
    }
    if which_exists("deno") {
        return Invocation::new("deno", vec!["run", "--allow-read", "--allow-env"]);
    }
    Invocation::new("npx", vec!["tsx"])
}

/// Shell scripts (.sh/.bash): bash on Unix, Git Bash -> PowerShell on Windows.
fn resolve_shell() -> Invocation {
    if cfg!(target_os = "windows") {
        if let Some(git_bash) = find_git_bash() {
            return Invocation::simple(git_bash.to_str().unwrap_or("bash"));
        }
        if which_exists("wsl") {
            return Invocation::new("wsl", vec!["bash"]);
        }
        resolve_powershell()
    } else {
        Invocation::simple("bash")
    }
}

/// PowerShell: `pwsh` (cross-platform PS 7+) -> `powershell` (Windows built-in).
fn resolve_powershell() -> Invocation {
    if which_exists("pwsh") {
        Invocation::new("pwsh", vec!["-NoProfile", "-NonInteractive", "-File"])
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
        )
    } else {
        Invocation::simple("sh")
    }
}

/// Find Git Bash on Windows.
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
    ];

    for candidate in candidates.into_iter().flatten() {
        if candidate.exists() {
            return Some(candidate);
        }
    }

    if which_exists("bash") {
        return Some(PathBuf::from("bash"));
    }

    None
}

#[cfg(not(target_os = "windows"))]
fn find_git_bash() -> Option<PathBuf> {
    None
}

// -- Utilities --

/// Check if a command exists on PATH.
fn which_exists(cmd: &str) -> bool {
    std::process::Command::new(cmd)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Format execution output into a structured result.
fn format_execution_result(
    skill_name: &str,
    script_path: &str,
    exit_code: i32,
    stdout: &str,
    stderr: &str,
    sandbox_type: &str,
) -> Result<ToolResult> {
    let mut output = format!(
        "<script_output skill=\"{}\" script=\"{}\" exit_code=\"{}\" sandbox=\"{}\">\n",
        skill_name, script_path, exit_code, sandbox_type,
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
            bytes: None,
        })
    }
}

// -- Tests --

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_python() {
        let inv = resolve_python();
        assert!(
            inv.program == "python3" || inv.program == "python" || inv.program == "py",
            "unexpected python program: {}",
            inv.program
        );
    }

    #[test]
    fn test_resolve_typescript() {
        let inv = resolve_typescript();
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
    fn test_shlex_split_simple() {
        let result = shlex::split("--input data.csv --output result.json");
        assert_eq!(
            result,
            Some(vec![
                "--input".to_string(),
                "data.csv".to_string(),
                "--output".to_string(),
                "result.json".to_string(),
            ])
        );
    }

    #[test]
    fn test_shlex_split_quotes() {
        let result = shlex::split(r#"--name "hello world" --verbose"#);
        assert_eq!(
            result,
            Some(vec![
                "--name".to_string(),
                "hello world".to_string(),
                "--verbose".to_string(),
            ])
        );
    }

    #[test]
    fn test_shlex_split_empty() {
        assert!(shlex::split("").unwrap_or_default().is_empty());
        assert!(shlex::split("   ").unwrap_or_default().is_empty());
    }

    #[test]
    fn test_invocation_no_longer_has_shell_prefix() {
        let inv = Invocation::simple("python3");
        assert_eq!(inv.program, "python3");
        assert!(inv.prefix_args.is_empty());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn test_find_git_bash_returns_something_or_none() {
        let _ = find_git_bash();
    }
}
