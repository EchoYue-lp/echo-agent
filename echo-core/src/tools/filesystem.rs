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

/// Only the metadata consumed by the original file/search algorithms.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileMetadata {
    pub file_type: FileType,
    pub len: u64,
    /// POSIX mode when the target exposes it; unrelated host bits are never used.
    pub mode: Option<u32>,
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

/// File I/O in a single target namespace, independent of product/transport.
///
/// Local errors retain `ReactError::Io` and their actual `ErrorKind`. Remote
/// adapters reuse `ToolError::RemoteFailure` and `ToolFailure` for structured
/// facts, including `PartialSideEffect/Possible/VerifyThenRetry` when a write
/// may have reached the target. Returning an error does not prove cancellation
/// or rollback. Adapters retain the original physical owner until settlement;
/// neither this trait nor its consumers add a retry loop or a second owner.
pub trait FileSystem: Send + Sync {
    /// Whether these path tokens name this process's actual native filesystem.
    /// Non-native providers must never authorize a native-only fallback.
    fn paths_are_native(&self) -> bool {
        false
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
