---
schema_version: 1
id: finding.diagnostic-persistence-failure-visibility
kind: finding
type: intent_gap
status: resolved
severity: high
primary_focus: data_durability
focus: [contract_evidence, failure_concurrency, state_authority]
boundary_ref: boundary.observation-persistence-delivery
behavior_refs: [behavior.observation-persistence, behavior.effect-permission-execution]
rule_refs: [rule.fact-projection-separation]
evidence_refs: [evidence.persistence-observation, evidence.effects-extensions, evidence.diagnostic-persistence-failure-visibility-repair, evidence.diagnostic-persistence-failure-visibility-verification, evidence.diagnostic-delivery-current-repair, evidence.diagnostic-delivery-current-verification, evidence.framework-only-finding-closure-verification]
audit_refs: [audit.observation-persistence-delivery.data-durability, audit.diagnostic-persistence-failure-visibility-rereview]
decision_refs: [decision-adr-0053-trace-audit-persistence-visibility]
repair_evidence_refs: [evidence.diagnostic-persistence-failure-visibility-repair]
verification_evidence_refs: [evidence.diagnostic-persistence-failure-visibility-verification, evidence.framework-only-finding-closure-verification]
rereview_audit_refs: [audit.diagnostic-persistence-failure-visibility-rereview]
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

Data-durability Audit 确认；本 Finding 与 secret retention、InMemory audit 成功丢写和
tool terminal authority 分离。Framework main 现明确 direct Store/Logger Result、diagnostic
delivery 与 best-effort telemetry 三层，并让 `RunStore::finalize_run` 与 append 共享 backend
mutation authority。PR #131 及后续 main 增量已通过 focused tests、17 项 feature matrix、完整
framework 门禁、远端 CI 与 revision-bound 独立复审。SDK inventory 属于独立 consumer，不是
本 Finding 或 Issue #46 的关闭条件。
