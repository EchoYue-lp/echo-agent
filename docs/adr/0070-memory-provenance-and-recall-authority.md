# ADR 0070: Evidence-Bound Memory Activation and Recall

- Date: 2026-09-24
- Owners: `echo-core::memory`, `src::evolution`, `src::agent::react`

## Status

Accepted

## Context

Pre-compaction LLM extraction combined user, assistant, and tool messages in
one prompt, then wrote model-produced facts as `L3Promotion + Active`.
The eviction promoter also wrote its heuristic extracts as Active. The shared
`MemoryRecaller` excluded only Superseded records, so an unsupported assertion
could enter later model context and accumulate recall counts. Source mechanism,
speaker trust, exact evidence, and an approval decision were conflated.

[ADR 0065](0065-evolution-memory-audit-reconciliation.md) already makes
`MemoryLayerManager` the durable owner of warm Store, hot `MEMORY.md`, and
the memory operation journal. [ADR 0008](0008-canonical-runtime-task-authority.md)
keeps task plans outside conversation checkpoints. Neither authority should be
duplicated for memory review.

Claude Code distinguishes human-written project instructions from agent-written
[auto memory](https://code.claude.com/docs/en/memory), and describes memory as
context rather than enforced configuration. LangGraph separates thread
[checkpoints](https://docs.langchain.com/oss/python/langgraph/add-memory) from
cross-thread long-term [Stores](https://docs.langchain.com/oss/python/langgraph/stores).
The OpenAI Agents SDK places
[tool guardrails](https://openai.github.io/openai-agents-python/guardrails/)
at the execution boundary rather than trusting a model's description of tool
output. These are design inputs, not a claim that another product has this exact
Draft/Active schema.

## Options

1. Keep automatic Active writes and strengthen the extraction prompt. Rejected:
   the model can still misattribute a tool quote or invent an approval.
2. Add a separate candidate database and recall resolver. Rejected: it would
   create another state authority and duplicate ADR 0065 recovery semantics.
3. Rewrite or delete historical records lacking provenance. Rejected: public
   File/SQLite Store data remains useful for inspection, and destructive
   migration is unnecessary.
4. Extend the existing typed memory record, keep automatic writes as Draft,
   and activate only an exact journal-bound proposal. Chosen.

## Decision

`MemorySource` continues to describe the extraction mechanism.
`MemoryMeta.provenance` records source roles, verbatim excerpts, derived trust,
and an optional caller-owned `MemoryApproval`. The framework checks each
pre-compaction LLM quote against the original message with the claimed role;
user preferences require user evidence. The heuristic promoter carries
excerpts from its actual user, assistant, or tool message. Trigger, layered
remember, and Background Review candidates use the same Draft contract.
Framework context projections, Hook/runtime notes, Horizon summaries, and
compression placeholders are excluded as source evidence even when represented
as User or Assistant messages. A failed Horizon promotion restores the original
conversation before returning its error.
Evidence with a secret or instruction-like content is rejected before the
manager writes it. The security verdict's risk is persisted, so tool or mixed
origin cannot silently appear low-risk.

`MemoryLayerManager::write_memory` only proposes a Draft, even if a caller
supplies Active or an approval in metadata. It preserves an already approved
record when the same content is extracted again. A distinct replacement
becomes a new Draft and starts a fresh recall telemetry lifetime.
`preview_activation` returns the exact content, metadata, key, and latest
operation-journal generation. An embedding host or reviewer supplies an
approval identity. `activate_draft` checks the snapshot and generation under
the existing manager lock, prepares the mutation in ADR 0065's journal, writes
the Active projection, records business audit, and settles. A concurrent edit,
including an A-to-B-to-A value cycle, fails as stale. A retry returns
`AlreadyActivated` only while the original activation (and its direct hot
promotion, if any) remains the latest lineage for that key. A cancelled or
uncertain write is reconciled through the same journal after restart.

`MemoryMeta::is_recallable` is the single eligibility rule: only explicitly
approved Active and Archived typed memories qualify. A UserPreference also
requires actual User evidence, even when an older record claims approval.
The automatic ReAct path asks its layer manager to settle pending operations,
loads all approved Hot entries, then calls the shared `MemoryRecaller` for
relevant Warm entries. Layered tool search uses that recaller; the Store-backed
Agent tools use it too. Without a manager, the Agent registers only read-only
`recall` and `search_memory`. Installing a manager adds journaled `remember`
and `forget`. Manager installation also binds `ReactAgent::store()` to that
manager's Store. Store replacement returns an error while a different manager
owns memory; a busy context rejects synchronous installation before publishing
any tool or Store change. Direct raw Store KV access remains an independent
caller API.
Hot content and hot search apply the same eligibility check. Candidate search
expands past Draft-heavy result windows, re-reads each candidate before
publication, and updates recall telemetry with Store compare-and-put so a
stale search cannot overwrite a concurrent status or provenance edit.
Dreaming ignores Draft and unverified entries regardless of recall count.

Old typed records and old hot frontmatter without provenance decode with
`LegacyUnknown`. They remain available through explicit Store or manager
inspection, but are absent from automatic and layered recall until a new
evidence-bearing Draft is reviewed. Raw `Store` APIs remain general-purpose
key-value operations; callers that expose their values directly are responsible
for that separate presentation policy.

## Consequences

The framework owns evidence validation, durable status transitions, and recall
eligibility. The embedding host owns who may issue an approval; the framework
cannot authenticate a self-described reviewer string. `MemoryApproval` is an
explicit caller receipt, not a permission grant inferred from LLM confidence.
No second Store, SQLite migration, task state, or approval UI is introduced.

An external writer holding a raw Store can still bypass the layered manager's
journal. As in ADR 0065, settled reads require the manager; direct raw Store
reads are not a claim of reviewed memory. Unsupported Store CAS means recall
telemetry can fail, but it cannot replace the business memory value. Telemetry
failure does not turn a Draft into Active.

## Verification

Focused tests cover exact and forged evidence, mixed roles, secret-bearing
evidence, Draft and legacy recall exclusion, explicit activation and retry,
ABA/stale fences, cancellation and audit failure recovery, File/SQLite restart,
hot provenance round trip, candidate saturation, and concurrent telemetry
changes. Public docs, executable examples, feature compilation, workspace
gates, and independent rereview are required before Issue #76 closes.
