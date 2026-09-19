---
schema_version: 1
id: audit.plugin-lifecycle-coordinator-rereview
kind: audit
boundary_ref: boundary.extension-lifecycle
lens: time_lifecycle
freshness: examined
revision: dc61ef0e407daf43384cc7dc83bdd94298bce4e5
finding_refs: [finding.plugin-lifecycle-coordination]
challenges:
  desired-actual-and-cleanup-debt:
    revision: dc61ef0e407daf43384cc7dc83bdd94298bce4e5
    source_refs: [src/plugin/coordinator.rs, src/plugin/prepared.rs, echo-core/src/plugin/registry.rs, echo-core/src/plugin/lifecycle.rs]
    evidence_refs: [evidence.plugin-lifecycle-coordinator-repair, evidence.plugin-lifecycle-coordinator-verification]
  dependency-target-and-generation-fences:
    revision: dc61ef0e407daf43384cc7dc83bdd94298bce4e5
    source_refs: [src/plugin/coordinator.rs, src/plugin/prepared.rs, echo-integration/src/mcp/identity.rs]
    evidence_refs: [evidence.plugin-lifecycle-coordinator-repair, evidence.plugin-lifecycle-coordinator-verification]
  cancellation-refresh-and-scope-policy:
    revision: dc61ef0e407daf43384cc7dc83bdd94298bce4e5
    source_refs: [src/plugin/coordinator.rs, echo-core/src/plugin/registry.rs, tests/plugin_coordinator.rs]
    evidence_refs: [evidence.plugin-lifecycle-coordinator-repair, evidence.plugin-lifecycle-coordinator-verification]
---

# Plugin host lifecycle coordinator independent rereview

## 审查范围

复审 PluginCoordinator 的 startup、reconcile、reload、enable、disable、uninstall、retry 与
shutdown，覆盖 Registry desired state、callback lifecycle debt、prepared generation publication、
typed MCP owner、Hook attempt、dependency topology、Agent target 与 scope discovery policy。

## 已检查故障假设

- callback 或 unwire 失败后 desired/actual 被错误回滚或 debt 丢失；
- reload 在新 generation 无效时先撤销旧 callback/wiring；
- dependency 词法顺序覆盖拓扑，或撤销未按反向拓扑执行；
- converged receipt 被另一个 Agent target 复用；
- late callback registration 未使 convergence 失效；
- publication/event future 取消后重试重复 effect 或错误报告完成；
- retry 无条件 scan all scopes，扩大 host 的 Registry discovery policy；
- 同名 Direct、Plugin A、Plugin B MCP identity 发生碰撞。

## 实际实现路径与证据

Coordinator 串行化 operation，并把 durable desired intent、live generation receipt 与 callback
effect/debt 留在各自既有 authority。Preparation 在任何 withdrawal 前完成完整 dependency 和
generation 校验；失败保持旧 actual。Registry refresh 使用 last successful scope policy，并只在
候选 scan 全成功后 swap。取消保留精确 ActualPending phase，同一 operation retry 从对应 phase
继续；Hook attempt 在 operation 内去重，跨进程 durable acknowledgement明确留给 #58。

候选先后经过三轮实现复审与 advancing-base 增量复审，最终均为 0 Critical、0 Important、
0 Minor。最终 head 通过 coordinator 14/14、完整 `./scripts/verify.sh`、17-feature matrix、
semantic strict/change-evidence、formatter 与 diff-check。PR #145 的 Linux quality、三组 Linux
tests、learning、Windows atomic replacement 与 dependency policy 七项 CI 全绿，修复以 GitHub
verified squash commit `dc61ef0e407daf43384cc7dc83bdd94298bce4e5` 进入远端 main。

## 问题记录

post-merge closure rereview 未发现 Critical、Important 或 Minor 问题；未新增 Finding。

## 残余风险

Hook event attempt 只保证 operation 内 at-most-once；跨进程 durable delivery 由 Issue #58 继续
追踪。外部 MCP transport fault settlement 继续由 #72/#75 evidence 覆盖。

## 未检查项

未宣称 #58、外部 MCP transport E2E 或 EKO application fan-out 已闭合。
