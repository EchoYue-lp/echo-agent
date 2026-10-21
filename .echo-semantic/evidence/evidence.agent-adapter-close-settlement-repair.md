---
schema_version: 1
id: evidence.agent-adapter-close-settlement-repair
kind: evidence
observed_at: 733d352fc719f922b21bab1cd46206139564367f
source_refs:
  - src/acp/session.rs
  - src/acp/runtime.rs
  - src/acp/adapter.rs
  - src/acp/mod.rs
  - echo-orchestration/src/runtime/turn_driver.rs
  - src/headless.rs
  - src/lib.rs
  - echo-integration/src/channels/manager.rs
  - echo-integration/src/channels/session.rs
  - echo-integration/src/channels/types.rs
  - src/channels.rs
  - src/agent/react/mod.rs
  - docs/adr/0066-agent-adapter-close-ownership.md
supports: [behavior.agent-turn-lifecycle, behavior.protocol-projection, rule.turn-terminal-authority]
limitations:
  - ACP requires a caller-retained AcpAdapterCloseHandle; consumers without one fail before Agent creation
  - A2A remains untouched and owned by its separate Findings
  - Agent implementations that ignore cancellation can exceed one close attempt and require the caller to retain the owner
---

# Agent adapter close settlement repair

## 支持的结论

## Owner and ordering

ACP uses its existing `SessionRegistry` close lease and framework `RunEntry`
receipt. The registry permanently fences creation, including factory work
already in progress. Connection close cancels extension invocations and Runs
concurrently, retains a Run without its framework receipt, waits for profile
settlement, and attempts Agent close after callback/Run drain even if profile
flush fails. A failed Session Agent close retains the original registry entry;
transport close failure triggers one EOF fallback retry. A mandatory
`AcpAdapterCloseHandle` retains the exact same services/profile owner after
both attempts fail; an unretained adapter cannot begin the connection or
create an Agent. `connect_retaining_close_owner` synchronously returns the
handle before its connection future is polled. Manual official Client
callers must keep their handle until cleanup succeeds. No adapter-generated
Turn terminal is introduced.

Headless synchronously publishes `HeadlessRunHandle` before its owned task runs.
Dropping or aborting a `run_headless` waiter requests Turn cancellation while
the task retains the Agent through close and publishes one result receipt. A
failed close remains owned by the handle for `retry_close`. Missing runtime
startup and runtime shutdown before first poll also return a failure receipt
without dropping the close owner. A caller token is observed through a child
scope, and `retry_close` rejects calls before the result receipt exists.

ReactAgent has one close authority that atomically fences admission, owns the
child cancellation token for every accepted direct or streaming Turn, and
waits for the Turn's existing terminal path before MCP cleanup. Preparation
becomes settlement-bearing before reconcile, hydrate, guard, audit, trace, or
context awaits. Normal pre-turn rejection releases the lease explicitly;
caller abort, producer abort, or panic records persistent close debt and blocks
MCP cleanup. A cancelled close waiter does not reopen admission or discard
leases; a later close waits or reports the same debt. The existing
`AgentTurnDriver` classifies a typed cancellation before stream creation as
`TurnOutcome::Cancelled`, preserving the same terminal kind at every phase.

ChannelManager records transport-stopped and handler-close as separate phases,
then retains handlers until both succeed. A retry after transport success goes
directly to the same handler even when `ChannelPlugin::stop` is non-idempotent.
SessionHandler fences session creation, cancels
sender generations, waits for every accepted stream and delivery lease during
shutdown, and closes each inner Agent before removing it or publishing a
timeout/reset replacement. Legacy/custom stream receipts are resource lifetime
only; reset keeps its existing driven-only terminal settlement. Failed or
cancelled close leaves the same handler/session owner. `ReactAgent::drop` no
longer spawns unawaited MCP cleanup; adapters await the existing Agent close.

## 来源与范围

The sources above cover ACP, Headless, Channel and ReactAgent close owners.
A2A remains outside this repair candidate.

## Decision authority

ADR 0066 records alternatives, industry patterns, layer ownership, bounds,
and one-shot limitations. ADR 0046 remains Turn execution/delivery authority;
ADR 0057 remains Channel generation-delivery authority; ADR 0049 remains MCP
transport/client cleanup authority.

## 已知缺口

ACP requires external consumers to retain its close handle before `ConnectTo`
consumes the adapter. Dropping a manual handle after admission forfeits retry
ownership and violates this contract. A2A remains outside this Evidence.
Integration gates, independent rereview, and mainline delivery are separate
obligations.


## 2026-10-21 ACP original-driver custody 修复切片

工作树源码身份为实际 HEAD `ebc69a56e6d0d5ff3a3d7c25e1cd2e25d578a1d5`
加当前未提交差异；canonical 非semantic摘要 `source:5634d776bec790f8ee601f9b9339d0b3e51fc209b1e99f3aff47337f5e58a193`，
2329文件 mode/byte fence 为 `decb8ab5e8700149eef630078ab4088a11af1c0eb3a5e30ac3c85ec6a9844e6b`
（后者不是 canonical source digest）。runtime文件SHA-256
`c13554f88cbbf1fa054a91669fac0a220c53cb70a72b2a2f7e74b47a2edf4c89`，
两入口测试 `f353c7294a209eeb6af37cf0063dd5e9a85b9e85d538bf6802a216d5d63d3cbe`，
ADR0066 `7ff6491285a2c21ddd1d61c096d170182e79b715b2ce61c8d0a1915e0b8314f1`。
这些是工程候选文件身份，不是假称修复已在 HEAD 或公开提交中。

原 `prepare_run` 把唯一 driver future 交官方 connection task actor。另一个 task Err
能销毁它，而 retained services/Run entry 无法自行取得尚未发布的 receipt。修复仍由
既有 prepare_run 接纳：注册后立即建立 cold resource owner并复核admission fence；
首次 observer poll 单次 spawn 原 AgentTurnDriver，observer Drop只取消，不abort。
RunDriverOwner 保原entry/lease并Weak引用唯一map；cold Drop按exactptr同步回滚，
started panic/abort只把原lease移交既有RunEntry的not_settled知识。realreceipt才settle
并释放lease；running remove_run只revokes并保close可见，cold setup failure随后drop
原task完成回滚。私有map短StdMutex段不跨await或用户回调，没有第二registry/Run状态。

标准入口、SDK式shared入口共用同机制，标准/SDK调用和公共签名不改。cold回滚、
异常owner loss以及完成后weak释放的实际新controls见原 verification Evidence追加；
异常control仅private owner.started/abort注入，不冒充完整Agent panic现场。
源码与同两项结果独立复审C0/I0/M0；runtime15与adapter31只为各自focused集合，
全六/17、政策、SDK managed12、精确main/CI/cold均待后继资格。无新增ADR/API、
示例调用或官网合同变化；既有ADR0066同步机制，examples由后续all-target门禁覆盖。
