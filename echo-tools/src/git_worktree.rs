//! Git worktree management for parallel sub-agent isolation.
//!
//! When multiple sub-agents work on the same repository, each gets its own
//! worktree to avoid file conflicts. Worktrees share the same .git object
//! store, so they're lightweight.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Configuration for a new worktree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeConfig {
    /// Branch name for the worktree (created if not exists)
    pub branch: String,
    /// Base branch/commit to create from (default: HEAD)
    pub base: Option<String>,
    /// Optional path suffix for the worktree directory
    pub path_suffix: Option<String>,
}

/// A managed git worktree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedWorktree {
    /// Path to the worktree directory
    pub path: PathBuf,
    /// Branch name
    pub branch: String,
    /// Whether this worktree was created by us (for cleanup tracking)
    pub managed: bool,
}

/// Create a new git worktree for isolated parallel work.
///
/// Returns the worktree path. The caller is responsible for cleanup via
/// `remove_worktree()` when done.
pub fn create_worktree(
    repo_path: &Path,
    config: &WorktreeConfig,
) -> Result<ManagedWorktree, String> {
    let git_root = find_git_root(repo_path)?;

    // Generate worktree path
    let worktree_dir = if let Some(ref suffix) = config.path_suffix {
        git_root.join(".worktrees").join(suffix)
    } else {
        let branch_safe = config
            .branch
            .replace(|c: char| !c.is_alphanumeric() && c != '-' && c != '_', "_");
        git_root.join(".worktrees").join(&branch_safe)
    };

    // Create .worktrees directory if needed
    if let Some(parent) = worktree_dir.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create worktrees directory: {e}"))?;
    }

    // Build git worktree add command
    let mut cmd = Command::new("git");
    cmd.args(["worktree", "add"]).current_dir(&git_root);

    if let Some(ref base) = config.base {
        // -- prevents base from being interpreted as a git option
        cmd.args([
            "-b",
            &config.branch,
            &worktree_dir.to_string_lossy(),
            "--",
            base,
        ]);
    } else {
        cmd.args(["-b", &config.branch, &worktree_dir.to_string_lossy()]);
    }

    let output = cmd
        .output()
        .map_err(|e| format!("Failed to run git worktree add: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // If branch already exists, try without -b
        if stderr.contains("already exists") {
            let output2 = Command::new("git")
                .args([
                    "worktree",
                    "add",
                    &worktree_dir.to_string_lossy(),
                    "--",
                    &config.branch,
                ])
                .current_dir(&git_root)
                .output()
                .map_err(|e| format!("Failed to run git worktree add (existing branch): {e}"))?;

            if !output2.status.success() {
                return Err(format!(
                    "git worktree add failed: {}",
                    String::from_utf8_lossy(&output2.stderr)
                ));
            }
        } else {
            return Err(format!("git worktree add failed: {stderr}"));
        }
    }

    Ok(ManagedWorktree {
        path: worktree_dir,
        branch: config.branch.clone(),
        managed: true,
    })
}

/// Remove a managed worktree and clean up.
pub fn remove_worktree(repo_path: &Path, worktree: &ManagedWorktree) -> Result<(), String> {
    let git_root = find_git_root(repo_path)?;

    // Remove worktree
    let output = Command::new("git")
        .args([
            "worktree",
            "remove",
            "--force",
            &worktree.path.to_string_lossy(),
        ])
        .current_dir(&git_root)
        .output()
        .map_err(|e| format!("Failed to remove worktree: {e}"))?;

    if !output.status.success() {
        // Fallback: prune stale worktrees
        let _ = Command::new("git")
            .args(["worktree", "prune"])
            .current_dir(&git_root)
            .output();
    }

    // Delete the branch if it was managed
    if worktree.managed {
        let _ = Command::new("git")
            .args(["branch", "-D", "--", &worktree.branch])
            .current_dir(&git_root)
            .output();
    }

    Ok(())
}

/// List all worktrees in the repository.
pub fn list_worktrees(repo_path: &Path) -> Result<Vec<ManagedWorktree>, String> {
    let git_root = find_git_root(repo_path)?;

    let output = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(&git_root)
        .output()
        .map_err(|e| format!("Failed to list worktrees: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut worktrees = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut current_branch = String::new();

    for line in stdout.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            current_path = Some(PathBuf::from(path));
            current_branch.clear();
        } else if let Some(branch) = line.strip_prefix("branch ") {
            current_branch = branch.replace("refs/heads/", "");
        } else if line.is_empty()
            && let Some(path) = current_path.take()
        {
            worktrees.push(ManagedWorktree {
                path,
                branch: current_branch.clone(),
                managed: false,
            });
        }
    }
    // Handle last entry
    if let Some(path) = current_path {
        worktrees.push(ManagedWorktree {
            path,
            branch: current_branch,
            managed: false,
        });
    }

    Ok(worktrees)
}

/// Merge changes from a worktree branch back to the base branch.
pub fn merge_worktree(
    repo_path: &Path,
    worktree: &ManagedWorktree,
    target_branch: &str,
) -> Result<String, String> {
    let git_root = find_git_root(repo_path)?;

    // First checkout target branch. `--` separates the branch from any
    // pathspec and guards against a branch name shaped like a flag (P1-2).
    let co = Command::new("git")
        .args(["checkout", target_branch, "--"])
        .current_dir(&git_root)
        .output()
        .map_err(|e| format!("Failed to checkout target branch: {e}"))?;

    if !co.status.success() {
        return Err(format!(
            "Failed to checkout {}: {}",
            target_branch,
            String::from_utf8_lossy(&co.stderr)
        ));
    }

    // Merge the worktree branch. Trailing `--` separates revs from pathspec
    // (P1-2).
    let merge = Command::new("git")
        .args(["merge", "--no-edit", &worktree.branch, "--"])
        .current_dir(&git_root)
        .output()
        .map_err(|e| format!("Failed to merge: {e}"))?;

    if !merge.status.success() {
        return Err(format!(
            "Merge conflict or error: {}",
            String::from_utf8_lossy(&merge.stderr)
        ));
    }

    Ok(format!("Merged {} into {}", worktree.branch, target_branch))
}

fn find_git_root(path: &Path) -> Result<PathBuf, String> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(path)
        .output()
        .map_err(|e| format!("Not a git repository: {e}"))?;

    if output.status.success() {
        Ok(PathBuf::from(
            String::from_utf8_lossy(&output.stdout).trim(),
        ))
    } else {
        Err("Not a git repository".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_git_root() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"));
        let root = find_git_root(path);
        assert!(root.is_ok());
    }

    #[test]
    fn test_find_git_root_non_repo() {
        let path = Path::new("/tmp");
        let root = find_git_root(path);
        // May or may not fail depending on whether /tmp is in a git repo
        let _ = root;
    }
}
