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
/// 相对路径先按所选 base 展开一次；绝对路径保原路径交 provider。
/// selected provider 在词法归一化前解析 symlink/..，再核真实目录上界。
/// 写目标尚不存在时，解析同 provider 的已有祖先后重建缺失后缀；
/// 已展开的递归目录项直接验证，不重复拼接 base。
#[cfg(feature = "files")]
fn resolve_path(
    file_system: &dyn FileSystem,
    tool: &str,
    path_str: &str,
    base_dir: &Option<PathBuf>,
    working_dir: Option<&Path>,
) -> Result<PathBuf> {
    let requested = Path::new(path_str);
    let base = base_dir.as_deref().or(working_dir);
    let expanded = if requested.is_relative() {
        base.map(|base| base.join(requested))
            .unwrap_or_else(|| requested.to_path_buf())
    } else {
        requested.to_path_buf()
    };
    resolve_path_buf(file_system, tool, &expanded, base_dir)
}

/// 校验已展开的实际 I/O 路径，目录项不再拼接 base、不经有损 UTF-8 转换。
/// 先让 provider 解析 symlink/..；提前词法折叠会改变实际读取对象。
#[cfg(feature = "files")]
fn resolve_path_buf(
    file_system: &dyn FileSystem,
    tool: &str,
    expanded: &Path,
    base_dir: &Option<PathBuf>,
) -> Result<PathBuf> {
    let resolved = match file_system.canonicalize(expanded) {
        Ok(path) => path,
        Err(ReactError::Io(_)) if base_dir.is_some() => {
            best_effort_canonicalize(file_system, expanded)?
                .unwrap_or_else(|| normalize_path(expanded))
        }
        Err(ReactError::Io(_)) => {
            // 无显式根时保原 raw + best-effort 行为，不新增祖先范围推断。
            let normalized = normalize_path(expanded);
            if normalized.as_os_str().is_empty() {
                file_system.current_dir()?
            } else {
                normalized
            }
        }
        Err(error) => return Err(error),
    };
    if let Some(base) = base_dir {
        let canonical_base =
            best_effort_canonicalize(file_system, base)?.unwrap_or_else(|| normalize_path(base));
        if !resolved.starts_with(&canonical_base) {
            return Err(ToolError::ExecutionFailed {
                tool: tool.to_string(),
                message: format!(
                    "Path '{}' resolves outside allowed directory scope '{}'",
                    expanded.display(),
                    canonical_base.display()
                ),
            }
            .into());
        }
    }
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
    let mut ancestor = path;
    let mut missing = Vec::new();
    loop {
        let candidate = if ancestor.as_os_str().is_empty() {
            file_system.current_dir()?
        } else {
            ancestor.to_path_buf()
        };
        match file_system.canonicalize(&candidate) {
            Ok(mut canonical) => {
                // 写目标尚不存在时，沿同 provider 找到真实已有祖先，再接回缺失后缀。
                // 不先折叠 symlink/..，也不把 typed provider 错误伪装成本机缺失。
                for component in missing.iter().rev() {
                    canonical.push(component);
                }
                return Ok(Some(normalize_path(&canonical)));
            }
            Err(ReactError::Io(_)) => {
                let Some(component) = ancestor.components().next_back() else {
                    return Ok(None);
                };
                if matches!(component, Component::RootDir | Component::Prefix(_)) {
                    return Ok(None);
                }
                let Some(parent) = ancestor.parent() else {
                    return Ok(None);
                };
                missing.push(component.as_os_str().to_os_string());
                ancestor = parent;
            }
            Err(error) => return Err(error),
        }
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
