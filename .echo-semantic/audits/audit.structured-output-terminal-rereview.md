---
schema_version: 1
id: audit.structured-output-terminal-rereview
kind: audit
boundary_ref: boundary.llm-provider-runtime
lens: result_side_effect
freshness: examined
revision: source:bd676c53ff6a431d3ef88581ad7a92a4db718dad7e88dd872b64e20170f61b93
finding_refs: [finding.structured-output-main-path, finding.structured-output-schema-validation-contract]
challenges:
  request-format-authority:
    revision: source:bd676c53ff6a431d3ef88581ad7a92a4db718dad7e88dd872b64e20170f61b93
    source_refs: [src/agent/snapshot.rs, src/agent/react/run/phases/think.rs, src/testing/mock_llm.rs]
    evidence_refs: [evidence.structured-output-main-path-repair, evidence.structured-output-main-path-verification]
  final-success-before-observers:
    revision: source:bd676c53ff6a431d3ef88581ad7a92a4db718dad7e88dd872b64e20170f61b93
    source_refs: [src/agent/react/run/stream_channel.rs, src/agent/react/run/phases/tools.rs, src/agent/react/run/phases/finalize.rs]
    evidence_refs: [evidence.structured-output-schema-validation-repair, evidence.structured-output-schema-validation-verification]
---

# Structured output independent terminal rereview

## 审查范围

Independent reviewers examined ADR 0045 DU-68/DU-97, ADR 0078/0079, the
complete #96/#97 implementation diff and its integration with #40 Channel
projection and #57 Guard directions. Reviews covered provider hints, model
fact freshness, one-shot extraction, text/tool terminal paths, Critic,
callbacks, Trace, paired Store projection, cancellation, and accepted steer.

## 已检查故障假设

- A provider hint could be silently omitted or accepted on an unknown model.
- A strict-invalid final value could be published before typed validation,
  or Output/ToolOutput Guard could transform a valid value afterward.
- A Critic could mask schema failure or wait through cancellation; tool
  batches could emit correction notes before all results or select an invalid
  peer before a later valid answer.
- Stop hooks and final interventions could persist unissued assistant text
  when the run actually failed or was cancelled.

## 实际实现路径与证据

The final independent implementation review returned PASS with no remaining
Critical or Important finding. Each discovered counterexample was reproduced
and repaired on the actual caller path. The run snapshot is the format/fact
authority; the driver validates Guard-processed final candidates before
success observations. Tool batches settle before candidate selection; the
driver owns steer/cancel safe points and typed exhaustion. The final candidate
diff was reviewed before the complete merge gate, which then exited zero on
the same execution source.

## 问题记录

Earlier review rounds blocked post-terminal `execute_typed` validation,
malformed JSON retry regression, Text/Anthropic compatibility, pre-loop
Guard/Hook false-success, missing checkpoint failure reasons, Critic strict
bypass, and #57 integration order. Follow-up reviews found and closed
tool-batch ordering, cancellation, Stop continuation, and intervention
transcript counterexamples. The final round reported no new blocker.

## 残余风险

Provider tokens and tool-result events remain provisional before final
schema validation. External `$ref` resolution is intentionally rejected.
The optional Critic retains its own fail-open error policy, but cannot make
a strict-invalid main Agent answer successful. Remote CI and mainline
delivery remain separate obligations.

## 未检查项

The independent review did not itself run the complete all-feature workspace
gate or per-feature matrix; the delivery owner ran both after review. Real
remote providers, PR CI, and EKO application surface were not checked.
