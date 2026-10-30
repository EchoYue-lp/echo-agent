# ReAct Agent —— 核心执行引擎

## 是什么

ReAct（**Re**asoning + **Act**ing）是目前最主流的 Agent 执行范式。每一轮迭代分为三步：

```
Thought（推理）→ Action（调用工具）→ Observation（观测结果）
```

循环持续到完整原生响应形成最终候选，再经校验、Stop Hook、持久化与真实执行结算确认完成。

echo-agent 的核心实现是 `ReactAgent`，它将 ReAct 范式与工具管理、记忆、压缩、人工介入、Subagent 编排等能力全部集成在一个结构体中。

---

## 解决什么问题

纯粹的 LLM 调用是一次性的：给定输入，返回输出。这无法处理需要多步骤推理、外部工具访问、动态决策的复杂任务。

ReAct 范式解决了：
- **推理与行动分离**：LLM 先"思考"，再"行动"，再"观测"，可处理任意复杂度任务
- **工具调用**：让 LLM 能够执行代码、查询数据库、调用 API
- **迭代纠错**：工具返回错误时，LLM 可以调整策略重试
- **原生消息**：分别保留 provider reasoning 与普通 assistant 正文，不把普通文字推断为 thinking

---

## 执行流程

```
execute(task)
    │
    ├─ 1. 加载运行时状态（RuntimeStateStore）
    ├─ 2. 注入长期记忆（Store）
    │
    └─ Loop（max_iterations 次）:
          │
          ├─ context.prepare()    ← 自动压缩（若超 token_limit）
          │
          ├─ llm.chat()           ← 调用 LLM
          │
          ├─ 解析响应：
          │     ├─ assistant blocks → 按原 phase 分别保留 reasoning 与正文
          │     └─ tool_calls   → 工具调用列表
          │
          ├─ 并行执行所有工具调用：
          │     ├─ 人工审批检查（若该工具已标记）
          │     ├─ ToolManager.execute_tool()
          │     └─ 触发 on_tool_start_with_id / on_tool_end_with_id 回调
          │
          ├─ 原生最终候选 → 校验/Stop/持久化/结算后返回
          │
          └─ 将 assistant + tool_results 消息追加到上下文

    └─ 保存运行时状态（RuntimeStateStore）
```

---

## 可组合能力

Agent 不使用单独的角色状态机。普通 `ReactAgent` 启用 Subagent 调度并注册
Subagent 后即可承担编排职责；版本化的 `task_create`、`task_update` 和
`task_list` 工具提供规划能力，无需切换 Agent 运行时角色。

---

## 关键配置

```rust
AgentConfig::new("qwen3-max", "my_agent", "你是一个助手")
    .enable_tool(true)          // 启用工具调用（默认 true）
    .enable_subagent(true)      // 启用 Subagent 编排（Orchestrator 模式）
    .enable_memory(true)        // 启用已批准记忆的 Store 召回；manager 增加写入工具
    .enable_human_in_loop(true) // 启用人工介入
    .session_id("thread-001")   // run-grouping 标签（进程内）
    .conversation_id("conv-001")// 对话 ID，同时作为 RuntimeStateStore 的恢复键
    .token_limit(8192)          // 上下文 token 上限（超限自动压缩）
    .max_iterations(30)         // 最大迭代次数（防止死循环）
```

---

## Invocation 级工具面

调用方需要只对某一轮隐藏工具、又不能修改 pooled/shared agent 时，使用
`AgentInvocationContext::disabled_tools`：

```rust
use echo_agent::agent::AgentInvocationContext;
use std::collections::HashSet;

let invocation = AgentInvocationContext {
    disabled_tools: Some(HashSet::from(["create_complex_task".to_string()])),
    ..Default::default()
};
```

run snapshot 会把 invocation 排除项与 agent 默认排除项合并，再叠加已激活 skill 的
allowlist 和 plan mode 只读工具面，并在该 invocation 生命周期内冻结。隐藏工具既不会
出现在发给模型的 schema 中；即使 provider 仍返回该调用，执行 pipeline 也会拒绝。

`ReactAgent::set_disabled_tools` 现在只设置后续 run 的 agent 默认值，不会改变已经创建的
snapshot。

需要在多个 pooled Agent 之间共享带 revision 的策略时，使用 framework
`ToolControlService`。它只负责显式 disabled-tool 集合和单调 mutation revision；将
`ToolControlSnapshot::disabled_option()` 应用到各 Agent 或 invocation 即可。产品代码可以
额外做注册检查和 effective visibility 投影，但不应再创建第二个策略 authority。

### Run 预算

`RunBudgetPolicy` 提供默认关闭的收束控制，不改变既有 `max_iterations` 硬停止语义。
`iteration_wind_down_remaining` 在剩余迭代数达到阈值时只注入一次短提示；
`max_model_tokens` 只累计 provider 实际返回的 input/output token。provider 未返回 usage
时保持 unknown，不用估算值伪造精确阈值。达到 token 阈值后，下一次请求不暴露工具并发送
`tool_choice=none`。决策通过 `AgentEvent::BudgetDecision` 输出，同时写入 run trace。

消费方可以通过 `ReactAgentBuilder::run_budget` 设置 agent 默认值，也可以用
`AgentInvocationContext::run_budget` 覆盖单次调用。run snapshot 创建时冻结最终值，因此排队
invocation 不会互相修改预算。

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

    fn on_tool_start_with_id<'a>(&'a self, agent: &'a str, call_id: &'a str, tool: &'a str, args: &'a Value) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            println!("[{}] 调用工具: {} call_id={call_id} {:?}", agent, tool, args);
        })
    }

    fn on_tool_end_with_id<'a>(&'a self, agent: &'a str, call_id: &'a str, tool: &'a str, result: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let preview: String = result.chars().take(80).collect();
            println!("[{}] 工具结果: {} call_id={call_id} -> {}", agent, tool, preview);
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

#[echo_agent::callback]
impl LogCallback {
    async fn on_tool_start_with_id(&self, agent: &str, call_id: &str, tool: &str, args: &serde_json::Value) {
        println!("  [{}] 调用工具 {} call_id={call_id} args={}", agent, tool, args);
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

对应示例：`echo-agent-learning/examples/demo01_tools.rs`、`echo-agent-learning/examples/demo11_callbacks.rs`

---

## 关键设计细节

原生 reasoning 与 assistant 正文使用独立 typed 块；不再注册 Think/FinalAnswer 控制工具，也不注入工具前强制分析提示。普通文字不因同响应调用工具而成为 thinking；最终发布须经原 Guard/Critic/Stop、持久化与物理结算。这里的运行时结算不等于应用 TaskRun 或 Goal 已完成。块身份、来源、阶段、增量与运行时最后终态见[原生消息合同](../zh/43-native-message-execution.md)。

**并行工具调用**

当 LLM 在一次响应中返回多个工具调用时，ReactAgent 使用 `join_all()` 并行执行所有工具，受 `ToolExecutionConfig::max_concurrency` 约束。
