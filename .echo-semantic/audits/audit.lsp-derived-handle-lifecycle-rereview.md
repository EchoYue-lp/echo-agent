---
schema_version: 1
id: audit.lsp-derived-handle-lifecycle-rereview
kind: audit
boundary_ref: boundary.extension-lifecycle
lens: time_lifecycle
freshness: examined
revision: 2eab1ac9923e0f99a70cc08d88de0ee64ea90ee6
finding_refs: [finding.lsp-manager-derived-handle-resurrection]
challenges:
  manager-close-cascade:
    revision: 2eab1ac9923e0f99a70cc08d88de0ee64ea90ee6
    source_refs: [echo-integration/src/lsp/client.rs, echo-integration/src/lsp/manager.rs]
    evidence_refs: [evidence.lsp-derived-handle-lifecycle-repair, evidence.lsp-derived-handle-lifecycle-verification]
  host-derived-record-settlement:
    revision: 2eab1ac9923e0f99a70cc08d88de0ee64ea90ee6
    source_refs: [echo-sdk-host/src/core_profile/facade/integrations.rs]
    evidence_refs: [evidence.lsp-derived-handle-lifecycle-verification]
---

# LSP derived handle lifecycle独立复审

## 审查范围

Reviewer检查manager/client共享lifecycle、spawn/initialize fence、replacement、Host显式shutdown、
Session close和connection close；runtime status字段排除。

## 已检查故障假设

检查manager关闭后旧handle重启child、spawn与close竞态泄漏进程、replacement旧handle复活，
以及Host先删manager record却不等待child teardown。

## 实际实现路径与证据

所有派生client共享manager generation/closed authority；关闭先fence再await，旧handle在每个
spawn/handshake边界失败并abort child。最终review pass。

## 问题记录

Critical 0、Important 0、Minor 0，#63可关闭。

## 残余风险

#64继续追踪status/restart contract。

## 未检查项

未执行所有真实language server的故障矩阵。
