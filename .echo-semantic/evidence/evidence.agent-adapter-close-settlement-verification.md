---
schema_version: 1
id: evidence.agent-adapter-close-settlement-verification
kind: evidence
observed_at: source:512b2adda3fbd65e8d7e3c2f4d23a036338d495ab4c3b276f09a15af58ed99f9
source_refs:
  - src/headless.rs
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
  - A2A remains untouched and unverified; its findings stay open
  - Outer run_headless future cancellation before the close phase remains an explicit one-shot API limitation
  - Combined gates, semantic snapshot and integrated review receipts are owned by evidence.foundation-36-72-51-integration-verification
  - Remote-main acceptance remains outside this focused evidence
---

# Agent adapter close settlement direct verification

## 支持的结论

The listed red-to-green checks support the current uncommitted adapter-close
candidate at the stated source digest. They do not establish remote-main
acceptance or a fresh independent Audit.

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

## 来源与范围

All commands ran in the isolated Issue #36 worktree based on
`39a348b3c9b8a6a7fb3caaa1146832c570e988cb`. The focused suites cover
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
  handler close. Root `channels::tests`: 8 passed, exit 0.
- `cargo test -p echo_agent headless::tests --locked`: 7 passed, exit 0.
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
cancellation limitation is documented rather than claimed as fixed.

## 集成收据

Combined workspace/feature gates, strict semantic validation, and final
integrated review are not duplicated here. Their exact commands, exit codes,
logs and source digest belong to
`evidence.foundation-36-72-51-integration-verification`.

## 已知缺口

Remote-main delivery is not established by local integration receipts. A2A is
untouched and remains an open part of the broader Finding.
