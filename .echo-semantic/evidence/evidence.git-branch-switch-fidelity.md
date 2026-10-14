---
schema_version: 1
id: evidence.git-branch-switch-fidelity
kind: evidence
observed_at: 046674d4775d31ddc2daad7107a8425edb648c8f
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
supports:
- behavior.git-branch-switch-fidelity
limitations:
- Specific branch switching only; not every GitTool/worktree/Sandbox behavior.
- Original CLI2 and real old Tool3 failures retained; no claim current SDK pin already
  fixed.
- Full six gates are separately recorded on a193; remote dependency CI/main and later
  SDK consumers remain independent.
---

# GitBranchTool 分支切换保真证据

## 来源与范围

根实现是现有通用框架 GitBranchTool，不新增公共API、状态机或SDK绕修。五个归档.rs副本只作为before/after取证输入，与manifest/sourceSHA/同断言闭包对应；不执行、不注册、不产生第二runtime权威。真实046674 Git保存a193修复源、原focused包与六门禁包；git.rs最终SHA cfd9565be083ea846651c476bc37e37788fd94a5589939b7b960b8203c7e00b5。原源码82cf保在before目录，CLI两个真实data-loss事实及同一四个Tool测试的旧3失败/1通过、新4通过保原字节。测试最后ReadDir收集改为传播io::Result错误，不静默丢观察，生产prefix与原15cb已审修复相同，最终4个相同具名断言再次通过。

## 支持的结论

switch使用原已有分支专用 git switch -- target，不再将同名目标解释成路径checkout；保持bounded_process原repo cwd、原ToolResult和原Git唯一HEAD/refs/index/files。空白/option形状失败在effect前明确invalid，正常target真实切HEAD；兼容dirty同名文件完整保留，冲突dirty/不存在目标/option错误前后HEAD、fixture全部heads refs、逻辑index、status、文件bytes不变，没有force、path restore或平行Git检查器。

framework_final_review实际读源与保护oracle增强，独立PASS C0/I0/M0。后继a193完整六命令收据真实全exit0，该命令当期570输入before/after一致且与当期a193代码匹配，后来dependency lock更新不冒这次current全同；workspace3532主tests零fail、3既有opt-in ignored，父test另2子进程摘要不累加成主计数。六门禁包已冻结在046674可恢复树，source refs直接指正式收据与输入清单；它不覆盖后继lock更新、远端CI失败或SDK仍a634的消费者修复。

## 已知缺口

范围只本branch switch缺陷；其它Git工具/所有平台、worktree业务、SDK新pin消费者和main交付单独验收。原Tools227/2opt-in ignored属于保护oracle增强前的focused阶段，不冒最终整个workspace。条件feature矩阵不触发：没有Cargo/feature/依赖/生产cfg/跨crateAPI变更。双语API形状未变；echo-tools README同步，checkout无examples目录，不新示例/网站业务合同。