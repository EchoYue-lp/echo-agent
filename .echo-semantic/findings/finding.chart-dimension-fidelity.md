---
schema_version: 1
id: finding.chart-dimension-fidelity
kind: finding
type: implementation_bug
status: resolved
severity: high
primary_focus: contract_evidence
focus:
- trigger_input
- state_authority
- result_side_effect
boundary_ref: boundary.tool-permission-sandbox
behavior_refs:
- behavior.native-tool-value-fidelity
rule_refs:
- rule.native-tool-value-authority
evidence_refs:
- evidence.chart-dimension-fidelity
audit_refs:
- audit.chart-dimension-fidelity-rereview
decision_refs:
- docs/adr/0083-shared-tool-input-validation.md
repair_evidence_refs:
- evidence.chart-dimension-fidelity
verification_evidence_refs:
- evidence.chart-dimension-fidelity
rereview_audit_refs:
- audit.chart-dimension-fidelity-rereview
discovered_at: b9099558620f5f9693c736c85c214db14f62a8cb
---

# Chart dimension narrows or defaults supplied invalid integers

## 问题

Width/height asu32 wraps4294967296 to0; negative/float/string/bool/null suppliedvalues become
600/400 defaults. Canonical schema has no min/max, admitting integers outside originalu32 range.

## 触发条件与影响

Direct Tool.execute and ToolManager/validate_tool_input both reach original GenerateChartTool.
Invalidwidth emits plausible wrong chartdimensions instead of expliciterr; thinSDK conversion
cannot compensate by clamping or hidden defaults.

## 证据

Originalactual direct2red and officialmanager1red, sixfocusedgreen and same strictClippygreen.
Defaults/zero/one/u32MAX unchanged; exact field InvalidParameter and schema range constraints.
Rawreceipts/source/private replayable snapshots remain separated from actualruntime proof.

## 处理记录

Local repairedcandidate and direct+manager6green present, independentreview/fullgates pending.
Keep open until repair/verification/rereview three chains match exactfinal source. No newresource
limit or publicoperation/SDKcapability accepted by this fix. Existingtool/schema owns authority.

## 当前有限处理结论

局部实现闭合：originaldirect/manager真实red→same6green；原chart-onlyClippylint及测试
warning对应samecommand修复0warning；freshfull23/test3508与依赖audit/deny0；源独review
PASS0并root确认唯一非语义文稿Minor清除。repair/verification/rereview三链精确源码。
resolved只表示该nativeTool转换缺陷，不表示remoteCI/exactmain/SDK全goal交付。
