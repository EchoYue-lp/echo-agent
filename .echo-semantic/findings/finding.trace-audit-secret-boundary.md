---
schema_version: 1
id: finding.trace-audit-secret-boundary
kind: finding
type: intent_gap
status: open
severity: high
primary_focus: permission_external
focus: [data_durability, result_side_effect, contract_evidence]
boundary_ref: boundary.observation-persistence-delivery
behavior_refs: [behavior.observation-persistence, behavior.effect-permission-execution]
rule_refs: [rule.fact-projection-separation, rule.permission-effect-order]
evidence_refs: [evidence.persistence-observation, evidence.effects-extensions]
audit_refs: [audit.observation-persistence-delivery.data-durability, audit.observation-persistence-delivery.contract-evidence]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Trace 与 audit 没有统一 secret retention 合同

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/103

## 问题

ReactAgent trace 保存 guard 转换后的 effective input，AuditStage 提交完整 tool input；in-memory RunStore/AuditLogger 不做清洗或统一容量限制，只有 JSONL/File 等部分持久 backend 有 retention sanitization。

## 触发条件与影响

用户 prompt 或 tool parameters 含 credential/secret 时，启用 trace/audit 可能原样保留敏感数据，而上层材料若宣称全局脱敏会产生错误安全预期。

## 证据

`src/agent/react/mod.rs`、`src/agent/react/run/pipeline.rs`、`src/trace/mod.rs` 与 `echo-state/src/audit/memory.rs` 展示 producer/backend 行为。

## 处理记录

Discovery 已收窄全局承诺；后续 audit 决定每个 backend 的 redaction、retention 与明确 opt-in contract。
