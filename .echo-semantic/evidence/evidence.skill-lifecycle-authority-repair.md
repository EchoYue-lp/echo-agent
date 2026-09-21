---
schema_version: 1
id: evidence.skill-lifecycle-authority-repair
kind: evidence
observed_at: source:4887582b3c8c982732d721189145bdf28cbd3a06ce0881a785706fc514d3c6e7
source_refs:
  - src/evolution/skill_mutation.rs
  - src/evolution/curator.rs
  - src/evolution/draft.rs
  - src/evolution/merge.rs
  - src/evolution/patch.rs
  - src/agent/snapshot.rs
  - docs/adr/0069-skill-lifecycle-mutation-authority.md
supports: [finding.evolution-skill-promotion-audit, finding.evolution-changelog-rollback-authority, behavior.eval-evolution, rule.quality-observation-boundary]
limitations:
  - Candidate Store payload remains under the independently delivered ADR 0068 CAS journal; Issue 94 is resolved
  - Rule persistence and rollback are host-owned
---

# Skill lifecycle authority repair candidate

## 支持的结论

Draft, Merge, Patch, Curator lifecycle metadata, exact SKILL.md bytes, and
runtime usage now converge on `SkillMutationAuthority`. Digest-bound one-use
approval precedes durable prepare; projection, idempotent ChangeLog append,
settlement, restart reconciliation, generation CAS, and later inverse share one
owner. A reserved ChangeLog marker, created and re-read under the shared journal
serial, binds the canonical durable audit destination without trusting a caller
ID. A copied marker at another path and a ChangeLog without durable identity
both fail closed. The reserved marker key is rejected from business mutation
entity keys before journal prepare, preventing duplicate marker poisoning.
Business audit envelopes expose approval and rollback lineage while exact bytes
remain private. Former public Curator mutation methods are no longer public and the old
best-effort compensation loops no longer execute. Runtime usage cannot create
an unknown Active skill. The authority binds one ChangeLog destination at
construction and rejects rebinding, keeps rollback resolution and inverse
prepare under one journal serial, reconciles unsettled retry before returning
AlreadyApplied, and merge-CASes only affected Curator entries so an unrelated
candidate insert is preserved. Canonical paths and UTF-8 validation prevent
resource aliases; exact bytes stay private while business audit summaries are
bounded and secret-redacted. Rule rollback is typed HostOwned.

## 来源与范围

ADR 0068 remains the candidate Store payload owner and preserves its Store CAS
and Curator lineage. ADR 0069 owns lifecycle metadata and SKILL.md resources.
The repair does not introduce a memory/skill/rule aggregate state machine.

## 已知缺口

The candidate passed final independent rereview, complete local gates and PR #147 CI, then entered
remote main at GitHub verified commit `37b6908c`. Host UI and policy plus Rule persistence remain
outside this framework authority. Issue #94 is resolved and its candidate authority remains independent.
