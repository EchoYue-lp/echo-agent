# ADR 0069: Canonical Skill Lifecycle Mutation and Rollback Authority

- Date: 2026-09-20
- Owners: `src::evolution::skill_mutation`, `src::evolution::curator`

## Status

Accepted and delivered in PR #147 at GitHub verified main commit `37b6908c`. Post-merge governance
closure is recorded with the same delivered source.

## Context

Candidate detection already has the durable Store CAS and reconciliation owner
defined by ADR 0068. Later lifecycle operations still used unrelated sequences:
Draft wrote `SKILL.md`, changed Curator, then appended audit; Merge and Patch did
the same with best-effort compensation; public Curator mutation methods could
bypass security, approval, audit, and rollback entirely.

The resources have distinct authorities. Candidate payload JSON remains owned
by ADR 0068's `TypedMemoryStore` CAS journal. Curator metadata and exact
`SKILL.md` bytes form the Skill lifecycle resource. `ChangeLog` is append-only
business evidence and cannot safely infer bytes or apply state. Rule persistence
has no framework owner and belongs to the embedding host.

Git `revert` appends a compensating fact rather than erasing history.
Kubernetes `resourceVersion` binds writes to the exact observed generation.
Temporal compensation requires idempotent effects and stable operation
identities. KurrentDB expected version rejects stale stream appends and makes
retained history an explicit precondition. These patterns all separate an
owner-applied inverse from a query log.

## Options

1. Let every component keep its own write/compensate loop. Rejected because a
   crash can lose compensation and each loop defines different conflict rules.
2. Add `ChangeLog::rollback`. Rejected because the log does not own Curator or
   file state, and old entries do not always contain exact restorable bytes.
3. Put memory, candidate, Skill, and Rule into one evolution state machine.
   Rejected because their resources, owners, and product policies differ.
4. Keep candidate Store CAS under ADR 0068, add one canonical lifecycle owner
   for Curator plus `SKILL.md`, and return typed HostOwned for Rule. Chosen.

## Decision

`SkillMutationAuthority` owns every framework Skill lifecycle projection after
candidate discovery. A request contains a stable request ID, mutation kind,
exact optional before/after bytes for every affected `SKILL.md`, and complete
before/after `CuratorState`. Preview performs path/value/state CAS and security
checks before prepare, then returns a SHA-256 digest. The host supplies a
one-use `SkillApprovalArtifact` binding that digest, approval identity,
approver, and timestamp. Missing, mismatched, or replayed approval fails closed.

Construction binds one `Arc<dyn ChangeLog>` to the journal before any prepare.
The authority does not accept a caller-asserted identity: under the shared
journal serial it queries a reserved ChangeLog marker, creates and re-queries a
UUID marker containing the ChangeLog's canonical durable destination identity
on an empty first binding, then requires the journal `Bound` fact to match. A
ChangeLog implementation without a durable identity is unsupported. Apply,
reconcile, rollback, Draft, Merge, Patch, and runtime usage all reuse that
authority instance; no method accepts a per-call audit destination. Reopening
against a different ChangeLog, including a copied marker at another path,
fails before projection or settlement, so pending debt cannot migrate to
another business log. Concurrent first-open attempts cannot append two `Bound`
facts. The reserved marker key is excluded from the public Skill mutation
entity-key space before prepare, so a business request cannot create a second
marker-shaped audit entry or poison later reopen.

Apply records the complete batch in a SyncData `FileEventJournal`, projects all
files, merge-CASes only the affected Curator Skill entries, appends stable
ChangeLog entries idempotently, and settles. Restart reconciliation completes
an unsettled batch without duplicate audit. Observer callbacks occur only after
live settlement.
Draft, Merge, and Patch public apply paths require preview plus approval; their
former best-effort compensation paths are retired. Direct Curator mutation
primitives are crate-private test/adapter surfaces, so external framework users
cannot bypass the owner.

Later rollback targets a retained ChangeLog ID or batch ID. Every file and
changed lifecycle identity must still point to the target's latest journal
generation; this rejects non-tip and ABA values even when bytes compare equal.
Reconcile, request replay, target resolution, tip revalidation, inverse prepare,
projection, audit, and settlement execute under one journal serial critical
section. An unsettled inverse retry reconciles first and returns
`AlreadyApplied` only after its settlement is durable.
The authority creates a fresh inverse batch, so request retry returns the same
receipt, request reuse with different content conflicts, and rollback of a
rollback is an ordinary new inverse. A merge is one batch and rolls back as a
group. Raw external file or Curator edits fail closed.

Candidate creation/reinforcement remains ADR 0068's Store CAS adapter and
preserves its authority markers and Curator lineage. It does not become a
second `SKILL.md` lifecycle writer. Candidate and Skill operations can
interleave on unrelated names because Curator projection compares and merges
only affected entries; an unrelated candidate insert is preserved rather than
turning a file-prepared Skill batch into permanent debt. Rule rollback preview returns
`HostOwned`; the framework never reconstructs Rule state from ChangeLog JSON.
Host approval policy, UI, thresholds, and Rule persistence remain outside the
framework.

File resource identities are canonical absolute paths. Parent traversal,
relative raw requests, duplicate symlink aliases, and invalid UTF-8 `after`
bytes fail before prepare. The private journal retains exact before/after bytes
for recovery. Business ChangeLog entries contain only canonical path, SHA-256,
length, and a bounded security-redacted summary; removing a secret never copies
that secret into the business log, while an approved inverse can still restore
the exact private bytes.

Each business audit carries a machine-readable operation envelope: mutation
kind, request and batch identities, approval id/digest/approver/timestamp, and
for inverses the target batch, requested/contained change IDs, and target
generation. Receipts expose the approval ID alongside their business change
IDs. This lets a ChangeLog consumer prove approval and rollback lineage without
reading private recovery payloads.

## Consequences

Callers must retain the previewed request and supply a matching approval before
Draft, Merge, or Patch application. Curator's former direct mutation API is no
longer public. Journal history must be retained for rollback and generation
proof. There is no atomic visibility across multiple files and Curator JSON;
manager users reconcile before exposing settled state. Direct filesystem
readers can observe an intermediate projection.

Runtime usage now enters through the shared authority-owned `SkillUsageHandle`: it
updates only an existing Skill, never creates Active state for an unknown name,
and uses the same journal/audit/reconcile ordering. Findings #54 and #52 are
resolved by the delivered authority and its post-merge governance closure.

## Verification

Focused tests cover exact forward/rollback bytes and Curator metadata,
approval mismatch and replay, security before prepare, audit failure plus
restart reconciliation, request idempotency, external edit failure, non-tip
ABA fencing, rollback-of-rollback, whole merge application, same-stem state
path isolation, observer-after-settlement, and typed Rule HostOwned behavior.
Draft, Merge, Patch, Curator, candidate, demo51, formatter, Clippy, and strict
semantic change evidence remain required before delivery.
