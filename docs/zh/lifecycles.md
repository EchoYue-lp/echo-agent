# Framework 生命周期

## 生命周期不变量

每条生命周期都必须指明 trigger、authority、可见 event、外部 Effect、cancellation/failure、
terminal boundary、recovery/cleanup owner 和 Projection。相似的状态名不会让两条生命周期变成一个状态机。

- Cancellation request 不是 terminal result。
- EOF、final text、Trace、feed position 和 UI rendering 不能证明成功。
- Terminal publication 发生在该 owner 所要求的 producer 和 cleanup settlement 之后。
- Retry 和 recovery 复用 operation identity 或显式创建新 generation，不静默重放结果未知的 Effect。
- Retention 只移除符合条件的历史，不移除 live authority。

## Agent Turn

```text
surface request
  -> resolve Agent and Session resources
  -> assemble Invocation context
  -> admit and accept Turn
  -> Agent loop: model <-> Tool / Effect
     +-> owner-defined Trace / transcript / Checkpoint projections
  -> settle Agent-owned producers
  -> publish TurnReceipt terminal for a driven Turn
  -> adapter-owned projection and close follow the adapter contract
```

| 边界 | 契约 |
| --- | --- |
| Trigger | Raw execute/chat、Headless、ACP prompt、Eval invocation 或其它 adapter |
| Admission | Driven Turn 先接受输入并建立 identity，再将工作报告为 active |
| Authority | Driven execution 由 AgentTurnDriver 和 TurnReceipt 拥有；raw Agent 仍是低层 API |
| Event 与 Effect | Model output、Tool call、tracked input、file/process/network effect和owner-defined projection可在执行中发生 |
| Cancellation 与 failure | 请求取消，按合同收敛producer，failure保持typed |
| Terminal | Completed、Failed 或 Cancelled execution receipt，以及独立的 delivery result；不从stream EOF推断terminal |
| Recovery 与 Projection | Session/runtime policy 恢复状态；Trace、transcript、checkpoint、sink和adapter各自拥有write/close顺序 |

Reply 和 Wait 操作返回或观察owner的result/receipt，不把partial text、notification或EOF变成新terminal fact。
Delivery failure 在 execution terminal 旁边单独报告，不能改写 producer 已经发出的 execution result。
Trace、transcript和checkpoint write可在finalization前或期间发生。它们各自存储的status和ordering不定义TurnReceipt，
但Agent producer contract显式传播的write error可令driven Turn失败；best-effort transcript diagnostic本身不会。
Adapter projection和awaited close覆盖由各详细adapter contract定义，本概览不定义一条固定顺序。

详见 [ReAct Agent](./01-react-agent.md)、[流式输出](./10-streaming.md)和
[Headless](./33-headless-mode.md)。Adapter-specific 保证仍属于各自协议章节；本页不声称所有
adapter 都已进入 driven Turn 路径。

## Context 与持久化

```text
stable Conversation scope
  -> select runtime-state incarnation
  -> hydrate transcript and Checkpoint
  -> select / budget / assemble Context
  -> optionally compress model-visible messages
  -> execute Turn
  -> independently attempt transcript Projection and runtime Checkpoint writes
  -> report each owner-specific outcome
  -> clear or prune one named authority
```

| 操作 | 受影响权威 | 保留内容 |
| --- | --- | --- |
| Reset active Context | Agent/ContextManager | Durable transcript、runtime state、long-term Memory 和 Trace，除非显式变更 |
| Clear runtime incarnation | RuntimeStateStore | Stable Conversation history 和 long-term Memory |
| Delete transcript | ConversationStore | 由其它owner拥有的runtime或Memory数据 |
| Delete long-term Memory | 被选中的 Store | Transcript、Checkpoint 和 Trace |
| Prune Checkpoint 或 Trace history | 限定retention owner | Live state 和其它persistence domain |

Compression 改变模型可见内容，不改写 transcript 或 Journal fact。详见 [Context 系统](./40-context-system.md)、
[上下文压缩](./04-compression.md)、[记忆](./03-memory.md)和[持久化概念](./41-persistence-concepts.md)。
Checkpoint 和 transcript write 是两个 owner commit。Runtime Checkpoint 可能成功，而当前 transcript Projection
只报告best-effort persistence failure；不得从其中一个结果推导另一个结果。

## Task 与 Subagent

```text
commit Task revision
  -> compute ready frontier
  -> claim PlanTask
  -> dispatch Subagent attempt
  -> observe progress and Effects
  -> receive Subagent outcome
  -> settle / retry / pause / cancel Task
  -> project Todo, event, and UI views
```

| 边界 | 契约 |
| --- | --- |
| Trigger | Task create/update/execute 或 orchestration request |
| Admission | Revision 和 claim 对选中执行的specification加fence |
| Authority | TaskRevisionService 拥有graph；RuntimeTaskService拥有dependency execution；Subagent executor拥有attempt |
| Event 与 Effect | Task progress 和 Subagent envelope观测执行；Tool拥有自己的外部Effect |
| Cancellation 与 failure | Owner runtime 收敛claim并记录retry、pause、cancel或failure，不改写specification |
| Terminal | Task 与 Subagent outcome terminal 有独立 owner；只有 runtime 记录 correlation 时才建立关联 |
| Recovery 与 Projection | Checkpoint/claim recovery属Task owner；Todo和UI仍是projection |

Plan 是可审阅artifact，不是另一个runtime state machine。Workflow 是相邻orchestration capability，
不替代revisioned Task graph。详见 [Task](./09-tasks.md)、[Subagent](./06-subagent.md)、
[多 Agent 模式](./26-multi-agent.md)和[运行时](./29-long-running-tasks.md)。

## Tool、Permission 与 Effect

```text
automatic Agent call
  -> resolve -> React Permission pipeline -> ToolManager validation / admission
direct ToolManager call
  -> caller-owned policy boundary -> ToolManager validation / admission
both
  -> backend executes Effect -> typed result / observation -> backend cleanup
```

| 边界 | 契约 |
| --- | --- |
| Trigger | Automatic React Agent Tool call或direct framework ToolManager invocation |
| Admission | ToolManager validation先于其cache/permit/effect路径；automatic React路径在调用它前评估Permission，direct caller拥有外层policy |
| Authority | ToolManager拥有validation、dispatch、cache和admission；PermissionService只在caller组合它的路径拥有decision；backend拥有Effect |
| Event 与 Effect | Result、Trace和audit观测file、process、network、sandbox或MCP动作 |
| Cancellation 与 failure | Cancellation和cleanup是backend-specific；只有backend contract实际await时caller才能声称已settle |
| Terminal | Typed Tool result或error；observer text不能取代 |
| Recovery 与 Projection | Idempotency、retry和cleanup使用backend policy；Trace/audit仍是observation |

Prompt、project rule 或 Plan 可以引导行为，但不能授予 Permission。Framework policy 适用于 Agent 自动effect，
交互式产品策略属于embedding application。Direct ToolManager使用不会隐式运行PermissionService。
Hook reduction、protected path、read-only classification和layered shell policy保留各自详细合同；本概览不声称它们已形成全局唯一decision owner。详见 [Tool](./02-tools.md)、[人工环路](./05-human-loop.md)、
[安全](./security.md)和[Guard 系统](./18-guard-system.md)。

## Observation 与 Delivery

```text
domain event or fact
  -> named authority commits or publishes
  -> EventEnvelope supplies identity and order
  -> Journal / Delivery Ledger / explicit Outbox records its own domain
  -> Projection, Feed, history, and Trace consume
  -> retention / Checkpoint / generation fence
```

| 边界 | 契约 |
| --- | --- |
| Trigger | Domain commit、runtime event、delivery request或diagnostic observation |
| Admission | 限定authority为自己的domain校验identity、ordering和generation |
| Authority | Journal拥有journal-backed fact；Delivery Ledger或Outbox owner拥有delivery lifecycle；RunStore拥有Trace record |
| Event 与 Effect | EventEnvelope承载identity/order；delivery Effect可能在进程外发生 |
| Cancellation 与 failure | 未知delivery outcome需要reconciliation；lag或EOF不是success |
| Terminal | Domain commit和delivery settlement是显式的；Trace terminal是diagnostic |
| Recovery 与 Projection | Replay/checkpoint重建projection；retention不会把projection变成fact |

没有workspace-wide全局Journal。每个domain自行声明是否journal-backed。详见[持久化概念](./41-persistence-concepts.md)、
[Delivery Ledger](./41-delivery-ledger.md)和[Trace](./27-tracing.md)。Trace、Metrics和Telemetry用于诊断执行；
它们的exporter和retention不会成为business commit权威。

## Extension

```text
discover -> parse -> prepare -> validate -> component publication
       -> activate / use
replace / reload -> component-specific old/new generation coordination
withdraw request -> component-specific close / cleanup observation
```

| 边界 | 契约 |
| --- | --- |
| Trigger | Project、user、Plugin、SDK或application configuration |
| Admission | Publish前解析和校验identity、scope、capability和required resource |
| Authority | MCP、Hook、Skill、Plugin 和 LSP 保留各自registry和resource owner；没有拥有全部组件的通用active generation |
| Event 与 Effect | Tool/resource registration、child process、network connection、resource transfer、Hook action或Skill activation |
| Cancellation 与 failure | Partial publication、rollback和cleanup debt是component-specific且必须可观测；当前组件不共享一个settlement protocol |
| Terminal | 只有specific owner报告resource已settle时close才是terminal；并非所有management API郺wait全部child resource |
| Recovery 与 Projection | Generation fencing存在时也由component拥有；stale derived-handle覆盖、catalog和status view遵循详细component contract |

Plugin publication 可组合component，但不消除child cleanup责任。当前行为必须从 [MCP](./08-mcp.md)、
当前framework不声称拥有覆盖全部extension的单一production coordinator、通用generation fence或awaited close。
[Hook](./23-hooks.md)、[Skill](./07-skills.md)、[Plugin](./32-plugin-system.md) 和 [LSP](./31-lsp-integration.md)
读取；本概览不承诺atomic hot reload。

## SDK Consumer

多语言 SDK 和源码构建的 ACP Host 由独立的
[echo-agent-sdk](https://github.com/EchoYue-lp/echo-agent-sdk) 仓库维护。
它消费本 framework 的 Rust facade 和运行时权威；framework 不拥有 SDK 的语言合同、
生成 catalog 或客户端生命周期实现。

## Failure 与 Terminal 矩阵

| Signal | 含义 | 不代表 |
| --- | --- | --- |
| Accepted | Owner已接纳工作并建立identity | 工作已成功或所有Effect已开始 |
| Cancellation requested | Owner应按policy停止 | Producer、child process或cleanup已收敛 |
| Timeout observed | Caller预算已过期 | 底层工作已terminal，除非owner发布该结果 |
| Stream EOF | 该stream不再收到frame | Turn、Tool、delivery或protocol成功terminal |
| Trace terminal | Diagnostic Run记录到达某个状态 | Product Task或Turn提交同一terminal |
| Completed receipt | 限定lifecycle已发布成功terminal | 所有外部Effect均exactly-once |
| Failed or Cancelled receipt | 限定lifecycle已发布非成功terminal | 所有关联lifecycle共享同一状态 |

## 示例路由

| 生命周期 | 可执行消费者 |
| --- | --- |
| Agent Turn 与 Tool | [`demo01_tools`](../../echo-agent-learning/examples/demo01_tools.rs) |
| Context 与 compression | [`demo53_adaptive_compression`](../../echo-agent-learning/tests/example_contracts/demo53_adaptive_compression.rs) |
| Task 与 Subagent | [`demo04_subagent`](../../echo-agent-learning/tests/example_contracts/demo04_subagent.rs) |
| Workflow event | [`demo34_workflow_stream`](../../echo-agent-learning/tests/example_contracts/demo34_workflow_stream.rs) |
| MCP lifecycle | [`demo30_mcp_server`](../../echo-agent-learning/tests/example_contracts/demo30_mcp_server.rs) |
| Eval 与 Trace | [`demo50_eval`](../../echo-agent-learning/tests/example_contracts/demo50_eval.rs) |
| Headless Projection | [`demo54_headless`](../../echo-agent-learning/tests/example_contracts/demo54_headless.rs) |

## 继续阅读

- [Framework 架构](./architecture.md)
- [核心概念](./concepts.md)
- [运行时与 Task](./29-long-running-tasks.md)
- [持久化概念](./41-persistence-concepts.md)
- [Framework 与应用边界](./39-framework-application-boundary.md)
- [ADR 0040](../adr/0040-framework-concept-documentation-authority.md)
