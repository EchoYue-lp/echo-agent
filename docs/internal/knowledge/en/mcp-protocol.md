# MCP Protocol (Model Context Protocol)

This document describes the core concepts of the Model Context Protocol (MCP) and the echo-agent implementation.

---

## Overview

MCP (Model Context Protocol) is an open protocol proposed by Anthropic for standardized communication between LLM applications and external tools/data sources. It addresses the following problems:

- **Tool Discovery**: A unified way to discover available tools
- **Tool Invocation**: A standardized tool calling protocol
- **Resource Access**: Access to external resources such as files and databases
- **Prompt Templates**: Reusable prompt templates

---

## Core Concepts

### Architecture Model

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

### Transport Layer

MCP supports three transport methods:

| Transport Type | Description | Suitable Scenarios |
|----------------|-------------|---------------------|
| **stdio** | Standard input/output streams | Local processes, e.g., servers started via npx |
| **SSE** | Server-Sent Events | Server push events, unidirectional streaming |
| **HTTP** | REST API | Remote services, standard HTTP requests |

---

## echo-agent Implementation

### McpManager

```rust
// echo-mcp/src/client.rs
pub struct McpManager {
    connections: HashMap<String, McpConnection>,
}

impl McpManager {
    /// Connect to an MCP server
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
        
        // Discover tools
        let tools = connection.list_tools().await?;
        self.connections.insert(config.name, connection);
        Ok(tools)
    }
    
    /// Call an MCP tool
    pub async fn call_tool(&self, server: &str, tool: &str, args: Value) -> Result<Value> {
        self.connections.get(server)
            .ok_or(McpError::ServerNotFound)?
            .call_tool(tool, args).await
    }
}
```

### Server Configuration

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

// Convenience constructors
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

### MCP Tool Adapter

```rust
// echo-mcp/src/tool_adapter.rs

/// Converts an MCP Tool into the echo-agent Tool trait
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

## Usage Examples

### Connecting to a Filesystem MCP Server

```rust
use echo_agent::mcp::{McpManager, McpServerConfig};

let mut mcp = McpManager::new();

// Connect to a stdio MCP server
let tools = mcp.connect(McpServerConfig::stdio(
    "filesystem",
    "npx",
    vec!["-y", "@modelcontextprotocol/server-filesystem", "/workspace"]
)).await?;

println!("Discovered {} tools:", tools.len());
for tool in &tools {
    println!("  - {}: {}", tool.name(), tool.description());
}

// Add tools to Agent
agent.add_tools(tools);
```

### YAML Configuration

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

### Runtime Dynamic Connection

```rust
// Load configuration at runtime
let config = McpConfigLoader::from_yaml("application configuration")?;

for server in config.mcp.servers {
    let tools = mcp.connect(server).await?;
    agent.add_tools(tools);
}
```

---

## MCP Server Implementation

echo-agent also supports running as an MCP server:

```rust
// demo30_mcp_server.rs
use echo_agent::mcp::server::McpServer;

let agent = ReactAgentBuilder::simple("qwen3-max", "Translation Assistant")?;

let server = McpServer::new(agent)
    .with_tool("translate", "Translate text", |args| {
        // Tool implementation
    })
    .with_resource("dictionary", "Dictionary resource", || {
        // Resource implementation
    });

server.start("127.0.0.1:3000").await?;
```

---

## MCP vs Traditional Tool Systems

| Dimension | Traditional Tools | MCP |
|-----------|-------------------|-----|
| Discovery mechanism | Manual registration | Automatic discovery |
| Deployment model | Same process | Separate process / remote |
| Hot reloading | Not supported | Supports dynamic connections |
| Resource access | Must be self-implemented | Standardized API |
| Cross-language support | Difficult | Any language can implement an MCP server |

---

## Official MCP Servers

| Server | Functionality | Installation |
|--------|---------------|--------------|
| filesystem | Filesystem operations | `npx @modelcontextprotocol/server-filesystem` |
| postgres | PostgreSQL database | `npx @modelcontextprotocol/server-postgres` |
| github | GitHub API | `npx @modelcontextprotocol/server-github` |
| brave-search | Brave Search | `npx @modelcontextprotocol/server-brave-search` |
| puppeteer | Browser automation | `npx @modelcontextprotocol/server-puppeteer` |

---

## References

- [MCP Official Specification](https://modelcontextprotocol.io/)
- [MCP GitHub Repository](https://github.com/modelcontextprotocol)
- [Anthropic MCP Announcement](https://www.anthropic.com/news/model-context-protocol)