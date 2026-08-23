# MCP 协议 (Model Context Protocol)

本文档介绍 Model Context Protocol (MCP) 的核心概念及 echo-agent 的实现。

---

## 概述

MCP (Model Context Protocol) 是 Anthropic 提出的开放协议，用于 LLM 应用与外部工具/数据源之间的标准化通信。它解决了以下问题：

- **工具发现**：统一的方式发现可用工具
- **工具调用**：标准化的工具调用协议
- **资源访问**：访问文件、数据库等外部资源
- **提示词模板**：可复用的提示词模板

---

## 核心概念

### 架构模型

```
┌─────────────────────────────────────────────────────────────────────┐
│                        MCP Architecture                              │
│                                                                      │
│   ┌─────────────┐                          ┌─────────────────────┐  │
│   │   Client    │◀────── Transport ───────▶│      Server         │  │
│   │  (echo-agent)│                          │  (MCP Server)      │  │
│   └─────────────┘                          └─────────────────────┘  │
│         │                                           │               │
│         │  - discover tools                         │               │
│         │  - call tools                             │               │
│         │  - access resources                       │               │
│         │  - get prompts                            │               │
│         │                                           │               │
│   ┌─────┴─────┐                              ┌──────┴──────┐       │
│   │ McpManager│                              │ Tool Impl   │       │
│   └───────────┘                              │ Resource    │       │
│                                              │ Prompt      │       │
│                                              └─────────────┘       │
└─────────────────────────────────────────────────────────────────────┘
```

### 传输层

MCP 支持三种传输方式：

| 传输类型 | 描述 | 适用场景 |
|----------|------|----------|
| **stdio** | 标准输入/输出流 | 本地进程，如 npx 启动的服务器 |
| **SSE** | Server-Sent Events | 服务器推送事件，单向流 |
| **HTTP** | REST API | 远程服务，标准 HTTP 请求 |

---

## echo-agent 实现

### McpManager

```rust
// echo-mcp/src/client.rs
pub struct McpManager {
    connections: HashMap<String, McpConnection>,
}

impl McpManager {
    /// 连接 MCP 服务器
    pub async fn connect(&mut self, config: McpServerConfig) -> Result<Vec<McpTool>> {
        let connection = match config.transport {
            TransportConfig::Stdio { command, args } => {
                StdioTransport::connect(&command, &args).await?
            }
            TransportConfig::Http { url } => {
                HttpTransport::connect(&url).await?
            }
            TransportConfig::Sse { url } => {
                SseTransport::connect(&url).await?
            }
        };
        
        // 发现工具
        let tools = connection.list_tools().await?;
        self.connections.insert(config.name, connection);
        Ok(tools)
    }
    
    /// 调用 MCP 工具
    pub async fn call_tool(&self, server: &str, tool: &str, args: Value) -> Result<Value> {
        self.connections.get(server)
            .ok_or(McpError::ServerNotFound)?
            .call_tool(tool, args).await
    }
}
```

### 服务器配置

```rust
// echo-mcp/src/config_loader.rs
pub struct McpServerConfig {
    pub name: String,
    pub transport: TransportConfig,
}

pub enum TransportConfig {
    Stdio {
        command: String,
        args: Vec<String>,
    },
    Http {
        url: String,
    },
    Sse {
        url: String,
    },
}

// 便捷构造
impl McpServerConfig {
    pub fn stdio(name: &str, command: &str, args: Vec<&str>) -> Self {
        Self {
            name: name.to_string(),
            transport: TransportConfig::Stdio {
                command: command.to_string(),
                args: args.iter().map(|s| s.to_string()).collect(),
            },
        }
    }
    
    pub fn http(name: &str, url: &str) -> Self {
        Self {
            name: name.to_string(),
            transport: TransportConfig::Http { url: url.to_string() },
        }
    }
}
```

### MCP 工具适配

```rust
// echo-mcp/src/tool_adapter.rs

/// 将 MCP Tool 转换为 echo-agent Tool trait
pub struct McpTool {
    name: String,
    description: String,
    parameters: Value,  // JSON Schema
    server_name: String,
    manager: Arc<McpManager>,
}

impl Tool for McpTool {
    fn name(&self) -> &str { &self.name }
    fn description(&self) -> &str { &self.description }
    fn parameters(&self) -> Value { self.parameters.clone() }
    
    fn execute(&self, params: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        let server = self.server_name.clone();
        let tool = self.name.clone();
        let manager = self.manager.clone();
        
        Box::pin(async move {
            let value = serde_json::to_value(&params)?;
            let result = manager.call_tool(&server, &tool, value).await?;
            Ok(ToolResult::success(serde_json::to_string(&result)?))
        })
    }
}
```

---

## 使用示例

### 连接文件系统 MCP 服务器

```rust
use echo_agent::mcp::{McpManager, McpServerConfig};

let mut mcp = McpManager::new();

// 连接 stdio MCP 服务器
let tools = mcp.connect(McpServerConfig::stdio(
    "filesystem",
    "npx",
    vec!["-y", "@modelcontextprotocol/server-filesystem", "/workspace"]
)).await?;

println!("发现 {} 个工具:", tools.len());
for tool in &tools {
    println!("  - {}: {}", tool.name(), tool.description());
}

// 将工具添加到 Agent
agent.add_tools(tools);
```

### YAML 配置

```yaml
# application configuration
mcp:
  servers:
    filesystem:
      transport: stdio
      command: npx
      args:
        - "-y"
        - "@modelcontextprotocol/server-filesystem"
        - "/workspace"
    
    github:
      transport: http
      url: http://localhost:3001/mcp
      
    database:
      transport: sse
      url: http://localhost:3002/sse
```

### 运行时动态连接

```rust
// 运行时加载配置
let config = McpConfigLoader::from_yaml("application configuration")?;

for server in config.mcp.servers {
    let tools = mcp.connect(server).await?;
    agent.add_tools(tools);
}
```

---

## MCP 服务器实现

echo-agent 也支持作为 MCP 服务器运行：

```rust
// demo30_mcp_server.rs
use echo_agent::mcp::server::McpServer;

let agent = ReactAgentBuilder::simple("qwen3-max", "翻译助手")?;

let server = McpServer::new(agent)
    .with_tool("translate", "翻译文本", |args| {
        // 工具实现
    })
    .with_resource("dictionary", "词典资源", || {
        // 资源实现
    });

server.start("127.0.0.1:3000").await?;
```

---

## MCP vs 传统工具系统

| 维度 | 传统工具 | MCP |
|------|----------|-----|
| 发现机制 | 手动注册 | 自动发现 |
| 部署方式 | 同进程 | 独立进程/远程 |
| 热更新 | 不支持 | 支持动态连接 |
| 资源访问 | 需自己实现 | 标准化 API |
| 跨语言 | 困难 | 任何语言实现 MCP 服务器 |

---

## 官方 MCP 服务器

| 服务器 | 功能 | 安装 |
|--------|------|------|
| filesystem | 文件系统操作 | `npx @modelcontextprotocol/server-filesystem` |
| postgres | PostgreSQL 数据库 | `npx @modelcontextprotocol/server-postgres` |
| github | GitHub API | `npx @modelcontextprotocol/server-github` |
| brave-search | Brave 搜索 | `npx @modelcontextprotocol/server-brave-search` |
| puppeteer | 浏览器自动化 | `npx @modelcontextprotocol/server-puppeteer` |

---

## 参考资料

- [MCP 官方规范](https://modelcontextprotocol.io/)
- [MCP GitHub 仓库](https://github.com/modelcontextprotocol)
- [Anthropic MCP 公告](https://www.anthropic.com/news/model-context-protocol)