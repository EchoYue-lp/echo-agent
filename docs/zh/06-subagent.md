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

### 核准实际注册记录的宿主准入

原 registry 支持可失败的宿主检查，在用户 factory、模型或后台子执行开始前核准
真正待消费的 definition／executable 配对。它与返回 `()` 的生命周期观察 hook
分开。在向调用方发布 registry 前配置：

```rust,ignore
#[async_trait::async_trait]
impl SubagentRegistrationAdmission for HostAdmission {
    async fn admit(
        &self,
        registration: &SubagentRegistration,
        request: &DispatchRequest,
    ) -> echo_agent::error::Result<()> {
        self.check_actual_role(registration.definition(), request.runtime_context.as_ref())
    }
}
registry.set_registration_admission(Some(Arc::new(host_admission))).await;
```

回调直接取得原 invocation 的 run context 和 resource guards。应用自行解释
provider、内容版本与任务绑定，框架不从名字或 revision 推断持久版本。不配置策略时，
仍允许原动态注册和普通 General 自扩展，不新增 GUI 权限或 HITL 闸。

`resolve_registration(name)` 不构造 Agent。需要按 definition 决定路由时，
将同一 handle 传入 `dispatch_registered` 或 `dispatch_background_registered`；
它们仍核准宿主策略，并拒绝其它 registry／目标的 handle。原按名字派发入口也统一
解析和核准一次。Sync、Fork、Teammate、Team 的实际成员以及重试／转派都消费自己
真正选中的记录。后台首轮在 spawn／返回 handle 前核准，进入原 owner 后不重复
核准这一轮；后继重试重新核准新记录。

同名替换或移除只影响后续选择。在途调用继续使用原 factory／实例，不将结果填入
替换记录的缓存。`revision()` 只作单 registry 内的诊断，
`is_same_registration` 比较实际进程内记录；两者均不表示持久插件内容或版本。
原 `get_agent`／`create_fresh_agent` 是原始构造接口，不能替代带准入的派发。
详见 [ADR 0097](../adr/0097-subagent-registration-admission.md) 和可执行示例
`demo50_subagent_communication`。

### 组合并执行 Agent

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

### Invocation 工具范围

非空 `SubagentContext.allowed_tools` 是实际 invocation 的硬上界，与 Fresh
或对话历史继承无关。Sync、Fork、Teammate 和具体 Team member 同时在模型 schema
与原工具执行 pipeline 中落实该范围。`None` 和 `Some([])` 继续表示没有附加白名单；
非空上界在新角色中零匹配时，仍然禁用该角色全部工具。

`agent_tool` 沿原 context 传递调用方 `ToolContext` 的硬 **available** 集合，
不把共享注册表或较小的首轮 visible 集合当作祖先许可。后代只能收窄；更换角色、
Fresh 模式或工具搜索激活都不能恢复祖先排除的工具。越界的缓存调用在创建 child
前拒绝。没有这类 context 的独立 framework 调用保持原行为；层数配置与运行政策
继续由应用负责，见 [ADR 0094](../adr/0094-subagent-tool-boundary-inheritance.md)。

程序调用可在 `AgentInvocationContext.allowed_tools: Option<HashSet<String>>`
携带同一个非空正向上界。Agent 取得原执行锁之后，才与当时的注册目录求交；排队
期间新注册的工具不能扩大该 invocation。`visible_tools` 仅选择硬范围内首轮显示的
schema；`allowed_tools` 缺省或空集合仍表示无附加白名单。穷尽 Rust invocation
字面量需要补该可选字段，或使用 `..Default::default()`。

每次具体 Subagent invocation 还携带请求中已确定的**目标 actor**委派 policy。
Executor 只复制该值，不再次推进层数；真实派发边独占 `child_policy`。显式不能
委派或已到上限的 actor，通过原 available/eligible 资格统一排除模型 schema、
搜索和真实执行中的 `agent_tool`。`None` 保留框架原 fallback，具体层数政策由应用
选择。独立 focused 批次已通过真实模型请求和 ToolContext 中的目标 policy 回归，
包含通用 maximum 4 对照；该框架结果不代表应用 GUI 配置、run 恢复或完整 feature
矩阵已经验收。

接受显式 runtime context 并汇入两种 prompt-payload inner 的 text/message
helper（含 exact-attempt 变体）同样保留
`ExternalRunContext.delegation_policy` 已有的完整目标策略，包含可委派资格、当前层数
和上限。该上下文描述目标 actor，不再调用 `child_policy`；显式 `depth` 参数必须与
其中当前层数一致，冲突在派发前报错。没有完整 runtime policy 的独立旧调用保留
原 `policy_from_depth` 行为，不借此更改框架默认上限或引入应用产品常量。

### Run 与下一次尝试指导

复用同一个 Agent/registry 的不同运行可以使用相同的 task ID 和 attempt。
调用方应从既有 `SubagentCommandIdentity.attempt_identity()` 投影 exact attempt，
保留 `run_id` 与 `control_scope_id`；`for_run` 以 Run 同时作为默认 control scope，
显式嵌套运行仍可使用既有 `for_runtime_scope`。同一 scope/Run/task/attempt 的重复
执行继续拒绝，完成后的有界去重记录不因开始新运行而清空。

在未来 attempt 尚未有 execution ID 时，使用
`SubagentExecutor::queue_guidance_scoped(scope, run, task, attempt, instruction)`
把指导加入同一个原 control queue。准入只领取该 scope/Run/task/attempt 的指导一次；
其它 Run、其它 control scope 和旧的无 scope 队列互不消费。scope、Run、task 不得为空，
attempt 不得为零，指导不得为空。同一逻辑 attempt 已 active 或 settled 时拒绝追加。

原 `queue_guidance(task, attempt, instruction)` 保持无 scope 的独立调用语义；它不猜测
当前 Run，也不能给有 scope 的准入提供指导。两种入口共用原队列、锁与准入检查。
revision、command 幂等及 durable journal 继续由调用方核验，活消息和 interrupt 仍定位
真实 execution/attempt；排队成功不等于模型已消费或应用了指导。参见
[ADR 0058](../adr/0058-task-claim-subagent-attempt-control.md)。

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
可能漏掉整条短 attempt 的进程级 consumer 先调用 `retained_stream_ids` 与
`active_stream_ids`，再从自己的 cursor 对并集逐条 replay；未观察过的 stream 从零开始。
若某条活跃 stream 的完整 replay 后缀已被挤出，`active_stream_anchor` 仍保留其不可变的
dispatch 身份，使 consumer 能以准确地址持久化 gap。
`SubagentEventReplay::gap` 会明确报告保留后缀不连续；高频 thinking/token
delta 可能在 gap 后缺失，但保留的 lifecycle/tool 边界与 `terminal` 可用于恢复状态和最终输出。
该能力是进程内恢复窗口，不是永久存储；应用仍负责自己的 durable projection。

## 调用级只读委派

只读 Agent 可以继续向声明为只读的 Subagent 委派。框架在原装配时使用受限 dispatcher，普通 dispatcher 的写能力声明不变。每次调用的 `tool_access` 使用现有 `ToolAccess`：同一目标声明的 ReadOnly 与任一上游 ReadOnly 都继续收窄后代；它与父工具正向上界共同作用，Fresh 或空历史也不会丢失。

真实可用集由锁后快照计算，模型 schema、tool_search 和强制工具调用采用相同限制。只读角色若错误装有写工具，实际调用仍被阻止；同名工具替换、晚注册、Team 写成员、后台和子调用声明 Mutating 都不能提升能力。受限工具缺少有效 ToolContext/visibility 或工具硬交集为空时明确失败，不靠角色名、prompt 或全局把 agent_tool 标只读。

自定义 Agent 若仅实现旧 execute/chat，默认 invocation 回退会拒绝新的 ReadOnly 请求，避免静默忽略约束；None 保留原行为。新增公开字段的完整 literal 需显式补值，SDK 不自动继承。泛用框架合法深度与 EKO 产品层数分别配置和验收。详细取舍见 [ADR0094](../adr/0094-subagent-tool-boundary-inheritance.md)。
