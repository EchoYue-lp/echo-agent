//! Plugin installation scopes — where plugins are stored and who can use them.
//!
//! | Scope | Path | Use case |
//! |-------|------|----------|
//! | `User` | `<plugin_data_root>/plugins/` | Personal plugins |
//! | `Project` | `.echo-agent/plugins/` (project root) | Team-shared via VCS |
//! | `Local` | `.echo-agent/plugins.local/` (project root) | Project-private, gitignored |

use std::path::{Path, PathBuf};

/// Where a plugin is installed and who can access it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginScope {
    /// Available in all projects for this user.
    /// Stored in `~/.echo-agent/plugins/`.
    User,
    /// Shared with the team via version control.
    /// Stored in `<project-root>/.echo-agent/plugins/`.
    Project,
    /// Project-specific, not committed to VCS.
    /// Stored in `<project-root>/.echo-agent/plugins.local/`.
    Local,
}

impl PluginScope {
    /// Resolve the filesystem path for this scope.
    ///
    /// - `User`: the application-supplied user plugin directory
    /// - `Project`: `<project_root>/.echo-agent/plugins/`
    /// - `Local`: `<project_root>/.echo-agent/plugins.local/`
    ///
    /// Project/Local scopes intentionally stay `.echo-agent` regardless of the
    /// user-scope override: they are workspace-relative convention directories,
    /// not user-data directories, and team-shared plugins committed to VCS must
    /// land at a stable path independent of any one developer's brand setting.
    pub fn resolve_dir(&self, user_plugins_dir: &Path, project_root: Option<&Path>) -> PathBuf {
        match self {
            Self::User => user_plugins_dir.to_path_buf(),
            Self::Project => {
                let root = project_root.unwrap_or_else(|| Path::new("."));
                root.join(".echo-agent").join("plugins")
            }
            Self::Local => {
                let root = project_root.unwrap_or_else(|| Path::new("."));
                root.join(".echo-agent").join("plugins.local")
            }
        }
    }

    /// All scopes in priority order (user → project → local).
    pub fn all() -> &'static [PluginScope] {
        &[PluginScope::User, PluginScope::Project, PluginScope::Local]
    }
}

impl std::str::FromStr for PluginScope {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_lowercase().as_str() {
            "user" | "u" => Ok(Self::User),
            "project" | "p" => Ok(Self::Project),
            "local" | "l" => Ok(Self::Local),
            _ => Err(format!("unknown plugin scope: {value}")),
        }
    }
}

impl std::fmt::Display for PluginScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::User => write!(f, "user"),
            Self::Project => write!(f, "project"),
            Self::Local => write!(f, "local"),
        }
    }
}

/// Source for installing a plugin — either a local directory or a git URL.
#[derive(Debug, Clone)]
pub enum InstallSource {
    /// Install from a local directory path.
    Local(PathBuf),
    /// Install from a git repository URL, optionally a subdirectory.
    Git { url: String, subdir: Option<String> },
}

impl InstallSource {
    /// Parse an install source string — detects git URLs vs local paths.
    pub fn parse(s: &str) -> Self {
        if s.starts_with("http://")
            || s.starts_with("https://")
            || s.starts_with("ssh://")
            || s.starts_with("git://")
            || s.starts_with("git@")
            || s.ends_with(".git")
        {
            Self::Git {
                url: s.to_string(),
                subdir: None,
            }
        } else {
            Self::Local(PathBuf::from(s))
        }
    }

    /// Whether this is a git source.
    pub fn is_git(&self) -> bool {
        matches!(self, Self::Git { .. })
    }
}

impl std::fmt::Display for InstallSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Local(p) => write!(f, "{}", p.display()),
            Self::Git { url, subdir } => {
                write!(f, "{url}")?;
                if let Some(sub) = subdir {
                    write!(f, ":{sub}")?;
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scope_resolve_user() {
        let dir = PluginScope::User.resolve_dir(Path::new("/data/plugins"), None);
        assert_eq!(dir, PathBuf::from("/data/plugins"));
    }

    #[test]
    fn test_scope_resolve_project() {
        let dir = PluginScope::Project.resolve_dir(
            Path::new("/data/plugins"),
            Some(Path::new("/home/user/my-project")),
        );
        assert_eq!(
            dir,
            PathBuf::from("/home/user/my-project/.echo-agent/plugins")
        );
    }

    #[test]
    fn test_scope_resolve_local() {
        let dir = PluginScope::Local.resolve_dir(
            Path::new("/data/plugins"),
            Some(Path::new("/home/user/my-project")),
        );
        assert_eq!(
            dir,
            PathBuf::from("/home/user/my-project/.echo-agent/plugins.local")
        );
    }

    #[test]
    fn test_install_source_parse_local() {
        let src = InstallSource::parse("/home/user/my-plugin");
        assert!(!src.is_git());
    }

    #[test]
    fn test_install_source_parse_git() {
        let src = InstallSource::parse("https://github.com/echo/plugin.git");
        assert!(src.is_git());
    }

    #[test]
    fn test_scope_from_str() {
        assert_eq!("user".parse(), Ok(PluginScope::User));
        assert_eq!("project".parse(), Ok(PluginScope::Project));
        assert_eq!("local".parse(), Ok(PluginScope::Local));
        assert!("invalid".parse::<PluginScope>().is_err());
    }
}
