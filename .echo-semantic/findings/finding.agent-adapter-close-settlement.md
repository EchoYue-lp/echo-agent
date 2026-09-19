---
schema_version: 1
id: finding.agent-adapter-close-settlement
kind: finding
type: authority_conflict
status: open
severity: high
primary_focus: time_lifecycle
focus: [failure_concurrency, result_side_effect, state_authority]
boundary_ref: boundary.agent-session-turn
behavior_refs: [behavior.agent-turn-lifecycle, behavior.protocol-projection]
rule_refs: [rule.turn-terminal-authority, rule.protocol-role-separation]
evidence_refs: [evidence.agent-context-execution, evidence.provider-protocol-quality]
audit_refs: [audit.agent-session-turn.state-authority, audit.protocol-surfaces.time-lifecycle]
decision_refs: []
repair_evidence_refs: [evidence.agent-adapter-close-settlement-repair]
verification_evidence_refs: [evidence.agent-adapter-close-settlement-verification, evidence.foundation-36-72-51-integration-verification]
rereview_audit_refs: []
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Agent adapter close 与资源结算 owner 未闭合

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/36

## 问题

ACP 已正确停止接纳、取消/等待 Run 并 await Agent close；Headless 返回结果不 close，A2A 无 close API，Channel manager 只停止 transport 且 MessageHandler 无 close 合同；ReactAgent Drop 的 MCP cleanup 也是未等待任务。

## 触发条件与影响

Adapter shutdown、disconnect 或 owner drop 时，in-flight Turn、MCP/LSP/child resource 与 cleanup debt 可能晚于外部 close 返回或被进程退出截断。

## 证据

`echo-integration/src/channels/manager.rs`、`channels/types.rs` 与 `src/agent/react/mod.rs` 展示 stop/handler/Drop 合同。

## 处理记录

State-authority Audit 记录；后续 time-lifecycle audit 需分别确认每个 adapter 的 Agent ownership、admission stop、cancel、drain 与 awaited close 顺序。

当前工作树形成部分 repair candidate：ACP adapter在poll连接future前同步交出同一服务的close handle，连接future取消或双失败后仍可重试；registry保留Run receipt与Agent。Headless awaited close、Channel manager在start前登记handler并关闭sender session、ReactAgent Drop取消detached MCP cleanup。A2A未修改且仍是本Finding的开放缺口，因此Finding保持`open`；即使其它范围通过复审和门禁也不得关闭#36。
