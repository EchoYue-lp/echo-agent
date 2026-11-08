//! Location-bound file operations for the existing tools and context loaders.
//!
//! A provider owns one immutable target. A [`Path`] is a path in that target's
//! namespace, never an address or proof that a file exists on the local host.
//! Callers must run these synchronous operations on their existing blocking
//! boundary and retain invocation resource guards until the operation settles.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::error::{ReactError, Result};

/// Portable file kind; adapters do not manufacture local `std::fs::Metadata`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileType {
    File,
    Directory,
    Symlink,
    Other,
}

impl FileType {
    pub fn is_file(self) -> bool {
        self == Self::File
    }
    pub fn is_dir(self) -> bool {
        self == Self::Directory
    }
    pub fn is_symlink(self) -> bool {
        self == Self::Symlink
    }
}

/// Metadata a provider must report without reading the file body.
///
/// `file_type`/`len`/`mode` serve the original file/search algorithms; `modified`
/// is additive so upper layers can build a cheap change fingerprint from stat
/// alone (length alone misses same-length edits).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileMetadata {
    pub file_type: FileType,
    pub len: u64,
    /// POSIX mode when the target exposes it; unrelated host bits are never used.
    pub mode: Option<u32>,
    /// 目标文件系统的修改时间；提供者不暴露时保持 `None`，不得用本机时间填充。
    pub modified: Option<std::time::SystemTime>,
}

impl FileMetadata {
    pub fn is_file(&self) -> bool {
        self.file_type.is_file()
    }
    pub fn is_dir(&self) -> bool {
        self.file_type.is_dir()
    }
    pub fn len(&self) -> u64 {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// One entry, including its type without following a directory-entry symlink.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileEntry {
    pub path: PathBuf,
    pub file_name: OsString,
    pub file_type: FileType,
}

/// A retained, no-follow directory identity, not a path/mtime comparison.
/// The provider must keep the original identity alive until this guard drops.
pub trait DirectoryIdentityGuard: Send + Sync {
    fn verify(&self, path: &Path) -> Result<()>;
}

/// 一次有界只读阶段借用的文件通道。它不是跨请求内容缓存。
///
/// 调用方在原 blocking 边界使用并显式 finish，以传播关闭失败；实现的
/// Drop 也必须结算原资源，不能以丢弃观察者冒充操作已经停止。
pub trait FileSystemReadSession: Send {
    fn files(&self) -> &dyn FileSystem;
    fn finish(self: Box<Self>) -> Result<()>;
}

/// File I/O in a single target namespace, independent of product/transport.
///
/// Local errors retain `ReactError::Io` and their actual `ErrorKind`. Remote
/// adapters reuse `ToolError::RemoteFailure` and `ToolFailure` for structured
/// facts, including `PartialSideEffect/Possible/VerifyThenRetry` when a write
/// may have reached the target. Returning an error does not prove cancellation
/// or rollback. Adapters retain the original physical owner until settlement;
/// neither this trait nor its consumers add a retry loop or a second owner.
pub trait FileSystem: Send + Sync {
    /// 可选的只读传输复用接缝。默认直接使用原提供者，无新资源或缓存。
    /// 返回的会话只用于当前准备阶段，不能被后续 invocation 保留。
    ///
    /// ```no_run
    /// use std::path::Path;
    /// use echo_core::{error::Result, tools::filesystem::FileSystem};
    /// fn read_rules(provider: &dyn FileSystem, path: &Path) -> Result<Vec<u8>> {
    ///     let session = provider.begin_read_session()?;
    ///     let files = session.as_ref().map(|s| s.files()).unwrap_or(provider);
    ///     let result = files.read(path);
    ///     if let Some(session) = session { session.finish()?; }
    ///     result
    /// }
    /// ```
    fn begin_read_session(&self) -> Result<Option<Box<dyn FileSystemReadSession>>> {
        Ok(None)
    }

    /// 按输入优先级返回首个存在的路径；在此前发生的错误必须传播。
    /// 提供者可以有界流水化 metadata，但不能枚举无关目录项或隐藏早期错误。
    fn first_existing_path(&self, paths: &[PathBuf]) -> Result<Option<PathBuf>> {
        for path in paths {
            if self.exists(path)? {
                return Ok(Some(path.clone()));
            }
        }
        Ok(None)
    }
    /// Whether these path tokens name this process's actual native filesystem.
    /// Non-native providers must never authorize a native-only fallback.
    fn paths_are_native(&self) -> bool {
        false
    }
    /// 该提供者所代表的物理目标的稳定命名空间；`None` 表示不参与跨消息缓存。
    ///
    /// 上层可能缓存"读到的文件正文 + stat 指纹"，用绝对路径做次级键；如果只有
    /// 路径，两个不同远端主机上的 `/home/u/proj/AGENTS.md` 会互相污染。因此值
    /// 必须是"同一物理目标稳定、不同目标不同"的事实（例如远端连接已有的稳定
    /// 身份 + 目标根目录），不能用进程内指针、随机数或本次调用时间。
    ///
    /// 默认 `None`：框架不假设任何提供者可以跨请求复用内容。本地读取便宜且
    /// 目录可能被远程操作影响，保持不参与；缓存条目、容量和失效窗口全部由
    /// 调用方拥有，本 trait 不新增资源或副作用。
    fn cache_namespace(&self) -> Option<String> {
        None
    }
    /// Retain the same directory identity across a protected operation. A
    /// backend without stable handle/reopen identity must reject this before
    /// reserving or mutating a managed worktree; stat timestamps are insufficient.
    fn retain_directory(&self, path: &Path) -> Result<Box<dyn DirectoryIdentityGuard>> {
        let _ = path;
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "retained no-follow directory identity is unavailable for this provider",
        )
        .into())
    }
    fn current_dir(&self) -> Result<PathBuf>;
    fn canonicalize(&self, path: &Path) -> Result<PathBuf>;
    /// Follows symlinks, like `std::fs::metadata`.
    fn metadata(&self, path: &Path) -> Result<FileMetadata>;
    /// Inspect the link itself for strict project-rule/extension reads.
    fn symlink_metadata(&self, path: &Path) -> Result<FileMetadata>;
    fn read(&self, path: &Path) -> Result<Vec<u8>>;
    /// Open the same target for bounded streaming reads. Consumers retain their
    /// existing digest/size algorithm and fixed-size buffer. Implementations
    /// must not preload the file into a Vec before returning the reader.
    fn open_read(&self, path: &Path) -> Result<Box<dyn std::io::Read + Send>> {
        let _ = path;
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "streaming reads are unavailable for this provider",
        )
        .into())
    }
    fn read_dir(&self, path: &Path) -> Result<Vec<FileEntry>>;
    /// Exclusive directory creation, used by existing isolation owners.
    fn create_dir(&self, path: &Path) -> Result<()> {
        let _ = path;
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "exclusive directory creation is unavailable for this provider",
        )
        .into())
    }
    fn create_dir_all(&self, path: &Path) -> Result<()>;
    /// Exclusive creation: an existing file must not be truncated.
    fn create_file(&self, path: &Path) -> Result<()>;
    fn write(&self, path: &Path, bytes: &[u8]) -> Result<()>;
    fn append(&self, path: &Path, bytes: &[u8]) -> Result<()>;
    fn rename(&self, from: &Path, to: &Path) -> Result<()>;
    fn remove_file(&self, path: &Path) -> Result<()>;
    fn remove_dir(&self, path: &Path) -> Result<()>;
    fn remove_dir_all(&self, path: &Path) -> Result<()>;
    fn set_mode(&self, path: &Path, mode: u32) -> Result<()>;
    /// Replace bytes atomically or return an explicit unsupported diagnostic.
    /// Preserve native parent creation before the replacement. A transport
    /// must not implement this with a non-atomic truncate/write.
    fn atomic_write(&self, path: &Path, bytes: &[u8]) -> Result<()>;
    /// Compare and replace under the target's original cooperating-writer lock.
    /// A read followed by atomic_write is not a compare-and-swap operation.
    /// Only writers using that same lock participate; external editors are not
    /// fenced. An error may occur after replacement, so it is not proof that
    /// the original bytes are unchanged.
    fn atomic_compare_and_swap(&self, path: &Path, expected: &[u8], bytes: &[u8]) -> Result<bool> {
        let _ = (path, expected, bytes);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "atomic compare-and-swap is unavailable for this provider",
        )
        .into())
    }
    /// Preserve the original optional recovery checkpoint before mutation.
    /// `None` means this target is not checkpoint-managed (for example, not a
    /// Git repository), never a fallback to checking a local same-name path.
    fn checkpoint(&self, path: &Path) -> Result<Option<String>>;

    fn exists(&self, path: &Path) -> Result<bool> {
        match self.metadata(path) {
            Ok(_) => Ok(true),
            Err(ReactError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    fn is_file(&self, path: &Path) -> Result<bool> {
        match self.metadata(path) {
            Ok(metadata) => Ok(metadata.is_file()),
            Err(ReactError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    fn is_dir(&self, path: &Path) -> Result<bool> {
        match self.metadata(path) {
            Ok(metadata) => Ok(metadata.is_dir()),
            Err(ReactError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    fn read_to_string(&self, path: &Path) -> Result<String> {
        String::from_utf8(self.read(path)?)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error).into())
    }
}

/// 在提供者支持的只读会话作用域内执行一次多文件读取。
///
/// 支持会话的提供者（例如远程工作区）可以把一次工具或准备阶段的多次读取
/// 复用到同一条传输上；不支持的提供者（例如本地文件系统）直接执行，
/// 行为与逐次读取完全一致。
///
/// 返回第二项表示本次是否实际建立了会话，供调用方记录结构化日志，
/// 不参与任何正确性判断。读取已经失败时保留原读取错误，关闭失败只记录，
/// 不覆盖它；读取成功时关闭失败必须传播，不能假装会话正常结束。
pub fn with_read_session<T>(
    files: &dyn FileSystem,
    read: impl FnOnce(&dyn FileSystem) -> Result<T>,
) -> (Result<T>, bool) {
    let session = match files.begin_read_session() {
        Ok(session) => session,
        Err(error) => return (Err(error), false),
    };
    let reused = session.is_some();
    let scoped = session
        .as_ref()
        .map(|session| session.files())
        .unwrap_or(files);
    let result = read(scoped);
    let closed = match session {
        Some(session) => session.finish(),
        None => Ok(()),
    };
    let result = match (result, closed) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), closure) => {
            if let Err(close_error) = closure {
                tracing::error!(%close_error, "read session also failed to close");
            }
            Err(error)
        }
        (Ok(_), Err(error)) => Err(error),
    };
    (result, reused)
}

impl FileEntry {
    pub fn path(&self) -> PathBuf {
        self.path.clone()
    }
    pub fn file_name(&self) -> OsString {
        self.file_name.clone()
    }
    pub fn file_type(&self) -> FileType {
        self.file_type
    }
}
