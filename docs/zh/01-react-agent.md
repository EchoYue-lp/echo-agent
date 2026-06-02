# ReAct Agent —— 核心执行引擎

## 是什么

ReAct（**Re**asoning + **Act**ing）是目前最主流的 Agent 执行范式。每一轮迭代分为三步：

```
Thought（推理）→ Action（调用工具）→ Observation（观测结果）
```

循环执行，直到 LLM 认为任务已完成并调用 `final_answer` 工具输出结果。

echo-agent 的核心实现是 `ReactAgent`，它将 ReAct 范式与工具管理、记忆、压缩、人工介入、SubAgent 编排等能力全部集成在一个结构体中。

---

## 解决什么问题

纯粹的 LLM 调用是一次性的：给定输入，返回输出。这无法处理需要多步骤推理、外部工具访问、动态决策的复杂任务。

ReAct 范式解决了：
- **推理与行动分离**：LLM 先"思考"，再"行动"，再"观测"，可处理任意复杂度任务
- **工具调用**：让 LLM 能够执行代码、查询数据库、调用 API
- **迭代纠错**：工具返回错误时，LLM 可以调整策略重试
- **Chain-of-Thought**：自然产生可追溯的推理链，便于调试

---

## 执行流程

```
execute(task)
    │
    ├─ 1. 加载会话历史（Checkpointer）
    ├─ 2. 注入长期记忆（Store）
    │
    └─ Loop（max_iterations 次）:
          │
          ├─ context.prepare()    ← 自动压缩（若超 token_limit）
          │
          ├─ llm.chat()           ← 调用 LLM
          │
          ├─ 解析响应：
          │     ├─ content 非空  → Token 事件（流式 / CoT 推理文本）
          │     └─ tool_calls   → 工具调用列表
          │
          ├─ 并行执行所有工具调用：
          │     ├─ 人工审批检查（若该工具已标记）
          │     ├─ ToolManager.execute_tool()
          │     └─ 触发 on_tool_start / on_tool_end 回调
          │
          ├─ 调用 final_answer → 返回结果，退出循环
          │
          └─ 将 assistant + tool_results 消息追加到上下文

    └─ 保存会话历史（Checkpointer）
```

---

## Agent 角色

`AgentRole` 控制 Agent 的执行模式：

| 角色 | 说明 |
|------|------|
| `Worker`（默认） | 直接执行任务，使用工具 |
| `Orchestrator` | 编排者，优先通过 `agent_tool` 将任务分发给 SubAgent |

> **注意：** 任务规划能力通过 `.enable_task(true)` 启用（注册 `plan`/`create_task`/`update_task` 工具），无需单独的 `Planner` 角色。

---

## 关键配置

```rust
AgentConfig::new("qwen3-max", "my_agent", "你是一个助手")
    .enable_tool(true)          // 启用工具调用（默认 true）
    .enable_task(true)          // 启用 DAG 任务规划
    .enable_subagent(true)      // 启用 SubAgent 编排（Orchestrator 模式）
    .enable_memory(true)        // 启用长期记忆（Store + remember/recall/forget 工具）
    .enable_human_in_loop(true) // 启用人工介入
    .enable_cot(true)           // 启用 Chain-of-Thought 引导语（Builder 默认 true）
    .session_id("thread-001")   // 线程 ID：用于 Checkpointer 恢复/续接
    .conversation_id("conv-001")// 可选：用于 transcript/history 投影
    .token_limit(8192)          // 上下文 token 上限（超限自动压缩）
    .max_iterations(30)         // 最大迭代次数（防止死循环）
```

---

## 生命周期回调

实现 `AgentCallback` trait，可以监听 Agent 执行的每个阶段（用于埋点、日志、UI 实时更新等）：

```rust
use echo_agent::agent::{AgentCallback, AgentEvent};
use echo_agent::llm::types::Message;
use futures::future::BoxFuture;
use serde_json::Value;

struct MyCallback;

impl AgentCallback for MyCallback {
    fn on_think_start<'a>(&'a self, agent: &'a str, messages: &'a [Message]) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            println!("[{}] 开始推理，上下文 {} 条消息", agent, messages.len());
        })
    }

    fn on_tool_start<'a>(&'a self, agent: &'a str, tool: &'a str, args: &'a Value) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            println!("[{}] 调用工具: {} {:?}", agent, tool, args);
        })
    }

    fn on_tool_end<'a>(&'a self, agent: &'a str, tool: &'a str, result: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            println!("[{}] 工具结果: {} -> {}", agent, tool, &result[..result.len().min(80)]);
        })
    }

    fn on_final_answer<'a>(&'a self, agent: &'a str, answer: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            println!("[{}] 最终答案: {}", agent, answer);
        })
    }
}
```

---

## 最简 Demo

```rust
use echo_agent::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let config = AgentConfig::new("qwen3-max", "assistant", "你是一个有帮助的助手");
    let mut agent = ReactAgent::new(config);

    let answer = agent.execute("1 + 1 等于几？").await?;
    println!("{}", answer);
    Ok(())
}
```

---

## 完整 Demo（带工具 + 回调）

```rust
use echo_agent::prelude::*;
use echo_agent::tools::others::math::{AddTool, MultiplyTool};
use std::sync::Arc;

struct LogCallback;

#[async_trait::async_trait]
impl AgentCallback for LogCallback {
    async fn on_tool_start(&self, agent: &str, tool: &str, args: &serde_json::Value) {
        println!("  [{}] 调用工具 {} args={}", agent, tool, args);
    }
    async fn on_final_answer(&self, _agent: &str, answer: &str) {
        println!("最终答案: {}", answer);
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let config = AgentConfig::new(
        "qwen3-max",
        "math_agent",
        "你是一个数学助手，使用工具进行计算。",
    )
    .enable_tool(true)
    .max_iterations(10);

    let mut agent = ReactAgent::new(config);
    agent.add_tools(vec![Box::new(AddTool), Box::new(MultiplyTool)]);
    agent.add_callback(Arc::new(LogCallback));

    let answer = agent
        .execute("计算 (3 + 4) * 5 等于多少？")
        .await?;
    println!("{}", answer);
    Ok(())
}
```

对应示例：`examples/demo01_tools.rs`、`examples/demo11_callbacks.rs`

---

## 关键设计细节

**为什么不用 `think` 工具而用 CoT 文本？**

旧方案是专门提供一个 `think` 工具让 LLM "思考"。新方案是在系统提示词末尾追加 `COT_INSTRUCTION`，让 LLM 在每次工具调用前在 `content` 字段输出推理文本。好处是：
1. 推理内容天然进入消息历史（context）
2. 直接产生流式 Token 事件，UI 可实时展示思考过程
3. 减少一次无意义的工具调用

**并行工具调用**

当 LLM 在一次响应中返回多个工具调用时，ReactAgent 使用 `join_all()` 并行执行所有工具，受 `ToolExecutionConfig::max_concurrency` 约束。
