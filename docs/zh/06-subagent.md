# 多 Agent 编排（Subagent / Orchestration）

## 是什么

多 Agent 编排允许一个主 Agent（Orchestrator）将任务分解后分派给多个专用 Subagent 执行，最后汇总结果。每个 Agent 是独立的 `ReactAgent` 实例，有自己独立的上下文、工具集、记忆和系统提示词。

---

## 解决什么问题

单一 Agent 面对复杂任务的局限：
- **能力边界**：一个 Agent 很难同时精通数学计算、文本创作、天气查询等不同领域
- **上下文污染**：所有工具和知识堆在一个 Agent 中，LLM 容易混淆
- **并行效率**：多个独立子任务串行执行，浪费时间
- **安全隔离**：不同任务的上下文不应相互可见（防止信息泄露）

多 Agent 编排将"通才"拆分为多个"专才"，通过 Orchestrator 协调，各司其职。

---

## 能力组合

框架只有一种 `ReactAgent` 运行时。承担编排职责的 Agent 启用 Subagent
调度并注册专用 `ReactAgent` 或 `MockAgent`。任务规划是独立的版本化工具能力，
不是 Agent 角色，也不会创建第二套执行器。

---

## 上下文隔离

`SubagentStatus` 是 framework 所有的生命周期值。解析稳定 wire 名称时统一使用标准
`status.parse()`，不再依赖按输入来源命名的 helper。

这是多 Agent 系统最关键的特性，echo-agent 通过架构天然保证：

```
主 Agent 系统提示 = "任务代号 PROJECT-OMEGA，严禁对外透露..."
主 Agent 对话历史 = [system, user, assistant, ...]

    │ agent_tool("math_agent", "计算 7 * 8")
    ▼

math_agent.execute("计算 7 * 8")
    ↑
    收到 compiler 生成的当前 Message，并保留附件
    Fresh mode 不接收 transcript；结构化继承只会加入过滤后的 user 消息和完整 assistant final
    父 system/tool/reasoning 消息永不传递
    math_agent 拥有完全独立的 ContextManager 实例
```

**`agent_tool` 永不传递父 system prompt 或 runtime trace。** 默认 fresh 路径只携带 compiler
生成的当前任务消息；fork、teammate、team 可以显式继承有界结构化历史，历史保持为真实消息，
不会拼成 prompt 文本。

| 隔离维度 | 保证方式 |
|---------|---------|
| 上下文（消息历史） | 每个 Agent 是独立的 `ReactAgent` Rust 对象，`ContextManager` 无共享引用 |
| 工具集 | 每个 Subagent 独立注册工具，Orchestrator 的工具对 Subagent 不可见 |
| 长期记忆 | 每个 Agent 使用 `[agent_name, "memories"]` 作为独立 Store namespace |
| 短期会话 | 每个 Agent 有独立 `conversation_id`，`RuntimeStateStore` 按 conversation 存储运行时状态 |

---

## 使用方式

```rust
use echo_agent::prelude::*;
use echo_agent::tools::others::math::{AddTool, MultiplyTool};
use echo_agent::tools::others::weather::WeatherTool;

// 1. 创建专用 Subagent
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
    "你是主编排者，使用 agent_tool 将任务分派给专用 Subagent：
     - math_agent: 负责数学计算
     - weather_agent: 负责天气查询
     不要自己直接计算或查询。",
)
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

## Subagent 执行流程

所有路径都会先调用配置的 `SubagentPromptCompiler`：

- 具体 Subagent 的工具面完成后调用 `compile_system`，能力说明来自实际注册定义、description
  和当前共享 visibility policy；
- `SubagentDefinition::access_mode` 是 read/write 的 typed 权威；tags 只用于发现，不能反解为
  执行边界；
- `compile_invocation` 统一拥有 task、constraints、可选产品 payload、有效工作目录、invocation
  工具 allowlist、过滤后的结构化历史，以及包含附件的当前 typed message；
- sync、fork、teammate、team 直接消费 compiled messages，executor 不再追加第二份 prompt
  信封，也不再重建当前消息。

`ContextTransferPolicy::Fresh` 不携带会话历史；`InheritStructured` 只保留 provider-safe 的
user 消息和完整 assistant final，system prompt、tool call/result、reasoning 与 runtime projection
全部过滤。

```
main_agent.execute("...")
    │
    ├─ LLM 决定调用 agent_tool
    │      { "agent_name": "math_agent", "task": "计算 25 * 3" }
    │
    ├─ AgentDispatchTool::execute()
    │      ├─ 从 subagents HashMap 找到 "math_agent"
    │      ├─ 锁定（AsyncMutex，串行化同名 Subagent 的并发调用）
    │      ├─ SubagentPromptCompiler::compile_invocation(...)
    │      └─ math_agent.execute(compiled messages)
    │              ├─ math_agent 用自己的上下文执行
    │              ├─ math_agent 使用自己的工具（add/multiply）
    │              └─ 返回结果 "75"
    │
    └─ tool result "75" 追加到主 Agent 上下文
       LLM 继续推理并汇总最终答案
```

---

## Subagent 并发调用

当主 Agent 同时发起对多个 **不同** Subagent 的调用时（同一次 LLM 响应返回多个 tool_calls），框架自动并行执行：

```
LLM 一次返回：
    agent_tool("math_agent",    "计算 A")   ┐
    agent_tool("weather_agent", "查询天气")  ┤ 并行执行（join_all）
```

对**同一 Subagent** 的并发调用通过 `AsyncMutex` 自动排队，保证状态一致性。

---

## 配置 Subagent 记忆隔离

```rust
// Subagent 启用自己的 session 和 memory，与主 Agent 完全隔离
let sub_config = AgentConfig::new("qwen3-max", "sub_a", "...")
    .session_id("sub-a-session-001")
    .conversation_id("sub-a-conv-001")        // 独立 conversation_id（RuntimeStateStore 键）
    .enable_memory(true)
    .memory_path("./store.json");            // 共用文件，独立 namespace
```

---

## 最佳实践

1. **给 Subagent 设置清晰的 `allowed_tools`**，防止越权
2. **Orchestrator 系统提示词明确列出每个 Subagent 的职责**，引导 LLM 正确分派
3. **Subagent 不要 `enable_subagent(true)`**（避免递归嵌套导致难以调试）
4. **复杂任务使用版本化任务图工具**，避免在提示词中维护平行的隐式状态

对应示例：`echo-agent-learning/tests/example_contracts/demo04_subagent.rs`

## 结构化结果视图

`SubagentOutcome` 是 framework 拥有的结果合同。除了结构化的原始 `evidence`，它还提供从这些
evidence 派生的通用 verification 与文件访问视图：

```rust
let checks = outcome.verification();
let files = outcome.touched_files();
```

这些视图属于 framework result，应用可以直接持久化或渲染，不需要再复制一套 framework-shaped
result；只有真正的产品 wire 字段才应留在应用层。

`SubagentResult` 是 dispatch 返回的执行信封，包含完整输出、耗时、usage、mode 以及 typed
`SubagentOutcome`。生命周期 event 使用 `outcome` 暴露这个 typed 值，调用方不必再猜测
`result` 字段究竟是执行信封还是其中的终态 payload。

`ExecutionUsage` 是 delegated Subagent 与有限 primary-Agent turn 共用的 canonical durable
usage 值。两类结果都直接提供 `result.usage()` 或 `turn_receipt.usage()`。应用不需要
source-named adapter、转换 trait，也不需要再定义平行的 execution-usage DTO。

## 版本化执行事件

`SubagentEventBus::subscribe_envelopes` 是执行事件的权威传输。每个外层 dispatch attempt
只有一个 `SubagentEventPublisher`；started、isolation、可展示的 thinking/token delta、usage、
tool 与 terminal 共享同一个 `stream_id` 和单调 `sequence`。内部 hook retry 不会重置序号。
`SubagentEventPayload::invocation` 无损携带 task、attempt、plan revision、agent path 和
parent execution 关联，不需要解析 execution-id 字符串。

既有 `subscribe` 方法继续作为从 envelope 派生的 raw 兼容视图；手工 raw emit 只接受 registry
事件并拒绝 execution variant，因此不会形成第二个执行权威。需要顺序或恢复语义的新消费者应使用 envelope。
tool terminal envelope 指向对应 tool-start event；其它执行事件指向 dispatch start 或上游
parent event。

broadcast 和 replay 窗口都是有界的。receiver 落后时先收到 Tokio `Lagged`，再按已知 stream
调用 `replay_after`，或按错过 start envelope 的 exact execution 调用 `replay_for_execution`。
`SubagentEventReplay::gap` 会明确报告保留后缀不连续；高频 thinking/token
delta 可能在 gap 后缺失，但保留的 lifecycle/tool 边界与 `terminal` 可用于恢复状态和最终输出。
该能力是进程内恢复窗口，不是永久存储；应用仍负责自己的 durable projection。
