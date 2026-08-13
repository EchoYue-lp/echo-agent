//! Per-file Git worktree checkpoints used before destructive file mutations.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static CHECKPOINT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Serialize, Deserialize)]
struct IndexEntry {
    mode: String,
    object_id: String,
    stage: String,
}

#[derive(Serialize, Deserialize)]
struct CheckpointManifest {
    relative_path: String,
    worktree_existed: bool,
    #[cfg(unix)]
    worktree_mode: Option<u32>,
    index_entry: Option<IndexEntry>,
}

/// Snapshot the exact worktree bytes and index entry for one file.
///
/// `Ok(None)` means the target is not inside a Git repository. Once a Git root
/// is found, every snapshot failure is returned so callers can stop the pending
/// destructive mutation.
pub fn create_checkpoint(file_path: &Path) -> Result<Option<String>, String> {
    let Some(git_root) = find_git_root(file_path)? else {
        return Ok(None);
    };
    let target = fs::canonicalize(file_path)
        .map_err(|error| format!("failed to resolve checkpoint target: {error}"))?;
    let relative = target.strip_prefix(&git_root).map_err(|_| {
        format!(
            "checkpoint target '{}' is outside Git root '{}'",
            target.display(),
            git_root.display()
        )
    })?;
    let relative_path = relative
        .to_str()
        .ok_or_else(|| "checkpoint path is not valid UTF-8".to_string())?
        .to_string();
    let id = checkpoint_id();
    let checkpoint_dir = checkpoint_root(&git_root)?.join(&id);
    fs::create_dir_all(&checkpoint_dir)
        .map_err(|error| format!("failed to create checkpoint directory: {error}"))?;

    let result = (|| {
        let bytes = fs::read(&target)
            .map_err(|error| format!("failed to read checkpoint target: {error}"))?;
        echo_core::utils::fs::atomic_write(&checkpoint_dir.join("worktree.bin"), &bytes)
            .map_err(|error| format!("failed to persist checkpoint bytes: {error}"))?;
        let manifest = CheckpointManifest {
            relative_path,
            worktree_existed: true,
            #[cfg(unix)]
            worktree_mode: file_mode(&target)?,
            index_entry: read_index_entry(&git_root, relative)?,
        };
        let encoded = serde_json::to_vec(&manifest)
            .map_err(|error| format!("failed to encode checkpoint manifest: {error}"))?;
        echo_core::utils::fs::atomic_write(&checkpoint_dir.join("manifest.json"), &encoded)
            .map_err(|error| format!("failed to persist checkpoint manifest: {error}"))?;
        Ok(Some(id))
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&checkpoint_dir);
    }
    result
}

/// Restore only the file represented by `checkpoint_id`, including its exact
/// pre-mutation Git index entry. Other worktree and staged changes are untouched.
pub fn rollback_to_checkpoint(file_path: &Path, checkpoint_id: &str) -> Result<(), String> {
    echo_core::utils::fs::validate_path_segment(checkpoint_id)
        .map_err(|error| format!("invalid checkpoint id: {error}"))?;
    let git_root = find_git_root(file_path)?
        .ok_or_else(|| "checkpoint target is not inside a Git repository".to_string())?;
    let checkpoint_dir = checkpoint_root(&git_root)?.join(checkpoint_id);
    let manifest_bytes = fs::read(checkpoint_dir.join("manifest.json"))
        .map_err(|error| format!("failed to read checkpoint manifest: {error}"))?;
    let manifest: CheckpointManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| format!("invalid checkpoint manifest: {error}"))?;
    let target = git_root.join(&manifest.relative_path);
    let expected = absolute_target(file_path)?;
    if target != expected {
        return Err("checkpoint does not belong to the requested file".to_string());
    }

    if manifest.worktree_existed {
        let bytes = fs::read(checkpoint_dir.join("worktree.bin"))
            .map_err(|error| format!("failed to read checkpoint bytes: {error}"))?;
        echo_core::utils::fs::atomic_write(&target, &bytes)
            .map_err(|error| format!("failed to restore checkpoint bytes: {error}"))?;
        #[cfg(unix)]
        if let Some(mode) = manifest.worktree_mode {
            set_file_mode(&target, mode)?;
        }
    } else if target.exists() {
        fs::remove_file(&target)
            .map_err(|error| format!("failed to remove restored-absent file: {error}"))?;
    }
    restore_index_entry(
        &git_root,
        &manifest.relative_path,
        manifest.index_entry.as_ref(),
    )
}

/// Remove older per-file checkpoints, preserving the newest `keep` entries.
pub fn cleanup_old_checkpoints(file_path: &Path, keep: usize) {
    let Ok(Some(git_root)) = find_git_root(file_path) else {
        return;
    };
    let Ok(root) = checkpoint_root(&git_root) else {
        return;
    };
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.file_name()));
    for entry in entries.into_iter().skip(keep) {
        let _ = fs::remove_dir_all(entry.path());
    }
}

fn checkpoint_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let sequence = CHECKPOINT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{nanos:032x}-{:08x}-{sequence:016x}", std::process::id())
}

fn checkpoint_root(git_root: &Path) -> Result<PathBuf, String> {
    let output = Command::new("git")
        .args(["rev-parse", "--git-path", "echo-checkpoints"])
        .current_dir(git_root)
        .output()
        .map_err(|error| format!("failed to locate Git metadata directory: {error}"))?;
    if !output.status.success() {
        return Err("git rev-parse --git-path failed".to_string());
    }
    let path = String::from_utf8(output.stdout)
        .map_err(|error| format!("Git metadata path is not valid UTF-8: {error}"))?;
    let path = PathBuf::from(path.trim());
    Ok(if path.is_absolute() {
        path
    } else {
        git_root.join(path)
    })
}

fn absolute_target(file_path: &Path) -> Result<PathBuf, String> {
    if let Ok(path) = fs::canonicalize(file_path) {
        return Ok(path);
    }
    if file_path.is_absolute() {
        return Ok(file_path.to_path_buf());
    }
    std::env::current_dir()
        .map(|directory| directory.join(file_path))
        .map_err(|error| format!("failed to resolve checkpoint target: {error}"))
}

fn find_git_root(file_path: &Path) -> Result<Option<PathBuf>, String> {
    let dir = if file_path.is_file() {
        file_path.parent().unwrap_or(file_path)
    } else {
        file_path
    };
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(dir)
        .output()
        .map_err(|error| format!("failed to run git rev-parse: {error}"))?;
    if !output.status.success() {
        return Ok(None);
    }
    let root = String::from_utf8(output.stdout)
        .map_err(|error| format!("Git root is not valid UTF-8: {error}"))?;
    let root = fs::canonicalize(root.trim())
        .map_err(|error| format!("failed to resolve Git root: {error}"))?;
    Ok(Some(root))
}

fn read_index_entry(git_root: &Path, relative: &Path) -> Result<Option<IndexEntry>, String> {
    let output = Command::new("git")
        .args(["ls-files", "--stage", "--"])
        .arg(relative)
        .current_dir(git_root)
        .output()
        .map_err(|error| format!("failed to inspect Git index: {error}"))?;
    if !output.status.success() {
        return Err("git ls-files failed while creating checkpoint".to_string());
    }
    let line = String::from_utf8(output.stdout)
        .map_err(|error| format!("Git index output is not valid UTF-8: {error}"))?;
    let Some(metadata) = line.split_once('\t').map(|(metadata, _)| metadata) else {
        return Ok(None);
    };
    let mut fields = metadata.split_whitespace();
    let mode = fields
        .next()
        .ok_or_else(|| "missing index mode".to_string())?;
    let object_id = fields
        .next()
        .ok_or_else(|| "missing index object id".to_string())?;
    let stage = fields
        .next()
        .ok_or_else(|| "missing index stage".to_string())?;
    Ok(Some(IndexEntry {
        mode: mode.to_string(),
        object_id: object_id.to_string(),
        stage: stage.to_string(),
    }))
}

fn restore_index_entry(
    git_root: &Path,
    relative_path: &str,
    entry: Option<&IndexEntry>,
) -> Result<(), String> {
    let mut child = Command::new("git")
        .args(["update-index", "--index-info"])
        .current_dir(git_root)
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|error| format!("failed to start git update-index: {error}"))?;
    let line = match entry {
        Some(entry) => format!(
            "{} {} {}\t{}\n",
            entry.mode, entry.object_id, entry.stage, relative_path
        ),
        None => format!("0 {} 0\t{}\n", "0".repeat(40), relative_path),
    };
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "git update-index stdin unavailable".to_string())?;
    stdin
        .write_all(line.as_bytes())
        .map_err(|error| format!("failed to restore Git index entry: {error}"))?;
    drop(stdin);
    let status = child
        .wait()
        .map_err(|error| format!("failed to wait for git update-index: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("git update-index rejected checkpoint entry".to_string())
    }
}

#[cfg(unix)]
fn file_mode(path: &Path) -> Result<Option<u32>, String> {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .map(|metadata| Some(metadata.permissions().mode()))
        .map_err(|error| format!("failed to inspect checkpoint permissions: {error}"))
}

#[cfg(unix)]
fn set_file_mode(path: &Path, mode: u32) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|error| format!("failed to restore checkpoint permissions: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rollback_restores_only_target_worktree_and_index() -> Result<(), Box<dyn std::error::Error>>
    {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        run_git(root, &["init"])?;
        run_git(root, &["config", "user.email", "test@example.com"])?;
        run_git(root, &["config", "user.name", "Test"])?;
        run_git(root, &["config", "commit.gpgsign", "false"])?;
        let target = root.join("target.txt");
        let unrelated = root.join("unrelated.txt");
        fs::write(&target, "base")?;
        fs::write(&unrelated, "base")?;
        run_git(root, &["add", "."])?;
        run_git(root, &["commit", "-m", "base"])?;
        fs::write(&target, "staged")?;
        run_git(root, &["add", "target.txt"])?;
        fs::write(&target, "worktree")?;
        fs::write(&unrelated, "keep me")?;

        let id = create_checkpoint(&target)?.ok_or("missing checkpoint")?;
        fs::write(&target, "mutated")?;
        rollback_to_checkpoint(&target, &id)?;

        assert_eq!(fs::read_to_string(&target)?, "worktree");
        assert_eq!(fs::read_to_string(&unrelated)?, "keep me");
        let staged = Command::new("git")
            .args(["show", ":target.txt"])
            .current_dir(root)
            .output()?;
        assert!(staged.status.success());
        assert_eq!(String::from_utf8(staged.stdout)?, "staged");
        Ok(())
    }

    fn run_git(root: &Path, args: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
        let status = Command::new("git").args(args).current_dir(root).status()?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("git command failed: {args:?}").into())
        }
    }
}
