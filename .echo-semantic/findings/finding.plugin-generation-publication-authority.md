---
schema_version: 1
id: finding.plugin-generation-publication-authority
kind: finding
type: authority_conflict
status: resolved
severity: high
primary_focus: state_authority
focus: [failure_concurrency, time_lifecycle, contract_evidence]
boundary_ref: boundary.extension-lifecycle
behavior_refs: [behavior.extension-publication]
rule_refs: [rule.extension-generation-authority]
evidence_refs: [evidence.effects-extensions]
audit_refs: [audit.extension-lifecycle.state-authority, audit.plugin-generation-publication-authority-rereview]
decision_refs: []
repair_evidence_refs: [evidence.plugin-generation-publication-authority-repair]
verification_evidence_refs: [evidence.plugin-generation-publication-authority-verification, evidence.foundation-36-72-51-integration-verification]
rereview_audit_refs: [audit.plugin-generation-publication-authority-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Plugin wiring 缺 active generation authority

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/72

## 问题

PreparedPluginSet 有 generation/identity，但 PluginWiringResult 不携带 generation，Integrator 不记录 active generation；旧 Prepared Arc 可在新代后发布，旧 receipt 也可撤销当前资源。

## 触发条件与影响

并发 prepare/apply/unwire 或持有旧 handle 的 caller 会让 stale generation 覆盖/撤销新 generation，破坏 Plugin publication authority。

## 证据

`src/plugin/prepared.rs` 的 cache、wire result、apply/unwire 与 tests 展示缺少 generation fence。

## 处理记录

主线 `cb4ee9ed` 在每个 ReactAgent 内绑定唯一 target authority，并增加 process-wide
generation、receipt token、stale/foreign/altered receipt、独立 Integrator、取消与 cleanup debt
测试。repair、verification、独立 rereview、完整工程门禁、PR #138 七项 CI 与远端主线交付
均已闭合。MCP owner isolation 与统一 plugin coordinator 分别继续由 #75 与 #73 负责。
