---
schema_version: 1
id: audit.sdk-gap-generation-validation-rereview
kind: audit
boundary_ref: boundary.protocol-surfaces
lens: contract_evidence
freshness: examined
revision: 01c203b85a3451cf0c3bcfc91e68ad25534f8041
finding_refs: [finding.sdk-gap-generation-validation-parity]
challenges:
  complete-handle-generation-fence:
    revision: 01c203b85a3451cf0c3bcfc91e68ad25534f8041
    source_refs: [sdks/python/src/echo_agent_sdk/client.py, sdks/java/src/main/java/com/echoagent/sdk/BoundedPublisher.java]
    evidence_refs: [evidence.sdk-gap-generation-validation-repair, evidence.sdk-gap-generation-validation-verification]
  cursor-and-ack-non-advancement:
    revision: 01c203b85a3451cf0c3bcfc91e68ad25534f8041
    source_refs: [sdks/python/tests/test_lifecycle.py, sdks/java/src/test/java/com/echoagent/sdk/LifecycleTest.java]
    evidence_refs: [evidence.sdk-gap-generation-validation-verification]
---

# SDK gap generation validation独立复审

## 审查范围

独立reviewer检查五个修改文件、Python/Java直接consumer、TypeScript对照、Rust WireHandle、
Event/Gap/ACK合同与Host delivery路径。

## 已检查故障假设

检查同ID错generation/kind是否进入当前feed、非法sequence/gap是否推进cursor或ACK、ACK
是否早于subscriber/notification成功、重复gap是否破坏幂等，以及预订阅错代缓存是否泄露。

## 实际实现路径与证据

最终diff hash `2a3f96602086936114018973f7237a4e87480a79703549ec450a94f5d29925d0`。
Python focused 36与全套175/1 skipped、Java Lifecycle/全套Maven、Ruff与diff check通过；
reviewer结论pass，Critical、Important、Minor均为0。

## 问题记录

审查过程中发现并修复生命周期类型回归、Java u64解析、错误码漂移、ACK过早、gap连续性和
预订阅缓存泄露。Host replay watermark问题不是SDK generation fence，已建立Issue #120。

## 残余风险

Host仍需独立证明gap ACK后的replay/live continuation不会重发snapshot已覆盖的事件。

## 未检查项

未在本切片运行完整Rust workspace门禁或远端CI；按用户要求留到汇总MR前执行。
