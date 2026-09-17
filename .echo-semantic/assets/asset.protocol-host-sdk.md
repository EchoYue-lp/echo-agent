---
schema_version: 1
id: asset.protocol-host-sdk
kind: asset
title: ACP、A2A、Channels、Headless 与 SDK Host
asset_type: protocol
status: deprecated
risk: high
observed_at: 9d1f3f2b5fdc204c08ecdec32ed22e8df95870e9
boundary_refs: [boundary.protocol-surfaces, boundary.sdk-facade-parity]
code_refs: [src/acp/runtime.rs, src/a2a/server.rs, echo-integration/src/channels/manager.rs, src/channels.rs, src/headless.rs, echo-sdk-protocol/src/lib.rs, echo-sdk-host/src/lib.rs, contracts/sdk/parity-manifest.json]
consumer_refs: [tests/acp_agent_adapter.rs, echo-sdk-host/tests/core_profile_e2e.rs, sdks/typescript/src/client.ts, sdks/python/src/echo_agent_sdk/client.py]
behavior_refs: [behavior.protocol-projection, behavior.sdk-facade-routing]
rule_refs: [rule.protocol-role-separation, rule.sdk-rust-authority]
evidence_refs: [evidence.provider-protocol-quality, evidence.sdk-contracts, evidence.tool-registry-owned-handle-verification, evidence.background-task-terminal-authority-verification]
finding_refs: [finding.a2a-terminal-authority, finding.a2a-stream-cleanup, finding.channel-attachment-projection, finding.turn-driver-entry-coverage, finding.sdk-repository-extraction]
candidate_refs: [asset.sdk-source-product, asset.framework-acp-adapter]
---

# Historical combined protocol and SDK asset

## 资产身份

该聚合资产记录迁移前把 framework protocol surfaces 与 SDK Host 混在一起的历史观察。

## 来源与消费者

其旧消费者包括 Editor/Client、Agent peer、IM channel、CI/script 与三语言 SDK；当前 SDK
产品已由 `asset.sdk-source-product` 在独立仓库拥有。

## 生命周期

Initialize/create Session、start Turn/task、stream/update/cancel/replay、close/disconnect/recover。

## 候选关系

各协议角色不同；该聚合资产已 deprecated，新的 framework ACP adapter 与外部 SDK product
分别承担其当前边界。

## 未知与限制

历史 Channel/A2A Findings 继续按各自 framework map 维护；SDK Host、protocol 和合同的后续
语义在独立仓库复核。
