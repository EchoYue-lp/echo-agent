---
schema_version: 1
id: finding.tool-registry-mutation-active-call-deadlock
kind: finding
type: implementation_bug
status: resolved
severity: high
primary_focus: failure_concurrency
focus: [time_lifecycle, state_authority, contract_evidence]
boundary_ref: boundary.tool-permission-sandbox
behavior_refs: [behavior.effect-permission-execution]
rule_refs: [rule.permission-effect-order]
evidence_refs: [evidence.effects-extensions, evidence.tool-read-cache-authority-verification, evidence.tool-registry-owned-handle-repair, evidence.tool-registry-owned-handle-verification]
audit_refs: [audit.tool-permission-sandbox.failure-concurrency, audit.tool-read-cache-authority-rereview, audit.tool-registry-owned-handle-rereview]
decision_refs: []
repair_evidence_refs: [evidence.tool-registry-owned-handle-repair]
verification_evidence_refs: [evidence.tool-registry-owned-handle-verification]
rereview_audit_refs: [audit.tool-registry-owned-handle-rereview]
discovered_at: 50890faac10ab91c90dc45769854c4b6e35f8376
---

# Tool registry mutation 可被跨 await Ref 阻塞

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/115

## 问题

ToolManager执行路径把DashMap Tool Ref持有到完整异步执行和cache publication结束；同步replace/unregister需要取得同一shard的写访问。

## 触发条件与影响

在current-thread runtime中，Tool future挂起时从同一executor线程调用replace或unregister会阻塞该线程，而旧future必须由该线程恢复后才能释放Ref，可能形成runtime deadlock。

## 证据

`echo-execution/src/tools.rs`的`get_tool`返回DashMap Ref，non-stream与stream局部变量均跨await存活；本轮最初同步replacement交错测试稳定挂起，改为spawn_blocking后才允许旧Read释放、replacement完成。

## 处理记录

第三轮Tool cache复审确认该问题独立于cache freshness。Issue #115先于修复建立；Arc owned handle以red/green、ADR、SDK生成和独立复审闭合本Finding。GitHub Issue保持open，等待本地修复进入远端main后关闭。
