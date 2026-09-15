---
schema_version: 1
id: evidence.checkpoint-journal-sdk-inventory
kind: evidence
observed_at: 29cea08cd3b487d59fef1a769e5d6fe9dbf3e36e
source_refs:
  - echo-state/src/journal/mod.rs
  - echo-sdk-protocol/src/facade.rs
  - echo-sdk-protocol/tests/facade_inventory.rs
  - contracts/sdk/public-api.txt
  - contracts/sdk/parity-manifest.json
  - contracts/sdk/facade-operation-catalog.json
  - contracts/sdk/source-contract.json
  - sdks/shared/facade-operation-catalog.json
  - sdks/shared/contract-digests.json
  - scripts/check-language-sdks.sh
  - sdks/typescript/test/catalog.test.js
  - sdks/python/tests/test_catalog.py
  - sdks/java/src/test/java/com/echoagent/sdk/FacadeParityTest.java
supports: [behavior.observation-persistence, behavior.sdk-facade-routing, rule.fact-projection-separation, rule.sdk-rust-authority]
limitations:
  - 本Evidence只证明Journal identity公共面已进入现有inventory分类和生成链，不证明其它deferred SDK能力已经完成
  - 本切片没有增加TypeScript、Python或Java源码，也没有改变extension wire schema或fixture
  - echo-sdk-host与echo-sdk-protocol独立仓库迁移继续由Issue 122跟踪，远端平台CI等待PR执行
---

# Checkpoint 与 Journal identity SDK inventory 证据

## 支持的结论

`JournalIdentity`、checkpoint字段、Journal trait方法和receipt访问器已经进入锁定Rustdoc清单。
生成器把可序列化值归入external contract，把process-local generic Journal trait归入Host/Rust-only，
把Rust trait impl归入language intrinsic，并把`JournalIdentity`构造、解析和访问器保留为本地value
方法；没有为这些语言本地方法新增Host adapter或逐identity语言包装。

canonical inventory由9713项变为9724项：external contract 5622、Host/Rust-only 1774、language
intrinsic 790、internal helper 90、deferred 1448。extension schema和全部fixture内容未变化，刷新仅
涉及Rust public API、parity manifest、operation catalog、source contract及语言SDK共享catalog/digest。

## 来源与范围

该变化由Issue #43的Journal generation身份合同触发，沿用ADR 0031/0032的scope分类和现有生成器；
不建立第二个SDK contract authority，也不与Issue #122的仓库拆分并行修改Host/Protocol实现。

## 已知缺口

本地完整SDK合同、三语言合同和workspace门禁已通过；远端平台CI等待PR执行。
