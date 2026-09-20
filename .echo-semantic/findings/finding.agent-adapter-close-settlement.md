---
schema_version: 1
id: finding.agent-adapter-close-settlement
kind: finding
type: authority_conflict
status: resolved
severity: high
primary_focus: time_lifecycle
focus: [failure_concurrency, result_side_effect, state_authority]
boundary_ref: boundary.agent-session-turn
behavior_refs: [behavior.agent-turn-lifecycle, behavior.protocol-projection]
rule_refs: [rule.turn-terminal-authority, rule.protocol-role-separation]
evidence_refs: [evidence.agent-context-execution, evidence.provider-protocol-quality]
audit_refs: [audit.agent-session-turn.state-authority, audit.protocol-surfaces.time-lifecycle]
decision_refs: [decision-adr-0066-agent-adapter-close-ownership]
repair_evidence_refs: [evidence.agent-adapter-close-settlement-repair]
verification_evidence_refs: [evidence.agent-adapter-close-settlement-verification, evidence.foundation-36-72-51-integration-verification]
rereview_audit_refs: [audit.agent-adapter-close-settlement-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Agent adapter close 与资源结算 owner

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/36

## 问题

ACP、Headless 与 Channel 曾缺少一致的 retained close owner；ReactAgent close 也曾只结算 MCP，
没有 fence/cancel/wait active 或 queued Turn。

## 触发条件与影响

Adapter shutdown、disconnect、caller cancellation 或 owner drop 时，in-flight Turn、MCP/LSP/child
resource 与 cleanup debt 可能晚于外部 close 返回或被进程退出截断。

## 证据

`src/acp/adapter.rs`、`src/headless.rs`、`echo-integration/src/channels/manager.rs`、
`channels/session.rs` 与 `src/agent/react/lifecycle.rs` 展示当前 owner 合同。

## 处理记录

ACP adapter 在 poll connection future 前同步交出同一 services/profile 的 close handle；Headless
通过 owned task 与 `HeadlessRunHandle` 保留结果和 close retry owner；Channel manager/session 在
start 前登记 handler，取消或失败后保留同一 owner；ReactAgent close fence admission、取消并等待
active/queued Turn terminal 后再关闭 MCP；preparation 或 producer 异常 Drop 会形成 persistent
close debt，不能释放 MCP；driven stream start 前的 typed cancellation 仍结算为 Cancelled。
A2A 由独立 Findings 持有，不属于 #36 的完成边界。
