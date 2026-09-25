---
schema_version: 1
id: evidence.channel-attachment-projection-repair
kind: evidence
observed_at: source:5e9a01f48cdf8338bbf8ccfdf225290e1c3f3db90fbea6c30cf1219b92cd1f48
source_refs:
  - src/channels.rs
  - echo-integration/src/channels/types.rs
  - echo-core/src/llm/types.rs
  - echo-orchestration/src/runtime/turn_driver.rs
  - docs/adr/0077-channel-attachment-projection.md
  - docs/en/15-im-channels.md
  - docs/zh/15-im-channels.md
supports: [finding.channel-attachment-projection, behavior.protocol-projection, rule.protocol-role-separation]
limitations:
  - ImageUrl does not retain the original image filename and File cannot retain an absent filename without a generated name
  - Typed File bytes do not guarantee every provider can read arbitrary binary files
  - QQ and Feishu transport media acquisition is outside this adapter
  - Mainline delivery and remote CI remain pending
---

# Channel attachment projection repair

## 支持的结论

`AgentChannelHandler::drive_turn_with_sink` keeps text-only input on the existing
plain-text path. When attachments exist, it creates one ordered typed user
message through the established `Message::user_multimodal` and
`TurnRequest::from_message` contracts. Non-empty text precedes received
attachments; attachment-only input adds no text part. PNG, JPEG, GIF, and WebP
image bytes become MIME-correct data URLs. File bytes become base64 `File`
parts. Unknown images, audio, and video fail before the Turn driver calls the
model rather than silently succeeding with text alone.

## 来源与范围

The framework channel adapter owns this projection, not a new channel-specific
message schema, Turn terminal, or provider capability authority. The root
`ContentPart` contract cannot encode an image's original filename or a
filename-less File; the adapter preserves bytes and order but does not claim
every transport metadata field is lossless.

## 已知缺口

Provider adapters retain responsibility for whether a typed file can actually
be read by a model. The built-in QQ and Feishu transports still advertise their
own media acquisition capabilities. Independent review and mainline delivery
are pending; this evidence alone does not close Finding #40.
