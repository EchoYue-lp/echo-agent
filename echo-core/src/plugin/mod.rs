//! Plugin system — core types for EchoAgent plugin packages.
//!
//! A **plugin** is a flat package rooted by `plugin.json`. Every component uses
//! a conventional location, so authors do not repeat paths in the manifest.
//!
//! ## Architecture
//!
//! ```text
//! plugin.json
//! skills/<skill>/SKILL.md
//! mcp.json
//! agents/
//! hooks/hooks.yaml
//! lsp.yaml
//! ```
//!
//! ## Manifest format
//!
//! Plugins declare metadata and optional local configuration in `plugin.json`:
//!
//! ```json
//! {
//!   "$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
//!   "name": "my-plugin",
//!   "displayName": "My Plugin"
//! }
//! ```
//!
//! ## Installation scopes
//!
//! | Scope | Path | Use case |
//! |-------|------|----------|
//! | User | `~/.echo-agent/plugins/` | Personal plugins, available in all projects |
//! | Project | `.echo-agent/plugins/` (project root) | Team plugins shared via VCS |
//! | Local | `.echo-agent/plugins.local/` | Project-specific, gitignored |

pub mod capability;
pub mod lifecycle;
pub mod manifest;
pub mod registry;
pub mod scope;
pub mod variables;

// Re-export primary types at module level.
pub use capability::PluginCapability;
pub use lifecycle::{PluginLifecycle, PluginLifecycleManager};
pub use manifest::{
    AGENT_PLUGIN_SCHEMA_V1, PluginAuthor, PluginDependency, PluginManifest, PluginUserConfigEntry,
    PluginUserConfigType,
};
pub use registry::{PluginEntry, PluginId, PluginRegistry, ResolvedComponents};
pub use scope::{InstallSource, PluginScope};
pub use variables::PluginVariables;

// ── Plugin base directory resolution (configurable, single source) ───────
//
// The plugin system previously hard-coded `~/.echo-agent/plugins` in three
// places (scope.rs, registry.rs, variables.rs). That bypassed the facade's
// `paths::set_user_data_dir_name` override, so EKO (which sets `.eko`) ended
// up with non-plugin data in `~/.eko/` but plugin data in `~/.echo-agent/` —
// a split data layout (audit P0-3).
//
// `echo-core` cannot depend on the facade crate (`paths` lives in `echo_agent`,
// depending back here would cycle), so this module owns its own configurable
// base dir with the same OnceLock pattern. Applications call
// [`set_plugin_data_base_dir`] at startup to align plugins with their brand
// directory; the framework default stays `~/.echo-agent` for reuse by other
// consumers (AGENTS.md: framework stays neutral, app decides brand dir).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Default plugin base directory name (under the user's home).
const DEFAULT_PLUGIN_BASE_DIR_NAME: &str = ".echo-agent";

static PLUGIN_DATA_BASE_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Resolve the user's home directory.
///
/// Reads `$HOME`, falling back to `~` literal (never panics; AGENTS.md hard
/// constraint). Kept here so scope/registry/variables share one resolver
/// instead of each re-reading `HOME`.
pub(crate) fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("~"))
}

/// Resolve the plugin base directory.
///
/// Unconfigured default: `~/.echo-agent`. Applications override via
/// [`set_plugin_data_base_dir`] (or the convenience
/// [`set_plugin_data_base_dir_name`]) at the earliest startup point, before
/// any `PluginRegistry`/`PluginScope`/`PluginVariables` path resolution.
pub fn plugin_data_base_dir() -> PathBuf {
    PLUGIN_DATA_BASE_DIR
        .get_or_init(|| home_dir().join(DEFAULT_PLUGIN_BASE_DIR_NAME))
        .clone()
}

/// Override the plugin base directory with an explicit absolute path.
///
/// Must be called at startup before any plugin path is resolved. Setting the
/// same value is idempotent; a different value after initialization returns
/// the currently-effective path as `Err` so callers can detect "set too late".
pub fn set_plugin_data_base_dir(dir: impl Into<PathBuf>) -> Result<(), PathBuf> {
    let dir = dir.into();
    match PLUGIN_DATA_BASE_DIR.set(dir.clone()) {
        Ok(()) => Ok(()),
        Err(_) => {
            let current = plugin_data_base_dir();
            if current == dir { Ok(()) } else { Err(current) }
        }
    }
}

/// Convenience: set the plugin base directory to `~/<name>` (e.g. `.eko`).
///
/// Recommended entry point for applications switching to their brand directory.
/// Applications using the `echo_agent` facade can separately align its branded
/// data directory during startup. This split-crate example has no facade
/// dependency:
///
/// ```no_run
/// echo_core::plugin::set_plugin_data_base_dir_name(".eko").ok();
/// ```
pub fn set_plugin_data_base_dir_name(name: impl AsRef<str>) -> Result<(), PathBuf> {
    set_plugin_data_base_dir(home_dir().join(name.as_ref()))
}

/// Resolve `plugins` under the (possibly overridden) base directory.
pub(crate) fn plugins_dir() -> PathBuf {
    plugin_data_base_dir().join("plugins")
}

/// Resolve `plugins/<child>` under the base directory.
pub(crate) fn plugins_child(child: impl AsRef<Path>) -> PathBuf {
    plugins_dir().join(child)
}
