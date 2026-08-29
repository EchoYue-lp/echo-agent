# Learning Examples

This directory is the progressive learning path for `echo-agent`. The original
numbered `demo_*.rs` files remain intact and are intentionally small enough to
read in sequence. They use the public `echo_agent` facade and demonstrate one
or more framework capabilities.

Run an example from the `echo-agent` repository root with:

```bash
cargo run -p echo-agent-learning --example demo01_tools --locked
```

Each file contains its own feature and environment requirements. Examples that
need a provider, credential, network, local process, or platform capability
return a clear prerequisite error rather than pretending to be deterministic.

## Numbered Walkthroughs

The current runnable demos are:

- `demo00_quickstart.rs`
- `demo01_tools.rs`
- `demo02_tasks.rs`
- `demo03_approval.rs`
- `demo05_compressor.rs`
- `demo06_mcp.rs`
- `demo07_skills.rs`
- `demo08_external_skills.rs`
- `demo09_file_shell.rs`
- `demo10_streaming.rs`
- `demo11_callbacks.rs`
- `demo13_tool_execution.rs`
- `demo15_structured_output.rs`
- `demo17_chat.rs`
- `demo18_semantic_memory.rs`
- `demo19_guard.rs`
- `demo20_audit.rs`
- `demo23_a2a.rs`
- `demo25_macros.rs`
- `demo26_provider_factory.rs`
- `demo27_sqlite_memory.rs`
- `demo28_workflow.rs`
- `demo29_sandbox.rs`
- `demo32_token_budget.rs`
- `demo33_retry_policy.rs`
- `demo35_dynamic_tools.rs`
- `demo36_multimodal.rs`
- `demo38_im_channels.rs`
- `demo40_snapshot.rs`
- `demo41_web_tools.rs`
- `demo42_playwright_mcp.rs`
- `demo44_code_laboratory.rs`
- `demo45_customer_service.rs`
- `demo46_data_analyst.rs`
- `demo47_enterprise.rs`
- `demo48_personal_assistant.rs`
- `demo49_research_agent.rs`
- `demo56_plugin_system.rs`
- `demo58_git_worktree.rs`
- `demo59_code_search.rs`
- `demo61_agent_factory.rs`
- `demo68_human_gate.rs`
- `demo70_scheduler.rs`

## Comprehensive Examples

New multi-capability walkthroughs belong in this directory and should use the
`comprehensive_*.rs` naming convention. They should explain the scenario,
show the public API composition, and state which steps require external
services. They complement the numbered lessons; they do not replace them.

The checked-in `comprehensive_agent.rs` is a deterministic starting point: it
composes a public-facade tool, a mock model, and the ReAct builder without
credentials or network access.

## Deterministic Contracts

The deterministic `demo_*.rs` contracts that have no standalone `main` function
are under [`../tests/example_contracts/`](../tests/example_contracts/). They are
compiled and run by the shared harness in
[`../tests/example_contracts.rs`](../tests/example_contracts.rs). The contracts
exercise public-facade behavior without becoming a second framework
implementation.

The contract sources are:

- `demo04_subagent.rs`
- `demo12_resilience.rs`
- `demo24_topology.rs`
- `demo30_mcp_server.rs`
- `demo31_memory_tools.rs`
- `demo34_workflow_stream.rs`
- `demo37_declarative_workflow.rs`
- `demo39_workflow.rs`
- `demo43_data_tools.rs`
- `demo50_eval.rs`
- `demo51_self_improvement.rs`
- `demo53_adaptive_compression.rs`
- `demo54_headless.rs`
- `demo55_lsp_tools.rs`
- `demo57_data_pipeline.rs`
- `demo60_data_quality.rs`
- `demo62_prompt_templates.rs`
- `demo64_tool_pipeline.rs`
- `demo65_context_assembler.rs`
- `demo66_context_selector.rs`
- `demo67_progress.rs`

## Fixture Data

The file-based skill fixtures used by `demo08_external_skills.rs` and
`demo47_enterprise.rs` are in [`demo_skills/`](demo_skills/). The examples resolve
this directory from `CARGO_MANIFEST_DIR`, so they work when invoked through
`cargo run -p echo-agent-learning` from the workspace root.

All example code follows the repository safety rules: no `unwrap`/`expect`, no
panic macros, no unchecked UTF-8 slicing, and no `worker` terminology.
