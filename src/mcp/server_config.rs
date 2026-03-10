use std::collections::HashMap;

/// MCP 服务端完整配置
#[derive(Debug, Clone)]
pub struct McpServerConfig {
    /// 服务端标识名称
    pub name: String,
    /// 传输层配置
    pub transport: TransportConfig,
}

/// 传输层配置
#[derive(Debug, Clone)]
pub enum TransportConfig {
    /// stdio 传输：框架启动子进程，通过 stdin/stdout 通信
    /// 适用场景：本地工具（filesystem、git、sqlite 等）
    Stdio {
        command: String,
        args: Vec<String>,
        /// 额外注入的环境变量
        env: Vec<(String, String)>,
    },
    /// HTTP 传输：MCP Streamable HTTP（推荐）
    ///
    /// 符合 MCP 官方规范：
    /// - 直接 POST 到端点 URL
    /// - 自动携带 MCP-Protocol-Version 请求头
    /// - 支持 MCP-Session-Id 会话管理
    /// - 支持 GET SSE 通知流（服务端可选）
    ///
    /// 适用场景：远程 MCP 服务
    Http {
        /// MCP 服务端端点 URL
        base_url: String,
        /// 自定义请求头（如 Authorization）
        headers: HashMap<String, String>,
    },
    /// SSE (Server-Sent Events) 传输：旧版 HTTP+SSE
    ///
    /// 适用于旧版 MCP SDK（2024-11-05 协议）：
    /// - 在 `{base_url}/sse` 建立 SSE 连接
    /// - 从 endpoint 事件动态获取 POST URI
    Sse {
        base_url: String,
        /// 自定义请求头（如 Authorization）
        headers: HashMap<String, String>,
    },
}

impl McpServerConfig {
    /// 创建 stdio 配置（最常用）
    ///
    /// # 示例
    /// ```
    /// // 连接文件系统 MCP 服务端
    /// McpServerConfig::stdio("filesystem", "npx", vec![
    ///     "-y", "@modelcontextprotocol/server-filesystem", "/tmp"
    /// ]);
    /// ```
    pub fn stdio(
        name: impl Into<String>,
        command: impl Into<String>,
        args: Vec<impl Into<String>>,
    ) -> Self {
        Self {
            name: name.into(),
            transport: TransportConfig::Stdio {
                command: command.into(),
                args: args.into_iter().map(Into::into).collect(),
                env: vec![],
            },
        }
    }

    /// 创建 stdio 配置（带额外环境变量）
    pub fn stdio_with_env(
        name: impl Into<String>,
        command: impl Into<String>,
        args: Vec<impl Into<String>>,
        env: Vec<(impl Into<String>, impl Into<String>)>,
    ) -> Self {
        Self {
            name: name.into(),
            transport: TransportConfig::Stdio {
                command: command.into(),
                args: args.into_iter().map(Into::into).collect(),
                env: env.into_iter().map(|(k, v)| (k.into(), v.into())).collect(),
            },
        }
    }

    /// 创建 HTTP 配置（MCP Streamable HTTP，推荐用于远程服务）
    ///
    /// # 示例
    /// ```
    /// McpServerConfig::http("my-api", "http://localhost:3000/mcp");
    /// ```
    pub fn http(name: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            transport: TransportConfig::Http {
                base_url: base_url.into(),
                headers: HashMap::new(),
            },
        }
    }

    /// 创建 HTTP 配置（带自定义请求头）
    ///
    /// # 示例
    /// ```
    /// use std::collections::HashMap;
    /// let mut headers = HashMap::new();
    /// headers.insert("Authorization".to_string(), "Bearer token".to_string());
    /// McpServerConfig::http_with_headers("secure-api", "https://api.example.com/mcp", headers);
    /// ```
    pub fn http_with_headers(
        name: impl Into<String>,
        base_url: impl Into<String>,
        headers: HashMap<String, String>,
    ) -> Self {
        Self {
            name: name.into(),
            transport: TransportConfig::Http {
                base_url: base_url.into(),
                headers,
            },
        }
    }

    /// 创建 SSE 配置（旧版 HTTP+SSE，用于旧版 MCP SDK）
    ///
    /// # 示例
    /// ```
    /// McpServerConfig::sse("legacy-api", "http://localhost:8080");
    /// ```
    pub fn sse(name: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            transport: TransportConfig::Sse {
                base_url: base_url.into(),
                headers: HashMap::new(),
            },
        }
    }

    /// 创建 SSE 配置（带自定义请求头）
    pub fn sse_with_headers(
        name: impl Into<String>,
        base_url: impl Into<String>,
        headers: HashMap<String, String>,
    ) -> Self {
        Self {
            name: name.into(),
            transport: TransportConfig::Sse {
                base_url: base_url.into(),
                headers,
            },
        }
    }
}
