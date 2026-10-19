---
schema_version: 1
id: evidence.configured-client-construction
kind: evidence
observed_at: source:1828e264adb8119b8b19c84ccc0e18b35067b6333f89c775e51856563290659e
source_refs:
  - docs/adr/0047-model-facts-freshness-authority.md
  - docs/en/38-factory-modes.md
  - docs/zh/38-factory-modes.md
  - src/agent/react/mod.rs
  - src/agent/react/tests.rs
  - src/intent/classifier.rs
supports: [behavior.llm-provider-execution, rule.provider-protocol-boundary]
limitations:
  - Bounded framework construction and Intent test qualification; no refreshed SDK factory/Host/two-Session or language proof
  - Original client/config association is caller supplied; only model identity is checked, not arbitrary endpoint or credential provenance
  - Full six gates, 17 features, exact main CI and planned website mirrors remain separate
command_results:
  - {"command": "cargo test -p echo_agent --lib configured_client_constructor --locked -- --nocapture", "exit_code": 0}
  - {"command": "cargo test -p echo_agent --lib runtime_model_publication --locked -- --nocapture", "exit_code": 0}
  - {"command": "cargo test -p echo_agent --lib test_intent_router_config_threshold_and_available_skill_fence --locked -- --nocapture", "exit_code": 0}
  - {"command": "cargo test -p echo_agent --doc new_with_configured_llm_client --locked -- --nocapture", "exit_code": 0}
  - {"command": "cargo clippy -p echo_agent --lib --tests --locked -- -D warnings", "exit_code": 0}
  - {"command": "cargo fmt --all -- --check", "exit_code": 0}
---

# Original configured-client construction and focused consumer evidence

## 来源与范围

The base is framework `674f2b41fd1bd25baedbf7102b26565f2771391d`. The single
new Rust constructor preserves an already prepared client Arc together with
its original LlmConfig. Existing opaque-client injection intentionally drops
rebuild config; existing config injection builds another client. The two old
paths' distinct behavior is directly checked, not presented as a newly failing
product regression. The approved API fills that construction seam without a
second factory, Store, live setting or remote SDK operation.

Source is `source:1828e264adb8119b8b19c84ccc0e18b35067b6333f89c775e51856563290659e`. The shared private profile preparation still uses
one existing merge owner; constructor queries once, freezes facts before normal
Context creation, and reuses the pure install publisher. No client build or
full prepared-generation token-limit commit occurs in the new constructor.
ADR0047 records references, alternatives, caller responsibilities and tradeoffs.

## 支持的结论

Actual focused tests: two new constructor cases; seven existing model
publication cases; one new IntentRouter case. Ten native tests all passed, with
zero ignored. The constructor's executable public Rustdoc also passed one test.
The Intent case is one test with three required controls, not a parameter or
identity matrix. No production Intent code changed.

The counter-bearing constructor client verifies exactly one profile query during
construction, exact Arc/original-config retention, untouched explicit-limit
marker and sampling, initialized implicit 64k versus explicit 100k Context
allowances, and same-model no-op. A compressible user probe is seeded to observe
the real initial Context: system-only context has nothing compressible even at
zero allowance. Bad pairs reject before profile query or subsystem ownership,
release the supplied Arc, and return the existing typed ConfigFileError without
the sentinel credential in actual Display/Debug diagnostics. This does not
claim universal network/journal/protocol secret filtering.

Three existing model cases now construct through this public entry: same-Agent
real concrete-adapter HTTP A then B, implicit/explicit budget consumption after
real turns, and custom replacement followed by typed MissingConfig refusal.
The other four preserve no-op/failure, sourced freshness, self-builder and pure
prepared-commit controls through the refactored shared preparation helper.
The old HTTP case's historical before/after bytes and qualifications remain at
their own revisions; the updated constructor case is a new executed source cut.

The Intent candidate was adopted from exact patch SHA-256
`c266510a8fe1ccc7a0205b9f5503cb0173118c50885c2da47ed464272b630b21`.
It first checks a valid parsed SkillRequired, then exercises the real router:
0.9 confidence fails configured 0.95; confidence 1.0 fails an excluding skill
fence; the same skill succeeds when both permit it. Malformed parsing cannot
make either Fallback control accidentally pass. UTF-8 skill names are retained.
This is bounded configuration-consumption evidence, not new tool enforcement.

Focused root lib/tests Clippy with `-D warnings` and fmt check exited zero.
All six successful commands retained source guards without drift or resource
interruption. Raw logs, exact source copies and test binary stay in ignored
`.echo-semantic/local/configured-client-construction/`; `focused-proof.json`
binds exact command/source/raw hashes and resource bounds. Hashes identify
those originals and do not replace them.

## 已知缺口

Initial official preflight rejected a nonexistent new Evidence mistakenly listed
as mutable; only its allowed path was retained and official record then passed.
The first test compile failed on an erroneous assertion of SDK FrameworkConfig's
field on framework AgentConfig and executed zero tests (651-byte raw). After
that fixture fix, the next native run had one pass and one failure at the
system-only compression oracle (1,117-byte raw); query/identity/config/marker/
sampling assertions preceding it passed, later no-op assertions had not run.
A real user probe corrected the oracle with production unchanged. Both failures
and their raw/source guard identities remain recorded in `fixture-errors.json`;
neither is claimed as a new production counterexample or silently discarded.

Original configured-pair validation cannot establish arbitrary Arc endpoint or
credential provenance; the caller must supply the original validated pair.
The error carries no supplied config/credentials, while the existing in-process
llm_config getter remains intentionally available. SDK factories must carry the
original config and use this entry before custom extensions, then prove shared
Arc across independent Sessions and real new-pin model behavior themselves.
Rust inventory inclusion is telemetry, not automatic language/API acceptance.

The earlier compacted-context Evidence/Audit observations are losslessly bound
to actual Git `674f2b41` after official digest-at-revision exactly reproduced
`b2639597dfa94b7e16aa312e46db6585d9df7636d6340c004ae978f18711d0f1`.
Only four machine markers changed; earlier body/counts/status/source_refs and
qualification remain unchanged. Broad Behavior and baseline closure flags are
not promoted. Same installed full semantic validation at actual current base,
known task-reader/native Stop limits, full engineering gates and main delivery
remain distinct. F38/ADR0047 mirrors are part of the planned website sync after
exact framework main; there is no changed standalone examples call site, and
this new public example is covered by actual Rustdoc execution.
