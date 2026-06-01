//! Plugin system — core types for EchoAgent's extension architecture.
//!
//! A **plugin** is a self-contained component directory that extends EchoAgent
//! with custom skills, hooks, MCP servers, LSP servers, agents, monitors, and themes.
//!
//! ## Architecture
//!
//! ```text
//! .echo-plugin/
//! └── manifest.yaml          # Plugin metadata and component paths
//! skills/                    # SKILL.md files
//! agents/                    # Agent definition markdown files
//! hooks/
//! └── hooks.yaml             # Hook configuration
//! .mcp.json                  # MCP server configuration
//! .lsp.yaml                  # LSP server configuration
//! monitors/
//! └── monitors.json          # Background monitor configuration
//! themes/                    # Color theme JSON files
//! ```
//!
//! ## Manifest format
//!
//! Plugins declare their components in `manifest.yaml`:
//!
//! ```yaml
//! name: my-plugin
//! version: "1.0.0"
//! description: "Example plugin"
//! components:
//!   skills: "./skills/"
//!   hooks: "./hooks/hooks.yaml"
//!   mcp_servers: "./.mcp.json"
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
pub use lifecycle::PluginLifecycle;
pub use manifest::{
    PluginAuthor, PluginComponents, PluginDependency, PluginManifest, PluginUserConfigEntry,
    PluginUserConfigType,
};
pub use registry::{PluginEntry, PluginId, PluginRegistry, ResolvedComponents};
pub use scope::{InstallSource, PluginScope};
pub use variables::PluginVariables;
