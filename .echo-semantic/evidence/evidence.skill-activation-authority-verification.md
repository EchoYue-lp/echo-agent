---
schema_version: 1
id: evidence.skill-activation-authority-verification
kind: evidence
observed_at: source:8d625811f35783570937db90b78b56e1e5e3b9273bebcf5a3c5a3c10fb863965
source_refs:
  - echo-execution/src/skills/registry.rs
  - echo-execution/src/skills/external/resource_tool.rs
  - echo-execution/src/skills/external/run_script_tool.rs
  - src/agent/react/capabilities.rs
  - src/agent/react/mod.rs
  - src/agent/react/run/context.rs
  - src/agent/react/tests.rs
  - src/agent/snapshot.rs
  - docs/adr/0044-skill-activation-authority.md
  - docs/en/07-skills.md
  - docs/zh/07-skills.md
supports: [behavior.extension-publication, rule.extension-generation-authority]
limitations:
  - Independent rereview and the complete workspace merge gate remain for the integration branch
  - activation_view is a doc-hidden cross-crate Rust API and the final combined SDK inventory regeneration is deferred to the integration branch
---

# Skill activation 唯一权威验证证据

## 支持的结论

基准`0878a676c9128619fdfb2faa15868a1388689822`仅应用新增checkpoint roundtrip
测试后，`checkpoint_skill_activation_restores_resource_tool_authority`在resource
success断言处失败，证明restore只写主registry时progressive tool无法读取同一activation。

修复后的`cargo test -p echo_execution skill`通过188项，覆盖registry activation、
dependency、resource/script、loader、prompt与相关Hook合同；`cargo test -p echo_agent skill`
通过21项，覆盖tool/API重复激活、checkpoint恢复后resource读取、projection、compression、
conditional activation及snapshot allowlist。`cargo clippy -p echo_execution -p echo_agent
--all-targets --all-features --locked -- -D warnings`、`cargo fmt --all -- --check`和
`git diff --check`均exit 0。

严格semantic snapshot/change-evidence以基准`0878a676`执行并exit 0，确认changed path均在
preflight范围内，Finding、map、Behavior、Rule、repair/verification Evidence和源码摘要闭合。

## 来源与范围

新增测试分别覆盖shared-state repeat/reset/remove、从主registry激活后resource/script可见，
以及持久checkpoint恢复到resource tool的端到端roundtrip。英文、中文文档与ADR描述同一边界。

## 已知缺口

本分支按并行开发约定未执行完整workspace gate、全部feature矩阵、完整SDK contract导出、
远端CI或MR；Finding在独立复审前保持open。
