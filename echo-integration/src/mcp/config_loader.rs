//! mcp.json 配置文件加载器
//!
//! 支持与 Claude Desktop / Cursor / VS Code 等主流 Agent 工具兼容的
//! `mcp.json` 配置格式，可直接复用现有的 MCP 服务端配置。
//!
//! ## 文件格式
//!
//! ```json
//! {
//!   "mcpServers": {
//!     "filesystem": {
//!       "command": "npx",
//!       "args": ["-y", "@modelcontextprotocol/server-filesystem", "/workspace"],
//!       "env": {
//!         "OPTIONAL_VAR": "value"
//!       }
//!     },
//!     "github": {
//!       "command": "npx",
//!       "args": ["-y", "@modelcontextprotocol/server-github"],
//!       "env": {
//!         "GITHUB_PERSONAL_ACCESS_TOKEN": "ghp_xxx"
//!       }
//!     },
//!     "remote-api": {
//!       "url": "http://localhost:8080/mcp",
//!       "headers": {
//!         "Authorization": "Bearer token"
//!       }
//!     },
//!     "legacy-sse": {
//!       "url": "http://localhost:3000",
//!       "transport": "sse"
//!     },
//!     "disabled-server": {
//!       "command": "npx",
//!       "args": ["-y", "@modelcontextprotocol/server-postgres", "postgres://localhost/db"],
//!       "disabled": true
//!     }
//!   }
//! }
//! ```

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::server_config::{McpServerConfig, TransportConfig};
use echo_core::error::{McpError, ReactError, Result};

/// Canonical Agent Plugins 1.0 MCP schema identifier.
pub const AGENT_PLUGIN_MCP_SCHEMA_V1: &str =
    "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json";

// ── 配置文件结构 ──────────────────────────────────────────────────────────────

/// mcp.json 文件的顶层结构
///
/// 与 Claude Desktop / Cursor 的 `mcp.json` 格式完全兼容。
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct McpConfigFile {
    /// Optional schema identifier. General EchoAgent MCP files do not require
    /// it; Agent Plugin packages use [`McpConfigFile::parse_agent_plugin`].
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,

    /// 服务端配置映射（key 为服务端名称，在同一 Agent 中唯一）
    #[serde(rename = "mcpServers", default)]
    pub mcp_servers: HashMap<String, McpServerEntry>,
}

/// 单个 MCP 服务端的配置项
///
/// 支持两种模式：
/// - **stdio**：提供 `command`，可选 `args` 和 `env`
/// - **HTTP**：提供 `url`，可选 `headers` 和 `transport`
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq, Eq)]
pub struct McpServerEntry {
    /// Explicit transport discriminator used by Agent Plugins 1.0.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub server_type: Option<String>,

    // ── stdio 模式 ────────────────────────────────────────────────────────────
    /// 启动服务端的命令（如 `"npx"`、`"uvx"`、`"python"`）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,

    /// 命令参数列表
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,

    /// 额外注入子进程的环境变量（不影响当前进程环境）
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,

    /// Optional working directory for stdio processes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,

    // ── HTTP 模式 ─────────────────────────────────────────────────────────────
    /// HTTP 服务端 URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,

    /// HTTP 请求头（如 `Authorization: Bearer <token>`）
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub headers: HashMap<String, String>,

    /// HTTP 传输类型（仅在提供 `url` 时有效）
    ///
    /// - `"sse"`：旧版 HTTP+SSE（适用于旧版 MCP SDK），
    ///   在 `{url}/sse` 建立 SSE 连接，向动态获取的端点 POST 请求
    /// - 默认（不指定或其他值）：MCP Streamable HTTP，直接 POST 到端点 URL
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<String>,

    // ── 通用选项 ──────────────────────────────────────────────────────────────
    /// 设为 `true` 时跳过该服务端（默认为 `false`）
    #[serde(default)]
    pub disabled: bool,
}

impl McpServerEntry {
    /// 将此配置项转换为 [`McpServerConfig`]
    pub fn to_server_config(&self, name: &str) -> Result<McpServerConfig> {
        // 检查是否被禁用
        if self.disabled {
            return Err(ReactError::Mcp(Box::new(McpError::ConnectionFailed(
                format!("服务端 '{}' 已禁用（disabled: true）", name),
            ))));
        }

        if let Some(command) = &self.command {
            validate_stdio_command(command)?;

            let env: Vec<(String, String)> = self
                .env
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            Ok(McpServerConfig {
                name: name.to_string(),
                transport: TransportConfig::Stdio {
                    command: command.clone(),
                    args: self.args.clone(),
                    env,
                    cwd: self.cwd.as_ref().map(PathBuf::from),
                },
            })
        } else if let Some(url) = &self.url {
            let transport = match self.server_type.as_deref().or(self.transport.as_deref()) {
                Some("sse") => TransportConfig::Sse {
                    base_url: url.clone(),
                    headers: self.headers.clone(),
                },
                _ => TransportConfig::Http {
                    base_url: url.clone(),
                    headers: self.headers.clone(),
                },
            };
            Ok(McpServerConfig {
                name: name.to_string(),
                transport,
            })
        } else {
            Err(ReactError::Mcp(Box::new(McpError::ConnectionFailed(
                format!(
                    "服务端 '{}' 配置无效：stdio 模式需提供 'command'，HTTP 模式需提供 'url'",
                    name
                ),
            ))))
        }
    }
}

impl McpConfigFile {
    /// 从 JSON 字符串解析配置
    pub fn parse(s: &str) -> Result<Self> {
        serde_json::from_str(s).map_err(|e| {
            ReactError::Mcp(Box::new(McpError::ProtocolError(format!(
                "mcp.json 格式解析失败: {}",
                e
            ))))
        })
    }

    /// Parse the closed Agent Plugins 1.0 `mcp.json` format.
    ///
    /// Top-level errors disable MCP for the package. Invalid server entries
    /// are omitted and reported in `diagnostics`, preserving the standard's
    /// per-entry failure isolation.
    pub fn parse_agent_plugin(
        s: &str,
        plugin_root: &Path,
        plugin_data: &Path,
    ) -> Result<AgentPluginMcpLoad> {
        let raw: AgentPluginMcpDocument = serde_json::from_str(s).map_err(|error| {
            protocol_error(format!("Agent Plugin mcp.json is invalid: {error}"))
        })?;
        if raw.schema != AGENT_PLUGIN_MCP_SCHEMA_V1 {
            return Err(protocol_error(format!(
                "Unsupported Agent Plugins MCP schema '{}'; expected '{AGENT_PLUGIN_MCP_SCHEMA_V1}'",
                raw.schema
            )));
        }

        let plugin_root = canonical_directory(plugin_root, "PLUGIN_ROOT")?;
        let plugin_data = canonical_directory(plugin_data, "PLUGIN_DATA")?;
        let mut diagnostics = Vec::new();
        let mut mcp_servers = HashMap::new();
        let mut entries = raw.mcp_servers.into_iter().collect::<Vec<_>>();
        entries.sort_by(|left, right| left.0.cmp(&right.0));

        for (name, value) in entries {
            match parse_agent_plugin_server(&name, value, &plugin_root, &plugin_data) {
                Ok(entry) => {
                    mcp_servers.insert(name, entry);
                }
                Err(error) => diagnostics.push(format!("MCP server '{name}' skipped: {error}")),
            }
        }

        Ok(AgentPluginMcpLoad {
            config: Self {
                schema: Some(AGENT_PLUGIN_MCP_SCHEMA_V1.to_string()),
                mcp_servers,
            },
            diagnostics,
        })
    }

    /// 从文件路径加载配置
    ///
    /// # 示例
    /// ```rust,no_run
    /// use echo_integration::mcp::config_loader::McpConfigFile;
    ///
    /// let config = McpConfigFile::from_file("mcp.json")?;
    /// println!("共 {} 个服务端", config.mcp_servers.len());
    /// # Ok::<(), echo_core::error::ReactError>(())
    /// ```
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let content = std::fs::read_to_string(path).map_err(|e| {
            ReactError::Mcp(Box::new(McpError::ConnectionFailed(format!(
                "读取配置文件失败 ({}): {}",
                path.display(),
                e
            ))))
        })?;
        Self::parse(&content)
    }

    /// 将所有**启用**的服务端转换为 [`McpServerConfig`] 列表
    pub fn to_server_configs(&self) -> Result<Vec<McpServerConfig>> {
        let mut configs = Vec::new();
        for (name, entry) in &self.mcp_servers {
            if entry.disabled {
                tracing::debug!("MCP: 跳过已禁用的服务端 '{}'", name);
                continue;
            }
            configs.push(entry.to_server_config(name)?);
        }
        Ok(configs)
    }

    /// 返回启用的服务端数量
    pub fn enabled_count(&self) -> usize {
        self.mcp_servers.values().filter(|e| !e.disabled).count()
    }
}

/// Result of parsing an Agent Plugin's fixed root `mcp.json` component.
#[derive(Debug, Clone)]
pub struct AgentPluginMcpLoad {
    pub config: McpConfigFile,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentPluginMcpDocument {
    #[serde(rename = "$schema")]
    schema: String,
    #[serde(rename = "mcpServers")]
    mcp_servers: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentPluginStdioServer {
    #[serde(rename = "type")]
    server_type: AgentPluginStdioType,
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: HashMap<String, String>,
    #[serde(default)]
    cwd: Option<String>,
}

#[derive(Debug, Deserialize)]
enum AgentPluginStdioType {
    #[serde(rename = "stdio")]
    Stdio,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentPluginRemoteServer {
    #[serde(rename = "type")]
    server_type: AgentPluginRemoteType,
    url: String,
    #[serde(default)]
    headers: HashMap<String, String>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
enum AgentPluginRemoteType {
    #[serde(rename = "streamable-http")]
    StreamableHttp,
    #[serde(rename = "sse")]
    Sse,
}

fn parse_agent_plugin_server(
    name: &str,
    value: serde_json::Value,
    plugin_root: &Path,
    plugin_data: &Path,
) -> std::result::Result<McpServerEntry, String> {
    let server_type = value
        .as_object()
        .and_then(|object| object.get("type"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "missing string field 'type'".to_string())?;
    match server_type {
        "stdio" => {
            let raw: AgentPluginStdioServer = serde_json::from_value(value)
                .map_err(|error| format!("invalid stdio configuration: {error}"))?;
            let _ = raw.server_type;
            validate_agent_plugin_command(&raw.command)?;
            if raw.env.contains_key("PLUGIN_ROOT") || raw.env.contains_key("PLUGIN_DATA") {
                return Err("env must not override PLUGIN_ROOT or PLUGIN_DATA".to_string());
            }
            let args = raw
                .args
                .into_iter()
                .map(|argument| expand_agent_plugin_variables(&argument, plugin_root, plugin_data))
                .collect();
            let mut env = raw
                .env
                .into_iter()
                .map(|(key, value)| {
                    (
                        key,
                        expand_agent_plugin_variables(&value, plugin_root, plugin_data),
                    )
                })
                .collect::<HashMap<_, _>>();
            env.insert(
                "PLUGIN_ROOT".to_string(),
                plugin_root.to_string_lossy().into_owned(),
            );
            env.insert(
                "PLUGIN_DATA".to_string(),
                plugin_data.to_string_lossy().into_owned(),
            );
            let cwd = resolve_agent_plugin_cwd(raw.cwd.as_deref(), plugin_root, plugin_data)?;
            Ok(McpServerEntry {
                server_type: Some("stdio".to_string()),
                command: Some(raw.command),
                args,
                env,
                cwd: Some(cwd.to_string_lossy().into_owned()),
                url: None,
                headers: HashMap::new(),
                transport: None,
                disabled: false,
            })
        }
        "streamable-http" | "sse" => {
            let raw: AgentPluginRemoteServer = serde_json::from_value(value)
                .map_err(|error| format!("invalid remote configuration: {error}"))?;
            validate_agent_plugin_url(&raw.url)?;
            let transport = match raw.server_type {
                AgentPluginRemoteType::StreamableHttp => "streamable-http",
                AgentPluginRemoteType::Sse => "sse",
            };
            Ok(McpServerEntry {
                server_type: Some(transport.to_string()),
                command: None,
                args: Vec::new(),
                env: HashMap::new(),
                cwd: None,
                url: Some(raw.url),
                headers: raw.headers,
                transport: None,
                disabled: false,
            })
        }
        other => Err(format!("unsupported transport type '{other}'")),
    }
    .map_err(|error| format!("{name}: {error}"))
}

fn canonical_directory(path: &Path, label: &str) -> Result<PathBuf> {
    let canonical = std::fs::canonicalize(path).map_err(|error| {
        protocol_error(format!(
            "{label} directory '{}' is unavailable: {error}",
            path.display()
        ))
    })?;
    if !canonical.is_dir() {
        return Err(protocol_error(format!(
            "{label} path '{}' is not a directory",
            canonical.display()
        )));
    }
    Ok(canonical)
}

fn expand_agent_plugin_variables(input: &str, plugin_root: &Path, plugin_data: &Path) -> String {
    let source = input.chars().collect::<Vec<_>>();
    let root_marker = "${PLUGIN_ROOT}".chars().collect::<Vec<_>>();
    let data_marker = "${PLUGIN_DATA}".chars().collect::<Vec<_>>();
    let mut output = String::with_capacity(input.len());
    let mut position = 0usize;
    while position < source.len() {
        if chars_match_at(&source, position, &root_marker) {
            output.push_str(&plugin_root.to_string_lossy());
            position = position.saturating_add(root_marker.len());
        } else if chars_match_at(&source, position, &data_marker) {
            output.push_str(&plugin_data.to_string_lossy());
            position = position.saturating_add(data_marker.len());
        } else if let Some(character) = source.get(position) {
            output.push(*character);
            position = position.saturating_add(1);
        } else {
            break;
        }
    }
    output
}

fn chars_match_at(source: &[char], position: usize, pattern: &[char]) -> bool {
    pattern.iter().enumerate().all(|(offset, expected)| {
        position
            .checked_add(offset)
            .and_then(|index| source.get(index))
            == Some(expected)
    })
}

fn resolve_agent_plugin_cwd(
    cwd: Option<&str>,
    plugin_root: &Path,
    plugin_data: &Path,
) -> std::result::Result<PathBuf, String> {
    let (base, suffix) = match cwd {
        None => return Ok(plugin_root.to_path_buf()),
        Some("${PLUGIN_ROOT}") => (plugin_root, ""),
        Some("${PLUGIN_DATA}") => (plugin_data, ""),
        Some(value) if value.starts_with("${PLUGIN_ROOT}/") => {
            (plugin_root, value.trim_start_matches("${PLUGIN_ROOT}/"))
        }
        Some(value) if value.starts_with("${PLUGIN_DATA}/") => {
            (plugin_data, value.trim_start_matches("${PLUGIN_DATA}/"))
        }
        Some(value) if value.starts_with("./") => (plugin_root, value.trim_start_matches("./")),
        Some(value) => {
            return Err(format!(
                "cwd '{value}' must begin with './', '${{PLUGIN_ROOT}}', or '${{PLUGIN_DATA}}'"
            ));
        }
    };
    if Path::new(suffix).components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err("cwd must remain inside its selected plugin directory".to_string());
    }
    let candidate = base.join(suffix);
    let canonical = std::fs::canonicalize(&candidate)
        .map_err(|error| format!("cwd '{}' is unavailable: {error}", candidate.display()))?;
    if !canonical.starts_with(base) || !canonical.is_dir() {
        return Err(format!(
            "cwd '{}' must be an existing directory inside the plugin package or data directory",
            candidate.display()
        ));
    }
    Ok(canonical)
}

fn validate_agent_plugin_command(command: &str) -> std::result::Result<(), String> {
    if command.is_empty() {
        return Err("command must not be empty".to_string());
    }
    if command.contains("${PLUGIN_ROOT}") || command.contains("${PLUGIN_DATA}") {
        return Err("command does not support plugin variable expansion".to_string());
    }
    if command.starts_with("./") {
        if Path::new(command).components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            return Err("plugin-relative command must remain inside the plugin root".to_string());
        }
        return Ok(());
    }
    if Path::new(command).is_absolute() || command.contains('/') || command.contains('\\') {
        return Err("command must be a bare executable name or begin with './'".to_string());
    }
    Ok(())
}

fn validate_agent_plugin_url(value: &str) -> std::result::Result<(), String> {
    let url = url::Url::parse(value).map_err(|error| format!("invalid URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("URL scheme must be http or https".to_string());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("URL must not contain user information".to_string());
    }
    if url.fragment().is_some() {
        return Err("URL must not contain a fragment".to_string());
    }
    let loopback = match url.host() {
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        None => false,
    };
    if url.scheme() != "https" && !loopback {
        return Err("non-loopback MCP URLs must use https".to_string());
    }
    Ok(())
}

fn protocol_error(message: String) -> ReactError {
    ReactError::Mcp(Box::new(McpError::ProtocolError(message)))
}

/// Validate the general MCP command shape.
///
/// Commands are spawned directly, without a shell. The framework therefore
/// validates only that an executable token exists and leaves trust policy to
/// the embedding local application.
pub fn validate_stdio_command(command: &str) -> Result<()> {
    if command.trim().is_empty() {
        return Err(protocol_error("MCP stdio command is empty".to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin_directories() -> std::result::Result<(tempfile::TempDir, PathBuf, PathBuf), String> {
        let temporary = tempfile::tempdir().map_err(|error| error.to_string())?;
        let root = temporary.path().join("plugin");
        let data = temporary.path().join("data");
        std::fs::create_dir_all(root.join("workspace")).map_err(|error| error.to_string())?;
        std::fs::create_dir_all(&data).map_err(|error| error.to_string())?;
        Ok((temporary, root, data))
    }

    #[test]
    fn parses_agent_plugin_mcp_and_expands_only_stdio_values() -> std::result::Result<(), String> {
        let (_temporary, root, data) = plugin_directories()?;
        let document = serde_json::json!({
            "$schema": AGENT_PLUGIN_MCP_SCHEMA_V1,
            "mcpServers": {
                "local": {
                    "type": "stdio",
                    "command": "node",
                    "args": ["${PLUGIN_ROOT}/server.js", "你好"],
                    "env": {"CACHE": "${PLUGIN_DATA}/cache"},
                    "cwd": "./workspace"
                },
                "remote": {
                    "type": "streamable-http",
                    "url": "http://localhost:8080/mcp",
                    "headers": {"X-Literal": "${PLUGIN_ROOT}"}
                }
            }
        });
        let loaded = McpConfigFile::parse_agent_plugin(&document.to_string(), &root, &data)
            .map_err(|error| error.to_string())?;
        assert!(loaded.diagnostics.is_empty());
        let local = loaded
            .config
            .mcp_servers
            .get("local")
            .ok_or_else(|| "local server missing".to_string())?;
        let canonical_root = std::fs::canonicalize(&root)
            .map_err(|error| error.to_string())?
            .to_string_lossy()
            .into_owned();
        assert!(
            local
                .args
                .iter()
                .any(|argument| argument.ends_with("/server.js"))
        );
        assert_eq!(local.args.get(1).map(String::as_str), Some("你好"));
        assert_eq!(
            local.env.get("PLUGIN_ROOT").map(String::as_str),
            Some(canonical_root.as_str())
        );
        let remote = loaded
            .config
            .mcp_servers
            .get("remote")
            .ok_or_else(|| "remote server missing".to_string())?;
        assert_eq!(
            remote.headers.get("X-Literal").map(String::as_str),
            Some("${PLUGIN_ROOT}")
        );
        Ok(())
    }

    #[test]
    fn isolates_invalid_agent_plugin_mcp_entries() -> std::result::Result<(), String> {
        let (_temporary, root, data) = plugin_directories()?;
        let document = serde_json::json!({
            "$schema": AGENT_PLUGIN_MCP_SCHEMA_V1,
            "mcpServers": {
                "valid": {"type": "stdio", "command": "node"},
                "invalid": {"type": "stdio", "command": "node", "future": true},
                "insecure": {"type": "streamable-http", "url": "http://example.com/mcp"}
            }
        });
        let loaded = McpConfigFile::parse_agent_plugin(&document.to_string(), &root, &data)
            .map_err(|error| error.to_string())?;
        assert!(loaded.config.mcp_servers.contains_key("valid"));
        assert!(!loaded.config.mcp_servers.contains_key("invalid"));
        assert!(!loaded.config.mcp_servers.contains_key("insecure"));
        assert_eq!(loaded.diagnostics.len(), 2);
        Ok(())
    }

    #[test]
    fn rejects_invalid_agent_plugin_mcp_top_level() -> std::result::Result<(), String> {
        let (_temporary, root, data) = plugin_directories()?;
        let document = serde_json::json!({
            "$schema": AGENT_PLUGIN_MCP_SCHEMA_V1,
            "mcpServers": {},
            "future": true
        });
        assert!(McpConfigFile::parse_agent_plugin(&document.to_string(), &root, &data).is_err());
        Ok(())
    }

    #[test]
    fn variable_expansion_is_single_pass_and_unicode_safe() {
        let root = Path::new("/tmp/${PLUGIN_DATA}/插件");
        let data = Path::new("/tmp/data");
        assert_eq!(
            expand_agent_plugin_variables("前缀/${PLUGIN_ROOT}", root, data),
            "前缀//tmp/${PLUGIN_DATA}/插件"
        );
    }
}
