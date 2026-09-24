//! Git worktree management for parallel subagent isolation.
//!
//! When multiple subagents work on the same repository, each gets its own
//! worktree to avoid file conflicts. Worktrees share the same .git object
//! store, so they're lightweight.

use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::process::Command;
use std::time::Duration;

use echo_core::tools::ToolContext;
use echo_core::utils::fs::{
    ExistingDirectoryGuard, open_existing_directory_guard, verify_existing_directory,
};

use crate::process::BoundedProcessOutput;

const MANAGED_MARKER: &str = "echo-agent-managed";
const GIT_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_GIT_OUTPUT_BYTES: usize = 1024 * 1024;

enum CheckoutRef {
    Branch(String),
    Commit(String),
}

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

struct WorktreeCreationGuards {
    checkout: ExistingDirectoryGuard,
    admin_path: PathBuf,
    admin: ExistingDirectoryGuard,
}

impl WorktreeCreationGuards {
    fn capture(checkout: ExistingDirectoryGuard, marker_path: &Path) -> Result<Self, String> {
        let admin_path = marker_path
            .parent()
            .ok_or_else(|| "Worktree marker has no Git administration directory".to_string())?
            .to_path_buf();
        let admin = open_existing_directory_guard(&admin_path)
            .map_err(|error| format!("Cannot retain Git administration identity: {error}"))?;
        Ok(Self {
            checkout,
            admin_path,
            admin,
        })
    }

    fn verify(&self, worktree_dir: &Path) -> Result<(), String> {
        verify_existing_directory(worktree_dir, &self.checkout)
            .map_err(|error| format!("Checkout identity changed: {error}"))?;
        verify_existing_directory(&self.admin_path, &self.admin)
            .map_err(|error| format!("Git administration identity changed: {error}"))
    }
}

struct CreatedWorktree {
    worktree: ManagedWorktree,
    guards: WorktreeCreationGuards,
}

/// Create a new git worktree for isolated parallel work.
///
/// Returns the worktree path. The caller is responsible for cleanup via
/// `remove_worktree()` when done.
pub async fn create_worktree(
    repo_path: &Path,
    config: &WorktreeConfig,
) -> Result<ManagedWorktree, String> {
    create_worktree_with_context(repo_path, config, &ToolContext::default()).await
}

pub async fn create_worktree_with_context(
    repo_path: &Path,
    config: &WorktreeConfig,
    context: &ToolContext,
) -> Result<ManagedWorktree, String> {
    let git_root = find_git_root(repo_path, context).await?;

    // Generate worktree path
    let worktrees_root = git_root.join(".worktrees");
    let worktree_dir = if let Some(ref suffix) = config.path_suffix {
        echo_core::utils::fs::join_path_segment(&worktrees_root, suffix)
            .map_err(|error| format!("Invalid worktree path suffix: {error}"))?
    } else {
        let branch_safe = config
            .branch
            .replace(|c: char| !c.is_alphanumeric() && c != '-' && c != '_', "_");
        git_root.join(".worktrees").join(&branch_safe)
    };

    if context
        .cancel
        .as_ref()
        .is_some_and(|cancel| cancel.is_cancelled())
    {
        return Err("Worktree creation was cancelled before resource reservation".to_string());
    }

    if let Some(parent) = worktree_dir.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create worktrees directory: {e}"))?;
    }
    std::fs::create_dir(&worktree_dir).map_err(|error| {
        format!(
            "Refusing to create worktree at an existing or unavailable path {}: {error}",
            worktree_dir.display()
        )
    })?;
    let checkout_guard = open_existing_directory_guard(&worktree_dir).map_err(|error| {
        let _ = std::fs::remove_dir(&worktree_dir);
        format!(
            "Cannot retain reserved worktree identity at {}: {error}",
            worktree_dir.display()
        )
    })?;

    // Git add may commit before its client reports a result. The owner must
    // finish marker publication or bounded compensation after caller drop.
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let owned_branch = config.branch.clone();
    let recovery_path = worktree_dir.clone();
    let detached_path = recovery_path.clone();
    let detached_branch = owned_branch.clone();
    let owned_config = config.clone();
    let recovery_root = git_root.clone();
    tokio::spawn(async move {
        let result =
            create_reserved_worktree(git_root, worktree_dir, owned_config, checkout_guard).await;
        deliver_created_worktree(
            recovery_root,
            result,
            sender,
            detached_path,
            detached_branch,
        )
        .await;
    });
    let (result, acknowledgement) = receiver.await.map_err(|error| {
        format!(
            "Worktree creation owner ended without a receipt for {} ({owned_branch}): {error}",
            recovery_path.display()
        )
    })?;
    let _ = acknowledgement.send(());
    result
}

async fn deliver_created_worktree(
    git_root: PathBuf,
    result: Result<CreatedWorktree, String>,
    sender: tokio::sync::oneshot::Sender<(
        Result<ManagedWorktree, String>,
        tokio::sync::oneshot::Sender<()>,
    )>,
    recovery_path: PathBuf,
    branch: String,
) {
    let public_result = result
        .as_ref()
        .map(|created| created.worktree.clone())
        .map_err(Clone::clone);
    let (acknowledgement, accepted) = tokio::sync::oneshot::channel();
    if sender.send((public_result, acknowledgement)).is_ok() && accepted.await.is_ok() {
        return;
    }
    match result {
        Ok(created) => {
            if let Err(error) = remove_worktree_checked(
                &git_root,
                &created.worktree,
                &ToolContext::default(),
                Some(&created.guards),
            )
            .await
            {
                tracing::warn!(
                    path = %recovery_path.display(),
                    %branch,
                    %error,
                    "worktree creation owner retained cleanup debt after caller departure"
                );
            }
        }
        Err(error) => {
            tracing::warn!(
                path = %recovery_path.display(),
                %branch,
                %error,
                "worktree creation result was not observed after caller departure"
            );
        }
    }
}

async fn create_reserved_worktree(
    git_root: PathBuf,
    worktree_dir: PathBuf,
    config: WorktreeConfig,
    checkout_guard: ExistingDirectoryGuard,
) -> Result<CreatedWorktree, String> {
    let context = ToolContext::default();

    let mut args = vec!["worktree".to_string(), "add".to_string()];
    if let Some(ref base) = config.base {
        args.extend([
            "-b".to_string(),
            config.branch.clone(),
            worktree_dir.to_string_lossy().to_string(),
            "--".to_string(),
            base.clone(),
        ]);
    } else {
        args.extend([
            "-b".to_string(),
            config.branch.clone(),
            worktree_dir.to_string_lossy().to_string(),
        ]);
    }

    let output = run_git(&git_root, &args, &context).await.map_err(|error| {
        format!(
            "Worktree add has an ambiguous result for {} ({}): {error}; inspect this exact path before retry",
            worktree_dir.display(),
            config.branch
        )
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // If branch already exists, try without -b
        if stderr.contains("already exists") {
            let output2 = run_git(
                &git_root,
                &[
                    "worktree".to_string(),
                    "add".to_string(),
                    worktree_dir.to_string_lossy().to_string(),
                    "--".to_string(),
                    config.branch.clone(),
                ],
                &context,
            )
            .await
            .map_err(|error| {
                format!(
                    "Worktree add has an ambiguous result for {} ({}): {error}; inspect this exact path before retry",
                    worktree_dir.display(),
                    config.branch
                )
            })?;

            if !output2.status.success() {
                return Err(report_failed_worktree_add(
                    &worktree_dir,
                    &String::from_utf8_lossy(&output2.stderr),
                ));
            }
        } else {
            return Err(report_failed_worktree_add(&worktree_dir, &stderr));
        }
    }

    let marker = worktree_git_path(&worktree_dir, MANAGED_MARKER, &context)
        .await
        .map_err(|error| {
            format!(
                "Worktree was created but Git administration identity is unknown; cleanup debt at {} ({}): {error}",
                worktree_dir.display(),
                config.branch
            )
        })?;
    let guards = WorktreeCreationGuards::capture(checkout_guard, &marker).map_err(|error| {
        format!(
            "Worktree was created but its owner could not be retained; cleanup debt at {} ({}): {error}",
            worktree_dir.display(),
            config.branch
        )
    })?;
    settle_created_worktree(
        &git_root,
        &worktree_dir,
        &config.branch,
        &marker,
        guards,
        echo_core::utils::fs::atomic_write,
    )
    .await
}

fn report_failed_worktree_add(worktree_dir: &Path, stderr: &str) -> String {
    match std::fs::remove_dir(worktree_dir) {
        Ok(()) => format!("git worktree add failed: {stderr}; empty reservation released"),
        Err(error) => format!(
            "git worktree add failed: {stderr}; inspect retained path {} before retry: {error}",
            worktree_dir.display()
        ),
    }
}

async fn settle_created_worktree(
    git_root: &Path,
    worktree_dir: &Path,
    branch: &str,
    marker: &Path,
    guards: WorktreeCreationGuards,
    write_marker: fn(&Path, &[u8]) -> std::io::Result<()>,
) -> Result<CreatedWorktree, String> {
    let context = ToolContext::default();
    let marker_result = guards.verify(worktree_dir).and_then(|()| {
        write_marker(marker, branch.as_bytes()).map_err(|error| {
            format!(
                "Worktree ownership marker could not be written at {}: {error}",
                marker.display()
            )
        })
    });
    if let Err(marker_error) = marker_result {
        return Err(
            match compensate_unmarked_worktree(git_root, worktree_dir, branch, &context, &guards)
                .await
            {
                Ok(()) => {
                    format!("Worktree marker failed ({marker_error}); exact checkout removed")
                }
                Err(cleanup_error) => format!(
                    "Worktree marker failed ({marker_error}); cleanup debt at {} ({}): {cleanup_error}",
                    worktree_dir.display(),
                    branch
                ),
            },
        );
    }
    guards.verify(worktree_dir).map_err(|error| {
        format!(
            "Worktree marker was written but ownership changed; cleanup debt at {} ({branch}): {error}",
            worktree_dir.display()
        )
    })?;

    Ok(CreatedWorktree {
        worktree: ManagedWorktree {
            path: worktree_dir.to_path_buf(),
            branch: branch.to_string(),
            managed: true,
        },
        guards,
    })
}

async fn compensate_unmarked_worktree(
    git_root: &Path,
    worktree_dir: &Path,
    branch: &str,
    context: &ToolContext,
    guards: &WorktreeCreationGuards,
) -> Result<(), String> {
    guards.verify(worktree_dir)?;
    let actual_root = find_git_root(worktree_dir, context).await?;
    let expected = std::fs::canonicalize(worktree_dir)
        .map_err(|error| format!("Cannot identify created checkout: {error}"))?;
    let actual = std::fs::canonicalize(actual_root)
        .map_err(|error| format!("Cannot identify current checkout: {error}"))?;
    if actual != expected {
        return Err("Created checkout identity changed before compensation".to_string());
    }
    let actual_branch = run_git(worktree_dir, &["branch", "--show-current"], context).await?;
    if !actual_branch.status.success()
        || String::from_utf8_lossy(&actual_branch.stdout).trim() != branch
    {
        return Err("Created checkout branch changed before compensation".to_string());
    }
    let status = run_git(
        worktree_dir,
        &["status", "--porcelain=v1", "--untracked-files=all"],
        context,
    )
    .await?;
    if !status.status.success() || !status.stdout.is_empty() {
        return Err("Created checkout is not clean; refusing compensation".to_string());
    }
    guards.verify(worktree_dir)?;
    let removed = run_git(
        git_root,
        &[
            "worktree",
            "remove",
            worktree_dir.to_string_lossy().as_ref(),
        ],
        context,
    )
    .await?;
    if removed.status.success() {
        Ok(())
    } else {
        Err(format!(
            "git worktree remove failed: {}",
            String::from_utf8_lossy(&removed.stderr)
        ))
    }
}

/// Remove a managed worktree and clean up.
pub async fn remove_worktree(repo_path: &Path, worktree: &ManagedWorktree) -> Result<(), String> {
    remove_worktree_with_context(repo_path, worktree, &ToolContext::default()).await
}

pub async fn remove_worktree_with_context(
    repo_path: &Path,
    worktree: &ManagedWorktree,
    context: &ToolContext,
) -> Result<(), String> {
    remove_worktree_checked(repo_path, worktree, context, None).await
}

async fn remove_worktree_checked(
    repo_path: &Path,
    worktree: &ManagedWorktree,
    context: &ToolContext,
    guards: Option<&WorktreeCreationGuards>,
) -> Result<(), String> {
    if let Some(guards) = guards {
        guards.verify(&worktree.path)?;
    }
    let git_root = find_git_root(repo_path, context).await?;
    let canonical_worktree = verify_managed_worktree(&git_root, worktree, context).await?;

    let status = run_git(
        &canonical_worktree,
        &["status", "--porcelain=v1", "--untracked-files=all"],
        context,
    )
    .await?;
    if !status.status.success() {
        return Err(format!(
            "Failed to inspect worktree status: {}",
            String::from_utf8_lossy(&status.stderr)
        ));
    }
    if !status.stdout.is_empty() {
        return Err("Refusing to remove a worktree with uncommitted changes".to_string());
    }

    if let Some(guards) = guards {
        guards.verify(&canonical_worktree)?;
    }

    let output = run_git(
        &git_root,
        &[
            "worktree".to_string(),
            "remove".to_string(),
            canonical_worktree.to_string_lossy().to_string(),
        ],
        context,
    )
    .await?;

    if !output.status.success() {
        return Err(format!(
            "git worktree remove failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    Ok(())
}

/// List all worktrees in the repository.
pub async fn list_worktrees(repo_path: &Path) -> Result<Vec<ManagedWorktree>, String> {
    list_worktrees_with_context(repo_path, &ToolContext::default()).await
}

pub async fn list_worktrees_with_context(
    repo_path: &Path,
    context: &ToolContext,
) -> Result<Vec<ManagedWorktree>, String> {
    let git_root = find_git_root(repo_path, context).await?;

    let output = run_git(&git_root, &["worktree", "list", "--porcelain"], context).await?;
    if !output.status.success() {
        return Err(command_error(&output));
    }

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
            let managed = worktree_git_path(&path, MANAGED_MARKER, context)
                .await
                .is_ok_and(|marker| marker.is_file());
            worktrees.push(ManagedWorktree {
                path,
                branch: current_branch.clone(),
                managed,
            });
        }
    }
    // Handle last entry
    if let Some(path) = current_path {
        let managed = worktree_git_path(&path, MANAGED_MARKER, context)
            .await
            .is_ok_and(|marker| marker.is_file());
        worktrees.push(ManagedWorktree {
            path,
            branch: current_branch,
            managed,
        });
    }

    Ok(worktrees)
}

/// Merge changes from a worktree branch back to the base branch.
pub async fn merge_worktree(
    repo_path: &Path,
    worktree: &ManagedWorktree,
    target_branch: &str,
) -> Result<String, String> {
    merge_worktree_with_context(repo_path, worktree, target_branch, &ToolContext::default()).await
}

pub async fn merge_worktree_with_context(
    repo_path: &Path,
    worktree: &ManagedWorktree,
    target_branch: &str,
    context: &ToolContext,
) -> Result<String, String> {
    let git_root = find_git_root(repo_path, context).await?;
    verify_managed_worktree(&git_root, worktree, context).await?;
    ensure_clean_checkout(&git_root, context).await?;
    let original_ref = current_checkout(&git_root, context).await?;
    let merge_in_progress = run_git(
        &git_root,
        &["rev-parse", "--verify", "-q", "MERGE_HEAD"],
        context,
    )
    .await?;
    if merge_in_progress.status.success() {
        return Err("Refusing to start a worktree merge while another merge is active".to_string());
    }

    let co = run_git(&git_root, &["switch", "--", target_branch], context).await?;

    if !co.status.success() {
        return Err(format!(
            "Failed to checkout {}: {}",
            target_branch,
            String::from_utf8_lossy(&co.stderr)
        ));
    }

    let merge = match run_git(
        &git_root,
        &[
            "-c",
            "commit.gpgsign=false",
            "merge",
            "--no-edit",
            &worktree.branch,
            "--",
        ],
        context,
    )
    .await
    {
        Ok(output) => output,
        Err(error) => {
            restore_checkout(&git_root, &original_ref, context)
                .await
                .map_err(|restore_error| {
                    format!(
                        "Failed to start merge ({error}); checkout restoration failed: {restore_error}"
                    )
                })?;
            return Err(format!("Failed to start merge: {error}"));
        }
    };

    if !merge.status.success() {
        let merge_error = String::from_utf8_lossy(&merge.stderr).to_string();
        abort_merge_if_active(&git_root, &merge_error, context).await?;
        restore_checkout(&git_root, &original_ref, context)
            .await
            .map_err(|restore_error| {
                format!("Merge failed ({merge_error}); merge was aborted but checkout restoration failed: {restore_error}")
            })?;
        return Err(format!("Merge conflict or error: {merge_error}"));
    }

    Ok(format!("Merged {} into {}", worktree.branch, target_branch))
}

async fn verify_managed_worktree(
    git_root: &Path,
    worktree: &ManagedWorktree,
    context: &ToolContext,
) -> Result<PathBuf, String> {
    if !worktree.managed {
        return Err("Refusing to operate on a worktree not owned by echo-agent".to_string());
    }
    let canonical_root = std::fs::canonicalize(git_root.join(".worktrees"))
        .map_err(|error| format!("Failed to resolve worktrees root: {error}"))?;
    let canonical_worktree = std::fs::canonicalize(&worktree.path)
        .map_err(|error| format!("Failed to resolve worktree path: {error}"))?;
    if !canonical_worktree.starts_with(&canonical_root) {
        return Err(format!(
            "Refusing to operate on worktree outside {}: {}",
            canonical_root.display(),
            canonical_worktree.display()
        ));
    }
    let marker = worktree_git_path(&canonical_worktree, MANAGED_MARKER, context).await?;
    let marker_branch = std::fs::read_to_string(&marker)
        .map_err(|error| format!("Worktree ownership marker is missing or unreadable: {error}"))?;
    let actual_branch =
        run_git(&canonical_worktree, &["branch", "--show-current"], context).await?;
    if !actual_branch.status.success() {
        return Err(String::from_utf8_lossy(&actual_branch.stderr).to_string());
    }
    let actual_branch = String::from_utf8_lossy(&actual_branch.stdout);
    if marker_branch.trim() != worktree.branch || actual_branch.trim() != worktree.branch {
        return Err(
            "Worktree ownership metadata does not match its checked-out branch".to_string(),
        );
    }
    Ok(canonical_worktree)
}

async fn ensure_clean_checkout(git_root: &Path, context: &ToolContext) -> Result<(), String> {
    let status = run_git(
        git_root,
        &["status", "--porcelain=v1", "--untracked-files=all"],
        context,
    )
    .await?;
    if !status.status.success() {
        return Err(String::from_utf8_lossy(&status.stderr).to_string());
    }
    if status.stdout.is_empty() {
        Ok(())
    } else {
        Err("Refusing to merge while the target checkout has uncommitted changes".to_string())
    }
}

async fn abort_merge_if_active(
    git_root: &Path,
    merge_error: &str,
    context: &ToolContext,
) -> Result<(), String> {
    let active = run_git(
        git_root,
        &["rev-parse", "--verify", "-q", "MERGE_HEAD"],
        context,
    )
    .await
    .map_err(|error| {
        format!("Merge failed ({merge_error}); merge-state inspection failed: {error}")
    })?;
    if !active.status.success() {
        return Ok(());
    }
    let abort = run_git(git_root, &["merge", "--abort"], context)
        .await
        .map_err(|error| format!("Merge failed ({merge_error}); abort could not start: {error}"))?;
    if abort.status.success() {
        Ok(())
    } else {
        Err(format!(
            "Merge failed ({merge_error}); abort also failed: {}",
            String::from_utf8_lossy(&abort.stderr)
        ))
    }
}

async fn worktree_git_path(
    worktree_path: &Path,
    name: &str,
    context: &ToolContext,
) -> Result<PathBuf, String> {
    let output = run_git(
        worktree_path,
        &["rev-parse", "--path-format=absolute", "--git-path", name],
        context,
    )
    .await?;
    if !output.status.success() {
        return Err(format!(
            "Failed to resolve worktree metadata path: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let path = String::from_utf8(output.stdout)
        .map_err(|error| format!("Worktree metadata path is not UTF-8: {error}"))?;
    Ok(PathBuf::from(path.trim()))
}

async fn current_checkout(git_root: &Path, context: &ToolContext) -> Result<CheckoutRef, String> {
    let branch = run_git(
        git_root,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
        context,
    )
    .await?;
    if branch.status.success() {
        return String::from_utf8(branch.stdout)
            .map(|value| CheckoutRef::Branch(value.trim().to_string()))
            .map_err(|error| format!("Current branch is not UTF-8: {error}"));
    }
    let commit = run_git(git_root, &["rev-parse", "HEAD"], context).await?;
    if !commit.status.success() {
        return Err(String::from_utf8_lossy(&commit.stderr).to_string());
    }
    String::from_utf8(commit.stdout)
        .map(|value| CheckoutRef::Commit(value.trim().to_string()))
        .map_err(|error| format!("Detached HEAD is not UTF-8: {error}"))
}

async fn restore_checkout(
    git_root: &Path,
    checkout: &CheckoutRef,
    context: &ToolContext,
) -> Result<(), String> {
    let args = match checkout {
        CheckoutRef::Branch(branch) => vec!["switch", "--", branch.as_str()],
        CheckoutRef::Commit(commit) => vec!["switch", "--detach", commit.as_str()],
    };
    let output = run_git(git_root, &args, context).await?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).to_string())
    }
}

async fn find_git_root(path: &Path, context: &ToolContext) -> Result<PathBuf, String> {
    let output = run_git(path, &["rev-parse", "--show-toplevel"], context).await?;

    if output.status.success() {
        Ok(PathBuf::from(
            String::from_utf8_lossy(&output.stdout).trim(),
        ))
    } else {
        Err("Not a git repository".to_string())
    }
}

async fn run_git<S: AsRef<OsStr>>(
    working_dir: &Path,
    args: &[S],
    context: &ToolContext,
) -> Result<BoundedProcessOutput, String> {
    crate::process::run_bounded_command(
        "git_worktree",
        "git",
        args,
        working_dir,
        context,
        GIT_TIMEOUT,
        MAX_GIT_OUTPUT_BYTES,
    )
    .await
    .map_err(|error| error.to_string())
}

fn command_error(output: &BoundedProcessOutput) -> String {
    let mut error = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if output.truncated {
        error.push_str("\n[git output truncated at 1 MiB]");
    }
    error
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_git(repo: &Path, args: &[&str]) -> Result<String, String> {
        let output = Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .map_err(|error| error.to_string())?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).to_string());
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    fn test_repo() -> Result<tempfile::TempDir, String> {
        let dir = tempfile::tempdir().map_err(|error| error.to_string())?;
        run_git(dir.path(), &["init", "-b", "main"])?;
        run_git(dir.path(), &["config", "user.email", "test@example.com"])?;
        run_git(dir.path(), &["config", "user.name", "Test User"])?;
        std::fs::write(dir.path().join("value.txt"), "base\n")
            .map_err(|error| error.to_string())?;
        run_git(dir.path(), &["add", "value.txt"])?;
        run_git(
            dir.path(),
            &["-c", "commit.gpgsign=false", "commit", "-m", "base"],
        )?;
        Ok(dir)
    }

    fn fail_marker_write(_path: &Path, _bytes: &[u8]) -> std::io::Result<()> {
        Err(std::io::Error::other("injected marker failure"))
    }

    async fn test_creation_guards(
        path: &Path,
    ) -> Result<(PathBuf, WorktreeCreationGuards), String> {
        let checkout = open_existing_directory_guard(path).map_err(|error| error.to_string())?;
        let marker = worktree_git_path(path, MANAGED_MARKER, &ToolContext::default()).await?;
        let guards = WorktreeCreationGuards::capture(checkout, &marker)?;
        Ok((marker, guards))
    }

    async fn test_created_worktree(worktree: ManagedWorktree) -> Result<CreatedWorktree, String> {
        let (_, guards) = test_creation_guards(&worktree.path).await?;
        Ok(CreatedWorktree { worktree, guards })
    }

    #[tokio::test]
    async fn test_find_git_root() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"));
        let root = find_git_root(path, &ToolContext::default()).await;
        assert!(root.is_ok());
    }

    #[tokio::test]
    async fn test_find_git_root_non_repo() {
        let path = Path::new("/tmp");
        let root = find_git_root(path, &ToolContext::default()).await;
        // May or may not fail depending on whether /tmp is in a git repo
        let _ = root;
    }

    #[tokio::test]
    async fn unmanaged_worktree_is_not_removable() -> Result<(), String> {
        let repo = test_repo()?;
        let path = repo.path().join(".worktrees").join("external");
        std::fs::create_dir_all(path.parent().ok_or_else(|| "missing parent".to_string())?)
            .map_err(|error| error.to_string())?;
        run_git(
            repo.path(),
            &[
                "worktree",
                "add",
                "-b",
                "external",
                path.to_string_lossy().as_ref(),
            ],
        )?;
        let worktree = ManagedWorktree {
            path,
            branch: "external".to_string(),
            managed: false,
        };
        assert!(remove_worktree(repo.path(), &worktree).await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn marker_failure_compensates_only_clean_exact_checkout() -> Result<(), String> {
        let repo = test_repo()?;
        let path = repo.path().join(".worktrees").join("marker-fault");
        std::fs::create_dir_all(path.parent().ok_or_else(|| "missing parent".to_string())?)
            .map_err(|error| error.to_string())?;
        run_git(
            repo.path(),
            &[
                "worktree",
                "add",
                "-b",
                "marker-fault",
                path.to_string_lossy().as_ref(),
            ],
        )?;

        let (marker, guards) = test_creation_guards(&path).await?;
        let error = settle_created_worktree(
            repo.path(),
            &path,
            "marker-fault",
            &marker,
            guards,
            fail_marker_write,
        )
        .await
        .err()
        .ok_or_else(|| "marker failure unexpectedly succeeded".to_string())?;
        assert!(error.contains("exact checkout removed"), "{error}");
        assert!(!path.exists());
        assert_eq!(
            run_git(repo.path(), &["branch", "--list", "marker-fault"])?,
            "marker-fault",
            "compensation must not delete the branch"
        );
        Ok(())
    }

    #[tokio::test]
    async fn marker_failure_retains_dirty_checkout_as_debt() -> Result<(), String> {
        let repo = test_repo()?;
        let path = repo.path().join(".worktrees").join("dirty-marker-fault");
        std::fs::create_dir_all(path.parent().ok_or_else(|| "missing parent".to_string())?)
            .map_err(|error| error.to_string())?;
        run_git(
            repo.path(),
            &[
                "worktree",
                "add",
                "-b",
                "dirty-marker-fault",
                path.to_string_lossy().as_ref(),
            ],
        )?;
        std::fs::write(path.join("user.txt"), "retain").map_err(|error| error.to_string())?;
        let (marker, guards) = test_creation_guards(&path).await?;
        let error = settle_created_worktree(
            repo.path(),
            &path,
            "dirty-marker-fault",
            &marker,
            guards,
            fail_marker_write,
        )
        .await
        .err()
        .ok_or_else(|| "marker failure unexpectedly succeeded".to_string())?;
        assert!(error.contains("cleanup debt"), "{error}");
        assert!(path.join("user.txt").exists());
        Ok(())
    }

    #[tokio::test]
    async fn marker_failure_cannot_compensate_replaced_same_branch_checkout() -> Result<(), String>
    {
        let repo = test_repo()?;
        let path = repo.path().join(".worktrees").join("replaced-marker-fault");
        std::fs::create_dir_all(path.parent().ok_or_else(|| "missing parent".to_string())?)
            .map_err(|error| error.to_string())?;
        run_git(
            repo.path(),
            &[
                "worktree",
                "add",
                "-b",
                "replaced-marker-fault",
                path.to_string_lossy().as_ref(),
            ],
        )?;
        let (marker, guards) = test_creation_guards(&path).await?;
        run_git(
            repo.path(),
            &["worktree", "remove", path.to_string_lossy().as_ref()],
        )?;
        run_git(
            repo.path(),
            &[
                "worktree",
                "add",
                path.to_string_lossy().as_ref(),
                "--",
                "replaced-marker-fault",
            ],
        )?;

        let error = settle_created_worktree(
            repo.path(),
            &path,
            "replaced-marker-fault",
            &marker,
            guards,
            fail_marker_write,
        )
        .await
        .err()
        .ok_or_else(|| "marker failure unexpectedly succeeded".to_string())?;
        assert!(error.contains("cleanup debt"), "{error}");
        assert!(path.exists(), "replacement checkout was removed");
        Ok(())
    }

    #[tokio::test]
    async fn existing_worktree_path_is_never_adopted() -> Result<(), String> {
        let repo = test_repo()?;
        let path = repo.path().join(".worktrees").join("external");
        std::fs::create_dir_all(&path).map_err(|error| error.to_string())?;
        let result = create_worktree(
            repo.path(),
            &WorktreeConfig {
                branch: "external".to_string(),
                base: None,
                path_suffix: Some("external".to_string()),
            },
        )
        .await;
        assert!(result.is_err());
        assert!(path.exists());
        Ok(())
    }

    #[tokio::test]
    async fn definite_add_failure_releases_only_empty_reservation() -> Result<(), String> {
        let repo = test_repo()?;
        let path = repo.path().join(".worktrees").join("invalid-base");
        let result = create_worktree(
            repo.path(),
            &WorktreeConfig {
                branch: "invalid-base".to_string(),
                base: Some("missing-base-commit".to_string()),
                path_suffix: Some("invalid-base".to_string()),
            },
        )
        .await;
        assert!(result.is_err());
        assert!(!path.exists(), "empty reservation blocked a later retry");
        Ok(())
    }

    #[tokio::test]
    async fn dropped_receipt_after_send_reclaims_clean_checkout() -> Result<(), String> {
        let repo = test_repo()?;
        let worktree = create_worktree(
            repo.path(),
            &WorktreeConfig {
                branch: "dropped-receipt".to_string(),
                base: None,
                path_suffix: Some("dropped-receipt".to_string()),
            },
        )
        .await?;
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let created = test_created_worktree(worktree.clone()).await?;
        let delivery = tokio::spawn(deliver_created_worktree(
            repo.path().to_path_buf(),
            Ok(created),
            sender,
            worktree.path.clone(),
            worktree.branch.clone(),
        ));
        let (_result, acknowledgement) = receiver.await.map_err(|error| error.to_string())?;
        drop(acknowledgement);
        delivery.await.map_err(|error| error.to_string())?;
        assert!(!worktree.path.exists(), "unclaimed checkout was retained");
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn dropped_receipt_does_not_remove_replaced_checkout() -> Result<(), String> {
        let repo = test_repo()?;
        let worktree = create_worktree(
            repo.path(),
            &WorktreeConfig {
                branch: "replaced-receipt".to_string(),
                base: None,
                path_suffix: Some("replaced-receipt".to_string()),
            },
        )
        .await?;
        let created = test_created_worktree(worktree.clone()).await?;
        run_git(
            repo.path(),
            &[
                "worktree",
                "remove",
                worktree.path.to_string_lossy().as_ref(),
            ],
        )?;
        run_git(
            repo.path(),
            &[
                "worktree",
                "add",
                worktree.path.to_string_lossy().as_ref(),
                "--",
                &worktree.branch,
            ],
        )?;
        let (sender, receiver) = tokio::sync::oneshot::channel();
        drop(receiver);
        deliver_created_worktree(
            repo.path().to_path_buf(),
            Ok(created),
            sender,
            worktree.path.clone(),
            worktree.branch.clone(),
        )
        .await;
        assert!(worktree.path.exists(), "unclaimed replacement was removed");
        Ok(())
    }

    #[tokio::test]
    async fn acknowledged_receipt_transfers_worktree_to_caller() -> Result<(), String> {
        let repo = test_repo()?;
        let worktree = create_worktree(
            repo.path(),
            &WorktreeConfig {
                branch: "accepted-receipt".to_string(),
                base: None,
                path_suffix: Some("accepted-receipt".to_string()),
            },
        )
        .await?;
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let created = test_created_worktree(worktree.clone()).await?;
        let delivery = tokio::spawn(deliver_created_worktree(
            repo.path().to_path_buf(),
            Ok(created),
            sender,
            worktree.path.clone(),
            worktree.branch.clone(),
        ));
        let (result, acknowledgement) = receiver.await.map_err(|error| error.to_string())?;
        acknowledgement
            .send(())
            .map_err(|_| "owner stopped before acceptance".to_string())?;
        delivery.await.map_err(|error| error.to_string())?;
        assert!(result?.path.exists());
        remove_worktree(repo.path(), &worktree).await?;
        Ok(())
    }

    #[tokio::test]
    async fn merge_conflict_aborts_and_restores_original_branch() -> Result<(), String> {
        let repo = test_repo()?;
        let worktree = create_worktree(
            repo.path(),
            &WorktreeConfig {
                branch: "feature".to_string(),
                base: None,
                path_suffix: Some("feature".to_string()),
            },
        )
        .await?;
        std::fs::write(worktree.path.join("value.txt"), "feature\n")
            .map_err(|error| error.to_string())?;
        run_git(&worktree.path, &["add", "value.txt"])?;
        run_git(
            &worktree.path,
            &["-c", "commit.gpgsign=false", "commit", "-m", "feature"],
        )?;

        run_git(repo.path(), &["switch", "-c", "target"])?;
        std::fs::write(repo.path().join("value.txt"), "target\n")
            .map_err(|error| error.to_string())?;
        run_git(repo.path(), &["add", "value.txt"])?;
        run_git(
            repo.path(),
            &["-c", "commit.gpgsign=false", "commit", "-m", "target"],
        )?;
        run_git(repo.path(), &["switch", "main"])?;

        assert!(
            merge_worktree(repo.path(), &worktree, "target")
                .await
                .is_err()
        );
        assert_eq!(run_git(repo.path(), &["branch", "--show-current"])?, "main");
        assert!(run_git(repo.path(), &["rev-parse", "--verify", "-q", "MERGE_HEAD"]).is_err());
        assert_eq!(
            std::fs::read_to_string(repo.path().join("value.txt"))
                .map_err(|error| error.to_string())?,
            "base\n"
        );
        Ok(())
    }

    #[tokio::test]
    async fn merge_rejects_forged_managed_flag_and_dirty_checkout() -> Result<(), String> {
        let repo = test_repo()?;
        let external_path = repo.path().join(".worktrees").join("external");
        std::fs::create_dir_all(
            external_path
                .parent()
                .ok_or_else(|| "missing parent".to_string())?,
        )
        .map_err(|error| error.to_string())?;
        run_git(
            repo.path(),
            &[
                "worktree",
                "add",
                "-b",
                "external",
                external_path.to_string_lossy().as_ref(),
            ],
        )?;
        let forged = ManagedWorktree {
            path: external_path,
            branch: "external".to_string(),
            managed: true,
        };
        assert!(merge_worktree(repo.path(), &forged, "main").await.is_err());

        let managed = create_worktree(
            repo.path(),
            &WorktreeConfig {
                branch: "feature".to_string(),
                base: None,
                path_suffix: Some("feature".to_string()),
            },
        )
        .await?;
        std::fs::write(repo.path().join("untracked.txt"), "preserve\n")
            .map_err(|error| error.to_string())?;
        assert!(merge_worktree(repo.path(), &managed, "main").await.is_err());
        assert_eq!(
            std::fs::read_to_string(repo.path().join("untracked.txt"))
                .map_err(|error| error.to_string())?,
            "preserve\n"
        );
        Ok(())
    }
}
