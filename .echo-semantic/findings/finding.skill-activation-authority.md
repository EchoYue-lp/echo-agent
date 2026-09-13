---
schema_version: 1
id: finding.skill-activation-authority
kind: finding
type: authority_conflict
status: open
severity: high
primary_focus: state_authority
focus: [data_durability, time_lifecycle, contract_evidence]
boundary_ref: boundary.extension-lifecycle
behavior_refs: [behavior.extension-publication]
rule_refs: [rule.extension-generation-authority]
evidence_refs: [evidence.effects-extensions]
audit_refs: [audit.extension-lifecycle.state-authority]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
---

# Skill activation 存在两个状态权威

## 问题

ReactAgent 同时维护主 SkillRegistry 与 progressive registry；API activation、resource/script tools 和 checkpoint restore 读取或写入不同集合。

## 触发条件与影响

API 激活或进程恢复后，prompt 可显示 Skill 已激活，而 resource/script tool 仍返回 not activated，重复激活又可能重复注入指令。

## 证据

`src/agent/react/capabilities.rs`、`src/agent/react/mod.rs` 与 `echo-execution/src/skills/external/resource_tool.rs` 展示两份 activation state。

## 处理记录

Discovery 记录为 authority conflict；下一阶段确定一个 registry 并做 API/tool/checkpoint round trip。
