---
schema_version: 1
id: audit.hook-permission-precedence-rereview
kind: audit
boundary_ref: boundary.extension-lifecycle
lens: permission_external
freshness: examined
revision: eadf1a3d5a498bdbccd3742a7e1a457cb27b172d
finding_refs: [finding.hook-permission-precedence]
challenges:
  external-permission-stop:
    revision: eadf1a3d5a498bdbccd3742a7e1a457cb27b172d
    source_refs: [echo-execution/src/skills/hooks.rs]
    evidence_refs: [evidence.hook-permission-precedence-repair, evidence.hook-permission-precedence-verification]
  non-permission-stop-preservation:
    revision: eadf1a3d5a498bdbccd3742a7e1a457cb27b172d
    source_refs: [echo-execution/src/skills/hooks.rs, docs/en/23-hooks.md, docs/zh/23-hooks.md]
    evidence_refs: [evidence.hook-permission-precedence-repair, evidence.hook-permission-precedence-verification]
---

# Hook permission precedence 独立复审

## 审查范围

复审 declarative、command、HTTP 和 programmatic Hook 输出进入同一 permission reducer 的
路径，并检查 `continue: false` 在 permission 与非 permission 结果中的不同传播合同。

## 已检查故障假设

验证较早来源的 allow 或 ask 是否能通过 `continue: false` 阻止后续匹配来源的 deny 被观察；
同时验证修复是否错误取消普通非 permission Hook 的显式停止传播能力。

## 实际实现路径与证据

`merge_result` 只在 incoming 结果不携带 permission decision 时传播 stop；所有 permission
action 继续由唯一 priority reducer 按 `deny > ask > require_approval > allow` 结算。纯解析
测试覆盖 allow/ask 与普通 stop，真实 command 测试覆盖外部输出到跨来源 deny 的生产路径。

Reviewer 检查提交 `eadf1a3d` 后未发现 Critical、Important 或 Minor 问题，结论 PASS。

## 问题记录

`finding.hook-permission-precedence` 具备 repair、verification 与独立 rereview 证据，可标记
resolved。GitHub Issue #59 等待修复进入远端 main 后关闭。

## 残余风险

HTTP 传输本身未在本切片建立网络 fixture；它与 command 共享 `parse_hook_output` 和
`merge_result` 生产入口，协议输出的反例由共享解析测试覆盖。完整 workspace 门禁由集成
分支执行。

## 未检查项

未执行完整 workspace gate、全部 feature 矩阵、远端 CI 或其它 open Finding。
