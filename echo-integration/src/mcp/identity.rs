use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

/// Stable authority that owns an MCP server connection.
///
/// `Direct` preserves the historical public API. Plugin identities are kept as
/// the canonical plugin id supplied by the prepared plugin snapshot; the
/// manager never derives authority by parsing a projected tool or resource
/// name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum McpServerOwner {
    Direct,
    Plugin(String),
}

impl McpServerOwner {
    pub fn plugin(id: impl Into<String>) -> Self {
        Self::Plugin(id.into())
    }

    pub fn is_direct(&self) -> bool {
        matches!(self, Self::Direct)
    }

    pub fn stable_name(&self) -> Option<&str> {
        match self {
            Self::Direct => None,
            Self::Plugin(id) => Some(id),
        }
    }
}

impl fmt::Display for McpServerOwner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Direct => formatter.write_str("direct"),
            Self::Plugin(id) => write!(formatter, "plugin:{id}"),
        }
    }
}

/// Canonical identity for one MCP server within an owner namespace.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct McpServerId {
    pub owner: McpServerOwner,
    pub local_name: String,
}

impl McpServerId {
    pub fn direct(local_name: impl Into<String>) -> Self {
        Self {
            owner: McpServerOwner::Direct,
            local_name: local_name.into(),
        }
    }

    pub fn plugin(owner: impl Into<String>, local_name: impl Into<String>) -> Self {
        Self {
            owner: McpServerOwner::plugin(owner),
            local_name: local_name.into(),
        }
    }

    pub fn selector(&self) -> String {
        match &self.owner {
            McpServerOwner::Direct
                if self.local_name.starts_with("plugin:")
                    || self.local_name.starts_with("direct:") =>
            {
                format!("direct:{}", encode_segment(&self.local_name))
            }
            McpServerOwner::Direct => self.local_name.clone(),
            McpServerOwner::Plugin(plugin) => format!(
                "plugin:{}:{}",
                encode_segment(plugin),
                encode_segment(&self.local_name)
            ),
        }
    }

    pub fn parse_selector(selector: &str) -> Option<Self> {
        let mut parts = selector.split(':');
        match parts.next()? {
            "direct" => {
                let local_name = decode_segment(parts.next()?)?;
                if parts.next().is_some() {
                    return None;
                }
                Some(Self::direct(local_name))
            }
            "plugin" => {
                let plugin = decode_segment(parts.next()?)?;
                let local_name = decode_segment(parts.next()?)?;
                if parts.next().is_some() {
                    return None;
                }
                Some(Self::plugin(plugin, local_name))
            }
            _ => Some(Self::direct(selector.to_string())),
        }
    }
}

impl fmt::Display for McpServerId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.owner, self.local_name)
    }
}

fn encode_segment(value: &str) -> String {
    URL_SAFE_NO_PAD.encode(value.as_bytes())
}

fn decode_segment(value: &str) -> Option<String> {
    let bytes = URL_SAFE_NO_PAD.decode(value).ok()?;
    String::from_utf8(bytes).ok()
}

/// Lossy tool-name projection with a digest suffix when sanitization would
/// collapse distinct canonical names. The digest is derived from the full
/// Unicode identity, so the canonical identity remains available in metadata.
pub fn plugin_tool_projection(plugin: &str, server: &str, tool: &str) -> String {
    let base = format!(
        "mcp__plugin_{}_{}__{}",
        slug(plugin),
        slug(server),
        slug(tool)
    );
    if [plugin, server, tool].iter().all(|value| {
        value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
    }) {
        return base;
    }
    let mut hasher = Sha256::new();
    hasher.update(plugin.as_bytes());
    hasher.update([0]);
    hasher.update(server.as_bytes());
    hasher.update([0]);
    hasher.update(tool.as_bytes());
    let digest = hex_digest(&hasher.finalize());
    format!("{base}__{}", digest.chars().take(12).collect::<String>())
}

fn slug(value: &str) -> String {
    let mut result = String::new();
    for character in value.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
            result.push(character);
        } else {
            result.push('_');
        }
    }
    if result.is_empty() {
        "unnamed".to_string()
    } else {
        result
    }
}

fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_selector_round_trips_unicode_and_punctuation() {
        let id = McpServerId::plugin("供应商/α", "数据:server");
        assert_eq!(McpServerId::parse_selector(&id.selector()), Some(id));
    }

    #[test]
    fn direct_selector_keeps_legacy_wire_name() {
        let id = McpServerId::direct("filesystem");
        assert_eq!(id.selector(), "filesystem");
        assert_eq!(McpServerId::parse_selector("filesystem"), Some(id));
    }

    #[test]
    fn direct_selector_cannot_be_misparsed_as_plugin() {
        let plugin = McpServerId::plugin("owner", "server");
        let direct = McpServerId::direct(plugin.selector());
        assert_ne!(direct.selector(), plugin.selector());
        assert_eq!(
            McpServerId::parse_selector(&direct.selector()),
            Some(direct)
        );
        assert_eq!(
            McpServerId::parse_selector(&plugin.selector()),
            Some(plugin)
        );
    }

    #[test]
    fn lossy_plugin_projection_has_distinguishing_digest() {
        let left = plugin_tool_projection("a/b", "same", "tool");
        let right = plugin_tool_projection("a_b", "same", "tool");
        assert_ne!(left, right);
    }

    #[test]
    fn component_boundaries_cannot_collapse_tool_projection() {
        let left = plugin_tool_projection("a", "b_c", "tool");
        let right = plugin_tool_projection("a_b", "c", "tool");
        assert_ne!(left, right);
    }
}
