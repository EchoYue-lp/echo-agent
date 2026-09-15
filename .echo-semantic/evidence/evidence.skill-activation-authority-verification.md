---
schema_version: 1
id: evidence.skill-activation-authority-verification
kind: evidence
observed_at: 8b912e97840b000adf6d844c805508aa5f18a2c8
source_refs:
  - echo-execution/src/skills/registry.rs
  - echo-execution/src/skills/mod.rs
  - echo-execution/src/skills/external/activate_tool.rs
  - echo-execution/src/skills/external/resource_tool.rs
  - echo-execution/src/skills/external/run_script_tool.rs
  - src/skills/mod.rs
  - src/agent/react/capabilities.rs
  - src/agent/react/mod.rs
  - src/agent/react/run/context.rs
  - src/agent/react/tests.rs
  - src/agent/snapshot.rs
  - echo-sdk-host/src/core_profile/facade/source_operations.rs
  - echo-sdk-protocol/src/facade.rs
  - CHANGELOG.md
  - docs/adr/0044-skill-activation-authority.md
  - docs/en/07-skills.md
  - docs/zh/07-skills.md
supports: [behavior.extension-publication, rule.extension-generation-authority]
limitations:
  - The complete workspace merge gate remains for the integration branch
  - Final combined rustdoc inventory regeneration and artifact-level SDK tests are deferred to the integration branch so parallel public API branches update generated contracts once
  - The all-feature Host e2e build exhausted local disk before reaching its assertion; the sdk-facade-adapters Host library and routing code passed focused check and Clippy
---

# Skill activation 唯一权威验证证据

## 支持的结论

基准`0878a676c9128619fdfb2faa15868a1388689822`仅应用新增checkpoint roundtrip
测试后，`checkpoint_skill_activation_restores_resource_tool_authority`在resource
success断言处失败，证明restore只写主registry时progressive tool无法读取同一activation。

首轮共享set实现后，独立review以I1-I3/M1阻断：checkpoint仍写telemetry Vec、restore丢
policy、SDK mutation只改primary definition、activation无epoch/single-flight且public seam被
`doc(hidden)`排除inventory。当前delta逐项修复后，registry定向测试27/27通过，覆盖同key
API/tool concurrent single-flight、parameter key、different-key conflict、cancel poison、
reset/remove stale publication fence及shared reset/remove。

Agent定向测试证明checkpoint在run snapshot后仍读取live handle、restore从descriptor恢复
sandbox policy、register/remove同步progressive tools且stale ActivateSkillTool不能复活Skill；
deny replacement回归同时证明旧descriptor、activation、sandbox policy、catalog、progressive
definition与resource读取保持可用；parameterized projection/compression回归也通过。Protocol
classifier test固定五个authority identity为`process-local-skill-activation-authority`。

`cargo clippy -p echo_execution -p echo_agent --lib --locked -- -D warnings`和
`cargo clippy -p echo-sdk-host --no-default-features --features sdk-facade-adapters --lib --locked
-- -D warnings`均exit 0；同Host feature的library check、formatter与diff check通过。

同一独立reviewer在首轮I1-I3/M1与第二轮replacement rollback Finding修复后执行最终复审，
确认Critical、Important和Minor均为0并返回PASS。

## 来源与范围

新增测试覆盖canonical checkpoint、policy roundtrip、definition reconciliation、stale handle、
epoch/generation fencing、single-flight、cancel ambiguity及exactly-once effect。英文、中文文档、
ADR与source-level SDK classifier描述同一边界。

## 已知缺口

本分支按并行开发约定未执行完整workspace gate、全部feature矩阵、最终SDK artifact生成、
远端CI或MR。首次Host all-feature e2e因`errno 28`终止且随后按父任务要求清理本worktree target；
它没有产生行为失败结论。完整workspace/SDK artifact/远端CI由集成分支按合并门禁执行。
