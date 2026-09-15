---
schema_version: 1
id: finding.diagnostic-persistence-failure-visibility
kind: finding
type: intent_gap
status: open
severity: high
primary_focus: data_durability
focus: [contract_evidence, failure_concurrency, state_authority]
boundary_ref: boundary.observation-persistence-delivery
behavior_refs: [behavior.observation-persistence, behavior.effect-permission-execution]
rule_refs: [rule.fact-projection-separation]
evidence_refs: [evidence.persistence-observation, evidence.effects-extensions, evidence.diagnostic-persistence-failure-visibility-repair]
audit_refs: [audit.observation-persistence-delivery.data-durability]
decision_refs: []
repair_evidence_refs: [evidence.diagnostic-persistence-failure-visibility-repair]
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Trace 与 Audit 持久化失败缺少统一可见结果

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/46

## 问题

RunStore 默认 append 对缺失 run 返回成功，trace 初始 save 失败仍返回 run ID，多条 trace/audit callback 丢弃 backend error，FileAuditLogger 仅 flush 无 durability barrier/torn-tail recovery。

## 触发条件与影响

业务运行可成功而诊断/审计记录部分或全部缺失，且调用方没有结构化 failure fact 判断观察证据是否完整。

## 证据

`src/trace/mod.rs`、`src/agent/react/mod.rs`、`src/agent/snapshot.rs`、`echo-state/src/audit/mod.rs` 与 `audit/file.rs` 展示错误处理。

## 处理记录

Data-durability Audit 确认；本 Finding 与secret retention、InMemory audit成功丢写和tool terminal authority分离。当前repair候选已明确direct Store/Logger Result、diagnostic delivery与best-effort telemetry三层，并通过独立静态复审；工程focused验证、verification Evidence和最终revision rereview尚未完成，因此保持open。
