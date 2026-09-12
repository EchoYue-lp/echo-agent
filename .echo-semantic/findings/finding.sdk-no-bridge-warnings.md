---
schema_version: 1
id: finding.sdk-no-bridge-warnings
kind: finding
type: evidence_gap
status: resolved
severity: high
primary_focus: contract_evidence
focus: [failure_concurrency, time_lifecycle]
boundary_ref: boundary.sdk-facade-parity
behavior_refs: [behavior.sdk-facade-routing]
rule_refs: [rule.sdk-rust-authority]
evidence_refs: [evidence.sdk-contracts]
audit_refs: [audit.sdk-facade-plan08-final]
decision_refs: []
repair_evidence_refs: [evidence.sdk-contracts]
verification_evidence_refs: [evidence.sdk-contracts]
rereview_audit_refs: [audit.sdk-facade-plan08-final]
discovered_at: source:ccc4d0577720ebe8c3b3229d8ea1c78c7ae7cafd128f2e9bfde196dab612adf2
---

# facade/bridge/improve feature组合未闭合

## 问题

第五轮复审发现open_ephemeral及Eval/Improve helper未按真实feature使用点编译，no-bridge组合产生dead_code告警；后续组合暴露improve未声明eval依赖、bridge-only缺失tokenizer facade authority及factory helper多余编译。

## 触发条件与影响

关闭sdk-extension-bridge、单独启用improve或只启用bridge时，适用编译组合会产生告警、缺失公开类型或无法编译tokenizer回调，阻塞零告警与bridge E2E门禁。

## 证据

方法与helper已按真实feature gate收窄，根improve和Host framework-improve显式包含eval；sdk-extension-bridge显式包含其canonical tokenizer route依赖的facade adapter。最小adapter、CI no-bridge、基础bridge、improve bridge及根improve组合均以RUSTFLAGS=-D warnings通过，bridge-only与full E2E均通过。

## 处理记录

代码和focused验证已完成；A2A-only test helper也按真实feature收窄。CI现在保留Linux lld flags并追加-D warnings，先编译全部bridge test targets，再以明确的--test参数真实执行19个ExtensionBridge E2E。第十轮独立复审确认该finding闭合。
