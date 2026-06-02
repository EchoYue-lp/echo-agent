//! demo58: Git Worktree 隔离 — 并行子 Agent 的安全工作区
//!
//! 演示 git worktree 管理和 checkpoint 机制：
//! 1. 列出当前仓库的 worktree
//! 2. 创建新 worktree 用于并行子 Agent 工作
//! 3. 再次列出 worktree 确认创建成功
//! 4. 移除 worktree 并清理分支
//! 5. 创建 git checkpoint（轻量标签）
//!
//! 需要：
//! - 当前目录在 git 仓库内（echo-agent 项目本身即可）
//!
//! ```bash
//! cargo run --example demo58_git_worktree --features git
//! ```

use echo_tools::git_checkpoint::{cleanup_old_checkpoints, create_checkpoint};
use echo_tools::git_worktree::{WorktreeConfig, create_worktree, list_worktrees, remove_worktree};
use std::path::Path;

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    println!("═══════════════════════════════════════════════════════");
    println!("    demo58: Git Worktree 隔离 + Checkpoint");
    println!("═══════════════════════════════════════════════════════\n");

    // Use the echo-agent repo root as the working repository.
    let repo_path = Path::new(env!("CARGO_MANIFEST_DIR"));
    println!("  仓库路径: {}\n", repo_path.display());

    // ── Part 1：列出现有 worktree ──────────────────────────────────────────
    println!("───────────────────────────────────────────────────────");
    println!("Part 1：列出现有 worktree");
    println!("───────────────────────────────────────────────────────\n");

    let worktrees_before = list_worktrees(repo_path)?;
    println!("  当前 worktree 数量: {}", worktrees_before.len());
    for wt in &worktrees_before {
        let branch = if wt.branch.is_empty() {
            "detached"
        } else {
            &wt.branch
        };
        println!("    {} (branch: {})", wt.path.display(), branch);
    }
    println!();

    // ── Part 2：创建新 worktree ────────────────────────────────────────────
    println!("───────────────────────────────────────────────────────");
    println!("Part 2：创建新 worktree（模拟并行子 Agent）");
    println!("───────────────────────────────────────────────────────\n");

    let branch_name = "demo58-worktree-example";
    let config = WorktreeConfig {
        branch: branch_name.to_string(),
        base: Some("HEAD".to_string()),
        path_suffix: Some("demo58-isolated".to_string()),
    };

    println!("  创建 worktree…");
    println!("    branch      : {}", config.branch);
    println!("    base        : {:?}", config.base);
    println!("    path_suffix : {:?}", config.path_suffix);
    println!();

    let worktree = match create_worktree(repo_path, &config) {
        Ok(wt) => {
            println!("  ✓ worktree 创建成功");
            println!("    path   : {}", wt.path.display());
            println!("    branch : {}", wt.branch);
            println!("    managed: {}", wt.managed);
            wt
        }
        Err(e) => {
            // May fail if branch already exists from a previous interrupted run.
            println!("  ⚠ worktree 创建失败（可能上次运行未清理）: {e}");
            println!("  尝试清理残留…");
            let _ = std::process::Command::new("git")
                .args(["worktree", "prune"])
                .current_dir(repo_path)
                .output();
            let _ = std::process::Command::new("git")
                .args(["branch", "-D", branch_name])
                .current_dir(repo_path)
                .output();
            // Retry
            let wt = create_worktree(repo_path, &config)?;
            println!("  ✓ 重试成功: {}", wt.path.display());
            wt
        }
    };
    println!();

    // ── Part 3：再次列出确认 ───────────────────────────────────────────────
    println!("───────────────────────────────────────────────────────");
    println!("Part 3：再次列出 worktree（确认新增）");
    println!("───────────────────────────────────────────────────────\n");

    let worktrees_after = list_worktrees(repo_path)?;
    println!(
        "  worktree 数量: {} → {}",
        worktrees_before.len(),
        worktrees_after.len()
    );
    for wt in &worktrees_after {
        let branch = if wt.branch.is_empty() {
            "detached"
        } else {
            &wt.branch
        };
        let marker = if wt.path == worktree.path {
            " ← 新建"
        } else {
            ""
        };
        println!("    {} (branch: {}){}", wt.path.display(), branch, marker);
    }
    println!();

    // ── Part 4：移除 worktree ──────────────────────────────────────────────
    println!("───────────────────────────────────────────────────────");
    println!("Part 4：移除 worktree 并清理分支");
    println!("───────────────────────────────────────────────────────\n");

    remove_worktree(repo_path, &worktree)?;
    println!("  ✓ worktree 已移除: {}", worktree.path.display());
    println!("  ✓ 分支 '{}' 已删除", worktree.branch);
    println!();

    // ── Part 5：Git Checkpoint ─────────────────────────────────────────────
    println!("───────────────────────────────────────────────────────");
    println!("Part 5：Git Checkpoint（文件修改前的安全快照）");
    println!("───────────────────────────────────────────────────────\n");

    println!("  checkpoint 会在修改文件前为当前 HEAD 创建轻量标签，");
    println!("  方便在出错时回滚到修改前的状态。\n");

    let checkpoint = create_checkpoint(repo_path);
    match &checkpoint {
        Some(tag) => {
            println!("  ✓ Checkpoint 创建成功: {}", tag);
            println!("    可用 `git checkout {tag} -- .` 回滚");
        }
        None => {
            println!("  ⚠ 未创建 checkpoint（可能不在 git 仓库中）");
        }
    }
    println!();

    // Clean up old checkpoints, keeping last 5
    cleanup_old_checkpoints(repo_path, 5);
    println!("  已清理旧 checkpoint（保留最近 5 个）");

    // ── Summary ─────────────────────────────────────────────────────────
    println!("\n───────────────────────────────────────────────────────");
    println!("架构说明");
    println!("───────────────────────────────────────────────────────\n");
    println!("  Worktree 隔离的核心场景：");
    println!("    多个子 Agent 同时修改同一仓库时，每个 Agent 创建独立 worktree，");
    println!("    避免文件冲突。Worktree 共享 .git 对象存储，因此非常轻量。");
    println!();
    println!("  Checkpoint 的核心场景：");
    println!("    Agent 在修改文件前自动创建 checkpoint 标签，");
    println!("    如果修改导致问题，可以一键回滚到 checkpoint 状态。");

    println!("\n═══════════════════════════════════════════════════════");
    println!("    demo58 完成");
    println!("═══════════════════════════════════════════════════════");

    Ok(())
}
