---
schema_version: 1
id: evidence.channel-generation-delivery-fence-repair
kind: evidence
observed_at: source:757f499d4d9a40a4c27791933cb3d5e9d3b2dda76a1a28e4561e317d4719be94
source_refs:
  - echo-core/src/error.rs
  - echo-integration/src/channels/types.rs
  - echo-integration/src/channels/session.rs
  - echo-integration/src/channels/channels/mod.rs
  - echo-integration/src/channels/channels/qq/channel.rs
  - echo-integration/src/channels/channels/feishu/channel.rs
  - docs/adr/0057-channel-generation-delivery-fence.md
  - docs/en/15-im-channels.md
  - docs/zh/15-im-channels.md
supports: [finding.channel-reset-stale-generation-delivery, behavior.protocol-projection, rule.protocol-role-separation]
limitations:
  - A remote provider may already have accepted a request before local reset begins; reset waits for that admitted operation but cannot retract the remote side effect
  - Custom transports that bypass ChannelPlugin::send and the built-in delivery helper own their own final delivery boundary
  - Channel attachments, driven Turn adoption, and Agent close settlement remain separate Findings
---

# Channel generation delivery fence 修复证据

## 支持的结论

`SessionGeneration`继续是sender-scoped channel生命周期的唯一权威。每个incarnation token
共享同一个generation-level current identity与active delivery计数；application `rotate()`
只推进current token，旧token不再接纳新delivery，但旧token已经取得的permit仍由同一计数追踪。

内置QQ/飞书在queue或direct send进入网络边界前取得permit。framework reset先retire权威、
取消旧stream并等待已接纳permit结算，然后才创建replacement并返回reset acknowledgement。
retire后的输出返回typed `ChannelError::StaleDelivery`，不会进入发送队列或网络调用。

stream wrapper在inner setup与逐项poll两个阶段都select同一CancellationToken；setup阻塞、
parked stream、未poll stream、item error、panic/drop仍通过既有`SessionStreamReceipt`精确结算。
未poll stream没有产生输出，因此reset无需等待它；其cleanup callback继续在receipt drop时触发。

## 来源与范围

修复复用既有`SessionHandler`、`SessionGeneration`、bounded `DeliveryRequest`队列、oneshot receipt
和CancellationToken。opaque fence保留在channel模块内部，不形成store、wire field或第二生命周期。
公开Rust面只新增可匹配的`ChannelError::StaleDelivery`；语言SDK与channel wire没有新增合同。

ADR 0057记录Claude Code interrupt与Codex generation/cancellation的参考依据、四个候选方案、
transport admission线性化点、application rotate共享计数及不可撤回远端side effect边界。

## 已知缺口

第三方自定义transport若直接绕过framework `ChannelPlugin::send`和built-in wrapper，必须在自身
最终side-effect边界保留等价generation validation；本修复不伪造全局网络撤回保证。
