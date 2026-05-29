# Examples Classification

This directory is intentionally split into three contract levels.

- `Acceptance example`: should validate behavior locally and fail loudly on regressions.
- `Conditional acceptance example`: still follows fail-fast validation, but depends on external services, feature flags, credentials, local runtimes, or system capabilities.
- `Teaching example`: primarily explains APIs or interaction patterns. It may still contain assertions, but it is not the repository's primary acceptance surface.

Use this file to decide whether an example should be tightened into an acceptance contract or kept readable as a teaching artifact.

## Acceptance Examples

These are the most suitable examples for deterministic local verification.

- `demo16_testing.rs`
- `demo30_mcp_server.rs`
- `demo32_token_budget.rs`
- `demo37_declarative_workflow.rs`

## Conditional Acceptance Examples

These examples are expected to fail loudly when their prerequisites are missing.

- `demo03_approval.rs`
- `demo06_mcp.rs`
- `demo07_skills.rs`
- `demo08_external_skills.rs`
- `demo09_file_shell.rs`
- `demo12_resilience.rs`
- `demo13_tool_execution.rs`
- `demo14_memory_isolation.rs`
- `demo18_semantic_memory.rs`
- `demo26_provider_factory.rs`
- `demo27_sqlite_memory.rs`
- `demo29_sandbox.rs`
- `demo33_retry_policy.rs`
- `demo38_im_channels.rs`
- `demo41_web_tools.rs`
- `demo42_playwright_mcp.rs`
- `demo43_data_tools.rs`
- `demo44_code_laboratory.rs`
- `demo45_customer_service.rs`
- `demo46_data_analyst.rs`
- `demo47_enterprise.rs`
- `demo48_personal_assistant.rs`
- `demo49_research_agent.rs`

Typical prerequisites:

- LLM API keys or provider config
- `npx`, Docker, browsers, or other local runtimes
- feature flags such as `mcp`, `channels`, or `full`
- repository-local assets such as `skills/` or `mcp.json`
- environment-dependent services such as web search, embeddings, or browser automation

## Teaching Examples

These examples stay focused on API understanding, interaction style, or walkthrough value.

- `demo00_quickstart.rs`
- `demo01_tools.rs`
- `demo02_tasks.rs`
- `demo04_subagent.rs`
- `demo05_compressor.rs`
- `demo10_streaming.rs`
- `demo11_callbacks.rs`
- `demo15_structured_output.rs`
- `demo17_chat.rs`
- `demo19_guard.rs`
- `demo20_audit.rs`
- `demo21_handoff.rs`
- `demo22_plan_execute.rs`
- `demo23_a2a.rs`
- `demo24_topology.rs`
- `demo25_macros.rs`
- `demo28_workflow.rs`
- `demo31_memory_tools.rs`
- `demo34_workflow_stream.rs`
- `demo35_dynamic_tools.rs`
- `demo36_multimodal.rs`
- `demo39_workflow.rs`
- `demo40_snapshot.rs`

## Maintenance Rules

- Do not silently skip prerequisites inside `Acceptance example` or `Conditional acceptance example`.
- If an example depends on an external service, fail with a clear prerequisite error instead of returning `Ok(())`.
- If an example is mainly pedagogical, prefer readability over brittle output assertions.
- When a teaching example starts becoming a product-level verification surface, move it into one of the acceptance buckets and tighten its checks accordingly.

## Human-in-Loop Notes

- `demo03_approval.rs`: use this when you want to validate both proactive `human_in_loop` input requests and sensitive-tool approval.
- `demo05_compressor.rs`: use this when you want to validate the real `add_need_appeal_tool()` path inside a broader workflow example.

## Naming Convention

All examples follow the `demo{NN}_{feature}.rs` pattern:
- `NN` — two-digit sequential number (00-99)
- `feature` — snake_case description of the demonstrated feature

Comprehensive scenario examples (demo44-demo49) combine multiple features into real-world use cases.
