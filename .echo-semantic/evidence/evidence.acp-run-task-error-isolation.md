---
schema_version: 1
id: evidence.acp-run-task-error-isolation
kind: evidence
observed_at: source:1744259e3d9571ddb65a2fe542c858bc87acee1ad0243d3e50e14a97aecda72f
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
- Full workspace gates, standalone features and exact merged-main CI remain pending at this checkpoint.
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
These focused results do not replace the pending full gates, standalone feature
matrix, SDK pin validation or exact merged-main CI. The installed semantic task
validator's historical-reference diagnostics are tracked independently; no old
semantic object is rewritten to make that tool accept a historical path.
