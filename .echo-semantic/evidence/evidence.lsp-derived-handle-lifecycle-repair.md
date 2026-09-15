---
schema_version: 1
id: evidence.lsp-derived-handle-lifecycle-repair
kind: evidence
observed_at: 2eab1ac9923e0f99a70cc08d88de0ee64ea90ee6
source_refs:
  - echo-integration/src/lsp/client.rs
  - echo-integration/src/lsp/manager.rs
  - echo-sdk-host/src/core_profile/facade/integrations.rs
  - docs/adr/0043-lsp-derived-handle-lifecycle.md
supports: [behavior.extension-publication, behavior.protocol-projection, rule.extension-generation-authority, rule.protocol-role-separation]
limitations:
  - LSP runtime status与restart字段仍由finding.lsp-runtime-state追踪
  - 本证据不覆盖MCP transport cleanup
---

# LSP derived handle lifecycle修复证据

## 支持的结论

`LspManager`是唯一child-process owner。Manager创建的client共享closed fence与generation；
`shutdown_all`先关闭fence，再await每个client teardown。保留的派生handle在manager关闭或
同语言replacement后立即stale，initialize在spawn前后及握手各边界复核generation，失效时
abort并依赖kill-on-drop防止child脱离。

SDK Host显式shutdown、Session close与connection close均先take manager并await
`shutdown_all`，之后才清理派生record，不能留下可复活的独立owner。

## 来源与范围

最终实现固定于commit `2eab1ac9923e0f99a70cc08d88de0ee64ea90ee6`，复用现有
LspManager/LspClient与Host handle registry。

## 已知缺口

Status/restart/backoff的对外字段与运行事实仍由#64处理。
