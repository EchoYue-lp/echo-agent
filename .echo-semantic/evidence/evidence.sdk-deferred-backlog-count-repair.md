---
schema_version: 1
id: evidence.sdk-deferred-backlog-count-repair
kind: evidence
observed_at: source:269f99e8904fd56ec35e795640c0f2a742d18a792e88ff4f5b15b4852c1b64f4
source_refs:
  - docs/adr/0031-sdk-identity-governance-scope.md
  - docs/adr/0032-sdk-contract-scope-classification.md
  - contracts/sdk/parity-manifest.json
  - echo-sdk-protocol/src/inventory.rs
  - scripts/check-language-sdks.sh
supports: [behavior.protocol-projection, behavior.sdk-facade-routing, rule.protocol-role-separation, rule.sdk-rust-authority]
limitations:
  - 只修复语义材料中的backlog名称、数量和scope边界，不修改manifest、route、language status或runtime
  - 1441个deferred identity仍需按capability判断，本Evidence不将其标为已交付
---

# SDK deferred backlog口径修复证据

## 支持的结论

Workspace discovery和protocol capability map现统一采用ADR 0032的唯一consumer-facing口径：当前待产品合同决策的是1441个`deferred` identity，推进单位是externally useful capability，不是单个identity。

Protocol场景已从`sdk-intrinsic-backlog`改名为`sdk-deferred-backlog`。Host/Rust-only、language intrinsic、internal helper和已经接受的external contract均保留各自disposition，不构成语言parity backlog。

## 来源与范围

范围只包括两处语义材料和Finding #116。数量来自schema v2 parity manifest的当前canonical scope统计，并由Rust inventory及三语言catalog gate重算。

## 已知缺口

本修复不决定deferred capability的外部价值、不新增SDK接口，也不关闭其它71个open Finding。远程CI、PR/merge和发布仍未执行。
