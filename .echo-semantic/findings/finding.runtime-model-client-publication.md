---
schema_version: 1
id: finding.runtime-model-client-publication
kind: finding
type: authority_conflict
status: resolved
severity: high
primary_focus: state_authority
focus: [result_side_effect, failure_concurrency, contract_evidence]
boundary_ref: boundary.llm-provider-runtime
behavior_refs: [behavior.llm-provider-execution]
rule_refs: [rule.provider-protocol-boundary]
evidence_refs: [evidence.model-facts-authority-repair, evidence.model-facts-authority-verification]
audit_refs: [audit.model-facts-freshness-rereview]
decision_refs: []
repair_evidence_refs: [evidence.model-facts-authority-repair]
verification_evidence_refs: [evidence.model-facts-authority-verification]
rereview_audit_refs: [audit.model-facts-freshness-rereview]
discovered_at: e3ba883a9abaf4e7070185304ccb676c55292144
---

# Runtime model selection does not replace the executing client

## 问题

`ReactAgent::set_model` only changed a config string. Concrete provider adapters
continued to send their own installed model, so a successful-looking update did
not change the model used by the same Agent's next run. The legacy sourced config
setter also changed config/profile before construction and swallowed a build
error, permitting partial publication.

## 触发条件与影响

The native SDK ran two completed real turns on the same Agent/Session around
`set_model(B)`. Original HTTP fixture requests were `[A, A]`, rather than `[A, B]`,
using immutable original Host SHA256
`0982121cb1e99284b4a7d7e07af35037f9f22bd653528d9716d425f5609407a0`.
Its Session accessor also read the boxed wrapper's conversion-time model cache;
that is a separate thin-adapter current-value projection gap, not proof that
ReactAgent's trait getter prioritizes its client.

## 证据

Publication and preparation are in `src/agent/react/capabilities.rs` and
`src/agent/react/mod.rs`; construction and facts are in
`echo-integration/src/providers/config.rs`; snapshots consume the installed
client in `src/agent/snapshot.rs`. `src/agent/handle.rs` explicitly documents the
boxed wrapper's cached borrowed-string contract.
The new `runtime_model_publication_switches_actual_http` test independently ran
on unchanged e3 production: both turns completed, with original HTTP models A/A
instead of A/B. The identical selected case passed after the repair. Focused
rollback, sourced-fact, actual Context budget and pure-commit controls also pass;
raw identities and limits are recorded by the existing verification Evidence.

## 处理记录

ADR 0047 now requires existing fallible setters to build and resolve before one
private frozen-value publication. A custom client clears stale rebuild config;
different-model switching without a rebuild config returns the existing typed
`ConfigError::MissingConfig` without mutation. Same consistent identity is a
no-op. Prepared model generation freezes resolution before returning; commit may
not call client traits or perform parsing/provider lookup. Old-model facts must
not transfer across identity; fresh sourced fields retain precedence.

The reviewed design passed independent C0/I0/M0 review after moving arbitrary
profile queries out of commit. The production candidate and its framework natural before/after are implemented
and verified. Independent final source rereview returned C0/I0/M0 after the
channels conditional-caller correction and its actual isolated compile pass.
Resolution is limited to framework publication: the SDK construction/getter
adaptation, refreshed consumers, complete engineering gates and main delivery
remain separate; the old immutable Host counterexample is not relabeled fixed. This entry does not reuse the identity or historical proof of
the older model-freshness/capability-default Findings.


The final local framework source is `source:6e61249f5796e67d8daa2038c750f8452c9c6d41f5e14dcda791405fd2db5d06`;
its five Rust source identities and focused outcomes are retained in the ignored
`live-model-publication` task results. Existing Evidence and the two specific
current Audit challenges record the bounded proof; broad Behavior status is not
promoted to verified.
