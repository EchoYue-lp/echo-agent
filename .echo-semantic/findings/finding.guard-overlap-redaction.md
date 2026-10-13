---
schema_version: 1
id: finding.guard-overlap-redaction
kind: finding
type: implementation_bug
status: resolved
severity: high
primary_focus: result_side_effect
focus:
- trigger_input
- failure_concurrency
- contract_evidence
boundary_ref: boundary.tool-permission-sandbox
behavior_refs:
- behavior.effect-permission-execution
rule_refs:
- rule.permission-effect-order
evidence_refs:
- evidence.guard-overlap-redaction-repair
- evidence.guard-overlap-redaction-verification
- evidence.framework-guard-pivot-channel-focused-archive
- evidence.framework-final-34d5-full-verification
- evidence.framework-final-34d5-gate-archive
audit_refs:
- audit.guard-overlap-redaction-rereview
- audit.framework-final-34d5-gate-rereview
decision_refs: []
repair_evidence_refs:
- evidence.guard-overlap-redaction-repair
verification_evidence_refs:
- evidence.guard-overlap-redaction-verification
- evidence.framework-final-34d5-full-verification
rereview_audit_refs:
- audit.guard-overlap-redaction-rereview
- audit.framework-final-34d5-gate-rereview
discovered_at: 95ec66c6083a8feb17a1c28de0d01e1a24ca8a28
---



# ContentGuard 重叠敏感区间重复替换损坏输出

## 问题

真实SDK Host曾把4513812345678111输出为 [REDACTED:CARD]E]111。旧实现按反序替换原偏移，重叠区间在已改变文本上失效；三项实际incorrect-output反例失败。日志未观察到UTF-8 panic，不能将潜在panic写为实际现象。

## 触发条件与影响

原始偏移重叠、跨越、同起点不同长度及中文emoji包围；区间选择与最终文本无重复替换损伤。

## 唯一权威与边界

ContentGuard 在原输入上合并重叠覆盖区间，选择最宽成员标记（同宽按既有detect顺序）；char_indices 单次输出，邻接不重叠区间分别保留。detect的原始byte offsets合同不变，未新增策略/API/SDK parser。

## 证据

guard-red.log保留3项实际输出失败；guard-green.log覆盖完整content模块9 passed/0 failed/0 ignored。core-focused-clippy.log为production lib、guard feature、locked、warnings及unwrap/expect/panic/unreachable拒绝检查通过。owned源码格式与diff检查通过。 独立局部复审pass/action0，原始字节归档可核验。

## 处理记录

本地此故障已具修复、真实focused验证及独立复审，故resolved；fresh完整23、正式发布pin、SDK/CLI/官网与精确main交付仍分别待验。此状态不声明外部Issue关闭，也不替代其它Finding。


## 当前完整工程验收

同一source34d5的新版23完整门禁已真实通过（evidence.framework-final-34d5-full-verification）：3471/0/3外部optin ignored、86targets、59examples，全部warnings0/source/index不变。上文fresh23待验是此前focused阶段记录；完整工程缺口由此补齐，公开pin、SDK/CLI/官网及exact main仍独立待验。原独立局部Audit范围不扩大。


## 当前候选最终复审

audit.framework-final-34d5-gate-rereview 将本地此故障的精确源码/既有红绿证据与同source34d5新完整23门禁绑定；独立implementation review PASS/action0。当前可以进入授权framework里程碑交付，SDK/remoteCI/公开pin及精确main仍另验，其它Finding与全risk lenses不随此关闭。
