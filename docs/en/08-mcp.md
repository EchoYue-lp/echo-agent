# MCP Integration (Model Context Protocol)

## What It Is

MCP (Model Context Protocol) is an open standard proposed by Anthropic in 2024 that unifies communication between LLM applications and external tool services. An MCP server exposes tools, resources, and prompts; an MCP client (i.e., an Agent) connects and automatically discovers and invokes those capabilities.

echo-agent implements a complete MCP client supporting the latest protocol version (2025-03-26), capable of connecting to any spec-compliant server and seamlessly adapting its tools to the framework's `Tool` trait.

---

## Problem It Solves

### Tool Ecosystem Fragmentation

Traditionally, every AI framework reimplements tools in its own language:

```
┌─────────────────┐     ┌─────────────────┐     ┌─────────────────┐
│   LangChain     │     │   AutoGPT       │     │   echo-agent    │
│ (Python tools)  │     │ (Python tools)  │     │  (Rust tools)   │
└─────────────────┘     └─────────────────┘     └─────────────────┘
        ↓                       ↓                       ↓
  Implement once         Implement once          Implement once
```

### MCP's Solution

MCP decouples tool capabilities from the application:

```
                    ┌─────────────────────────────────────┐
                    │          MCP Server                  │
                    │  (Python / Node.js / Java / ...)    │
                    │  exposes tools/resources/prompts    │
                    └──────────────┬──────────────────────┘
                                   │ MCP Protocol
           ┌───────────────────────┼───────────────────────┐
           ↓                       ↓                       ↓
   ┌───────────────┐       ┌───────────────┐       ┌───────────────┐
   │ Claude Desktop │       │    Cursor     │       │  echo-agent   │
   │ (MCP client)  │       │ (MCP client)  │       │ (MCP client)  │
   └───────────────┘       └───────────────┘       └───────────────┘
```

Tool services run independently, and any MCP client can connect and use them—"write once, run anywhere."

---

## MCP Protocol Fundamentals

### Protocol Stack

MCP is built on JSON-RPC 2.0, a request-response protocol:

```
┌─────────────────────────────────────────────────────────────┐
│                   Application Layer (MCP)                   │
│  tools/list, tools/call, resources/list, prompts/get...    │
├─────────────────────────────────────────────────────────────┤
│                   JSON-RPC 2.0 Layer                         │
│  { "jsonrpc": "2.0", "id": 1, "method": "...", ... }       │
├─────────────────────────────────────────────────────────────┤
│                     Transport Layer                          │
│         stdio / HTTP (Streamable HTTP) / SSE                │
└─────────────────────────────────────────────────────────────┘
```

### Connection Lifecycle

```
Client                                        Server
  │                                            │
  │──── initialize(protocolVersion, ──────────>│  1. Handshake
  │          capabilities, clientInfo)         │
  │                                            │
  │<─── InitializeResult(protocolVersion, ─────│
  │          capabilities, serverInfo)         │
  │                                            │
  │──── notifications/initialized ────────────>│  2. Ready notification
  │                                            │
  │──── tools/list ───────────────────────────>│  3. Capability discovery
  │<─── tools: [{name, description, schema}] ──│
  │                                            │
  │──── tools/call(name, arguments) ──────────>│  4. Tool invocation
  │<─── content: [{type: "text", text: ...}] ──│
  │                                            │
  │──── resources/list ───────────────────────>│  5. Resource access
  │<─── resources: [{uri, name, mimeType}] ────│
  │                                            │
  │──── prompts/list ─────────────────────────>│  6. Prompt retrieval
  │<─── prompts: [{name, description}] ────────│
  │                                            │
```

### Three Core Capabilities

| Capability | Description | Typical Use |
|------------|-------------|-------------|
| **Tools** | Executable operations | File read/write, API calls, database queries |
| **Resources** | Readable data | File contents, database records, configuration |
| **Prompts** | Reusable prompt templates | Code review templates, documentation generators |

---

## Transport Layer Details

echo-agent supports three transport modes:

### 1. stdio (subprocess, recommended for local tools)

Agent spawns the tool service as a child process and communicates via stdin/stdout:

```
┌───────────────────┐                    ┌───────────────────┐
│   echo-agent      │                    │   MCP Server      │
│   (parent)        │                    │   (child)         │
│                   │   stdin ────────>  │                   │
│                   │   stdout <───────  │                   │
└───────────────────┘                    └───────────────────┘
```

**Advantages**:
- Zero network overhead, lowest latency
- Lifecycle bound to Agent, automatic cleanup
- Seamless integration with existing Node.js/Python ecosystem

**Configuration**:
```rust
McpServerConfig::stdio(
    "filesystem",             // server name (any identifier)
    "npx",                    // command
    vec![
        "-y",
        "@modelcontextprotocol/server-filesystem",
        "/tmp"                // root directory the server can access
    ],
)
```

### 2. HTTP (Streamable HTTP, recommended for remote services)

Modern HTTP transport compliant with MCP 2025-03-26 specification:

```
┌───────────────────┐                    ┌───────────────────┐
│   echo-agent      │                    │   MCP Server      │
│                   │   POST /mcp ────>  │                   │
│                   │   <── JSON-RPC ─── │                   │
│                   │                    │                   │
│                   │   GET /mcp (SSE)   │  Optional: push   │
│                   │   <── events ────  │                   │
└───────────────────┘                    └───────────────────┘
```

**Features**:
- Single endpoint POST requests
- Automatic `MCP-Protocol-Version` header
- `MCP-Session-Id` session management support
- Optional GET SSE notification stream

**Configuration**:
```rust
McpServerConfig::http("my-api", "http://localhost:3000/mcp");

// With authentication headers
let mut headers = HashMap::new();
headers.insert("Authorization".to_string(), "Bearer token".to_string());
McpServerConfig::http_with_headers("secure-api", "https://api.example.com/mcp", headers);
```

### 3. SSE (Legacy HTTP+SSE, for older SDKs)

For older MCP SDKs (2024-11-05 protocol):

```
┌───────────────────┐                    ┌───────────────────┐
│   echo-agent      │                    │   MCP Server      │
│                   │   GET /sse ──────> │                   │
│                   │   <── endpoint ─── │  Establish SSE    │
│                   │                    │                   │
│                   │   POST /msg/xxx ─> │  Dynamic endpoint │
│                   │   <── JSON-RPC ─── │                   │
└───────────────────┘                    └───────────────────┘
```

**Configuration**:
```rust
McpServerConfig::sse("legacy-api", "http://localhost:8080");
```

---

## Configuration File Format

echo-agent supports `mcp.json` format compatible with Claude Desktop / Cursor / VS Code:

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

**Loading from config file**:
```rust
let mut agent = ReactAgent::new(config);
let clients = agent.load_mcp_from_file("mcp.json").await?;
println!("Connected {} MCP servers", clients.len());
```

---

## Usage

### Method 1: Direct Connection via Agent

```rust
use echo_agent::prelude::*;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    let config = AgentConfig::new("qwen3-max", "agent", "You are a filesystem assistant")
        .enable_tool(true);
    let mut agent = ReactAgent::new(config);

    // Connect MCP filesystem server (lifecycle bound to Agent)
    let client = agent.connect_mcp_from_config(McpServerConfig::stdio(
        "filesystem",
        "npx",
        vec!["-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
    )).await?;

    // Execute task
    let answer = agent.execute("List all files in /tmp").await?;
    println!("{}", answer);

    // MCP connection closed automatically when Agent is dropped
    Ok(())
}
```

### Method 2: Manage Connections via McpManager

```rust
use echo_agent::prelude::*;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("file-agent")
        .system_prompt("You are a file operation assistant")
        .enable_tools()
        .build()?;

    // Application layer manages MCP lifecycle
    let mut mcp = McpManager::new();
    let tools = mcp.connect(McpServerConfig::stdio(
        "filesystem",
        "npx",
        vec!["-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
    )).await?;

    // Register tools
    agent.add_tools(tools);

    // Execute task
    let answer = agent.execute("List all files in /tmp").await?;
    println!("{}", answer);

    // Manually close connections
    mcp.close_all().await;
    Ok(())
}
```

---

## Multiple Server Connections

```rust
let mut mcp = McpManager::new();

// Filesystem tools
let fs_tools = mcp.connect(McpServerConfig::stdio(
    "filesystem",
    "npx",
    vec!["-y", "@modelcontextprotocol/server-filesystem", "/workspace"],
)).await?;

// GitHub tools
let gh_tools = mcp.connect(McpServerConfig::stdio(
    "github",
    "npx",
    vec!["-y", "@modelcontextprotocol/server-github"],
)).await?;

// Remote API tools
let api_tools = mcp.connect(McpServerConfig::http(
    "internal-api",
    "http://api-server:8080/mcp"
)).await?;

// Register all tools
agent.add_tools(fs_tools);
agent.add_tools(gh_tools);
agent.add_tools(api_tools);

// Or register all at once
agent.add_tools(mcp.get_all_tools());
```

---

## Tool Adaptation Mechanism

MCP tools are adapted to the framework's `Tool` trait via `McpToolAdapter`:

```
MCP server declares:
{
  "name": "read_file",
  "description": "Read file contents",
  "inputSchema": {
    "type": "object",
    "properties": {
      "path": { "type": "string", "description": "File path" }
    },
    "required": ["path"]
  }
}

                    ↓ McpToolAdapter adaptation

impl Tool for McpToolAdapter {
    fn name(&self)        -> "read_file"
    fn description(&self) -> "Read file contents"
    fn parameters(&self)  -> original inputSchema (JSON Schema)
    async fn execute(&self, params) -> Result<ToolResult> {
        // 1. Serialize params to JSON
        // 2. Call MCP tools/call method
        // 3. Convert MCP content to ToolResult
    }
}
```

To an Agent, MCP tools are indistinguishable from native Rust tools—both are invoked via `execute()`.

---

## Resource Access

MCP resources are read-only data sources exposed as URI-addressable resources:

```rust
// List resources
if let Some(client) = mcp.get_client("filesystem") {
    if client.supports_resources() {
        for resource in client.resources() {
            println!("Resource: {} ({})", resource.name, resource.uri);
        }

        // Read resource content
        let result = client.read_resource("file:///workspace/README.md").await?;
        for content in result.contents {
            match content {
                McpResourceContents::Text { text, .. } => println!("{}", text),
                McpResourceContents::Blob { blob, .. } => {/* Base64 data */},
            }
        }
    }
}
```

---

## Prompt Usage

MCP prompts are predefined templates that accept parameters:

```rust
if let Some(client) = mcp.get_client("code-review") {
    if client.supports_prompts() {
        // List prompts
        for prompt in client.prompts() {
            println!("Prompt: {} - {}", prompt.name, prompt.description.unwrap_or_default());
        }

        // Get prompt content
        let mut args = HashMap::new();
        args.insert("language".to_string(), "rust".to_string());
        args.insert("file".to_string(), "src/main.rs".to_string());

        let result = client.get_prompt("code_review", Some(args)).await?;
        for msg in result.messages {
            println!("[{}] {:?}", msg.role, msg.content);
        }
    }
}
```

---

## Inspecting Connected Servers

```rust
// List all connected servers
println!("Connected MCP servers: {:?}", mcp.server_names());

// Get specific server client reference
if let Some(client) = mcp.get_client("filesystem") {
    println!("filesystem provides {} tools", client.tools().len());
    println!("Protocol version: {}", client.protocol_version());

    // View server capabilities
    let caps = client.server_capabilities();
    println!("Supports tools: {}", caps.tools.is_some());
    println!("Supports resources: {}", caps.resources.is_some());
    println!("Supports prompts: {}", caps.prompts.is_some());

    // Health check
    client.ping().await?;
}
```

---

## Popular MCP Servers

| Server | Install Command | Capabilities |
|--------|----------------|--------------|
| Filesystem | `npx -y @modelcontextprotocol/server-filesystem <dir>` | File read/write, directory listing |
| GitHub | `npx -y @modelcontextprotocol/server-github` | PRs, Issues, code search |
| Brave Search | `npx -y @modelcontextprotocol/server-brave-search` | Web search |
| PostgreSQL | `npx -y @modelcontextprotocol/server-postgres <url>` | SQL queries |
| Puppeteer | `npx -y @modelcontextprotocol/server-puppeteer` | Browser automation |
| Slack | `npx -y @modelcontextprotocol/server-slack` | Message sending, channel management |
| Google Maps | `npx -y @modelcontextprotocol/server-google-maps` | Geocoding, directions |

> Full list: [MCP Servers Directory](https://github.com/modelcontextprotocol/servers)

---

## Error Handling

Potential MCP errors:

| Error Type | Description | Handling Suggestion |
|------------|-------------|---------------------|
| `McpError::ConnectionFailed` | Cannot connect to server | Check command/URL correctness |
| `McpError::InitializationFailed` | Handshake failed | Check protocol version compatibility |
| `McpError::ProtocolError` | Protocol layer error | Check JSON format |
| `McpError::ToolCallFailed` | Tool invocation failed | Check parameter correctness |
| `McpError::TransportClosed` | Transport layer closed | Reconnect to server |

---

See: `examples/demo06_mcp.rs`