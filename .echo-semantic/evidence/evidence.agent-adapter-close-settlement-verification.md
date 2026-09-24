---
schema_version: 1
id: evidence.agent-adapter-close-settlement-verification
kind: evidence
observed_at: source:87b717676a7b51e213630677989947777b4bed441acd8c7d655fb6c96dca77ad
source_refs:
  - src/headless.rs
  - src/lib.rs
  - echo-orchestration/src/runtime/turn_driver.rs
  - src/agent/react/lifecycle.rs
  - src/agent/react/run/react_loop.rs
  - src/agent/react/run/stream_channel.rs
  - src/acp/session.rs
  - src/acp/runtime.rs
  - tests/acp_agent_adapter.rs
  - tests/acp_extension_runtime.rs
  - echo-integration/src/channels/manager.rs
  - echo-integration/src/channels/session.rs
  - src/channels.rs
  - src/agent/react/capabilities.rs
  - echo-agent-learning/examples/demo38_im_channels.rs
  - echo-agent-learning/examples/demo72_acp_agent_adapter.rs
supports: [behavior.agent-turn-lifecycle, behavior.protocol-projection, rule.turn-terminal-authority]
limitations:
  - A2A remains untouched and owned by its separate Findings
  - Remote-main delivery and post-merge receipts remain delivery gates
---

# Agent adapter close settlement verification

## 支持的结论

The listed red-to-green checks support the framework adapter-close repair at
the stated source digest. Final delivery evidence is recorded separately.

## Failing evidence before repair

- Headless close test: missing awaited owner path, `E0425`, exit 101.
- Channel manager handler-close contract: `E0407`, exit 101; sender Session
  close retry test failed its owner assertion, exit 101.
- ACP creation after close, Run wait timeout retaining receipt, profile flush
  followed by Agent close, and connection-services extension drain each
  failed their behavioral assertions, exit 101.
- ACP mandatory retained-close handle initially failed to compile
  (`E0425`/`E0599`, exit 101) before that owner path existed. The first async
  helper then failed cancellation review because it returned the handle only
  after await; the synchronous owner-plus-future API closes that window.
- Channel start cancellation test first observed close count 0 and failed
  exit 101 before handler ownership moved ahead of the start await.
- Legacy/custom Session stream close first failed with `legacy stream resource
  was still active`, exit 101. The first wait implementation then failed Send
  compilation because a sync MutexGuard crossed await; the scoped-state repair
  removed that compile failure.
- Current-main rereview added `ReactAgent::close` active/queued Turn coverage.
  The red test failed because close returned before either Turn settled. The
  repair fences admission, cancels both leases, and remains retryable after a
  cancelled close waiter.
- Current-main Headless review added waiter cancellation and close retry
  coverage. The prior local-owner implementation could be dropped before
  close; the owned task and `HeadlessRunHandle` retain the same Agent owner.
- The first all-feature workspace gate exposed a start-time typed cancellation
  being wrapped as `Failed(cancelled)` by `AgentTurnDriver`. The focused driver
  test reproduced that classification before the repair; the driver and
  Channel projection tests now preserve `Cancelled`.

## 来源与范围

All commands ran in the isolated Issue #36 worktree based on
the original `39a348b3c9b8a6a7fb3caaa1146832c570e988cb` candidate and the current
main-based Issue #36 worktree. The focused suites cover
ACP, Headless, Channel, MCP close and their compiled learning consumers. A2A
commands are deliberately excluded from the current candidate.

## Focused green evidence at current candidate

- `cargo test -p echo_agent --features acp --test acp_agent_adapter --locked`:
  24 passed, exit 0. Includes no-handle rejection before Agent construction,
  profile flush failure, EOF fallback retry, third close through the retained
  handle after two failures, connection-future abort after a settled Run, and
  official ACP channel cancellation. A handle obtained and dropped before the
  first connection poll also rejects setup without Agent creation.
- `cargo test -p echo_agent --features acp --test acp_extension_runtime --locked`:
  8 passed, exit 0. `acp::session::tests` separately: 9 passed, exit 0.
- `cargo test -p echo_integration --features channels channels:: --locked`:
  49 passed, exit 0, including cancellation after plugin start retained its
  handler and shutdown waited for a stalled legacy stream to drop before
  handler close. The manager tests prove failed or cancelled handler close
  retries without calling a non-idempotent transport stop twice. Root
  `channels::tests`: 8 passed, exit 0.
- `cargo test -p echo_agent --lib headless::tests --features
  channels,mcp,acp --locked`: 10 passed, exit 0. This includes caller-token
  isolation, pre-settlement retry rejection, missing runtime and runtime
  shutdown before first poll.
  `cargo test -p echo_agent --features mcp
  agent_close_keeps_failed_target_projections_until_retry_settles --locked`:
  1 passed, exit 0.
- `cargo check -p echo-agent-learning --features channels --example
  demo38_im_channels --locked`: exit 0. `cargo check -p echo_agent --features mcp --locked`:
  exit 0. Every Cargo invocation used debug-info disabled, incremental off,
  and two build jobs due shared disk pressure.
- `cargo check -p echo-agent-learning --features acp --example
  demo72_acp_agent_adapter --locked`: exit 0; the example receives the close
  owner alongside the direct connection result without an unused-import warning.
- `cargo fmt --all` and `cargo fmt --all -- --check`: exit 0.
- `cargo clippy -p echo_agent --lib --tests --features acp,channels,mcp
  --locked -- -D warnings`: exit 0. The matching strict lib check with
  `-D clippy::unwrap_used -D clippy::expect_used -D clippy::panic
  -D clippy::unreachable` also exited 0.
- `cargo clippy -p echo_integration --lib --tests --features channels
  --locked -- -D warnings`: exit 0.

Post-integration review added the legacy/custom stream settlement counterexample.
The old implementation returned `channel session Agent close failed: legacy
stream resource was still active`, exit 101. The first fix exposed a non-Send
MutexGuard-across-await compile error, also exit 101. The scoped implementation
then passed the complete Channel suite 49/49; receipt log:
`.git/worktrees/foundation-36-72-51/supreme/logs/command-1789805569534.log`.
Channel `-D warnings` Clippy and panic-API Clippy exited 0 at
`command-1789805589839.log` and `command-1789805610595.log`. Headless focused
tests remained 7/7 at `command-1789805628555.log`; their outer-future
cancellation limitation was closed by the current repair.

Current focused verification also passes ReactAgent lifecycle 2/2, the 17
close-named tests, active/queued close 1/1, early input-guard abort 1/1, forced
producer abort 1/1, provider-failure settlement 1/1, and framework
documentation contracts 13/13. The early-abort and forced-abort tests prove
that Drop cannot satisfy a settlement-bearing lease; two close attempts return
the same debt. TurnDriver start-time cancellation and the Channel
failure/cancellation projection test also pass 1/1 each. `demo38_im_channels` and
`demo72_acp_agent_adapter` remain executable learning consumers; demo54
compiles the retained Headless handle pattern.

## 集成收据

At source digest `87b717676a7b51e213630677989947777b4bed441acd8c7d655fb6c96dca77ad`:

- `CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 RUSTFLAGS='-C debuginfo=0
  -A linker_messages' ./scripts/verify.sh`: exit 0. The root suite passed
  1064/1064; workspace tests, examples, benches, both Clippy gates and
  no-default compilation were green with zero warnings. Receipt:
  `.supreme/logs/issue36-final-verify.log`.
- Independent `cargo check -p echo_agent --no-default-features --features
  <feature> --locked` passed 17/17 for
  `acp/a2a/mcp/lsp/sqlite/telemetry/topology/subagent/web/media/data/statistics/channels/git/database/rag/chart`.
  Receipt: `.supreme/logs/issue36-final-feature-matrix.log`.
- Strict semantic snapshot plus high-risk change evidence exited 0. Receipt:
  `.supreme/logs/issue36-final-semantic.log`.
- Final independent rereview passed after the Channel transport/handler phase
  counterexample was repaired: Critical 0, Important 0, Minor 0.

## 已知缺口

Remote-main delivery is not established by local integration receipts. A2A is
untouched and remains owned by separate Findings.
