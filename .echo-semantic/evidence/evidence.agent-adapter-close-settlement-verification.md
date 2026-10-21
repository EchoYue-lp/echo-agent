---
schema_version: 1
id: evidence.agent-adapter-close-settlement-verification
kind: evidence
observed_at: 733d352fc719f922b21bab1cd46206139564367f
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


## 2026-10-21 same-source ownership RED→GREEN 与有限 lifecycle 验证

工作树源码身份为实际 HEAD `ebc69a56e6d0d5ff3a3d7c25e1cd2e25d578a1d5`
加当前未提交差异；canonical 非semantic摘要 `source:5634d776bec790f8ee601f9b9339d0b3e51fc209b1e99f3aff47337f5e58a193`，
2329文件 mode/byte fence 为 `decb8ab5e8700149eef630078ab4088a11af1c0eb3a5e30ac3c85ec6a9844e6b`
（后者不是 canonical source digest）。runtime文件SHA-256
`c13554f88cbbf1fa054a91669fac0a220c53cb70a72b2a2f7e74b47a2edf4c89`，
两入口测试 `f353c7294a209eeb6af37cf0063dd5e9a85b9e85d538bf6802a216d5d63d3cbe`，
ADR0066 `7ff6491285a2c21ddd1d61c096d170182e79b715b2ce61c8d0a1915e0b8314f1`。
这些是工程候选文件身份，不是假称修复已在 HEAD 或公开提交中。

RED 使用 runtime旧SHA `3cd3196fe07b142b099bbb13150e23546b0a81094c87cb833bb38e1fa8e0b791`，
测试整文件f353不变。命令为 `cargo test -p echo_agent --no-default-features --features
acp --locked --test acp_agent_adapter actor_failure_keeps_original_driver -- --nocapture`。
原real Channel两入口均先poll原raw source，再注入同officialconnection的另task Err；
保持producer至cancel，释放并join原producer后，原entry.wait_receipt超时：自然101，
0pass/2fail/0ignore/29filtered；raw12096B SHA-256
`b7333c83b429fda3644b02e5362a2333f487e8ca7d3d537458fa323ae015f222`。
修后同argv/同test，GREEN自然0，2pass/0fail/0ignore/29filtered，raw606B
`c11e084c6becae4f9a9480d2bbb72bccdffb2f2b7a1829e016a966ee847a321b`。
GREEN达到原active-slot真观察、realCancelled/同turnid/同Arc、Agentclose1及幂等；
没有manualsettle或假terminal。GREEN显式unset三项观测环境变量，不声称ambient全同。

其后 `cargo test -p echo_agent --no-default-features --features acp --locked --lib
acp::runtime::` 自然0，15pass/0fail/0ignore/870filtered，包含具名cold rollback、
started owner abort保unknown+lease、real completion弱引用释放3control。raw2147B
`7f504c8d7b6fc31e978dbd0e9bf132eedd62ba7930b68c2e41bd241970763347`。
`cargo test -p echo_agent --no-default-features --features acp --locked --test
acp_agent_adapter` 自然0，31pass/0fail/0ignore/0filtered，raw2549B
`364fdcddd5b6ae38482955b339f266cd4ad213825ce2eda194951033ea496d58`。
15与31分harness记账，31含前两GREEN，不相加为独立新行为数。abortcontrol在private
owner边界注入started/abort，不宣完整Agent panic/运行时销毁所有路径已证。

以上实际Node/guard/caller/mode/evidence/outer均0，零warning/UNKNOWN/源码漂移；
全2329mode字节与原3TLS保持。F默认opt0/jobs1/inc0/debug0/stripSymbols、原2s采样、
9GiBcap/5GiBfloor不变；生命周期两组峰target约1.08GB、最低free约20.46GB，原件
保存精数。原RED/GREEN raw/proof与独立actualproducer二进制保持ignored，不入库。
原REDproof072ce、GREENproof33127、lifecycleproofdf27各保持原执行epoch；
独立source/same2/lifecycle有限复审均C0/I0/M0。后继全六/17/政策5、SDK12重验、
精确main/CI/cold及consumer仍未验，不预填PASS；最初SDK otheractor Err具体cause
仍UNKNOWN，不能由本通用反例把SDK旧Finding全闭，也不动已闭#120水位Issue。


## 同Rust源码后继政策资格的有限承接

后继政策工具链采用完成后，当前 canonical工程摘要为
`source:42398e4e4b01b53bb11e712ec1d303efbb6c71c14aaec4d39291e7736406159b`；
runtime c135、两入口test f353、ADR7ff与Cargo.lock7607均逐字不变，前述2RED→2GREEN、
15runtimeunit/31adapter资格仍限其真实执行epoch。新2332文件范围包含正式政策
helper、测试和机器data；其13单测实际通过，四处fixture空白的源码表示修正保持
完整AST及实际fixture字节，未改变比较或告警合同。原cached diffcheck实际2失败与
修后0、两个canonical源码cut均保留，不能用旧worktreecheck漏掉untracked文件。

在这个精确工程cut上的新4b原policy5已全部实际0；完整事实由既有
`evidence.framework-dependency-policy-repair` 的后继段唯一承载。该结果不等于
全6/Rust feature矩阵、SDK12/consumer或mainCI；该精policy5实际独审已C0/I0/M0。
最初managed otheractor Err类别仍UNKNOWN，原通用driver custody反例与历史SDK
未知退出问题不合并，不增加Run状态或另一receipt authority。
