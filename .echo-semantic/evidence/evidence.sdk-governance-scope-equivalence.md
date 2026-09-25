---
schema_version: 1
id: evidence.sdk-governance-scope-equivalence
kind: evidence
observed_at: c5f7688212d45d5bdcdbf60342605e8bfb176cae
source_refs:
  - docs/adr/0028-source-first-multilanguage-sdk-runtime.md
  - docs/adr/0031-sdk-identity-governance-scope.md
  - docs/adr/0032-sdk-contract-scope-classification.md
  - docs/adr/0041-semantic-governance-continuity.md
  - echo-sdk-protocol/src/inventory.rs
  - echo-sdk-protocol/tests/facade_inventory.rs
  - echo-sdk-host/src/core_profile/facade/source_operations.rs
  - echo-sdk-host/tests/core_profile_e2e.rs
  - contracts/sdk/parity-manifest.json
  - contracts/sdk/parity-manifest.schema.json
  - scripts/check-sdk-contracts.sh
  - scripts/check-language-sdks.sh
supports:
  - behavior.sdk-facade-routing
  - evidence.sdk-contracts
  - map.sdk-facade-parity#scenario:source-operation-closure
  - rule.sdk-rust-authority
  - map.sdk-facade-parity#scenario:sdk-contract-scope
limitations:
  - 只证明SDK治理从逐identity完成度转为capability和contract scope后，Rust权威、route闭包和external contract行为保持一致
  - 不证明1443个deferred identity已经进入外部合同，也不替代其后续capability决策
  - 远程CI、PR/merge、发布和docs.rs渲染由独立交付证据负责
evidence_type: behavior_equivalence
before_revision: b21aba01b34e74c93d783a89db895282ba831c3c
after_revision: source:33686a6a0273c6f8ea85608bff92fed9774f00bfefe1bbd34aa1a2316b236cbc
scenario_results:
  rust-runtime-authority:
    status: matched
    source_refs: [docs/adr/0028-source-first-multilanguage-sdk-runtime.md, docs/adr/0032-sdk-contract-scope-classification.md, echo-sdk-host/src/core_profile/handler.rs, echo-sdk-host/src/core_profile/handles.rs]
  source-operation-route-closure:
    status: matched
    source_refs: [echo-sdk-host/src/core_profile/facade/source_operations.rs, echo-sdk-host/tests/core_profile_e2e.rs, echo-sdk-protocol/tests/facade_inventory.rs]
  sdk-contract-scope:
    status: matched
    source_refs: [echo-sdk-protocol/src/inventory.rs, contracts/sdk/parity-manifest.json, contracts/sdk/parity-manifest.schema.json, docs/adr/0032-sdk-contract-scope-classification.md]
  language-contract-gates:
    status: matched
    source_refs: [scripts/check-language-sdks.sh, sdks/typescript/test/catalog.test.js, sdks/python/tests/test_catalog.py, sdks/java/src/test/java/com/echoagent/sdk/FacadeParityTest.java]
command_results:
  - command: ./scripts/verify.sh
    exit_code: 0
  - command: ./scripts/check-sdk-contracts.sh
    exit_code: 0
  - command: ./scripts/check-language-sdks.sh
    exit_code: 0
coverage:
  - Rust runtime authority and Host adapter delegation
  - Canonical source-operation route closure
  - Identity-level SDK contract scope classification
  - TypeScript, Python, and Java external-contract gates
---

# SDK 治理范围行为等价证据

## 支持的结论

从`b21aba01`到当前候选结果，SDK治理单位由逐identity完成度改为capability、Behavior、Rule、Finding和identity级contract scope，但Rust仍是唯一运行语义权威，canonical source operation仍到达真实Host adapter，`external_contract`仍要求TypeScript、Python和Java证据全部完成。

四个变化对象的正文、引用和职责描述发生了调整；对应运行入口、错误边界、路由闭包和三语言external contract gate没有被第二套实现替代。ADR 0041将它们统一收敛到`map.sdk-facade-parity#scenario:sdk-contract-scope`，同时保留`source-operation-closure`场景用于Host路由证明。

## 来源与范围

来源是Rust inventory/Host adapter、parity manifest及schema、Rust和三语言合同测试，以及本轮从头执行且exit 0的完整Rust门禁、SDK contract和language SDK脚本。`before_revision`绑定原义务所在的远程main，`after_revision`绑定排除`.echo-semantic`后的候选源码摘要。

## 已知缺口

本Evidence只覆盖列出的四个SDK治理场景，不声称所有Rust public identity都属于三语言合同。Deferred capability、远程CI和发布状态继续保持显式未完成。
