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
    validate_path_segment(value).map(|segment| root.join(segment))
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
    let write_result = (|| {
        let mut file = std::fs::File::create(&tmp)?;
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
}
