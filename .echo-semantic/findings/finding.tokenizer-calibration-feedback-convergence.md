---
schema_version: 1
id: finding.tokenizer-calibration-feedback-convergence
kind: finding
type: implementation_bug
status: open
severity: medium
primary_focus: time_lifecycle
focus: [state_authority, contract_evidence]
boundary_ref: boundary.llm-provider-runtime
behavior_refs: [behavior.llm-provider-execution]
rule_refs: [rule.provider-protocol-boundary]
evidence_refs: [evidence.provider-protocol-quality]
audit_refs: [audit.llm-provider-runtime.time-lifecycle]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
---

# Tokenizer calibration 生产反馈不收敛到真实比例

## 问题

生产路径把已经乘过当前 factor 的 estimate 传给 calibrate，calibrate 又把 actual/adjusted 当新的绝对 factor 做 EMA；真实比例 2 会趋向 sqrt(2)。

## 触发条件与影响

长期 usage 回灌后 token budget 仍系统性低估/高估，且 message-only estimate 未含 tool schema，可能影响 compaction 与 context window 决策。

## 证据

`echo-core/src/tokenizer.rs`、`src/agent/react/run/phases/think.rs` 与 calibration tests/smoke 展示生产参数与测试不一致。

## 处理记录

Time-lifecycle Audit 确认；后续 repair 使用 base estimate 或更新相对因子，并补生产反馈迭代测试。
