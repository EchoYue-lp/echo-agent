---
schema_version: 1
id: finding.clinical-trials-total-fidelity
kind: finding
type: implementation_bug
status: resolved
severity: medium
primary_focus: contract_evidence
focus:
- trigger_input
- result_side_effect
- failure_concurrency
boundary_ref: boundary.tool-permission-sandbox
behavior_refs:
- behavior.native-tool-value-fidelity
rule_refs:
- rule.native-tool-value-authority
evidence_refs:
- evidence.clinical-trials-total-repair
- evidence.clinical-trials-total-verification
- evidence.native-tool-value-full-gates
audit_refs:
- audit.native-tool-value-final-rereview
- audit.clinical-trials-total-rereview
decision_refs: []
repair_evidence_refs:
- evidence.clinical-trials-total-repair
verification_evidence_refs:
- evidence.clinical-trials-total-verification
- evidence.native-tool-value-full-gates
rereview_audit_refs:
- audit.native-tool-value-final-rereview
- audit.clinical-trials-total-rereview
discovered_at: 554f2f50d3705d219519f306055a3d26d6a5211c
---

# ClinicalTrials 缺失总量被成功填为零

## 问题

ClinicalTrials.gov API v2默认不保证totalCount；旧请求未countTotal=true，解析缺失/错误type默0，造成存在研究记录但宣称total_results0。

## 触发条件与影响

HTTP200 studies非空但totalCount缺失，以及null/string/negative/fraction/bool/array/object总量；用户获得错误的总量语义，错误签名被成功掩盖。

## 证据

同官方GET请求只添加countTotal=true，原响应study1无totalCount，后响应study1+totalCount386；原4offline3fail1pass→新4pass，完整before/after和fixture分类可复核。

## 处理记录

复用原Tool URLbuilder/parser/HTTPclient，唯一total authority是原provider已存在的totalCount，不新增API/state/权限。Focused修复已绿，但第三源码独立复审与整候选门禁/公开main未齐，故open，不借DB/RAG Audit关闭。


## 同源码最终本地验收

当前source1d52可恢复Git848fa的23完整门禁和独立framework_final_review已PASS/action0，实际3504/0/3externaloptin、86targets/59examples。Clinical第三文件另有独立源码Audit，不借原DB/RAG历史审阅；本具体故障local三链齐，resolved不表示remoteCI/main/SDK/官网已交付。
