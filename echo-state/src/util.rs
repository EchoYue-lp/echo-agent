//! Shared utility functions for the echo-state crate.

#[cfg(feature = "sqlite")]
use echo_core::error::MemoryError;
use std::path::{Path, PathBuf};

/// Expand a leading `~` or `~/` in a path to the user's home directory.
///
/// Checks `HOME` first, then falls back to `USERPROFILE` (Windows).
pub fn expand_tilde(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if s.starts_with("~/")
        && let Some(home) = std::env::var("HOME")
            .ok()
            .or_else(|| std::env::var("USERPROFILE").ok())
    {
        return PathBuf::from(home).join(&s[2..]);
    }
    path.to_path_buf()
}

/// Create a consistent `MemoryError::IoError` with context text.
#[cfg(feature = "sqlite")]
pub fn memory_io_error(context: &str, err: impl std::fmt::Display) -> MemoryError {
    MemoryError::IoError(format!("{context}: {err}"))
}
