---
schema_version: 1
id: finding.canonical-root-system-refresh
kind: finding
type: authority_conflict
status: resolved
severity: high
primary_focus: state_authority
focus: [data_durability, time_lifecycle, contract_evidence]
boundary_ref: boundary.context-memory
behavior_refs: [behavior.context-memory-lifecycle]
rule_refs: [rule.context-persistence-separation]
evidence_refs: [evidence.compacted-context-projection-repair]
audit_refs: [audit.context-memory.data-durability]
decision_refs: []
repair_evidence_refs: [evidence.compacted-context-projection-repair]
verification_evidence_refs: [evidence.compacted-context-projection-repair]
rereview_audit_refs: [audit.context-memory.data-durability]
discovered_at: f9906007599a9c9842474a65f4d8cb5a8a5228b8
---

# Root system refresh overwrites an independently owned System message

## 问题

The real managed Horizon control correctly persisted a model view whose first
System message was its Global Objective and whose second System message was
the canonical root prompt. A new ReactAgent resumed that checkpoint, then its
working-directory refresh replaced the first System message with the root
prompt, losing the objective and leaving two root prompt messages.

This is an independent framework invariant from retained transcript ordinals.
The ordinal repair already passed its exact before/after regression. The new
five-case control run had three passes and two failures: this actual restore
violation and a separate observer-count fixture mistake. Its raw is 3,403 bytes,
SHA-256 `4e3fbcbcfbdc820440017b2cc1aff116c63c7cad4d9db7a0f8e37adb67066280`.
Source/raw/binary are frozen under ignored
`.echo-semantic/local/transcript-compaction-projection/before-root-*`.
No SDK forwarding error or compensation is implicated.

## 触发条件与影响

ReactAgent construction already supplies CanonicalContext.system_prompt.
ContextManager reinjection matches that exact canonical prompt rather than
assuming that the first System message owns the root role. In contrast,
refresh_root_system_prompt and the Agent trait setter replace the first System
message; ContextManager.update_system has the same assumption for skill and
async prompt updates. System summaries, objectives and canonical roots can
legitimately coexist. Role or a summary header is insufficient identity.

The repair should reuse the existing canonical root and update_system path,
preserve independently owned System payloads, reuse an already present desired
root, and retain the standalone no-canonical update contract. Changing cwd
refresh timing, disabling Horizon/Global Objective or weakening restore equality
would hide the violation. No second root state, Store or SDK protocol field is
needed. This Finding remains open pending design review, repair and actual
focused verification.

## 证据

After the occurrence-position repair passed, the same AgentRoot sequence was
strengthened to check existing override/base priority. The current getter kept
the trait override, but cwd refresh rendered the raw config base; async base
updates and skill injection could likewise select the wrong root, while reset
skipped the common harness layers. The existing borrowed getter is base-only,
`current_system_prompt` is override-first, and only replace clears the override.
Natural before failed at the cwd root-value assertion (1,035 bytes /
`2330a30cdb542ff8969f3b82470896b20bda93235e332e014a280f722fb62a95`);
its later sequence assertions had not executed. This is a second subcause of
the same canonical-root authority invariant, not another ordinal/SDK defect.
The existing position qualification remains limited to its original source cut.

## 处理记录

The repair source is `source:b2639597dfa94b7e16aa312e46db6585d9df7636d6340c004ae978f18711d0f1`. The current Evidence records the natural
before, successful focused after/controls and independent C0/I0/M0 source/runtime
rereview. Earlier open/design-pending statements above describe the discovery
epoch; this resolution is limited to this framework invariant. It does not claim
full engineering gates, refreshed SDK consumption, website sync or main delivery.
