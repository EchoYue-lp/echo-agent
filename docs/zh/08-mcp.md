# MCP 协议集成（Model Context Protocol）

## 是什么

MCP（Model Context Protocol）是 Anthropic 提出的开放标准，用于统一 LLM 应用与外部工具服务之间的通信格式。MCP 服务端暴露工具（tools）、资源（resources）和提示词（prompts），客户端（即 Agent）连接后自动发现并调用这些能力。

echo-agent 实现了 MCP 客户端，可以连接任何符合 MCP 规范的服务端，并将其工具无缝适配为框架的 `Tool` trait。

---

## 解决什么问题

工具生态的碎片化问题：
- 每个工具都需要用 Rust 实现 `Tool` trait
- 现有大量 Python/Node.js 生态工具无法复用
- 团队内部有工具服务，希望多个 AI 应用共享

MCP 将工具能力从应用中解耦：工具服务独立运行，任何 MCP 客户端（包括 Claude Desktop、Cursor、echo-agent 等）都可以连接使用。

---

## 两种传输方式

### stdio（子进程，推荐）

Agent 作为父进程启动工具服务作为子进程，通过 stdin/stdout 通信。

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

适用：本地工具、Node.js/Python 生态工具包（`npx`、`uvx`、`python -m`）

### HTTP SSE（远程服务）

连接到已运行的 MCP HTTP 服务端。

```rust
McpServerConfig::http(
    "remote_tools",
    "http://tool-server.internal:8080/sse"
)
```

适用：团队共享工具服务、远程部署的工具服务器

---

## 使用方式

```rust
use echo_agent::prelude::*;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    let config = AgentConfig::new("qwen3-max", "agent", "你是一个文件系统助手")
        .enable_tool(true);
    let mut agent = ReactAgent::new(config);

    // 连接 MCP 文件系统服务端
    let mut mcp = McpManager::new();
    let tools = mcp.connect(McpServerConfig::stdio(
        "filesystem",
        "npx",
        vec!["-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
    )).await?;

    // 将 MCP 工具注册到 Agent（无需额外适配）
    agent.add_tools(tools);

    let answer = agent
        .execute("列出 /tmp 目录下的所有文件")
        .await?;
    println!("{}", answer);

    // 关闭所有 MCP 连接
    mcp.close_all().await;
    Ok(())
}
```

---

## 同时连接多个 MCP 服务端

```rust
let mut mcp = McpManager::new();

// 文件系统工具
let fs_tools = mcp.connect(McpServerConfig::stdio(
    "filesystem",
    "npx",
    vec!["-y", "@modelcontextprotocol/server-filesystem", "/workspace"],
)).await?;

// 数据库查询工具
let db_tools = mcp.connect(McpServerConfig::http(
    "database",
    "http://db-mcp-server:8080/sse"
)).await?;

// 将所有工具注册到 Agent
agent.add_tools(fs_tools);
agent.add_tools(db_tools);

// 或者直接获取所有已连接的工具
let all_tools = mcp.get_all_tools();
agent.add_tools(all_tools);
```

---

## 工具适配原理

MCP 工具 → `McpToolAdapter` → `Tool` trait

```
MCP 服务端声明：
{
  "name": "read_file",
  "description": "Read file contents",
  "inputSchema": { ... }
}

McpToolAdapter 适配后：
impl Tool for McpToolAdapter {
    fn name() → "read_file"
    fn description() → "Read file contents"
    fn parameters() → 原始 inputSchema
    async fn execute(params) → 调用 MCP 协议 call_tool，返回结果
}
```

对 Agent 来说，MCP 工具和本地 Rust 工具没有任何区别。

---

## 查询已连接服务端

```rust
// 列出所有已连接的服务端
println!("已连接 MCP 服务端: {:?}", mcp.server_names());

// 获取特定服务端的客户端引用
if let Some(client) = mcp.get_client("filesystem") {
    println!("filesystem 提供 {} 个工具", client.tools().len());
    for tool in client.tools() {
        println!("  - {}: {}", tool.name, tool.description);
    }
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

> 完整列表见 [MCP 服务端目录](https://github.com/modelcontextprotocol/servers)

对应示例：`examples/demo06_mcp.rs`
