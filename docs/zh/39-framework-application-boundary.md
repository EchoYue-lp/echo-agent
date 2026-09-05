# Framework 与应用边界

`echo-agent` 是通用的 Agent 开发框架和工具箱。EKO 是构建在 framework
之上的一个应用，而不是 framework 能提供什么能力的定义。产品无关能力不需要
等第二个外部用户出现后才能进入 framework。

## 归属规则

如果一项能力脱离 EKO 的 `AppState`、workspace identity、Tauri、UI DTO、
产品文件布局和 EKO policy 后仍然语义完整，它就可以归入 `echo-agent`。
它的公共类型必须表达可复用的 Agent 概念，依赖方向必须保持
framework 到 application，并且要有 framework 自己的测试、示例和文档。

当前采用量可以作为 API 成熟度和拆包收益的证据，但不是准入门槛。如果必须先等
用户采用才能建设工具箱，framework 就会依赖它本来要吸引的用户。

## 边界规则

应用可以解析用户或传输输入、选择产品 policy、注入产品 metadata，并把 framework
值投影到具体 surface。但通用 framework 类型本身必须保持唯一权威：应用不得再镜像
一份 framework DTO，也不得暴露 `to_framework_*`、`from_framework_*` 这类来源命名的
转换 helper。framework 缺少通用能力时，应先补到 framework，并让真实应用主路径切换
到该 API，再删除重复模型。

公开 facade 路径按能力命名，不按源码 crate 命名。例如 `echo_agent::llm` 直接导出 client
和配置，`echo_agent::llm::types` 是经过文档化的低层 wire surface；不保留
`llm::core`、`llm::integration` 这类 split-crate 迁移路径。

## 当前所有权

| Framework | EKO 应用 |
| --- | --- |
| Agent turn 执行、tracked receipt、Task DAG、retry 和取消 | workspace identity、文件任务事实、review/worktree policy |
| Tool 协议、ToolManager、artifact 和 permission 原语 | direct-user 可见性、保留策略和 UI/tool 投影 |
| Subagent 生命周期和 attempt-scoped control | EKO pool policy、workspace generation 和 surface command |
| Journal/checkpoint 原语和 immutable plugin preparation | Chat payload、retention、target publication 和 EKO preference |

`KeyedExecutionAdmission` 是可复用的 framework 原语，负责 opaque key 的 lease、每个
key 的 process permit、retirement fence、close 和 shutdown wait。EKO 的 `AgentPool`
在其上提供 wrapper，同时继续拥有 Agent 创建、capacity class、workspace transition
以及 plugin/model/tool publication policy。

因此 `AgentPool`、`AgentRouter`、`ChatEventLog`、`PluginRuntimeService` 和
`ExtensionControlService` 只能在产品无关的 seam 上评估 kernel 抽取，不批准整体迁移。
EKO 的 `AppState`、workspace registry、DomainProfile、research/analysis/browser policy、
review/worktree 行为和各 surface projection 继续属于应用层。

## 单一权威

`ExecutionAdmission` 是 `KeyedExecutionAdmission` 的组合入口，不携带 EKO 配额。
`max_concurrent_subagents` 仍是 standalone per-runtime width，
`max_concurrent_forks` 仍是无 shared admission 时的 fallback。

Framework 和应用必须对通用生命周期或 receipt 共享同一种语义。如果某个 adapter
需要自己的 DAG 遍历、status reducer、retry policy 或 durable input lifecycle，说明边界
不正确。应复用或扩展已有 framework authority；如果无法在不耦合产品的前提下形成更小的
kernel，就继续把能力留在 EKO。

决策记录见 [ADR 0014](../adr/0014-framework-capability-placement.md) 和
[ADR 0015](../adr/0015-keyed-execution-admission.md)，跨仓候选审计见
顶层 `docs/2026-08-30-framework-capability-placement-audit.md`。
