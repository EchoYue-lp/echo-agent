---
schema_version: 1
id: evidence.sdk-deferred-backlog-count-verification
kind: evidence
observed_at: d492c676d1bf0744452d96a6960124546ed3fff9
source_refs:
  - docs/adr/0031-sdk-identity-governance-scope.md
  - docs/adr/0032-sdk-contract-scope-classification.md
  - contracts/sdk/parity-manifest.json
  - echo-sdk-protocol/src/inventory.rs
  - scripts/check-language-sdks.sh
supports: [behavior.protocol-projection, behavior.sdk-facade-routing, rule.protocol-role-separation, rule.sdk-rust-authority]
limitations:
  - Change-evidence在同一working tree内同时覆盖已授权的Plan 13 ADR/索引和Plan 14语义修复；Plan 14自身diff仍只包含.echo-semantic
  - 不替代远程CI、PR/merge、发布或1441个deferred identity的后续capability决策
---

# SDK deferred backlog口径验证证据

## 支持的结论

对当前parity manifest按canonical `sdk_scope`重算得到：`external_contract=5607`、`host_or_rust_only=1765`、`language_intrinsic=781`、`internal_helper=90`、`deferred=1441`。Workspace discovery与protocol map都使用1441 deferred capability backlog，且两处不再出现4076或4,076旧口径。

Semantic strict snapshot exit 0。以`1cb25e80515ea17624fe652be1fd29c096b9a880`为当前复合候选preflight base的`--require-change-evidence` exit 0；preflight显式声明Plan 13已有ADR/索引与Plan 14语义修复的联合允许路径，没有将其伪装成Plan 14单独修改正式文档。

包含本修复的临时候选tree `ee3c22217803cf78df7deab54fa6a285a802d9db`对`b21aba01b34e74c93d783a89db895282ba831c3c`执行semantic continuity，428个义务为408 preserved、4 replaced、16 retired，`passed=true`且errors为空。

## 来源与范围

验证包括结构化manifest计数、限定discovery/maps的旧口径搜索、strict snapshot、task-scoped change-evidence和Git tree continuity。修复diff只改变Finding、两处语义表述及repair/verification Evidence，不改变Rust、Cargo、contracts、SDK、examples或正式docs。

## 已知缺口

Finding在独立rereview前保持open。Issue #116必须等修复进入远程main后才能关闭；其它71个open Finding不受本修复影响。
