# ADR 0081: Token-budgeted recent conversation tail

## Status

accepted

## Context and alternatives

A message-count split can put the active user request into the summarized
span when a single turn produces many tool results. Keeping all user messages
forever is unbounded; duplicating the EKO goal store in the framework would
create a second authority. Existing ContextManager, compressors, projection,
transcript and runtime-checkpoint boundaries already provide the owners.

The selected reference patterns are Codex's bounded retained user material
and reconstructed initial context, and Pi's token-budgeted recent tail with
tool-call/result boundaries and split-turn handling:

- https://github.com/openai/codex/blob/53eaa297e595fc98df0f33d4c63686a7014d7c9a/codex-rs/core/src/compact.rs
- https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/compaction.md

These patterns support bounded raw material plus summary and reconstructed
authority. They do not require a second runtime or permanent raw user history.

## Decision

Extend SummaryCompressor, IncrementalSummaryCompressor and
SlidingWindowCompressor with `with_recent_token_budget`. Zero retains the
legacy constructor's message cap. One internal pure selector partitions old
and retained messages; it owns no state. Token mode retains older turns whole.
A large active turn may retain its exact request plus recent atomic execution
blocks. Its request may exceed the soft recent allowance, but cannot exceed
the hard input allowance. Such input fails explicitly, without text truncation.

`FrameworkConfig.agent.compress_window = 0` opts into a recent allowance of
25% of the live Agent window, capped at 20K tokens. Positive values retain
the legacy cap. Framework defaults remain compatible; applications choose
their default. Hybrid's configured stages share this allowance. Adaptive
levels retain their existing separate policy.

The automatic compact boundary reads the latest real request before horizon
folding and passes it as summary focus. Runtime notes and projections are not
user requests. Protected projections, canonical system context, transcript,
checkpoint and long-term memory keep their existing owners. A noncontiguous
selection does not pretend that its covered messages form one index range.

Manual focus and provider cancellation flow through one ContextManager
operation and one ReactAgent hook/trace lifecycle. Protected payloads are
subtracted before compression and the reconstructed result is checked against
the input allowance. Cancellation before transform commit preserves context;
product journal settlement after a successful transform stays application-owned.

## Impact and validation

Incremental calculation derives its previous checkpoint from accepted input
messages. Its `current_summary`/`current_structured_summary` cache is an
observation only. `ContextCompressor::context_committed` has a default no-op;
ContextManager invokes it after final budget/verification/promotion succeeds,
and on explicit restore/clear. Hybrid forwards the notification. A rejected or
cancelled candidate cannot publish this cache. Standalone compressor users
notify acceptance explicitly rather than treating calculation as a commit.

Runtime Hook notes and rich tool attachments use the existing runtime-context
marker, including multimodal text. Tool batches close over their call IDs across
these notes; no User-role image produced by a tool becomes the active request.
Cancellation is checked before calculation and before acceptance. A zero remaining
allowance fails before fallback can reinterpret zero as a legacy policy.

Existing Rust constructors and method signatures remain available. The optional
methods are reusable Rust API, with no new protocol, store or lifecycle owner.
EKO's goal/recovery state remains in echo-agent-cli. Rollback removes the new
opt-in config and restores the old constructors; transcript history is retained.

Regression coverage includes an exact Chinese request before a tool batch,
whole recent turns, a split active turn, oversized input, repeated summary and
incremental passes, protected budget accounting and pre-transform cancellation.

## 补充决策：必要投影与最终请求的可用性

有界引用能够避免将完整材料永久塞入 protected context，但读取引用必须在当前真实
请求中可用。原 best-effort 投影失败后继续的策略仍适合可选观察，不能代表所有输入
都是可选。框架因而在原 `PreModelContextProjector` 增加默认 false 的 `is_required`
与默认成功的 `validate_model_request`，复用原 compact/think 错误消费点和有限执行
生命周期。后者在最终请求构建后读取该请求与实际 artifact 配置，只核验、不改写。

候选比较：全局将旧投影错误变 fatal 会破坏原可选消费者；应用共享失败标志、文本
marker 解析或第二 validator 会脱离原 run 与调用；只确认注册表里存在 reader 会漏掉
本次工具上界或 final-only 的移除。所选默认方法保留未采用者行为，把必须性留给调用方。

选择 required 的失败沿既有 `AgentFailure.code` 传递
`required_context_unavailable`，不新增错误枚举、SDK 协议、store 或执行器。
原取消与物理未结算仍走自己的 typed 通道，不冒充来源缺失。应用用原暂停/恢复服务
消费该代码；框架不认识产品 TaskRun 或 NeedsInput。

本次已核实的一手模式依据是
[LangChain 官方 middleware](https://docs.langchain.com/oss/python/langchain/middleware/custom.md)
允许显式调用 handler、重试或重新抛出失败。将其用于必要输入失败阻断、可选输入
显式降级，是结合本项目既有错误和上下文边界的推论；该文档没有本项目同名 API，
也没有替本项目决定所有投影均为必要输入。

验证包括 optional 失败旧行为、required 错误停止、取消/未结算保留，以及在真实
最终请求中工具被移除时停止而模型零调用；本轮四项框架定向回归已实际通过。
EKO 的正式任务与普通聊天也已通过具名的缺失来源暂停、恢复及硬工具上界回归。
这些结果不代表完整有效要求、全部 consumer、SDK、全量 feature 矩阵或 main 已合格；
产品来源身份、作用域与版本仍由应用自身的实际 consumer 证据负责。
