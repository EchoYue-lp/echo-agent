# echo-agent 文档

> **English docs** → [docs/en/README.md](../en/README.md)

---

## 中文文档

| 文档 | 功能模块 | 核心关键词 |
|------|---------|-----------|
| [01 - ReAct Agent](01-react-agent.md) | 核心执行引擎 | Thought→Action→Observation、CoT、并行工具调用、回调 |
| [02 - 工具系统](02-tools.md) | Tools | Tool trait、ToolManager、超时重试、并发限流 |
| [03 - 记忆系统](03-memory.md) | Memory | Store（长期）、Checkpointer（短期）、namespace 隔离 |
| [04 - 上下文压缩](04-compression.md) | Compression | SlidingWindow、Summary、Hybrid、ContextManager |
| [05 - 人工介入](05-human-loop.md) | Human-in-the-Loop | 审批 Guard、Console/Webhook/WebSocket Provider |
| [06 - 多 Agent 编排](06-subagent.md) | SubAgent / Orchestration | Orchestrator/Worker/Planner、上下文隔离 |
| [07 - Skill 系统](07-skills.md) | Skills | 能力包、系统提示词注入、外部 SKILL.md 加载 |
| [08 - MCP 协议](08-mcp.md) | MCP | stdio/HTTP 传输、工具适配、多服务端管理 |
| [09 - 任务规划](09-tasks.md) | Tasks / DAG | 有向无环图、拓扑排序、循环依赖检测、Mermaid 可视化 |
| [10 - 流式输出](10-streaming.md) | Streaming | execute_stream、AgentEvent、SSE、TTFT |
| [11 - 结构化输出](11-structured-output.md) | Structured Output | ResponseFormat、JsonSchema、extract()、extract_json() |
| [12 - Mock 测试工具](12-mock.md) | Testing | MockLlmClient、MockTool、MockAgent、InMemoryStore |
| [13 - 多轮对话](13-chat.md) | Chat | chat()、chat_stream()、跨轮记忆、reset() |

---

## 快速上手

### 单次任务模式（`execute`）

```rust
use echo_agent::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let config = AgentConfig::new("qwen3-max", "assistant", "你是一个有帮助的助手");
    let mut agent = ReactAgent::new(config);
    let answer = agent.execute("你好，介绍一下自己").await?;
    println!("{}", answer);
    Ok(())
}
```

### 多轮对话模式（`chat`）

`chat()` 在现有上下文上追加消息，天然支持多轮连续对话；
`execute()` 每次都会重置上下文，适合独立的单次任务。

```rust
use echo_agent::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let config = AgentConfig::new("qwen3-max", "assistant", "你是一个有帮助的助手");
    let mut agent = ReactAgent::new(config);

    let r1 = agent.chat("你好，我叫小明，是一名 Rust 程序员。").await?;
    println!("Agent: {r1}");

    let r2 = agent.chat("你还记得我的名字吗？").await?;
    println!("Agent: {r2}"); // Agent 能记住上下文中的"小明"

    agent.reset(); // 清除历史，开启新会话
    Ok(())
}
```

---

## 架构总览

```
┌─────────────────────────────────────────────────────────┐
│                      用户 / 应用                          │
└────────────────────────┬────────────────────────────────┘
                         │ execute() / execute_stream()   （单次任务，每次重置上下文）
                         │ chat()    / chat_stream()      （多轮对话，保留历史）
┌────────────────────────▼────────────────────────────────┐
│                    ReactAgent                            │
│                                                         │
│  ┌──────────────┐  ┌────────────┐  ┌─────────────────┐  │
│  │ContextManager│  │ToolManager │  │  SkillManager   │  │
│  │ (压缩/历史)  │  │(注册/执行) │  │ (Skill 元数据)  │  │
│  └──────────────┘  └────────────┘  └─────────────────┘  │
│                                                         │
│  ┌──────────────┐  ┌────────────┐  ┌─────────────────┐  │
│  │  Checkpointer│  │   Store    │  │HumanApprovalMgr │  │
│  │ (会话持久化) │  │(长期记忆)  │  │  (审批 Guard)   │  │
│  └──────────────┘  └────────────┘  └─────────────────┘  │
│                                                         │
│  ┌──────────────────────────────────────────────────┐   │
│  │              SubAgent 注册表                      │   │
│  └──────────────────────────────────────────────────┘   │
└────────────────────────┬────────────────────────────────┘
                         │ HTTP (OpenAI API)
┌────────────────────────▼────────────────────────────────┐
│                  LLM Provider                            │
└─────────────────────────────────────────────────────────┘
```

---

## 示例文件

| 示例 | 演示功能 |
|------|---------|
| `examples/demo01_tools.rs` | 基础工具注册与调用 |
| `examples/demo02_tasks.rs` | DAG 任务规划 |
| `examples/demo03_approval.rs` | 人工审批 |
| `examples/demo04_suagent.rs` | SubAgent 编排 |
| `examples/demo05_compressor.rs` | 上下文压缩 |
| `examples/demo06_mcp.rs` | MCP 协议集成 |
| `examples/demo07_skills.rs` | Skill 系统 |
| `examples/demo08_external_skills.rs` | 外部 SKILL.md 加载 |
| `examples/demo09_file_shell.rs` | 文件和 Shell 工具 |
| `examples/demo10_streaming.rs` | 流式输出 |
| `examples/demo11_callbacks.rs` | 生命周期回调 |
| `examples/demo12_resilience.rs` | 容错与重试 |
| `examples/demo13_tool_execution.rs` | 工具执行配置 |
| `examples/demo14_memory_isolation.rs` | 记忆隔离与上下文隔离 |
| `examples/demo15_structured_output.rs` | 结构化输出（extract / JSON Schema） |
| `examples/demo16_testing.rs` | Mock 测试基础设施（零真实 LLM 调用） |
| `examples/demo17_chat.rs` | 多轮对话（chat / chat_stream / reset） |
