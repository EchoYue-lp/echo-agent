---
schema_version: 1
id: audit.git-branch-switch-rereview
kind: audit
boundary_ref: boundary.tool-permission-sandbox
lens: result_side_effect
freshness: examined
revision: 046674d4775d31ddc2daad7107a8425edb648c8f
finding_refs:
- finding.git-branch-switch-dirty-path
challenges:
  branch-vs-path-and-dirty-state:
    revision: 046674d4775d31ddc2daad7107a8425edb648c8f
    source_refs:
    - echo-tools/src/git.rs#GitBranchTool
    - echo-tools/src/git.rs#git_branch_switch_existing_branch_without_same_named_file
    - echo-tools/src/git.rs#git_branch_switch_preserves_dirty_same_named_tracked_file
    - echo-tools/src/git.rs#git_branch_switch_conflicting_dirty_changes_fail_without_mutation
    - echo-tools/src/git.rs#git_branch_switch_invalid_option_and_missing_targets_preserve_repo
    - echo-tools/README.md
    - docs/evidence/git-branch-switch/original-cli-counterexamples.json
    - docs/evidence/git-branch-switch/before-actual-run.json
    - docs/evidence/git-branch-switch/before-actual.log
    - docs/evidence/git-branch-switch/final-protection-receipt.json
    - docs/evidence/git-branch-switch/final-focused-run.json
    - docs/evidence/git-branch-switch/final-focused.log
    - docs/evidence/git-branch-switch/independent-review.json
    - docs/evidence/git-branch-switch/manifest.json
    - docs/evidence/git-branch-switch/before/echo-tools/src/git.rs
    - docs/evidence/git-branch-switch/reviewed-before-protection-accuracy.rs
    - docs/evidence/git-branch-switch/source/echo-tools/src/git.rs
    - docs/evidence/git-branch-switch/before-production-fix.rs
    - docs/evidence/git-branch-switch/final-protection-source.rs
    - docs/evidence/git-branch-switch/full-gates/receipt.json
    - docs/evidence/git-branch-switch/full-gates/README.md
    - docs/evidence/git-branch-switch/full-gates/inputs-before.json
    - docs/evidence/git-branch-switch/full-gates/inputs-after.json
    - docs/evidence/git-branch-switch/full-gates/05-workspace-tests.log
    evidence_refs:
    - evidence.git-branch-switch-fidelity
---

# GitBranchTool switch 原缺陷独立复审

## 审查范围

framework_final_review实际源码与最后ReadDir错误传播保护增量审，仅GitBranchTool switch和原四真实Tool断言；不是模型作者新的业务审或全部Git复审。

## 已检查故障假设

checkout --将分支名视作路径而覆盖dirty bytes；假success却HEAD不变；invalid/option导致force；冲突失败后refs/index或文件变动；测试保护iterator错误被filter吞；旧源码或旧门禁冒当前source。

## 实际实现路径与证据

现execute_with_context保原bounded process、Tool context cwd和Git错误传播，原Git worktree switch模式复用。原CLI2及Tool3fail/1pass对同四测试after4pass，最后iterator真实Result收集后同4再次pass。independent-review.json明确最终cfd/之前15cb及完整source scope。

## 问题记录

原checkout-path解释具体缺陷根已修，有修复/验证/独审三链，Finding仅本地resolved。SDK旧a634消费者和remote/main不由这里关闭。

## 残余风险

全部Git动作/平台、worktree资源清理与远端SDKpin未在本审；完整六门禁属于自己的a193收据，不借原focused测试数。

## 未检查项

任何新公共API、第二Git state authority、force checkout、任意全部Git故障组合均未新增。