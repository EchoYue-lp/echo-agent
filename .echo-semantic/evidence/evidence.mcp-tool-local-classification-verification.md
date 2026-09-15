---
schema_version: 1
id: evidence.mcp-tool-local-classification-verification
kind: evidence
observed_at: 72d1fccf74b85afe9a74e684ca3748b64642affb
source_refs:
  - echo-integration/src/mcp/tool_adapter.rs
  - echo-orchestration/src/human_loop/service.rs
  - src/agent/snapshot.rs
  - src/agent/react/run/pipeline.rs
  - echo-sdk-protocol/tests/facade_inventory.rs
supports: [behavior.effect-permission-execution, behavior.extension-publication, rule.permission-effect-order]
limitations:
  - 完整 workspace 合并门禁与远端 CI 尚未执行
  - 最终集成分支需统一刷新后续 public trait implementation 和 current_mode inventory identity
---

# MCP Tool 本地分类验证证据

## 支持的结论

失败基线中，伪造 `readOnlyHint=true` 的 adapter 返回 `ToolAccess::ReadOnly`，定向测试以 exit 101
失败。修复后 adapter 六项测试验证 annotations 不改变默认 capability、显式本地降级保持字段一致，
且 transport error 与 `isError=true` 两类失败均按本地 access 结算。

跨层测试验证 `PermissionMode::Plan` 下 MCP-qualified mutating tool 不进入 LLM surface，完整
`execute_tool_with_policy` 在 Execute 前返回 Plan block，执行计数为 0。第二条反例先以 Default
创建 snapshot，再注册 PermissionRequest hook Allow 并直接把 live PermissionService 切换到 Plan；
既有 snapshot 立即隐藏工具并在 hook 前阻断，执行计数仍为 0。

## 来源与范围

- `cargo test -p echo_integration --features mcp --locked`：107 passed，9 doctests passed。
- `cargo test -p echo_agent --features mcp permission_plan_hides_and_blocks_locally_mutating_mcp_tool --locked`：1 passed。
- `cargo test -p echo_agent --features mcp,human-loop live_permission_plan_precedes_hook_allow_for_mutating_mcp_tool --locked`：1 passed。
- `cargo test -p echo_agent --features mcp tool_visibility_combines_skill_plan_and_disabled_policies --locked`：1 passed。
- `cargo test -p echo_execution tool_search_activates_only_eligible_matches --locked`：1 passed。
- `cargo test -p echo_orchestration human_loop::service --locked`：20 passed。
- `cargo check -p echo_agent --no-default-features --features mcp --locked`：passed。
- `cargo clippy -p echo_agent --lib --features mcp,human-loop --locked -- -D warnings`：passed。
- `cargo fmt --all -- --check` 与 `git diff --check`：passed。
- adapter 公共方法生成后 `scripts/check-sdk-contracts.sh`：90 artifacts current；5、29、75 项测试通过。

## 已知缺口

后续 Plan authority 增量新增 `PermissionService::current_mode` 并显式实现 Tool trait capability
方法，最终 integration branch 必须按主代理计划统一重生成 public inventory，再执行完整 MR gate。

## 处理记录

对应 Finding #66；最终行为快照为 `72d1fccf74b85afe9a74e684ca3748b64642affb`。
