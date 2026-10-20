---
schema_version: 1
id: finding.compacted-transcript-retained-ordinals
kind: finding
type: authority_conflict
status: resolved
severity: high
primary_focus: data_durability
focus: [state_authority, time_lifecycle, failure_concurrency, contract_evidence]
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

# Retained context messages acquire new transcript ordinals after compression

## 问题

In the same managed conversation and runtime generation, a durable checkpoint
contains user, assistant tool call, tool result and final response at ordinals
0–3. A valid compressor retains the first three messages and evicts only the
final response. The next explicit checkpoint assigns the retained messages
ordinals 4–6 and appends them again, although the tool executed only once.
The original SDK live-components observation is a consumer before observation;
its passing runtime assertion did not assert absence of duplicate transcript rows.

## 触发条件与影响

`TranscriptProjectionCursor::assign` in `src/agent/snapshot.rs` only matches the
previous suffix against the current prefix. Retained prefix/interior windows
are mistaken for new messages. The managed coordinator restores its cursor from
the durable checkpoint, so calling in-memory realignment alone cannot repair
this. Its no-new-message branch must also persist the realigned existing cursor
through the original checkpoint CAS before publishing it in memory.

ContextManager owns the retained model window, RuntimeStateStore owns the
generation cursor/checkpoint, and ConversationStore owns append-only transcript
facts. Stable normalized fingerprints and generation-local ordinals remain the
existing authority; no global content deduplication, new Store or receipt is
required. This invariant differs from the already resolved generation-identity
and prepare/apply/ack settlement Findings.

## 证据

The prepared framework regression uses ReactAgent, real FileConversationStore
and FileRuntimeStateStore, force compression and force checkpoint, then reopens
the stores. Required outcome: durable transcript remains four unchanged rows,
runtime retains three, cursor retains 0/1/2 with next ordinal 4, and a later
genuinely new turn with repeated text receives ordinals 4/5. Existing repeated
suffix and generation-reset behavior must remain valid. Checkpoint failure may
not publish the candidate cursor before its existing CAS settles.

The corrected framework regression naturally failed on unchanged f990 production
source: one test failed when four durable rows became seven, with retained
ordinals 0/1/2 copied to 4/5/6. Source guard showed no drift and no resource
interruption. Its current before raw is 6,049 bytes, SHA-256
`0a0f69a3042c5b1550798bcb1a10c5acd7668b7768a9c1118906fccca17cec51`;
frozen source and binary remain in ignored
`.echo-semantic/local/transcript-compaction-projection/`.
The assertion preceded later runtime/new-turn controls, so those controls have
not passed yet. The separate first run (15,731-byte raw) also showed this real
violation before an unexecuted postcondition parser mistake was corrected;
its original source/raw/binary are retained without replacing current evidence.

Independent design review rejected a contiguous-window-only repair: built-in
Horizon and L1 can rewrite interior context while retaining later original
facts. A plain LCS assigning new ordinals to interior summaries would violate
existing strict ordinal ordering. An explicit context-view provenance extension
within the existing checkpoint/cursor is under review; no production repair has
been applied. This Finding remains open. SDK refresh, full framework gates and
main delivery remain separate obligations.

## 处理记录

The repair source is `source:b2639597dfa94b7e16aa312e46db6585d9df7636d6340c004ae978f18711d0f1`. The current Evidence records the natural
before, successful focused after/controls and independent C0/I0/M0 source/runtime
rereview. Earlier open/design-pending statements above describe the discovery
epoch; this resolution is limited to this framework invariant. It does not claim
full engineering gates, refreshed SDK consumption, website sync or main delivery.
