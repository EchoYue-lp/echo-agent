---
schema_version: 1
id: finding.turn-driver-entry-coverage
kind: finding
type: authority_conflict
status: open
severity: high
primary_focus: state_authority
focus: [time_lifecycle, failure_concurrency, contract_evidence]
boundary_ref: boundary.agent-session-turn
behavior_refs: [behavior.agent-turn-lifecycle, behavior.protocol-projection]
rule_refs: [rule.turn-terminal-authority, rule.protocol-role-separation]
evidence_refs: [evidence.agent-context-execution, evidence.provider-protocol-quality]
audit_refs: []
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
---

# Channel 与 direct Rust 绕过 driven Turn authority

## 问题

Headless、ACP 与经 ACP 的 SDK 使用 `AgentTurnDriver`/`TurnReceipt`，但 Channel handler 直接调用 `ReactAgent::chat`，raw execute/chat 也直接进入 ReAct run path。

## 触发条件与影响

Channel 或直接 Rust invocation 遇到 EOF、sink failure、cancel、usage accounting 或 close 时，没有共享 TurnReceipt 作为统一终态事实。

## 证据

`src/headless.rs`、`src/acp/runtime.rs` 展示 driven path；`src/channels.rs` 与 `src/agent/react/mod.rs` 展示 bypass path。

## 处理记录

Discovery 记录；后续 audit 必须建立逐入口 route matrix，并区分 raw Agent API 与应统一的产品 adapter。
