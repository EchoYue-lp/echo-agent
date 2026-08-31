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

impl std::str::FromStr for PluginCapability {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_lowercase().as_str() {
            "skill" | "skills" => Ok(Self::Skill),
            "hook" | "hooks" => Ok(Self::Hook),
            "mcp" | "mcp_server" | "mcpserver" | "mcp_servers" => Ok(Self::McpServer),
            "lsp" | "lsp_server" | "lspserver" | "lsp_servers" => Ok(Self::LspServer),
            "agent" | "agents" => Ok(Self::Agent),
            "tool" | "tools" => Ok(Self::Tool),
            _ => Err(format!("unknown plugin capability: {value}")),
        }
    }
}

impl std::fmt::Display for PluginCapability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.display_name())
    }
}
