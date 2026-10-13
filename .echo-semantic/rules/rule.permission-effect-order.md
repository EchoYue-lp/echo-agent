---
schema_version: 1
id: rule.permission-effect-order
kind: rule
status: needs_review
expectation: human_confirmed
risk: high
primary_focus: permission_external
focus:
- result_side_effect
- state_authority
- failure_concurrency
- time_lifecycle
observed_at: source:34d5f5574c9cf5a645970cd8b58367c39bce268630488b68d3ea54e996728e17
behavior_refs:
- behavior.effect-permission-execution
code_refs:
- echo-core/src/tools/permission.rs
- echo-orchestration/src/human_loop/service.rs
- echo-execution/src/skills/hooks.rs
- src/agent/react/run/pipeline.rs
- echo-execution/src/tools.rs
- echo-execution/src/sandbox/k8s.rs
- echo-core/src/guard/content.rs
- docs/en/18-guard-system.md
- docs/zh/18-guard-system.md
- echo-tools/src/data.rs
- docs/en/21-common-tools.md
- docs/zh/21-common-tools.md
evidence_refs:
- evidence.effects-extensions
- evidence.streaming-tool-validation-repair
- evidence.streaming-tool-validation-verification
- evidence.tool-read-cache-authority-repair
- evidence.tool-read-cache-authority-verification
- evidence.tool-registry-owned-handle-repair
- evidence.tool-registry-owned-handle-verification
- evidence.mcp-tool-local-classification-repair
- evidence.mcp-tool-local-classification-verification
- evidence.hook-protected-path-repair
- evidence.hook-protected-path-verification
- evidence.readonly-tool-capability-repair
- evidence.readonly-tool-capability-verification
- evidence.effect-cleanup-owner-repair
- evidence.effect-cleanup-owner-verification
- evidence.shared-tool-input-validation-candidate
- evidence.guard-overlap-redaction-repair
- evidence.guard-overlap-redaction-verification
- evidence.data-pivot-value-fidelity-repair
- evidence.data-pivot-value-fidelity-verification
finding_refs:
- finding.tool-registry-mutation-active-call-deadlock
- finding.streaming-tool-validation
- finding.plan-mode-write-surface
- finding.approval-authority
- finding.hook-protected-path
- finding.sandbox-minimum-isolation
- finding.trace-audit-secret-boundary
- finding.effect-cleanup-owner
- finding.tool-pipeline-example-drift
- finding.guard-overlap-redaction
- finding.data-pivot-value-fidelity
---




# Permission 与 Effect 顺序

## 不变量或唯一权威

ReactAgent 的 LLM 驱动自动 Tool effect 必须先完成该入口定义的可见性、参数校验、permission/hook 决策和 invocation resource 绑定，再执行外部副作用与结果持久化。

## 适用行为

适用于 ReactAgent 自动 Tool pipeline。公开 ToolManager 是 caller-owned primitive；用户配置 Hook 与 Skill 是 trusted extension，其 command/http/MCP/Subagent effect 由扩展合同与 embedding application policy 负责。

## 当前实现

PermissionPolicy/Service、Hook/Guard与ToolExecutionPipeline分层影响ReactAgent自动调用；protected-path 由 PermissionService 在 Hook Allow 前按有效输入拒绝，readonly Agent 的构造、LLM surface 与执行门禁共用 ToolCapabilities。ToolManager以owned Arc generation进入stream/non-stream，在effect前完成统一validation，并用context key与write-lifetime epoch限制read result复用，Sandbox/具体Tool持有资源。
Artifact、Sandbox 与 Git worktree 各自持有精确资源身份及清理债务；Agent close 尝试结算保留的 Sandbox executor，普通实例 cleanup 不执行全局 label sweep。

## 期望行为

直接用户交互与 Agent 自动权限路径分离；deny/ask/allow 优先级只有一个解释，修改参数在执行前生效，cleanup 完成后才发布对应终态。

## 证据

Tool/permission contracts、PermissionService、Hook reducer、pipeline 和 sandbox tests 提供部分证据。

## 裁决记录

Stream validation、#60 protected-path 和 #81 readonly custom Tool 已在当前任务分支闭合；其它 Hook source order、deny-first、approval与cleanup冲突仍需审计，且本Rule不扩展为direct-user或trusted-extension权限门控。
本轮 #47 只闭合 Artifact/Sandbox/Worktree 精确清理 owner，不替代剩余 permission 与 trace Finding 的复审。

直接适配复用：validate_tool_input沿同工具schema/customvalidator，在副作用前校验；只有visibility/docs新增，执行语义不归adapter。当前增量证据明确候选范围，历史23不冒新版通过。


本轮限定规则（guard-overlap-redaction）：ContentGuard 在原输入上合并重叠覆盖区间，选择最宽成员标记（同宽按既有detect顺序）；char_indices 单次输出，邻接不重叠区间分别保留。detect的原始byte offsets合同不变，未新增策略/API/SDK parser。 失败/错误合同：原始偏移重叠、跨越、同起点不同长度及中文emoji包围；区间选择与最终文本无重复替换损伤。 guard-red.log保留3项实际输出失败；guard-green.log覆盖完整content模块9 passed/0 failed/0 ignored。core-focused-clippy.log为production lib、guard feature、locked、warnings及unwrap/expect/panic/unreachable拒绝检查通过。owned源码格式与diff检查通过。 只关闭这一故障；其余needs_review与历史证据保持各自范围。


本轮限定规则（data-pivot-value-fidelity）：保留既有Polars grouping/aggregation/join权威；LiteralValue::from(AnyValue)保留原生值、null使用is_null、join使用nulls_equal。header与匹配值分离，冲突在join前返回InvalidParameter。aggregation只有缺省sum，声明外string及null/number/bool类型拒绝。未新增pivot引擎或平行validator。 失败/错误合同：native string/numeric/null分组及值，null index不丢失；含糊header失败关闭；不声明的aggregation不得用成功sum掩盖无效输入。 pivot-red.log和pivot-adjacent-review-red.log分别保留真实3+3失败；pivot-invalid-runtime-red.log只是Tokio current-thread配置错误，明确不是行为反例。data-adjacent-review-green.log完整data模块14 passed/0 failed/0 ignored；tools-adjacent-review-clippy.log通过production约束，前序tools-focused-clippy保留。UTF-8输出、数值/null bucket与absent cell、null index、碰撞、aggregation类型/缺省均有actual-tool测试。 只关闭这一故障；其余needs_review与历史证据保持各自范围。
