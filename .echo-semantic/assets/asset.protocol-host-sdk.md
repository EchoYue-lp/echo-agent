---
schema_version: 1
id: asset.protocol-host-sdk
kind: asset
title: ACP、A2A、Channels、Headless 与 SDK Host
asset_type: protocol
status: needs_review
risk: high
observed_at: 78b9f06b4320531fd8f41260887cd69c1343e995
boundary_refs: [boundary.protocol-surfaces, boundary.sdk-facade-parity]
code_refs: [src/acp/runtime.rs, src/a2a/server.rs, echo-integration/src/channels/manager.rs, src/channels.rs, src/headless.rs, echo-sdk-protocol/src/lib.rs, echo-sdk-host/src/lib.rs]
consumer_refs: [tests/acp_agent_adapter.rs, echo-sdk-host/tests/core_profile_e2e.rs, sdks/typescript/src/client.ts, sdks/python/src/echo_agent_sdk/client.py]
behavior_refs: [behavior.protocol-projection, behavior.sdk-facade-routing]
rule_refs: [rule.protocol-role-separation, rule.sdk-rust-authority]
evidence_refs: [evidence.provider-protocol-quality, evidence.sdk-contracts]
finding_refs: [finding.a2a-terminal-authority, finding.a2a-stream-cleanup, finding.channel-attachment-projection, finding.turn-driver-entry-coverage]
candidate_refs: []
---

# ACP、A2A、Channels、Headless 与 SDK Host

## 资产身份

外部调用面、Session/Run projection、capability/handle/generation 和 source SDK transport 的协议集合。

## 来源与消费者

Editor/Client、Agent peer、IM channel、CI/script 与三语言 SDK 消费。

## 生命周期

Initialize/create Session、start Turn/task、stream/update/cancel/replay、close/disconnect/recover。

## 候选关系

各协议角色不同；A2A 自持 TaskState 是否形成第二终态 authority 需 audit。

## 未知与限制

Channel Turn/attachments 与 A2A terminal/cleanup 已形成 Findings；Device sync 不在 framework 中。
