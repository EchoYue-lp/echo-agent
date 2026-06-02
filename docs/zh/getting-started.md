# 快速上手

本教程将带你用 echo-agent 从零构建一个带工具的 AI Agent，全程不超过 10 分钟。

## 前置条件

- **Rust 1.95+**（2024 edition）
- **cargo**（随 Rust 一起安装）
- 一个 LLM API Key（支持 OpenAI、DeepSeek、Qwen、Anthropic、Ollama 等）

```bash
# 确认 Rust 版本
rustc --version   # 应输出 rustc 1.95.x 或更高

# 若需安装或升级
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

设置你的 API Key（任选其一）：

```bash
export OPENAI_API_KEY=sk-xxx         # OpenAI
export DEEPSEEK_API_KEY=sk-xxx       # DeepSeek
export DASHSCOPE_API_KEY=sk-xxx      # 阿里通义千问
export ANTHROPIC_API_KEY=sk-ant-xxx  # Anthropic
```

---

## 第一步：创建项目

```bash
cargo new my-agent
cd my-agent
```

编辑 `Cargo.toml`，添加依赖：

```toml
[dependencies]
echo-agent = "0.2.0"
tokio = { version = "1", features = ["full"] }
```

---

## 第二步：最小 Agent（无工具）

编辑 `src/main.rs`：

```rust
use echo_agent::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let agent = ReactAgentBuilder::simple("deepseek-chat", "你是一个有帮助的助手")?;

    let answer = agent.execute("Rust 的所有权机制是什么？").await?;
    println!("{answer}");
    Ok(())
}
```

运行：

```bash
cargo run
```

这就是一个最基础的对话 Agent——没有工具，纯靠 LLM 回答。

---

## 第三步：添加自定义工具

Agent 的真正威力来自工具调用。用 `#[tool]` 宏定义工具，一行注解自动生成 JSON Schema：

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
        model: "deepseek-chat",
        system_prompt: "你是一个数学助手，使用工具进行计算。",
        tools: [AddTool, MultiplyTool],
    }?;

    let answer = agent.execute("计算 (3 + 4) * 5").await?;
    println!("答案: {answer}");
    Ok(())
}
```

运行后，Agent 会自动推理：先调用 `add(3, 4)` 得到 7，再调用 `multiply(7, 5)` 得到 35。

```bash
cargo run
```

---

## 第四步：理解执行流程

Agent 内部运行的是 ReAct 循环：

```
用户输入: "计算 (3 + 4) * 5"
  │
  ├─ Thought: 需要先算 3+4
  ├─ Action:  add(3, 4) → 7
  │
  ├─ Thought: 再乘以 5
  ├─ Action:  multiply(7, 5) → 35
  │
  └─ Final Answer: (3 + 4) * 5 = 35
```

每一轮都是 **思考 → 行动 → 观测**，直到 LLM 判断任务完成。

---

## 第五步：添加记忆

让 Agent 跨会话记住信息，只需一行代码：

```rust
use echo_agent::prelude::*;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<()> {
    let store = Arc::new(InMemoryStore::new());

    let agent = ReactAgentBuilder::new()
        .model("deepseek-chat")
        .system_prompt("你是一个助手，可以记住用户告诉你的信息。")
        .with_memory_tools(store)  // 自动注册 remember / recall / forget 工具
        .build()?;

    // Agent 现在可以记住和回忆信息了
    let answer = agent.execute("请记住我的名字叫小明").await?;
    println!("{answer}");
    Ok(())
}
```

需要持久化到磁盘？把 `InMemoryStore` 换成 `FileStore` 或 `SqliteStore`（需要 `sqlite` feature）。

---

## 第六步：流式输出

实时逐 token 输出，适合构建聊天界面：

```rust
use echo_agent::prelude::*;
use futures::StreamExt;

#[tokio::main]
async fn main() -> Result<()> {
    let agent = ReactAgentBuilder::new()
        .model("deepseek-chat")
        .system_prompt("你是一个有帮助的助手")
        .build()?;

    let mut stream = agent.execute_stream("解释量子计算").await?;
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

需要额外依赖：

```toml
[dependencies]
futures = "0.3"
```

---

## 下一步

恭喜你完成了第一个带工具的 AI Agent！以下是深入学习的方向：

| 主题 | 文档 |
|------|------|
| ReAct 引擎详解 | [01-react-agent.md](./01-react-agent.md) |
| 工具系统 | [02-tool-system.md](./02-tool-system.md) |
| 记忆系统 | [03-memory.md](./03-memory.md) |
| 流式输出 | [04-streaming.md](./04-streaming.md) |
| 上下文压缩 | [05-context-compression.md](./05-context-compression.md) |
| MCP 协议 | [06-mcp.md](./06-mcp.md) |
| 多 Agent 编排 | [07-multi-agent.md](./07-multi-agent.md) |

也可以直接运行仓库中的示例：

```bash
cargo run --example demo01_tools          # 自定义工具
cargo run --example demo10_streaming      # 流式输出
cargo run --example demo04_subagent       # 多 Agent 编排
cargo run --example demo06_mcp            # MCP 协议集成
```

完整示例列表见 [examples/](../examples/) 目录。
