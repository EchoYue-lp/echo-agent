---
schema_version: 1
id: evidence.acp-run-task-error-isolation
kind: evidence
observed_at: source:2a003bba7a854b548d1e65040aa00d0390af215d9f1114d5755cd6a9563197a2
source_refs:
- src/acp/runtime.rs
- src/acp/adapter.rs
- tests/acp_agent_adapter.rs
- docs/adr/0088-subagent-settlement-observation.md
- docs/supreme/specs/2026-10-14-subagent-physical-settlement/design.md
- docs/subagent-cancellation-observation.md
- docs/en/lifecycles.md
- docs/zh/lifecycles.md
supports:
- behavior.protocol-projection
- behavior.agent-turn-lifecycle
- rule.turn-terminal-authority
- rule.framework-layer-ownership
limitations:
- Task-scoped ACP repair only; global semantic closure is not checked.
- SDK immutable-Host and three-language consumer qualification require their own exact framework pin.
- Exact merged-main CI and independently owned SDK pin qualification remain pending at this pre-merge checkpoint.
---

# ACP Run observation is not a connection-fatal task error

## 来源与范围

On base `9886502`, `prepare_run` retained the original uncertain Session lease
but returned its driver error to official `ConnectionTo::spawn`. The pinned ACP
2.1.0 runtime explicitly makes that result fatal to the whole server. Ordinary
Agent failures already become Failed receipts and request errors; the new
nonterminal refusal needed the same request-level isolation without a receipt.

The existing RunEntry Running payload now records its structured driver error
under the lifecycle lock while retaining the original lease. `wait_result`
observes a committed receipt first or that error, with registered notifications
for concurrent and later observers. Standard Prompt uses it in both normal and
request-cancel waits. The prepared task returns successfully after recording a
Run error; transport/spawn/responder failures retain their own error paths.
Receipt-only wait, original registry identity, admission fence, cancellation and
close remain authoritative. No new terminal, store or wire method is added.

## 支持的结论

The two original official-Channel connection regressions compiled successfully
against unchanged production base `9886502`, then failed as behavior assertions:
standard Prompt received `incoming_transport_closed` instead of the original
not-settled run/execution data, and shared `prepare_run + spawn` made the next
`session/new` fail with a closed transport. The focused selector was
`cargo test -p echo_agent --no-default-features --features acp --test
acp_agent_adapter run_error_isolation --locked -- --nocapture`.

Before exit: 101, 0 passed / 2 failed. Raw 12,473 bytes,
SHA-256 `0e681c0cb016d859f2301322e526e7a1099b790bd8b9039ed23eda00afd837bc`.
After the local production repair, the same two tests passed, exit 0. Both
source guards reported no Rust input changes during execution.

The expanded official-connection suite passed 29 tests, including a cancelled
request whose original source stays held through an observation and then reaches
real EOF. That result remains nonterminal, retains the original Session lease,
and permits requests on other Sessions over the same connection. Ordinary
failure and successful/cancelled receipts remain covered by the existing suite.
An initial expanded run passed 28 and failed the new held-source fixture: its
`try_stream` adapter stopped at an error item. The fixture now passes Result
items through without abandoning its source; the full 29 were rerun successfully.

Raw logs, resource receipts, source guards and the before-test patch remain only
in ignored `.echo-semantic/local/acp-run-task-error-isolation/`. They are not
formal source references or bundled delivery files. Design review passed with
zero Critical/Important findings.

## 已知缺口

The ACP unit suite subsequently passed 43/43, including concurrent and later
observers, receipt-only pending behavior, committed receipt priority and rejection
of a late observation. Focused clippy passed with `-D warnings`. Independent
implementation review passed with zero Critical/Important/Minor findings.
These focused results do not replace SDK pin validation or exact merged-main CI.

## 最终本地资格

All six required workspace gates and all 17 standalone feature checks passed
on the frozen Rust/Cargo inputs committed as `30b9bf05`. Commands are exactly
the repository AGENTS matrix: fmt, fmt check, workspace/all-target/all-feature
clippy with warnings denied, production panic-class clippy, workspace/all-target/
all-feature locked tests, workspace/lib/no-default locked check; then individual
`cargo check -p echo_agent --no-default-features --features <feature> --locked`
for acp, a2a, mcp, lsp, sqlite, telemetry, topology, subagent, web, media, data,
statistics, channels, git, database, rag and chart. All 23 commands exited zero,
with zero warnings and unchanged Rust/Cargo inputs. Only resource settings
jobs1, incremental0, debug0 and stripped symbols were applied; assertions,
overflow checks and panic behavior were not overridden.

Full tests: 87 top-level harness summaries, 3,627 passed / 0 failed / 3 ignored.
Two nested `echo_state` file-lease subprocesses each passed one test, each with
382 filtered; they are not added again to the top-level count. Existing ignored
tests require installed LSP, online scholarly providers or Zotero credentials.
The full-test raw has 376,026 bytes and SHA-256
`7581e998aa7e23513f7cb788deaf50d6e573f8f3aff775a20cecd1bd36d34655`.

The accepted c0bc306b semantic validator, verified against its clean Git blob,
passed strict-source and base9886502 change-evidence qualification on the clean
independent Git candidate with the exact staged delivery tree. The old workspace
map's `source:d05...` was proven identical to exact public9886502 and rebound to
that recoverable historical commit; its claims and historical epoch are unchanged.
Only the baseline's necessary current source digest/base metadata is refreshed.

Installed0.5 task checks remain distinct: the new Evidence's three required
headings were corrected; 551 historical missing-path errors disappeared under
the read-only historical-reference candidate. Its remaining 211 owner-map errors
were narrowed by declaring the existing direct maps, exposing the task closure
budget (29 diagnostics). No tool budget, old semantic claims or native receipts
were altered to make that task mode pass. This source-tree qualification is not
a claim of installed Hook/Stop success, global behavioral closure, SDK consumer
success or merged-main CI. Raw attempts and classification remain ignored locally.
