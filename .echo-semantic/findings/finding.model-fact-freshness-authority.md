---
schema_version: 1
id: finding.model-fact-freshness-authority
kind: finding
type: authority_conflict
status: open
severity: medium
primary_focus: state_authority
focus: [contract_evidence, trigger_input]
boundary_ref: boundary.llm-provider-runtime
behavior_refs: [behavior.llm-provider-execution]
rule_refs: [rule.provider-protocol-boundary]
evidence_refs: [evidence.provider-protocol-quality]
audit_refs: [audit.llm-provider-runtime.contract-evidence]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# 内置动态 Model facts 缺 freshness authority

## 问题

Core 硬编码 context window、max output tokens 与 thinking family 并用精确测试锁定，但没有来源版本、更新时间、失效策略或 provider/application override 的统一 owner。

## 触发条件与影响

Provider 更新模型能力后，预算、工具和 thinking policy 可继续使用过期值，且消费者无法判断何时必须覆盖。

## 证据

`echo-core/src/llm/capabilities.rs` 与 model profile 文档展示 catalog/default/override 结构和缺少 provenance。

## 处理记录

Contract Audit 确认；需 semantic-decide 确定允许内置的保守范围与刷新责任。
