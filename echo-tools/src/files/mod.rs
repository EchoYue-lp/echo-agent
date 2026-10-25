#[cfg(feature = "files")]
pub mod apply_patch;
pub mod artifact;
#[cfg(feature = "files")]
pub mod code_search;
#[cfg(feature = "files")]
pub mod diff;
#[cfg(feature = "files")]
#[allow(clippy::module_inception)]
pub mod files;
#[cfg(feature = "files")]
pub use crate::filesystem::LocalFileSystem;
#[cfg(feature = "files")]
pub mod glob;
#[cfg(feature = "files")]
pub mod grep;
#[cfg(feature = "files")]
pub mod repo_map;

#[cfg(feature = "files")]
use std::path::{Component, Path, PathBuf};

#[cfg(feature = "files")]
use echo_core::error::{ReactError, Result, ToolError};
#[cfg(feature = "files")]
use echo_core::tools::filesystem::FileSystem;

/// Resolve a user-supplied relative/absolute path into a safe absolute path.
///
/// Path base priority for relative paths:
/// 1. `base_dir` (construction-time confinement, e.g. `with_base_dir`)
/// 2. `working_dir` (runtime per-call dir, e.g. a session-bound worktree)
/// 3. process `current_dir` (fallback)
///
/// When `base_dir` is set, resolution is confined within it (absolute paths
/// must stay inside; relative paths are joined under it; symlinks are
/// canonicalized defensively). When only `working_dir` is set, relative paths
/// are joined under it but there is no confinement check (it acts as a CWD
/// override). When neither is set, behavior is unchanged from before
/// (raw path + best-effort canonicalize).
///
/// - Absolute path: normalize then (if base_dir set) verify it stays within base_dir
/// - Relative path: expand relative to the chosen base then verify
///
/// After textual normalization, the selected provider canonicalizes symlinks
/// and verify the real path stays within the allowed directory. For write operations
/// where the target file doesn't exist yet, the parent directory is canonicalized instead.
#[cfg(feature = "files")]
fn resolve_path(
    file_system: &dyn FileSystem,
    tool: &str,
    path_str: &str,
    base_dir: &Option<PathBuf>,
    working_dir: Option<&Path>,
) -> Result<PathBuf> {
    let requested = Path::new(path_str);

    // Effective base: construction-time base_dir takes priority (confinement);
    // otherwise fall back to the runtime working_dir (CWD override, no confinement).
    let effective_base: Option<PathBuf> = base_dir
        .clone()
        .or_else(|| working_dir.map(|p| p.to_path_buf()));

    let resolved = if let Some(ref base) = effective_base {
        let normalized_base = normalize_path(base);

        // Relative path: expand with base_dir as root; absolute path: normalize directly
        let normalized = if requested.is_absolute() {
            normalize_path(requested)
        } else {
            normalize_path(&normalized_base.join(requested))
        };

        if !normalized.starts_with(&normalized_base) {
            return Err(ToolError::ExecutionFailed {
                tool: tool.to_string(),
                message: format!(
                    "Path '{}' is outside the allowed working directory '{}'. \
                     Use a relative path, or an absolute path under the working directory.",
                    path_str,
                    normalized_base.display()
                ),
            }
            .into());
        }

        // Defense against symlink bypass: canonicalize to resolve symlinks and
        // verify the real (resolved) path stays within the base directory.
        match file_system.canonicalize(&normalized) {
            Ok(canonical) => {
                let canonical_base = file_system.canonicalize(&normalized_base).map_err(|e| {
                    file_error(e, |e| ToolError::ExecutionFailed {
                        tool: tool.to_string(),
                        message: format!("Cannot resolve base directory: {}", e),
                    })
                })?;
                if !canonical.starts_with(&canonical_base) {
                    return Err(ToolError::ExecutionFailed {
                        tool: tool.to_string(),
                        message: format!(
                            "Path '{}' resolves via symlink to location outside allowed scope",
                            path_str
                        ),
                    }
                    .into());
                }
                canonical
            }
            Err(ReactError::Io(_)) => {
                // File doesn't exist yet (write operation) — canonicalize parent directory
                if let Some(parent) = normalized.parent()
                    && parent != Path::new("")
                    && let Some(canonical_parent) = best_effort_canonicalize(file_system, parent)?
                {
                    let canonical_base = best_effort_canonicalize(file_system, &normalized_base)?
                        .unwrap_or_else(|| normalized_base.clone());
                    if !canonical_parent.starts_with(&canonical_base) {
                        return Err(ToolError::ExecutionFailed {
                            tool: tool.to_string(),
                            message: format!(
                                "Parent directory of '{}' resolves outside allowed scope",
                                path_str
                            ),
                        }
                        .into());
                    }
                    let filename = normalized.file_name().unwrap_or_default();
                    return Ok(canonical_parent.join(filename));
                }
                // Fallback: return textually normalized path if parent doesn't exist
                normalized
            }
            Err(error) => return Err(error),
        }
    } else {
        let normalized = normalize_path(requested);
        // normalize_path strips "." to an empty path — use CWD as fallback
        let normalized = if normalized.as_os_str().is_empty() {
            file_system.current_dir()?
        } else {
            normalized
        };
        // Best-effort canonicalization when no base_dir constraint
        match file_system.canonicalize(&normalized) {
            Ok(path) => path,
            Err(ReactError::Io(_)) => normalized,
            Err(error) => return Err(error),
        }
    };

    Ok(resolved)
}

/// Filesystem-independent path normalization (resolves `.` and `..`)
#[cfg(feature = "files")]
fn normalize_path(path: &Path) -> PathBuf {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                if let Some(Component::Normal(_)) = components.last() {
                    components.pop();
                }
            }
            Component::CurDir => {}
            c => components.push(c),
        }
    }
    components.iter().collect()
}

/// Keep the existing synchronous transaction and its resource guards together.
/// Dropping the observer does not cancel a started filesystem mutation.
#[cfg(feature = "files")]
fn run_file_operation<F>(
    tool: &'static str,
    ctx: &echo_core::tools::ToolContext,
    operation: F,
) -> futures::future::BoxFuture<'static, Result<echo_core::tools::ToolResult>>
where
    F: FnOnce() -> Result<echo_core::tools::ToolResult> + Send + 'static,
{
    let guards = ctx.resource_guards.clone();
    Box::pin(async move {
        tokio::task::spawn_blocking(move || {
            let _guards = guards;
            operation()
        })
        .await
        .map_err(|error| ToolError::ExecutionFailed {
            tool: tool.to_string(),
            message: format!("File operation task failed: {error}"),
        })?
    })
}

#[cfg(feature = "files")]
fn is_io_kind(error: &ReactError, kind: std::io::ErrorKind) -> bool {
    matches!(error, ReactError::Io(inner) if inner.kind() == kind)
}

/// Add the old local diagnostic without erasing a provider's recovery facts.
#[cfg(feature = "files")]
fn file_error(error: ReactError, local: impl FnOnce(&std::io::Error) -> ToolError) -> ReactError {
    match error {
        ReactError::Io(error) => local(&error).into(),
        error => error,
    }
}

#[cfg(feature = "files")]
fn best_effort_canonicalize(file_system: &dyn FileSystem, path: &Path) -> Result<Option<PathBuf>> {
    match file_system.canonicalize(path) {
        Ok(path) => Ok(Some(path)),
        Err(ReactError::Io(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(feature = "files")]
fn check_file_cancel(
    cancel: Option<&echo_core::agent::CancellationToken>,
    tool: &str,
) -> Result<()> {
    if cancel.is_some_and(|cancel| cancel.is_cancelled()) {
        return Err(ToolError::Cancelled(tool.to_string()).into());
    }
    Ok(())
}
