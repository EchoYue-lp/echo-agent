# Getting Started

This tutorial walks you through building a working AI Agent with echo-agent from scratch — in under 10 minutes.

## Prerequisites

- **Rust 1.95+** (2024 edition)
- **cargo** (ships with Rust)
- An LLM API key (OpenAI, DeepSeek, Qwen, Anthropic, Ollama, etc.)

```bash
# Check your Rust version
rustc --version   # should print rustc 1.95.x or later

# Install or upgrade if needed
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Set your API key (pick one):

```bash
export OPENAI_API_KEY=sk-xxx         # OpenAI
export DEEPSEEK_API_KEY=sk-xxx       # DeepSeek
export DASHSCOPE_API_KEY=sk-xxx      # Alibaba Qwen
export ANTHROPIC_API_KEY=sk-ant-xxx  # Anthropic
```

---

## Step 1: Create a project

```bash
cargo new my-agent
cd my-agent
```

Edit `Cargo.toml` and add dependencies:

```toml
[dependencies]
echo-agent = "0.2.0"
tokio = { version = "1", features = ["full"] }
```

---

## Step 2: Minimal Agent (no tools)

Edit `src/main.rs`:

```rust
use echo_agent::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let agent = ReactAgentBuilder::simple("deepseek-v4-flash", "You are a helpful assistant")?;

    let answer = agent.execute("What is Rust's ownership model?").await?;
    println!("{answer}");
    Ok(())
}
```

Run it:

```bash
cargo run
```

This is the simplest possible agent — no tools, just the LLM answering directly.

---

## Step 3: Add custom tools

The real power of agents comes from tool use. The `#[tool]` macro defines tools as async functions and auto-generates JSON Schema:

```rust
use echo_agent::prelude::*;
use echo_agent::{agent, tool};

#[tool(name = "add", description = "Add two numbers")]
async fn add(a: f64, b: f64) -> Result<ToolResult> {
    Ok(ToolResult::success(format!("{}", a + b)))
}

#[tool(name = "multiply", description = "Multiply two numbers")]
async fn multiply(a: f64, b: f64) -> Result<ToolResult> {
    Ok(ToolResult::success(format!("{}", a * b)))
}

#[tokio::main]
async fn main() -> Result<()> {
    let agent = agent! {
        model: "deepseek-v4-flash",
        system_prompt: "You are a math assistant. Use tools to calculate.",
        tools: [AddTool, MultiplyTool],
    }?;

    let answer = agent.execute("Calculate (3 + 4) * 5").await?;
    println!("Answer: {answer}");
    Ok(())
}
```

When you run this, the agent will reason through the problem: first call `add(3, 4)` to get 7, then `multiply(7, 5)` to get 35.

```bash
cargo run
```

---

## Step 4: Understand the execution loop

Under the hood, the agent runs a ReAct loop:

```
User input: "Calculate (3 + 4) * 5"
  │
  ├─ Thought: I need to compute 3+4 first
  ├─ Action:  add(3, 4) → 7
  │
  ├─ Thought: Now multiply by 5
  ├─ Action:  multiply(7, 5) → 35
  │
  └─ Final Answer: (3 + 4) * 5 = 35
```

Each iteration is **Think → Act → Observe**, repeating until the LLM decides the task is complete.

---

## Step 5: Add memory

Give your agent cross-session memory with a single line:

```rust
use echo_agent::prelude::*;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<()> {
    let store = Arc::new(InMemoryStore::new());

    let agent = ReactAgentBuilder::new()
        .model("deepseek-v4-flash")
        .system_prompt("You are an assistant that can remember information the user tells you.")
        .with_memory_tools(store)  // registers remember / recall / forget tools
        .build()?;

    // The agent can now remember and recall information
    let answer = agent.execute("Please remember that my name is Alice").await?;
    println!("{answer}");
    Ok(())
}
```

Want persistence to disk? Swap `InMemoryStore` for `FileStore` or `SqliteStore` (requires the `sqlite` feature).

---

## Step 6: Streaming output

Get real-time token-by-token output — ideal for building chat interfaces:

```rust
use echo_agent::prelude::*;
use futures::StreamExt;

#[tokio::main]
async fn main() -> Result<()> {
    let agent = ReactAgentBuilder::new()
        .model("deepseek-v4-flash")
        .system_prompt("You are a helpful assistant")
        .build()?;

    let mut stream = agent.execute_stream("Explain quantum computing").await?;
    while let Some(event) = stream.next().await {
        match event? {
            AgentEvent::Token(t) => print!("{t}"),
            AgentEvent::FinalAnswer(a) => { println!("\n{a}"); break; }
            _ => {}
        }
    }
    Ok(())
}
```

Add the extra dependency:

```toml
[dependencies]
futures = "0.3"
```

---

## Next steps

You've built your first agent with tools! Here's where to go deeper:

| Topic | Doc |
|-------|-----|
| ReAct engine in depth | [01-react-agent.md](./01-react-agent.md) |
| Tool system | [02-tools.md](./02-tools.md) |
| Memory system | [03-memory.md](./03-memory.md) |
| Streaming | [10-streaming.md](./10-streaming.md) |
| Context compression | [04-compression.md](./04-compression.md) |
| MCP protocol | [08-mcp.md](./08-mcp.md) |
| Multi-agent orchestration | [26-multi-agent.md](./26-multi-agent.md) |

Or run the examples from the repo directly:

```bash
cargo run --example demo01_tools          # Custom tools
cargo run --example demo10_streaming      # Streaming output
cargo run --example demo04_subagent       # Multi-agent orchestration
cargo run --example demo06_mcp            # MCP protocol integration
```

See the full list of 52 demos in the [examples/](../examples/) directory.
