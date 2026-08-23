//! Explicit framework data-root value.

use std::path::{Path, PathBuf};

/// An application-selected root for framework persistence.
///
/// The framework never chooses a home-directory name or product brand. Pass a
/// cloned value into the stores and services that need persistence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataRoot {
    root: PathBuf,
}

impl DataRoot {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn as_path(&self) -> &Path {
        &self.root
    }

    pub fn path(&self, child: impl AsRef<Path>) -> PathBuf {
        self.root.join(child)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_under_explicit_root() {
        let root = DataRoot::new("/tmp/echo-agent-consumer");
        assert_eq!(
            root.path("store.json"),
            PathBuf::from("/tmp/echo-agent-consumer/store.json")
        );
    }
}
