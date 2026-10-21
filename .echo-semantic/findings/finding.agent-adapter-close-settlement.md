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


## 2026-10-21 ACP original-driver custody 的有界后继 challenge

本对象原 resolved、#36 与旧资格保持各自历史范围，不重开外部 #36，也不声称本次
反例就是旧 #36 的历史根因。后继实现反例显示：保留 close handle/Run entry 仍可能
丢失 receipt 前的唯一 driver；同一官方 task actor 中另一任务 Err 会 drop 原 driver。
标准 Prompt 和 shared prepare_run 两条真实入口均已复现 red101（0pass/2fail），
原生产3cd修为c135后相同测试 green0（2pass/0fail/29filtered）。原 Evidence repair/
verification 与同 owner Audit 的新 challenge 保存此有限修复。
当前仅具名 focused 资格；后继全量/政策/精确main/消费者未完成，不把本段算作新
resolved 或广域 closed。SDK managed12 首个 other-actor Err 仍 UNKNOWN，其重验
独立；framework #120 水位 Issue 的既有关闭不在本切片改动。
