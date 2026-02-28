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

---

## 快速上手

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

---

## 架构总览

```
┌─────────────────────────────────────────────────────────┐
│                      用户 / 应用                          │
└────────────────────────┬────────────────────────────────┘
                         │ execute() / execute_stream()
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
