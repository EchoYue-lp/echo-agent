---
schema_version: 1
id: audit.guard-direction-contract-rereview
kind: audit
boundary_ref: boundary.tool-permission-sandbox
lens: result_side_effect
freshness: examined
revision: source:bd676c53ff6a431d3ef88581ad7a92a4db718dad7e88dd872b64e20170f61b93
finding_refs: [finding.guard-direction-contract]
challenges:
  tool-result-observation:
    revision: source:bd676c53ff6a431d3ef88581ad7a92a4db718dad7e88dd872b64e20170f61b93
    source_refs: [src/agent/react/run/pipeline.rs, src/agent/react/run/phases/tools.rs, src/agent/snapshot.rs]
    evidence_refs: [evidence.guard-direction-contract-repair, evidence.guard-direction-contract-verification]
  final-text-guard:
    revision: source:bd676c53ff6a431d3ef88581ad7a92a4db718dad7e88dd872b64e20170f61b93
    source_refs: [src/agent/react/run/phases/think.rs, src/agent/react/run/phases/finalize.rs]
    evidence_refs: [evidence.guard-direction-contract-repair, evidence.guard-direction-contract-verification]
---

# Guard direction independent rereview

## 审查范围

Independent reviewers inspected the GuardManager direction authority, real
tool caller, streaming event channel, callbacks, Trace, Audit, transcript,
model context, final text, and the post-use Hook boundary. The final reviewed
candidate was `9c2cebff`; the later `a136a424` increment only corrected ADR
retry wording without changing execution behavior.

## 已检查故障假设

- Raw stream chunks or error-only diagnostics could escape before ToolOutput.
- Structured data, metadata, rich content, and simultaneous output/error
  could bypass text-only checks and reach caller or model context.
- PostToolUse block reasons and ToolFailure free-text recovery fields could
  escape independently of guarded `ToolResult.error`.
- ToolInput rewriting could invalidate the exact approval receipt; Output
  could be unreachable or leak final text tokens before inspection.

## 实际实现路径与证据

PASS with no remaining Critical or Important finding. ToolInput checks final
effective args without transformation, ToolOutput checks each caller-visible
text projection and suppresses opaque rich projections, and Output guards
model final text and partial provider-failure text before token publication.
Guarded tool streaming withholds raw chunks until one guarded terminal result;
no-Guard live streams remain unchanged. Confirmed typed effect/path facts
remain governed by ADR 0074, separate from freely supplied output text.
Focused pipeline 40/40, tool publishing 6/6, and stream-channel 62/62 tests
passed on the candidate. The main merge added #40 Channel projection without
overlapping Guard execution code.

## 问题记录

Two review rounds found and fixed raw streaming/error-only and structured
result/error bypasses. A later increment fixed post-use block reason and
ToolFailure free-text bypasses. The final review found only an inaccurate ADR
retry sentence; `a136a424` narrowed it to possibly effectful retries.

## 残余风险

User-installed PostToolUse/Failure Hooks run before ToolOutput Guard and may
observe raw tool facts as trusted extensions. ADR 0074 typed effect/path
diagnostic facts remain visible to caller and Trace; this Guard does not claim
generic redaction of those facts. Full gates, remote CI, and mainline delivery
remain separate acceptance steps.

## 未检查项

Independent review did not run the full workspace gate, feature matrix,
remote CI, or EKO application policies.
