---
schema_version: 1
id: finding.runtime-token-limit-explicit-publication
kind: finding
type: authority_conflict
status: resolved
severity: medium
primary_focus: state_authority
focus: [time_lifecycle, failure_concurrency, contract_evidence]
boundary_ref: boundary.context-memory
behavior_refs: [behavior.context-memory-lifecycle]
rule_refs: [rule.context-persistence-separation]
evidence_refs: [evidence.compression-token-tail, evidence.model-facts-authority-verification]
audit_refs: [audit.model-facts-freshness-rereview]
decision_refs: []
repair_evidence_refs: [evidence.compression-token-tail]
verification_evidence_refs: [evidence.compression-token-tail]
rereview_audit_refs: [audit.model-facts-freshness-rereview]
discovered_at: f3fa0705f62fe813a5a86bc41cd673f82f915a61
---

# Explicit runtime token limit loses precedence at the next run

## 问题

Successful direct and prepared limit setters changed the live Context and config
value without setting the existing explicit-limit marker. The next snapshot
resolved the 128,000-token model profile and the next prepare phase overwrote the
same Context, undoing the caller's successful 100,000-token limit.

## 触发条件与影响

The actual native SDK compressor observed an 80,000-token allowance before a run
and 102,309 plus 91 protected tokens after real completed runs. The expected
effective allowance remains 80,000 after output and safety reservations.
The framework's own natural before regression independently observed 102,400
instead of 80,000 on both direct and prepared publication paths.

## 证据

`src/agent/react/capabilities.rs` owns publication;
`src/agent/config.rs` already supplies the explicit marker;
`src/agent/snapshot.rs` and `src/agent/react/run/phases/prepare.rs` consume it.
The four `token_limit_publication` tests observe the installed compressor before
and after actual Agent turns, with unconfigured and rejected/dropped controls.

## 处理记录

Repair commit `e3ba883a9abaf4e7070185304ccb676c55292144` marks only successful
publications explicit. Busy Context, invalid budget and dropped preparation do
not publish. Reserved/protected accounting and implicit profile budgets remain
owned by the existing ContextManager and invocation safe point.
Focused before/after evidence and independent C0/I0/M0 source review exist;
the integrated framework candidate also passed the current four regressions
and independent final source rereview (C0/I0/M0), including its unchanged
reserved/protected and failure policies. Resolution is limited to this framework
publication invariant; refreshed SDK consumer qualification and main delivery
remain separate. It does not reopen or
claim identity with the older model-freshness/capability-default Findings.


The final local framework source is `source:6e61249f5796e67d8daa2038c750f8452c9c6d41f5e14dcda791405fd2db5d06`;
its five Rust source identities and focused outcomes are retained in the ignored
`live-model-publication` task results. Existing Evidence and the two specific
current Audit challenges record the bounded proof; broad Behavior status is not
promoted to verified.
