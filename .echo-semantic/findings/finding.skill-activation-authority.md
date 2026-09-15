---
schema_version: 1
id: finding.skill-activation-authority
kind: finding
type: authority_conflict
status: resolved
severity: high
primary_focus: state_authority
focus: [data_durability, time_lifecycle, contract_evidence]
boundary_ref: boundary.extension-lifecycle
behavior_refs: [behavior.extension-publication]
rule_refs: [rule.extension-generation-authority]
evidence_refs: [evidence.effects-extensions, evidence.skill-activation-authority-repair, evidence.skill-activation-authority-verification]
audit_refs: [audit.extension-lifecycle.state-authority, audit.skill-activation-authority-rereview]
decision_refs: []
repair_evidence_refs: [evidence.skill-activation-authority-repair]
verification_evidence_refs: [evidence.skill-activation-authority-verification]
rereview_audit_refs: [audit.skill-activation-authority-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Skill activation 存在两个状态权威

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/93

## 问题

ReactAgent 同时维护主 SkillRegistry 与 progressive registry；API activation、resource/script tools 和 checkpoint restore 读取或写入不同集合。

## 触发条件与影响

API 激活或进程恢复后，prompt 可显示 Skill 已激活，而 resource/script tool 仍返回 not activated，重复激活又可能重复注入指令。

## 证据

`src/agent/react/capabilities.rs`、`src/agent/react/mod.rs` 与 `echo-execution/src/skills/external/resource_tool.rs` 展示两份 activation state。

## 处理记录

首轮review发现checkpoint telemetry镜像、definition mutation、in-flight publication和doc-hidden inventory四类缺口；第二轮又发现deny replacement先删除旧代。epoch-fenced handle、descriptor-derived policy restore、prevalidate-then-swap Agent reconciliation、single-flight/cancel poison与Host-only classifier逐项修复后，同一reviewer最终复审Critical/Important/Minor均为0，本Finding已关闭。
