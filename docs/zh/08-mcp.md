# MCP 协议集成（Model Context Protocol）

## 是什么

MCP（Model Context Protocol）是 Anthropic 于 2024 年提出的开放标准，用于统一 LLM 应用与外部工具服务之间的通信格式。MCP 服务端暴露工具（tools）、资源（resources）和提示词（prompts），客户端（即 Agent）连接后自动发现并调用这些能力。

echo-agent 实现了完整的 MCP 客户端，支持最新的协议版本（2025-03-26），可以连接任何符合 MCP 规范的服务端，并将其工具无缝适配为框架的 `Tool` trait。

---

## 解决什么问题

### 工具生态的碎片化

传统方式下，每个 AI 框架都需要用自己的语言重新实现工具：

```
┌─────────────────┐     ┌─────────────────┐     ┌─────────────────┐
│   LangChain     │     │   AutoGPT       │     │   echo-agent    │
│  (Python 工具)  │     │  (Python 工具)  │     │  (Rust 工具)    │
└─────────────────┘     └─────────────────┘     └─────────────────┘
        ↓                       ↓                       ↓
  各自实现一遍            各自实现一遍              各自实现一遍
```

### MCP 的解决方案

MCP 将工具能力从应用中解耦：

```
                    ┌─────────────────────────────────────┐
                    │          MCP 服务端                  │
                    │  (Python / Node.js / Java / ...)    │
                    │  暴露 tools / resources / prompts   │
                    └──────────────┬──────────────────────┘
                                   │ MCP 协议
           ┌───────────────────────┼───────────────────────┐
           ↓                       ↓                       ↓
   ┌───────────────┐       ┌───────────────┐       ┌───────────────┐
   │ Claude Desktop │       │    Cursor     │       │  echo-agent   │
   │  (MCP 客户端)  │       │ (MCP 客户端)  │       │ (MCP 客户端)  │
   └───────────────┘       └───────────────┘       └───────────────┘
```

工具服务独立运行，任何 MCP 客户端都可以连接使用，实现"一次开发，处处可用"。

---

## MCP 协议原理

### 协议栈

MCP 基于 JSON-RPC 2.0 构建，是一个请求-响应协议：

```
┌─────────────────────────────────────────────────────────────┐
│                     应用层 (MCP)                             │
│  tools/list, tools/call, resources/list, prompts/get...    │
├─────────────────────────────────────────────────────────────┤
│                   JSON-RPC 2.0 层                            │
│  { "jsonrpc": "2.0", "id": 1, "method": "...", ... }       │
├─────────────────────────────────────────────────────────────┤
│                     传输层                                   │
│         stdio / HTTP (Streamable HTTP) / SSE                │
└─────────────────────────────────────────────────────────────┘
```

### 连接生命周期

```
客户端                                        服务端
  │                                            │
  │──── initialize(protocolVersion, ──────────>│  1. 握手
  │          capabilities, clientInfo)         │
  │                                            │
  │<─── InitializeResult(protocolVersion, ─────│
  │          capabilities, serverInfo)         │
  │                                            │
  │──── notifications/initialized ────────────>│  2. 通知就绪
  │                                            │
  │──── tools/list ───────────────────────────>│  3. 能力发现
  │<─── tools: [{name, description, schema}] ──│
  │                                            │
  │──── tools/call(name, arguments) ──────────>│  4. 工具调用
  │<─── content: [{type: "text", text: ...}] ──│
  │                                            │
  │──── resources/list ───────────────────────>│  5. 资源访问
  │<─── resources: [{uri, name, mimeType}] ────│
  │                                            │
  │──── prompts/list ─────────────────────────>│  6. 提示词获取
  │<─── prompts: [{name, description}] ────────│
  │                                            │
```

### 三大核心能力

| 能力 | 说明 | 典型用途 |
|------|------|---------|
| **Tools** | 可执行的操作 | 文件读写、API 调用、数据库查询 |
| **Resources** | 可读取的数据 | 文件内容、数据库记录、配置信息 |
| **Prompts** | 可复用的提示词模板 | 代码审查模板、文档生成模板 |

---

## 传输层详解

echo-agent 支持三种传输方式：

### 1. stdio（子进程，推荐本地工具）

Agent 启动工具服务作为子进程，通过 stdin/stdout 进行 JSON-RPC 通信：

```
┌───────────────────┐                    ┌───────────────────┐
│   echo-agent      │                    │   MCP Server      │
│   (父进程)        │                    │   (子进程)        │
│                   │   stdin ────────>  │                   │
│                   │   stdout <───────  │                   │
└───────────────────┘                    └───────────────────┘
```

**优点**：
- 零网络开销，延迟最低
- 生命周期与 Agent 绑定，自动清理
- 与现有 Node.js/Python 生态无缝集成

**配置方式**：
```rust
McpServerConfig::stdio(
    "filesystem",             // 服务名（任意标识）
    "npx",                    // 命令
    vec![
        "-y",
        "@modelcontextprotocol/server-filesystem",
        "/tmp"                // 服务访问的根目录
    ],
)
```

### 2. HTTP（Streamable HTTP，推荐远程服务）

符合 MCP 2025-03-26 规范的现代 HTTP 传输：

```
┌───────────────────┐                    ┌───────────────────┐
│   echo-agent      │                    │   MCP Server      │
│                   │   POST /mcp ────>  │                   │
│                   │   <── JSON-RPC ───  │                   │
│                   │                    │                   │
│                   │   GET /mcp (SSE)   │  可选：服务端推送 │
│                   │   <── events ────  │                   │
└───────────────────┘                    └───────────────────┘
```

**特点**：
- 单端点 POST 请求
- 自动携带 `MCP-Protocol-Version` 请求头
- 支持 `MCP-Session-Id` 会话管理
- 可选的 GET SSE 通知流

**配置方式**：
```rust
McpServerConfig::http("my-api", "http://localhost:3000/mcp");

// 带认证头
let mut headers = HashMap::new();
headers.insert("Authorization".to_string(), "Bearer token".to_string());
McpServerConfig::http_with_headers("secure-api", "https://api.example.com/mcp", headers);
```

### 3. SSE（旧版 HTTP+SSE，兼容旧 SDK）

适用于旧版 MCP SDK（2024-11-05 协议）：

```
┌───────────────────┐                    ┌───────────────────┐
│   echo-agent      │                    │   MCP Server      │
│                   │   GET /sse ──────> │                   │
│                   │   <── endpoint ─── │  建立 SSE 连接    │
│                   │                    │                   │
│                   │   POST /msg/xxx ─> │  动态端点通信     │
│                   │   <── JSON-RPC ─── │                   │
└───────────────────┘                    └───────────────────┘
```

**配置方式**：
```rust
McpServerConfig::sse("legacy-api", "http://localhost:8080");
```

---

## 配置文件格式

echo-agent 支持与 Claude Desktop / Cursor / VS Code 兼容的 `mcp.json` 格式：

```json
{
  "mcpServers": {
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "/workspace"],
      "env": {
        "OPTIONAL_VAR": "value"
      }
    },
    "github": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-github"],
      "env": {
        "GITHUB_PERSONAL_ACCESS_TOKEN": "ghp_xxx"
      }
    },
    "remote-api": {
      "url": "http://localhost:8080/mcp",
      "headers": {
        "Authorization": "Bearer token"
      }
    },
    "legacy-sse": {
      "url": "http://localhost:3000",
      "transport": "sse"
    },
    "disabled-server": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-postgres"],
      "disabled": true
    }
  }
}
```

**从配置文件加载**：
```rust
let mut agent = ReactAgent::new(config);
let clients = agent.load_mcp_from_file("mcp.json").await?;
println!("已连接 {} 个 MCP 服务端", clients.len());
```

---

## 使用方式

### 方式一：通过 Agent 直接连接

```rust
use echo_agent::prelude::*;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    let config = AgentConfig::new("qwen3-max", "agent", "你是一个文件系统助手")
        .enable_tool(true);
    let mut agent = ReactAgent::new(config);

    // 连接 MCP 文件系统服务端（生命周期绑定到 Agent）
    let client = agent.connect_mcp_from_config(McpServerConfig::stdio(
        "filesystem",
        "npx",
        vec!["-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
    )).await?;

    // 执行任务
    let answer = agent.execute("列出 /tmp 目录下的所有文件").await?;
    println!("{}", answer);

    // Agent 析构时自动关闭 MCP 连接
    Ok(())
}
```

### 方式二：通过 McpManager 管理连接

```rust
use echo_agent::prelude::*;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("file-agent")
        .system_prompt("你是一个文件操作助手")
        .enable_tools()
        .build()?;

    // 应用层管理 MCP 生命周期
    let mut mcp = McpManager::new();
    let tools = mcp.connect(McpServerConfig::stdio(
        "filesystem",
        "npx",
        vec!["-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
    )).await?;

    // 注册工具
    agent.add_tools(tools);

    // 执行任务
    let answer = agent.execute("列出 /tmp 目录下的所有文件").await?;
    println!("{}", answer);

    // 手动关闭连接
    mcp.close_all().await;
    Ok(())
}
```

---

## 多服务端连接

```rust
let mut mcp = McpManager::new();

// 文件系统工具
let fs_tools = mcp.connect(McpServerConfig::stdio(
    "filesystem",
    "npx",
    vec!["-y", "@modelcontextprotocol/server-filesystem", "/workspace"],
)).await?;

// GitHub 工具
let gh_tools = mcp.connect(McpServerConfig::stdio(
    "github",
    "npx",
    vec!["-y", "@modelcontextprotocol/server-github"],
)).await?;

// 远程 API 工具
let api_tools = mcp.connect(McpServerConfig::http(
    "internal-api",
    "http://api-server:8080/mcp"
)).await?;

// 注册所有工具
agent.add_tools(fs_tools);
agent.add_tools(gh_tools);
agent.add_tools(api_tools);

// 或一次性注册
agent.add_tools(mcp.get_all_tools());
```

---

## 工具适配原理

MCP 工具通过 `McpToolAdapter` 适配为框架的 `Tool` trait：

```
MCP 服务端声明：
{
  "name": "read_file",
  "description": "读取文件内容",
  "inputSchema": {
    "type": "object",
    "properties": {
      "path": { "type": "string", "description": "文件路径" }
    },
    "required": ["path"]
  }
}

                    ↓ McpToolAdapter 适配

impl Tool for McpToolAdapter {
    fn name(&self)        -> "read_file"
    fn description(&self) -> "读取文件内容"
    fn parameters(&self)  -> 原始 inputSchema (JSON Schema)
    async fn execute(&self, params) -> Result<ToolResult> {
        // 1. 将 params 序列化为 JSON
        // 2. 调用 MCP 的 tools/call 方法
        // 3. 将 MCP 返回的 content 转换为 ToolResult
    }
}
```

对 Agent 来说，MCP 工具和本地 Rust 工具没有任何区别，都可以通过 `execute()` 调用。

---

## 资源（Resources）访问

MCP 资源是一种只读数据源，由服务端暴露 URI 形式的资源：

```rust
// 获取资源列表
if let Some(client) = mcp.get_client("filesystem") {
    if client.supports_resources() {
        for resource in client.resources() {
            println!("资源: {} ({})", resource.name, resource.uri);
        }

        // 读取资源内容
        let result = client.read_resource("file:///workspace/README.md").await?;
        for content in result.contents {
            match content {
                McpResourceContents::Text { text, .. } => println!("{}", text),
                McpResourceContents::Blob { blob, .. } => {/* Base64 数据 */},
            }
        }
    }
}
```

---

## 提示词（Prompts）使用

MCP 提示词是预定义的模板，可接受参数：

```rust
if let Some(client) = mcp.get_client("code-review") {
    if client.supports_prompts() {
        // 获取提示词列表
        for prompt in client.prompts() {
            println!("提示词: {} - {}", prompt.name, prompt.description.unwrap_or_default());
        }

        // 获取提示词内容
        let mut args = HashMap::new();
        args.insert("language".to_string(), "rust".to_string());
        args.insert("file".to_string(), "src/main.rs".to_string());

        let result = client.get_prompt("code_review", Some(args)).await?;
        for msg in result.messages {
            println!("[{}] {}", msg.role, /* content text */);
        }
    }
}
```

---

## 查询已连接服务端

```rust
// 列出所有已连接的服务端
println!("已连接 MCP 服务端: {:?}", mcp.server_names());

// 获取特定服务端的客户端引用
if let Some(client) = mcp.get_client("filesystem") {
    println!("filesystem 提供 {} 个工具", client.tools().len());
    println!("协议版本: {}", client.protocol_version());

    // 查看服务端能力
    let caps = client.server_capabilities();
    println!("支持工具: {}", caps.tools.is_some());
    println!("支持资源: {}", caps.resources.is_some());
    println!("支持提示词: {}", caps.prompts.is_some());

    // 健康检查
    client.ping().await?;
}
```

---

## 常用 MCP 服务端

| 服务端 | 安装命令 | 能力 |
|--------|---------|------|
| 文件系统 | `npx -y @modelcontextprotocol/server-filesystem <dir>` | 文件读写、目录列表 |
| GitHub | `npx -y @modelcontextprotocol/server-github` | PR、Issue、代码搜索 |
| Brave Search | `npx -y @modelcontextprotocol/server-brave-search` | 网页搜索 |
| PostgreSQL | `npx -y @modelcontextprotocol/server-postgres <url>` | SQL 查询 |
| Puppeteer | `npx -y @modelcontextprotocol/server-puppeteer` | 浏览器自动化 |
| Slack | `npx -y @modelcontextprotocol/server-slack` | 消息发送、频道管理 |
| Google Maps | `npx -y @modelcontextprotocol/server-google-maps` | 地理编码、路线规划 |

> 完整列表见 [MCP 服务端目录](https://github.com/modelcontextprotocol/servers)

---

## 错误处理

MCP 操作可能产生的错误：

| 错误类型 | 说明 | 处理建议 |
|---------|------|---------|
| `McpError::ConnectionFailed` | 无法连接服务端 | 检查命令/URL 是否正确 |
| `McpError::InitializationFailed` | 握手失败 | 检查协议版本兼容性 |
| `McpError::ProtocolError` | 协议层错误 | 检查 JSON 格式 |
| `McpError::ToolCallFailed` | 工具调用失败 | 检查参数是否正确 |
| `McpError::TransportClosed` | 传输层已关闭 | 重新连接服务端 |

---

对应示例：`examples/demo06_mcp.rs`