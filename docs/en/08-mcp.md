# MCP Integration (Model Context Protocol)

## What It Is

MCP (Model Context Protocol) is an open standard proposed by Anthropic that unifies communication between LLM applications and external tool services. An MCP server exposes tools, resources, and prompts; an MCP client (i.e., an Agent) connects and automatically discovers and invokes those capabilities.

echo-agent implements an MCP client that connects to any spec-compliant MCP server and seamlessly adapts its tools to the framework's `Tool` trait.

---

## Problem It Solves

Tool ecosystem fragmentation:
- Every tool must be implemented in Rust with the `Tool` trait
- Existing Python/Node.js ecosystem tools cannot be reused
- Teams have internal tool services they want to share across multiple AI applications

MCP decouples tool capabilities from the application: tool services run independently, and any MCP client (Claude Desktop, Cursor, echo-agent, etc.) can connect and use them.

---

## Two Transport Modes

### stdio (subprocess, recommended)

The Agent spawns the tool service as a child process and communicates via stdin/stdout.

```rust
McpServerConfig::stdio(
    "filesystem",             // server name (any identifier)
    "npx",                    // command to run
    vec![
        "-y",
        "@modelcontextprotocol/server-filesystem",
        "/tmp"                // root directory the server can access
    ],
)
```

Best for: local tools, Node.js/Python ecosystem packages (`npx`, `uvx`, `python -m`)

### HTTP SSE (remote server)

Connect to an already-running MCP HTTP server.

```rust
McpServerConfig::http(
    "remote_tools",
    "http://tool-server.internal:8080/sse"
)
```

Best for: team-shared tool services, remotely deployed tool servers

---

## Usage

```rust
use echo_agent::prelude::*;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    let config = AgentConfig::new("gpt-4o", "agent", "You are a filesystem assistant")
        .enable_tool(true);
    let mut agent = ReactAgent::new(config);

    // Connect to the MCP filesystem server
    let mut mcp = McpManager::new();
    let tools = mcp.connect(McpServerConfig::stdio(
        "filesystem",
        "npx",
        vec!["-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
    )).await?;

    // Register MCP tools on the Agent (no extra adapter needed)
    agent.add_tools(tools);

    let answer = agent
        .execute("List all files in /tmp")
        .await?;
    println!("{}", answer);

    mcp.close_all().await;
    Ok(())
}
```

---

## Connecting Multiple MCP Servers

```rust
let mut mcp = McpManager::new();

// Filesystem tools
let fs_tools = mcp.connect(McpServerConfig::stdio(
    "filesystem",
    "npx",
    vec!["-y", "@modelcontextprotocol/server-filesystem", "/workspace"],
)).await?;

// Database query tools
let db_tools = mcp.connect(McpServerConfig::http(
    "database",
    "http://db-mcp-server:8080/sse"
)).await?;

agent.add_tools(fs_tools);
agent.add_tools(db_tools);

// Or register all tools from all connected servers at once
let all_tools = mcp.get_all_tools();
agent.add_tools(all_tools);
```

---

## How Tool Adaptation Works

MCP tool → `McpToolAdapter` → `Tool` trait

```
MCP server declares:
{
  "name": "read_file",
  "description": "Read file contents",
  "inputSchema": { ... }
}

After McpToolAdapter:
impl Tool for McpToolAdapter {
    fn name()         → "read_file"
    fn description()  → "Read file contents"
    fn parameters()   → original inputSchema
    async fn execute(params) → call MCP protocol call_tool, return result
}
```

To an Agent, MCP tools are indistinguishable from native Rust tools.

---

## Inspecting Connected Servers

```rust
println!("Connected MCP servers: {:?}", mcp.server_names());

if let Some(client) = mcp.get_client("filesystem") {
    println!("filesystem provides {} tools", client.tools().len());
    for tool in client.tools() {
        println!("  - {}: {}", tool.name, tool.description);
    }
}
```

---

## Popular MCP Servers

| Server | Install | Capabilities |
|--------|---------|-------------|
| Filesystem | `npx -y @modelcontextprotocol/server-filesystem <dir>` | File read/write, directory listing |
| GitHub | `npx -y @modelcontextprotocol/server-github` | PRs, Issues, code search |
| Brave Search | `npx -y @modelcontextprotocol/server-brave-search` | Web search |
| PostgreSQL | `npx -y @modelcontextprotocol/server-postgres <url>` | SQL queries |
| Puppeteer | `npx -y @modelcontextprotocol/server-puppeteer` | Browser automation |

> Full list: [MCP Servers Directory](https://github.com/modelcontextprotocol/servers)

See: `examples/demo06_mcp.rs`
