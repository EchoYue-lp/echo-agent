---
schema_version: 1
id: rule.extension-generation-authority
kind: rule
status: needs_review
expectation: inferred
risk: high
primary_focus: failure_concurrency
focus:
- state_authority
- time_lifecycle
- data_durability
- permission_external
observed_at: source:d0568a66d7ccb8955e15a0d6f7aaaf0799ae25c287264ded14a9f167a4e01fbe
behavior_refs:
- behavior.extension-publication
code_refs:
- echo-core/src/error.rs
- echo-integration/src/channels/manager.rs
- echo-integration/src/channels/types.rs
- echo-integration/src/channels/session.rs
- echo-integration/src/channels/mod.rs
- src/channels.rs
- docs/adr/0085-channel-handler-invocation-ownership.md
- docs/en/15-im-channels.md
- docs/zh/15-im-channels.md
- echo-integration/src/mcp/mod.rs
- echo-execution/src/skills/hooks.rs
- echo-execution/src/skills/registry.rs
- echo-core/src/plugin/registry.rs
- src/plugin/coordinator.rs
- src/plugin/prepared.rs
- src/agent/react/mod.rs
- echo-core/src/plugin/lifecycle.rs
- echo-integration/src/lsp/manager.rs
- echo-integration/src/channels/manager.rs
- echo-integration/src/channels/mod.rs
- docs/adr/0084-channel-owned-operations.md
- docs/en/15-im-channels.md
- docs/zh/15-im-channels.md
- docs/supreme/specs/2026-10-13T1430-channel-owned-operations/design.md
- docs/supreme/specs/2026-10-13T1430-channel-owned-operations/plans/plan_01_闭合ChannelOwned操作.md
evidence_refs:
- evidence.effects-extensions
- evidence.skill-activation-authority-repair
- evidence.skill-activation-authority-verification
- evidence.mcp-client-capability-advertisement-repair
- evidence.mcp-client-capability-advertisement-verification
- evidence.lsp-derived-handle-lifecycle-repair
- evidence.lsp-derived-handle-lifecycle-verification
- evidence.plugin-component-preparation-repair
- evidence.plugin-component-preparation-verification
- evidence.plugin-generation-publication-authority-repair
- evidence.plugin-generation-publication-authority-verification
- evidence.plugin-mcp-owner-isolation-repair
- evidence.plugin-mcp-owner-isolation-verification
- evidence.plugin-lifecycle-coordinator-repair
- evidence.plugin-lifecycle-coordinator-verification
- evidence.channel-manager-borrowed-await-repair
- evidence.channel-manager-borrowed-await-verification
finding_refs:
- finding.skill-activation-authority
- finding.hook-permission-precedence
- finding.mcp-client-capability-advertisement
- finding.mcp-tool-permission-classification
- finding.plugin-mcp-owner-isolation
- finding.plugin-failure-isolation-contract
- finding.plugin-lifecycle-coordination
- finding.lsp-runtime-state
- finding.lsp-manager-derived-handle-resurrection
- finding.extension-cleanup-settlement
- finding.mcp-version-doc-drift
- finding.channel-manager-borrowed-await
---



# Extension Owner 与 Generation

## 不变量或唯一权威

MCP、Hook、Skill、Plugin 和 LSP 各自使用明确 owner/registry；Plugin publish 基于不可变 prepared generation 和可回滚 wiring receipt。

## 适用行为

适用于 discovery、install/enable/disable/uninstall、prepare/apply/unwire、reload、connect/disconnect、start/stop 和 checkpoint restore。

## 当前实现

各manager/registry持有自身状态；SkillRegistry主视图、definition adapter、run snapshot与checkpoint共享唯一epoch/generation-fenced activation handle。MCP client只协商已实现capability，McpManager以typed owner identity统一索引active/prepared/debt状态；LspManager的generation/closed lifecycle覆盖所有派生client。PluginRegistry、PluginIntegrator与PluginLifecycleManager分别负责持久状态、immutable preparation和callbacks；prepare的进程级序号跨独立Integrator排序，ReactAgent持有唯一target-scoped active publication与cleanup receipt，cloned Integrator不拥有全局active generation。MCP每次成功连接在下一次await前登记typed receipt，连接前先登记可等待、可重试的preparation scope；SSE construction在暂停前交付receive task owner。

## 期望行为

同名资源不能跨 owner 被错误替换或撤销；部分 apply/close 失败保留可恢复 debt。Plugin
prepare的组件错误只排除该组件并记录诊断，健康兄弟保持在同一不可变generation；依赖排序
等代次级失败仍阻断完整set。

## 证据

MCP/Hook/Skill/Plugin/LSP 源码、ADR 0012/0023/0026 和 focused tests 提供部分证据。

## 裁决记录

Skill state、MCP capability/local classification与LSP derived ownership已修复复审；ADR 0045
DU-71与ADR 0012也已统一component isolation和atomic generation合同。Plugin active generation
已在主线 `cb4ee9ed` 交付target-scoped authority并通过独立复审；MCP owner isolation已在
`c327f2d0` 交付并复审。lifecycle coordinator 候选复用这些 authority，绑定 Agent target、
保留 dependency-ordered phase receipt，在撤销旧代前验证新代 applicable，并让取消后的
receipt 显式保持 ActualPending；其
独立复审和远端交付尚未完成，#58 与其它开放 Finding 也不由本切片关闭，故本 Rule 继续待审计。


本轮限定规则（channel-manager-borrowed-await）：复用ChannelManager唯一map/entry；ChannelHandle直接持原plugin、handler清理责任与transport-stop epoch。get_owned和owned批量future不借用manager，SDK须短锁取owned后释放再await。entry独立read/write锁；start先登记handler，stop先记录transport成功再await close，失败/取消保留责任；get_mut使epoch失效防旧stop覆盖。旧get/get_mut保留，registration metadata固定声明的兼容取舍见ADR0084，不是第二registry/生命周期。 失败/错误合同：registry guard不得跨owned await；原plugin/handler/stop责任只能存在于同entry，取消失败不能抹去close责任，批量操作不得吞兄弟结果，metadata不会等待entry锁。 test-manager-repaired.log为11 passed/0 failed/0 ignored；初次AtomicU64警告尝试保留、修复后同11测试及双lint零警告退出0。clippy-alltargets、clippy-production、doc-channels(2 passed/0 failed)、example-demo38均真实receipt exitCode0，所有log/source SHA核对。覆盖五类callback、lazy batch快照、独立entry、rollback/取消/close retry/get_mut epoch/metadata声明。 只关闭这一故障；其余needs_review与历史证据保持各自范围。


Channel forward增量由behavior.channel-handler-invocation及rule.channel-invocation-generation
限定：唯一原entry generation、callchildcancel、realreceipt/close retry和sameadmission
streamtoken；具体开放Finding与新修复待验见evidence.channel-handler-forward-focused。
不扩大本对象其它场景的needs_review或关闭状态。
