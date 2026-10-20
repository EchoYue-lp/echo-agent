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
  - src/agent/react/builder.rs
  - echo-agent-learning/examples/demo26_provider_factory.rs
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

## Final local engineering qualification

The final local engineering run used Git
`5eeb8d47e9158c449a1e6400892502e75202edab`, tree
`7ee24682b21942ca1f6a818c1a9283342e0fe95b`, canonical source
`source:4ad31ae30cca2dfe0d0b04d90dd656b2f3ddb3280337f5e4407e9fbe6fed8276`.
Immediately before that run, fetching remote main succeeded at
`f3fa0705f62fe813a5a86bc41cd673f82f915a61`; merging it returned
Already up to date. The task branch was clean, conflict-free and contained main.
All 1,764 non-semantic tracked inputs remained byte-identical throughout the
run and this subsequent Evidence-only tail. Official source_digest still
reproduces 4ad31; baseline therefore needs no digest or closure-field change.

Every one of the six prescribed workspace gates and seventeen independent
root-feature checks naturally exited0 on this same source snapshot:

| Command | Exit |
| --- | --- |
| `cargo fmt --all` | 0 |
| `cargo fmt --all -- --check` | 0 |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | 0 |
| `cargo clippy --workspace --lib --bins --all-features --locked -- -D clippy::unwrap_used -D clippy::expect_used -D clippy::panic -D clippy::unreachable` | 0 |
| `cargo test --workspace --all-targets --all-features --locked` | 0 |
| `cargo check --workspace --lib --no-default-features --locked` | 0 |
| `cargo check -p echo_agent --no-default-features --features acp --locked` | 0 |
| `cargo check -p echo_agent --no-default-features --features a2a --locked` | 0 |
| `cargo check -p echo_agent --no-default-features --features mcp --locked` | 0 |
| `cargo check -p echo_agent --no-default-features --features lsp --locked` | 0 |
| `cargo check -p echo_agent --no-default-features --features sqlite --locked` | 0 |
| `cargo check -p echo_agent --no-default-features --features telemetry --locked` | 0 |
| `cargo check -p echo_agent --no-default-features --features topology --locked` | 0 |
| `cargo check -p echo_agent --no-default-features --features subagent --locked` | 0 |
| `cargo check -p echo_agent --no-default-features --features web --locked` | 0 |
| `cargo check -p echo_agent --no-default-features --features media --locked` | 0 |
| `cargo check -p echo_agent --no-default-features --features data --locked` | 0 |
| `cargo check -p echo_agent --no-default-features --features statistics --locked` | 0 |
| `cargo check -p echo_agent --no-default-features --features channels --locked` | 0 |
| `cargo check -p echo_agent --no-default-features --features git --locked` | 0 |
| `cargo check -p echo_agent --no-default-features --features database --locked` | 0 |
| `cargo check -p echo_agent --no-default-features --features rag --locked` | 0 |
| `cargo check -p echo_agent --no-default-features --features chart --locked` | 0 |

The complete test command executed 89 Cargo targets: 87 standard Rust test
harnesses and two Criterion targets. Taking the final summary of each Cargo
target yields 3,653 passed / 0 failed / 3 ignored / 0 filtered; two nested child/selfprobe
summaries are deliberately excluded from that aggregate. The framework root
library passed all 1,262 tests, including the three corrected original oracles.
The old 382 full-run failure is retained as a separate epoch, not reclassified
or replaced by the focused subset or by cleanup.

The following three unchanged, pre-existing opt-in live tests were not executed
and are not counted as coverage. Their complete source blobs match the fetched
main snapshot; this task added no ignore or skip flag:

- `lsp::manager::tests::live_installed_servers_initialize_on_real_project_fixture`: `echo-integration/src/lsp/manager.rs:1456`; opt-in live LSP smoke test; set ECHO_AGENT_LSP_SMOKE=1.
- `research::clients::tests::live_open_scholarly_providers_return_results`: `echo-tools/src/research/clients.rs:979`; opt-in live smoke test; set ECHO_AGENT_PROVIDER_SMOKE=1.
- `research::clients::tests::live_zotero_credentials_can_list_items`: `echo-tools/src/research/clients.rs:1005`; requires ZOTERO_API_KEY and ZOTERO_LIBRARY_ID.

All 23 official receipts have exit0, null signal/error and no truncation; their
seen/written byte counts and SHA-256 match the original logs. Every source guard
is empty, no resource interruption occurred, and compiler-warning scans are
empty. The peak owned target was 9,217,630,208 bytes (below 9GiB) and minimum free
space 6,860,873,728 bytes (above 5GiB). Top-level Cargo commands ran sequentially with jobs=1,
incremental=0, dev/test debug=0 and symbol stripping. This runner and the source
profiles introduced no disabling of assertions, overflow checks or unwind, and
introduced no RUSTFLAGS override or warning suppression. The receipt does not
fully record inherited effective environment overrides, so no broader claim
about their absence is made.

The complete local proof is ignored
`project-rules-test-oracles/full-result-proof.json`, 30,378 bytes/SHA-256
`374aa9916319c8f8eb65c880a06d429fd896f51d9d0b1b6543ea87119ae13c87`.
Its full test raw is 366,168 bytes/SHA-256
`3ffcdbe83e9a3d6c3e2b708d00fac3a3b9b8b9caa91954c0cfd6f418fe5f28db`;
`full-test-counts.json` retains each top-level harness and excluded nested
summary, and `existing-ignored-tests-proof.json` records the original ignore
annotations and matching main blobs. The raw/source/binary artifacts remain
outside Git; these hashes identify them rather than substitute for originals.

An independent engineering-proof review returned C0/I0/M0 for the receipts,
top-level counts, source guards and frozen inputs recorded here.

This tail closes local engineering verification only. It does not promote
broad Behavior or Finding scope, relabel native Hook/Stop, qualify a refreshed
SDK Host/contract or website mirror, or claim framework main and its exact CI
already passed. Historical observations above keep their original revisions,
source_refs, counts and qualifications. Main integration, precise main CI and
the independently owned SDK/website delivery remain separate steps.

## Builder known-pair increment: scope and counterexample

The following increment binds source:5f000f23fee488a6fecb1a972b155cd72ad206384c05f3802cdfec0d52c11872 on framework base
3560812caede84998a1f6f26d7bf3fa81ea0486c. Earlier constructor receipts and
observations above keep their original execution epochs. No old receipt counts
as current Builder qualification.

The real consumer counterexample is the CLI original complete core harness:
1,630 passed, 17 failed and nine pre-existing ignored, with exit101. The 17
failures require retained configured state. That raw was 170,665 bytes/SHA-256
bc2c9ab9b33de8fd610f790413f6d2d90cd919fbf3c7670d23b6ed9d776d3ddb; the
CLI owner preserves its actual executable and original output outside Git. This
framework task does not substitute an absent-new-API compile failure as before.
The parent CLI full command stopped at this failing core harness, so later
harnesses and remaining commands were not executed.

Source inspection follows all three configured CLI factory paths: configuration
injection constructs a client, then opaque installation of the original shared
Arc intentionally clears the config. Removing opaque installation would lose
the original Arc instead. Existing framework Builder config-wins construction
and the direct paired constructor are distinct: the latter cannot replace the
full Builder's supplied Subagent registry and other subsystem injections.

`ReactAgentBuilder::configured_llm_client` fills only that initial-construction
seam. Both entries share the existing identity check/profile merge and pure
publisher. Pair preparation precedes owned Agent/Context creation; pair build
does not call build_client or the opaque setter, and initialization preserves
implicit versus explicit token-limit policy. Old ordinary setters retain their
config-wins precedence; explicit pair and later ordinary setters replace one
another without retaining stale pair input. Original credentials remain private
in-process state; arbitrary endpoint/credential association is not provable.

Focused engineering tests, source review and consumer-after qualification are
not yet established by this source-only increment. Exact commands, receipts and
limits will be appended after execution. The first local cold-budget guard
refused before launching Cargo (target remained absent); a later authorized
format-only command exited0 and changed only the new tests/example formatting.
No compile/runtime success follows from formatting. The original 9GiB target cap
and 5GiB free floor, raw failures and scope boundaries are retained.

Only one new public Rust Builder method is added. It is inventory telemetry, not
a newly accepted remote operation. SDK's qualified direct-constructor path and
exact framework pin356 remain unchanged. Framework six gates, 17 feature checks,
CLI original 17 assertions and complete eight gates, website synchronization and
precise main CI remain separate acceptance work.
