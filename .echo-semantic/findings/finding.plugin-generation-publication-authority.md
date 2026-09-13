---
schema_version: 1
id: finding.plugin-generation-publication-authority
kind: finding
type: authority_conflict
status: open
severity: high
primary_focus: state_authority
focus: [failure_concurrency, time_lifecycle, contract_evidence]
boundary_ref: boundary.extension-lifecycle
behavior_refs: [behavior.extension-publication]
rule_refs: [rule.extension-generation-authority]
evidence_refs: [evidence.effects-extensions]
audit_refs: [audit.extension-lifecycle.state-authority]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Plugin wiring 缺 active generation authority

## 问题

PreparedPluginSet 有 generation/identity，但 PluginWiringResult 不携带 generation，Integrator 不记录 active generation；旧 Prepared Arc 可在新代后发布，旧 receipt 也可撤销当前资源。

## 触发条件与影响

并发 prepare/apply/unwire 或持有旧 handle 的 caller 会让 stale generation 覆盖/撤销新 generation，破坏 Plugin publication authority。

## 证据

`src/plugin/prepared.rs` 的 cache、wire result、apply/unwire 与 tests 展示缺少 generation fence。

## 处理记录

State-authority Audit 确认；后续 repair 需 active generation CAS 与 stale apply/receipt tests。
