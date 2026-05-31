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
/// If base_dir is set, restrict resolution within it; otherwise use the raw path directly.
///
/// - Absolute path: normalize then verify it stays within base_dir
/// - Relative path: expand relative to base_dir then verify
///
/// After textual normalization, `std::fs::canonicalize()` is used to resolve symlinks
/// and verify the real path stays within the allowed directory. For write operations
/// where the target file doesn't exist yet, the parent directory is canonicalized instead.
fn resolve_path(tool: &str, path_str: &str, base_dir: &Option<PathBuf>) -> Result<PathBuf> {
    let requested = Path::new(path_str);

    let resolved = if let Some(base) = base_dir {
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
                message: format!("Path '{}' is outside the allowed directory scope", path_str),
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
                if let Some(parent) = normalized.parent() {
                    if parent != Path::new("") {
                        if let Ok(canonical_parent) = std::fs::canonicalize(parent) {
                            let canonical_base =
                                std::fs::canonicalize(&normalized_base).unwrap_or_else(|_| {
                                    normalized_base.clone()
                                });
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
                            let filename = normalized
                                .file_name()
                                .unwrap_or_default();
                            return Ok(canonical_parent.join(filename));
                        }
                    }
                }
                // Fallback: return textually normalized path if parent doesn't exist
                normalized
            }
        }
    } else {
        let normalized = normalize_path(requested);
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
