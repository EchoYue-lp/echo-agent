---
schema_version: 1
id: audit.plugin-lifecycle-reconcile-rereview
kind: audit
boundary_ref: boundary.extension-lifecycle
lens: time_lifecycle
freshness: examined
revision: fe018b059d85bb6b92c1d23e421915fa8970ffb9
finding_refs: [finding.plugin-lifecycle-reconcile-overlap]
challenges:
  deactivate-and-shutdown-debt:
    revision: fe018b059d85bb6b92c1d23e421915fa8970ffb9
    source_refs: [echo-core/src/plugin/lifecycle.rs, docs/adr/0060-plugin-lifecycle-reconcile-settlement.md]
    evidence_refs: [evidence.plugin-lifecycle-reconcile-repair, evidence.plugin-lifecycle-reconcile-verification]
  init-failure-cleanup:
    revision: fe018b059d85bb6b92c1d23e421915fa8970ffb9
    source_refs: [echo-core/src/plugin/lifecycle.rs]
    evidence_refs: [evidence.plugin-lifecycle-reconcile-verification]
---

# Plugin lifecycle reconcile 独立复审

## 审查范围

独立 reviewer 分三轮检查 reconcile、直接 activate、deactivate、unregister、shutdown debt、init failure、回归测试和 ADR。

## 已检查故障假设

检查旧代 deactivate 失败后新代启动、shutdown debt 被成功 deactivate 误清除、以及 init 未成功却错误调用 deactivate 导致永久 cleanup debt。

## 实际实现路径与证据

最终实现分别跟踪 deactivation 与 shutdown debt；任一未结算都阻断新代 activate。成功 deactivate 不清 shutdown debt，init failure 只要求 shutdown。反例测试明确拒绝未激活时 deactivate，并验证 shutdown 后可移除。

## 问题记录

前两轮发现的两个 Important 问题均由后续增量修复；第三轮复审结论 pass。

## 残余风险

跨 Registry、wiring 和 callback 的统一 coordinator 仍由独立 Finding #73 追踪，本修复不建立第二 coordinator。

## 未检查项

未执行真实第三方 plugin 外部进程或网络资源故障注入。
