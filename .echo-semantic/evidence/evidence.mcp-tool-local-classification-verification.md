---
{
  "schema_version": 1,
  "id": "evidence.mcp-tool-local-classification-verification",
  "kind": "evidence",
  "observed_at": "7110a27e7b0e9841dcfa0d4cca1cdbc053d96177",
  "source_refs": [
    "echo-integration/src/mcp/tool_adapter.rs",
    "echo-orchestration/src/human_loop/service.rs",
    "src/agent/snapshot.rs",
    "src/agent/react/run/pipeline.rs"
  ],
  "supports": [
    "behavior.effect-permission-execution",
    "behavior.extension-publication",
    "rule.permission-effect-order"
  ],
  "limitations": [
    "Current7110 native observation only; original unavailable history, old executions and old reviews are not reconstructed or reexecuted.",
    "Current source/test/limit mapping is in evidence.framework-history-current-reobservation; full05 is existing execution, not a newly run historical command.",
    "Prior SDK source paths moved to the independent accepted SDK product; current SDK runtime/inventory acceptance remains unverified by this F observation."
  ]
}
---

# Current observation and historical record

The machine header now records only the bounded native observation at `7110a27e7b0e9841dcfa0d4cca1cdbc053d96177`; current source/tests and limitations are in `evidence.framework-history-current-reobservation`. Original observation `72d1fccf74b85afe9a74e684ca3748b64642affb` and its complete metadata remain in [the immutable875 document](https://github.com/EchoYue-lp/echo-agent/blob/875cc22f2e99c7ccd83c07e02c03fe221c5e3f6c/.echo-semantic/evidence/evidence.mcp-tool-local-classification-verification.md). The original execution counts, before/after claims and review prose below are preserved verbatim as historical statements, not rerun or equivalence claims for7110.

The former F references `echo-sdk-protocol/tests/facade_inventory.rs` belong to the independent SDK accepted-contract owner. These obligations remain in scope there; the common current evidence gives stable019e source links and the old7f14 execution epoch, while new SDK Host/three-language acceptance is unverified here.


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
