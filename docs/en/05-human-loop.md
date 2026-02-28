# Human-in-the-Loop

## What It Is

Human-in-the-Loop (HIL) inserts human decision points into the Agent's automatic execution flow. Before performing a high-risk operation (deleting files, sending emails, making payments), the Agent pauses and requests human confirmation before proceeding.

echo-agent supports two intervention scenarios:

| Scenario | Description |
|----------|-------------|
| **Approval** | Show a y/n prompt before a tool executes; the user decides whether to allow it |
| **Input** | When the Agent needs additional information, request free-text input from the user |

---

## Problem It Solves

Fully autonomous Agents carry risks:
- No confirmation before irreversible operations (delete, send, charge)
- Acting on guesses when information is insufficient, rather than asking the user
- Production environments require audit trails (who approved what operation)

Human-in-the-Loop strikes the balance between automation efficiency and human safety.

---

## Three Built-in Providers

### ConsoleHumanLoopProvider (terminal, default)

```
🔔 Tool [delete_file] requires human approval
   Args: {"path": "/important/data.csv"}
   Approve? (y/n): _
```

### WebhookHumanLoopProvider (HTTP callback)

Sends the approval request to an external HTTP service and waits for a decision. Suitable for:
- Enterprise approval systems (Slack bots, DingTalk, WeChat Work)
- Forwarding approvals to external ticketing systems

```rust
use echo_agent::prelude::*;

let provider = WebhookHumanLoopProvider::new(
    "https://your-approval-service/approve",
    30, // timeout in seconds
);
agent.set_approval_provider(Arc::new(provider));
```

### WebSocketHumanLoopProvider (WebSocket push)

Starts a local WebSocket server and pushes approval requests to connected clients (frontend UI). Suitable for:
- Agent applications with a visual interface
- Mobile apps receiving approval notifications

```rust
use echo_agent::prelude::*;

let provider = WebSocketHumanLoopProvider::new("127.0.0.1:9000").await?;
agent.set_approval_provider(Arc::new(provider));
```

---

## Usage

### Tool Approval: `add_need_appeal_tool`

Mark a tool as "requires approval" — a human confirmation gate fires before execution:

```rust
use echo_agent::prelude::*;
use echo_agent::tools::shell::ShellTool;

let config = AgentConfig::new("gpt-4o", "agent", "You are a system administration assistant")
    .enable_tool(true)
    .enable_human_in_loop(true);

let mut agent = ReactAgent::new(config);

// The shell tool will pause and request approval before every execution
agent.add_need_appeal_tool(Box::new(ShellTool));

let answer = agent.execute("Delete all .log files in /tmp").await?;
```

---

### Free-text Input: `human_in_loop` tool

When the Agent needs more information, it proactively requests user input. The `HumanInLoop` tool is automatically registered when `enable_human_in_loop=true`:

```rust
let system = "When you need additional information to complete a task, \
              use the human_in_loop tool to ask the user.";

let config = AgentConfig::new("gpt-4o", "agent", system)
    .enable_tool(true)
    .enable_human_in_loop(true);

let mut agent = ReactAgent::new(config);
let answer = agent.execute("Book me a flight").await?;
// Agent calls human_in_loop("Which city are you flying to? What date?")
// Waits for user input in the terminal, then continues
```

---

## Custom Provider

Implement `HumanLoopProvider` to connect any approval system:

```rust
use echo_agent::prelude::*;
use async_trait::async_trait;

struct SlackApprovalProvider;

#[async_trait]
impl HumanLoopProvider for SlackApprovalProvider {
    async fn request(&self, req: HumanLoopRequest) -> echo_agent::error::Result<HumanLoopResponse> {
        // Post to Slack channel, wait for reaction or reply
        let approved = send_slack_and_wait(&req.prompt).await;
        if approved {
            Ok(HumanLoopResponse::Approved)
        } else {
            Ok(HumanLoopResponse::Rejected { reason: Some("Rejected by Slack user".to_string()) })
        }
    }
}
```

---

## Execution Flow

```
Agent about to execute tool "delete_file"
    │
    ├─ Check: HumanApprovalManager.needs_approval("delete_file")?
    │
    ├─ YES → call approval_provider.request(HumanLoopRequest::approval(...))
    │          │
    │          ├─ Console:   wait for y/n in terminal
    │          ├─ Webhook:   POST to external service, poll for result
    │          └─ WebSocket: push to client, wait for callback
    │
    ├─ Approved  → proceed with tool execution
    └─ Rejected  → rejection reason returned as tool result (LLM can adjust strategy)
       Timeout   → treated as rejection by default
```

See: `examples/demo03_approval.rs`
