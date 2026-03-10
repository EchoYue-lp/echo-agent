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
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{McpError, ReactError, Result};
use crate::mcp::server_config::{McpServerConfig, TransportConfig};

// ── 配置文件结构 ──────────────────────────────────────────────────────────────

/// mcp.json 文件的顶层结构
///
/// 与 Claude Desktop / Cursor 的 `mcp.json` 格式完全兼容。
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct McpConfigFile {
    /// 服务端配置映射（key 为服务端名称，在同一 Agent 中唯一）
    #[serde(rename = "mcpServers", default)]
    pub mcp_servers: HashMap<String, McpServerEntry>,
}

/// 单个 MCP 服务端的配置项
///
/// 支持两种模式：
/// - **stdio**：提供 `command`，可选 `args` 和 `env`
/// - **HTTP**：提供 `url`，可选 `headers` 和 `transport`
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct McpServerEntry {
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
            return Err(ReactError::Mcp(McpError::ConnectionFailed(format!(
                "服务端 '{}' 已禁用（disabled: true）",
                name
            ))));
        }

        if let Some(command) = &self.command {
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
                },
            })
        } else if let Some(url) = &self.url {
            let transport = match self.transport.as_deref() {
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
            Err(ReactError::Mcp(McpError::ConnectionFailed(format!(
                "服务端 '{}' 配置无效：stdio 模式需提供 'command'，HTTP 模式需提供 'url'",
                name
            ))))
        }
    }
}

impl McpConfigFile {
    /// 从 JSON 字符串解析配置
    pub fn from_str(s: &str) -> Result<Self> {
        serde_json::from_str(s).map_err(|e| {
            ReactError::Mcp(McpError::ProtocolError(format!(
                "mcp.json 格式解析失败: {}",
                e
            )))
        })
    }

    /// 从文件路径加载配置
    ///
    /// # 示例
    /// ```rust,no_run
    /// use echo_agent::mcp::McpConfigFile;
    ///
    /// let config = McpConfigFile::from_file("mcp.json")?;
    /// println!("共 {} 个服务端", config.mcp_servers.len());
    /// # Ok::<(), echo_agent::error::ReactError>(())
    /// ```
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let content = std::fs::read_to_string(path).map_err(|e| {
            ReactError::Mcp(McpError::ConnectionFailed(format!(
                "读取配置文件失败 ({}): {}",
                path.display(),
                e
            )))
        })?;
        Self::from_str(&content)
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
