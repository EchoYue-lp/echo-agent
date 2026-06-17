//! Git version control tools
//!
//! Aligning with Claude Code/Cursor, provides full Git operation capabilities:
//! - git_status: working directory status
//! - git_diff: diff comparison (unstaged + staged)
//! - git_log: commit history
//! - git_blame: line-level annotation
//! - git_branch: branch operations
//! - git_commit: create commits

use futures::future::BoxFuture;
use serde_json::Value;
use std::path::Path;
use std::process::Command;

use echo_core::error::{Result, ToolError};
use echo_core::tools::permission::ToolPermission;
use echo_core::tools::{Tool, ToolParameters, ToolResult, ToolRiskLevel, ToolRunner};

// ── Git status ──────────────────────────────────────────────────────────────

#[derive(Default, echo_macros::Tool)]
#[tool(
    name = "git_status",
    description = "View working directory status of the current repo: modified, staged, untracked files",
    risk_level = "ReadOnly"
)]
// TODO(v0.3): integrate into default tool registry once git tool suite is stabilized
#[allow(dead_code)]
pub struct GitStatusTool {
    #[tool_param(description = "Repository path (defaults to current working directory)")]
    repo_path: Option<String>,
}

impl ToolRunner<GitStatusToolParams> for GitStatusTool {
    async fn run(&self, params: GitStatusToolParams) -> Result<ToolResult> {
        let repo_path = params.repo_path.as_deref().unwrap_or(".");
        let output = run_git(&repo_path,&["status", "--short"])?;
        if output.is_empty() {
            Ok(ToolResult::success("Working directory clean, no changes"))
        } else {
            Ok(ToolResult::success(format!("Git status:\n{}", output)))
        }
    }
}

// ── Git diff ────────────────────────────────────────────────────────────────

pub struct GitDiffTool;

impl Tool for GitDiffTool {
    fn name(&self) -> &str {
        "git_diff"
    }

    fn description(&self) -> &str {
        "View code diffs: unstaged changes, staged changes, or diffs between specified branches/commits"
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Read]
    }
    fn risk_level(&self) -> ToolRiskLevel {
        ToolRiskLevel::ReadOnly
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "repo_path": {
                    "type": "string",
                    "description": "Repository path (defaults to current working directory)"
                },
                "staged": {
                    "type": "boolean",
                    "description": "Whether to view staged changes (default false)"
                },
                "target": {
                    "type": "string",
                    "description": "Comparison target: branch name, commit hash, or HEAD~1"
                },
                "file_path": {
                    "type": "string",
                    "description": "Show diff for specified file only"
                }
            },
            "required": []
        })
    }

    fn execute_with_context<'a>(
        &'a self,
        parameters: ToolParameters,
        ctx: &'a echo_core::tools::ToolContext,
    ) -> BoxFuture<'a, Result<ToolResult>> {
        Box::pin(async move {
            let repo_path = parameters
                .get("repo_path")
                .and_then(|v| v.as_str())
                .unwrap_or(".");
            let repo_path = effective_repo_path(&repo_path, ctx);

            let mut args = vec!["diff"];
            let staged = parameters
                .get("staged")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if staged {
                args.push("--staged");
            }

            let target = parameters.get("target").and_then(|v| v.as_str());
            let file_path = parameters.get("file_path").and_then(|v| v.as_str());

            // Add -- separator before user-supplied arguments to prevent
            // argument injection (e.g., target starting with -)
            if target.is_some() || file_path.is_some() {
                args.push("--");
            }
            if let Some(t) = target {
                args.push(t);
            }
            if let Some(fp) = file_path {
                args.push(fp);
            }

            let output = run_git(&repo_path,&args)?;
            if output.is_empty() {
                Ok(ToolResult::success("No differences".to_string()))
            } else {
                Ok(ToolResult::success(format!("```diff\n{}```", output)))
            }
        })
    }
}

// ── Git log ─────────────────────────────────────────────────────────────────

pub struct GitLogTool;

impl Tool for GitLogTool {
    fn name(&self) -> &str {
        "git_log"
    }

    fn description(&self) -> &str {
        "View Git commit history, with options for count limit and format"
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Read]
    }
    fn risk_level(&self) -> ToolRiskLevel {
        ToolRiskLevel::ReadOnly
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "repo_path": {
                    "type": "string",
                    "description": "Repository path (defaults to current working directory)"
                },
                "count": {
                    "type": "integer",
                    "description": "Number of commits to show (default 20)"
                },
                "oneline": {
                    "type": "boolean",
                    "description": "Single-line mode (default true)"
                },
                "author": {
                    "type": "string",
                    "description": "Filter by author"
                },
                "since": {
                    "type": "string",
                    "description": "Start date, e.g. '2024-01-01'"
                }
            },
            "required": []
        })
    }

    fn execute_with_context<'a>(
        &'a self,
        parameters: ToolParameters,
        ctx: &'a echo_core::tools::ToolContext,
    ) -> BoxFuture<'a, Result<ToolResult>> {
        Box::pin(async move {
            let repo_path = parameters
                .get("repo_path")
                .and_then(|v| v.as_str())
                .unwrap_or(".");
            let repo_path = effective_repo_path(&repo_path, ctx);

            let count = parameters
                .get("count")
                .and_then(|v| v.as_u64())
                .unwrap_or(20);
            let oneline = parameters
                .get("oneline")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);

            let mut args: Vec<&str> = vec!["log"];
            if oneline {
                args.push("--oneline");
            }
            let count_str = count.to_string();
            args.extend_from_slice(&["-n", &count_str]);

            let author = parameters.get("author").and_then(|v| v.as_str());
            let since = parameters.get("since").and_then(|v| v.as_str());
            let mut extra_args: Vec<String> = Vec::new();
            if let Some(a) = author {
                extra_args.push(format!("--author={}", a));
            }
            if let Some(s) = since {
                extra_args.push(format!("--since={}", s));
            }
            let extra_strs: Vec<&str> = extra_args.iter().map(|s| s.as_str()).collect();
            args.extend(&extra_strs);

            let output = run_git(&repo_path,&args)?;
            if output.is_empty() {
                Ok(ToolResult::success(
                    "Repository has no commit history".to_string(),
                ))
            } else {
                Ok(ToolResult::success(format!("Commit history:\n{}", output)))
            }
        })
    }
}

// ── Git blame ───────────────────────────────────────────────────────────────

pub struct GitBlameTool;

impl Tool for GitBlameTool {
    fn name(&self) -> &str {
        "git_blame"
    }

    fn description(&self) -> &str {
        "View line-by-line annotations for a file, showing the last author and commit for each line"
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Read]
    }
    fn risk_level(&self) -> ToolRiskLevel {
        ToolRiskLevel::ReadOnly
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "File path to inspect (relative to repo root)"
                },
                "repo_path": {
                    "type": "string",
                    "description": "Repository path (defaults to current working directory)"
                },
                "start_line": {
                    "type": "integer",
                    "description": "Start line number"
                },
                "end_line": {
                    "type": "integer",
                    "description": "End line number"
                }
            },
            "required": ["file_path"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let file_path = parameters
                .get("file_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("file_path".to_string()))?;
            let repo_path = parameters
                .get("repo_path")
                .and_then(|v| v.as_str())
                .unwrap_or(".");

            let mut args: Vec<String> = vec!["blame".to_string()];
            if let (Some(start), Some(end)) = (
                parameters.get("start_line").and_then(|v| v.as_u64()),
                parameters.get("end_line").and_then(|v| v.as_u64()),
            ) {
                args.push("-L".to_string());
                args.push(format!("{},{}", start, end));
            }
            // Add -- separator before user-supplied file path to prevent argument injection
            args.push("--".to_string());
            args.push(file_path.to_string());

            let str_args: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            let output = run_git(&repo_path,&str_args)?;
            Ok(ToolResult::success(output))
        })
    }
}

// ── Git branch ──────────────────────────────────────────────────────────────

pub struct GitBranchTool;

impl Tool for GitBranchTool {
    fn name(&self) -> &str {
        "git_branch"
    }

    fn description(&self) -> &str {
        "View, create, or switch Git branches. No args lists all branches; with name creates a new branch"
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Read]
    }
    fn risk_level(&self) -> ToolRiskLevel {
        ToolRiskLevel::ReadOnly
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "repo_path": {
                    "type": "string",
                    "description": "Repository path (defaults to current working directory)"
                },
                "name": {
                    "type": "string",
                    "description": "New branch name (creates a branch if provided)"
                },
                "switch": {
                    "type": "string",
                    "description": "Switch to the specified branch"
                },
                "delete": {
                    "type": "string",
                    "description": "Delete the specified branch (must be merged)"
                }
            },
            "required": []
        })
    }

    fn execute_with_context<'a>(
        &'a self,
        parameters: ToolParameters,
        ctx: &'a echo_core::tools::ToolContext,
    ) -> BoxFuture<'a, Result<ToolResult>> {
        Box::pin(async move {
            let repo_path = parameters
                .get("repo_path")
                .and_then(|v| v.as_str())
                .unwrap_or(".");
            let repo_path = effective_repo_path(&repo_path, ctx);

            let action;
            let args: Vec<String>;

            if let Some(name) = parameters.get("name").and_then(|v| v.as_str()) {
                args = vec!["branch".to_string(), name.to_string()];
                action = format!("Create branch '{}'", name);
            } else if let Some(target) = parameters.get("switch").and_then(|v| v.as_str()) {
                args = vec!["checkout".to_string(), target.to_string()];
                action = format!("Switch to branch '{}'", target);
            } else if let Some(target) = parameters.get("delete").and_then(|v| v.as_str()) {
                args = vec!["branch".to_string(), "-d".to_string(), target.to_string()];
                action = format!("Delete branch '{}'", target);
            } else {
                args = vec!["branch".to_string(), "-a".to_string()];
                action = "List all branches".to_string();
            }

            let str_args: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            let output = run_git(&repo_path,&str_args)?;
            Ok(ToolResult::success(format!("{}:\n{}", action, output)))
        })
    }
}

// ── Git commit ──────────────────────────────────────────────────────────────

pub struct GitCommitTool;

impl Tool for GitCommitTool {
    fn name(&self) -> &str {
        "git_commit"
    }

    fn description(&self) -> &str {
        "Create a Git commit. Files must be staged via git add first. Only call this tool when explicitly requested by the user."
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Write, ToolPermission::Execute]
    }
    fn risk_level(&self) -> ToolRiskLevel {
        ToolRiskLevel::Dangerous
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "repo_path": {
                    "type": "string",
                    "description": "Repository path (defaults to current working directory)"
                },
                "message": {
                    "type": "string",
                    "description": "Commit message"
                },
                "files": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "List of files to stage and commit (empty = commit all staged)"
                }
            },
            "required": ["message"]
        })
    }

    fn execute_with_context<'a>(
        &'a self,
        parameters: ToolParameters,
        ctx: &'a echo_core::tools::ToolContext,
    ) -> BoxFuture<'a, Result<ToolResult>> {
        Box::pin(async move {
            let repo_path = parameters
                .get("repo_path")
                .and_then(|v| v.as_str())
                .unwrap_or(".");
            let repo_path = effective_repo_path(&repo_path, ctx);
            let message = parameters
                .get("message")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("message".to_string()))?;

            // If file list is specified, run git add first
            if let Some(files) = parameters.get("files").and_then(|v| v.as_array()) {
                for f_val in files {
                    if let Some(f) = f_val.as_str() {
                        let add_args = ["add", f];
                        run_git(&repo_path,&add_args)?;
                    }
                }
            }

            let commit_args = ["commit", "-m", message];
            let output = run_git(&repo_path,&commit_args)?;
            Ok(ToolResult::success(format!(
                "Commit succeeded:\n{}",
                output
            )))
        })
    }
}

// ── Helper ──────────────────────────────────────────────────────────────────

/// Resolve the effective repository path for a git tool invocation.
///
/// When the LLM passes no `repo_path` (the default `"."`), run the git
/// command inside the session-bound `working_dir` (e.g. a worktree) if one
/// is set, so git operations target the isolated checkout instead of the
/// process cwd. An explicit user-supplied `repo_path` always wins.
fn effective_repo_path(user_repo_path: &str, ctx: &echo_core::tools::ToolContext) -> String {
    // When working_dir is set (worktree binding), resolve relative paths
    // against it — matching file tool semantics. Absolute paths and "." pass
    // through ("." resolves to working_dir when set).
    if user_repo_path == "." {
        if let Some(dir) = &ctx.working_dir {
            return dir.to_string_lossy().to_string();
        }
        return ".".to_string();
    }
    // Relative path: join with working_dir if set
    if !std::path::Path::new(user_repo_path).is_absolute() {
        if let Some(dir) = &ctx.working_dir {
            return dir.join(user_repo_path).to_string_lossy().to_string();
        }
    }
    user_repo_path.to_string()
}

fn run_git(repo_path: &str, args: &[&str]) -> Result<String> {
    // Validate repo_path: reject path traversal and ensure the path exists
    let path = Path::new(repo_path);
    if path
        .components()
        .any(|c| c == std::path::Component::ParentDir)
    {
        return Err(ToolError::ExecutionFailed {
            tool: "git".to_string(),
            message: format!(
                "Invalid repo_path: path traversal not allowed: {}",
                repo_path
            ),
        }
        .into());
    }
    if repo_path != "." && !path.is_dir() {
        return Err(ToolError::ExecutionFailed {
            tool: "git".to_string(),
            message: format!(
                "repo_path does not exist or is not a directory: {}",
                repo_path
            ),
        }
        .into());
    }

    let output = Command::new("git")
        .current_dir(repo_path)
        .args(args)
        .output()
        .map_err(|e| ToolError::ExecutionFailed {
            tool: "git".to_string(),
            message: format!(
                "Unable to execute git command (please verify git is installed): {}",
                e
            ),
        })?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(ToolError::ExecutionFailed {
            tool: "git".to_string(),
            message: stderr.trim().to_string(),
        }
        .into())
    }
}
