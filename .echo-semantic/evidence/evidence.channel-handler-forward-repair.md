---
schema_version: 1
id: evidence.channel-handler-forward-repair
kind: evidence
observed_at: 554f2f50d3705d219519f306055a3d26d6a5211c
source_refs:
- echo-core/src/error.rs
- echo-integration/src/channels/manager.rs
- echo-integration/src/channels/types.rs
- echo-integration/src/channels/session.rs
- echo-integration/src/channels/mod.rs
- src/channels.rs
- docs/adr/0085-channel-handler-invocation-ownership.md
- docs/en/15-im-channels.md
- docs/zh/15-im-channels.md
- docs/evidence/channel-handler-forward-20261014/README.md
- docs/evidence/channel-handler-forward-20261014/command-receipts.json
- docs/evidence/channel-handler-forward-20261014/source-freeze.json
- docs/evidence/channel-handler-forward-20261014/current-source-freeze.json
- docs/evidence/channel-handler-forward-20261014/review-increment-manager.rs.diff
- docs/evidence/channel-handler-forward-20261014/review-increment-session.rs.diff
supports: [behavior.channel-handler-invocation, rule.channel-invocation-generation]
limitations:
- Current retirement-mutex fix requires fresh23/80/lint and full23; historical logs do not validate new manager source.
- SDK native consumers, website regeneration and exact-main delivery require their independent evidence.
---

# Channel forward 修复依据

## 支持的结论

原entry/SessionHandler/AgentChannelHandler完整责任与typedcancel支持见ADR0085；配额/SDKresource属于SDK。
原22新增test、79完整Channeltest、12rootAgenttest、2完整doctests与双组strictclippy
真实通过；原始previous源SHA精确ff665397/a44e937d，final源SHA5e0c866a/7155998d。

## 来源与范围

command-receipts列准确命令、退出码、时间和13日志hash。final-sources逐文件字节保留，
previous-sources精确校验后给完整review-increment diff。以真实80353501候选HEAD构建，
随后实际main77de合入0f692668，无源码冲突；mutexrace新改的currentfreeze单独标记待验。

## 已知缺口

新mutexrace是同guard线性化修复，OSbarrier测试针对signal边界锁持有而非宣称真实
message误准入。旧日志不可冒新sourcegreen。新的23/80与lint、final全23和独立复核
以及框架/SDKmain交付尚待。不得以此Evidence关闭无关Finding或扩完整Rustinventory。


## 已发布历史快照定位

本对象原 source:d0568a66d7ccb8955e15a0d6f7aaaf0799ae25c287264ded14a9f167a4e01fbe 的完整非语义源码与已发布 Git 554f2f50d3705d219519f306055a3d26d6a5211c 唯一摘要相同。全部本对象code/source引用逐blob核对当前仍相同，现只重绑定可恢复Git定位；原N审查范围、运行时间、状态与结论保持。此历史事实不覆盖D新增Database/RAG/Clinical三源码、其测试或消费者，更不冒作本轮门禁通过。
