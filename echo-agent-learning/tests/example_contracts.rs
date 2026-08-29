//! Executable contracts migrated from deterministic framework examples.

#[cfg(all(feature = "testing", feature = "human-loop"))]
#[path = "example_contracts/demo04_subagent.rs"]
mod demo04_subagent;
#[cfg(feature = "testing")]
#[path = "example_contracts/demo12_resilience.rs"]
mod demo12_resilience;
#[cfg(feature = "topology")]
#[path = "example_contracts/demo24_topology.rs"]
mod demo24_topology;
#[cfg(feature = "mcp")]
#[path = "example_contracts/demo30_mcp_server.rs"]
mod demo30_mcp_server;
#[path = "example_contracts/demo31_memory_tools.rs"]
mod demo31_memory_tools;
#[path = "example_contracts/demo34_workflow_stream.rs"]
mod demo34_workflow_stream;
#[path = "example_contracts/demo37_declarative_workflow.rs"]
mod demo37_declarative_workflow;
#[cfg(feature = "testing")]
#[path = "example_contracts/demo39_workflow.rs"]
mod demo39_workflow;
#[cfg(all(feature = "testing", feature = "data", feature = "media"))]
#[path = "example_contracts/demo43_data_tools.rs"]
mod demo43_data_tools;
#[cfg(feature = "eval")]
#[path = "example_contracts/demo50_eval.rs"]
mod demo50_eval;
#[cfg(all(feature = "eval", feature = "improve"))]
#[path = "example_contracts/demo51_self_improvement.rs"]
mod demo51_self_improvement;
#[path = "example_contracts/demo53_adaptive_compression.rs"]
mod demo53_adaptive_compression;
#[path = "example_contracts/demo54_headless.rs"]
mod demo54_headless;
#[cfg(feature = "lsp")]
#[path = "example_contracts/demo55_lsp_tools.rs"]
mod demo55_lsp_tools;
#[cfg(feature = "testing")]
#[path = "example_contracts/demo57_data_pipeline.rs"]
mod demo57_data_pipeline;
#[cfg(all(feature = "data", feature = "statistics"))]
#[path = "example_contracts/demo60_data_quality.rs"]
mod demo60_data_quality;
#[path = "example_contracts/demo62_prompt_templates.rs"]
mod demo62_prompt_templates;
#[cfg(feature = "testing")]
#[path = "example_contracts/demo64_tool_pipeline.rs"]
mod demo64_tool_pipeline;
#[path = "example_contracts/demo65_context_assembler.rs"]
mod demo65_context_assembler;
#[path = "example_contracts/demo66_context_selector.rs"]
mod demo66_context_selector;
#[path = "example_contracts/demo67_progress.rs"]
mod demo67_progress;
