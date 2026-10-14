---
schema_version: 1
id: finding.git-branch-switch-dirty-path
kind: finding
type: implementation_bug
status: resolved
severity: high
primary_focus: result_side_effect
focus:
- trigger_input
- state_authority
- data_durability
- contract_evidence
boundary_ref: boundary.tool-permission-sandbox
behavior_refs:
- behavior.git-branch-switch-fidelity
rule_refs:
- rule.git-branch-switch-authority
evidence_refs:
- evidence.git-branch-switch-fidelity
audit_refs:
- audit.git-branch-switch-rereview
decision_refs: []
repair_evidence_refs:
- evidence.git-branch-switch-fidelity
verification_evidence_refs:
- evidence.git-branch-switch-fidelity
rereview_audit_refs:
- audit.git-branch-switch-rereview
discovered_at: a6346643bda47679a96760f56f45e41fd73b61b0
---

# Git分支切换覆盖同名dirty路径

## 问题

已发布a634原checkout -- target实际解释为路径恢复，覆盖同名tracked dirty内容且文案假报切换成功。

## 触发条件与影响

同名分支/文件导致真实CLI2数据丢失、同原Tool四fixture旧3fail/1pass，违反分支切换与未提交数据保全。

## 证据

综合Evidence包括原根源码、原真实失败和修后同四green、cfd最后保护oracle及独立review。此框架Finding不是SDK跨仓Finding代替；根code在框架现GitTool。

## 处理记录

原分支专用switch/no-force/严格目标输入根修已实际完成，同源focused与完整六门禁通过，源码独审PASS0，本地具体resolved。SDK仍旧pin时其Finding可以保持open；remoteCI/main/其它Git或worktree不借这个状态关闭。