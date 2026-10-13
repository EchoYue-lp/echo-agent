---
schema_version: 1
id: finding.rag-preview-unicode-count
kind: finding
type: implementation_bug
status: resolved
severity: medium
primary_focus: contract_evidence
focus:
- trigger_input
- result_side_effect
- state_authority
boundary_ref: boundary.tool-permission-sandbox
behavior_refs:
- behavior.native-tool-value-fidelity
rule_refs:
- rule.native-tool-value-authority
evidence_refs:
- evidence.rag-preview-unicode-repair
- evidence.rag-preview-unicode-verification
- evidence.native-tool-value-full-gates
audit_refs:
- audit.native-tool-value-fidelity-rereview
- audit.native-tool-value-final-rereview
decision_refs: []
repair_evidence_refs:
- evidence.rag-preview-unicode-repair
verification_evidence_refs:
- evidence.rag-preview-unicode-verification
- evidence.native-tool-value-full-gates
rereview_audit_refs:
- audit.native-tool-value-fidelity-rereview
- audit.native-tool-value-final-rereview
discovered_at: 77de6ca90ad842eb4b7b2921a4baa954f41cccfe
---

# RAG 预览字符统计与截断标记不一致

## 问题

char_count/truncated 使用 UTF-8 bytes，而 chunk_size/overlap/preview 使用 Unicode scalars；中文与emoji造成错误值。

## 唯一权威与修复

保留原框架 Unicode scalar chunk_text/preview，没有第二套SDK实现、公共协议、状态权威或新backend。

## 触发条件与影响

现有公开Tool结果值错误；具体触发与旧红/新绿范围见下列证据。

## 证据

精确真实旧红、新绿及focused fmt/lint由引用的 repair/verification Evidence 保存；独立 sdk_capability_acceptance_audit 只读复审源码和20原始归档，Critical/Important/Minor均0。复审原文由协调者转发，作为 conversation transcript 分类保存，不冒充脚本stdout。

## 处理记录

局部具体故障已具修复、验证和独立复审，故为resolved。此状态不声明外部Issue关闭、所有框架风险视角通过、SDK三语言修后消费、完整workspace新门禁、公开pin或精确main交付。


## 同源码最终本地验收

当前source1d52可恢复Git848fa的23完整门禁和独立framework_final_review已PASS/action0，实际3504/0/3externaloptin、86targets/59examples。Clinical第三文件另有独立源码Audit，不借原DB/RAG历史审阅；本具体故障local三链齐，resolved不表示remoteCI/main/SDK/官网已交付。
