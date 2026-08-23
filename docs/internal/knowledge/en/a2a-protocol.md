# A2A Protocol (Agent-to-Agent Protocol)

This document describes the A2A (Agent-to-Agent) protocol proposed by Google and the echo-agent implementation.

---

## Overview

The A2A protocol is an open standard for interoperability between different Agent frameworks. It defines:

- **Agent Card**: A card describing the Agent's capabilities
- **Task Lifecycle**: A task state machine
- **Message Format**: Standardized request/response messages
- **Streaming Events**: SSE real-time push events

---

## Core Concepts

### Agent Card

The Agent Card is the Agent's "business card," describing its capabilities and endpoints.

```json
{
  "name": "translator",
  "description": "Multilingual translation Agent",
  "version": "1.0.0",
  "url": "http://localhost:8080",
  "capabilities": {
    "streaming": true,
    "pushNotifications": false
  },
  "skills": [
    {
      "name": "translate",
      "description": "Translate text to a specified language",
      "input": {
        "type": "object",
        "properties": {
          "text": { "type": "string" },
          "target_lang": { "type": "string" }
        }
      }
    }
  ]
}
```

### Task State Machine

```
┌─────────────────────────────────────────────────────────────────────┐
│                     Task State Machine                               │
│                                                                      │
│                        submitted                                     │
│                           │                                         │
│                           ▼                                         │
│                        working ──────────────────┐                  │
│                           │                      │                   │
│              ┌────────────┼────────────┐        │                   │
│              │            │            │        │                   │
│              ▼            ▼            ▼        │                   │
│          completed    failed    input-required │                   │
│              │            │            │       │                   │
│              │            │            └───────┘                   │
│              │            │                                        │
│              ▼            ▼                                        │
│         (terminal)   (terminal)                                    │
│                                                                      │
│   Any non-terminal state ──────────▶ canceled                      │
└─────────────────────────────────────────────────────────────────────┘
```

| State | Description |
|-------|-------------|
| `submitted` | Task has been submitted, awaiting processing |
| `working` | Currently executing |
| `input-required` | Requires user input before continuing |
| `completed` | Execution completed (terminal state) |
| `failed` | Execution failed (terminal state) |
| `canceled` | Canceled (terminal state) |

---

## echo-agent Implementation

### A2AServer

```rust
// src/a2a/server.rs
pub struct A2AServer {
    card: AgentCard,
    agent: Box<dyn Agent>,
}

impl A2AServer {
    pub fn new(card: AgentCard, agent: impl Agent + 'static) -> Self {
        Self {
            card,
            agent: Box::new(agent),
        }
    }
    
    /// Handle synchronous requests
    pub async fn handle_request(&mut self, request: A2ARequest) -> Result<A2AResponse> {
        match request.method.as_str() {
            "tasks/send" => self.handle_send(request).await,
            "tasks/get" => self.handle_get(request).await,
            "tasks/cancel" => self.handle_cancel(request).await,
            _ => Err(A2AError::MethodNotFound),
        }
    }
    
    /// Handle streaming requests (SSE)
    pub async fn handle_request_stream(
        &mut self,
        request: A2ARequest,
    ) -> Result<impl Stream<Item = Result<A2AStreamEvent>>> {
        // Execute the agent and produce streaming events
    }
}
```

### A2AClient

```rust
// src/a2a/client.rs
pub struct A2AClient {
    base_url: String,
    http_client: reqwest::Client,
}

impl A2AClient {
    /// Discover a remote Agent
    pub async fn discover(&self) -> Result<AgentCard> {
        let url = format!("{}/.well-known/agent.json", self.base_url);
        let card = self.http_client.get(&url).send().await?.json().await?;
        Ok(card)
    }
    
    /// Send a task
    pub async fn send_task(&self, message: &str) -> Result<String> {
        let request = A2ARequest {
            method: "tasks/send".into(),
            params: json!({ "message": message }),
        };
        let response = self.http_client
            .post(&format!("{}/tasks/send", self.base_url))
            .json(&request)
            .send().await?
            .json().await?;
        Ok(response.result["task_id"].as_str().unwrap().to_string())
    }
    
    /// Subscribe to streaming events
    pub async fn subscribe(&self, task_id: &str) -> Result<impl Stream<Item = A2AStreamEvent>> {
        // SSE connection
    }
}
```

### AgentCard Builder

```rust
// src/a2a/types.rs
pub struct AgentCard {
    pub name: String,
    pub description: String,
    pub version: String,
    pub url: String,
    pub capabilities: AgentCapabilities,
    pub skills: Vec<AgentSkill>,
}

impl AgentCard {
    pub fn builder(name: &str, url: &str) -> AgentCardBuilder {
        AgentCardBuilder::new(name, url)
    }
}

pub struct AgentCardBuilder {
    card: AgentCard,
}

impl AgentCardBuilder {
    pub fn description(mut self, desc: &str) -> Self {
        self.card.description = desc.to_string();
        self
    }
    
    pub fn version(mut self, v: &str) -> Self {
        self.card.version = v.to_string();
        self
    }
    
    pub fn skill(mut self, skill: AgentSkill) -> Self {
        self.card.skills.push(skill);
        self
    }
    
    pub fn streaming(mut self) -> Self {
        self.card.capabilities.streaming = true;
        self
    }
    
    pub fn build(self) -> AgentCard {
        self.card
    }
}
```

---

## Usage Examples

### Server: Publishing an Agent

```rust
use echo_agent::a2a::{AgentCard, AgentSkill, A2AServer};
use echo_agent::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    // Create Agent
    let agent = ReactAgentBuilder::simple("qwen3-max", "Translation Assistant")?;
    
    // Define Agent Card
    let card = AgentCard::builder("translator", "http://localhost:8080")
        .description("Multilingual translation Agent, supporting Chinese, English, Japanese, and Korean")
        .version("1.0.0")
        .skill(AgentSkill::new("translate", "Translate text to a specified language"))
        .skill(AgentSkill::new("detect_language", "Detect the language of text"))
        .streaming()
        .build();
    
    // Start the server
    let server = A2AServer::new(card, agent);
    server.serve("0.0.0.0:8080").await?;
    
    Ok(())
}
```

### Client: Discovery and Invocation

```rust
use echo_agent::a2a::A2AClient;

#[tokio::main]
async fn main() -> Result<()> {
    let client = A2AClient::new("http://localhost:8080");
    
    // Discover Agent
    let card = client.discover().await?;
    println!("Discovered Agent: {} - {}", card.name, card.description);
    println!("Skills:");
    for skill in &card.skills {
        println!("  - {}: {}", skill.name, skill.description);
    }
    
    // Send task
    let task_id = client.send_task("Translate the following text into Japanese: Hello World").await?;
    println!("Task submitted: {}", task_id);
    
    // Get result
    let result = client.get_task(&task_id).await?;
    println!("Result: {:?}", result);
    
    Ok(())
}
```

### Streaming Subscription

```rust
// Subscribe to real-time events
let mut stream = client.subscribe(&task_id).await?;

while let Some(event) = stream.next().await {
    match event? {
        A2AStreamEvent::StatusUpdate { status, .. } => {
            println!("Status update: {:?}", status);
        }
        A2AStreamEvent::Artifact { content, .. } => {
            print!("{}", content);  // Streaming output
        }
        A2AStreamEvent::Completed { result } => {
            println!("\nCompleted: {}", result);
            break;
        }
    }
}
```

---

## API Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/.well-known/agent.json` | GET | Get Agent Card |
| `/tasks/send` | POST | Send task (synchronous) |
| `/tasks/sendSubscribe` | POST | Send task (streaming SSE) |
| `/tasks/get` | POST | Query task status |
| `/tasks/cancel` | POST | Cancel task |

### Request/Response Format

**Send task:**

```json
// POST /tasks/send
{
  "jsonrpc": "2.0",
  "method": "tasks/send",
  "params": {
    "message": "Translate: Hello",
    "skill": "translate"
  },
  "id": "req-123"
}

// Response
{
  "jsonrpc": "2.0",
  "result": {
    "task_id": "task-456",
    "status": "completed",
    "artifacts": [
      { "type": "text", "content": "Hello" }
    ]
  },
  "id": "req-123"
}
```

---

## Comparison with Other Protocols

| Protocol | Focus | Characteristics |
|----------|-------|-----------------|
| **A2A** | Inter-Agent communication | Standardized state machine, Agent Card |
| **MCP** | Tool/resource access | Tool discovery, resource access |
| **OpenAI Functions** | LLM tool calling | Simple, widely supported |
| **LangChain Tools** | In-application tools | No cross-framework standard |

### A2A vs MCP

- **A2A**: Agent discovery and task delegation
- **MCP**: Tool and resource access

The two are complementary:

```rust
// Agent A calls Agent B via A2A
// Agent B accesses external tools via MCP

let a2a_client = A2AClient::new("http://agent-b:8080");
let result = a2a_client.send_task("Analyze data").await?;
```

---

## Practical Application Scenarios

### 1. Multi-Agent Collaboration

```
┌─────────────┐     A2A      ┌─────────────┐     A2A      ┌─────────────┐
│ Orchestrator│─────────────▶│ Researcher  │─────────────▶│   Writer    │
└─────────────┘              └─────────────┘              └─────────────┘
      │                            │                            │
      │                            │ MCP                        │ MCP
      ▼                            ▼                            ▼
┌─────────────┐              ┌─────────────┐              ┌─────────────┐
│   Planner   │              │ Web Search  │              │ File System │
└─────────────┘              └─────────────┘              └─────────────┘
```

### 2. Cross-Framework Interoperability

```
┌───────────────────────────────────────────────────────────────┐
│                     A2A Gateway                               │
│                                                               │
│  ┌─────────┐     ┌─────────┐     ┌─────────┐                 │
│  │echo-agent│     │LangChain│     │ CrewAI  │                 │
│  │ (Rust)  │     │(Python) │     │(Python) │                 │
│  └────┬────┘     └────┬────┘     └────┬────┘                 │
│       │               │               │                       │
│       └───────────────┴───────────────┘                       │
│                       │                                       │
│                  A2A Protocol                                 │
└───────────────────────────────────────────────────────────────┘
```

---

## References

- [A2A Protocol Specification](https://github.com/google/A2A)
- [Google A2A Announcement](https://blog.google/technology/ai/agent-to-agent-protocol/)
- [Agent Card Specification](https://github.com/google/A2A/blob/main/specification.md#agent-card)