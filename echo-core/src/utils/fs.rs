//! Filesystem safety primitives shared by framework crates.

use std::io::Write;
use std::path::{Component, Path, PathBuf};

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
    if let Err(error) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(error);
    }
    sync_parent_directory(parent)
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

/// Append to an existing regular file without following a final symlink.
///
/// Durable JSONL stores use this instead of a check-then-open sequence so a
/// path replacement cannot redirect an append outside the store root.
pub fn append_existing(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(not(unix))]
    if std::fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("refusing to append through a symlink: {}", path.display()),
        ));
    }

    let mut file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("append target is not a regular file: {}", path.display()),
        ));
    }
    file.write_all(bytes)?;
    file.sync_data()
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

    #[cfg(unix)]
    #[test]
    fn append_existing_rejects_symlink_target() -> std::io::Result<()> {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!("echo-core-append-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root)?;
        let outside = root.join("outside.jsonl");
        let link = root.join("run.jsonl");
        std::fs::write(&outside, b"outside\n")?;
        symlink(&outside, &link)?;

        assert!(append_existing(&link, b"event\n").is_err());
        assert_eq!(std::fs::read(&outside)?, b"outside\n");
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
}
