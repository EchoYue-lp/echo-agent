---
schema_version: 1
id: audit.plugin-component-preparation-rereview
kind: audit
boundary_ref: boundary.extension-lifecycle
lens: failure_concurrency
freshness: examined
revision: source:b0dfa235d236bee9185ff26953b982f196556459fd0bf165f7114a232373b224
finding_refs: [finding.plugin-failure-isolation-contract]
challenges:
  whole-plugin-dependency-closure:
    revision: source:b0dfa235d236bee9185ff26953b982f196556459fd0bf165f7114a232373b224
    source_refs: [src/plugin/prepared.rs, docs/adr/0012-immutable-plugin-preparation.md]
    evidence_refs: [evidence.plugin-component-preparation-repair, evidence.plugin-component-preparation-verification]
  component-parse-isolation:
    revision: source:b0dfa235d236bee9185ff26953b982f196556459fd0bf165f7114a232373b224
    source_refs: [src/plugin/prepared.rs, echo-execution/src/skills/external/loader.rs, docs/en/32-plugin-system.md, docs/zh/32-plugin-system.md]
    evidence_refs: [evidence.plugin-component-preparation-repair, evidence.plugin-component-preparation-verification]
  immutable-input-versus-active-publication:
    revision: source:b0dfa235d236bee9185ff26953b982f196556459fd0bf165f7114a232373b224
    source_refs: [src/plugin/prepared.rs, docs/adr/0012-immutable-plugin-preparation.md]
    evidence_refs: [evidence.plugin-component-preparation-repair]
---

# Plugin component preparation 独立复审

## 审查范围

复审framework whole-plugin dependency closure、Skill/Hook/MCP组件隔离、结构化诊断、不可变
prepared set边界，以及EKO第二阶段对Subagent/LSP/product component的消费合同。审查快照为
framework tracked diff `50be4ef0...`与CLI compatibility commit `0182e8e`加产品增量
`2ea4acdf...`。

## 已检查故障假设

验证完整Plugin准备失败后dependent是否仍可发布，Hook action与MCP parse错误是否在prepare
阶段排除并留下诊断，EKO是否仍把单个Subagent/LSP错误升级为完整候选失败，以及文档/证据
是否把immutable prepared input误述为active-generation fence。

## 实际实现路径与证据

variables、data、component resolution与identity serialization失败均使generation不可应用；
依赖Plugin data路径故障测试证明缺失必需依赖的set被拒绝。Skill、Hook parse/action和MCP错误
只排除对应component，健康兄弟仍可wire。EKO第二阶段返回有效组件与结构化diagnostics，
runtime role collision也只排除冲突Subagent；ReloadSummary归并framework/application两级诊断。

Framework prepared测试11/11、EKO真实组合check、TaskGraph compatibility测试1/1、Plugin
component isolation测试3/3、两仓库focused Clippy与fmt/diff检查均通过。独立reviewer确认上轮
3个Important与1个Minor全部关闭，未发现新的行动项。

## 问题记录

`finding.plugin-failure-isolation-contract`具备repair、verification与独立rereview证据，可标记
resolved。Finding #72 active generation、#73 lifecycle coordination、#74 reconcile overlap与
#75 MCP owner isolation不属于本切片，继续保持open。

## 残余风险

不可变prepare输入不阻止旧generation重新apply或旧receipt撤销新资源；跨代replacement顺序、
callback lifecycle与MCP owner-qualified identity仍需后续repair。统一semantic source digest
已由合流最新main后的集成候选刷新；完整workspace门禁仍受本机磁盘空间限制，留给后续MR
验收与远端CI。

## 未检查项

未执行完整workspace/all-feature门禁、远端CI、真实外部MCP/LSP进程故障或并行generation
apply/unwire；这些不由Finding #71单独证明。
