# ADR 0094: Invocation Tool Scope Across Subagent Dispatch

## Status

Implementation candidate. Hard-tool-scope and the separate target-policy
increment passed source review and their focused Rust checks. Full matrix and
consumer delivery remain pending.

## Context

The framework already has one tool registry, invocation disabled tools and
`ToolVisibilityState` with available, eligible and visible sets. A parent can
exclude tools for one task without removing them from a shared Agent or role.
That hard bound must survive a subsequent dispatch to a fresh or differently
configured Subagent.

`ParentContextFactory` builds transferable history and definitions, while
`SubagentContext.allowed_tools` carries a nonempty invocation whitelist. The
old dispatch factory did not populate that whitelist. Fork applied an existing
disabled mask, but did not construct invocation visibility for the next child.
Sync and Teammate omitted the mask altogether. Team discarded scope-only
parent contexts when they had no history or goal. A prompt-only capability
projection could therefore describe a restriction that actual execution lost.

This is a generic framework defect. Applications own their role selection,
depth defaults, configuration and run binding. This change does not impose an
application's maximum depth or introduce new policy storage.

## Reference implementations

The accepted design investigation consulted Claude Code's
[Subagent tool restrictions](https://code.claude.com/docs/en/sub-agents) and
the OpenAI Agents SDK's
[manager and agent-as-tool composition](https://openai.github.io/openai-agents-python/multi_agent/).
Those references separate a child invocation's capabilities from its prompt
and conversation context. Here the existing invocation mask and dispatch
context provide that boundary; another executor or permission service would
duplicate it.

## Options

1. Filter only the inherited prompt/tool descriptions. Rejected because direct
   or stale model calls could still execute a registered tool.
2. Copy the shared tool manager or the first-turn visible set. Rejected because
   the shared catalog is wider than task authority, while deferred tools may
   legitimately be available without being visible yet.
3. Reuse the original available set, `allowed_tools`, invocation and
   execution pipeline. Selected because all modes retain one authority.

## Decision

`AgentDispatchTool` snapshots `ToolContext.tool_visibility.available_names()`
when the invoking context supplies it. This is the immutable available catalog,
not the mutable eligible/visible subset. A cached direct dispatch whose hard
scope excludes `agent_tool` fails before creating a child. An absent visibility
context keeps the original standalone framework behavior.

The original `SubagentContext` carries the known nonempty bound independently
of transcript inheritance, including Fresh mode and calls without a history
factory. Its private merge intersects an existing nonempty bound, rejects an
empty intersection without mutation, and never converts that result into the
historical unrestricted `Some([])` spelling. Shared parent definitions are
only descriptive context and do not grant missing authority.

All concrete Sync, Fork and Teammate invocations carry the original nonempty
set in `AgentInvocationContext.allowed_tools: Option<HashSet<String>>`.
An exclusion list computed before admission cannot express tools registered
while the shared Agent waits for its execution lock. The original snapshot
therefore intersects the then-current catalog with the positive bound after
that lock is acquired. Initial visible schemas remain a separate subset;
omitting them exposes the eligible part of the bounded catalog. A nonempty
bound matching no current tools remains nonempty until this intersection.

This extends the original invocation carrier rather than adding another
permission state. The original available/eligible/visible authority in the
child governs model requests, tool search and execution, so a later dispatch
retains its hard available bound. Team preserves a context
containing only a tool scope and passes it through the existing member dispatch.
The original tool search and execution pipeline apply the same mask; promoting
eligibility cannot add names outside the available set.

`None` and an empty set/list in either `allowed_tools` carrier mean no
additional whitelist. A nonempty bound that matches no tool in a new role
still disables every concrete role tool. It is never rewritten as an empty
declaration that would restore the whole registry. Existing model, skill,
read-only, cancellation, exact-attempt and delegation policies can narrow the
result further and retain their original owners.

## Consequences

- The public Rust invocation structure gains one optional field. Exhaustive
  struct literals must provide it or use the original Default update syntax.
  The existing `ToolRuntime::from_agent` public signature stays unchanged.
  No store, validator, executor, dependency or external wire contract is added.
- A constrained Sync or Teammate dispatch now enforces its already-declared
  whitelist in actual model requests and tool execution, as Fork does.
- A child may use tools its parent deferred from the initial visible set if
  those tools are within the inherited hard available bound and its own role.
- Independent callers with no whitelist remain unchanged. Applications must
  supply trustworthy invocation bounds; missing application run bindings are
  not synthesized by this generic repair.
- Framework nested delegation defaults and consumer-selected maximums remain
  independent. This ADR does not implement any GUI depth setting, TaskRun
  persistence or SDK accepted contract.
- EKO exhaustive literals are adapted by the application owner. The separately
  pinned SDK's inspected literals use Default and need no mechanical field
  addition; a future pin still requires its own verification. Full Rust public
  inventory is telemetry and does not automatically add accepted SDK members.

## Verification and rollback

Test definitions use actual `LlmClient` requests and forced out-of-scope tool
calls across Sync, Fork, Teammate and Team, including zero role matches. They
also cover a nested Fresh factory dispatch to a new role, cached direct
dispatch, hard-set intersection, legacy empty/absent scopes and tool-search
promotion. A deterministic queued-turn control holds the real ReactAgent
execution mutex, observes the original invocation before queue admission,
registers a previously absent tool through the shared Agent, releases the
lock, then checks actual model requests/search and the forced tool side effect.
It covers all four modes through concrete original member dispatch.

Two controlled restorations of prior behavior compiled and failed their selected
assertions: one lost the ancestor scope, and one let a tool registered during
queueing enter later model requests. The new field's structure remained present;
these were narrow semantic controls, not builds of the whole previous revision.
Compilation failure or a missing API does not count as a behavioral counterexample.

Focused `subagent` feature validation passed 22 dispatch, 196 Subagent and 26
snapshot tests: 244 selected, zero failures or ignored tests. All nine new tests
ran. The changed `demo61_agent_factory` compiled under its existing `testing`
feature, and the unchanged communication demo50 compiled under `subagent`.
Focused library/test Clippy and demo61 Clippy passed without warnings.

These results qualify the framework contribution's focused paths. The complete
feature matrix, EKO configuration/run binding and GUI, independent SDK consumer,
native/service behavior and precise main CI still require their own acceptance.
Logs and source identities remain outside the build target; the owned target
was cleaned after the granted Rust batch.

Rollback is a normal source revert. No persisted schema or run record changes.
Reverting restores the old scope-propagation defect and must not be presented
as equivalent enforcement.

## Exact actor policy at the original invocation boundary

Source follow-up found two independent gaps not qualified by the 244 hard-scope
tests above. Sync and Teammate copied a parent's runtime policy while the
request already contained the target child's policy; an actual child could
therefore report the parent depth to its own tools. Snapshot also constructed
tool qualification before selecting the effective policy. An exhausted or
explicitly nondelegating actor could still advertise `agent_tool`, and absent
visibility let tool search read the wider registry.

The existing executor invocation helper now copies the request's target policy
into the original `ExternalRunContext`, creating that carrier when absent.
It does not call `child_policy` or add a level. The original actual dispatch
edge remains the single increment; correct Fork/exact/Team-member target
identities retain their existing values. No product depth constant is added.

Snapshot selects the effective policy once, using its original precedence:
an explicit invocation, including one with no policy, does not fall back to
legacy state. That same value feeds both ToolContext and original ToolRuntime.
An explicit `can_delegate() == false` removes `agent_tool` through the existing
disabled/available/eligible authority and establishes that authority even when
initial visibility is absent. Thus schema, search and forced calls share one
qualification. Policy `None` preserves the original framework behavior. A hard
scope containing only `agent_tool` intersected with a nondelegating policy
remains an empty available catalog, never an unrestricted fallback.
When policy alone requires this qualification, initial schemas retain the
original NoDeferred surface and keep `tool_search` hidden. Explicit deferred
visibility and hard-scope invocations retain their existing search behavior;
direct searches still consume the same qualified available/eligible catalog.

New regression definitions observe actual child ToolContext and model requests
for Sync/Fork/Teammate at maximums 1/2/3 and an independent generic maximum 4,
the original Team coordinator/member edge, explicit primary and legacy policy,
invocation precedence, search and forced stale dispatch with no new start event.
The separate target-policy batch selected all five new definitions and passed
249 distinct tests: 25 dispatch, 196 Subagent and 28 snapshot tests, with zero
failures, ignored tests or compiler warnings. Both existing examples compiled
with their actual features, and focused library/test Clippy passed. Two narrow
restorations of prior behavior each compiled and failed one selected assertion:
the child reported parent depth, and primary model schemas exposed delegation
under an explicit nondelegating policy. The first attempt instead failed to
compile because two new tests matched an Arc event as its enum; no test ran,
and that output is retained separately from behavioral red evidence.

This is a focused framework contribution. It does not qualify the full feature
matrix, EKO GUI persistence, run/recovery binding, role catalog or SDK contracts.
Raw results and exact target cleanup are recorded in
`evidence.subagent-depth-target-policy`; the earlier 244-test hard-scope evidence
remains a separate historical checkpoint.

## 程序化委派桥的完整目标策略

后续源码核对确认，带 runtime context 的 text/message 程序化 delegate helper
曾把调用方已经提供的完整目标 policy 重建为 legacy `policy_from_depth`，导致
Fork 再次覆盖目标上下文中的最大层数与可委派资格。应用冻结的 1/2 上限因此可能
被改成 3。该缺陷属于通用值传递，不应由应用重写调度器或逐工具查询配置修补。

两种 helper 的私有 inner 共用完整策略解析：显式 runtime policy 原样保留三字段，
同时核对原 `depth` 参数指向同一目标层数；冲突在选择/构造目标 Agent 前明确拒绝。
没有完整 policy 的旧调用继续原 legacy 转换。这里不调用 `child_policy`，不增加
public API，也不改变独立复用方的上限、原 exact attempt、取消或 Fork 结算 owner。
应用只负责提供自己原 TaskRun/turn 已冻结的策略。

回归通过真实 ReactAgent/Fork、原 `agent_tool` 和实际 ToolContext 观察，不以提示词
或伪 dispatcher 比字段代替。源码定义与 focused 执行、应用/GUI/main 资格分别记录，
不把本节说明当成尚未取得的完整矩阵或原生验收。
