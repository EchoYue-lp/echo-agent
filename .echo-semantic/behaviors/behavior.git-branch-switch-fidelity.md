---
schema_version: 1
id: behavior.git-branch-switch-fidelity
kind: behavior
status: verified
expectation: human_confirmed
risk: high
primary_focus: result_side_effect
focus:
- trigger_input
- state_authority
- data_durability
- contract_evidence
observed_at: 046674d4775d31ddc2daad7107a8425edb648c8f
code_refs:
- echo-tools/src/git.rs#GitBranchTool
- echo-tools/src/git.rs#git_branch_switch_existing_branch_without_same_named_file
- echo-tools/src/git.rs#git_branch_switch_preserves_dirty_same_named_tracked_file
- echo-tools/src/git.rs#git_branch_switch_conflicting_dirty_changes_fail_without_mutation
- echo-tools/src/git.rs#git_branch_switch_invalid_option_and_missing_targets_preserve_repo
- echo-tools/README.md
evidence_refs:
- evidence.git-branch-switch-fidelity
finding_refs:
- finding.git-branch-switch-dirty-path
boundary: boundary.tool-permission-sandbox
rule_refs:
- rule.git-branch-switch-authority
- rule.permission-effect-order
---

# 原生Git分支切换与未提交数据保全

## 重要承诺

现有GitBranchTool switch必须真正切换branch，不能以路径恢复覆盖用户未提交内容并返回假success；已接受旧形状不变。

## 当前行为

使用同repo上下文的git switch -- target，无force，原Git单权威。空白/option目标明确拒，兼容dirty保留，冲突遵循Git失败。

## 期望行为

成功HEAD指目标branch与完整返回一致；失败保HEAD/refs/index/status/files，默认不丢未提交内容。

## 触发、结果与副作用

同名branch/dirtytracked file、正常现存branch、冲突dirty、missing/option输入四组具名真实Tool场景。

## 失败、重试与恢复

原Git命令错误由原ToolResult失败可见；不自动force、stash或新回滚状态机。原repo仍可由调用者处理后重试。

## 证据

原CLI2/Tool3真fail→same4green、最终cfd保护oracle4green、实际独审PASS0见唯一综合Evidence。该verified只具体四组承诺，不全部Git族。

## 裁决记录

普通已接受行为实现缺陷，授权直接修复；不是新增permission/Web安全闸或新API选择。SDK、main与平台证明独立。