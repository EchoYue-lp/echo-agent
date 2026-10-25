//! Per-file Git checkpoints. Local and adapted targets share one format and algorithm.

use echo_core::error::{ReactError, Result as CoreResult, ToolError};
use echo_core::sandbox::{CommandKind, ExecutionResult, SandboxCommand};
use echo_core::tools::filesystem::FileSystem;
use serde::{Deserialize, Serialize};
#[cfg(test)]
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

/// Existing local entry point; its recovery format and path ownership are unchanged.
pub fn create_checkpoint(path: &Path) -> Result<Option<String>, String> {
    create_checkpoint_with_backend(path, &crate::filesystem::LocalFileSystem, &local_command)
        .map_err(legacy_error)
}

/// Existing local restore entry point.
pub fn rollback_to_checkpoint(path: &Path, id: &str) -> Result<(), String> {
    rollback_to_checkpoint_with_backend(
        path,
        id,
        &crate::filesystem::LocalFileSystem,
        &local_command,
    )
    .map_err(legacy_error)
}

/// Existing best-effort local retention behavior.
pub fn cleanup_old_checkpoints(path: &Path, keep: usize) {
    let _ = cleanup_old_checkpoints_with_backend(
        path,
        keep,
        &crate::filesystem::LocalFileSystem,
        &local_command,
    );
}

/// Snapshot the original bytes/index using the target's I/O and command executor.
/// The callback consumes an existing structured command; it does not parse a terminal.
/// `None` requires a completed Git query proving this is not a repository.
pub fn create_checkpoint_with_backend(
    path: &Path,
    file_system: &dyn FileSystem,
    execute: &dyn Fn(SandboxCommand) -> CoreResult<ExecutionResult>,
) -> CoreResult<Option<String>> {
    let Some(root) = find_git_root(path, file_system, execute)? else {
        return Ok(None);
    };
    let target = file_system.canonicalize(path)?;
    let relative = target.strip_prefix(&root).map_err(|_| {
        checkpoint_error(format!(
            "checkpoint target '{}' is outside Git root '{}'",
            target.display(),
            root.display()
        ))
    })?;
    let relative_path = relative
        .to_str()
        .ok_or_else(|| checkpoint_error("checkpoint path is not valid UTF-8"))?
        .to_string();
    let id = checkpoint_id();
    let directory = checkpoint_root(&root, execute)?.join(&id);
    file_system.create_dir_all(&directory)?;
    let result = (|| {
        let bytes = file_system.read(&target)?;
        file_system.atomic_write(&directory.join("worktree.bin"), &bytes)?;
        let manifest = CheckpointManifest {
            relative_path,
            worktree_existed: true,
            #[cfg(unix)]
            worktree_mode: file_system.metadata(&target)?.mode,
            index_entry: read_index_entry(&root, relative, execute)?,
        };
        let encoded = serde_json::to_vec(&manifest).map_err(|error| {
            checkpoint_error(format!("failed to encode checkpoint manifest: {error}"))
        })?;
        file_system.atomic_write(&directory.join("manifest.json"), &encoded)?;
        Ok(Some(id))
    })();
    // Preserve local/definite-failure cleanup, but do not send another mutation
    // after a provider reports that a write may still be unsettled.
    if let Err(error) = &result
        && echo_core::tools::ToolFailure::from_error(error, false).side_effect
            == echo_core::tools::ToolSideEffect::None
    {
        let _ = file_system.remove_dir_all(&directory);
    }
    result
}

/// Restore only the represented target and its exact original Git index entry.
pub fn rollback_to_checkpoint_with_backend(
    path: &Path,
    id: &str,
    file_system: &dyn FileSystem,
    execute: &dyn Fn(SandboxCommand) -> CoreResult<ExecutionResult>,
) -> CoreResult<()> {
    echo_core::utils::fs::validate_path_segment(id)
        .map_err(|error| checkpoint_error(format!("invalid checkpoint id: {error}")))?;
    let root = find_git_root(path, file_system, execute)?
        .ok_or_else(|| checkpoint_error("checkpoint target is not inside a Git repository"))?;
    let directory = checkpoint_root(&root, execute)?.join(id);
    let manifest: CheckpointManifest =
        serde_json::from_slice(&file_system.read(&directory.join("manifest.json"))?)
            .map_err(|error| checkpoint_error(format!("invalid checkpoint manifest: {error}")))?;
    let target = root.join(&manifest.relative_path);
    if target != absolute_target(path, file_system)? {
        return Err(checkpoint_error(
            "checkpoint does not belong to the requested file",
        ));
    }
    if manifest.worktree_existed {
        file_system.atomic_write(&target, &file_system.read(&directory.join("worktree.bin"))?)?;
        #[cfg(unix)]
        if let Some(mode) = manifest.worktree_mode {
            file_system.set_mode(&target, mode)?;
        }
    } else if file_system.exists(&target)? {
        file_system.remove_file(&target)?;
    }
    let line = match &manifest.index_entry {
        Some(entry) => format!(
            "{} {} {}\t{}\n",
            entry.mode, entry.object_id, entry.stage, manifest.relative_path
        ),
        None => format!("0 {} 0\t{}\n", "0".repeat(40), manifest.relative_path),
    };
    let output = execute(
        SandboxCommand::program("git", vec!["update-index".into(), "--index-info".into()])
            .with_working_dir(&root)
            .with_stdin(line),
    )?;
    require_success(output, "git update-index rejected checkpoint entry")?;
    Ok(())
}

/// Apply the original per-target checkpoint retention policy in that namespace.
pub fn cleanup_old_checkpoints_with_backend(
    path: &Path,
    keep: usize,
    file_system: &dyn FileSystem,
    execute: &dyn Fn(SandboxCommand) -> CoreResult<ExecutionResult>,
) -> CoreResult<()> {
    let Some(root) = find_git_root(path, file_system, execute)? else {
        return Ok(());
    };
    let directory = checkpoint_root(&root, execute)?;
    let entries = match file_system.read_dir(&directory) {
        Ok(entries) => entries,
        Err(ReactError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut entries = entries;
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.file_name()));
    for entry in entries.into_iter().skip(keep) {
        file_system.remove_dir_all(&entry.path())?;
    }
    Ok(())
}

fn checkpoint_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|v| v.as_nanos())
        .unwrap_or(0);
    let sequence = CHECKPOINT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{nanos:032x}-{:08x}-{sequence:016x}", std::process::id())
}

fn checkpoint_root(
    root: &Path,
    execute: &dyn Fn(SandboxCommand) -> CoreResult<ExecutionResult>,
) -> CoreResult<PathBuf> {
    let output = require_success(
        execute(
            SandboxCommand::program(
                "git",
                vec![
                    "rev-parse".into(),
                    "--git-path".into(),
                    "echo-checkpoints".into(),
                ],
            )
            .with_working_dir(root),
        )?,
        "git rev-parse --git-path failed",
    )?;
    let path = PathBuf::from(output.stdout.trim());
    Ok(if path.is_absolute() {
        path
    } else {
        root.join(path)
    })
}

fn absolute_target(path: &Path, file_system: &dyn FileSystem) -> CoreResult<PathBuf> {
    match file_system.canonicalize(path) {
        Ok(path) => Ok(path),
        Err(ReactError::Io(_)) if path.is_absolute() => Ok(path.to_path_buf()),
        Err(ReactError::Io(_)) => Ok(file_system.current_dir()?.join(path)),
        Err(error) => Err(error),
    }
}

fn find_git_root(
    path: &Path,
    file_system: &dyn FileSystem,
    execute: &dyn Fn(SandboxCommand) -> CoreResult<ExecutionResult>,
) -> CoreResult<Option<PathBuf>> {
    let directory = if file_system.is_file(path)? {
        path.parent().unwrap_or(path)
    } else {
        path
    };
    let output = execute(
        SandboxCommand::program("git", vec!["rev-parse".into(), "--show-toplevel".into()])
            .with_working_dir(directory),
    )?;
    if output.timed_out || output.cancelled || output.output_truncated {
        return Err(checkpoint_error("Git repository query did not complete"));
    }
    if output.exit_code != 0 {
        // A failed query inside an actual Git worktree is not proof that
        // checkpoint protection is unnecessary (permissions/safe.directory).
        let directory = file_system.canonicalize(directory)?;
        if !file_system.is_dir(&directory)? {
            return Err(checkpoint_error("checkpoint parent is not a directory"));
        }
        for ancestor in directory.ancestors() {
            match file_system.symlink_metadata(&ancestor.join(".git")) {
                Ok(_) => {
                    return Err(checkpoint_error(
                        "Git repository query failed for a checkpoint-managed target",
                    ));
                }
                Err(ReactError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        if matches!(output.exit_code, 125..=127 | 255) {
            return Err(checkpoint_error(
                "Git command or target environment is unavailable",
            ));
        }
        return Ok(None);
    }
    Ok(Some(
        file_system.canonicalize(Path::new(output.stdout.trim()))?,
    ))
}

fn read_index_entry(
    root: &Path,
    relative: &Path,
    execute: &dyn Fn(SandboxCommand) -> CoreResult<ExecutionResult>,
) -> CoreResult<Option<IndexEntry>> {
    let path = relative
        .to_str()
        .ok_or_else(|| checkpoint_error("checkpoint path is not valid UTF-8"))?;
    let output = require_success(
        execute(
            SandboxCommand::program(
                "git",
                vec![
                    "ls-files".into(),
                    "--stage".into(),
                    "--".into(),
                    path.into(),
                ],
            )
            .with_working_dir(root),
        )?,
        "git ls-files failed while creating checkpoint",
    )?;
    let Some(metadata) = output.stdout.split_once('\t').map(|(value, _)| value) else {
        return Ok(None);
    };
    let mut fields = metadata.split_whitespace();
    Ok(Some(IndexEntry {
        mode: fields
            .next()
            .ok_or_else(|| checkpoint_error("missing index mode"))?
            .into(),
        object_id: fields
            .next()
            .ok_or_else(|| checkpoint_error("missing index object id"))?
            .into(),
        stage: fields
            .next()
            .ok_or_else(|| checkpoint_error("missing index stage"))?
            .into(),
    }))
}

fn require_success(output: ExecutionResult, message: &str) -> CoreResult<ExecutionResult> {
    if output.success() && !output.output_truncated {
        Ok(output)
    } else {
        Err(checkpoint_error(message))
    }
}

fn checkpoint_error(message: impl Into<String>) -> ReactError {
    ToolError::ExecutionFailed {
        tool: "file_checkpoint".into(),
        message: message.into(),
    }
    .into()
}

fn legacy_error(error: ReactError) -> String {
    match error {
        ReactError::Tool(inner) => match *inner {
            ToolError::ExecutionFailed { message, .. } => message,
            other => other.to_string(),
        },
        other => other.to_string(),
    }
}

fn local_command(command: SandboxCommand) -> CoreResult<ExecutionResult> {
    let CommandKind::Program { program, args } = command.kind else {
        return Err(checkpoint_error(
            "checkpoint requires a direct program command",
        ));
    };
    let started = std::time::Instant::now();
    let mut process = Command::new(program);
    process
        .args(args)
        .envs(command.env)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(directory) = command.working_dir {
        process.current_dir(directory);
    }
    if command.stdin.is_some() {
        process.stdin(Stdio::piped());
    }
    let mut child = process.spawn()?;
    if let Some(input) = command.stdin {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| checkpoint_error("git update-index stdin unavailable"))?;
        stdin.write_all(input.as_bytes())?;
    }
    let output = child.wait_with_output()?;
    let stdout_bytes = u64::try_from(output.stdout.len()).unwrap_or(u64::MAX);
    let stderr_bytes = u64::try_from(output.stderr.len()).unwrap_or(u64::MAX);
    let stdout = String::from_utf8(output.stdout)
        .map_err(|error| checkpoint_error(format!("Git output is not valid UTF-8: {error}")))?;
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    Ok(ExecutionResult {
        exit_code: output.status.code().unwrap_or(-1),
        stdout,
        stderr,
        duration: started.elapsed(),
        sandbox_type: "local".into(),
        timed_out: false,
        cancelled: false,
        output_truncated: false,
        stdout_bytes,
        stderr_bytes,
    })
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
