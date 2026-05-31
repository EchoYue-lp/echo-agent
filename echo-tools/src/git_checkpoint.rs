//! Git checkpoint utility — creates lightweight tags before file mutations.

use std::path::Path;
use std::process::Command;

/// Create a git checkpoint tag for the current HEAD.
/// Returns the tag name if successful, None if not in a git repo.
pub fn create_checkpoint(file_path: &Path) -> Option<String> {
    // Find git root
    let git_root = find_git_root(file_path)?;

    // Get current HEAD
    let head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&git_root)
        .output()
        .ok()?;

    if !head.status.success() {
        return None;
    }

    let head_hash = String::from_utf8_lossy(&head.stdout).trim().to_string();
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let tag_name = format!("echo-checkpoint/{}", timestamp);

    // Create lightweight tag
    let tag_result = Command::new("git")
        .args(["tag", &tag_name, &head_hash])
        .current_dir(&git_root)
        .output()
        .ok()?;

    if tag_result.status.success() {
        Some(tag_name)
    } else {
        None
    }
}

/// Rollback to a checkpoint tag.
pub fn rollback_to_checkpoint(file_path: &Path, tag_name: &str) -> bool {
    let git_root = match find_git_root(file_path) {
        Some(root) => root,
        None => return false,
    };

    Command::new("git")
        .args(["checkout", tag_name, "--", "."])
        .current_dir(&git_root)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Clean up old checkpoint tags (keep last N).
pub fn cleanup_old_checkpoints(file_path: &Path, keep: usize) {
    let git_root = match find_git_root(file_path) {
        Some(root) => root,
        None => return,
    };

    let output = match Command::new("git")
        .args(["tag", "-l", "echo-checkpoint/*", "--sort=-creatordate"])
        .current_dir(&git_root)
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return,
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let tags: Vec<&str> = stdout.lines().collect();

    for tag in tags.iter().skip(keep) {
        let _ = Command::new("git")
            .args(["tag", "-d", tag.trim()])
            .current_dir(&git_root)
            .output();
    }
}

fn find_git_root(file_path: &Path) -> Option<std::path::PathBuf> {
    let dir = if file_path.is_file() {
        file_path.parent()?
    } else {
        file_path
    };

    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(dir)
        .output()
        .ok()?;

    if output.status.success() {
        let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Some(std::path::PathBuf::from(root))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_git_root_in_repo() {
        // This test runs in the echo-agent repo
        let path = Path::new(env!("CARGO_MANIFEST_DIR"));
        let root = find_git_root(path);
        assert!(root.is_some());
    }

    #[test]
    fn test_find_git_root_outside_repo() {
        let path = Path::new("/tmp");
        let root = find_git_root(path);
        // May or may not be in a git repo depending on system
        // Just verify it doesn't panic
        let _ = root;
    }
}
