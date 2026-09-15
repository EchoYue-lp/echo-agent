---
schema_version: 1
id: evidence.lsp-derived-handle-lifecycle-verification
kind: evidence
observed_at: 2eab1ac9923e0f99a70cc08d88de0ee64ea90ee6
source_refs:
  - echo-integration/src/lsp/client.rs
  - echo-integration/src/lsp/manager.rs
  - echo-sdk-host/src/core_profile/facade/integrations.rs
supports: [behavior.extension-publication, behavior.protocol-projection, rule.extension-generation-authority, rule.protocol-role-separation]
limitations:
  - 完整workspace门禁与远端CI留到汇总MR前执行
  - 测试使用fixture LSP child而非所有真实language server
---

# LSP derived handle lifecycle验证证据

## 支持的结论

LSP定向测试12项通过、1项环境型测试ignored，覆盖retained handle在manager shutdown后拒绝
initialize、replacement旧handle失效、spawn后fence变化时kill child，以及Host manager/session/
connection close等待级联结算。

## 来源与范围

独立reviewer沿manager/client/Host真实关闭路径复审，结论pass，Critical、Important、Minor
均为0。

## 已知缺口

未运行真实clangd/rust-analyzer重启矩阵。
