---
schema_version: 1
id: evidence.configured-client-construction
kind: evidence
observed_at: 382cb600b0c8c52d1f6866682c6553e0c3369409
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

## All-feature project-rule fixture epoch

This later epoch changes only `src/agent/react/tests.rs`, source:4ad31ae30cca2dfe0d0b04d90dd656b2f3ddb3280337f5e4407e9fbe6fed8276. The
constructor's prior 10 native tests/one Rustdoc and source1828 qualification stay
at actual Git382cb600. The old Evidence observation and two Model Audit markers
were losslessly rebound after official digest-at-revision reproduced source1828;
old body/counts/status/source_refs remain historical, not newly reobserved.

The final-engineering attempt at382cb600 passed fmt, fmt-check and both complete
Clippy commands, then the unchanged full workspace/all-target/all-feature test
command naturally exited101. Its root library target had1259passed/3failed/
0ignored, raw192183B/SHAfe08ae348e88d18d8c2a2d1715af022b9504b696d9c26addb1268f4799362774.
The failures were `file_checkpoint_legacy_plan_is_readable_but_not_republished`,
`sqlite_checkpoint_legacy_plan_is_readable_but_not_republished` and
`react_agent_reset_preserves_system_prompt`. Later targets and gates06/17feature
checks had not run. Original raw/guard/source/input identities and the affected
344784480-byte executable remain independently preserved outside target.

Both legacy cases had already proved stale current_plan readable and removed
on the next save; only their final message-count assertion failed. Ambient
project rules made the new Agent's canonical root differ from a legacy bare
System with no provenance. Preserving that unknown System and adding the actual
root is correct under evidence.compacted-context-projection-repair. The shared
fixture now disables ambient rule discovery to isolate plan metadata, retaining
its real File/SQLite reopen/restore/save/plan assertions and exact count.

Reset's bare-base oracle was wrong once the accepted effective renderer preserved
the rule layer. Its fixture keeps project rules enabled, captures the complete
constructor view, appends the same real user message, calls the same reset, then
requires full serialized message equality, one System and the base text still
present. Errors propagate through Result and first() replaces direct indexing.
No renderer, unknown-System protection, restore path, policy or production source
changes. No new Finding or reopening of the ordinal/root invariants follows.

The first local after used a -p feature-union variant and passed2legacy+1reset;
that is not reported as the original workspace variant. A subsequent workspace
selector was interrupted by the existing9GiB target guard while compiling,
exit-2/shell254, raw139B/SHA68eae9ff2e5959d25c065fa025e7fe3b9d64ca0c13b6ea157cefeb103207a310,
zero tests executed, source unchanged and minimum free still above5GiB. Whole
process-group interruption left no official runner receipt; none was invented.
The owned package-only official cleanup/readiness is recorded locally.

Current workspace/all-target/all-feature selectors actually execute the two
legacy cases and the reset case successfully; zero matches in unrelated targets
do not enlarge the count. Workspace same-union Clippy and fmt also exit0.
`project-rules-test-oracles/focused-proof.json` binds exact commands/raw/source
and the earlier failures/resource cut remain intact. The original full05 command
and final complete23-step gate set still require execution on this new source;
prior382 successes are not the new snapshot's full qualification.

The first new preflight record failed because reuse lacked selected candidates.
The already independently approved test patch was applied before that failure
was inspected; this timing is not represented as a passed pre-edit gate. Official
record2 exited0 before the first focused Cargo verification command. The exact
path check exited0 during that command (09:10:43 UTC, between09:09:35 and09:10:59);
it was not a pre-execution admission check. Neither is claimed as a pre-edit
or native Hook/Stop gate.

The independent Root reader reviewed the precise failure causes and test-only
design C0/I0/M0 before implementation. Current runtime/source and exact four-path
necessity rereview are required before the normal checkpoint. Formal docs/ADR/
examples/site have no changed public API or product behavior in this test-only
increment; their already pending framework mirror obligations remain in force.
