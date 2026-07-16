pub mod code_search;
pub mod diff;
pub mod edit;
#[allow(clippy::module_inception)]
pub mod files;
pub mod glob;
pub mod grep;
pub mod repo_map;

use std::path::{Component, Path, PathBuf};

use echo_core::error::{Result, ToolError};

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
/// After textual normalization, `std::fs::canonicalize()` is used to resolve symlinks
/// and verify the real path stays within the allowed directory. For write operations
/// where the target file doesn't exist yet, the parent directory is canonicalized instead.
fn resolve_path(
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
        match std::fs::canonicalize(&normalized) {
            Ok(canonical) => {
                let canonical_base = std::fs::canonicalize(&normalized_base).map_err(|e| {
                    ToolError::ExecutionFailed {
                        tool: tool.to_string(),
                        message: format!("Cannot resolve base directory: {}", e),
                    }
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
            Err(_) => {
                // File doesn't exist yet (write operation) — canonicalize parent directory
                if let Some(parent) = normalized.parent()
                    && parent != Path::new("")
                    && let Ok(canonical_parent) = std::fs::canonicalize(parent)
                {
                    let canonical_base = std::fs::canonicalize(&normalized_base)
                        .unwrap_or_else(|_| normalized_base.clone());
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
        }
    } else {
        let normalized = normalize_path(requested);
        // normalize_path strips "." to an empty path — use CWD as fallback
        let normalized = if normalized.as_os_str().is_empty() {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
        } else {
            normalized
        };
        // Best-effort canonicalization when no base_dir constraint
        std::fs::canonicalize(&normalized).unwrap_or(normalized)
    };

    Ok(resolved)
}

/// Filesystem-independent path normalization (resolves `.` and `..`)
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
