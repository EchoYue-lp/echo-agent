pub mod diff;
pub mod edit;
#[allow(clippy::module_inception)]
pub mod files;
pub mod glob;
pub mod grep;

use std::path::{Component, Path, PathBuf};

use echo_core::error::{Result, ToolError};

/// Resolve a user-supplied relative/absolute path into a safe absolute path.
/// If base_dir is set, restrict resolution within it; otherwise use the raw path directly.
///
/// - Absolute path: normalize then verify it stays within base_dir
/// - Relative path: expand relative to base_dir then verify
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
        normalized
    } else {
        normalize_path(requested)
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
