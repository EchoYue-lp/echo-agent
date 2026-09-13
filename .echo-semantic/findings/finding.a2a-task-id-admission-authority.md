---
schema_version: 1
id: finding.a2a-task-id-admission-authority
kind: finding
type: authority_conflict
status: open
severity: high
primary_focus: state_authority
focus: [failure_concurrency, time_lifecycle, contract_evidence]
boundary_ref: boundary.protocol-surfaces
behavior_refs: [behavior.protocol-projection, behavior.agent-turn-lifecycle]
rule_refs: [rule.protocol-role-separation, rule.turn-terminal-authority]
evidence_refs: [evidence.provider-protocol-quality]
audit_refs: [audit.protocol-surfaces.state-authority, audit.protocol-surfaces.time-lifecycle]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
---

# A2A 重复 task ID 缺 admission generation authority

## 问题

Sync/stream入口接受 client task ID并无条件覆盖 tasks/cancel_tokens；旧执行仍按同 key更新新task、追加旧输出并删除新 cancel token。

## 触发条件与影响

两个同 ID执行并发时，新执行的状态、取消和history可被旧generation覆盖，TaskState合法迁移无法识别代次。

## 证据

`src/a2a/server.rs` 的 admission、map insert、completion和token removal，以及 `src/a2a/types.rs`无generation状态机提供证据。

## 处理记录

Protocol state/time Audit共同确认；后续repair需原子admission或generation fence与duplicate-ID tests。
