---
schema_version: 1
id: evidence.skill-activation-authority-repair
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
  - Catalog descriptors and prepared documents remain copied into the concurrent tool adapter because their lookup and mutation contracts differ from runtime activation
  - Checkpoint restore re-establishes installed names and descriptor-derived policy but does not replay inline commands or recreate missing definitions
  - A restored active Skill has no process-local cached activation content; explicit reactivation fails closed until reset rather than replaying an uncertain effect
---

# Skill activation 唯一权威修复证据

## 支持的结论

基准`0878a676c9128619fdfb2faa15868a1388689822`中，主`SkillRegistry`与
progressive tool registry分别保存activated set和sandbox policy。直接API激活与
checkpoint restore只写主registry，`activate_skill`、`read_skill_resource`和
`run_skill_script`只读写progressive registry。把checkpoint roundtrip测试单独应用到
基准后，resource读取在`assert!(resource.success)`处稳定失败。

当前公开但process-local的`SkillActivationHandle`是跨crate唯一权威；其私有mutex原子拥有
epoch、per-Skill generation、activated content/policy与in-flight claim。API与tool对相同
`(name,args,source)`共享single-flight和已提交content，不重复执行inline command；不同key只在
前一代完成后创建新generation。reset/remove/descriptor replacement会fence旧完成，owner
cancel/panic会poison claim，显式reset前不得自动重放不确定effect。

`AgentRunSnapshot`克隆canonical handle，checkpoint保存不再读取telemetry Vec；restore只从
当前已安装descriptor原子重建name+policy。`activation_view`只复制definition view并共享handle。
SDK register/prepared/tag/remove/unregister改走ReactAgent reconciliation API，原始
`skill_registry_mut`被删除，因此primary与progressive definitions不会由Session facade分叉或
让旧tool handle复活已删除Skill。Replacement先完成load-policy与code-skill conflict预检，
通过后才撤销旧generation；commit阶段不重复调用可能有状态的policy，因此deny不会破坏旧
definition、activation、policy、projection或resource访问。

`SkillActivationHandle`、`activation_handle/view`和restore construction seam不再用
`doc(hidden)`逃逸inventory；facade classifier与artifact contract test将它们固定为
`host_or_rust_only`的`process-local-skill-activation-authority`。

## 来源与范围

Registry/Handle与三个progressive tools构成framework activation边界；ReactAgent负责
definition reconciliation、context projection和checkpoint hydration；SDK Host只把现有
source operation路由到该Agent authority。ADR 0044、双语文档和inventory classifier记录
持久合同。

## 已知缺口

本修复不改变Hook activation决策、Skill telemetry schema、plugin generation、MCP/LSP
生命周期或checkpoint文件写入原子性；对应open Finding不因本修复关闭。
