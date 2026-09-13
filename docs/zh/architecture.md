# Framework 架构

## 范围

echo-agent 是可复用的 Rust Agent framework。它拥有产品无关的 Agent、
执行、状态、编排、集成、Tool、协议和 SDK 机制。Embedding application 拥有自己的
产品 Workspace、用户体验、部署和 Device 策略。详见 [Framework 与应用边界](./39-framework-application-boundary.md)。

本页解释 package 和 owner 边界，不替代[配置参考](./28-config-reference.md)或各功能章节。

## Package 拓扑

Workspace 包含 root `echo_agent` package 和十个 member。Cargo manifest 是该图的权威。

```text
Embedding application / protocol surface
                    |
                    v
           echo_agent root facade
        /      |       |       \
 echo_core  execution  state  orchestration  integration  tools  macros
                    |
       +------------+-------------+
       |                          |
echo-sdk-protocol             echo-agent-learning
       |
echo-sdk-host --------------> echo_agent
```

精确 member 集合和 feature 图来自 root [`Cargo.toml`](../../Cargo.toml)。README 表格直接与
Cargo metadata 校验，不维护第二份 package registry。

## 分层 Owner

| Package | 职责 | 依赖方向 |
| --- | --- | --- |
| `echo_agent` | 稳定 public facade 与 framework 组合 | 消费七个 split framework crate |
| `echo-core` | Agent、LLM、Tool、permission、event 和共享领域契约 | 无 workspace 依赖的基础层 |
| `echo-execution` | Sandbox、Skill 和执行机制 | 依赖 `echo_core` |
| `echo-state` | Memory、compression、persistence 和 audit 实现 | 依赖 `echo_core` |
| `echo-orchestration` | Turn driver、Task、Subagent、Workflow、scheduler 和生命周期原语 | 依赖 `echo_core` |
| `echo-integration` | Provider、MCP、LSP、channel 和外部协议实现 | 依赖 `echo_core` |
| `echo-tools` | 可复用的 file、shell、web、data、media、database 和 research Tools | 依赖 core 契约和 macros |
| `echo-macros` | 编译期 Tool、callback、guard 和 handler adapter | 依赖 core 契约和 orchestration 类型 |
| `echo-sdk-protocol` | 确定性 facade inventory、schema、wire contract 和生成 | 依赖 `echo_core` |
| `echo-sdk-host` | 通过 ACP 和命名空间操作暴露 `echo_agent` 的运行时 Host | 消费 root facade 和 SDK protocol |
| `echo-agent-learning` | 不发布的可执行消费者、示例和文档契约 | 只消费 root facade |

Root [`src/lib.rs`](../../src/lib.rs) 是公共组合权威。合理的 public framework 选项不会因某个
application 没有使用就变成死代码。Library package 是编译期复用边界，不是runtime registry或状态权威。

## 公共组合

Framework 用户应从 `echo_agent` 及其文档化模块进入。Split crate 保持实现责任可测，
facade 保持下游路径稳定。SDK protocol 和 Host 是消费者/adapter；它们不把协议状态
移入 `echo_core`，也不使任何语言 SDK 取代 Rust API 权威。

Raw Agent API 仍是合理的低层契约。[Headless](./33-headless-mode.md)、ACP、Eval 和 SDK Host 等
driven surface 可以组合共享 Turn driver 和 receipt 生命周期。Surface 可翻译 identity、event
和 error，但不得发明第二个 terminal 或 Task graph 权威。

## Capability 路由

| 问题 | 起点 | 详细 owner |
| --- | --- | --- |
| 一次 Agent 请求如何执行？ | [生命周期](./lifecycles.md) | [ReAct Agent](./01-react-agent.md) 和 runtime Turn driver |
| 名称与 identity 如何关联？ | [核心概念](./concepts.md) | Agent、Session、Invocation、Turn 和限定 store |
| 什么进入模型窗口？ | [核心概念](./concepts.md) | [Context 系统](./40-context-system.md) 和[压缩](./04-compression.md) |
| 谁拥有 Task 和 Subagent 执行？ | [生命周期](./lifecycles.md) | [Task](./09-tasks.md)、[Subagent](./06-subagent.md) 和[Runtime](./29-long-running-tasks.md) |
| 什么是 fact、checkpoint、projection 或 Trace？ | [核心概念](./concepts.md) | [持久化概念](./41-persistence-concepts.md) 和[Trace](./27-tracing.md) |
| Tool 和 Permission effect 在哪里执行？ | [生命周期](./lifecycles.md) | [Tool](./02-tools.md)、[人工环路](./05-human-loop.md) 和[安全](./security.md) |
| MCP、Hook、Skill、Plugin 和 LSP 如何演进？ | [生命周期](./lifecycles.md) | [MCP](./08-mcp.md)、[Hook](./23-hooks.md)、[Skill](./07-skills.md)、[Plugin](./32-plugin-system.md) 和 [LSP](./31-lsp-integration.md) |
| SDK 契约如何派生？ | [核心概念](./concepts.md) | [Source-first SDK](../adr/0028-source-first-multilanguage-sdk-runtime.md) 和 SDK scope ADR |
| Effect 如何交付和观测？ | [生命周期](./lifecycles.md) | [Delivery Ledger](./41-delivery-ledger.md)、Outbox pattern 和 [Trace](./27-tracing.md) |
| Product Backend、Frontend、Desktop 或 Device 关注属于哪里？ | [应用边界](./39-framework-application-boundary.md) | Embedding application |

## Application 边界

Framework 可以接受 invocation workspace reference，并提供 file、process、sandbox 或 Git 原语。
它不拥有 EKO Workspace 记录、device synchronization、GUI/TUI state reduction、产品认证或部署控制面。
这些策略属于 embedding application。
Framework 的 Trace、Metrics 和 Telemetry 原语提供诊断；startup、exporter、deployment和retention policy仍是显式consumer选择。

Protocol adapter 也是边界而非 owner。ACP、A2A、Channels、Headless 和 SDK Host 将 framework
行为投影到不同 surface。它们的当前保证由详细章节和测试定义，本概览不扩大承诺。

## 事实权威

| 事实 | 权威 | 消费者检查 |
| --- | --- | --- |
| Package 和 feature 拓扑 | Cargo manifest 和 metadata | README 文档契约 |
| Public Rust surface | `echo_agent` facade | Facade smoke 和 SDK inventory |
| Runtime 行为 | Source、test 和 accepted ADR | Focused 与 integration test |
| 持久语义 | 限定 Store、Journal、checkpoint 或 ledger | Recovery 和 contract test |
| 可执行示例 | Cargo target metadata | `echo-agent-learning` contract |
| SDK 语言 scope | Parity manifest 和 source contract | Rust 与各语言 SDK gate |

文档权威及其取舍记录在 [ADR 0040](../adr/0040-framework-concept-documentation-authority.md)。

## 继续阅读

- [核心概念](./concepts.md)
- [Framework 生命周期](./lifecycles.md)
- [快速入门](./getting-started.md)
- [可执行 quickstart](../../echo-agent-learning/examples/demo00_quickstart.rs)
- [Framework 与应用边界](./39-framework-application-boundary.md)
