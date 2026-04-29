//! Git 版本控制工具
//!
//! 对标 Claude Code/Cursor，提供完整 Git 操作能力：
//! - git_status: 工作区状态
//! - git_diff: 差异比较（unstaged + staged）
//! - git_log: 提交历史
//! - git_blame: 逐行归属
//! - git_branch: 分支操作
//! - git_commit: 创建提交

use futures::future::BoxFuture;
use serde_json::Value;
use std::process::Command;

use crate::error::{Result, ToolError};
use crate::tools::{Tool, ToolParameters, ToolResult};

// ── Git status ──────────────────────────────────────────────────────────────

pub struct GitStatusTool;

impl Tool for GitStatusTool {
    fn name(&self) -> &str {
        "git_status"
    }

    fn description(&self) -> &str {
        "查看当前仓库的工作区状态：已修改、已暂存、未跟踪的文件列表"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "repo_path": {
                    "type": "string",
                    "description": "仓库路径（默认当前运行目录）"
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

            let output = run_git(repo_path, &["status", "--short"])?;
            if output.is_empty() {
                Ok(ToolResult::success("工作区干净，没有变更".to_string()))
            } else {
                Ok(ToolResult::success(format!("Git 状态:\n{}", output)))
            }
        })
    }
}

// ── Git diff ────────────────────────────────────────────────────────────────

pub struct GitDiffTool;

impl Tool for GitDiffTool {
    fn name(&self) -> &str {
        "git_diff"
    }

    fn description(&self) -> &str {
        "查看代码差异：unstaged 变更、staged 变更、或指定分支/提交之间的差异"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "repo_path": {
                    "type": "string",
                    "description": "仓库路径（默认当前运行目录）"
                },
                "staged": {
                    "type": "boolean",
                    "description": "是否查看已暂存的变更（默认 false）"
                },
                "target": {
                    "type": "string",
                    "description": "比较目标：分支名、commit hash、或 HEAD~1"
                },
                "file_path": {
                    "type": "string",
                    "description": "只显示指定文件的差异"
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

            if let Some(t) = target {
                args.push(t);
            }
            if let Some(fp) = file_path {
                args.push("--");
                args.push(fp);
            }

            let output = run_git(repo_path, &args)?;
            if output.is_empty() {
                Ok(ToolResult::success("没有差异".to_string()))
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
        "查看 Git 提交历史，支持限制条数和格式选择"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "repo_path": {
                    "type": "string",
                    "description": "仓库路径（默认当前运行目录）"
                },
                "count": {
                    "type": "integer",
                    "description": "显示的提交条数（默认 20）"
                },
                "oneline": {
                    "type": "boolean",
                    "description": "单行模式（默认 true）"
                },
                "author": {
                    "type": "string",
                    "description": "按作者筛选"
                },
                "since": {
                    "type": "string",
                    "description": "起始日期，如 '2024-01-01'"
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

            let output = run_git(repo_path, &args)?;
            if output.is_empty() {
                Ok(ToolResult::success("仓库没有提交记录".to_string()))
            } else {
                Ok(ToolResult::success(format!("提交历史:\n{}", output)))
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
        "查看文件的逐行注释，显示每行代码的最后修改者和提交"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "要查看的文件路径（相对于仓库根目录）"
                },
                "repo_path": {
                    "type": "string",
                    "description": "仓库路径（默认当前运行目录）"
                },
                "start_line": {
                    "type": "integer",
                    "description": "起始行号"
                },
                "end_line": {
                    "type": "integer",
                    "description": "结束行号"
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
            args.push(file_path.to_string());

            let str_args: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            let output = run_git(repo_path, &str_args)?;
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
        "查看、创建或切换 Git 分支。不带参数列出所有分支，带 name 创建新分支"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "repo_path": {
                    "type": "string",
                    "description": "仓库路径（默认当前运行目录）"
                },
                "name": {
                    "type": "string",
                    "description": "新分支名称（如提供则创建分支）"
                },
                "switch": {
                    "type": "string",
                    "description": "切换到指定分支"
                },
                "delete": {
                    "type": "string",
                    "description": "删除指定分支（需要已合并）"
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

            let action;
            let args: Vec<String>;

            if let Some(name) = parameters.get("name").and_then(|v| v.as_str()) {
                args = vec!["branch".to_string(), name.to_string()];
                action = format!("创建分支 '{}'", name);
            } else if let Some(target) = parameters.get("switch").and_then(|v| v.as_str()) {
                args = vec!["checkout".to_string(), target.to_string()];
                action = format!("切换到分支 '{}'", target);
            } else if let Some(target) = parameters.get("delete").and_then(|v| v.as_str()) {
                args = vec!["branch".to_string(), "-d".to_string(), target.to_string()];
                action = format!("删除分支 '{}'", target);
            } else {
                args = vec!["branch".to_string(), "-a".to_string()];
                action = "列出所有分支".to_string();
            }

            let str_args: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            let output = run_git(repo_path, &str_args)?;
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
        "创建 Git 提交。需要先通过 git add 暂存文件。仅当用户明确请求时才调用此工具。"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "repo_path": {
                    "type": "string",
                    "description": "仓库路径（默认当前运行目录）"
                },
                "message": {
                    "type": "string",
                    "description": "提交信息"
                },
                "files": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "要暂存并提交的文件列表（空 = 提交所有已暂存的）"
                }
            },
            "required": ["message"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let repo_path = parameters
                .get("repo_path")
                .and_then(|v| v.as_str())
                .unwrap_or(".");
            let message = parameters
                .get("message")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("message".to_string()))?;

            // 如果指定了文件列表，先 git add
            if let Some(files) = parameters.get("files").and_then(|v| v.as_array()) {
                for f_val in files {
                    if let Some(f) = f_val.as_str() {
                        let add_args = ["add", f];
                        run_git(repo_path, &add_args)?;
                    }
                }
            }

            let commit_args = ["commit", "-m", message];
            let output = run_git(repo_path, &commit_args)?;
            Ok(ToolResult::success(format!("提交成功:\n{}", output)))
        })
    }
}

// ── Helper ──────────────────────────────────────────────────────────────────

fn run_git(repo_path: &str, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .current_dir(repo_path)
        .args(args)
        .output()
        .map_err(|e| ToolError::ExecutionFailed {
            tool: "git".to_string(),
            message: format!("无法执行 git 命令（请确认已安装 git）: {}", e),
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
