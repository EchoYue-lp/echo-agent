---
schema_version: 1
id: evidence.skill-lifecycle-authority-verification
kind: evidence
observed_at: source:3a1ceacf4f7e1214698abf2ea09cc3217e426b87fadb0988c97926eb9dcd2bfa
source_refs:
  - src/evolution/skill_mutation.rs
  - src/evolution/draft.rs
  - src/evolution/merge.rs
  - src/evolution/patch.rs
  - src/agent/snapshot.rs
  - echo-agent-learning/tests/example_contracts/demo51_self_improvement.rs
supports: [finding.evolution-skill-promotion-audit, finding.evolution-changelog-rollback-authority, behavior.eval-evolution]
limitations:
  - Remote-main delivery and post-merge closure remain pending
  - Deterministic restart tests do not simulate physical power loss
---

# Skill lifecycle authority verification candidate

## 支持的结论

Focused tests cover approved exact file/Curator mutation, idempotent request
retry, approval mismatch, security-before-prepare, external edit refusal,
audit-failure restart reconciliation, non-tip ABA rollback fencing,
rollback-of-rollback, same-stem path isolation, observer-after-settlement,
typed Rule HostOwned, Draft/Merge/Patch approved paths, and existing/unknown
runtime usage behavior. Reviewer regressions additionally cover audit
destination rebinding, all prepared-stage restarts, unsettled inverse retry,
rollback/newer-write serialization, candidate/file interleaving, relative and
symlink aliases, invalid UTF-8, and secret removal without ChangeLog leakage.
Concurrent same/different destination first-open, copied-marker rejection,
unsupported destination rejection, and ChangeId queries prove one reserved
marker bound to the canonical destination plus machine-readable
Promote/Merge/Patch approval and inverse target lineage. A malicious business
request using the reserved marker key leaves operation history and business
audit empty, after which the same destination reopens with one marker.

## 来源与范围

Command receipts are produced in the isolated
`feature/Echoyue/issue-52-skill-rollback-closure` worktree.
After the fourth review repair, `cargo test -p echo_agent evolution:: --lib
--locked` passed 209/209; the authority suite passed 24/24;
`example_contracts` passed 19/19 and `documentation_contract` passed 12/12.
Both focused Clippy commands, no-default lib check, formatter/diff check, and
strict semantic snapshot plus change-evidence verification exited zero. After
merging latest main, complete `./scripts/verify.sh` and all 17 independent
feature checks passed. Four implementation review rounds plus advancing-base
review closed all findings with 0 Critical, 0 Important and 0 Minor remaining.

## 已知缺口

Remote-main delivery and post-merge closure remain pending. Physical power-loss
injection and a concrete host Rule persistence owner remain outside this candidate.
