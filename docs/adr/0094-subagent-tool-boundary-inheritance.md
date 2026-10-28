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

## 2026-10-27：调用级只读能力与受限委派

### 背景与候选

真实只读 reviewer 已声明可委派，但普通 `agent_tool` 的 Mutating 能力被原只读快照正确过滤，合法下一层因此不可达。仅把全部 dispatcher 标成 ReadOnly 会扩大普通 writer 的能力；仅核目标角色标签又无法防止错误只读标签的 Agent 实际装入写工具。二者均不采用。

采用原 `ToolAccess` 的可选调用上界，沿 `AgentInvocationContext`、`ToolContext`、`SubagentContext` 三个既有载体传递。它不形成新的权限 store、角色策略或任务状态。`None` 保留旧调用，任何已有 ReadOnly 均不得被 Mutating 值提升；Fresh、空历史及没有 Goal 时仍保留 access-only context。

### 决策与数据流

只读 Agent 的原装配选择 `AgentDispatchTool::read_only`，普通 `new` 继续声明 Mutating；受限实例的 capabilities.access 为 ReadOnly，模型 catalog 只列只读目标；运行 risk 保留 Standard，沿原 `tool_search` 的分类方式避免结果缓存。调用必须具备原 `ToolContext` 的有效 visibility，从不可变 available 集继承父工具上界；缺上下文、缺范围或空交集明确失败，不回退无限制。此检查防本地运行时丢失约束后误写用户文件，不限制用户直接打开终端或配置 MCP，也不要求 full-auto。

同一不可变 registration 在 factory 前合并目标自身的 ReadOnly 声明与祖先上界，再验证只读调用不得选择 writer。目标声明是已有 typed mutation boundary，普通 writer 父或 None 调用也不能让被标为 ReadOnly 的目标误执行写工具；这个上界保留在原请求中，失败后的重试/转派不能降权。后续执行仍用该 record；Team 的实际成员/manager 同样逐项进入原准入，不按外层 Team 标签放行写成员。Sync、Fork、Teammate、Team 与后台沿原执行器传递同一调用值，不另建调度或改变取消、物理结算与深度计数。

Agent 获得原执行锁后，`ToolRuntime` 从当前 catalog 计算配置只读与调用 ReadOnly 的有效值。模型 schema、tool_search 可用集及最终工具执行共同沿该快照过滤，再与父工具正向集合、disabled/model/skill/depth 条件求交；同名替换或排队后注册写工具不能扩大权限。`ToolRuntime::from_agent` 仍保原公开签名；只有内部构造接受调用上界。默认 `Agent` 三个 invocation 回退不能落实 ReadOnly，因此在旧 execute/chat 开始前明确返回不支持，None 正控制不变；ReactAgent 和 Arc 转发承接完整上下文。

### 影响与验收

追加公开 Rust 字段要求使用完整 struct literal 的调用方明确补值，使用 Default 的原代码无需机械改写；不宣称完全源码兼容，不自动升级 SDK pin/accepted 或把完整 public inventory 当跨语言承诺。Framework 保留合理 max4/None/writer 消费能力；EKO 的默认总层数1、GUI最多3及真实 reviewer 工厂资格归应用。

要求真实旧只读 schema/工厂行为先红后绿，验证四模式/后台、Fresh/access-only、同一 record、writer/错误标签/Team混写、工具搜索/强制调用/晚注册/同名替换及默认回退负控制。执行层数、公开字段 literal 与示例需适用编译；原生、完整 feature/工程矩阵、精确 main/CI 和 SDK 独立来源分别验收，定向通过不替代它们。官方参考沿本 ADR 与跨仓已审设计的原件，不把注册表 `tool_names()` 当实际模型曝光证据。

### 安全收口补充：只读能力不等于派发结果可缓存

`ToolAccess::ReadOnly` 约束后代不得改写用户环境；派发本身仍建立新的子调用生命周期与真实回执。原 ToolManager 以 `ToolRiskLevel::ReadOnly` 进入成功结果缓存，其上下文键不包含每个独立 call_id，因此不能把受限派发也按普通可缓存读取分类。这里复用原 tool_search 的既有模式：普通和受限 dispatcher 的运行 risk 均为 Standard，只有受限实例的 capability access 为 ReadOnly。只读 schema、搜索和最终准入继续检查 access，不依赖缓存分类。

没有新增 cache trait、工具名特例或全局缓存策略；两次相同参数但独立 call_id 的合法派发必须实际建立两次子调用，不重放旧结果或旧“已启动”回执。该回归经原 ToolManager 实际入口取得一次实际调用/预期两次的行为失败，再验证修复；这项局部修复不代表当前只读整体、原生或后继原生 reasoning/assistant 执行切换已经完成。
