---
schema_version: 1
id: audit.tokenizer-calibration-feedback-rereview
kind: audit
boundary_ref: boundary.llm-provider-runtime
lens: contract_evidence
freshness: examined
revision: 07e4270380c40df3f412f99c0aecb6145410cb2a
finding_refs: [finding.tokenizer-calibration-feedback-convergence]
challenges:
  full-request-denominator:
    revision: 07e4270380c40df3f412f99c0aecb6145410cb2a
    source_refs: [echo-core/src/tokenizer.rs, src/agent/react/run/phases/think.rs]
    evidence_refs: [evidence.tokenizer-calibration-feedback-repair, evidence.tokenizer-calibration-feedback-verification]
  comparable-media-feedback:
    revision: 07e4270380c40df3f412f99c0aecb6145410cb2a
    source_refs: [src/agent/react/run/phases/think.rs, echo-state/src/compression/mod.rs]
    evidence_refs: [evidence.tokenizer-calibration-feedback-verification]
  schema-compaction-draft-flush:
    revision: 07e4270380c40df3f412f99c0aecb6145410cb2a
    source_refs: [src/agent/react/run/context.rs, src/agent/react/run/phases/compact.rs, echo-state/src/compression/mod.rs]
    evidence_refs: [evidence.tokenizer-calibration-feedback-verification]
---

# Tokenizer calibration independent rereview record

## 审查范围

An independent reviewer examined the original candidate based on
`main@9abdf9de` and its #100 source diff. A separate incremental rereview
examined merge HEAD `a96ae92a` with parents `2191553d` and `d7c6aff4`,
the refreshed `5e9a01f4…` semantic snapshot, and the new evidence files.
It confirmed the eight-file #100 source diff was unchanged and #53/#101
mainline content was retained; its conclusion was PASS, 0 actionable findings.

## 已检查故障假设

- A calibrated, message-only denominator may converge to the wrong factor
  when provider usage counts tools and response-format schemas.
- Provider-specific image or file cost may pollute the text factor, or be
  multiplied by that factor in admission while ContextManager keeps it fixed.
- Format-only compression may skip both preparation and Draft-memory flush
  when preflight ignores schema overhead.

## 实际实现路径与证据

The reviewer followed `ChatRequest` creation, the normalized usage feedback,
the shared `ContextManager` budget rule, and real Agent/Draft regressions.
The final pre-integration review returned PASS with no actionable findings
after the shared overhead preflight and Draft regression were added.

## 问题记录

The initial review blocked on
two Important findings: fixed image costs were scaled with the text factor
and image usage polluted the feedback ratio; a large response-format schema
entered think admission but not the pre-compaction allowance. After focused
red/green repair, a second review confirmed those two boundaries and found
one Important Draft-memory flush gap: `should_compress()` omitted the same
schema overhead, so format-only compression skipped extraction.
The integrated incremental rereview found no new blocker, conflict index,
or conflict marker. It checked strict-snapshot, change-evidence, and focused
test/lint receipts supplied by the implementation; it did not execute the
full workspace gate itself.

## 残余风险

The pre-integration reviewer inspected the source candidate represented by
commit `2191553d2e67130f2e98592e69f35dcfc21c63e9`. The incremental
reviewer separately checked the merge snapshot
`a96ae92a86864449f47e02cc5e0048041fce0337` and semantic refresh.

## 未检查项

The independent reviewers did not execute the full workspace gate or
feature matrix. Separate local command receipts prove those gates passed;
remote CI, mainline delivery, and GitHub Issue closure remain pending.
