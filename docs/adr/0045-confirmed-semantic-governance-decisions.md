# ADR 0045: Confirmed Semantic Governance Decisions

## Status

Accepted

- Date: 2026-09-15
- Owners: Framework runtime, extension lifecycle, provider contract maintainers

本 ADR 固化本轮已由产品方明确确认的五个独立语义裁决。后续实现可以细化
数据结构和迁移顺序，但不得重新打开这些产品取舍；只有发现与当前代码、协议或
安全边界不可同时满足的事实时，才创建新的增量裁决。

## Decision Units

### DU-59: Agent Hook Permission Uses Global Deny-Wins

Agent 自动 Tool 副作用路径上，所有匹配的 Permission action 都参与归约，优先级为
`deny > ask > require_approval > allow`。早期 `allow` 或 `ask` 不能阻断后续来源的
`deny`；归约应保留来源和最终决定，供 PermissionService、Hook audit 与 protected-path
检查使用。

该规则只适用于 Agent 自动决策路径。用户主动打开终端、文件选择器或连接 MCP 的交互
动作不套用该 Agent 自动权限闸门。

### DU-63: LSP Derived Handles Cascade-Invalidate

`LspManager` 是 LSP child process 的唯一 owner；`get_client` 返回派生 handle，不转移
进程所有权。Manager close/shutdown 后，旧 handle 必须变为 typed stale/closed，禁止
再次 initialize、restart 或复活新的 child process。Manager generation/closed fence 是
唯一生命周期权威，调用方不能绕过它建立独立 owner。

### DU-71: Component Preparation, Atomic Generation Publication

Plugin generation 在 prepare 阶段按组件隔离：无效组件被排除并记录诊断，健康兄弟组件
可以保留。完整 immutable `PreparedPluginSet` 仍按 generation 一次性发布，禁止跨
generation 混合、旧组件重叠或旧 receipt 解绑新资源。

该决定依赖并约束 #72 generation publication、#73 lifecycle coordination、#74
reconcile overlap 和 #75 MCP owner isolation。

### DU-97: Strict JSON Schema Is Framework-Enforced

`ResponseFormat::JsonSchema` 且 `strict=true` 是 framework 端到端保证：Provider adapter
发送 provider hint，framework 在本地执行 JSON Schema validation；不符合 schema 不能
作为成功结果返回，使用 typed schema-validation error 和有界重试。`JsonObject` 只保证
JSON 语法；`strict=false` 是非强制 hint。

该决定先依赖 #77 provider capability authority 和 #96 structured-output main path，
再进入 schema validator 与 retry repair。

### DU-68: Layered Model Facts with Freshness and Conservative Fallback

所有 model/provider fact 必须携带 `source`、`provenance`、`version`、`observed_at`、
`expires_at` 和 `confidence`。Framework 拥有 resolution/freshness 规则，Provider adapter
提供协议级事实，应用/调用方拥有显式 override，内置 catalog 是带版本的保守 fallback。

解析优先级固定为：

`exact explicit override > fresh exact model fact > fresh provider fact > versioned built-in > conservative unknown`。

stale/unknown 不得静默提升 structured-output 或 tool capability；预算类事实采用保守
降级。该决定先影响 #77，再影响 #96/#97，并为 #100 tokenizer calibration 提供来源和
样本 provenance。

## Consequences

- 后续 repair 必须以这五个 decision unit 为 design/preflight 上游，并在 Evidence 中
  引用对应 DU 标识。
- 这些裁决不把 EKO 产品策略塞进通用 framework：Hook、LSP、Plugin、Provider facts
  的通用 authority 留在 framework，用户主动交互和 UI policy 仍在 adapter/application。
- #59、#63、#71、#97、#68 的旧 Finding 可以在各自 repair、verification 和独立复审证据
  完成后关闭；本 ADR 本身不代表任何 Finding 已修复。
- 业界实现调研仍是关键架构实现的必要证据；本 ADR 只固定产品期望，不替代每个 repair
  slice 的设计、验证和回滚说明。
