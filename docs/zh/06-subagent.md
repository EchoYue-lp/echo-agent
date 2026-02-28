# 多 Agent 编排（SubAgent / Orchestration）

## 是什么

多 Agent 编排允许一个主 Agent（Orchestrator）将任务分解后分派给多个专用 SubAgent 执行，最后汇总结果。每个 Agent 是独立的 `ReactAgent` 实例，有自己独立的上下文、工具集、记忆和系统提示词。

---

## 解决什么问题

单一 Agent 面对复杂任务的局限：
- **能力边界**：一个 Agent 很难同时精通数学计算、文本创作、天气查询等不同领域
- **上下文污染**：所有工具和知识堆在一个 Agent 中，LLM 容易混淆
- **并行效率**：多个独立子任务串行执行，浪费时间
- **安全隔离**：不同任务的上下文不应相互可见（防止信息泄露）

多 Agent 编排将"通才"拆分为多个"专才"，通过 Orchestrator 协调，各司其职。

---

## 三种 Agent 角色

```rust
AgentConfig::new(...).role(AgentRole::Orchestrator) // 编排者
AgentConfig::new(...).role(AgentRole::Worker)        // 执行者（默认）
AgentConfig::new(...).role(AgentRole::Planner)       // 任务规划者
```

| 角色 | 行为 |
|------|------|
| `Orchestrator` | 接收用户任务 → 拆解 → 通过 `agent_tool` 分派给 SubAgent → 汇总 |
| `Worker` | 接收具体任务 → 使用自己的工具集执行 → 返回结果 |
| `Planner` | 接收复杂任务 → 先用 `plan` 工具生成 DAG 子任务 → 逐步执行 |

---

## 上下文隔离

这是多 Agent 系统最关键的特性，echo-agent 通过架构天然保证：

```
主 Agent 系统提示 = "任务代号 PROJECT-OMEGA，严禁对外透露..."
主 Agent 对话历史 = [system, user, assistant, ...]

    │ agent_tool("math_agent", "计算 7 * 8")
    ▼

math_agent.execute("计算 7 * 8")
    ↑
    只收到这个字符串，不知道 PROJECT-OMEGA
    math_agent 拥有完全独立的 ContextManager 实例
```

**agent_tool 只传递任务字符串，不传递任何上下文。**

| 隔离维度 | 保证方式 |
|---------|---------|
| 上下文（消息历史） | 每个 Agent 是独立的 `ReactAgent` Rust 对象，`ContextManager` 无共享引用 |
| 工具集 | 每个 SubAgent 独立注册工具，Orchestrator 的工具对 SubAgent 不可见 |
| 长期记忆 | 每个 Agent 使用 `[agent_name, "memories"]` 作为独立 Store namespace |
| 短期会话 | 每个 Agent 有独立 `session_id`，Checkpointer 按 session 存储 |

---

## 使用方式

```rust
use echo_agent::prelude::*;
use echo_agent::tools::others::math::{AddTool, MultiplyTool};
use echo_agent::tools::others::weather::WeatherTool;

// 1. 创建专用 SubAgent
let math_agent = {
    let config = AgentConfig::new("qwen3-max", "math_agent", "你是数学计算专家")
        .enable_tool(true)
        .allowed_tools(vec!["add".into(), "multiply".into()]); // 限制工具边界
    let mut agent = ReactAgent::new(config);
    agent.add_tools(vec![Box::new(AddTool), Box::new(MultiplyTool)]);
    Box::new(agent) as Box<dyn Agent>
};

let weather_agent = {
    let config = AgentConfig::new("qwen3-max", "weather_agent", "你是天气查询专家")
        .enable_tool(true)
        .allowed_tools(vec!["get_weather".into()]);
    let mut agent = ReactAgent::new(config);
    agent.add_tool(Box::new(WeatherTool));
    Box::new(agent) as Box<dyn Agent>
};

// 2. 创建主编排 Agent
let main_config = AgentConfig::new(
    "qwen3-max",
    "orchestrator",
    "你是主编排者，使用 agent_tool 将任务分派给专用 SubAgent：
     - math_agent: 负责数学计算
     - weather_agent: 负责天气查询
     不要自己直接计算或查询。",
)
.role(AgentRole::Orchestrator)
.enable_subagent(true)
.enable_tool(true);

let mut main_agent = ReactAgent::new(main_config);
main_agent.register_agents(vec![math_agent, weather_agent]);

// 3. 执行任务
let result = main_agent
    .execute("今天北京天气如何？如果气温超过 20 度，计算 (20 + 5) * 3")
    .await?;
println!("{}", result);
```

---

## SubAgent 执行流程

```
main_agent.execute("...")
    │
    ├─ LLM 决定调用 agent_tool
    │      { "agent_name": "math_agent", "task": "计算 25 * 3" }
    │
    ├─ AgentDispatchTool::execute()
    │      ├─ 从 subagents HashMap 找到 "math_agent"
    │      ├─ 锁定（AsyncMutex，串行化同名 SubAgent 的并发调用）
    │      └─ math_agent.execute("计算 25 * 3")
    │              ├─ math_agent 用自己的上下文执行
    │              ├─ math_agent 使用自己的工具（add/multiply）
    │              └─ 返回结果 "75"
    │
    └─ tool result "75" 追加到主 Agent 上下文
       LLM 继续推理并汇总最终答案
```

---

## SubAgent 并发调用

当主 Agent 同时发起对多个 **不同** SubAgent 的调用时（同一次 LLM 响应返回多个 tool_calls），框架自动并行执行：

```
LLM 一次返回：
    agent_tool("math_agent",    "计算 A")   ┐
    agent_tool("weather_agent", "查询天气")  ┤ 并行执行（join_all）
```

对**同一 SubAgent** 的并发调用通过 `AsyncMutex` 自动排队，保证状态一致性。

---

## 配置 SubAgent 记忆隔离

```rust
// SubAgent 启用自己的 session 和 memory，与主 Agent 完全隔离
let sub_config = AgentConfig::new("qwen3-max", "sub_a", "...")
    .session_id("sub-a-session-001")
    .checkpointer_path("./checkpoints.json") // 共用文件，独立 session_id
    .enable_memory(true)
    .memory_path("./store.json");            // 共用文件，独立 namespace
```

---

## 最佳实践

1. **给 SubAgent 设置清晰的 `allowed_tools`**，防止越权
2. **Orchestrator 系统提示词明确列出每个 SubAgent 的职责**，引导 LLM 正确分派
3. **SubAgent 不要 `enable_subagent(true)`**（避免递归嵌套导致难以调试）
4. **复杂任务用 Planner 角色配合 DAG 任务系统**，而不是依赖 Orchestrator 临时决策

对应示例：`examples/demo04_suagent.rs`、`examples/demo14_memory_isolation.rs`
