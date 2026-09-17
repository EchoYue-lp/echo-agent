---
schema_version: 1
id: evidence.tool-terminal-observation-verification
kind: evidence
observed_at: source:469a276a3666fa7b9f836bc4c5751516ca360fcdb16efae6cc81c2d66ffb2560
source_refs:
  - src/agent/react/run/pipeline.rs
  - src/agent/snapshot.rs
  - echo-state/src/audit/mod.rs
supports: [finding.tool-terminal-observation-divergence, behavior.effect-permission-execution, rule.permission-effect-order]
limitations:
  - Verification ran on the task branch before merge to main; full merge gate is executed per AGENTS.md before squash merge
  - Evidence records the exact commands and revisions, not a formal equivalence proof
---

# 工具终态观察修复验证

## 支持的结论

在 fix/Echoyue/tool-terminal-observation 分支（base 44b2ed68）上执行以下命令并全部通过：

- `cargo fmt --all` && `cargo fmt --all -- --check`（exit 0）
- `cargo test -p echo_agent --lib --locked -- pipeline::`（17 passed, 0 failed）
- `cargo test -p echo_state --lib --locked -- audit`（passed）
- `cargo clippy -p echo_agent -p echo_state --lib --locked -- -D warnings`（exit 0）

关键回归（自真实 caller `execute_tool_with_policy` 驱动）：

- `post_hook_block_after_effect_still_records_full_observations`：PostToolUse `exit 2` 拦截后，
  caller 返回失败终态（PartialSideEffect + postcondition），保留工具输出，trace 记录
  ToolResult(success=false) + ToolError，audit 记录 success=false，`on_tool_error` 恰好触发一次。
- `pre_execution_block_does_not_emit_success_callback`：执行前拦截不伪造成功回调。
- `failed_tool_result_routes_to_on_tool_error_not_on_tool_end`：失败 ToolResult 不再触发 on_tool_end。

## 来源与范围

实现位于既有 16-stage ToolExecutionPipeline、canonical caller `execute_tool_with_policy` 与
AuditCallback 既有通道；回归测试位于 pipeline.rs 测试模块，从真实 caller 入口驱动。

## 覆盖范围

覆盖 Agent 自动工具路径的执行后拦截与失败分流；不覆盖 streaming 多路复用细节与
direct-user surface（excluded by map.tool-permission-sandbox）。

## 已知缺口

on_tool_error 事件粒度以 AuditLogger backend 为准；SDK 侧等价观察合同属于独立 SDK 仓库后续事项。
