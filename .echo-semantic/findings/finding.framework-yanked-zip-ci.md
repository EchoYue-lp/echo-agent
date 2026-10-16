---
schema_version: 1
id: finding.framework-yanked-zip-ci
kind: finding
type: implementation_bug
status: open
severity: high
primary_focus: contract_evidence
focus:
- trigger_input
- permission_external
boundary_ref: boundary.workspace-architecture
behavior_refs:
- behavior.workspace-composition
rule_refs:
- rule.framework-layer-ownership
evidence_refs:
- evidence.framework-yanked-zip-repair
audit_refs:
- audit.framework-yanked-zip-source-review
decision_refs: []
repair_evidence_refs:
- evidence.framework-yanked-zip-repair
verification_evidence_refs: []
rereview_audit_refs:
- audit.framework-yanked-zip-source-review
discovered_at: 875cc22f2e99c7ccd83c07e02c03fe221c5e3f6c
---

# 875主线的ZIP撤回版本阻断依赖CI

## 问题

PR187合入875后，CI38849170013的dependency-policy执行cargo audit0.22.2 --deny warnings，因锁内zip2.4.2和8.6.0已yanked失败。其余六job成功不抵消此失败；这与旧PR181 quinn/YAML故障不同。

## 触发条件与影响

现有媒体三父包的版本范围不能选择唯一未撤回zip9.0.3，锁更新本身无解。框架main不得按先前源码门禁绿宣称交付成功；SDK/CLI新pin需等修复精mainCI。

## 证据

原main依赖job完整28099B和本地audit-before1实际387B/exit1均保在ignored任务目录。独立候选审C0/I0/M0核对官方index/三archive checksum/许可证、最小源码差异、独立consumer解析及真实两媒体往返；见 evidence.framework-yanked-zip-repair。

## 处理记录

Root批准最小原版本源码适配：449必要vendor文件、原许可证、私有ZIP9适配和六处显式生命周期，以及继承relative path和真实zip9锁；不新加audit/deny豁免，不改公共工具API/发布范围。当前正式落盘/验证中，保持open；新全6+17、最终复审及精确mainCI未完成。原始失败与首次API不兼容编译失败/警告迭代全部保留，不能冒行为反例或零警告。


The source-adoption review found official scanners skip path/Git packages. This side effect is repaired by one canonical framework registry-origin audit view, with strict owner/source/version/checksum guards and unchanged actual build lock. Real historical quinn advisories prove the scanner control; no media CVE is invented. Eight persistent guards and the official origin-view scans passed; final review, full gates and mainCI remain pending.
