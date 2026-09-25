---
schema_version: 1
id: evidence.guard-direction-contract-repair
kind: evidence
observed_at: source:bd676c53ff6a431d3ef88581ad7a92a4db718dad7e88dd872b64e20170f61b93
source_refs:
  - echo-core/src/guard/mod.rs
  - src/agent/react/run/pipeline.rs
  - src/agent/react/run/phases/think.rs
  - src/agent/react/run/phases/tools.rs
  - src/agent/react/run/phases/finalize.rs
  - src/agent/snapshot.rs
  - docs/adr/0076-guard-direction-and-boundary-authority.md
supports: [finding.guard-direction-contract, behavior.effect-permission-execution, rule.permission-effect-order]
limitations:
  - 本证据只覆盖 framework Guard 四方向的生产可达性，不改变 EKO 应用策略、Hook 或 PermissionService 的权威
  - final_answer 工具结果已作为 ToolOutput 进入 transcript，不再二次用 Output 检查；Output 专属模型文本终态
  - 完整 workspace 合并门禁、远端 CI、main 交付与 Issue #57 关闭尚未完成
---

# Issue 57 Guard direction 修复证据

## 支持的结论

`GuardManager::check_all` 不再把单个 Guard 的 Err 转为 Warn，而是传播错误供
Input、ToolInput、ToolOutput 和 Output 边界 fail-closed。统一 Tool pipeline 在所有
intervention、hook 和 PermissionRequest 参数重写完成后、Invocation/CallbackStart 前，
以最终 `ctx.input` 的 JSON 检查 ToolInput；该边界不允许 Transform，从而保持
approval receipt 与真正执行的参数相同。ToolOutput 使用专属方向；模型文本答案在
写入 Context、回调、持久化、Trace 和 Token/FinalAnswer event 前检查 Output。
provider 失败路径上的部分文本在发送 Token 前也经 Output guard；工具
`final_answer` 结果不重复检查 Output。
未配置 GuardManager 时最终文本保持原样，不额外启用默认 secret redaction 策略。

## 来源与范围

配置 GuardManager 的 streaming tool 不再把未受检 Output/Progress 事件送到
caller；完整 ToolResult 经 ToolOutput Guard 和输出预算后作为唯一对外终态，
不伪造 stdout/stderr 合并通道。无 GuardManager 的 streaming tool 仍实时发送
stdout/stderr。无输出失败的诊断在 OutputGuardStage 被受检值替换于
`ToolResult.error`，后续 caller、ToolError Trace、Audit、callback 与 transcript
从同一值读取；typed failure 与 confirmed effects 未被重写。

后续复审的两个反例由真实 caller 红测复现：文本被替换后 `ToolResult.data` 和
`model_content` 原样返回；失败结果同时含非空 output 与 error 时，只检查 output，
Trace/callback/caller 仍见原始 error。OutputGuardStage 现在分别检查两段文本；
规范 JSON data、非空 metadata、携带内容的 kind 和 MIME 分别检查；Pass 保留已检
结构，任一被替换则撤销无法无损重建的平行投影。配置 Guard 时始终抑制不透明图片
富内容和护栏前 artifact；未配置时保持原有富内容。ToolFailure 和 ToolEffect
执行事实保留。
`publish_completed_call` 的真实 Context 路径证明受阻图像不会重新注入模型。
PostToolUse/Failure 是用户安装的可信 hook，仍在 Guard 前见原文；本修复不改变
#102 hook/terminal 结算顺序，ADR 0076 明记此边界。

PostToolUse block 可把原始工具输出复制进 `ctx.block_reason`，caller failure 与
skill telemetry 曾优先使用这份 raw reason。现在 blocked reason 与受检的
`ToolResult.error` 归一，`ctx.block_failure` 也同步受检 failure。
`ToolFailure.idempotency_key`、`postcondition` 的自由文本分别经 ToolOutput Guard：
key 一旦改变即撤销以免伪造重试身份，postcondition 使用受检文本；
category/recovery/side_effect 保留。已确认的 typed ToolEffect/path 依 ADR 0074
继续在 caller/Trace 作为恢复和诊断事实，Guard 不承诺对这些字段通用脱敏。

ADR 0076 记录候选方案、边界权威、双检取舍和影响；中英文 Guard 正式文档同步
描述四个方向、block-only 与错误策略。

## 已知缺口

独立复审已通过；完整 workspace/feature 门禁、远端 CI、main 交付与
Issue #57 关闭仍待整合阶段，不声明已完成远端交付。
