//! RunSkillScriptTool -- execute scripts bundled with skills (Tier 3).
//!
//! Many real-world skills contain Python/Node/Shell scripts in their `scripts/`
//! directory. This tool lets the LLM execute them directly within the correct
//! working directory (the skill's root), with proper interpreter detection.
//!
//! ## Sandbox integration
//!
//! 有绑定 executor 时保持该 Skill source 的原执行器；SandboxManager 按原政策选择隔离层：
//! 历史默认路由如下，不以宿主执行器替代 source executor：
//!
//! | Command type | Typical isolation |
//! |-------------|-------------------|
//! | `ls`, `cat`, `echo` | None (trusted) |
//! | `python3 script.py` | OS sandbox |
//! | Unknown / dangerous | Docker container |
//!
//! If no `SandboxManager` is configured, falls back to bare process spawning
//! with a minimal environment whitelist (`env_clear()` + PATH / SKILL_DIR / SESSION_ID)
//! 并复用本地进程 owner：取消、超时或调用者掉线后等待 kill／reap／drain。
//! 原生资源准入是可选能力，策略由宿主注入，guard 只随该次物理操作保有。
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
//! - Script paths must be relative and must canonicalize under the activated skill directory
//! - Interpreter is invoked directly (no shell wrapping)
//! - Configurable timeout (default 30 seconds)
//! - Full sandbox integration with policy-based routing
//! - Malformed `args` strings are rejected instead of being silently reinterpreted

use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::future::BoxFuture;
use serde_json::json;
use tokio::sync::RwLock;

use crate::sandbox::{ResourceLimits, SandboxCommand, SandboxExecutor};
use crate::skills::external::types::SkillSandboxPolicy;
use crate::skills::minimal_env;
use crate::skills::registry::SkillRegistry;
use echo_core::error::{Result, ToolError};
use echo_core::tools::permission::ToolPermission;
use echo_core::tools::{Tool, ToolParameters, ToolResult, ToolRiskLevel};

const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Tool for executing scripts from activated skill directories.
///
/// See the [module-level docs](self) for cross-platform behavior and security model.
pub struct RunSkillScriptTool {
    registry: Arc<RwLock<SkillRegistry>>,
    sandbox: Option<Arc<dyn SandboxExecutor>>,
    timeout_secs: u64,
}

/// Apply SkillSandboxPolicy constraints to a SandboxCommand.
///
/// Standalone function (not a method) so it can be called from within
/// the `execute` method's `BoxFuture` async block.
fn apply_sandbox_policy(cmd: &mut SandboxCommand, policy: &SkillSandboxPolicy) {
    if !policy.is_constraining() {
        return;
    }
    if let Some(t) = policy.timeout_secs {
        cmd.timeout = std::time::Duration::from_secs(t);
    }
    cmd.minimum_isolation = policy.isolation;
}

/// Build ResourceLimits from a SkillSandboxPolicy for `execute_with_limits`.
/// When the policy declares network/path restrictions, they translate to
/// sandbox resource limits enforced at the OS level (Seatbelt / Landlock).
fn sandbox_limits_from_policy(policy: &SkillSandboxPolicy) -> ResourceLimits {
    let mut limits = ResourceLimits::unrestricted(); // embedding application local trust model
    limits.network = policy.network.unwrap_or(true);
    if !policy.allowed_paths.is_empty() {
        limits.writable_paths = policy.allowed_paths.clone();
    }
    if let Some(t) = policy.timeout_secs {
        limits.cpu_time_secs = Some(t);
    }
    limits
}

impl RunSkillScriptTool {
    pub fn new(registry: Arc<RwLock<SkillRegistry>>) -> Self {
        Self {
            registry,
            sandbox: None,
            timeout_secs: DEFAULT_TIMEOUT_SECS,
        }
    }

    pub fn with_sandbox_manager(mut self, manager: Arc<dyn SandboxExecutor>) -> Self {
        self.sandbox = Some(manager);
        self
    }

    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }
    fn execute_in_context<'a>(
        &'a self,
        parameters: ToolParameters,
        context: Option<&'a echo_core::tools::ToolContext>,
    ) -> BoxFuture<'a, Result<ToolResult>> {
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

            let script_relative_path = Path::new(&script_path);
            if script_relative_path.is_absolute()
                || script_relative_path
                    .components()
                    .any(|component| matches!(component, std::path::Component::ParentDir))
            {
                return Ok(ToolResult::error(
                    "Script path must be a relative path inside the activated skill directory",
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
                Some(d) => d.clone(),
                None => {
                    return Ok(ToolResult::error(format!(
                        "Skill '{}' not found in catalog",
                        skill_name
                    )));
                }
            };
            let source = registry
                .file_source_with_fallback(&skill_name, self.sandbox.clone())
                .for_invocation(context);
            drop(registry);

            if !descriptor.permits_tool(self.name()) {
                return Ok(ToolResult::error(format!(
                    "Skill '{}' does not permit tool '{}'; allowed-tools: {}",
                    skill_name,
                    self.name(),
                    descriptor.allowed_tools.join(", ")
                )));
            }

            let skill_dir = match descriptor.location.parent() {
                Some(d) => d.to_path_buf(),
                None => {
                    return Ok(ToolResult::error(format!(
                        "Cannot determine skill directory for '{}'",
                        skill_name
                    )));
                }
            };

            let canonical_skill_dir = source.canonicalize(&skill_dir).await?;
            let full_script_path = canonical_skill_dir.join(script_relative_path);
            let canonical_script_path = source.canonicalize(&full_script_path).await?;

            if !canonical_script_path.starts_with(&canonical_skill_dir) {
                return Ok(ToolResult::error(
                    "Resolved script path escapes the skill directory",
                ));
            }

            if !source.is_file(&canonical_script_path).await? {
                return Ok(ToolResult::error(format!(
                    "Script not found: {} (in skill '{}')",
                    script_path, skill_name
                )));
            }

            // Validate arguments before interpreter probing can start a target
            // process; malformed input must not execute even a version probe.
            let user_args = shlex::split(&args_str).ok_or_else(|| ToolError::InvalidParameter {
                name: "args".to_string(),
                message: "Malformed script arguments: unmatched quotes or escapes".to_string(),
            })?;

            // A registered origin is authoritative even when its executor is
            // absent; a host default may belong to an entirely different target.
            let executor = source.sandbox();
            if source.requires_executor() && executor.is_none() {
                return Err(ToolError::ExecutionFailed {
                    tool: self.name().to_string(),
                    message: "This Skill's file source requires a matching script executor"
                        .to_string(),
                }
                .into());
            }
            if executor.is_none()
                && descriptor
                    .sandbox
                    .as_ref()
                    .is_some_and(SkillSandboxPolicy::is_constraining)
            {
                return Err(ToolError::ExecutionFailed {
                    tool: self.name().to_string(),
                    message: "Skill sandbox constraints require a matching executor".to_string(),
                }
                .into());
            }
            let invocation = if source.requires_executor() {
                let manager = executor
                    .as_deref()
                    .ok_or_else(|| ToolError::ExecutionFailed {
                        tool: self.name().to_string(),
                        message: "Skill script executor is unavailable".to_string(),
                    })?;
                resolve_interpreter_in_target(
                    &script_path,
                    &canonical_skill_dir,
                    manager,
                    descriptor.sandbox.as_ref(),
                    self.timeout_secs,
                    context,
                )
                .await?
            } else {
                resolve_interpreter(&script_path, context, self.timeout_secs).await?
            };

            // Build argument list: [prefix_args...] <script_path> [user_args...]
            let mut all_args: Vec<String> = invocation.prefix_args.to_vec();
            all_args.push(canonical_script_path.display().to_string());
            all_args.extend(user_args);

            let timeout = std::time::Duration::from_secs(self.timeout_secs);

            // -- Sandbox execution path --
            if let Some(ref manager) = executor {
                let mut sandbox_cmd =
                    SandboxCommand::program(&invocation.program, all_args.clone())
                        .with_working_dir(&canonical_skill_dir)
                        .with_timeout(timeout);
                sandbox_cmd.resource_guards = context
                    .map(|ctx| ctx.resource_guards.clone())
                    .unwrap_or_default();
                sandbox_cmd.cancel = context.and_then(|ctx| ctx.cancel.clone());

                let execution_result = execute_skill_sandbox_command(
                    manager.as_ref(),
                    sandbox_cmd,
                    descriptor.sandbox.as_ref(),
                    context,
                )
                .await;

                return match execution_result {
                    Ok(result) => format_execution_result(
                        &skill_name,
                        &script_path,
                        result.exit_code,
                        &result.stdout,
                        &result.stderr,
                        &result.sandbox_type,
                    ),
                    Err(error) => Err(error),
                };
            }

            // -- Fallback: direct process execution (no sandbox) --
            let mut cmd = tokio::process::Command::new(&invocation.program);
            for arg in &all_args {
                cmd.arg(arg);
            }
            cmd.current_dir(&canonical_skill_dir);
            // 保留原 Command::output 的关闭 stdin 语义；不能因改用原 child owner 而继承宿主输入。
            cmd.stdin(std::process::Stdio::null());
            cmd.kill_on_drop(true);

            // Use minimal environment to avoid leaking sensitive variables
            let env = minimal_env(
                &canonical_skill_dir.display().to_string(),
                "", // no session_id needed for script execution
                std::collections::HashMap::new(),
            );
            cmd.env_clear();
            for (k, v) in env {
                cmd.env(k, v);
            }

            let mut native_command =
                SandboxCommand::program(&invocation.program, all_args).with_timeout(timeout);
            native_command.resource_guards = context
                .map(|ctx| ctx.resource_guards.clone())
                .unwrap_or_default();
            native_command.cancel = context.and_then(|ctx| ctx.cancel.clone());
            if let Some(context) = context {
                echo_core::tools::native_execution::admit_native_execution(
                    context,
                    &mut native_command,
                    self.name(),
                )
                .await?;
            }
            let result =
                crate::sandbox::local::run_direct_skill_command(cmd, native_command, usize::MAX)
                    .await?;
            if result.timed_out {
                return Ok(ToolResult::error(format!(
                    "Script '{}' in skill '{}' timed out after {}s",
                    script_path, skill_name, self.timeout_secs,
                )));
            }
            format_execution_result(
                &skill_name,
                &script_path,
                result.exit_code,
                &result.stdout,
                &result.stderr,
                "direct",
            )
        })
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

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Execute]
    }
    fn risk_level(&self) -> ToolRiskLevel {
        ToolRiskLevel::Dangerous
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
        self.execute_in_context(parameters, None)
    }

    fn execute_with_context<'a>(
        &'a self,
        parameters: ToolParameters,
        context: &'a echo_core::tools::ToolContext,
    ) -> BoxFuture<'a, Result<ToolResult>> {
        self.execute_in_context(parameters, Some(context))
    }
}

// -- Interpreter Resolution --

/// Probe interpreters through the same executor that will run the script.
/// Local PATH and host OS detection cannot establish a target's runtimes.
async fn resolve_interpreter_in_target(
    script_path: &str,
    skill_dir: &Path,
    executor: &dyn SandboxExecutor,
    policy: Option<&SkillSandboxPolicy>,
    timeout_secs: u64,
    context: Option<&echo_core::tools::ToolContext>,
) -> Result<Invocation> {
    let extension = script_path
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let candidates = match extension.as_str() {
        "py" => vec![
            Invocation::new("uv", vec!["run", "--script"]),
            Invocation::simple("python3"),
            Invocation::simple("python"),
            Invocation::new("py", vec!["-3"]),
        ],
        "ts" | "mts" | "tsx" => vec![
            Invocation::simple("bun"),
            Invocation::new("deno", vec!["run", "--allow-read", "--allow-env"]),
            Invocation::new("npx", vec!["tsx"]),
        ],
        "js" | "mjs" | "cjs" => vec![Invocation::simple("node")],
        "ps1" => vec![
            Invocation::new("pwsh", vec!["-NoProfile", "-NonInteractive", "-File"]),
            Invocation::new("powershell", vec!["-NoProfile", "-NonInteractive", "-File"]),
        ],
        "rb" => vec![Invocation::simple("ruby")],
        "pl" => vec![Invocation::simple("perl")],
        "php" => vec![Invocation::simple("php")],
        "r" => vec![Invocation::simple("Rscript")],
        _ => vec![Invocation::simple("bash"), Invocation::simple("sh")],
    };
    for candidate in candidates {
        let mut command =
            SandboxCommand::program(&candidate.program, vec!["--version".to_string()])
                .with_working_dir(skill_dir)
                .with_timeout(std::time::Duration::from_secs(timeout_secs));
        command.resource_guards = context
            .map(|ctx| ctx.resource_guards.clone())
            .unwrap_or_default();
        command.cancel = context.and_then(|ctx| ctx.cancel.clone());
        let result = execute_skill_sandbox_command(executor, command, policy, context).await?;
        // An executor error propagates above without probing another program
        // or falling back to a local process when settlement may be unknown.
        if result.success() {
            return Ok(candidate);
        }
        if result.timed_out || result.cancelled {
            return Err(ToolError::ExecutionFailed {
                tool: "run_skill_script".to_string(),
                message: "Target interpreter probe did not settle successfully".to_string(),
            }
            .into());
        }
    }
    Err(ToolError::ExecutionFailed {
        tool: "run_skill_script".to_string(),
        message: format!("No interpreter for '{script_path}' is available in the Skill's target"),
    }
    .into())
}

/// Resolved invocation: how to run a script file.
struct Invocation {
    /// The executable to call (e.g. `python3`, `node`, `bash`).
    program: String,
    /// Arguments inserted between the program and the script path.
    prefix_args: Vec<String>,
}

/// Resolve local interpreters without blocking the async runtime; each actual probe
/// uses the original local child owner and the same optional resource policy.
async fn resolve_interpreter(
    script_path: &str,
    context: Option<&echo_core::tools::ToolContext>,
    timeout_secs: u64,
) -> Result<Invocation> {
    let ext = script_path
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "py" => resolve_python(context, timeout_secs).await,
        "js" | "mjs" | "cjs" => Ok(Invocation::simple("node")),
        "ts" | "mts" | "tsx" => resolve_typescript(context, timeout_secs).await,
        "sh" | "bash" => resolve_shell(context, timeout_secs).await,
        "ps1" => resolve_powershell(context, timeout_secs).await,
        "rb" => Ok(Invocation::simple("ruby")),
        "pl" => Ok(Invocation::simple("perl")),
        "php" => Ok(Invocation::simple("php")),
        "r" => Ok(Invocation::simple("Rscript")),
        _ => resolve_shell(context, timeout_secs).await,
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

async fn resolve_python(
    context: Option<&echo_core::tools::ToolContext>,
    timeout_secs: u64,
) -> Result<Invocation> {
    if which_exists("uv", context, timeout_secs).await? {
        return Ok(Invocation::new("uv", vec!["run", "--script"]));
    }
    if which_exists("python3", context, timeout_secs).await? {
        return Ok(Invocation::simple("python3"));
    }
    if which_exists("python", context, timeout_secs).await? {
        return Ok(Invocation::simple("python"));
    }
    if cfg!(target_os = "windows") && which_exists("py", context, timeout_secs).await? {
        return Ok(Invocation::new("py", vec!["-3"]));
    }
    Ok(Invocation::simple("python3"))
}

async fn resolve_typescript(
    context: Option<&echo_core::tools::ToolContext>,
    timeout_secs: u64,
) -> Result<Invocation> {
    if which_exists("bun", context, timeout_secs).await? {
        return Ok(Invocation::simple("bun"));
    }
    if which_exists("deno", context, timeout_secs).await? {
        return Ok(Invocation::new(
            "deno",
            vec!["run", "--allow-read", "--allow-env"],
        ));
    }
    Ok(Invocation::new("npx", vec!["tsx"]))
}

async fn resolve_shell(
    context: Option<&echo_core::tools::ToolContext>,
    timeout_secs: u64,
) -> Result<Invocation> {
    if cfg!(target_os = "windows") {
        if let Some(git_bash) = find_git_bash(context, timeout_secs).await? {
            return Ok(Invocation::simple(git_bash.to_str().unwrap_or("bash")));
        }
        if which_exists("wsl", context, timeout_secs).await? {
            return Ok(Invocation::new("wsl", vec!["bash"]));
        }
        resolve_powershell(context, timeout_secs).await
    } else {
        Ok(Invocation::simple("bash"))
    }
}

async fn resolve_powershell(
    context: Option<&echo_core::tools::ToolContext>,
    timeout_secs: u64,
) -> Result<Invocation> {
    if which_exists("pwsh", context, timeout_secs).await? {
        Ok(Invocation::new(
            "pwsh",
            vec!["-NoProfile", "-NonInteractive", "-File"],
        ))
    } else if cfg!(target_os = "windows") {
        Ok(Invocation::new(
            "powershell",
            vec![
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ],
        ))
    } else {
        Ok(Invocation::simple("sh"))
    }
}

#[cfg(target_os = "windows")]
async fn find_git_bash(
    context: Option<&echo_core::tools::ToolContext>,
    timeout_secs: u64,
) -> Result<Option<PathBuf>> {
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
            return Ok(Some(candidate));
        }
    }
    if which_exists("bash", context, timeout_secs).await? {
        return Ok(Some(PathBuf::from("bash")));
    }
    Ok(None)
}

#[cfg(not(target_os = "windows"))]
async fn find_git_bash(
    _context: Option<&echo_core::tools::ToolContext>,
    _timeout_secs: u64,
) -> Result<Option<PathBuf>> {
    Ok(None)
}

/// Apply policy before admission, then use the original target executor unchanged.
async fn execute_skill_sandbox_command(
    executor: &dyn SandboxExecutor,
    mut command: SandboxCommand,
    policy: Option<&SkillSandboxPolicy>,
    context: Option<&echo_core::tools::ToolContext>,
) -> Result<crate::sandbox::ExecutionResult> {
    let policy = policy.filter(|policy| policy.is_constraining());
    if let Some(policy) = policy {
        apply_sandbox_policy(&mut command, policy);
    }
    if let Some(context) = context {
        echo_core::tools::native_execution::admit_native_execution(
            context,
            &mut command,
            "run_skill_script",
        )
        .await?;
    }
    command.ensure_physical_resource_ownership(executor.supports_physical_resource_ownership())?;
    if let Some(policy) = policy {
        let mut limits = sandbox_limits_from_policy(policy);
        if context.is_some_and(|ctx| ctx.native_execution_admission.is_some()) {
            limits.cpu_time_secs = None;
        }
        executor
            .execute_with_limits_and_cancel(
                command,
                limits,
                context.and_then(|ctx| ctx.cancel.clone()),
            )
            .await
    } else {
        executor.execute(command).await
    }
}

// -- Utilities --

/// Check if a command exists on PATH.
async fn which_exists(
    cmd: &str,
    context: Option<&echo_core::tools::ToolContext>,
    timeout_secs: u64,
) -> Result<bool> {
    let mut command = tokio::process::Command::new(cmd);
    command.arg("--version");
    let mut native = SandboxCommand::program(cmd, vec!["--version".to_string()])
        .with_timeout(std::time::Duration::from_secs(timeout_secs));
    native.resource_guards = context
        .map(|ctx| ctx.resource_guards.clone())
        .unwrap_or_default();
    native.cancel = context.and_then(|ctx| ctx.cancel.clone());
    if let Some(context) = context {
        echo_core::tools::native_execution::admit_native_execution(
            context,
            &mut native,
            "run_skill_script",
        )
        .await?;
    }
    match crate::sandbox::local::run_direct_skill_command(
        command, native,
        0, // --version 原语义是 discard；原 owner 继续排空管道，绝不先缓存全部再丢弃。
    )
    .await
    {
        Ok(result) if result.timed_out || result.cancelled => Err(ToolError::ExecutionFailed {
            tool: "run_skill_script".to_string(),
            message: "Local interpreter probe did not settle successfully".to_string(),
        }
        .into()),
        Ok(result) => Ok(result.success()),
        Err(echo_core::error::ReactError::Sandbox(error))
            if matches!(
                error.as_ref(),
                echo_core::error::SandboxError::StartFailed(_)
            ) =>
        {
            Ok(false)
        }
        Err(error) => Err(error),
    }
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
        Ok(ToolResult::error(format!(
            "Script '{}' exited with code {}",
            script_path, exit_code
        ))
        .with_output(output))
    }
}

// -- Tests --

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::external::types::SkillDescriptor;
    use crate::skills::registry::SkillRegistry;
    use std::collections::HashMap;

    struct NativeAdmission {
        capacity: Arc<tokio::sync::Semaphore>,
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl echo_core::tools::NativeExecutionResourceAdmission for NativeAdmission {
        fn admit<'a>(
            &'a self,
            cancel: &'a tokio_util::sync::CancellationToken,
            deadline: std::time::Instant,
        ) -> BoxFuture<
            'a,
            std::result::Result<
                echo_core::tools::InvocationResourceGuard,
                echo_core::tools::NativeExecutionAdmissionError,
            >,
        > {
            Box::pin(async move {
                use echo_core::tools::NativeExecutionAdmissionError;
                let permit = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return Err(NativeExecutionAdmissionError::Cancelled),
                    _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => return Err(NativeExecutionAdmissionError::Deadline),
                    result = self.capacity.clone().acquire_owned() => result.map_err(|_| NativeExecutionAdmissionError::Closed)?,
                };
                self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(echo_core::tools::InvocationResourceGuard::new(permit))
            })
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn local_probe_and_direct_skill_share_capacity_without_retaining_the_context()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let root =
            std::env::temp_dir().join(format!("native-direct-skill-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(root.join("scripts")).await?;
        tokio::fs::write(root.join("SKILL.md"), "body").await?;
        tokio::fs::write(
            root.join("scripts/effect.sh"),
            "printf 'managed-direct-script-effect'\n",
        )
        .await?;
        let mut registry = SkillRegistry::new();
        registry.register_descriptor(SkillDescriptor {
            source: None,
            name: "direct-skill".to_string(),
            description: "controlled direct fixture".to_string(),
            location: root.join("SKILL.md"),
            license: None,
            compatibility: None,
            metadata: HashMap::new(),
            allowed_tools: vec![],
            shell: None,
            paths: vec![],
            triggers: vec![],
            hooks: None,
            sandbox: None,
            depends_on: vec![],
        });
        registry.mark_activated("direct-skill");
        let capacity = Arc::new(tokio::sync::Semaphore::new(1));
        let calls = Arc::new(AtomicUsize::new(0));
        let context = echo_core::tools::ToolContext {
            native_execution_admission: Some(Arc::new(NativeAdmission {
                capacity: capacity.clone(),
                calls: calls.clone(),
            })),
            ..echo_core::tools::ToolContext::default()
        };
        assert!(which_exists("sh", Some(&context), 5).await?);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(capacity.available_permits(), 1);
        let tool = RunSkillScriptTool::new(Arc::new(RwLock::new(registry)));
        let params: ToolParameters = [
            ("skill_name".to_string(), json!("direct-skill")),
            ("script".to_string(), json!("scripts/effect.sh")),
        ]
        .into();
        let result = tool.execute_with_context(params.clone(), &context).await?;
        assert!(result.success);
        assert!(result.output.contains("managed-direct-script-effect"));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(capacity.available_permits(), 1);
        assert!(context.resource_guards.is_empty());
        let mut malformed = params;
        malformed.insert("args".to_string(), json!("'unclosed"));
        assert!(
            tool.execute_with_context(malformed, &context)
                .await
                .is_err()
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "invalid args must not start even a native probe"
        );
        tokio::fs::remove_dir_all(root).await?;
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn local_interpreter_probe_is_async_cancellable_and_reaped_before_release()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let root =
            std::env::temp_dir().join(format!("native-probe-cancel-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await?;
        let probe = root.join("controlled-interpreter");
        let pid_file = root.join("probe.pid");
        // 真实 --version 探测入口；控制脚本只替代待测 interpreter，不替换进程 owner。
        tokio::fs::write(
            &probe,
            format!("#!/bin/sh\necho $$ > '{}'\nsleep 30\n", pid_file.display()),
        )
        .await?;
        tokio::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o700)).await?;
        let capacity = Arc::new(tokio::sync::Semaphore::new(1));
        let calls = Arc::new(AtomicUsize::new(0));
        let cancel = Arc::new(tokio_util::sync::CancellationToken::new());
        let context = echo_core::tools::ToolContext {
            native_execution_admission: Some(Arc::new(NativeAdmission {
                capacity: capacity.clone(),
                calls: calls.clone(),
            })),
            cancel: Some(cancel.clone()),
            ..echo_core::tools::ToolContext::default()
        };
        let task = tokio::spawn(async move {
            which_exists(&probe.to_string_lossy(), Some(&context), 30).await
        });
        let mut pid = None;
        for _ in 0..100 {
            if let Ok(text) = tokio::fs::read_to_string(&pid_file).await
                && let Ok(value) = text.trim().parse::<i32>()
            {
                pid = Some(value);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        cancel.cancel();
        let result = task.await?;
        let pid = pid.ok_or("native interpreter probe did not publish its real pid")?;
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // SAFETY: signal 0 checks the group created by this exact controlled probe.
        assert!(unsafe { libc::kill(-pid, 0) } != 0);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
        assert_eq!(capacity.available_permits(), 1);
        tokio::fs::remove_dir_all(root).await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_resolve_python() -> Result<()> {
        let inv = resolve_python(None, DEFAULT_TIMEOUT_SECS).await?;
        // uv 优先(P2):装了 uv 时返回 "uv",否则回退 python3/python/py
        assert!(
            inv.program == "uv"
                || inv.program == "python3"
                || inv.program == "python"
                || inv.program == "py",
            "unexpected python program: {}",
            inv.program
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_resolve_typescript() -> Result<()> {
        let inv = resolve_typescript(None, DEFAULT_TIMEOUT_SECS).await?;
        assert!(
            inv.program == "bun" || inv.program == "deno" || inv.program == "npx",
            "unexpected ts program: {}",
            inv.program
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_resolve_shell() -> Result<()> {
        let inv = resolve_shell(None, DEFAULT_TIMEOUT_SECS).await?;
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
        Ok(())
    }

    #[tokio::test]
    async fn test_resolve_powershell() -> Result<()> {
        let inv = resolve_powershell(None, DEFAULT_TIMEOUT_SECS).await?;
        assert!(
            inv.program == "pwsh" || inv.program == "powershell" || inv.program == "sh",
            "unexpected ps program: {}",
            inv.program
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_resolve_interpreter_extensions() -> Result<()> {
        assert_eq!(
            resolve_interpreter("test.js", None, DEFAULT_TIMEOUT_SECS)
                .await?
                .program,
            "node"
        );
        assert_eq!(
            resolve_interpreter("test.rb", None, DEFAULT_TIMEOUT_SECS)
                .await?
                .program,
            "ruby"
        );
        assert_eq!(
            resolve_interpreter("test.pl", None, DEFAULT_TIMEOUT_SECS)
                .await?
                .program,
            "perl"
        );
        assert_eq!(
            resolve_interpreter("test.php", None, DEFAULT_TIMEOUT_SECS)
                .await?
                .program,
            "php"
        );
        assert_eq!(
            resolve_interpreter("test.R", None, DEFAULT_TIMEOUT_SECS)
                .await?
                .program,
            "Rscript"
        );
        Ok(())
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
    fn test_shlex_split_malformed() {
        assert!(shlex::split(r#"--name "unterminated"#).is_none());
    }

    #[test]
    fn test_invocation_no_longer_has_shell_prefix() {
        let inv = Invocation::simple("python3");
        assert_eq!(inv.program, "python3");
        assert!(inv.prefix_args.is_empty());
    }

    #[tokio::test]
    async fn run_skill_script_remains_available_to_restricted_skill() {
        let root =
            std::env::temp_dir().join(format!("echo-skill-script-test-{}", std::process::id()));
        let skill_dir = root.join("locked-skill");
        tokio::fs::create_dir_all(skill_dir.join("scripts"))
            .await
            .unwrap();
        tokio::fs::write(skill_dir.join("SKILL.md"), "body")
            .await
            .unwrap();
        tokio::fs::write(skill_dir.join("scripts/test.py"), "print('hi')\n")
            .await
            .unwrap();

        let mut primary = SkillRegistry::new();
        primary.register_descriptor(SkillDescriptor {
            source: None,
            name: "locked-skill".into(),
            description: "desc".into(),
            location: skill_dir.join("SKILL.md"),
            license: None,
            compatibility: None,
            metadata: HashMap::new(),
            allowed_tools: vec!["read_skill_resource".into()],
            shell: None,
            paths: vec![],
            triggers: vec![],
            hooks: None,
            sandbox: None,
            depends_on: vec![],
        });
        let mut registry = primary.activation_view();
        registry.register_descriptor(SkillDescriptor {
            source: None,
            name: "locked-skill".into(),
            description: "desc".into(),
            location: skill_dir.join("SKILL.md"),
            license: None,
            compatibility: None,
            metadata: HashMap::new(),
            allowed_tools: vec!["read_skill_resource".into()],
            shell: None,
            paths: vec![],
            triggers: vec![],
            hooks: None,
            sandbox: None,
            depends_on: vec![],
        });
        primary.mark_activated("locked-skill");

        let tool = RunSkillScriptTool::new(Arc::new(RwLock::new(registry)));
        let result = tool
            .execute(
                [
                    ("skill_name".to_string(), json!("locked-skill")),
                    ("script".to_string(), json!("scripts/test.py")),
                ]
                .into(),
            )
            .await
            .unwrap();

        assert!(
            !result
                .error
                .unwrap_or_default()
                .contains("does not permit tool 'run_skill_script'")
        );

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn test_find_git_bash_returns_something_or_none() -> Result<()> {
        let _ = find_git_bash(None, DEFAULT_TIMEOUT_SECS).await?;
        Ok(())
    }
}
