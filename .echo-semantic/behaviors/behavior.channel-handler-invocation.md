---
schema_version: 1
id: behavior.channel-handler-invocation
kind: behavior
status: verified
expectation: human_confirmed
risk: high
primary_focus: failure_concurrency
focus: [trigger_input, result_side_effect, state_authority, time_lifecycle, failure_concurrency, contract_evidence]
boundary: boundary.extension-lifecycle
observed_at: source:d0568a66d7ccb8955e15a0d6f7aaaf0799ae25c287264ded14a9f167a4e01fbe
code_refs:
- echo-core/src/error.rs
- echo-integration/src/channels/manager.rs
- echo-integration/src/channels/types.rs
- echo-integration/src/channels/session.rs
- echo-integration/src/channels/mod.rs
- src/channels.rs
- docs/adr/0085-channel-handler-invocation-ownership.md
- docs/en/15-im-channels.md
- docs/zh/15-im-channels.md
rule_refs: [rule.channel-invocation-generation]
evidence_refs: [evidence.channel-handler-forward-full-gates, evidence.channel-handler-forward-focused, evidence.channel-handler-forward-repair]
finding_refs: [finding.channel-handler-invocation-settlement]
---

# 原 Channel entry 的 handler 前向调用

## 重要承诺

成功 start 后，handle_message、handle_message_stream、reply_message 只调用原 entry
的当前 handler；旧 capture 不转向新 generation。每次 cancellation 是原 generation
的 child，只取消该调用；stop 才关闭 generation 准入并取消全部 admitted calls。

## 当前行为

原 entry 持 handler 与 cleanup责任；admission mutex 线性化 ready/open/retire，retire
在同一个 guard 内关闭准入并发取消。driver持原 handler Arc/entry/receipt 至真实结算；
ChannelMessageStream 暴露同次 admission 的 owned流及 child signal，供 adapter 满缓冲
send select，正常EOF真实Join后disarm dropguard而不误报取消。

## 期望行为

legacy 本地 future drop 只证明本地停止；settles_on_cancel consumer 必须继续原驱动
至真实 receipt。SessionHandler 转发每次 child且drain内层，AgentChannelHandler 使用
既有 AgentTurnDriver，不能取消兄弟 Session generation 或伪造外部 terminal。

## 触发、结果与副作用

任何通用framework消费者可经原ChannelHandle调用；typed结果/错误来自原handler。
SDK wire/owner地址、三语言catalog属于SDK；新Rust supporting结构不自动languageaccept。

## 失败、重试与恢复

未启动/starting/failedstart拒绝，wrongchannel InvalidConfig，旧capture typed
StaleInvocation。close失败/取消保留原owner且不重开；queuedreset取消在取sender锁后
复查，不能替换原incarnation。metadata可短锁重入；自身mutation递归await不支持。

## 证据

focused档案有13真实command输出，原22/79/12/2doctest/lint绿及编译/lint失败均保留。
后继retire/open race修复后原证据是历史；新增23/80、lint及全23最终门禁待新源码验证。

## 裁决记录

用户保留三项handler承诺；ADR0085定义通用entry机制，SDKADR0035定义原六family
operations。Finding保持open至当前修复的新验证、独立复审、main验收；不声明全能力闭合。


当前完整23已补齐：evidence.channel-handler-forward-full-gates绑定同source d056，23/80具名回归、warning0、全部feature门禁通过；上文待验为历史准备记录。独立最终receipt复核与SDK/官网/exactmain仍分开，不借本地全量门禁关闭无关问题。


当前限定行为完成：source d056完整工程门禁及独立audit.channel-handler-invocation-rereview
源码/收据PASS0成立，故本对象在上述有限场景verified；不更改原宽对象needs_review，
不声明SDK/官网/main交付完成。
