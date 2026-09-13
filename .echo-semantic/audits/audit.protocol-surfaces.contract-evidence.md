---
schema_version: 1
id: audit.protocol-surfaces.contract-evidence
kind: audit
boundary_ref: boundary.protocol-surfaces
lens: contract_evidence
freshness: examined
revision: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
finding_refs: [finding.a2a-terminal-authority, finding.a2a-stream-cleanup, finding.a2a-advertised-capability-binding, finding.channel-attachment-projection, finding.turn-driver-entry-coverage, finding.sdk-gap-generation-validation-parity]
challenges:
  protocol-role-and-capability:
    revision: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
    source_refs: [src/a2a/server.rs, src/a2a/types.rs, src/channels.rs, echo-integration/src/channels/types.rs, src/headless.rs]
    evidence_refs: [evidence.provider-protocol-quality]
  sdk-handle-gap-and-replay:
    revision: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
    source_refs: [echo-sdk-host/src/core_profile/handles.rs, echo-sdk-host/src/core_profile/handler.rs, echo-sdk-host/tests/core_profile_e2e.rs, sdks/typescript/src/client.ts, sdks/python/src/echo_agent_sdk/client.py, sdks/java/src/main/java/com/echoagent/sdk/EchoAgentClient.java, sdks/java/src/main/java/com/echoagent/sdk/BoundedPublisher.java]
    evidence_refs: [evidence.provider-protocol-quality, evidence.sdk-contracts]
  sdk-inventory-scope:
    revision: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
    source_refs: [contracts/sdk/parity-manifest.json, docs/adr/0031-sdk-identity-governance-scope.md]
    evidence_refs: [evidence.sdk-contracts, evidence.sdk-pr23-squash-continuity]
---

# Protocol Capability、SDK Handle 与 Evidence 审计

## 审查范围

审查 ACP/A2A/Channel/Headless/SDK role/capability，Host handle generation/replay/ACK，三语言 gap validation和 SDK inventory 治理范围。

## 已检查故障假设

验证协议 capability是否绑定真实实现、Channel attachment是否投影、SDK gap是否验证完整 handle，以及 identity inventory是否被误作行为完成。

## 实际实现路径与证据

Host generation fencing、replay、ACK 有实现和 E2E；ADR 0031正确限定 inventory为漂移 telemetry。TS gap校验完整 handle，Python/Java主要按 stream id并用外来 watermark，缺 wrong-generation反例测试。A2A AgentCard可宣告 file input和push notifications，Server只实现 text压缩与 send/subscribe/get/cancel，无push delivery。Channel公开 attachments但adapter text-only。

## 问题记录

确认 A2A/Channel/Turn Findings；新增 SDK gap generation parity与A2A advertised capability binding。A2A text+SSE vs file/push、Channel multimodal vs text-only、gap item vs terminal error均需 semantic-decide。

## 残余风险

无论 gap表现形式如何，完整WireHandle generation校验必须三语言一致；SDK intrinsic count不进入全项目完成门禁。

## 未检查项

未检查第三方A2A互操作、网络fault、发布包安装或EKO product adapter。
