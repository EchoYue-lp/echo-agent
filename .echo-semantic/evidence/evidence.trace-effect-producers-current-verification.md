---
schema_version: 1
id: evidence.trace-effect-producers-current-verification
kind: evidence
observed_at: source:6ed43c02230186db2c60d15eeda68864a0c1dddb9719434c45439363536592e4
source_refs:
  - echo-tools/src/files/files.rs
  - echo-tools/src/files/apply_patch.rs
  - src/eval/runner.rs
  - src/eval/replay.rs
  - src/eval/regression.rs
  - src/trace/analyzer.rs
  - src/improve/analyzer.rs
  - src/improve/trajectory.rs
  - echo-agent-learning/tests/example_contracts/demo50_eval.rs
  - echo-agent-learning/tests/example_contracts/demo51_self_improvement.rs
  - src/agent/subagent/executor.rs
  - src/tools/builtin/agent_dispatch.rs
  - src/agent/react/run/pipeline.rs
  - src/agent/react/run/phases/tools.rs
  - src/trace/mod.rs
  - docs/en/27-tracing.md
  - docs/zh/27-tracing.md
  - docs/adr/0059-observed-tool-effects-and-background-dispatch.md
supports: [finding.trace-effect-event-producers]
limitations:
  - The focused contract test covers all 17 serialized variants and the producer matrix is documentation evidence; producer integration tests remain in their owning modules
  - Full workspace gates, final source digest, independent rereview, and cross-repository SDK/CLI consumer gates remain separate
command_results:
  - { command: "cargo test -p echo_agent run_event_contract_matrix_covers_all_variants --lib --locked", exit_code: 0, tests: "1 passed; 0 failed" }
  - { command: "cargo test -p echo_agent typed_tool_effects_are_projected_without_name_or_output_inference --lib --locked", exit_code: 0, tests: "1 passed; 0 failed" }
  - { command: "cargo test -p echo_agent permission_hook_decisions_are_traced_before_execution --lib --locked", exit_code: 0, tests: "1 passed; 0 failed" }
  - { command: "cargo test -p echo_agent confirmed_effects_survive_stalled_post_use_stage --lib --locked", exit_code: 0, tests: "1 passed; 0 failed" }
  - { command: "cargo test -p echo_agent --features eval test_criteria_append_only_completed_commands_to_the_correlated_trace --lib --locked", exit_code: 0, tests: "1 passed; 0 failed" }
  - { command: "cargo test -p echo_agent --features subagent background_effect_settles_once_after_start_for_success_failure_and_cancel --lib --locked", exit_code: 0, tests: "1 passed; 0 failed" }
  - { command: "cargo fmt --all -- --check", exit_code: 0 }
  - { command: "cargo clippy -p echo_agent --lib --locked -- -D warnings", exit_code: 0 }
  - { command: "cargo clippy -p echo_agent --lib --locked -- -D clippy::unwrap_used -D clippy::expect_used -D clippy::panic -D clippy::unreachable", exit_code: 0 }
  - { command: "git diff --check", exit_code: 0 }
---

# Issue 104 verification frontier

## 支持的结论

`run_event_contract_matrix_covers_all_variants` 对 17 个 `RunEvent` discriminator 执行
exhaustive match 与 JSON 序列化断言。现有 producer-focused tests 覆盖文件真实变更/无变更、
partial patch/dry-run、Eval TestPass/SWE-bench 实际命令与 126/127、typed effect 前于 stalled
post-use stage、SubagentExecutor 的真实 terminal 分类、后台 Subagent 反序完成与父 trace 关联、
timeout/cancel 后调用级终态数量，以及 Trace/Eval/Improvement analyzer、Replay/Regression 不把
skipped call 计作执行而 trajectory 仍保留 admitted pair。最终源码摘要上的 focused 命令结果由
本文件的 `command_results` 和独立复审补充；Finding 在主线交付前保持 open。

## 来源与范围

所列源码包含 17 variant contract、effect producer、projection、replay/eval/improvement consumer、
demo50/demo51 可执行示例与相关回归入口；完整工程命令见
`evidence.observation-current-final-gates`。

## 已知缺口

上述 focused 命令均在当前候选源码上通过；独立复审、完整 workspace gate 与 SDK 消费者仍未有通过收据。
