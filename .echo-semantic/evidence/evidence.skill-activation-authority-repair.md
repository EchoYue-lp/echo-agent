---
schema_version: 1
id: evidence.skill-activation-authority-repair
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
  - Catalog descriptors and prepared documents remain copied into the concurrent tool adapter because their lookup and mutation contracts differ from runtime activation
  - Checkpoint restore re-establishes active names but does not replay Skill inline commands or create missing catalog definitions
---

# Skill activation 唯一权威修复证据

## 支持的结论

基准`0878a676c9128619fdfb2faa15868a1388689822`中，主`SkillRegistry`与
progressive tool registry分别保存activated set和sandbox policy。直接API激活与
checkpoint restore只写主registry，`activate_skill`、`read_skill_resource`和
`run_skill_script`只读写progressive registry。把checkpoint roundtrip测试单独应用到
基准后，resource读取在`assert!(resource.success)`处稳定失败。

当前`SkillRegistry`私有`SkillActivationState`唯一拥有session id、activated names与
activation-derived sandbox policies。`activation_view`只创建共享该私有状态的空definition
view；ReactAgent discovery仍把descriptor/prepared document同步给tool adapter，但不再创建
第二套activation state。Tool activation、直接API、snapshot allowlist、checkpoint保存/恢复、
resource/script检查及remove/reset均读取或修改同一状态。

## 来源与范围

Registry实现和三个progressive tools构成framework activation边界；ReactAgent只负责
catalog接线、context projection和checkpoint hydration。ADR 0044记录定义数据与运行时状态的
边界，英文和中文Skill文档同步对外合同。

## 已知缺口

本修复不改变Hook activation决策、Skill telemetry、plugin generation、MCP/LSP生命周期或
checkpoint本身的持久化原子性；对应open Finding不因本修复关闭。
