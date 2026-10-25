//! Default local implementation of the shared target-aware file I/O boundary.

use echo_core::error::{Result, ToolError};
use echo_core::tools::filesystem::{
    DirectoryIdentityGuard, FileEntry, FileMetadata, FileSystem, FileType,
};
use std::io::Write;
use std::path::{Path, PathBuf};

/// The original local filesystem and optional Git checkpoint behavior.
#[derive(Debug, Default)]
pub struct LocalFileSystem;

fn file_type(kind: std::fs::FileType) -> FileType {
    if kind.is_file() {
        FileType::File
    } else if kind.is_dir() {
        FileType::Directory
    } else if kind.is_symlink() {
        FileType::Symlink
    } else {
        FileType::Other
    }
}

fn metadata(value: std::fs::Metadata) -> FileMetadata {
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        Some(value.permissions().mode())
    };
    #[cfg(not(unix))]
    let mode = None;
    FileMetadata {
        file_type: file_type(value.file_type()),
        len: value.len(),
        mode,
    }
}

struct LocalDirectoryGuard(echo_core::utils::fs::ExistingDirectoryGuard);

impl DirectoryIdentityGuard for LocalDirectoryGuard {
    fn verify(&self, path: &Path) -> Result<()> {
        Ok(echo_core::utils::fs::verify_existing_directory(
            path, &self.0,
        )?)
    }
}

impl FileSystem for LocalFileSystem {
    fn retain_directory(&self, path: &Path) -> Result<Box<dyn DirectoryIdentityGuard>> {
        Ok(Box::new(LocalDirectoryGuard(
            echo_core::utils::fs::open_existing_directory_guard(path)?,
        )))
    }
    fn paths_are_native(&self) -> bool {
        true
    }
    // Preserve the original native Path predicate behavior. Other providers
    // keep the trait's fallible predicates and cannot hide transport failures.
    fn exists(&self, path: &Path) -> Result<bool> {
        Ok(path.exists())
    }
    fn is_file(&self, path: &Path) -> Result<bool> {
        Ok(path.is_file())
    }
    fn is_dir(&self, path: &Path) -> Result<bool> {
        Ok(path.is_dir())
    }
    fn current_dir(&self) -> Result<PathBuf> {
        Ok(std::env::current_dir()?)
    }
    fn canonicalize(&self, path: &Path) -> Result<PathBuf> {
        Ok(std::fs::canonicalize(path)?)
    }
    fn metadata(&self, path: &Path) -> Result<FileMetadata> {
        Ok(metadata(std::fs::metadata(path)?))
    }
    fn symlink_metadata(&self, path: &Path) -> Result<FileMetadata> {
        Ok(metadata(std::fs::symlink_metadata(path)?))
    }
    fn read(&self, path: &Path) -> Result<Vec<u8>> {
        Ok(std::fs::read(path)?)
    }
    fn open_read(&self, path: &Path) -> Result<Box<dyn std::io::Read + Send>> {
        Ok(Box::new(std::fs::File::open(path)?))
    }
    fn read_dir(&self, path: &Path) -> Result<Vec<FileEntry>> {
        std::fs::read_dir(path)?
            .map(|entry| {
                let entry = entry?;
                Ok(FileEntry {
                    path: entry.path(),
                    file_name: entry.file_name(),
                    file_type: file_type(entry.file_type()?),
                })
            })
            .collect()
    }
    fn create_dir(&self, path: &Path) -> Result<()> {
        Ok(std::fs::create_dir(path)?)
    }
    fn create_dir_all(&self, path: &Path) -> Result<()> {
        Ok(std::fs::create_dir_all(path)?)
    }
    fn create_file(&self, path: &Path) -> Result<()> {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        Ok(())
    }
    fn write(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        Ok(std::fs::write(path, bytes)?)
    }
    fn append(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        file.write_all(bytes)?;
        file.flush()?;
        Ok(())
    }
    fn rename(&self, from: &Path, to: &Path) -> Result<()> {
        Ok(std::fs::rename(from, to)?)
    }
    fn remove_file(&self, path: &Path) -> Result<()> {
        Ok(std::fs::remove_file(path)?)
    }
    fn remove_dir(&self, path: &Path) -> Result<()> {
        Ok(std::fs::remove_dir(path)?)
    }
    fn remove_dir_all(&self, path: &Path) -> Result<()> {
        Ok(std::fs::remove_dir_all(path)?)
    }
    fn set_mode(&self, path: &Path, mode: u32) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            Ok(std::fs::set_permissions(
                path,
                std::fs::Permissions::from_mode(mode),
            )?)
        }
        #[cfg(not(unix))]
        {
            let _ = (path, mode);
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "POSIX mode is unavailable on this local target",
            )
            .into())
        }
    }
    fn atomic_write(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        Ok(echo_core::utils::fs::atomic_write(path, bytes)?)
    }
    fn atomic_compare_and_swap(&self, path: &Path, expected: &[u8], bytes: &[u8]) -> Result<bool> {
        Ok(echo_core::utils::fs::atomic_compare_and_swap(
            path,
            bytes,
            |current| current == expected,
        )?)
    }
    fn checkpoint(&self, path: &Path) -> Result<Option<String>> {
        let checkpoint = crate::git_checkpoint::create_checkpoint(path).map_err(|message| {
            ToolError::ExecutionFailed {
                tool: "file_checkpoint".to_string(),
                message,
            }
        })?;
        if checkpoint.is_some() {
            crate::git_checkpoint::cleanup_old_checkpoints(path, 10);
        }
        Ok(checkpoint)
    }
}

#[cfg(all(test, feature = "files"))]
mod target_file_system_tests {
    use super::*;
    use echo_core::error::ReactError;
    use echo_core::tools::{
        ToolContext, ToolFailure, ToolFailureCategory, ToolPack, ToolResult, ToolSideEffect,
    };
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// Two namespaces deliberately use the same visible path. Only actual
    /// provider I/O can select the target bytes; path-string changes cannot.
    struct MappedFileSystem {
        visible: PathBuf,
        target: PathBuf,
        online: AtomicBool,
        writes: AtomicUsize,
        fail_write: AtomicUsize,
        remote_failure: AtomicBool,
        disconnect_after_unknown: AtomicBool,
        unknown_write: AtomicBool,
        unknown_at: AtomicUsize,
    }

    impl MappedFileSystem {
        fn new(visible: &Path, target: &Path) -> Result<Self> {
            Ok(Self {
                visible: std::fs::canonicalize(visible)?,
                target: std::fs::canonicalize(target)?,
                online: AtomicBool::new(true),
                writes: AtomicUsize::new(0),
                fail_write: AtomicUsize::new(0),
                remote_failure: AtomicBool::new(false),
                disconnect_after_unknown: AtomicBool::new(false),
                unknown_write: AtomicBool::new(false),
                unknown_at: AtomicUsize::new(0),
            })
        }
        fn actual(&self, path: &Path) -> Result<PathBuf> {
            if !self.online.load(Ordering::SeqCst) {
                return Err(ToolError::RemoteFailure {
                    tool: "fixture_filesystem".into(),
                    message: "target disconnected".into(),
                    http_status: None,
                    failure: Box::new(ToolFailure::new(ToolFailureCategory::Unavailable)),
                }
                .into());
            }
            let path = if path.is_absolute() {
                path.to_path_buf()
            } else {
                self.visible.join(path)
            };
            let relative =
                path.strip_prefix(&self.visible)
                    .map_err(|_| ToolError::InvalidPath {
                        path: path.display().to_string(),
                        reason: "path is outside the fixture target".into(),
                    })?;
            Ok(self.target.join(relative))
        }
        fn visible(&self, path: &Path) -> Result<PathBuf> {
            let relative = path
                .strip_prefix(&self.target)
                .map_err(|_| ToolError::InvalidPath {
                    path: path.display().to_string(),
                    reason: "target symlink escaped its namespace".into(),
                })?;
            Ok(self.visible.join(relative))
        }
    }

    impl FileSystem for MappedFileSystem {
        fn current_dir(&self) -> Result<PathBuf> {
            Ok(self.visible.clone())
        }
        fn canonicalize(&self, path: &Path) -> Result<PathBuf> {
            self.visible(&LocalFileSystem.canonicalize(&self.actual(path)?)?)
        }
        fn metadata(&self, path: &Path) -> Result<FileMetadata> {
            LocalFileSystem.metadata(&self.actual(path)?)
        }
        fn symlink_metadata(&self, path: &Path) -> Result<FileMetadata> {
            LocalFileSystem.symlink_metadata(&self.actual(path)?)
        }
        fn read(&self, path: &Path) -> Result<Vec<u8>> {
            LocalFileSystem.read(&self.actual(path)?)
        }
        fn open_read(&self, path: &Path) -> Result<Box<dyn std::io::Read + Send>> {
            LocalFileSystem.open_read(&self.actual(path)?)
        }
        fn read_dir(&self, path: &Path) -> Result<Vec<FileEntry>> {
            LocalFileSystem
                .read_dir(&self.actual(path)?)?
                .into_iter()
                .map(|mut entry| {
                    entry.path = self.visible(&entry.path)?;
                    Ok(entry)
                })
                .collect()
        }
        fn create_dir(&self, path: &Path) -> Result<()> {
            LocalFileSystem.create_dir(&self.actual(path)?)
        }
        fn create_dir_all(&self, path: &Path) -> Result<()> {
            LocalFileSystem.create_dir_all(&self.actual(path)?)
        }
        fn create_file(&self, path: &Path) -> Result<()> {
            LocalFileSystem.create_file(&self.actual(path)?)
        }
        fn write(&self, path: &Path, bytes: &[u8]) -> Result<()> {
            LocalFileSystem.write(&self.actual(path)?, bytes)
        }
        fn append(&self, path: &Path, bytes: &[u8]) -> Result<()> {
            LocalFileSystem.append(&self.actual(path)?, bytes)
        }
        fn rename(&self, from: &Path, to: &Path) -> Result<()> {
            LocalFileSystem.rename(&self.actual(from)?, &self.actual(to)?)
        }
        fn remove_file(&self, path: &Path) -> Result<()> {
            LocalFileSystem.remove_file(&self.actual(path)?)
        }
        fn remove_dir(&self, path: &Path) -> Result<()> {
            LocalFileSystem.remove_dir(&self.actual(path)?)
        }
        fn remove_dir_all(&self, path: &Path) -> Result<()> {
            LocalFileSystem.remove_dir_all(&self.actual(path)?)
        }
        fn set_mode(&self, path: &Path, mode: u32) -> Result<()> {
            LocalFileSystem.set_mode(&self.actual(path)?, mode)
        }
        fn checkpoint(&self, path: &Path) -> Result<Option<String>> {
            LocalFileSystem.checkpoint(&self.actual(path)?)
        }
        fn atomic_write(&self, path: &Path, bytes: &[u8]) -> Result<()> {
            let sequence = self.writes.fetch_add(1, Ordering::SeqCst).saturating_add(1);
            if sequence == self.fail_write.load(Ordering::SeqCst) {
                if self.remote_failure.load(Ordering::SeqCst) {
                    return Err(ToolError::RemoteFailure {
                        tool: "fixture_filesystem".into(),
                        message: "definite target refusal before write".into(),
                        http_status: None,
                        failure: Box::new(ToolFailure::new(ToolFailureCategory::Unavailable)),
                    }
                    .into());
                }
                return Err(std::io::Error::other("injected definite pre-write failure").into());
            }
            LocalFileSystem.atomic_write(&self.actual(path)?, bytes)?;
            if self.unknown_write.load(Ordering::SeqCst)
                && (self.unknown_at.load(Ordering::SeqCst) == 0
                    || self.unknown_at.load(Ordering::SeqCst) == sequence)
            {
                if self.disconnect_after_unknown.load(Ordering::SeqCst) {
                    self.online.store(false, Ordering::SeqCst);
                }
                return Err(ToolError::RemoteFailure {
                    tool: "fixture_filesystem".into(),
                    message: "write response lost".into(),
                    http_status: None,
                    failure: Box::new(
                        ToolFailure::new(ToolFailureCategory::PartialSideEffect)
                            .with_side_effect(ToolSideEffect::Possible),
                    ),
                }
                .into());
            }
            Ok(())
        }
    }

    async fn call(
        pack: &crate::StandardToolPack,
        name: &str,
        args: serde_json::Value,
        cwd: &Path,
    ) -> Result<ToolResult> {
        let tool = pack
            .tools()
            .into_iter()
            .find(|entry| entry.tool.name() == name)
            .ok_or_else(|| ToolError::NotFound(name.into()))?
            .tool;
        let params = args
            .as_object()
            .ok_or_else(|| ToolError::MissingParameter("object".into()))?
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<HashMap<_, _>>();
        tool.execute_with_context(
            params,
            &ToolContext {
                working_dir: Some(cwd.to_path_buf()),
                ..Default::default()
            },
        )
        .await
    }

    #[tokio::test]
    async fn original_pack_reads_searches_and_patches_only_the_bound_namespace() -> Result<()> {
        let local = tempfile::tempdir()?;
        let target = tempfile::tempdir()?;
        std::fs::write(local.path().join("same.txt"), "local sentinel\n")?;
        std::fs::write(local.path().join("sample.rs"), "pub fn local_only() {}\n")?;
        std::fs::write(target.path().join("same.txt"), "远端🙂旧值\n")?;
        std::fs::write(target.path().join("sample.rs"), "pub fn target_only() {}\n")?;
        std::fs::write(target.path().join("only_target.txt"), "marker")?;
        let provider = Arc::new(MappedFileSystem::new(local.path(), target.path())?);
        let pack = crate::StandardToolPack::new().with_file_system(provider.clone());
        let cwd = &provider.visible;
        let read = call(
            &pack,
            "read_file",
            serde_json::json!({"path":"same.txt"}),
            cwd,
        )
        .await?;
        assert!(read.success && read.output.contains("远端🙂旧值"));
        let list = call(&pack, "list_dir", serde_json::json!({}), cwd).await?;
        assert!(list.success && list.output.contains("only_target.txt"));
        let grep = call(
            &pack,
            "grep",
            serde_json::json!({"pattern":"远端", "path":"."}),
            cwd,
        )
        .await?;
        assert!(grep.success && grep.output.contains("远端🙂旧值"));
        let glob = call(&pack, "glob", serde_json::json!({"pattern":"*.rs"}), cwd).await?;
        assert!(glob.success && glob.output.contains("sample.rs"));
        let diff = call(
            &pack,
            "diff",
            serde_json::json!({"path_a":"same.txt", "content_b":"other\n"}),
            cwd,
        )
        .await?;
        assert!(diff.success && diff.output.contains("远端🙂旧值"));
        let map = call(
            &pack,
            "repo_map",
            serde_json::json!({"format":"symbols"}),
            cwd,
        )
        .await?;
        assert!(map.success && map.output.contains("target_only"));
        assert!(!map.output.contains("local_only"));
        let symbols = call(
            &pack,
            "code_search",
            serde_json::json!({"query":"target_only", "symbol_type":"function"}),
            cwd,
        )
        .await?;
        assert!(symbols.success && symbols.output.contains("target_only"));
        let patch = "*** Begin Patch\n*** Update File: same.txt\n@@\n-远端🙂旧值\n+远端🙂新值\n*** End Patch";
        let result = call(
            &pack,
            "apply_patch",
            serde_json::json!({"patch":patch}),
            cwd,
        )
        .await?;
        assert!(result.success, "{:?}", result.error);
        assert_eq!(
            std::fs::read_to_string(target.path().join("same.txt"))?,
            "远端🙂新值\n"
        );
        assert_eq!(
            std::fs::read_to_string(local.path().join("same.txt"))?,
            "local sentinel\n"
        );
        Ok(())
    }

    #[tokio::test]
    async fn invocation_checkout_is_not_replaced_by_the_host_root() -> Result<()> {
        let local = tempfile::tempdir()?;
        let target = tempfile::tempdir()?;
        std::fs::create_dir(target.path().join("checkout"))?;
        std::fs::write(target.path().join("same.txt"), "host default")?;
        std::fs::write(
            target.path().join("checkout/same.txt"),
            "actual checkout 中文",
        )?;
        let provider = Arc::new(MappedFileSystem::new(local.path(), target.path())?);
        let pack = crate::StandardToolPack::new().with_file_system(provider.clone());
        let result = call(
            &pack,
            "read_file",
            serde_json::json!({"path":"same.txt"}),
            &provider.visible.join("checkout"),
        )
        .await?;
        assert!(result.success && result.output.contains("actual checkout 中文"));
        assert!(!result.output.contains("host default"));
        Ok(())
    }

    #[tokio::test]
    async fn disconnected_target_never_reads_local_same_name() -> Result<()> {
        let local = tempfile::tempdir()?;
        let target = tempfile::tempdir()?;
        std::fs::write(local.path().join("same.txt"), "must not be read")?;
        let provider = Arc::new(MappedFileSystem::new(local.path(), target.path())?);
        provider.online.store(false, Ordering::SeqCst);
        let pack = crate::StandardToolPack::new().with_file_system(provider.clone());
        let error = call(
            &pack,
            "read_file",
            serde_json::json!({"path":"same.txt"}),
            &provider.visible,
        )
        .await
        .err()
        .ok_or_else(|| ToolError::ExecutionFailed {
            tool: "fixture".into(),
            message: "disconnected provider unexpectedly succeeded".into(),
        })?;
        assert!(
            matches!(error, ReactError::Tool(inner) if matches!(inner.as_ref(), ToolError::RemoteFailure { .. }))
        );
        assert_eq!(provider.writes.load(Ordering::SeqCst), 0);
        Ok(())
    }

    #[tokio::test]
    async fn stale_preimage_keeps_both_namespaces_unchanged() -> Result<()> {
        let local = tempfile::tempdir()?;
        let target = tempfile::tempdir()?;
        std::fs::write(local.path().join("same.txt"), "local\n")?;
        std::fs::write(target.path().join("same.txt"), "changed externally\n")?;
        let provider = Arc::new(MappedFileSystem::new(local.path(), target.path())?);
        let pack = crate::StandardToolPack::new().with_file_system(provider.clone());
        let result = call(&pack, "apply_patch", serde_json::json!({"patch":"*** Begin Patch\n*** Update File: same.txt\n@@\n-old\n+new\n*** End Patch"}), &provider.visible).await?;
        assert!(!result.success);
        assert_eq!(provider.writes.load(Ordering::SeqCst), 0);
        assert_eq!(
            std::fs::read_to_string(target.path().join("same.txt"))?,
            "changed externally\n"
        );
        assert_eq!(
            std::fs::read_to_string(local.path().join("same.txt"))?,
            "local\n"
        );
        Ok(())
    }

    #[tokio::test]
    async fn definite_write_failure_uses_the_original_target_rollback() -> Result<()> {
        let local = tempfile::tempdir()?;
        let target = tempfile::tempdir()?;
        std::fs::write(target.path().join("first.txt"), "one\n")?;
        std::fs::write(target.path().join("second.txt"), "two\n")?;
        let provider = Arc::new(MappedFileSystem::new(local.path(), target.path())?);
        provider.fail_write.store(2, Ordering::SeqCst);
        let pack = crate::StandardToolPack::new().with_file_system(provider.clone());
        let result = call(&pack, "apply_patch", serde_json::json!({"patch":"*** Begin Patch\n*** Update File: first.txt\n@@\n-one\n+changed\n*** Update File: second.txt\n@@\n-two\n+changed\n*** End Patch"}), &provider.visible).await?;
        assert!(!result.success);
        assert_eq!(
            std::fs::read_to_string(target.path().join("first.txt"))?,
            "one\n"
        );
        assert_eq!(
            std::fs::read_to_string(target.path().join("second.txt"))?,
            "two\n"
        );
        assert_eq!(provider.writes.load(Ordering::SeqCst), 3);
        Ok(())
    }

    #[tokio::test]
    async fn unknown_write_is_not_replayed_or_speculatively_rolled_back() -> Result<()> {
        let local = tempfile::tempdir()?;
        let target = tempfile::tempdir()?;
        std::fs::write(local.path().join("same.txt"), "local sentinel\n")?;
        std::fs::write(target.path().join("same.txt"), "old\n")?;
        let provider = Arc::new(MappedFileSystem::new(local.path(), target.path())?);
        provider.unknown_write.store(true, Ordering::SeqCst);
        let pack = crate::StandardToolPack::new().with_file_system(provider.clone());
        let result = call(&pack, "apply_patch", serde_json::json!({"patch":"*** Begin Patch\n*** Update File: same.txt\n@@\n-old\n+new\n*** End Patch"}), &provider.visible).await?;
        assert!(!result.success);
        let failure = result
            .failure
            .as_ref()
            .ok_or_else(|| ToolError::MissingParameter("typed failure".into()))?;
        assert_eq!(failure.side_effect, ToolSideEffect::Possible);
        assert!(!failure.allows_automatic_retry());
        assert_eq!(provider.writes.load(Ordering::SeqCst), 1);
        assert_eq!(
            std::fs::read_to_string(target.path().join("same.txt"))?,
            "new\n"
        );
        assert_eq!(
            std::fs::read_to_string(local.path().join("same.txt"))?,
            "local sentinel\n"
        );
        assert!(result.metadata.contains_key("unsettled_paths"));
        Ok(())
    }

    #[tokio::test]
    async fn uncertain_undo_stops_the_remaining_original_rollback() -> Result<()> {
        let local = tempfile::tempdir()?;
        let target = tempfile::tempdir()?;
        for name in ["a.txt", "b.txt", "c.txt"] {
            std::fs::write(target.path().join(name), "old\n")?;
        }
        let provider = Arc::new(MappedFileSystem::new(local.path(), target.path())?);
        provider.fail_write.store(3, Ordering::SeqCst);
        provider.unknown_write.store(true, Ordering::SeqCst);
        provider.unknown_at.store(4, Ordering::SeqCst);
        let pack = crate::StandardToolPack::new().with_file_system(provider.clone());
        let result = call(&pack, "apply_patch", serde_json::json!({"patch":"*** Begin Patch\n*** Update File: a.txt\n@@\n-old\n+new\n*** Update File: b.txt\n@@\n-old\n+new\n*** Update File: c.txt\n@@\n-old\n+new\n*** End Patch"}), &provider.visible).await?;
        assert!(!result.success);
        assert_eq!(provider.writes.load(Ordering::SeqCst), 4);
        assert_eq!(
            std::fs::read_to_string(target.path().join("a.txt"))?,
            "new\n"
        );
        assert_eq!(
            std::fs::read_to_string(target.path().join("c.txt"))?,
            "old\n"
        );
        let failure = result
            .failure
            .as_ref()
            .ok_or_else(|| ToolError::MissingParameter("typed failure".into()))?;
        assert_eq!(failure.side_effect, ToolSideEffect::Possible);
        assert!(!failure.allows_automatic_retry());
        Ok(())
    }

    #[tokio::test]
    async fn definite_target_refusal_retains_its_typed_category_after_verified_rollback()
    -> Result<()> {
        let local = tempfile::tempdir()?;
        let target = tempfile::tempdir()?;
        std::fs::write(target.path().join("same.txt"), "old\n")?;
        let provider = Arc::new(MappedFileSystem::new(local.path(), target.path())?);
        provider.fail_write.store(1, Ordering::SeqCst);
        provider.remote_failure.store(true, Ordering::SeqCst);
        let pack = crate::StandardToolPack::new().with_file_system(provider.clone());
        let result = call(&pack, "apply_patch", serde_json::json!({"patch":"*** Begin Patch\n*** Update File: same.txt\n@@\n-old\n+new\n*** End Patch"}), &provider.visible).await?;
        let failure = result
            .failure
            .ok_or_else(|| ToolError::MissingParameter("typed failure".into()))?;
        assert_eq!(failure.category, ToolFailureCategory::Unavailable);
        assert_eq!(failure.side_effect, ToolSideEffect::None);
        assert_eq!(provider.writes.load(Ordering::SeqCst), 1);
        assert_eq!(
            std::fs::read_to_string(target.path().join("same.txt"))?,
            "old\n"
        );
        Ok(())
    }

    #[tokio::test]
    async fn disconnected_rollback_observation_cannot_authorize_a_patch_retry() -> Result<()> {
        let local = tempfile::tempdir()?;
        let target = tempfile::tempdir()?;
        for name in ["a.txt", "b.txt", "c.txt"] {
            std::fs::write(target.path().join(name), "old\n")?;
        }
        let provider = Arc::new(MappedFileSystem::new(local.path(), target.path())?);
        provider.fail_write.store(3, Ordering::SeqCst);
        provider.unknown_write.store(true, Ordering::SeqCst);
        provider.unknown_at.store(4, Ordering::SeqCst);
        provider
            .disconnect_after_unknown
            .store(true, Ordering::SeqCst);
        let pack = crate::StandardToolPack::new().with_file_system(provider.clone());
        let result = call(&pack, "apply_patch", serde_json::json!({"patch":"*** Begin Patch\n*** Update File: a.txt\n@@\n-old\n+new\n*** Update File: b.txt\n@@\n-old\n+new\n*** Update File: c.txt\n@@\n-old\n+new\n*** End Patch"}), &provider.visible).await?;
        let failure = result
            .failure
            .ok_or_else(|| ToolError::MissingParameter("typed failure".into()))?;
        assert_eq!(failure.category, ToolFailureCategory::PartialSideEffect);
        assert_eq!(failure.side_effect, ToolSideEffect::Possible);
        assert!(!failure.allows_automatic_retry());
        assert_eq!(provider.writes.load(Ordering::SeqCst), 4);
        assert_eq!(
            std::fs::read_to_string(target.path().join("a.txt"))?,
            "new\n"
        );
        assert!(result.metadata.contains_key("unsettled_paths"));
        Ok(())
    }

    #[test]
    fn project_rules_use_the_same_target_namespace() -> Result<()> {
        let local = tempfile::tempdir()?;
        let target = tempfile::tempdir()?;
        std::fs::write(local.path().join("AGENTS.md"), "local rules")?;
        std::fs::write(target.path().join("AGENTS.md"), "target rules 中文")?;
        let provider = MappedFileSystem::new(local.path(), target.path())?;
        let resolved = echo_core::project_rules::InstructionResolver::new(&provider.visible)
            .project_root(&provider.visible)
            .agents_files_only()
            .resolve_with_file_system(&provider)?;
        assert!(resolved.content.contains("target rules 中文"));
        assert!(!resolved.content.contains("local rules"));
        Ok(())
    }
    #[test]
    fn local_directory_guard_rejects_path_replacement() -> Result<()> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("checkout");
        LocalFileSystem.create_dir(&path)?;
        let guard = LocalFileSystem.retain_directory(&path)?;
        guard.verify(&path)?;
        LocalFileSystem.rename(&path, &root.path().join("original"))?;
        LocalFileSystem.create_dir(&path)?;
        assert!(guard.verify(&path).is_err());
        Ok(())
    }

    #[test]
    fn stream_reads_the_target_without_loading_same_named_local_bytes() -> Result<()> {
        use std::io::Read;
        let local = tempfile::tempdir()?;
        let target = tempfile::tempdir()?;
        std::fs::write(local.path().join("same.txt"), "local sentinel")?;
        std::fs::write(target.path().join("same.txt"), "目标🙂 streamed")?;
        let provider = MappedFileSystem::new(local.path(), target.path())?;
        let mut reader = provider.open_read(&provider.visible.join("same.txt"))?;
        let mut bytes = Vec::new();
        let mut chunk = [0_u8; 7];
        loop {
            let read = reader.read(&mut chunk)?;
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(
                chunk
                    .get(..read)
                    .ok_or_else(|| std::io::Error::other("invalid reader length"))?,
            );
        }
        assert_eq!(
            String::from_utf8(bytes).map_err(std::io::Error::other)?,
            "目标🙂 streamed"
        );
        Ok(())
    }

    #[cfg(feature = "git")]
    struct ListingExecutor {
        files: Arc<MappedFileSystem>,
        calls: AtomicUsize,
        disconnect_at_marker: AtomicBool,
    }

    #[cfg(feature = "git")]
    impl echo_core::sandbox::SandboxExecutor for ListingExecutor {
        fn name(&self) -> &str {
            "target-git-fixture"
        }
        fn isolation_level(&self) -> echo_core::sandbox::IsolationLevel {
            echo_core::sandbox::IsolationLevel::None
        }
        fn is_available(&self) -> futures::future::BoxFuture<'_, bool> {
            Box::pin(async { true })
        }
        fn execute(
            &self,
            command: echo_core::sandbox::SandboxCommand,
        ) -> futures::future::BoxFuture<'_, Result<echo_core::sandbox::ExecutionResult>> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                assert_eq!(command.working_dir.as_ref(), Some(&self.files.visible));
                assert_eq!(command.resource_guards.len(), 1);
                assert!(command.cancel.is_some());
                let echo_core::sandbox::CommandKind::Program { program, args } = command.kind
                else {
                    return Err(std::io::Error::other("expected structured Git command").into());
                };
                assert_eq!(program, "git");
                let stdout = if args == ["rev-parse", "--show-toplevel"] {
                    format!("{}\n", self.files.visible.display())
                } else if args == ["worktree", "list", "--porcelain"] {
                    format!(
                        "worktree {}\nHEAD fixture\nbranch refs/heads/target-only\n\n",
                        self.files.visible.display()
                    )
                } else if args
                    == [
                        "rev-parse",
                        "--path-format=absolute",
                        "--git-path",
                        "echo-agent-managed",
                    ]
                {
                    if self.disconnect_at_marker.load(Ordering::SeqCst) {
                        self.files.online.store(false, Ordering::SeqCst);
                    }
                    format!(
                        "{}\n",
                        self.files.visible.join(".git/echo-agent-managed").display()
                    )
                } else {
                    return Err(std::io::Error::other("unexpected Git operation").into());
                };
                Ok(echo_core::sandbox::ExecutionResult {
                    stdout_bytes: u64::try_from(stdout.len()).unwrap_or(u64::MAX),
                    stdout,
                    stderr: String::new(),
                    exit_code: 0,
                    duration: std::time::Duration::ZERO,
                    sandbox_type: "fixture".into(),
                    timed_out: false,
                    cancelled: false,
                    output_truncated: false,
                    stderr_bytes: 0,
                })
            })
        }
    }

    #[cfg(feature = "git")]
    #[tokio::test]
    async fn original_worktree_list_uses_target_git_and_marker_but_mutations_do_not_start()
    -> Result<()> {
        let local = tempfile::tempdir()?;
        let target = tempfile::tempdir()?;
        std::fs::create_dir_all(local.path().join(".git"))?;
        std::fs::write(
            local.path().join(".git/echo-agent-managed"),
            "local sentinel",
        )?;
        let files = Arc::new(MappedFileSystem::new(local.path(), target.path())?);
        let executor = Arc::new(ListingExecutor {
            files: files.clone(),
            calls: AtomicUsize::new(0),
            disconnect_at_marker: AtomicBool::new(false),
        });
        let pack = crate::StandardToolPack::new().with_file_system(files.clone());
        let context = ToolContext {
            working_dir: Some(files.visible.clone()),
            resource_guards: vec![echo_core::tools::InvocationResourceGuard::new(())],
            cancel: Some(Arc::new(echo_core::agent::CancellationToken::new())),
            ..Default::default()
        };
        for name in ["enter_worktree", "exit_worktree", "list_worktrees"] {
            let entry = pack
                .tools()
                .into_iter()
                .find(|entry| entry.tool.name() == name)
                .ok_or_else(|| ToolError::NotFound(name.into()))?;
            assert!(entry.tool.set_sandbox(executor.clone()));
            let result = entry
                .tool
                .execute_with_context(HashMap::new(), &context)
                .await?;
            if name == "list_worktrees" {
                assert!(result.output.contains("target-only"));
                assert_eq!(executor.calls.load(Ordering::SeqCst), 3);
                executor.disconnect_at_marker.store(true, Ordering::SeqCst);
                let disconnected = entry
                    .tool
                    .execute_with_context(HashMap::new(), &context)
                    .await;
                assert!(
                    matches!(disconnected, Err(ReactError::Tool(error)) if matches!(*error, ToolError::RemoteFailure { .. }))
                );
            } else {
                assert_eq!(
                    result.failure.map(|failure| failure.category),
                    Some(ToolFailureCategory::Unavailable)
                );
                assert!(entry.tool.description().contains("Unavailable"));
                assert_eq!(executor.calls.load(Ordering::SeqCst), 0);
                assert!(!local.path().join(".worktrees").exists());
                assert!(!target.path().join(".worktrees").exists());
            }
        }
        assert_eq!(
            std::fs::read_to_string(local.path().join(".git/echo-agent-managed"))?,
            "local sentinel"
        );
        Ok(())
    }
}
