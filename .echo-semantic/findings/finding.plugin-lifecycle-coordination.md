---
schema_version: 1
id: finding.plugin-lifecycle-coordination
kind: finding
type: authority_conflict
status: open
severity: high
primary_focus: time_lifecycle
focus: [state_authority, failure_concurrency, data_durability]
boundary_ref: boundary.extension-lifecycle
behavior_refs: [behavior.extension-publication]
rule_refs: [rule.extension-generation-authority]
evidence_refs: [evidence.effects-extensions]
audit_refs: [audit.extension-lifecycle.state-authority, audit.extension-lifecycle.time-lifecycle]
decision_refs: []
repair_evidence_refs: [evidence.plugin-lifecycle-coordinator-repair]
verification_evidence_refs: [evidence.plugin-lifecycle-coordinator-verification]
rereview_audit_refs: []
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Plugin Registry、wiring 与 callback lifecycle 未统一编排

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/73

## 问题

Registry disable/uninstall、Integrator unwire 与 LifecycleManager deactivate/shutdown 是三段状态，未发现一个生产入口确保顺序和 cleanup debt 闭合；PluginLoaded/Disabled 也未发现 emitter。

## 触发条件与影响

Plugin reload/disable/uninstall 或 callback failure 时，持久 enabled 状态、live components、callbacks 和 Hook events 可能分离。

## 证据

`echo-core/src/plugin/registry.rs`、`plugin/lifecycle.rs`、`src/plugin/prepared.rs` 与 Hook event definitions 提供证据。

## 处理记录

候选实现增加 framework `PluginCoordinator`，按 callback cleanup、精确 wiring receipt rollback、
immutable generation publish、callback activation 和 post-commit Hook attempt 的顺序收敛 durable
desired state。focused fault/retry/cancel/restart 与 Direct+A+B same-name identity 测试已通过；本
Finding 在独立复审、完整门禁、远端 CI 与主线交付前保持 open。跨进程 durable Hook delivery
继续由 #58 独立追踪。

独立复审指出的 dependency topology、Agent target binding、late callback registration 与
future-drop status 投影均已修复。第二轮复审新增的 invalid-preparation-before-withdrawal 与
deterministic publication cancellation 也已闭合；修复后 coordinator 14/14、lifecycle 10/10、
文档契约、demo 与 focused Clippy 全绿；仍待最终独立复审、完整门禁和远端交付。

第三轮复审要求的 Registry refresh authority 已改为 last-successful-scope、commit-on-success；
all-scope dependency repair、restricted-scope non-widening 与 scan failure preserving old actual
回归通过，最终 coordinator 为 14/14。
