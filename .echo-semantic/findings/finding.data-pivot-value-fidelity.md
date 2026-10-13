---
schema_version: 1
id: finding.data-pivot-value-fidelity
kind: finding
type: implementation_bug
status: resolved
severity: high
primary_focus: contract_evidence
focus:
- trigger_input
- result_side_effect
- failure_concurrency
boundary_ref: boundary.tool-permission-sandbox
behavior_refs:
- behavior.effect-permission-execution
rule_refs:
- rule.permission-effect-order
evidence_refs:
- evidence.data-pivot-value-fidelity-repair
- evidence.data-pivot-value-fidelity-verification
- evidence.framework-guard-pivot-channel-focused-archive
- evidence.framework-final-34d5-full-verification
- evidence.framework-final-34d5-gate-archive
audit_refs:
- audit.data-pivot-value-fidelity-rereview
- audit.framework-final-34d5-gate-rereview
decision_refs: []
repair_evidence_refs:
- evidence.data-pivot-value-fidelity-repair
verification_evidence_refs:
- evidence.data-pivot-value-fidelity-verification
- evidence.framework-final-34d5-full-verification
rereview_audit_refs:
- audit.data-pivot-value-fidelity-rereview
- audit.framework-final-34d5-gate-rereview
discovered_at: 95ec66c6083a8feb17a1c28de0d01e1a24ca8a28
---



# Data Pivot 原生值与null组合及错误语义失真

## 问题

旧AnyValue Display字符串被当filter literal：字符串带引号、数值与string比较失败、null bucket丢失。独立复审又发现null index join丢失、null/string label冲突和未知aggregation静默sum。原3及追加3实际输出/错误断言均失败。

## 触发条件与影响

native string/numeric/null分组及值，null index不丢失；含糊header失败关闭；不声明的aggregation不得用成功sum掩盖无效输入。

## 唯一权威与边界

保留既有Polars grouping/aggregation/join权威；LiteralValue::from(AnyValue)保留原生值、null使用is_null、join使用nulls_equal。header与匹配值分离，冲突在join前返回InvalidParameter。aggregation只有缺省sum，声明外string及null/number/bool类型拒绝。未新增pivot引擎或平行validator。

## 证据

pivot-red.log和pivot-adjacent-review-red.log分别保留真实3+3失败；pivot-invalid-runtime-red.log只是Tokio current-thread配置错误，明确不是行为反例。data-adjacent-review-green.log完整data模块14 passed/0 failed/0 ignored；tools-adjacent-review-clippy.log通过production约束，前序tools-focused-clippy保留。UTF-8输出、数值/null bucket与absent cell、null index、碰撞、aggregation类型/缺省均有actual-tool测试。 独立局部复审pass/action0，原始字节归档可核验。

## 处理记录

本地此故障已具修复、真实focused验证及独立复审，故resolved；fresh完整23、正式发布pin、SDK/CLI/官网与精确main交付仍分别待验。此状态不声明外部Issue关闭，也不替代其它Finding。


## 当前完整工程验收

同一source34d5的新版23完整门禁已真实通过（evidence.framework-final-34d5-full-verification）：3471/0/3外部optin ignored、86targets、59examples，全部warnings0/source/index不变。上文fresh23待验是此前focused阶段记录；完整工程缺口由此补齐，公开pin、SDK/CLI/官网及exact main仍独立待验。原独立局部Audit范围不扩大。


## 当前候选最终复审

audit.framework-final-34d5-gate-rereview 将本地此故障的精确源码/既有红绿证据与同source34d5新完整23门禁绑定；独立implementation review PASS/action0。当前可以进入授权framework里程碑交付，SDK/remoteCI/公开pin及精确main仍另验，其它Finding与全risk lenses不随此关闭。
