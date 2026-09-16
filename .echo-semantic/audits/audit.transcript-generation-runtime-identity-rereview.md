---
schema_version: 1
id: audit.transcript-generation-runtime-identity-rereview
kind: audit
boundary_ref: boundary.context-memory
lens: data_durability
freshness: examined
revision: source:71db36711961e65e13fcef99f7f7e671ddd391e4853eecc77c4e07ac041915f6
finding_refs: [finding.transcript-generation-runtime-identity]
challenges:
  resolver-precedence:
    revision: source:71db36711961e65e13fcef99f7f7e671ddd391e4853eecc77c4e07ac041915f6
    source_refs: [src/agent/snapshot.rs, src/agent/react/run/stream_channel.rs]
    evidence_refs: [evidence.transcript-generation-runtime-identity-repair, evidence.transcript-generation-runtime-identity-verification]
  admission-side-effects:
    revision: source:71db36711961e65e13fcef99f7f7e671ddd391e4853eecc77c4e07ac041915f6
    source_refs: [src/agent/react/run/stream_channel.rs]
    evidence_refs: [evidence.transcript-generation-runtime-identity-verification]
  snapshot-write-bypass:
    revision: source:71db36711961e65e13fcef99f7f7e671ddd391e4853eecc77c4e07ac041915f6
    source_refs: [src/agent/snapshot.rs, src/state/mod.rs]
    evidence_refs: [evidence.transcript-generation-runtime-identity-repair, evidence.transcript-generation-runtime-identity-verification]
---

# Runtime state 与 transcript generation identity 独立复审

## 审查范围

复审 effective identity precedence、stream admission 顺序、snapshot save 边界、None 兼容、错误分类、
public invocation rustdoc、SDK/wire 影响和故障注入测试。

## 已检查故障假设

- resolver 改变显式 invocation、产品 conversation、legacy 或 configured precedence；
- mismatch 等待 execution mutex 或先执行 guard、trace、input drain、context、LLM；
- public snapshot 调用绕过 admission 后仍能写入坏 checkpoint；
- None 或 A/A 兼容路径被错误拒绝；
- 新 helper 扩大 external public API 或 SDK contract。

## 实际实现路径与证据

独立实现 Review 和两项 Minor 修复后的增量 Review 均 pass；最终结果为 0 Critical、0 Important、
0 Minor。Resolver 与原 precedence 等价；validator 位于所有 admission 副作用之前，并在统一 checkpoint
write boundary 再次调用；helper 仅为 crate-private，public 字段形状和 wire route 不变。

## 问题记录

初次 Review 的两项 Minor（public rustdoc 可发现性、mutex/guard/input drain 直接断言）均已修复，
增量复审未发现新问题。

## 残余风险

ConversationStore projection 的 durable settlement 不属于本 Finding，继续由 #106 跟踪。

## 未检查项

未检查远端 CI 平台差异；未扩大到 transcript projection backend 的失败恢复。
