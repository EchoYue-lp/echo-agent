pub(crate) mod files;

use std::path::{Component, Path, PathBuf};

use crate::error::{Result, ToolError};

/// 将用户提供的相对/绝对路径解析为安全的绝对路径。
/// 若设置了 base_dir，则限制在其下；否则直接使用原路径。
///
/// - 绝对路径：规范化后直接校验是否在 base_dir 内
/// - 相对路径：以 base_dir 为根展开后校验
fn resolve_path(tool: &str, path_str: &str, base_dir: &Option<PathBuf>) -> Result<PathBuf> {
    let requested = Path::new(path_str);

    let resolved = if let Some(base) = base_dir {
        let normalized_base = normalize_path(base);

        // 相对路径以 base_dir 为根展开；绝对路径直接规范化
        let normalized = if requested.is_absolute() {
            normalize_path(requested)
        } else {
            normalize_path(&normalized_base.join(requested))
        };

        if !normalized.starts_with(&normalized_base) {
            return Err(ToolError::ExecutionFailed {
                tool: tool.to_string(),
                message: format!("路径 '{}' 超出允许的目录范围", path_str),
            }
            .into());
        }
        normalized
    } else {
        normalize_path(requested)
    };

    Ok(resolved)
}

/// 不依赖文件系统的路径规范化（消除 `.` 和 `..`）
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
