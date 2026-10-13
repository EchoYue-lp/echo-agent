---
schema_version: 1
id: finding.framework-dependency-policy-ci
kind: finding
type: implementation_bug
status: resolved
severity: high
primary_focus: contract_evidence
focus:
- trigger_input
- failure_concurrency
- permission_external
boundary_ref: boundary.workspace-architecture
behavior_refs:
- behavior.workspace-composition
rule_refs:
- rule.framework-layer-ownership
evidence_refs:
- evidence.framework-dependency-policy-repair
- evidence.framework-dependency-policy-focused-verification
- evidence.framework-dependency-final-full-verification
- evidence.framework-dependency-final-gate-archive
audit_refs:
- audit.framework-dependency-policy-rereview
decision_refs: []
repair_evidence_refs:
- evidence.framework-dependency-policy-repair
verification_evidence_refs:
- evidence.framework-dependency-policy-focused-verification
- evidence.framework-dependency-final-full-verification
rereview_audit_refs:
- audit.framework-dependency-policy-rereview
discovered_at: 4eb005c15b9c0036b73e409026c668c02f0a9575
---


# PR181依赖策略CI拒绝已锁脆弱/停止维护包

## 问题

真实候选4eb的CI dependency-policy失败：quinn-proto0.11.16命中5项RustSec，serde_yaml_ng0.10.0引unsafe-libyaml0.2.11停止维护warning被deny。源码本地34d5完整绿不代表此远端策略通过。

## 触发条件与影响

CI按完整locked graph审查，即使当前macOS feature graph不选择quinn-proto消费者也不能豁免已锁包；策略失败阻止PR/main验收。

## 证据

evidence.framework-dependency-policy-repair 与 evidence.framework-dependency-policy-focused-verification：官方来源选择、六Cargo最小差异、真实原锁失败/新锁成功、459focused、old/new YAML字段级结果与错误匹配，当前hash及全部证据归档校验。

## 处理记录

仅本地candidate修复；新完整23、独立复审及远端策略CI尚未齐，状态open。不添加忽略、阈值、许可例外或状态权威。SDK/公开pin/main独立待验。


## 当前本地处理结论

新23完整工程最终全绿与独立源码review PASS/action0已具，旧候选phase open描述由此局部resolved结论取代。一次真实test失败原因及修复后attempt2完整通过原始保留。finalmetadata/baseline3focused/readerreceipt、remote dependency-policy/CI/main与SDK仍各自验收，绝不由此宣布交付完成。
