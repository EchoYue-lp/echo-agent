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
    /// HTTP 传输：通过 HTTP POST 发送 JSON-RPC 请求
    /// 适用场景：远程 MCP 服务（SaaS 工具、共享服务等）
    Http {
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

    /// 创建 HTTP 配置
    ///
    /// # 示例
    /// ```
    /// McpServerConfig::http("my-api", "http://localhost:3000");
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
    /// McpServerConfig::http_with_headers("secure-api", "https://api.example.com", headers);
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
}
