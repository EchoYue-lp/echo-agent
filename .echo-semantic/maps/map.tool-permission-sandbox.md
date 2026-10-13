---
schema_version: 1
id: map.tool-permission-sandbox
kind: capability_map
title: Tool、Permission、Sandbox 与外部 Effect
risk: high
observed_at: source:34d5f5574c9cf5a645970cd8b58367c39bce268630488b68d3ea54e996728e17
boundary_refs:
- boundary.tool-permission-sandbox
behavior_refs:
- behavior.effect-permission-execution
rule_refs:
- rule.permission-effect-order
evidence_refs:
- evidence.effects-extensions
- evidence.high-risk-audit-frontier
- evidence.streaming-tool-validation-repair
- evidence.streaming-tool-validation-verification
- evidence.tool-read-cache-authority-repair
- evidence.tool-read-cache-authority-verification
- evidence.tool-registry-owned-handle-repair
- evidence.tool-registry-owned-handle-verification
- evidence.framework-concept-navigation
- evidence.mcp-tool-local-classification-repair
- evidence.mcp-tool-local-classification-verification
- evidence.hook-protected-path-repair
- evidence.hook-protected-path-verification
- evidence.readonly-tool-capability-repair
- evidence.readonly-tool-capability-verification
- evidence.k8s-sandbox-cleanup-settlement-repair
- evidence.k8s-sandbox-cleanup-settlement-verification
- evidence.effect-cleanup-owner-repair
- evidence.effect-cleanup-owner-verification
- evidence.docker-control-fixture-deadlines
- evidence.shared-tool-input-validation-candidate
- evidence.guard-overlap-redaction-repair
- evidence.guard-overlap-redaction-verification
- evidence.data-pivot-value-fidelity-repair
- evidence.data-pivot-value-fidelity-verification
- evidence.framework-final-34d5-full-verification
- evidence.framework-final-34d5-gate-archive
finding_refs:
- finding.tool-read-cache-scope
- finding.tool-read-cache-inflight-invalidation-race
- finding.tool-registry-mutation-active-call-deadlock
- finding.streaming-tool-validation
- finding.plan-mode-write-surface
- finding.readonly-tools-custom-registration-bypass
- finding.approval-authority
- finding.hook-protected-path
- finding.hook-permission-precedence
- finding.sandbox-minimum-isolation
- finding.sandbox-manager-stream-failure-typing
- finding.guard-direction-contract
- finding.trace-effect-event-producers
- finding.trace-audit-secret-boundary
- finding.effect-cleanup-owner
- finding.k8s-sandbox-cleanup-settlement
- finding.tool-terminal-observation-divergence
- finding.command-cell-retention-lease-prune-race
- finding.command-cell-cancel-artifact-settlement
- finding.tool-pipeline-example-drift
- finding.guard-overlap-redaction
- finding.data-pivot-value-fidelity
audit_refs:
- audit.tool-permission-sandbox.permission-external
- audit.tool-permission-sandbox.failure-concurrency
- audit.tool-permission-sandbox.result-side-effect
- audit.streaming-tool-validation-rereview
- audit.tool-read-cache-authority-rereview
- audit.tool-registry-owned-handle-rereview
- audit.mcp-tool-local-classification-rereview
- audit.hook-protected-path-rereview
- audit.readonly-tool-capability-rereview
- audit.k8s-sandbox-cleanup-settlement-rereview
- audit.effect-cleanup-owner-rereview
- audit.guard-overlap-redaction-rereview
- audit.data-pivot-value-fidelity-rereview
- audit.framework-final-34d5-gate-rereview
related_map_refs:
- map.agent-session-turn
- map.task-subagent-workflow
- map.observation-persistence-delivery
- map.extension-lifecycle
scenarios:
  exact-process-group-cleanup:
    status: mapped
    source_refs:
    - echo-orchestration/src/tasks/command_cell.rs
    - echo-tools/src/shell.rs
    - docs/en/02-tools.md
    - docs/zh/02-tools.md
    evidence_refs:
    - evidence.process-group-kill-delimiter
    rule_refs:
    - rule.permission-effect-order
  agent-automated-policy-pipeline:
    status: needs_review
    source_refs:
    - echo-core/src/tools/mod.rs
    - echo-execution/src/tools.rs
    - src/agent/react/run/pipeline.rs
    - src/agent/snapshot.rs
    behavior_refs:
    - behavior.effect-permission-execution
    evidence_refs:
    - evidence.effects-extensions
    - evidence.streaming-tool-validation-repair
    - evidence.streaming-tool-validation-verification
    - evidence.hook-protected-path-repair
    - evidence.hook-protected-path-verification
    finding_refs:
    - finding.streaming-tool-validation
    - finding.plan-mode-write-surface
    - finding.approval-authority
    - finding.hook-protected-path
    - finding.trace-audit-secret-boundary
    - finding.tool-terminal-observation-divergence
    - finding.tool-pipeline-example-drift
    audit_refs:
    - audit.streaming-tool-validation-rereview
    - audit.hook-protected-path-rereview
    unknown: ReactAgent自动工具路径有确定stage顺序，streaming validation与Hook Allow protected-path反例已闭合；其它permission precedence、secret
      retention与示例合同尚未闭合
    next_step: 沿真实 15-stage pipeline 执行 focused audit，不以静态示例数组作为权威
  programmatic-tool-manager:
    status: mapped
    source_refs:
    - echo-core/src/tools/mod.rs
    - echo-execution/src/tools.rs
    finding_refs:
    - finding.tool-read-cache-scope
    - finding.tool-read-cache-inflight-invalidation-race
    - finding.tool-registry-mutation-active-call-deadlock
    - finding.streaming-tool-validation
    evidence_refs:
    - evidence.streaming-tool-validation-repair
    - evidence.streaming-tool-validation-verification
    - evidence.tool-read-cache-authority-repair
    - evidence.tool-read-cache-authority-verification
    - evidence.tool-registry-owned-handle-repair
    - evidence.tool-registry-owned-handle-verification
    audit_refs:
    - audit.streaming-tool-validation-rereview
    - audit.tool-read-cache-authority-rereview
    - audit.tool-registry-owned-handle-rereview
  trusted-hook-effects:
    status: mapped
    source_refs:
    - echo-execution/src/skills/hooks.rs
    - src/agent/react/run/pipeline.rs
    behavior_refs:
    - behavior.extension-publication
    rule_refs:
    - rule.permission-effect-order
    evidence_refs:
    - evidence.effects-extensions
  plan-and-readonly-surface:
    status: needs_review
    source_refs:
    - src/agent/react/mod.rs
    - src/agent/react/run/pipeline.rs
    - echo-tools/src/registry.rs
    finding_refs:
    - finding.plan-mode-write-surface
    - finding.readonly-tools-custom-registration-bypass
    evidence_refs:
    - evidence.readonly-tool-capability-repair
    - evidence.readonly-tool-capability-verification
    audit_refs:
    - audit.readonly-tool-capability-rereview
    unknown: 构造期 readonly_tools 的 custom registration、晚注入可见性与执行反例已闭合；Plan mode 的 effect-boundary 修复已有真实 Hook、permit
      与动态替换回归，仍待独立复审与交付
    next_step: 复核真实运行期拒绝的 blocked/Unavailable 与 observation 结算，不影响 direct-user surface
  permission-and-approval:
    status: mapped
    source_refs:
    - echo-core/src/tools/permission.rs
    - echo-orchestration/src/human_loop/service.rs
    - echo-tools/src/shell.rs
    finding_refs:
    - finding.approval-authority
    - finding.hook-protected-path
    - finding.hook-permission-precedence
    rule_refs:
    - rule.permission-effect-order
    evidence_refs:
    - evidence.hook-protected-path-repair
    - evidence.hook-protected-path-verification
    audit_refs:
    - audit.hook-protected-path-rereview
  sandbox-and-resource-cleanup:
    status: mapped
    source_refs:
    - echo-core/src/sandbox.rs
    - echo-execution/src/sandbox/manager.rs
    - echo-execution/src/sandbox/resource_owner.rs
    - echo-execution/src/sandbox/docker.rs
    - echo-execution/src/sandbox/k8s.rs
    - echo-execution/src/sandbox/local.rs
    - echo-core/src/tools/artifact.rs
    - echo-tools/src/git_worktree.rs
    - src/agent/react/mod.rs
    finding_refs:
    - finding.sandbox-minimum-isolation
    - finding.sandbox-manager-stream-failure-typing
    - finding.effect-cleanup-owner
    - finding.k8s-sandbox-cleanup-settlement
    evidence_refs:
    - evidence.effects-extensions
    - evidence.k8s-sandbox-cleanup-settlement-repair
    - evidence.k8s-sandbox-cleanup-settlement-verification
    - evidence.effect-cleanup-owner-repair
    - evidence.effect-cleanup-owner-verification
    audit_refs:
    - audit.k8s-sandbox-cleanup-settlement-rereview
    - audit.effect-cleanup-owner-rereview
  guard-and-trace-projection:
    status: needs_review
    source_refs:
    - echo-core/src/guard/mod.rs
    - src/agent/snapshot.rs
    - src/trace/mod.rs
    - echo-state/src/audit/memory.rs
    finding_refs:
    - finding.guard-direction-contract
    - finding.trace-effect-event-producers
    - finding.trace-audit-secret-boundary
    unknown: Guard directions/event producers 未闭合，trace 与 in-memory audit 原样保存输入，不能作全局 secret-redaction 保证
    next_step: audit 每个 observation backend 的 retention/redaction contract
  command-cell-runtime:
    status: needs_review
    source_refs:
    - echo-core/src/tools/cell.rs
    - echo-orchestration/src/tasks/command_cell.rs
    - docs/adr/0025-deterministic-command-cell-watcher.md
    behavior_refs:
    - behavior.effect-permission-execution
    evidence_refs:
    - evidence.effects-extensions
    - evidence.task-subagent-workflow
    finding_refs:
    - finding.command-cell-retention-lease-prune-race
    - finding.command-cell-cancel-artifact-settlement
    unknown: retention lease prune 与普通 cancel artifact settlement 存在确认的生命周期缺口
    next_step: task runtime repair 分别闭合 lease-aware prune 与 cancellation-bounded finalization
  k8s-caller-drop:
    status: mapped
    source_refs:
    - echo-execution/src/sandbox/manager.rs
    - echo-execution/src/sandbox/k8s.rs
    finding_refs:
    - finding.k8s-sandbox-cleanup-settlement
    behavior_refs:
    - behavior.effect-permission-execution
    rule_refs:
    - rule.permission-effect-order
    evidence_refs:
    - evidence.k8s-sandbox-cleanup-settlement-repair
    - evidence.k8s-sandbox-cleanup-settlement-verification
    audit_refs:
    - audit.k8s-sandbox-cleanup-settlement-rereview
  direct-user-surface:
    status: excluded
    source_refs:
    - docs/en/39-framework-application-boundary.md
    reason: 用户主动 terminal/MCP/file-picker 的产品交互与 Agent 自动工具 permission 不是同一 framework 入口
    risk: high
    recheck_when: embedding application 将 direct-user 操作通过 framework Agent pipeline 执行时
  direct-tool-input-validation:
    status: needs_review
    source_refs:
    - echo-execution/src/tools.rs
    - src/tools/mod.rs
    - docs/adr/0083-shared-tool-input-validation.md
    - docs/en/02-tools.md
    - docs/zh/02-tools.md
    behavior_refs:
    - behavior.effect-permission-execution
    rule_refs:
    - rule.permission-effect-order
    evidence_refs:
    - evidence.shared-tool-input-validation-candidate
    unknown: 源码薄复用与SDK真实chart反例已有，正式全验证/发布消费尚待
    next_step: 完成当前框架门禁与实际SDK正式负例，不增加平行validator或把Rust inventory当跨语言合同
  guard-overlap-redaction:
    status: mapped
    source_refs:
    - echo-core/src/guard/content.rs
    - docs/en/18-guard-system.md
    - docs/zh/18-guard-system.md
    behavior_refs:
    - behavior.effect-permission-execution
    rule_refs:
    - rule.permission-effect-order
    finding_refs:
    - finding.guard-overlap-redaction
    evidence_refs:
    - evidence.guard-overlap-redaction-repair
    - evidence.guard-overlap-redaction-verification
  data-pivot-value-fidelity:
    status: mapped
    source_refs:
    - echo-tools/src/data.rs
    - docs/en/21-common-tools.md
    - docs/zh/21-common-tools.md
    behavior_refs:
    - behavior.effect-permission-execution
    rule_refs:
    - rule.permission-effect-order
    finding_refs:
    - finding.data-pivot-value-fidelity
    evidence_refs:
    - evidence.data-pivot-value-fidelity-repair
    - evidence.data-pivot-value-fidelity-verification
---






# Tool、Permission、Sandbox 与外部 Effect

## 能力范围

覆盖 Tool contract/manager/pipeline、visibility、permission/approval、Guard/Hook rewrite、Sandbox、artifact、CommandCell 和具体 file/process/network/Git effect。

## 入口与输出

LLM tool call 进入 ReactAgent policy pipeline；programmatic ToolManager call 由 caller 拥有 policy；用户配置 Hook 是 trusted extension 且可在 PermissionStage 前产生 effect。三类入口输出 typed ToolResult/Failure/artifact/event 或外部状态变化。

## 行为关系

ToolManager 管执行 primitive，ReactAgent pipeline 组合自动工具策略，PermissionService 管其用户许可，CommandPolicy 管 shell 命令类别，SandboxPolicy 管隔离；trusted Hook effect 不伪装成同一授权入口。

## 状态与数据流

ReactAgent requested/effective invocation在执行前固定；owned Arc Tool generation脱离registry guard后进入异步执行，permit/cache/retry由ToolManager管理。Stream validation已闭合，read cache以effective workspace/invocation key与write-lifetime epoch约束freshness；resource guard/artifact保存完整输出与cleanup信息。

## 策略来源与优先级

Intervention、visibility、plan/readonly、PreToolUse Hook rewrite、protected-path、Hook permission、read-before-edit、Skill allowlist、execute、post Hook、Guard、artifact、trace/callback 依序作用。protected-path 拒绝不能被 Hook Allow 短路；readonly Agent 的 LLM surface 与执行门禁使用同一 ToolCapabilities 分类。

## 生命周期与失败路径

Validate/authorize/rewrite/execute/stream/cancel/timeout/retry/drain/cleanup/terminal；partial side effect 不等于无副作用失败。

## 权限与敏感信息

Agent 自动路径使用 typed permission；本地直接用户功能不被其误挡；secret redaction/retention 是 backend-specific contract，当前不能宣称 trace/audit 全局不保存原始输入。

## 用户侧投影

Permission prompt、Tool progress/result、CommandCell snapshot 与 artifact ref 是投影，不自行扩大权限。

## 场景处置清单

Agent pipeline、programmatic primitive、trusted Hook effect 与 CommandCell 已拆分；#60 protected-path、#81 readonly custom Tool 和本轮 #47 精确资源 cleanup owner 反例已闭合。#47 最终源码的本地完整门禁、17-feature 矩阵与独立复审已通过；远端 PR/CI 和 main 交付待验收。其他已知缺口继续保留 Finding；K8s caller-drop 已由detached owner、typed cleanup debt和故障注入映射，direct-user surface明确excluded。

## 未展开项

具体 domain tool 的算法正确性在后续 Finding/audit 中按需展开。

历史快照定位说明：原 `source:cd5860e15353dbaf0c833ba1c224671c8b990e521a70f884e40ff2760884d5d0` 的非提交引用在本次整体源码变化后不可恢复。逐条核对该对象实际源码引用与 `95ec66c6083a8feb17a1c28de0d01e1a24ca8a28` 的 blob 完全相同，现仅将历史观察定位到该可恢复 Git revision；未提升状态或声称本轮重新审查该边界。

直接适配复用：validate_tool_input沿同工具schema/customvalidator，在副作用前校验；只有visibility/docs新增，执行语义不归adapter。当前增量证据明确候选范围，历史23不冒新版通过。


本轮场景 guard-overlap-redaction：ContentGuard 在原输入上合并重叠覆盖区间，选择最宽成员标记（同宽按既有detect顺序）；char_indices 单次输出，邻接不重叠区间分别保留。detect的原始byte offsets合同不变，未新增策略/API/SDK parser。 本地故障focused与独立局部复审闭合；完整工程/消费者/交付不由mapped代替。


本轮场景 data-pivot-value-fidelity：保留既有Polars grouping/aggregation/join权威；LiteralValue::from(AnyValue)保留原生值、null使用is_null、join使用nulls_equal。header与匹配值分离，冲突在join前返回InvalidParameter。aggregation只有缺省sum，声明外string及null/number/bool类型拒绝。未新增pivot引擎或平行validator。 本地故障focused与独立局部复审闭合；完整工程/消费者/交付不由mapped代替。


当前工程补充：同source34d5的新23门禁真实全通过，3471 passed/0 failed/3外部optin ignored、86targets/59examples；见 evidence.framework-final-34d5-full-verification 与原始 evidence.framework-final-34d5-gate-archive。原历史门禁和局部Audit不改写，mapped不表示所有lens/消费者/main已验收。


当前候选最终复审：audit.framework-final-34d5-gate-rereview 绑定新23真实工程收据与完整已审源码，implementation review PASS/action0；不替代独立SDK/main验收，不升级原broad状态。
