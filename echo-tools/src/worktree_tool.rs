//! Worktree tool wrappers — expose git worktree management as agent-callable tools.
//!
//! Provides three tools for parallel sub-agent isolation:
//! - `enter_worktree`: Create a new git worktree for isolated parallel work
//! - `exit_worktree`: Remove a managed worktree (optionally merging changes back)
//! - `list_worktrees`: List all worktrees in the repository

use futures::future::BoxFuture;
use serde_json::Value;
use std::path::Path;

use echo_core::error::{Result, ToolError};
use echo_core::tools::permission::ToolPermission;
use echo_core::tools::{Tool, ToolParameters, ToolResult, ToolRiskLevel};

use crate::git_worktree::{
    WorktreeConfig, create_worktree, list_worktrees, merge_worktree, remove_worktree,
};

// ── Enter Worktree ──────────────────────────────────────────────────────────

/// Creates a new git worktree for isolated parallel work.
///
/// When multiple sub-agents need to work on the same repository simultaneously,
/// each should create its own worktree to avoid file conflicts. Worktrees share
/// the same .git object store, so they are lightweight.
pub struct EnterWorktreeTool;

impl Tool for EnterWorktreeTool {
    fn name(&self) -> &str {
        "enter_worktree"
    }

    fn description(&self) -> &str {
        "Create a new git worktree for isolated parallel work. \
         Use this when a sub-agent needs to work on a separate branch \
         without conflicting with other agents or the main working tree. \
         NOTE: this only creates the worktree on disk — it does NOT switch the \
         agent's runtime working directory. To run subsequent tools inside the \
         new worktree, the host must bind it as the session working_dir \
         (e.g. via the /worktree command), not via this tool."
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Write, ToolPermission::Execute]
    }

    fn risk_level(&self) -> ToolRiskLevel {
        ToolRiskLevel::Standard
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "branch": {
                    "type": "string",
                    "description": "Branch name for the new worktree (created if it does not exist)"
                },
                "base": {
                    "type": "string",
                    "description": "Base branch or commit to create from (defaults to HEAD)"
                },
                "path_suffix": {
                    "type": "string",
                    "description": "Optional custom directory name under .worktrees/ (defaults to sanitized branch name)"
                },
                "repo_path": {
                    "type": "string",
                    "description": "Repository path (defaults to current working directory)"
                }
            },
            "required": ["branch"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let branch = parameters
                .get("branch")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("branch".to_string()))?;
            let base = parameters
                .get("base")
                .and_then(|v| v.as_str())
                .map(String::from);
            let path_suffix = parameters
                .get("path_suffix")
                .and_then(|v| v.as_str())
                .map(String::from);
            let repo_path = parameters
                .get("repo_path")
                .and_then(|v| v.as_str())
                .unwrap_or(".");

            let config = WorktreeConfig {
                branch: branch.to_string(),
                base,
                path_suffix,
            };

            let worktree = create_worktree(Path::new(repo_path), &config).map_err(|e| {
                ToolError::ExecutionFailed {
                    tool: "enter_worktree".to_string(),
                    message: e,
                }
            })?;

            let msg = format!(
                "Created worktree at '{}' on branch '{}'. \
                 Use this directory as the working root for the sub-agent. \
                 When done, call exit_worktree to clean up.",
                worktree.path.display(),
                worktree.branch
            );

            let mut result = ToolResult::success(msg);
            result.metadata.insert(
                "worktree_path".to_string(),
                worktree.path.to_string_lossy().to_string(),
            );
            result
                .metadata
                .insert("branch".to_string(), worktree.branch.clone());
            Ok(result)
        })
    }
}

// ── Exit Worktree ───────────────────────────────────────────────────────────

/// Removes a managed worktree, optionally merging its changes back.
pub struct ExitWorktreeTool;

impl Tool for ExitWorktreeTool {
    fn name(&self) -> &str {
        "exit_worktree"
    }

    fn description(&self) -> &str {
        "Remove a git worktree and clean up its branch. \
         Optionally merge the worktree branch into a target branch before removal."
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
                "worktree_path": {
                    "type": "string",
                    "description": "Path to the worktree directory to remove"
                },
                "merge_to": {
                    "type": "string",
                    "description": "If set, merge the worktree branch into this target branch before removal"
                },
                "repo_path": {
                    "type": "string",
                    "description": "Repository path (defaults to current working directory)"
                }
            },
            "required": ["worktree_path"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let worktree_path = parameters
                .get("worktree_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("worktree_path".to_string()))?;
            let merge_to = parameters.get("merge_to").and_then(|v| v.as_str());
            let repo_path = parameters
                .get("repo_path")
                .and_then(|v| v.as_str())
                .unwrap_or(".");

            // Determine the branch name from the worktree path.
            // We look up the worktree in the list to get its branch.
            let worktrees =
                list_worktrees(Path::new(repo_path)).map_err(|e| ToolError::ExecutionFailed {
                    tool: "exit_worktree".to_string(),
                    message: e,
                })?;

            let requested = std::fs::canonicalize(worktree_path).map_err(|error| {
                ToolError::ExecutionFailed {
                    tool: "exit_worktree".to_string(),
                    message: format!("Failed to resolve worktree path: {error}"),
                }
            })?;
            let wt = worktrees
                .iter()
                .find(|worktree| {
                    std::fs::canonicalize(&worktree.path).is_ok_and(|path| path == requested)
                })
                .cloned()
                .ok_or_else(|| ToolError::ExecutionFailed {
                    tool: "exit_worktree".to_string(),
                    message: "Refusing to remove an unknown or unregistered worktree".to_string(),
                })?;

            // Optionally merge before removal
            let merge_msg = if let Some(target) = merge_to {
                let msg = merge_worktree(Path::new(repo_path), &wt, target).map_err(|e| {
                    ToolError::ExecutionFailed {
                        tool: "exit_worktree".to_string(),
                        message: e,
                    }
                })?;
                Some(msg)
            } else {
                None
            };

            // Remove the worktree
            remove_worktree(Path::new(repo_path), &wt).map_err(|e| ToolError::ExecutionFailed {
                tool: "exit_worktree".to_string(),
                message: e,
            })?;

            let msg = match merge_msg {
                Some(m) => format!("{m}. Worktree at '{}' removed.", worktree_path),
                None => format!(
                    "Worktree at '{}' removed; branch '{}' was preserved.",
                    worktree_path, wt.branch
                ),
            };
            Ok(ToolResult::success(msg))
        })
    }
}

// ── List Worktrees ──────────────────────────────────────────────────────────

/// Lists all git worktrees in the repository.
pub struct ListWorktreesTool;

impl Tool for ListWorktreesTool {
    fn name(&self) -> &str {
        "list_worktrees"
    }

    fn description(&self) -> &str {
        "List all git worktrees in the repository, showing their paths and branches."
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
                }
            },
            "required": []
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let repo_path = parameters
                .get("repo_path")
                .and_then(|v| v.as_str())
                .unwrap_or(".");

            let worktrees =
                list_worktrees(Path::new(repo_path)).map_err(|e| ToolError::ExecutionFailed {
                    tool: "list_worktrees".to_string(),
                    message: e,
                })?;

            if worktrees.is_empty() {
                return Ok(ToolResult::success(
                    "No worktrees found in this repository.",
                ));
            }

            let mut lines = Vec::with_capacity(worktrees.len());
            for wt in &worktrees {
                lines.push(format!(
                    "  {} (branch: {})",
                    wt.path.display(),
                    if wt.branch.is_empty() {
                        "detached"
                    } else {
                        &wt.branch
                    }
                ));
            }
            Ok(ToolResult::success(format!(
                "Worktrees ({}):\n{}",
                worktrees.len(),
                lines.join("\n")
            )))
        })
    }
}
