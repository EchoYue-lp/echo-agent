---
schema_version: 1
id: evidence.tool-terminal-observation-verification
kind: evidence
observed_at: b71f03ba16fdbefa0a595b92fe82feee39f8e09e
source_refs:
  - src/agent/react/run/pipeline.rs
  - src/agent/snapshot.rs
  - echo-state/src/audit/mod.rs
  - echo-agent-learning/tests/example_contracts/demo64_tool_pipeline.rs
supports: [finding.tool-terminal-observation-divergence, behavior.effect-permission-execution, rule.permission-effect-order]
limitations:
  - Verification ran on the task branch before merge to main; remote CI must be checked on the pushed final head
  - Evidence records the exact commands and revisions, not a formal equivalence proof
---

# 工具终态观察修复验证

## 支持的结论

在 fix/Echoyue/tool-terminal-observation 分支（base 44b2ed68）上，针对原 PR 的公开配置路径补测，
审计提前成功与三个护栏投影分支均取得 exit 101 红测；修复后执行：

- `cargo fmt --all`（exit 0）
- `cargo test -p echo_agent --lib --locked -- pipeline::`（20 passed, 0 failed）
- `cargo test -p echo_agent --lib --all-features --locked -- pipeline::`（最终 fixture：23 passed, 0 failed）
- demo64 示例同步后，`cargo test -p echo-agent-learning --test example_contracts --all-features --locked contract_demo64_tool_pipeline`（1 passed, 0 failed）
- 最终示例同步后的 `./scripts/verify.sh`（exit 0），包含 fmt check、workspace all-target/all-feature
  Clippy、lib/bins panic-policy Clippy、all-target/all-feature tests 与 no-default library check。
  82 条测试结果汇总：2,796 passed、0 failed、3 ignored；另执行 benchmark test harness，不能称全部测试无 ignored。

关键回归（自真实 caller `execute_tool_with_policy` 驱动）：

- `post_hook_block_after_effect_still_records_full_observations`：PostToolUse `exit 2` 拦截后，
  caller 返回失败终态（PartialSideEffect + postcondition），保留工具输出，trace 记录
  ToolResult(success=false) + ToolError，audit 记录 success=false，`on_tool_error` 恰好触发一次。
- `pre_execution_block_does_not_emit_success_callback`：执行前拦截不伪造成功回调。
- `failed_tool_result_routes_to_on_tool_error_not_on_tool_end`：失败 ToolResult 不再触发 on_tool_end。
- `audit_does_not_restore_output_cleared_by_guard`：成功工具输出被护栏清空后，caller/audit 不恢复原文。
- `failed_tool_output_is_guarded_before_caller_and_audit`：失败工具 stdout/stderr 同样经过输出护栏。
- `error_only_audit_uses_guarded_diagnostic`：无输出错误的审计诊断经护栏，caller 错误契约不变。

全量门禁曾在 PostToolUse fixture 失败（根 crate 901 passed / 1 failed）。独立 all-feature
复现捕获 Hook sandbox execution failed；临时大 stdin 样本稳定复现 stdin/Broken pipe/
cleanup 错误。fixture 改为先消费 hook context 再 exit 2 后同一样本通过，随后删除所有临时
诊断与大样本。最终 23/23 包含真实 caller 的修正后回归，不修改生产 sandbox 错误策略。

## 来源与范围

最终完整日志：Git 状态目录 `supreme/logs/issue102-final-workspace-gate-1789648416381.log`。
合并基准 origin/main@44b2ed68 已核实，最终摘要为本 Evidence 的 observed_at。

实现位于既有 16-stage ToolExecutionPipeline、canonical caller `execute_tool_with_policy` 与
公开 builder `audit_logger` 的既有 AuditStage；回归测试位于 pipeline.rs 测试模块，从真实 caller 入口驱动。

## 覆盖范围

覆盖 Agent 自动工具路径的执行后拦截与失败分流；不覆盖 streaming 多路复用细节与
direct-user surface（excluded by map.tool-permission-sandbox）。

## 已知缺口

on_tool_error 事件粒度以 AuditLogger backend 为准；SDK 侧等价观察合同属于独立 SDK 仓库后续事项。
