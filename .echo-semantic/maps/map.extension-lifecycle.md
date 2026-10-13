---
schema_version: 1
id: map.extension-lifecycle
kind: capability_map
title: MCP、Hook、Skill、Plugin 与 LSP 生命周期
risk: high
observed_at: 554f2f50d3705d219519f306055a3d26d6a5211c
boundary_refs:
- boundary.extension-lifecycle
behavior_refs:
- behavior.channel-handler-invocation
- behavior.extension-publication
rule_refs:
- rule.extension-generation-authority
- rule.permission-effect-order
evidence_refs:
- evidence.effects-extensions
- evidence.high-risk-audit-frontier
- evidence.framework-concept-navigation
- evidence.extension-credential-debug-redaction-repair
- evidence.extension-credential-debug-redaction-verification
- evidence.skill-activation-authority-repair
- evidence.skill-activation-authority-verification
- evidence.mcp-tool-local-classification-repair
- evidence.mcp-tool-local-classification-verification
- evidence.mcp-client-capability-advertisement-repair
- evidence.mcp-client-capability-advertisement-verification
- evidence.mcp-protocol-negotiation-repair
- evidence.mcp-protocol-negotiation-verification
- evidence.lsp-derived-handle-lifecycle-repair
- evidence.lsp-derived-handle-lifecycle-verification
- evidence.plugin-component-preparation-repair
- evidence.plugin-generation-publication-authority-repair
- evidence.plugin-generation-publication-authority-verification
- evidence.plugin-mcp-owner-isolation-repair
- evidence.plugin-mcp-owner-isolation-verification
- evidence.plugin-lifecycle-coordinator-repair
- evidence.plugin-lifecycle-coordinator-verification
- evidence.channel-manager-borrowed-await-repair
- evidence.channel-manager-borrowed-await-verification
- evidence.framework-final-34d5-full-verification
- evidence.framework-final-34d5-gate-archive
finding_refs:
- finding.skill-activation-authority
- finding.hook-permission-precedence
- finding.hook-protected-path
- finding.hook-event-producer-contract
- finding.mcp-client-capability-advertisement
- finding.mcp-tool-permission-classification
- finding.plugin-mcp-owner-isolation
- finding.plugin-failure-isolation-contract
- finding.plugin-lifecycle-coordination
- finding.plugin-generation-publication-authority
- finding.plugin-lifecycle-reconcile-overlap
- finding.lsp-runtime-state
- finding.lsp-manager-derived-handle-resurrection
- finding.extension-cleanup-settlement
- finding.extension-credential-debug-redaction
- finding.mcp-version-doc-drift
- finding.channel-manager-borrowed-await
audit_refs:
- audit.channel-handler-invocation-rereview
- audit.extension-lifecycle.state-authority
- audit.extension-lifecycle.time-lifecycle
- audit.extension-lifecycle.permission-external
- audit.extension-lifecycle.contract-evidence
- audit.skill-activation-authority-rereview
- audit.mcp-tool-local-classification-rereview
- audit.mcp-client-capability-advertisement-rereview
- audit.mcp-protocol-negotiation-rereview
- audit.lsp-derived-handle-lifecycle-rereview
- audit.plugin-component-preparation-rereview
- audit.plugin-generation-publication-authority-rereview
- audit.plugin-mcp-owner-isolation-rereview
- audit.plugin-lifecycle-coordinator-rereview
- audit.channel-manager-borrowed-await-rereview
- audit.framework-final-34d5-gate-rereview
related_map_refs:
- map.workspace-architecture
- map.context-memory
- map.tool-permission-sandbox
- map.protocol-surfaces
scenarios:
  channel-handler-forward-invocation:
    status: mapped
    source_refs: [echo-core/src/error.rs, echo-integration/src/channels/manager.rs, echo-integration/src/channels/types.rs, echo-integration/src/channels/session.rs, src/channels.rs, docs/adr/0085-channel-handler-invocation-ownership.md]
    behavior_refs: [behavior.channel-handler-invocation, behavior.channel-handler-invocation]
    rule_refs: [rule.channel-invocation-generation]
    finding_refs: [finding.channel-handler-invocation-settlement]
    evidence_refs: [evidence.channel-handler-forward-repair, evidence.channel-handler-forward-focused]
  mcp-connect-discover-close:
    status: needs_review
    source_refs:
    - echo-integration/src/mcp/client.rs
    - echo-integration/src/mcp/mod.rs
    - echo-integration/src/mcp/transport/sse.rs
    finding_refs:
    - finding.mcp-client-capability-advertisement
    - finding.mcp-tool-permission-classification
    - finding.extension-cleanup-settlement
    - finding.extension-credential-debug-redaction
    - finding.mcp-version-doc-drift
    evidence_refs:
    - evidence.effects-extensions
    - evidence.mcp-tool-local-classification-repair
    - evidence.mcp-tool-local-classification-verification
    - evidence.mcp-client-capability-advertisement-repair
    - evidence.mcp-client-capability-advertisement-verification
    - evidence.mcp-protocol-negotiation-repair
    - evidence.mcp-protocol-negotiation-verification
    - evidence.extension-cleanup-settlement-repair
    - evidence.extension-cleanup-settlement-verification
    audit_refs:
    - audit.mcp-tool-local-classification-rereview
    - audit.mcp-client-capability-advertisement-rereview
    - audit.mcp-protocol-negotiation-rereview
    - audit.extension-cleanup-settlement-rereview
    unknown: MCP cleanup在当前任务分支由retained scope等待并可重试；本地完整门禁与17项独立feature已通过，PR/CI和远端main仍待交付，MCP版本文档与其它独立Finding另行验收
    next_step: 完成#55的PR/CI与远端main交付，并单独处理MCP版本文档合同
  hook-source-and-reduction:
    status: mapped
    source_refs:
    - echo-core/src/hooks/types.rs
    - echo-execution/src/skills/hooks.rs
    finding_refs:
    - finding.hook-permission-precedence
    - finding.hook-protected-path
    rule_refs:
    - rule.permission-effect-order
    evidence_refs:
    - evidence.hook-protected-path-repair
    - evidence.hook-protected-path-verification
    audit_refs:
    - audit.hook-protected-path-rereview
  skill-discovery-activation-restore:
    status: mapped
    source_refs:
    - echo-execution/src/skills/external/loader.rs
    - echo-execution/src/skills/registry.rs
    - src/agent/react/capabilities.rs
    - src/agent/react/mod.rs
    finding_refs:
    - finding.skill-activation-authority
    evidence_refs:
    - evidence.effects-extensions
    - evidence.skill-activation-authority-repair
    - evidence.skill-activation-authority-verification
  plugin-prepare-publish-withdraw:
    status: mapped
    source_refs:
    - echo-core/src/plugin/registry.rs
    - echo-core/src/plugin/lifecycle.rs
    - echo-integration/src/mcp/identity.rs
    - echo-integration/src/mcp/mod.rs
    - echo-integration/src/mcp/resource_tool.rs
    - src/plugin/coordinator.rs
    - src/plugin/prepared.rs
    - src/agent/react/capabilities.rs
    - src/agent/react/mod.rs
    finding_refs:
    - finding.plugin-mcp-owner-isolation
    - finding.plugin-failure-isolation-contract
    - finding.plugin-lifecycle-coordination
    - finding.plugin-generation-publication-authority
    - finding.plugin-lifecycle-reconcile-overlap
    rule_refs:
    - rule.extension-generation-authority
    evidence_refs:
    - evidence.plugin-component-preparation-repair
    - evidence.plugin-generation-publication-authority-repair
    - evidence.plugin-generation-publication-authority-verification
    - evidence.plugin-mcp-owner-isolation-repair
    - evidence.plugin-mcp-owner-isolation-verification
    - evidence.plugin-lifecycle-coordinator-repair
    - evidence.plugin-lifecycle-coordinator-verification
    audit_refs:
    - audit.plugin-generation-publication-authority-rereview
    - audit.plugin-mcp-owner-isolation-rereview
  lsp-process-routing:
    status: mapped
    source_refs:
    - echo-core/src/lsp/client.rs
    - echo-integration/src/lsp/client.rs
    - echo-integration/src/lsp/manager.rs
    finding_refs:
    - finding.lsp-runtime-state
    - finding.lsp-manager-derived-handle-resurrection
    evidence_refs:
    - evidence.lsp-derived-handle-lifecycle-repair
    - evidence.lsp-derived-handle-lifecycle-verification
    audit_refs:
    - audit.lsp-derived-handle-lifecycle-rereview
  host-production-coordination:
    status: mapped
    source_refs:
    - src/plugin/coordinator.rs
    - src/plugin/prepared.rs
    - echo-core/src/plugin/registry.rs
    - echo-core/src/plugin/lifecycle.rs
    finding_refs:
    - finding.plugin-lifecycle-coordination
    - finding.hook-event-producer-contract
    evidence_refs:
    - evidence.plugin-lifecycle-coordinator-repair
    - evidence.plugin-lifecycle-coordinator-verification
    audit_refs:
    - audit.plugin-lifecycle-coordinator-rereview
    unknown: framework Host统一编排已通过完整门禁、三轮独立复审与remote-main delivery，并在verified commit dc61ef0e闭合；#58 durable
      Hook producer acknowledgement仍未闭合
    next_step: '#58 继续单独验收跨进程 Hook delivery；不再把#73路由为待交付候选'
  credential-debug-redaction:
    status: mapped
    source_refs:
    - echo-integration/src/redaction.rs
    - echo-integration/src/mcp/config_loader.rs
    - echo-integration/src/mcp/server_config.rs
    - echo-integration/src/mcp/transport/mod.rs
    - echo-integration/src/mcp/transport/http.rs
    - echo-integration/src/mcp/transport/sse.rs
    - echo-integration/src/mcp/transport/stdio.rs
    - echo-integration/src/channels/channels/qq/channel.rs
    - echo-integration/src/channels/channels/qq/api.rs
    - echo-integration/src/channels/channels/qq/gateway.rs
    - echo-integration/src/channels/channels/feishu/channel.rs
    - echo-integration/src/channels/channels/feishu/api.rs
    - echo-integration/src/channels/channels/feishu/long_poll.rs
    - echo-integration/src/channels/channels/feishu/webhook.rs
    finding_refs:
    - finding.extension-credential-debug-redaction
    evidence_refs:
    - evidence.extension-credential-debug-redaction-repair
    - evidence.extension-credential-debug-redaction-verification
  channel-manager-borrowed-await:
    status: mapped
    source_refs:
    - echo-integration/src/channels/manager.rs
    - echo-integration/src/channels/mod.rs
    - docs/adr/0084-channel-owned-operations.md
    - docs/en/15-im-channels.md
    - docs/zh/15-im-channels.md
    - docs/supreme/specs/2026-10-13T1430-channel-owned-operations/design.md
    - docs/supreme/specs/2026-10-13T1430-channel-owned-operations/plans/plan_01_闭合ChannelOwned操作.md
    behavior_refs:
    - behavior.extension-publication
    rule_refs:
    - rule.extension-generation-authority
    finding_refs:
    - finding.channel-manager-borrowed-await
    evidence_refs:
    - evidence.channel-manager-borrowed-await-repair
    - evidence.channel-manager-borrowed-await-verification
---





# MCP、Hook、Skill、Plugin 与 LSP 生命周期

## 能力范围

覆盖五类 extension 的发现、解析、命名、注册、激活、发布、热更新、撤销、关闭、generation 与 rollback。

## 入口与输出

用户/project/plugin config、SDK operation 与 Agent runtime 触发；输出 Tool/Resource/Hook/Skill/Subagent/LSP projections 和 child process/network effect。

## 行为关系

每类 registry 管自身状态，Plugin prepared generation 组合发布；跨组件不得因统一 Plugin 包而丢失 owner。

## 状态与数据流

MCP client/topology、Hook sources/result、Plugin registry/prepared/lifecycle、LSP config/client maps 分别演进。Plugin active publication和receipt的唯一authority在每个ReactAgent，Integrator只共享prepare cache。Skill descriptor/prepared document保留定义视图，主registry与progressive tool adapter共享唯一activation state。

## 策略来源与优先级

Scope/owner/config、official Skill/Plugin/MCP contracts、Hook source order 和 feature capabilities 决定发布。

## 生命周期与失败路径

Discover/prepare/connect/apply/activate/start，replace/reload，unwire/deactivate/stop/close；partial failure 需 rollback 或 cleanup debt。

## 权限与敏感信息

MCP annotation、Hook permission、Skill allowlist/sandbox 与 Plugin variables 不得泄漏 credential 或绕过不可替代的本地数据保护。

## 用户侧投影

Catalog/status/tool list 是 registry projection；仅可执行且当前 generation 的组件才能被广告。

## 场景处置清单

五类生命周期和十六个Finding已映射；Skill activation authority、Plugin prepare failure
isolation、active generation 与 MCP owner isolation 已在主线修复并通过独立复审。跨 Host
统一编排已覆盖 framework Host 主路径与失败重试矩阵，并以verified `dc61ef0e`进入远端main；
#73已完成，#58继续覆盖跨进程 Hook producer acknowledgement。MCP construction cleanup
owner 在本任务分支通过完整本地门禁、17项独立feature检查与独立复审，待PR/CI和远端main交付。

## 未展开项

EKO Plugin fan-out、preferences 和 UI catalog 属应用边界。


本轮场景 channel-manager-borrowed-await：复用ChannelManager唯一map/entry；ChannelHandle直接持原plugin、handler清理责任与transport-stop epoch。get_owned和owned批量future不借用manager，SDK须短锁取owned后释放再await。entry独立read/write锁；start先登记handler，stop先记录transport成功再await close，失败/取消保留责任；get_mut使epoch失效防旧stop覆盖。旧get/get_mut保留，registration metadata固定声明的兼容取舍见ADR0084，不是第二registry/生命周期。 本地故障focused与独立局部复审闭合；完整工程/消费者/交付不由mapped代替。


当前工程补充：同source34d5的新23门禁真实全通过，3471 passed/0 failed/3外部optin ignored、86targets/59examples；见 evidence.framework-final-34d5-full-verification 与原始 evidence.framework-final-34d5-gate-archive。原历史门禁和局部Audit不改写，mapped不表示所有lens/消费者/main已验收。


当前候选最终复审：audit.framework-final-34d5-gate-rereview 绑定新23真实工程收据与完整已审源码，implementation review PASS/action0；不替代独立SDK/main验收，不升级原broad状态。


本轮channel-handler-forward归原entry及原Session/Turn边界；新场景mapped表示owner、
生命周期、开放Finding和验证缺口有明确去向。23/80的新mutex修复与full23/exactmain
尚待，不能由历史22/79或本轮mapping代替。其它开放场景不在此关闭。


Channel forward当前本地三链闭合见audit.channel-handler-invocation-rereview；仅此场景具精确源码/工程收据复核，原其它needs_review不变。


## 已发布历史快照定位

本对象原 source:d0568a66d7ccb8955e15a0d6f7aaaf0799ae25c287264ded14a9f167a4e01fbe 的完整非语义源码与已发布 Git 554f2f50d3705d219519f306055a3d26d6a5211c 唯一摘要相同。全部本对象code/source引用逐blob核对当前仍相同，现只重绑定可恢复Git定位；原N审查范围、运行时间、状态与结论保持。此历史事实不覆盖D新增Database/RAG/Clinical三源码、其测试或消费者，更不冒作本轮门禁通过。
