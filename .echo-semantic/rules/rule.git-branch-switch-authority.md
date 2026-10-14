---
schema_version: 1
id: rule.git-branch-switch-authority
kind: rule
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
behavior_refs:
- behavior.git-branch-switch-fidelity
---

# Git分支切换的唯一权威

## 不变量或唯一权威

分支、HEAD、refs、index与working bytes均由同一nativeGitrepo事实决定；框架仅用既有Git命令/上下文，不能另维护状态或以消息推成功。

## 适用行为

仅现有GitBranchTool switch，路径恢复不能替代branch切换，未提交数据保全默认不force。

## 当前实现

原bounded process/context/error接口，git switch -- target，空/option先拒绝；无新registry、scheduler、validator或SDK绕修。

## 期望行为

原Git实际成功/失败与ToolResult及副作用匹配。冲突不得变更dirtybytes；兼容dirty完整保留。

## 证据

具名actualTool before/after及保护HEAD/全部heads refs（git show-ref --heads）/逻辑index/status/files断言、最后ReadDir错误传播与独审见Evidence。

## 裁决记录

复用既有工具/本地数据保全约束，已授权普通bug修复；verified仅此具体边界，原DB/RAG/nativevalue模型不扩。