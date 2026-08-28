# Framework Example Dispositions

The framework owns 64 example scenarios. They are now split between 43 root
Cargo examples and 21 executable integration-test contracts. This file is the
machine-checked disposition manifest: every numbered scenario source must
appear exactly once below. `tests/documentation_contract.rs` enforces the
mapping, source safety rules, and Cargo feature boundaries.

All root examples use the `echo_agent` facade. The non-published
[`echo-agent-examples`](../echo-agent-examples/) package remains a deliberately
small facade-only consumer probe whose only dependency is `echo_agent`. Moving
the 15 composition demos into that package would require duplicating Tokio,
Serde, tempfile, and other demo runtime dependencies, weakening that boundary;
they are therefore retained as root feature-composition examples.

Rust language lessons remain in
[`echo-rust-learning`](../echo-rust-learning/README.md).

## Root Composition And Teaching

These 29 sources remain Cargo examples. `demo13`, `demo32`, and `demo35` remain
here because their current entrypoints include live-provider execution and are
not wholly deterministic contracts.

- `demo00_quickstart.rs`
- `demo01_tools.rs`
- `demo02_tasks.rs`
- `demo05_compressor.rs`
- `demo10_streaming.rs`
- `demo11_callbacks.rs`
- `demo13_tool_execution.rs`
- `demo15_structured_output.rs`
- `demo17_chat.rs`
- `demo19_guard.rs`
- `demo20_audit.rs`
- `demo23_a2a.rs`
- `demo25_macros.rs`
- `demo26_provider_factory.rs`
- `demo28_workflow.rs`
- `demo32_token_budget.rs`
- `demo35_dynamic_tools.rs`
- `demo38_im_channels.rs`
- `demo40_snapshot.rs`
- `demo44_code_laboratory.rs`
- `demo45_customer_service.rs`
- `demo46_data_analyst.rs`
- `demo47_enterprise.rs`
- `demo48_personal_assistant.rs`
- `demo49_research_agent.rs`
- `demo56_plugin_system.rs`
- `demo61_agent_factory.rs`
- `demo68_human_gate.rs`
- `demo70_scheduler.rs`

## Executable Contract Tests

These 21 scenarios live under `tests/example_contracts/` and are loaded by
`tests/example_contracts.rs`. Their former root source paths were removed; no
wrapper or duplicate implementation remains.

- `tests/example_contracts/demo04_subagent.rs`
- `tests/example_contracts/demo12_resilience.rs`
- `tests/example_contracts/demo24_topology.rs`
- `tests/example_contracts/demo30_mcp_server.rs`
- `tests/example_contracts/demo31_memory_tools.rs`
- `tests/example_contracts/demo34_workflow_stream.rs`
- `tests/example_contracts/demo37_declarative_workflow.rs`
- `tests/example_contracts/demo39_workflow.rs`
- `tests/example_contracts/demo43_data_tools.rs`
- `tests/example_contracts/demo50_eval.rs`
- `tests/example_contracts/demo51_self_improvement.rs`
- `tests/example_contracts/demo53_adaptive_compression.rs`
- `tests/example_contracts/demo54_headless.rs`
- `tests/example_contracts/demo55_lsp_tools.rs`
- `tests/example_contracts/demo57_data_pipeline.rs`
- `tests/example_contracts/demo60_data_quality.rs`
- `tests/example_contracts/demo62_prompt_templates.rs`
- `tests/example_contracts/demo64_tool_pipeline.rs`
- `tests/example_contracts/demo65_context_assembler.rs`
- `tests/example_contracts/demo66_context_selector.rs`
- `tests/example_contracts/demo67_progress.rs`

## Conditional

These examples require credentials, network access, local processes, browser or
git capabilities, SQLite, or another explicit feature/runtime prerequisite.
They must fail with a clear prerequisite error instead of reporting a skipped
path as success.

- `demo03_approval.rs`
- `demo06_mcp.rs`
- `demo07_skills.rs`
- `demo08_external_skills.rs`
- `demo09_file_shell.rs`
- `demo18_semantic_memory.rs`
- `demo27_sqlite_memory.rs`
- `demo29_sandbox.rs`
- `demo33_retry_policy.rs`
- `demo36_multimodal.rs`
- `demo41_web_tools.rs`
- `demo42_playwright_mcp.rs`
- `demo58_git_worktree.rs`
- `demo59_code_search.rs`

## Maintenance Rules

- Example code must not use `unwrap`, `expect`, panic macros, unchecked string
  byte slicing, or byte length as a character count.
- Feature requirements in `Cargo.toml` must match code paths. Missing external
  prerequisites are errors, not successful skips.
- Root composition code only uses the `echo_agent` facade.
- Contract tests use deterministic fixtures, explicit failure assertions, and
  feature-gated modules in the shared integration-test harness.
- Framework SQLite examples remain valid framework capabilities. The EKO
  application's no-SQLite decision does not remove them.

All files follow `demo{NN}_{feature}.rs`, where `NN` is a two-digit sequence and
`feature` is a snake-case description.
