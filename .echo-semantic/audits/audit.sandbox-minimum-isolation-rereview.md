---
schema_version: 1
id: audit.sandbox-minimum-isolation-rereview
kind: audit
boundary_ref: boundary.tool-permission-sandbox
lens: failure_concurrency
freshness: examined
revision: source:13ff9de40ae621e1201c111201fda28402a397d7104e90595be0c5106482dbc0
finding_refs: [finding.sandbox-minimum-isolation]
challenges:
  explicit-floor-versus-fallback:
    revision: source:13ff9de40ae621e1201c111201fda28402a397d7104e90595be0c5106482dbc0
    source_refs: [echo-core/src/sandbox.rs, echo-execution/src/sandbox/policy.rs, echo-execution/src/sandbox/manager.rs]
    evidence_refs: [evidence.sandbox-minimum-isolation-repair, evidence.sandbox-minimum-isolation-verification]
---

# Sandbox explicit minimum independent rereview

## 审查范围

独立 reviewer 在 `origin/main@f7c1fef7` 集成源码上复核 Finding #83 的显式
minimum、policy 计算、executor 选择及执行入口，并读取 focused 回归收据。

## 已检查故障假设

Docker/Kubernetes 不可用且 `allow_fallback=true` 时，manager 可能将显式
`minimum_isolation=OsSandbox` 的命令交给仅提供 Process 隔离的本地执行器。

## 实际实现路径与证据

Policy 将显式 minimum 作为不可降低的 floor；manager 对最终 executor 的 actual level
再作检查。buffered、limits、stream 路径在低于 floor 时拒绝，`is_available_at`
也按实际选择结果报告。当前主线 manager 测试 12/12 通过，包括不可用 Docker 反例。

## 问题记录

独立 reviewer 对现有实现和本轮证据报告 pass、无阻塞发现；此审计支持本分支
Finding resolved，不代表 Issue 已按远端交付口径关闭。

## 残余风险

未设显式 minimum 的 policy preference 仍可按 `allow_fallback` 降级；这是独立于
caller floor 的可见配置行为。完整 workspace 门禁及远端交付仍待验收。

## 未检查项

独立 reviewer 未自行重跑 focused 测试；未连接真实 Docker/Kubernetes 环境，也未重跑
完整 workspace 或远端 CI。
