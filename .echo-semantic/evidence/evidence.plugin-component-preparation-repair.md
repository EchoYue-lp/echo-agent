---
schema_version: 1
id: evidence.plugin-component-preparation-repair
kind: evidence
observed_at: 7e74d1443567981f318b302845d31a5673c76462
source_refs:
  - src/plugin/prepared.rs
  - echo-execution/src/skills/external/loader.rs
  - docs/adr/0012-immutable-plugin-preparation.md
  - docs/en/32-plugin-system.md
  - docs/zh/32-plugin-system.md
supports: [behavior.extension-publication, rule.extension-generation-authority]
limitations:
  - 本修复不增加active generation CAS或禁止旧PreparedPluginSet重新apply
  - 本修复不协调PluginRegistry、wiring与callback lifecycle
  - 本修复不处理reconcile overlap或MCP server owner-qualified identity
  - EKO应用层不在本切片范围
---

# Plugin component preparation 隔离修复证据

## 支持的结论

基准`0878a676c9128619fdfb2faa15868a1388689822`把任意
`PluginDiagnosticSeverity::Error`归约为完整`PreparedPluginSet`不可应用。一个无效
Skill或Hook虽然已经被loader/parser排除，仍会阻止同一Plugin中的健康兄弟组件和其它
Plugin进入`wire_prepared`。

当前`PluginIntegrator::prepare`继续生成唯一、依赖有序、不可变的完整
`PreparedPluginSet`，但将适用性保留给代次级不变量。无效Skill、Hook或MCP组件以及无法
读取的Subagent/LSP冻结文档被排除并保留结构化error diagnostic；健康兄弟组件和Plugin仍
属于同一代次快照。依赖图无法排序、完整Plugin准备失败或generation无法分配时仍使完整set
不可应用，不允许发布缺失必需依赖的闭包。Subagent/LSP产品语法由EKO第二阶段prepare隔离。

## 来源与范围

`src/plugin/prepared.rs`是prepare/apply的既有framework authority；SkillLoader继续负责
单个Skill解析和诊断，未新增registry、发布状态机或EKO policy。ADR 0012与中英文Plugin
文档已统一为ADR 0045 DU-71确认的“组件隔离、完整代次原子发布”合同。

## 已知缺口

Finding #72的active generation authority、#73的lifecycle coordination、#74的旧新代资源
overlap及#75的MCP owner isolation保持开放。本Evidence只为这些后续repair提供兼容的
prepare输入，不证明apply/unwire已经具备generation fencing、跨代replacement ordering或
跨owner结算；不可变prepared input本身不等于active-generation publication fence。
