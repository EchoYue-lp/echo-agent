---
schema_version: 1
id: audit.sdk-facade-scope-contract
kind: audit
boundary_ref: boundary.sdk-facade-parity
lens: contract_evidence
freshness: examined
revision: source:5a12b544f08f549cccd424c6c0a22acf3f1cba0e15bcacc1febc283b76536f6b
finding_refs: []
challenges:
  identity-scope-versus-route:
    revision: source:5a12b544f08f549cccd424c6c0a22acf3f1cba0e15bcacc1febc283b76536f6b
    source_refs: [echo-sdk-protocol/src/inventory.rs, contracts/sdk/parity-manifest.schema.json, contracts/sdk/parity-manifest.json, docs/adr/0032-sdk-contract-scope-classification.md]
    evidence_refs: [evidence.sdk-contracts]
  cross-language-scope-gates:
    revision: source:5a12b544f08f549cccd424c6c0a22acf3f1cba0e15bcacc1febc283b76536f6b
    source_refs: [echo-sdk-protocol/tests/facade_inventory.rs, scripts/check-language-sdks.sh, sdks/typescript/test/catalog.test.js, sdks/python/tests/test_catalog.py, sdks/java/src/test/java/com/echoagent/sdk/FacadeParityTest.java]
    evidence_refs: [evidence.sdk-contracts]
  operation-catalog-stability:
    revision: source:5a12b544f08f549cccd424c6c0a22acf3f1cba0e15bcacc1febc283b76536f6b
    source_refs: [contracts/sdk/facade-operation-catalog.json, contracts/sdk/source-contract.json, sdks/shared/contract-digests.json]
    evidence_refs: [evidence.sdk-contracts]
---

# SDK Contract Scope 分类审计

## 审查范围

审查ManifestEntry.sdk_scope、确定性规则、alias继承、三语言consumer gate、生成artifact与operation catalog稳定性。

## 已检查故障假设

验证intrinsic route是否被错误等同非外部合同、scope是否以手工identity清单维护、external是否缺语言证据、schema变更是否误改wire/operation catalog，以及deferred是否重新成为逐identity门禁。

## 实际实现路径与证据

SdkScope是ManifestEntry属性并与status分离；非intrinsic route和具名intrinsic capability group独立决定external acceptance，其余按testing source与既有intrinsic reason分类。Language status随后验证external完整性，不能反向降级scope。Alias显式复制canonical scope。当前canonical计数为5606/1765/780/90/1441，551个intrinsic external保持不变；manifest schema升2，extension protocol保持1，operation catalog哈希保持不变。

## 问题记录

未关闭任何runtime/protocol Finding；`finding.sdk-gap-generation-validation-parity`仍open且不受scope分类影响。

## 残余风险

External scope以当前已交付证据定义，deferred仍需capability级产品判断；不能用未实现状态永久排除有用户价值的能力。

## 未检查项

未执行registry/binary publication；本项目source-first设计明确不把发布状态纳入本次合同。
