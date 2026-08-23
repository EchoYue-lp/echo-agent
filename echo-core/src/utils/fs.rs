//! Filesystem safety primitives shared by framework crates.

use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

/// Lifetime-held exclusive lease for one file-backed authority.
#[derive(Debug)]
pub struct ExclusiveFileLease {
    file: std::fs::File,
    path: PathBuf,
}

impl ExclusiveFileLease {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ExclusiveFileLease {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// Acquire a non-blocking exclusive sidecar lease for `authority_path`.
///
/// One process owns the lease for the authority lifetime. Additional handles
/// in that process must share the same authority rather than acquiring another
/// lease; a competing process fails open instead of silently racing writes.
pub fn try_exclusive_file_lease(authority_path: &Path) -> std::io::Result<ExclusiveFileLease> {
    use fs2::FileExt;

    let parent = authority_path.parent().ok_or_else(|| {
        std::io::Error::other(format!(
            "authority path has no parent: {}",
            authority_path.display()
        ))
    })?;
    std::fs::create_dir_all(parent)?;
    let name = authority_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("authority");
    let path = parent.join(format!(".{name}.lease"));
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)?;
    file.try_lock_exclusive().map_err(|error| {
        std::io::Error::new(
            error.kind(),
            format!(
                "file-backed authority {} is already open in another process: {error}",
                authority_path.display()
            ),
        )
    })?;
    Ok(ExclusiveFileLease { file, path })
}

/// Validate an external identifier before using it as one filesystem segment.
pub fn validate_path_segment(value: &str) -> std::io::Result<&str> {
    if value.is_empty()
        || Path::new(value).is_absolute()
        || Path::new(value)
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || value.contains('/')
        || value.contains('\\')
        || value.chars().any(char::is_control)
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("value is not a safe path segment: {value:?}"),
        ));
    }
    Ok(value)
}

/// Join a validated external identifier beneath `root`.
pub fn join_path_segment(root: &Path, value: &str) -> std::io::Result<PathBuf> {
    let joined = root.join(validate_path_segment(value)?);
    match std::fs::symlink_metadata(&joined) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "path segment resolves through a symlink: {}",
                joined.display()
            ),
        )),
        Ok(_) => Ok(joined),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(joined),
        Err(error) => Err(error),
    }
}

/// Atomically replace a file using a unique sibling temp file and durable rename.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other(format!("path has no parent: {}", path.display())))?;
    std::fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("data");
    let tmp = parent.join(format!(".{file_name}.{}.tmp", uuid::Uuid::new_v4()));
    let existing_permissions = std::fs::metadata(path)
        .ok()
        .map(|metadata| metadata.permissions());
    let write_result = (|| {
        let mut file = std::fs::File::create(&tmp)?;
        if let Some(permissions) = existing_permissions {
            file.set_permissions(permissions)?;
        }
        file.write_all(bytes)?;
        file.sync_all()
    })();
    if let Err(error) = write_result {
        let _ = std::fs::remove_file(&tmp);
        return Err(error);
    }
    if let Err(error) = replace_temp_file(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(error);
    }
    sync_parent_directory(parent)
}

#[cfg(not(windows))]
fn replace_temp_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::rename(source, destination)
}

#[cfg(windows)]
fn replace_temp_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let mut source_wide = source.as_os_str().encode_wide().collect::<Vec<_>>();
    source_wide.push(0);
    let mut destination_wide = destination.as_os_str().encode_wide().collect::<Vec<_>>();
    destination_wide.push(0);
    // SAFETY: both buffers are NUL-terminated, remain alive for the call, and
    // MoveFileExW does not retain their pointers after returning.
    let replaced = unsafe {
        MoveFileExW(
            source_wide.as_ptr(),
            destination_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if replaced == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Atomically replace a file only when its locked current bytes satisfy a
/// caller-provided revision predicate.
///
/// Cooperating writers serialize through a sibling sidecar lock. The
/// predicate and replacement happen while that lock is held, closing the
/// read/check/write gap used by revisioned editors and stores.
pub fn atomic_compare_and_swap<F>(
    path: &Path,
    bytes: &[u8],
    matches_expected: F,
) -> std::io::Result<bool>
where
    F: FnOnce(&[u8]) -> bool,
{
    use fs2::FileExt;

    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other(format!("path has no parent: {}", path.display())))?;
    std::fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("data");
    let lock_path = parent.join(format!(".{file_name}.lock"));
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)?;
    lock.lock_exclusive()?;
    let result = (|| {
        let current = std::fs::read(path)?;
        if !matches_expected(&current) {
            return Ok(false);
        }
        atomic_write(path, bytes)?;
        Ok(true)
    })();
    let unlock_result = lock.unlock();
    match (result, unlock_result) {
        (Err(error), _) | (Ok(_), Err(error)) => Err(error),
        (Ok(replaced), Ok(())) => Ok(replaced),
    }
}

/// Durability requested after mutating an existing file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileDurability {
    /// Flush language-level buffers without requesting a disk barrier.
    Flush,
    /// Request that mutated file data reach durable storage.
    SyncData,
}

#[derive(Clone, Copy)]
enum ExistingFileAccess {
    Read,
    Append,
    Write,
}

fn open_existing_regular(
    path: &Path,
    access: ExistingFileAccess,
) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    if matches!(access, ExistingFileAccess::Read) {
        options.read(true);
    } else {
        options.write(true);
    }
    if matches!(access, ExistingFileAccess::Append) {
        options.append(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    #[cfg(not(any(unix, windows)))]
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "atomic no-follow existing-file mutation is unavailable on this platform",
        ));
    }

    let file = options.open(path)?;
    let metadata = file.metadata()?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes()
            & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
            != 0
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("refusing to mutate a reparse point: {}", path.display()),
            ));
        }
    }
    if !metadata.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("mutation target is not a regular file: {}", path.display()),
        ));
    }
    Ok(file)
}

fn finish_existing_mutation(
    file: &mut std::fs::File,
    durability: FileDurability,
) -> std::io::Result<()> {
    match durability {
        FileDurability::Flush => file.flush(),
        FileDurability::SyncData => file.sync_data(),
    }
}

/// Append to an existing regular file without following a final symlink.
///
/// Durable JSONL stores use this instead of a check-then-open sequence so a
/// path replacement cannot redirect an append outside the store root.
pub fn append_existing(
    path: &Path,
    bytes: &[u8],
    durability: FileDurability,
) -> std::io::Result<()> {
    let mut file = open_existing_regular(path, ExistingFileAccess::Append)?;
    file.write_all(bytes)?;
    finish_existing_mutation(&mut file, durability)
}

/// Read an existing regular file without following a final symlink.
///
/// The file is opened before its handle metadata is validated, avoiding a
/// check-then-open race at the final path component.
pub fn read_existing(path: &Path) -> std::io::Result<Vec<u8>> {
    let mut file = open_existing_regular(path, ExistingFileAccess::Read)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Read an existing regular file starting at an exact byte offset without
/// following a final symlink.
///
/// Checkpointed journals use this to decode only the event suffix after a
/// validated sequence instead of re-reading the complete log on every replay.
pub fn read_existing_from(path: &Path, offset: u64) -> std::io::Result<Vec<u8>> {
    let mut file = open_existing_regular(path, ExistingFileAccess::Read)?;
    let len = file.metadata()?.len();
    if offset > len {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "read offset {offset} exceeds file length {len}: {}",
                path.display()
            ),
        ));
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Read at most `limit` complete newline-terminated records from `offset`.
///
/// Unlike [`read_existing_from`], this bounds materialized memory by the
/// requested record count and stops once the final requested newline is seen.
pub fn read_existing_lines_from(
    path: &Path,
    offset: u64,
    limit: usize,
) -> std::io::Result<Vec<u8>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let mut file = open_existing_regular(path, ExistingFileAccess::Read)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut reader = BufReader::new(file);
    let mut bytes = Vec::new();
    for _ in 0..limit {
        let read = reader.read_until(b'\n', &mut bytes)?;
        if read == 0 {
            break;
        }
        if bytes.last() != Some(&b'\n') {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!("incomplete newline-terminated record in {}", path.display()),
            ));
        }
    }
    Ok(bytes)
}

/// Truncate an existing regular file without following a final symlink.
///
/// Recovery code uses this after it has validated the last complete record;
/// the function never creates a missing target.
pub fn truncate_existing(path: &Path, len: u64, durability: FileDurability) -> std::io::Result<()> {
    let mut file = open_existing_regular(path, ExistingFileAccess::Write)?;
    file.set_len(len)?;
    finish_existing_mutation(&mut file, durability)
}

#[cfg(unix)]
fn sync_parent_directory(parent: &Path) -> std::io::Result<()> {
    std::fs::File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent_directory(_parent: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_segment_rejects_escape_and_separators() {
        for invalid in ["", ".", "..", "../x", "a/b", "a\\b", "/tmp"] {
            assert!(
                validate_path_segment(invalid).is_err(),
                "accepted {invalid:?}"
            );
        }
        assert!(validate_path_segment("case-01_中文").is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn joined_path_segment_rejects_existing_symlink() -> std::io::Result<()> {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!("echo-core-join-{}", uuid::Uuid::new_v4()));
        let outside =
            std::env::temp_dir().join(format!("echo-core-join-outside-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root)?;
        std::fs::create_dir_all(&outside)?;
        symlink(&outside, root.join("linked"))?;

        assert!(join_path_segment(&root, "linked").is_err());

        std::fs::remove_file(root.join("linked"))?;
        std::fs::remove_dir_all(root)?;
        std::fs::remove_dir_all(outside)?;
        Ok(())
    }

    #[test]
    fn atomic_write_replaces_content() -> std::io::Result<()> {
        let root = std::env::temp_dir().join(format!("echo-core-fs-{}", uuid::Uuid::new_v4()));
        let path = root.join("value.json");
        atomic_write(&path, b"one")?;
        atomic_write(&path, b"two")?;
        assert_eq!(std::fs::read(&path)?, b"two");
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn exclusive_file_lease_rejects_a_second_authority() -> std::io::Result<()> {
        let root = std::env::temp_dir().join(format!("echo-core-lease-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root)?;
        let authority = root.join("store.json");
        let first = try_exclusive_file_lease(&authority)?;
        assert!(try_exclusive_file_lease(&authority).is_err());
        drop(first);
        let reacquired = try_exclusive_file_lease(&authority)?;
        assert!(reacquired.path().exists());
        drop(reacquired);
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn read_existing_from_returns_exact_suffix() -> std::io::Result<()> {
        let root = std::env::temp_dir().join(format!("echo-core-read-{}", uuid::Uuid::new_v4()));
        let path = root.join("events.jsonl");
        std::fs::create_dir_all(&root)?;
        std::fs::write(&path, b"prefix-suffix")?;

        assert_eq!(read_existing_from(&path, 7)?, b"suffix");
        assert!(read_existing_from(&path, 14).is_err());
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn read_existing_lines_from_materializes_only_the_requested_prefix() -> std::io::Result<()> {
        let root = std::env::temp_dir().join(format!("echo-core-lines-{}", uuid::Uuid::new_v4()));
        let path = root.join("events.jsonl");
        std::fs::create_dir_all(&root)?;
        let mut contents = String::new();
        for index in 0..10_000usize {
            use std::fmt::Write as _;
            writeln!(&mut contents, "record-{index:05}")
                .map_err(|error| std::io::Error::other(error.to_string()))?;
        }
        std::fs::write(&path, contents.as_bytes())?;

        let bounded = read_existing_lines_from(&path, 0, 2)?;
        assert_eq!(bounded, b"record-00000\nrecord-00001\n");
        assert!(bounded.len() < contents.len());
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_preserves_existing_permissions() -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!("echo-core-mode-{}", uuid::Uuid::new_v4()));
        let path = root.join("secret.yaml");
        atomic_write(&path, b"one")?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        atomic_write(&path, b"two")?;
        assert_eq!(
            std::fs::metadata(&path)?.permissions().mode() & 0o777,
            0o600
        );
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn atomic_compare_and_swap_rejects_stale_bytes() -> std::io::Result<()> {
        let root = std::env::temp_dir().join(format!("echo-core-cas-{}", uuid::Uuid::new_v4()));
        let path = root.join("document.txt");
        atomic_write(&path, b"current")?;

        assert!(!atomic_compare_and_swap(&path, b"new", |bytes| bytes == b"stale")?);
        assert_eq!(std::fs::read(&path)?, b"current");
        assert!(atomic_compare_and_swap(&path, b"new", |bytes| bytes == b"current")?);
        assert_eq!(std::fs::read(&path)?, b"new");
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn append_existing_supports_flush_and_sync_data() -> std::io::Result<()> {
        let root = std::env::temp_dir().join(format!("echo-core-append-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root)?;
        let path = root.join("run.jsonl");
        std::fs::write(&path, b"start\n")?;

        append_existing(&path, b"flush\n", FileDurability::Flush)?;
        append_existing(&path, b"sync\n", FileDurability::SyncData)?;

        assert_eq!(std::fs::read(&path)?, b"start\nflush\nsync\n");
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn append_existing_rejects_symlink_target_for_both_modes() -> std::io::Result<()> {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!("echo-core-append-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root)?;
        let outside = root.join("outside.jsonl");
        let link = root.join("run.jsonl");
        std::fs::write(&outside, b"outside\n")?;
        symlink(&outside, &link)?;

        for durability in [FileDurability::Flush, FileDurability::SyncData] {
            assert!(append_existing(&link, b"event\n", durability).is_err());
        }
        assert_eq!(std::fs::read(&outside)?, b"outside\n");
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn truncate_existing_supports_flush_and_sync_data() -> std::io::Result<()> {
        let root =
            std::env::temp_dir().join(format!("echo-core-truncate-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root)?;
        let path = root.join("journal.jsonl");
        std::fs::write(&path, b"abcdef")?;

        truncate_existing(&path, 4, FileDurability::Flush)?;
        assert_eq!(std::fs::read(&path)?, b"abcd");
        truncate_existing(&path, 2, FileDurability::SyncData)?;
        assert_eq!(std::fs::read(&path)?, b"ab");

        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn truncate_existing_rejects_symlink_target_for_both_modes() -> std::io::Result<()> {
        use std::os::unix::fs::symlink;

        let root =
            std::env::temp_dir().join(format!("echo-core-truncate-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root)?;
        let outside = root.join("outside.jsonl");
        let link = root.join("journal.jsonl");
        std::fs::write(&outside, b"outside\n")?;
        symlink(&outside, &link)?;

        for durability in [FileDurability::Flush, FileDurability::SyncData] {
            assert!(truncate_existing(&link, 0, durability).is_err());
        }
        assert_eq!(std::fs::read(&outside)?, b"outside\n");
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn read_existing_reads_regular_file() -> std::io::Result<()> {
        let root = std::env::temp_dir().join(format!("echo-core-read-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root)?;
        let path = root.join("events.jsonl");
        std::fs::write(&path, "first\n第二\n")?;

        assert_eq!(read_existing(&path)?, "first\n第二\n".as_bytes());

        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn read_existing_rejects_symlink_target() -> std::io::Result<()> {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!("echo-core-read-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root)?;
        let target = root.join("target.json");
        let link = root.join("link.json");
        std::fs::write(&target, b"outside")?;
        symlink(&target, &link)?;

        assert!(read_existing(&link).is_err());

        std::fs::remove_dir_all(root)?;
        Ok(())
    }
}
