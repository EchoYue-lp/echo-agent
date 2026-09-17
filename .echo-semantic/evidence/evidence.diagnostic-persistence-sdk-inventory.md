---
schema_version: 1
id: evidence.diagnostic-persistence-sdk-inventory
kind: evidence
observed_at: source:cbbde65a0aed106aa28d69d4e514afd4038ce9eb4b407629ece1451cbf45279e
source_refs:
  - src/audit.rs
  - echo-state/src/audit/mod.rs
  - .echo-semantic/assets/asset.external-sdk-repository.md
supports: [behavior.observation-persistence, rule.fact-projection-separation]
limitations:
  - 本Evidence是核实记录，不是重建三语言映射的授权；外部SDK inventory的刷新由外部仓库owner执行
  - 核实基于本地检出 /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent-sdk 的静态内容，未执行外部仓库生成器
---

# DiagnosticDelivery 公共 API SDK inventory 核实

## 支持的结论

官方清单（Issue #46 评论 2026-09-15）第 3 项要求"为新增 DiagnosticDelivery 公共 API 重新生成
并复核 SDK public inventory"。核实结果：该项的生成义务已随 #126（SDK 独立仓库抽取）迁出本仓库，
不应在本仓库重建三语言映射。

## 来源与范围

核实事实（只读核实，基于合并 main@44b2ed68 之后的 worktree 现场）：

1. 本仓库的已跟踪源码不再包含 `sdks/`、`echo-sdk-host/` 或 `echo-sdk-protocol/`
   生成链；`.echo-semantic` 既有 evidence
   （evidence.checkpoint-journal-sdk-inventory）明确"原SDK生成物已经从framework删除；
   外部echo-agent-sdk PR必须吸收canonical payload后才能完成Issue 122"。
2. 新增 DiagnosticDelivery 公共面全部位于 framework 侧 `echo-state::audit` 与再导出面
   `echo_agent::audit`：`DiagnosticRecordKind`、`DiagnosticDeliveryOperation`
   （含 Start/Append/Load/Finalize/Record）、`DiagnosticDeliveryFailure`、
   `DiagnosticDeliveryObserver`、`AuditCallback::with_diagnostic_delivery_observer`、
   `diagnostic_delivery_dropped_count()` 与两个 `#[doc(hidden)]` process-local 入口。
3. 外部独立仓库 `/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent-sdk` 的 canonical
   inventory `contracts/sdk/public-api.txt`（23,668 行，9724 项口径）当前不含任何
   `DiagnosticDelivery*` / `diagnostic_delivery` 条目（ripgrep 零命中）；其 README 明确该仓库
   "must not be described as independently runnable or parity complete"，且吸收 framework 新
   payload 属于其自身迁移边界。

## 处置

按官方清单第 3 项原文的但书处理："keep process-local observer/control entries Host/Rust-only
or deferred rather than inventing language wrappers"——本 finding 不新增语言包装。刷新
9724→N 项 inventory 与三语言 catalog 的义务记为外部 echo-agent-sdk 仓库事项（其吸收
#43/#55/#106 payload 的同一迁移通道），不在本仓库闭合；本事实同时写入 rereview audit 的
残余风险，供 GitHub Issue #46 关闭判定时参考。

## 已知缺口

外部仓库未来吸收 payload 后的 inventory diff、Host adapter 是否需要
`AuditCallback::with_diagnostic_delivery_observer` 桥接，均属外部仓库后续 verification。
