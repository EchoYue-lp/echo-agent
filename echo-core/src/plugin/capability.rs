//! Plugin capability types — what a plugin can provide.
//!
//! Each variant maps to a component type that gets wired into
//! the corresponding subsystem during plugin loading.

use serde::{Deserialize, Serialize};

/// What a plugin provides.
///
/// Capabilities are declared in the plugin manifest and resolved
/// during the loading phase. Each capability type maps to a concrete
/// subsystem:
///
/// | Capability | Target subsystem |
/// |-----------|-----------------|
/// | `Skill` | `SkillRegistry` |
/// | `Hook` | `HookRegistry` |
/// | `McpServer` | `McpManager` |
/// | `LspServer` | `LspManager` |
/// | `Agent` | `SubagentRegistry` |
/// | `Tool` | `ToolManager` (native code plugins, future) |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginCapability {
    /// File-based skills (SKILL.md files) — registered with `SkillRegistry`.
    Skill,
    /// Hook definitions (hooks.yaml) — registered with `HookRegistry`.
    Hook,
    /// MCP server configurations (`mcp.json`) — connected via `McpManager`.
    McpServer,
    /// LSP server configurations — started via `LspManager`.
    LspServer,
    /// Agent definition files (agents/*.md) — registered with `SubagentRegistry`.
    Agent,
    /// Native code tools — registered with `ToolManager` (requires entry_point, future).
    Tool,
}

impl PluginCapability {
    /// Parse a capability string from the manifest.
    pub fn from_str_loose(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "skill" | "skills" => Some(Self::Skill),
            "hook" | "hooks" => Some(Self::Hook),
            "mcp" | "mcp_server" | "mcpserver" | "mcp_servers" => Some(Self::McpServer),
            "lsp" | "lsp_server" | "lspserver" | "lsp_servers" => Some(Self::LspServer),
            "agent" | "agents" => Some(Self::Agent),
            "tool" | "tools" => Some(Self::Tool),
            _ => None,
        }
    }

    /// Human-readable display name.
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Skill => "Skills",
            Self::Hook => "Hooks",
            Self::McpServer => "MCP Servers",
            Self::LspServer => "LSP Servers",
            Self::Agent => "Agents",
            Self::Tool => "Tools",
        }
    }
}

impl std::fmt::Display for PluginCapability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.display_name())
    }
}
