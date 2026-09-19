# ADR 0068: Reconcile Skill Candidate Mutation with Business Audit

- Date: 2026-09-19
- Owners: `src::evolution`, `echo-state::journal`

## Status

Accepted

## Context

`SkillCandidateDetector` owns two mutations of candidate payloads in
`TypedMemoryStore`: creation after a repeated pattern first crosses the
threshold, and reinforcement when later observations increase
`sample_count` and `confidence`. Creation previously wrote the Store, registered
the candidate with `Curator`, and then appended `ChangeLog`. Reinforcement only
wrote the Store. An audit failure or process stop could therefore expose a
candidate payload without the corresponding durable business fact, and a
reinforcement was never auditable at all.

The three authorities must remain distinct. `TypedMemoryStore` owns the
candidate JSON payload. `Curator` owns the Candidate -> Draft -> Active
lifecycle. `ChangeLog` is the append-only business audit. The
layered-memory `MemoryOperationJournal` cannot be reused because its schema and
reconciliation rules are hard-coded to warm/hot memory projections and memory
rollback lineage.

[ADR 0065](0065-evolution-memory-audit-reconciliation.md) records the relevant
industry basis: the AWS transactional outbox pattern couples a state change to
a recoverable message identity; PostgreSQL WAL durably records intent before
data-page projection; Temporal requires idempotent effects for replay. Candidate
payloads and a file-backed audit cannot share one storage transaction, so this
boundary needs the same durable ordering without pretending to provide atomic
cross-resource visibility.

## Options

1. Append audit before the candidate payload. Rejected because a failed Store
   write would leave an audit fact for a mutation that never became visible.
2. Keep best-effort compensation. Rejected because cancellation or process
   termination can lose the compensation, and a Store error may have an
   unknown outcome.
3. Route candidates through `MemoryOperationJournal`. Rejected because it
   would make Skill lifecycle state look like layered memory and would couple
   this repair to the public memory rollback contract.
4. Give candidate mutation a private durable operation journal that reuses the
   generic `FileEventJournal` primitive. Chosen.

## Decision

Creation and reinforcement use one private candidate mutation owner. Before
changing a payload, the owner records one complete operation containing a
stable operation/change ID, the exact optional before and required after Store
values, the candidate authority lineage, whether candidate lifecycle
registration is required, and the complete `ChangeEntry`. It then projects the
`TypedMemoryStore` value with an atomic compare-and-put, registers a new
candidate lineage with `Curator`, appends the business fact through
`ChangeLog::record_idempotent`, and records settlement.

The generic `Store` contract exposes exact-value compare-and-put with a default
`Unsupported` result. Built-in in-memory, file, and SQLite Stores implement a
real atomic path. `EmbeddingStore` deliberately remains Unsupported: its KV
payload and derived vector index cannot be committed atomically, and an inner
CAS followed by an async index update can be reordered or cancelled. Candidate
mutation refuses wrappers or Stores without the capability. A preflight read remains useful for diagnostics, but
the compare and projection happen under the Store's one write authority, so an
external write between the read and commit cannot be overwritten.

The journal creates one durable candidate `authority_id`. A reserved
TypedMemoryStore marker and a stable ChangeLog marker entry bind the journal to
the concrete payload and audit authorities. First use initializes both markers
before publishing the journal binding; an existing journal requires both exact
markers and never adopts an empty or different Store/ChangeLog. Matching
markers without a journal binding are recoverable after a stop at the final
first-use step. A one-sided marker fails closed because no cross-resource
transaction can prove which missing side is authoritative.

Every detection pass reconciles the journal before scanning observations. A
prepared operation after restart reuses its stored payload and audit ID, so it
can fill a missing audit without duplicating an already committed line. The
owner also verifies settled projections at startup. A current Store value may
equal the recorded before or after value; any unrelated external value fails
closed rather than being overwritten. The same check prevents a stale
reinforcement decision from replacing a newer candidate payload. `Curator`
stores the same authority lineage for each detector-created candidate in a
private sidecar map, leaving the public `SkillMeta` shape source-compatible.
The sidecar is persisted before a missing lifecycle record, so a stop between
the two writes is recoverable. Missing candidates are recreated during
reconciliation; a Draft/Active or later state with matching lineage is a
legitimate lifecycle advance, while an existing same-name skill with missing
or different lineage fails closed. Promotion and approval behavior remains
outside this decision.

Candidate values are compared as canonical typed JSON, including metadata.
Create and reinforcement audit entries include exact before/after candidate
payloads. A detection pass with no increase in observation count prepares no
operation, writes no candidate payload, appends no audit, and publishes no
report item. `CandidateReport::reinforced` and the new-candidate observer are
published only after settlement succeeds. Recovery completes durable state but
does not replay an observer callback, because the observer has no durable
consumer acknowledgement.

The journal path is derived from the consumer-supplied Curator state path, but
the path alone is not treated as Store or ChangeLog identity. The journal is
private implementation state and adds no public candidate rollback API. Every
private path appends its suffix to the complete Curator state path
(`state.json.candidate-operations.jsonl`,
`state.json.candidate-authorities.json`, and `state.json.lock`). Replacing an
extension is forbidden because `state.json` and `state.toml` would otherwise
alias the same private authorities; state temporary files follow the same
injective rule.
Promotion, approval, draft generation, merge, patch, and Rule lifecycle remain
outside this decision and continue under their existing owners and Issue #54.

## Consequences

There is still no atomic read boundary across Store, Curator state, audit, and
the journal. Callers that need settled candidate state must enter through
`detect`, which reconciles before reading or publishing a report. Direct Store
readers can briefly observe a prepared projection. Journal history must remain
available to distinguish known before/after values and to repair prior
operations after restart.

The first detection after upgrading can only reconcile operations created by
this implementation. Existing historical reinforcements that never had an
operation identity cannot be reconstructed retroactively. The semantic Finding
therefore stays open until the candidate reaches remote main and repair,
verification, and independent rereview evidence are complete.

## Verification

Deterministic tests cover create and reinforcement audit parity, stable
idempotent audit replay after failure and restart, Store/ChangeLog authority
rebinding refusal, a write injected between candidate read and CAS, no-growth
zero mutation and audit, external overwrite and stale projection refusal,
missing/matching-promoted/conflicting Curator lineage, an external `SkillMeta`
struct literal, `EmbeddingStore` CAS refusal with unchanged payload/index, and
same-stem/different-extension Curators running sequentially and concurrently
without journal, lineage, lock, or temporary-file aliasing. Report publication
remains post-settlement. The executable demo51 contract and bilingual
self-improvement documentation expose the durable candidate behavior without
introducing public rollback.
