# ADR 0090：A2A/LSP 退役边界与 Stage0 冻结诊断适用说明

## 状态

本文件属于 F517 Stage0 候选的中文适用说明。它不删除 A2A/LSP 实现，也不改变 `lsp` feature；Framework 物理退役必须等待 EKO 与 SDK 消费者先解除依赖并分别完成主线验收。

## Stage0 决策

当 Framework 未启用 `lsp` feature 时，`PluginIntegrator::prepare` 仍沿现有 PreparedPluginSet、identity、generation、cache 和 rollback owner 冻结插件的 LSP 声明输入。声明存在且来源是普通文件时，保存原文并产生组件级 Warning，说明该组件被冻结但当前构建不能应用它；目录、断链、循环链接、非法 UTF-8、来源消失和不可读输入产生带插件身份、组件名与路径的 Error。真正没有声明的插件保持可适用且不产生 LSP 诊断。

诊断使用既有 `PluginPreparationDiagnostic`，不新建 decoder、执行器、状态权威、wire mapper 或第二份 prepared cache。健康的 Skill、Hook、MCP 和 Subagent 组件继续独立准备；启用 `lsp` feature 的原有冻结与应用路径保持不变。

这项桥接只为消费者去依赖提供准确的冻结事实。它不代表 A2A/LSP 已从 Framework 删除，不代表 EKO/SDK 已进入 main，不代表 Source/final、CI、native 或发布资格完成。后续消费者切换和 Framework 物理退役必须使用各自准确源码与门禁，不能从本候选的 focused 结果推导。

## 绑定输入

- 基底：F517 `51726454985fd6f064b21a7e9c539e22d494c987`，snapshot 2333 files/283 dirs；完整 source manifest：`cd8735d62a4c1170bd06f535e6694514f723f99451f30f1f2a42728aedab16a1`。
- 唯一代码增量：`src/plugin/prepared.rs` patch `0f2fa4399c433c02a3664ff8bb3ad34090822c7938fb385cf268464790203471`，应用后源 blob `6277087c89a598e08e390077e8a9ef0a6b2fe2157418d58c10624d1aa654139a`。
- 依赖输入保持 F517 lock `f76fe10e64ec3faf062ff5bc7a66d6cd82f82cf95e989c824f40fe6f244d234c` 与 duplicate baseline `9ae90ac986f03a16df3213a777ff821e6849f40e7284fa0acc9ae5d89ca02aa3`。
- 详细 source、focused、artifact gap 和当前 Source/final 范围见 `.echo-semantic/evidence/evidence.stage0-generic-diagnostics.md`；Map 保持 F517 基线，本候选仅以普通 Evidence 记录 Stage0 事实。

## 非目标

本候选不修改 A2A/LSP feature、公共执行事件、ReAct producer、EKO/SDK 代码、examples 或官网，不关闭历史失败，不接受新风险，不改变用户文件和旧配置。英文 `docs/en/32-plugin-system.md` 保持现有历史表述；本次新增事实源按仓库中文文档规则只维护中文 ADR 与中文插件说明。
