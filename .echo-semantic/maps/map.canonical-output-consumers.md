---
schema_version: 1
id: map.canonical-output-consumers
kind: capability_map
title: 当前原生输出消费者与微信业务回执导航
risk: high
observed_at: source:ce50542c49d53ec1c636890ed8a0d5f9ba45701cf0863af23052b0164235802c
boundary_refs: []
behavior_refs: []
rule_refs: [rule.turn-terminal-authority, rule.protocol-role-separation]
evidence_refs: []
finding_refs: []
audit_refs: []
related_map_refs: [map.agent-session-turn, map.protocol-surfaces]
scenarios:
  canonical-output-consumers:
    status: mapped
    source_refs: [echo-core/src/agent/output_projection.rs, echo-core/src/agent/mod.rs, src/agent/subagent/executor.rs, echo-orchestration/src/workflow/node.rs, docs/adr/0102-canonical-agent-consumers.md]
    rule_refs: [rule.turn-terminal-authority]
  current-weixin-business-receipts:
    status: mapped
    source_refs: [echo-integration/src/channels/channels/weixin/wire.rs, echo-integration/src/channels/channels/weixin/api.rs, echo-integration/src/channels/channels/weixin/channel.rs, echo-integration/docs/weixin-business-acceptance.md]
    rule_refs: [rule.protocol-role-separation]
---

# 当前原生输出消费者与微信业务回执导航

## 能力范围

本对象只记录当前源码中的纯派生投影职责和微信业务接受边界。场景 mapped 表示已有
源码、所属文档和原规则可追踪，不表示历史运行等价、全项目闭合或下游已验收。
boundary_refs/behavior_refs 显式为空：原两地图继续拥有各自边界与行为关系；本对象
只通过 related_map_refs 和原规则提供当前来源导航，不新增或重复领域权威。

## 入口与输出

Subagent 与 Workflow 复用当前 Agent Message/invocation 入口并消费原生事件。
微信 getupdates/sendmessage 仍由原 client 与通道请求链返回业务状态或回执。

## 行为关系

AgentOutputProjection/AssistantTextDelta 仅归并已有块和身份。runtime FinalAnswer
通知与回调保留；不得按旧 FinalAnswerTool 的退役删除当前运行时通知。
微信业务说明由所属 integration 文档维护，不以 HTTP 成功补造 ACK 或 Received。

## 状态与数据流

完整 Message/history 继续传递，reasoning 与普通文字分离，signature/opaque 不成为正文。
已验证的新 native terminal 才能清理匹配 unknown；foreign unknown 保持优先。
terminal_failure 只读原失败事实，交给原消费者首因槽，不新建执行或持久化权威。

## 策略来源与优先级

遵循 ADR0102 与原 terminal/protocol 规则。getupdates 在无拒绝码、无 null 业务码时，
明确 ret/errcode 业务0或 typed msgs/cursor 才提供接受证据。sendmessage 的缺 ret ACK
要求原 typed 非空 ID 且无非零/null码；原明确 ret=0 与 String/u64 decoder 保留。

## 生命周期与失败路径

消费者必须继续读原 source 到物理 EOF 后才能 finish；尾部 cleanup 不覆盖更早的
原生取消类型或失败原因，Settled 后唯一 cleanup 错误仍使执行失败。微信复用原请求、
client_id、发送回执、cursor/ACK、Stop 与退出 owner；Unknown 不回填、不自动重发。

## 权限与敏感信息

原 client 的单次有界诊断只记允许字段的 presence/type、数值业务码、计数及受限响应
长度/SHA；不输出 body、token、cursor、消息内容或未知键名。不新增权限门控或 poller。

## 用户侧投影

Guard 安全最终正文完整呈现且不双拼。Accepted 仅是原请求的服务器业务回执，
不证明手机 Received；源内测试不授新 App、SDK Host、真实微信或 Main CI 资格。

## 场景处置清单

两个场景仅绑定本对象的当前来源，不借旧地图 observed_at 或旧六个 matched。
既有119项保留票与最小 lint 修正后的受影响复验、四 crate lint 均保各自源码身份；
本 metadata 更新未运行新 Cargo，不把原票重标为新执行。

## 未展开项

旧地图的 needs_review/unknown、Finding、限定等价 Evidence 与旧45/外部#10/旧402
保持原状；SDK inventory/pin、完整 workspace/matrix、真实账号和手机收件另验。
后继18组与 ProviderError 候选尚未正式采用，不由本图获得资格。

## 当前来源

observed_at 来自原 verify_semantic.py 的 source_digest API 对当前正式非语义文件的
实际读取；该摘要不包含本对象与其它 .echo-semantic 材料，因此没有自引用循环。
旧地图仅导航到本对象，不把新 helper 归入旧 Git/source 观察。
