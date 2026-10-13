---
schema_version: 1
id: rule.channel-invocation-generation
kind: rule
status: verified
expectation: human_confirmed
risk: high
primary_focus: state_authority
focus: [state_authority, time_lifecycle, failure_concurrency, contract_evidence]
observed_at: source:d0568a66d7ccb8955e15a0d6f7aaaf0799ae25c287264ded14a9f167a4e01fbe
behavior_refs: [behavior.channel-handler-invocation]
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
evidence_refs: [evidence.channel-handler-forward-full-gates, evidence.channel-handler-forward-repair, evidence.channel-handler-forward-focused]
finding_refs: [finding.channel-handler-invocation-settlement]
---

# Channel invocation generation 唯一权威

## 不变量或唯一权威

原 ChannelEntry 的handler、successful-start gate、generation、receipt与cleanup是一套
权威。SDK不可裸clone handler另建map。retire ready=false和token.cancel必须在同一原
admissionMutex内完成；open不能在signal前重开。调用receipt释放依据真实driver退出。

## 适用行为

handle/stream/reply、callcancel、Channel stop/restart、SDKmanager/resource/Session close。

## 当前实现

ChannelMessageStream的token与流来自同次admit，无TOCTOU token查询。driver捕原Arc
在内部创建trait借用流，无unsafe/lifetime泄漏。自然EOF disarm；drop/stop只取消原child。
SessionHandler与AgentChannelHandler复用既有session/fence/turndriver，没有第二执行器。

## 期望行为

不以cancel通知、future drop、senderEOF代替外部receipt；关闭失败原owner可重试。
取消排队reset不能替换session。旧token不取消新generation或兄弟调用。

## 证据

历史focused与source hash见两份Evidence；真实OS thread barrier验证retirement signal
边界guard持有，不声称测试已观察实际handler误准入。新源码execution仍待最终门禁。

## 裁决记录

ADR0085记录generic机制、SDK边界与RustHost-only supporting表示；完整inventory不
扩大acceptedscope，EKOpolicy不下沉。仅此slice观察，不改变其它Rule needs_review。


当前完整23已补齐：evidence.channel-handler-forward-full-gates绑定同source d056，23/80具名回归、warning0、全部feature门禁通过；上文待验为历史准备记录。独立最终receipt复核与SDK/官网/exactmain仍分开，不借本地全量门禁关闭无关问题。


当前限定行为完成：source d056完整工程门禁及独立audit.channel-handler-invocation-rereview
源码/收据PASS0成立，故本对象在上述有限场景verified；不更改原宽对象needs_review，
不声明SDK/官网/main交付完成。
