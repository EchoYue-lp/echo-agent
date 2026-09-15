//! Canonical, executable facade adapter catalog (plan 07, todo 1).
//!
//! The parity manifest classifies every public root-facade item, but until
//! now the adapter obligation was derived from facade-path heuristics and
//! collapsed into wildcard routes (`_echo_agent/task/*`,
//! `_echo_agent/agent/*`, …). That made the manifest descriptive but not
//! executable: the Host could not route on it and aliases could silently
//! drift into second handlers.
//!
//! This module replaces those heuristics with one canonical route table:
//!
//! - every facade item resolves to **exactly one** [`CanonicalRoute`] derived
//!   from its canonical *source identity* (the rustdoc source path of the
//!   defining item), never from the re-export facade path;
//! - re-export aliases (prelude, `advanced`, module re-exports) share the
//!   source identity and therefore share one route and one handler — the
//!   manifest marks them `alias_of` the canonical member;
//! - [`FACADE_FAMILIES`] is the closed family table: family → wire methods,
//!   capability, required root leaf feature, canonical source prefixes and
//!   the real validation references. [`validate_facade_route_table`] checks
//!   it against [`crate::catalog::METHOD_CATALOG`] mechanically: no
//!   wildcards, no dangling methods, every catalog method owned by exactly
//!   one family;
//! - [`build_facade_operation_catalog`] renders the generated
//!   `contracts/sdk/facade-operation-catalog.json` artifact from the parity
//!   manifest plus this table, so route drift between manifest, method
//!   catalog and generated contracts is a blocking generation failure.
//!
//! Granularity contract: route ids are family-level for designed families
//! (`family:memory`, `core:task`). Per-operation discriminants inside a
//! family method are frozen with the family handlers. Items outside those
//! families retain their exact source identity on `_echo_agent/facade/invoke`
//! (`source:<source-path>`), never a wildcard.

use std::collections::BTreeMap;

use crate::capability::ExtensionCapability;
use crate::catalog::METHOD_CATALOG;
use crate::handle::HandleKind;
use crate::inventory::{AcpRelationship, InventoryEntry, ItemKind, SemanticClass};
use crate::methods::ExtensionKind;

/// Digest of one frozen family-operation wire envelope. Deterministic from
/// the family and operation identity alone: the generated catalog embeds it
/// and the Host admission compares a request's `signature_digest` against it
/// exactly, so an outdated or hand-built envelope fails closed.
pub fn family_operation_signature_digest(family: &str, operation: &str) -> String {
    use sha2::{Digest as _, Sha256};
    let canonical = serde_json::json!({ "family": family, "operation": operation });
    let encoded = serde_json::to_vec(&canonical).unwrap_or_default();
    format!("sha256:{:x}", Sha256::digest(&encoded))
}

/// Closed set of facade adapter families. A family is either *source-routed*
/// (facade items reach it through canonical source prefixes) or
/// *protocol-native* (its wire surface is defined by the SDK profile itself:
/// the handle lifecycle methods whose DTOs are the contract).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FacadeFamily {
    // ── Protocol-native core families ──────────────────────────────────
    AgentLifecycle,
    Session,
    Run,
    EventReplay,
    StructuredOutput,
    Task,
    Subagent,
    // ── Stateful feature families (plan 07 todo 4) ─────────────────────
    Memory,
    Workflow,
    State,
    Delivery,
    Trace,
    Eval,
    Improve,
    Permission,
    // ── Integration families (plan 07 todo 5) ──────────────────────────
    Mcp,
    A2a,
    Lsp,
    Channels,
    Telemetry,
    Topology,
    // ── Tool families (plan 07 todo 5) ─────────────────────────────────
    Web,
    Files,
    Shell,
    Git,
    Database,
    Rag,
    Chart,
    Media,
    Data,
    Statistics,
    Research,
    ContentGuard,
    ProjectRules,
    Testing,
    /// Canonical source-operation route for public operations that are not
    /// covered by a typed family adapter. It remains an explicit executable
    /// route in the catalog; the Host must bind it to a source adapter or
    /// report a typed framework error, never silently downgrade it to an
    /// intrinsic item.
    SourceOperation,
    // ── Generic surfaces (not handler families) ────────────────────────
    /// Generic manifest-identified invocation surface (`facade/invoke`).
    Invoke,
    /// Serializable value surface carried by the extension schema.
    Value,
    /// Reverse extension bridge for consumer-implemented traits.
    Bridge,
    /// Stable ACP v1 projection surface.
    Standard,
    /// Process-local Rust mechanism; never crosses the wire.
    Intrinsic,
}

impl FacadeFamily {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AgentLifecycle => "agent_lifecycle",
            Self::Session => "session",
            Self::Run => "run",
            Self::EventReplay => "event_replay",
            Self::StructuredOutput => "structured_output",
            Self::Task => "task",
            Self::Subagent => "subagent",
            Self::Memory => "memory",
            Self::Workflow => "workflow",
            Self::State => "state",
            Self::Delivery => "delivery",
            Self::Trace => "trace",
            Self::Eval => "eval",
            Self::Improve => "improve",
            Self::Permission => "permission",
            Self::Mcp => "mcp",
            Self::A2a => "a2a",
            Self::Lsp => "lsp",
            Self::Channels => "channels",
            Self::Telemetry => "telemetry",
            Self::Topology => "topology",
            Self::Web => "web",
            Self::Files => "files",
            Self::Shell => "shell",
            Self::Git => "git",
            Self::Database => "database",
            Self::Rag => "rag",
            Self::Chart => "chart",
            Self::Media => "media",
            Self::Data => "data",
            Self::Statistics => "statistics",
            Self::Research => "research",
            Self::ContentGuard => "content_guard",
            Self::ProjectRules => "project_rules",
            Self::Testing => "testing",
            Self::SourceOperation => "source_operation",
            Self::Invoke => "invoke",
            Self::Value => "value",
            Self::Bridge => "bridge",
            Self::Standard => "standard",
            Self::Intrinsic => "intrinsic",
        }
    }

    /// Whether facade items reach this family through canonical source
    /// prefixes. Protocol-native families (agent/session/run handle
    /// lifecycle) and fallback surfaces (invoke/value/bridge/standard/
    /// intrinsic) are reached by classification instead of prefixes.
    pub fn is_source_routed(&self) -> bool {
        !matches!(
            self,
            Self::AgentLifecycle
                | Self::Session
                | Self::Run
                | Self::StructuredOutput
                | Self::Invoke
                | Self::Value
                | Self::Bridge
                | Self::Standard
                | Self::Intrinsic
                | Self::SourceOperation
                | Self::Testing
        )
    }

    fn descriptor(&self) -> &'static FamilyDescriptor {
        FACADE_FAMILIES
            .iter()
            .find(|family| family.family == *self)
            .unwrap_or(&FALLBACK_DESCRIPTOR)
    }

    pub fn methods(&self) -> &'static [&'static str] {
        self.descriptor().methods
    }

    pub fn capability(&self) -> ExtensionCapability {
        self.descriptor().capability
    }

    /// Root `echo_agent` leaf feature that gates this family, or `None` for
    /// always-compiled core surfaces.
    pub fn required_feature(&self) -> Option<&'static str> {
        self.descriptor().required_feature
    }

    pub fn validation(&self) -> &'static [&'static str] {
        self.descriptor().validation
    }

    /// Exact operation identities implemented by a family adapter. Keeping
    /// this list beside the canonical family table lets the generated
    /// catalog reject an operation before it reaches a handwritten match.
    pub fn operations(&self) -> &'static [&'static str] {
        match self {
            Self::Memory => &[
                "memory.store.put",
                "memory.store.get",
                "memory.store.delete",
                "memory.store.list",
                "memory.store.search",
                "memory.resource.put",
                "memory.resource.get",
                "memory.resource.search",
                "memory.resource.search_with",
                "memory.resource.delete",
                "memory.resource.list_namespaces",
                "memory.resource.list",
                "memory.resource.prune_expired",
                "memory.resource.dedup_by_content",
            ],
            Self::Workflow => &[
                "workflow.graph.build",
                "workflow.graph.run",
                "workflow.graph.run_stream",
                "workflow.stream.next",
                "workflow.stream.cancel",
                "workflow.stream.close",
                "workflow.graph.run_until_interrupt",
                "workflow.graph.resume",
                "workflow.graph.resume_exact",
                "workflow.graph.resume_with_state",
                "workflow.graph.branch",
                "workflow.graph.tag_checkpoint",
                "workflow.graph.list_checkpoints",
                "workflow.graph.list_checkpoints_by_graph",
                "workflow.graph.load_checkpoint",
                "workflow.graph.restore",
                "workflow.graph.cancel",
                "workflow.state.new",
                "workflow.state.get",
                "workflow.state.set",
                "workflow.state.keys",
                "workflow.state.snapshot",
                "workflow.extension.run",
                "workflow.extension.run_stream",
            ],
            Self::State => &[
                "state.checkpoint.get",
                "state.checkpoint.save",
                "state.runtime.list",
                "state.runtime.clear",
                "state.scope.clear",
            ],
            Self::Delivery => &[
                "delivery.ledger.open",
                "delivery.enqueue",
                "delivery.claim_next",
                "delivery.transition",
                "delivery.defer",
                "delivery.settle",
                "delivery.recover",
                "delivery.snapshot",
            ],
            Self::Trace => &[
                "trace.store.open",
                "trace.run.save",
                "trace.run.load",
                "trace.run.list_session",
                "trace.run.list_recent",
            ],
            Self::Eval => &["eval.constraints.run", "eval.report.build"],
            Self::Improve => &["improve.trajectory.sharegpt", "improve.run.analyze"],
            Self::Permission => &[
                "permission.mode",
                "permission.set_mode",
                "permission.check",
                "permission.apply_update",
                "permission.apply_updates",
                "permission.check_with_permissions",
                "permission.check_with_permissions_in_mode",
                "permission.check_with_permissions_result_in_mode",
                "permission.check_with_permissions_result_in_mode_and_context",
                "permission.add_rule",
                "permission.add_rules",
                "permission.remove_rule",
                "permission.clear_rules",
                "permission.all_rules",
                "permission.is_approved",
                "permission.record_approval",
                "permission.cleanup_expired",
                "permission.stats",
                "permission.revoke_cache",
                "permission.clear_cache",
                "permission.would_request_human",
            ],
            Self::Mcp => &[
                "mcp.manager.open",
                "mcp.manager.open_exact",
                "mcp.client.open_exact",
                "mcp.server.connect",
                "mcp.server.connect_exact",
                "mcp.server.list",
                "mcp.server.disconnect",
                "mcp.manager.close_all",
                "mcp.manager.connect_from_config",
                "mcp.manager.get_all_tools",
                "mcp.manager.get_client",
                "mcp.manager.get_clients",
                "mcp.manager.reconcile_target",
                "mcp.manager.resource_tools",
                "mcp.manager.server_names",
                "mcp.tool.execute",
                "echo_agent::mcp::McpClient::call_tool",
                "echo_agent::mcp::McpClient::close",
                "echo_agent::mcp::McpClient::get_prompt",
                "echo_agent::mcp::McpClient::list_resource_templates",
                "echo_agent::mcp::McpClient::list_resources",
                "echo_agent::mcp::McpClient::ping",
                "echo_agent::mcp::McpClient::prompts",
                "echo_agent::mcp::McpClient::protocol_version",
                "echo_agent::mcp::McpClient::read_resource",
                "echo_agent::mcp::McpClient::resources",
                "echo_agent::mcp::McpClient::server_capabilities",
                "echo_agent::mcp::McpClient::server_name",
                "echo_agent::mcp::McpClient::supports_prompts",
                "echo_agent::mcp::McpClient::supports_resources",
                "echo_agent::mcp::McpClient::tools",
            ],
            Self::A2a => &[
                "a2a.client.open",
                "a2a.discover",
                "a2a.task.send",
                "a2a.task.get",
                "a2a.task.cancel",
                "a2a.task.stream.open",
                "a2a.stream.next",
                "a2a.stream.cancel",
                "a2a.stream.close",
            ],
            Self::Lsp => &[
                "lsp.manager.open",
                "lsp.manager.open_exact",
                "lsp.manager.load_config",
                "lsp.manager.set_project_root",
                "lsp.manager.start_server",
                "lsp.manager.stop_server",
                "lsp.manager.restart_server",
                "lsp.manager.shutdown_all",
                "lsp.manager.get_client",
                "lsp.manager.get_client_for_file",
                "lsp.server.status",
                "lsp.language.list",
                "lsp.language.configured",
                "lsp.server.running",
                "lsp.server.status_all",
                "lsp.client.language",
                "lsp.client.is_running",
                "lsp.client.is_initialized",
                "lsp.client.initialize",
                "lsp.client.shutdown",
                "lsp.client.diagnostics",
                "lsp.client.goto_definition",
                "lsp.client.find_references",
                "lsp.client.hover",
                "lsp.client.completion",
                "lsp.client.did_open",
                "lsp.client.did_change",
                "lsp.client.did_save",
                "lsp.client.did_close",
                "lsp.client.status",
            ],
            Self::Channels => &[
                "channels.manager.open",
                "channels.plugin.register",
                "channels.manager.start",
                "channels.manager.stop",
                "channels.manager.health",
                "channels.manager.list",
                "channels.manager.ids",
                "channels.manager.stop_exact",
                "channels.manager.stop_all",
                "channels.plugin.send",
            ],
            Self::Topology => &[
                "topology.tracker.open",
                "topology.tracker.open_exact",
                "topology.node.add",
                "topology.node.add_exact",
                "topology.call.record",
                "topology.call.record_with_duration",
                "topology.snapshot",
                "topology.nodes",
                "topology.edges",
                "topology.stats",
                "topology.clear",
                "topology.to_mermaid",
                "topology.to_json",
                "topology.to_dot",
            ],
            Self::Telemetry => &["telemetry.init", "telemetry.status", "telemetry.shutdown"],
            Self::ContentGuard => &[
                "content-guard.detect",
                "content-guard.redact",
                "content-guard.is_clean",
                "content-guard.check",
                "content-guard.detect_exact",
                "content-guard.redact_exact",
                "content-guard.is_clean_exact",
            ],
            Self::ProjectRules => &["project-rules.resolve", "project-rules.load"],
            Self::Web => &["web_fetch", "web_extract", "web_search"],
            Self::Files => &[
                "read_file",
                "list_dir",
                "grep",
                "glob",
                "apply_patch",
                "diff",
                "repo_map",
                "code_search",
                "create_file",
                "delete_file",
                "write_file",
                "append_file",
                "update_file",
                "move_file",
            ],
            Self::Shell => &[
                "shell",
                "sandbox.extension.run_stream",
                "sandbox.stream.next",
                "sandbox.stream.cancel",
                "sandbox.stream.close",
            ],
            Self::Git => &[
                "git_status",
                "git_diff",
                "git_log",
                "git_blame",
                "git_branch",
                "git_commit",
                "enter_worktree",
                "exit_worktree",
                "list_worktrees",
            ],
            Self::Database => &["sql_query", "list_tables", "describe_table"],
            Self::Rag => &["rag_chunk_document", "rag_index", "rag_search"],
            Self::Chart => &["generate_chart"],
            Self::Media => &[
                "view_image",
                "pdf_extract",
                "pdf_info",
                "read_excel",
                "excel_info",
                "excel_to_csv",
                "excel_profile",
                "excel_write",
                "read_word",
                "word_info",
                "word_structure",
                "search_text",
                "text_stats",
                "process_text",
                "export_text",
                "image_fetch",
            ],
            Self::Data => &[
                "read_data",
                "filter_data",
                "aggregate_data",
                "data_stats",
                "transform_data",
                "export_data",
                "profile_data",
                "topn_data",
                "contribution_data",
                "bin_data",
                "ratio_data",
                "multi_read_data",
                "join_data",
                "correlate_data",
                "pivot_data",
                "missing_value_analysis",
                "outlier_detection",
                "consistency_check",
            ],
            Self::Statistics => &["exploratory_statistics"],
            Self::Research => &[
                "arxiv_search",
                "semantic_scholar_search",
                "pubmed_search",
                "clinical_trials_search",
                "pdf_fetch",
                "bibtex_generate",
            ],
            Self::Invoke => &["facade.resource.close"],
            _ => &[],
        }
    }
}

/// One closed family declaration.
pub struct FamilyDescriptor {
    pub family: FacadeFamily,
    /// Wire methods owned by this family (must exist in `METHOD_CATALOG`).
    pub methods: &'static [&'static str],
    pub capability: ExtensionCapability,
    /// Root leaf feature gating the family (`None` = always compiled).
    pub required_feature: Option<&'static str>,
    /// Canonical rustdoc source prefixes routing items into this family.
    /// Longest prefix wins; empty for protocol-native families.
    pub source_prefixes: &'static [&'static str],
    /// Real validation references (tests that exercise the family surface).
    pub validation: &'static [&'static str],
}

const CORE_VALIDATION: &[&str] = &[
    "echo-sdk-protocol/tests/core_rpc_contract.rs",
    "echo-sdk-host/tests/core_profile_e2e.rs",
];

const BRIDGE_VALIDATION: &[&str] = &[
    "echo-sdk-protocol/tests/extension_contract.rs",
    "echo-sdk-host/tests/extension_bridge_e2e.rs",
];

const FAMILY_VALIDATION: &[&str] = &[
    "echo-sdk-protocol/tests/facade_inventory.rs",
    "echo-sdk-protocol/tests/core_rpc_contract.rs",
];

/// Placeholder for lookups of families not present in the table; caught by
/// [`validate_facade_route_table`] as a table completeness failure.
static FALLBACK_DESCRIPTOR: FamilyDescriptor = FamilyDescriptor {
    family: FacadeFamily::Intrinsic,
    methods: &[],
    capability: ExtensionCapability::Runs,
    required_feature: None,
    source_prefixes: &[],
    validation: &[],
};

/// The closed canonical family table. Order matters only for readability;
/// prefix matching always selects the longest match across all families.
pub static FACADE_FAMILIES: &[FamilyDescriptor] = &[
    // ── Protocol-native core families ──────────────────────────────────
    FamilyDescriptor {
        family: FacadeFamily::AgentLifecycle,
        methods: &[
            "_echo_agent/agent/create",
            "_echo_agent/agent/describe",
            "_echo_agent/agent/close",
        ],
        capability: ExtensionCapability::AgentLifecycle,
        required_feature: None,
        source_prefixes: &[],
        validation: CORE_VALIDATION,
    },
    FamilyDescriptor {
        family: FacadeFamily::Session,
        methods: &[
            "_echo_agent/session/create",
            "_echo_agent/session/load",
            "_echo_agent/session/close",
        ],
        capability: ExtensionCapability::SessionHandles,
        required_feature: None,
        source_prefixes: &[],
        validation: CORE_VALIDATION,
    },
    FamilyDescriptor {
        family: FacadeFamily::Run,
        methods: &[
            "_echo_agent/run/start",
            "_echo_agent/run/get",
            "_echo_agent/run/wait",
            "_echo_agent/run/cancel",
            "_echo_agent/run/steer",
        ],
        capability: ExtensionCapability::Runs,
        required_feature: None,
        source_prefixes: &[],
        validation: CORE_VALIDATION,
    },
    FamilyDescriptor {
        family: FacadeFamily::EventReplay,
        methods: &[
            "_echo_agent/run/replay",
            "_echo_agent/event",
            "_echo_agent/event/ack",
            "_echo_agent/gap",
        ],
        capability: ExtensionCapability::EventReplay,
        required_feature: None,
        source_prefixes: &[
            "echo_core::agent::event_envelope",
            "echo_core::agent::AgentEvent",
        ],
        validation: CORE_VALIDATION,
    },
    FamilyDescriptor {
        family: FacadeFamily::StructuredOutput,
        methods: &["_echo_agent/structured_output/validate"],
        capability: ExtensionCapability::StructuredOutput,
        required_feature: None,
        source_prefixes: &["echo_agent::agent::react::structured"],
        validation: CORE_VALIDATION,
    },
    FamilyDescriptor {
        family: FacadeFamily::Task,
        methods: &[
            "_echo_agent/task/create",
            "_echo_agent/task/update",
            "_echo_agent/task/list",
            "_echo_agent/task/execute",
            "_echo_agent/task/control",
        ],
        capability: ExtensionCapability::TaskGraph,
        required_feature: None,
        source_prefixes: &["echo_orchestration::tasks"],
        validation: CORE_VALIDATION,
    },
    FamilyDescriptor {
        family: FacadeFamily::Subagent,
        methods: &[
            "_echo_agent/subagent/dispatch",
            "_echo_agent/subagent/await",
            "_echo_agent/subagent/control",
        ],
        capability: ExtensionCapability::Subagents,
        required_feature: Some("subagent"),
        source_prefixes: &["echo_agent::agent::subagent"],
        validation: CORE_VALIDATION,
    },
    // ── Stateful feature families ──────────────────────────────────────
    FamilyDescriptor {
        family: FacadeFamily::Memory,
        methods: &["_echo_agent/memory/op"],
        capability: ExtensionCapability::FeatureSurfaces,
        required_feature: None,
        source_prefixes: &[
            "echo_core::memory",
            "echo_state::memory",
            "echo_core::compression",
            "echo_state::compression",
            "echo_agent::memory_promoter",
        ],
        validation: FAMILY_VALIDATION,
    },
    FamilyDescriptor {
        family: FacadeFamily::Workflow,
        methods: &["_echo_agent/workflow/op"],
        capability: ExtensionCapability::FeatureSurfaces,
        required_feature: None,
        source_prefixes: &["echo_orchestration::workflow", "echo_agent::workflow"],
        validation: FAMILY_VALIDATION,
    },
    FamilyDescriptor {
        family: FacadeFamily::Delivery,
        methods: &["_echo_agent/delivery/op"],
        capability: ExtensionCapability::FeatureSurfaces,
        required_feature: None,
        // Must match before the broader `echo_agent::state` prefix below:
        // the state module re-exports the delivery surface.
        source_prefixes: &["echo_state::delivery"],
        validation: FAMILY_VALIDATION,
    },
    FamilyDescriptor {
        family: FacadeFamily::State,
        methods: &["_echo_agent/state/op"],
        capability: ExtensionCapability::FeatureSurfaces,
        required_feature: None,
        source_prefixes: &["echo_state::journal", "echo_agent::state"],
        validation: FAMILY_VALIDATION,
    },
    FamilyDescriptor {
        family: FacadeFamily::Trace,
        methods: &["_echo_agent/trace/op"],
        capability: ExtensionCapability::FeatureSurfaces,
        required_feature: None,
        source_prefixes: &["echo_agent::trace"],
        validation: FAMILY_VALIDATION,
    },
    FamilyDescriptor {
        family: FacadeFamily::Eval,
        methods: &["_echo_agent/eval/op"],
        capability: ExtensionCapability::FeatureSurfaces,
        required_feature: Some("eval"),
        source_prefixes: &["echo_agent::eval"],
        validation: FAMILY_VALIDATION,
    },
    FamilyDescriptor {
        family: FacadeFamily::Improve,
        methods: &["_echo_agent/improve/op"],
        capability: ExtensionCapability::FeatureSurfaces,
        required_feature: Some("improve"),
        source_prefixes: &["echo_agent::improve"],
        validation: FAMILY_VALIDATION,
    },
    FamilyDescriptor {
        family: FacadeFamily::Permission,
        methods: &["_echo_agent/permission/op"],
        capability: ExtensionCapability::FeatureSurfaces,
        required_feature: Some("human-loop"),
        source_prefixes: &["echo_orchestration::human_loop::service"],
        validation: FAMILY_VALIDATION,
    },
    // ── Integration families ───────────────────────────────────────────
    FamilyDescriptor {
        family: FacadeFamily::Mcp,
        methods: &["_echo_agent/mcp/op"],
        capability: ExtensionCapability::FeatureSurfaces,
        required_feature: Some("mcp"),
        source_prefixes: &["echo_integration::mcp"],
        validation: FAMILY_VALIDATION,
    },
    FamilyDescriptor {
        family: FacadeFamily::A2a,
        methods: &["_echo_agent/a2a/op"],
        capability: ExtensionCapability::FeatureSurfaces,
        required_feature: Some("a2a"),
        source_prefixes: &["echo_agent::a2a"],
        validation: FAMILY_VALIDATION,
    },
    FamilyDescriptor {
        family: FacadeFamily::Lsp,
        methods: &["_echo_agent/lsp/op"],
        capability: ExtensionCapability::FeatureSurfaces,
        required_feature: Some("lsp"),
        source_prefixes: &[
            "echo_core::lsp",
            "echo_integration::lsp",
            "echo_agent::tools::lsp",
        ],
        validation: FAMILY_VALIDATION,
    },
    FamilyDescriptor {
        family: FacadeFamily::Channels,
        methods: &["_echo_agent/channels/op"],
        capability: ExtensionCapability::FeatureSurfaces,
        required_feature: Some("channels"),
        source_prefixes: &["echo_integration::channels"],
        validation: FAMILY_VALIDATION,
    },
    FamilyDescriptor {
        family: FacadeFamily::Telemetry,
        methods: &["_echo_agent/telemetry/op"],
        capability: ExtensionCapability::FeatureSurfaces,
        required_feature: Some("telemetry"),
        source_prefixes: &["echo_agent::telemetry", "echo_state::skill_telemetry"],
        validation: FAMILY_VALIDATION,
    },
    FamilyDescriptor {
        family: FacadeFamily::Topology,
        methods: &["_echo_agent/topology/op"],
        capability: ExtensionCapability::FeatureSurfaces,
        required_feature: Some("topology"),
        source_prefixes: &["echo_agent::topology"],
        validation: FAMILY_VALIDATION,
    },
    // ── Tool families ──────────────────────────────────────────────────
    FamilyDescriptor {
        family: FacadeFamily::Web,
        methods: &["_echo_agent/web/op"],
        capability: ExtensionCapability::FeatureSurfaces,
        required_feature: Some("web"),
        source_prefixes: &["echo_tools::web"],
        validation: FAMILY_VALIDATION,
    },
    FamilyDescriptor {
        family: FacadeFamily::Files,
        methods: &["_echo_agent/files/op"],
        capability: ExtensionCapability::FeatureSurfaces,
        required_feature: Some("files"),
        source_prefixes: &["echo_tools::files", "echo_tools::skills::filesystem"],
        validation: FAMILY_VALIDATION,
    },
    FamilyDescriptor {
        family: FacadeFamily::Shell,
        methods: &["_echo_agent/shell/op"],
        capability: ExtensionCapability::FeatureSurfaces,
        required_feature: Some("shell"),
        source_prefixes: &["echo_tools::shell", "echo_tools::skills::shell"],
        validation: FAMILY_VALIDATION,
    },
    FamilyDescriptor {
        family: FacadeFamily::Git,
        methods: &["_echo_agent/git/op"],
        capability: ExtensionCapability::FeatureSurfaces,
        required_feature: Some("git"),
        source_prefixes: &[
            "echo_tools::git",
            "echo_tools::git_checkpoint",
            "echo_tools::git_worktree",
            "echo_tools::worktree_tool",
        ],
        validation: FAMILY_VALIDATION,
    },
    FamilyDescriptor {
        family: FacadeFamily::Database,
        methods: &["_echo_agent/database/op"],
        capability: ExtensionCapability::FeatureSurfaces,
        required_feature: Some("database"),
        source_prefixes: &["echo_tools::database"],
        validation: FAMILY_VALIDATION,
    },
    FamilyDescriptor {
        family: FacadeFamily::Rag,
        methods: &["_echo_agent/rag/op"],
        capability: ExtensionCapability::FeatureSurfaces,
        required_feature: Some("rag"),
        source_prefixes: &["echo_tools::rag"],
        validation: FAMILY_VALIDATION,
    },
    FamilyDescriptor {
        family: FacadeFamily::Chart,
        methods: &["_echo_agent/chart/op"],
        capability: ExtensionCapability::FeatureSurfaces,
        required_feature: Some("chart"),
        source_prefixes: &["echo_tools::chart"],
        validation: FAMILY_VALIDATION,
    },
    FamilyDescriptor {
        family: FacadeFamily::Media,
        methods: &["_echo_agent/media/op"],
        capability: ExtensionCapability::FeatureSurfaces,
        required_feature: Some("media"),
        source_prefixes: &[
            "echo_tools::media",
            "echo_tools::excel",
            "echo_tools::image",
            "echo_tools::pdf",
            "echo_tools::text",
            "echo_tools::word",
        ],
        validation: FAMILY_VALIDATION,
    },
    FamilyDescriptor {
        family: FacadeFamily::Data,
        methods: &["_echo_agent/data/op"],
        capability: ExtensionCapability::FeatureSurfaces,
        required_feature: Some("data"),
        source_prefixes: &["echo_tools::data", "echo_tools::data_quality"],
        validation: FAMILY_VALIDATION,
    },
    FamilyDescriptor {
        family: FacadeFamily::Statistics,
        methods: &["_echo_agent/statistics/op"],
        capability: ExtensionCapability::FeatureSurfaces,
        required_feature: Some("statistics"),
        source_prefixes: &["echo_tools::statistics"],
        validation: FAMILY_VALIDATION,
    },
    FamilyDescriptor {
        family: FacadeFamily::Research,
        methods: &["_echo_agent/research/op"],
        capability: ExtensionCapability::FeatureSurfaces,
        required_feature: Some("research"),
        source_prefixes: &["echo_tools::research"],
        validation: FAMILY_VALIDATION,
    },
    FamilyDescriptor {
        family: FacadeFamily::ContentGuard,
        methods: &["_echo_agent/content-guard/op"],
        capability: ExtensionCapability::FeatureSurfaces,
        required_feature: Some("content-guard"),
        source_prefixes: &["echo_core::guard", "echo_agent::guard"],
        validation: FAMILY_VALIDATION,
    },
    FamilyDescriptor {
        family: FacadeFamily::ProjectRules,
        methods: &["_echo_agent/project-rules/op"],
        capability: ExtensionCapability::FeatureSurfaces,
        required_feature: Some("project-rules"),
        source_prefixes: &["echo_core::project_rules", "echo_agent::project_rules"],
        validation: FAMILY_VALIDATION,
    },
    FamilyDescriptor {
        family: FacadeFamily::Testing,
        methods: &["_echo_agent/testing/op"],
        capability: ExtensionCapability::FeatureSurfaces,
        required_feature: Some("testing"),
        source_prefixes: &["echo_agent::testing"],
        validation: FAMILY_VALIDATION,
    },
    FamilyDescriptor {
        family: FacadeFamily::SourceOperation,
        methods: &[],
        capability: ExtensionCapability::FeatureSurfaces,
        required_feature: None,
        source_prefixes: &[],
        validation: &["echo-sdk-protocol/tests/facade_inventory.rs"],
    },
    // ── Generic surfaces ───────────────────────────────────────────────
    FamilyDescriptor {
        family: FacadeFamily::Invoke,
        methods: &["_echo_agent/facade/invoke"],
        capability: ExtensionCapability::FeatureSurfaces,
        required_feature: None,
        source_prefixes: &[],
        validation: FAMILY_VALIDATION,
    },
    FamilyDescriptor {
        family: FacadeFamily::Value,
        methods: &[],
        capability: ExtensionCapability::FeatureSurfaces,
        required_feature: None,
        source_prefixes: &[],
        validation: &["echo-sdk-protocol/tests/facade_inventory.rs"],
    },
    FamilyDescriptor {
        family: FacadeFamily::Bridge,
        methods: &[
            "_echo_agent/extension/register",
            "_echo_agent/extension/unregister",
            "_echo_agent/extension/invoke",
            "_echo_agent/extension/cancel",
            "_echo_agent/extension/stream",
        ],
        capability: ExtensionCapability::ExtensionBridge,
        required_feature: None,
        source_prefixes: &[],
        validation: BRIDGE_VALIDATION,
    },
    FamilyDescriptor {
        family: FacadeFamily::Standard,
        methods: &[],
        capability: ExtensionCapability::AgentLifecycle,
        required_feature: None,
        source_prefixes: &[],
        validation: &[
            "tests/acp_agent_adapter.rs",
            "echo-sdk-protocol/tests/acp_baseline.rs",
        ],
    },
    FamilyDescriptor {
        family: FacadeFamily::Intrinsic,
        methods: &[],
        capability: ExtensionCapability::AgentLifecycle,
        required_feature: None,
        source_prefixes: &[],
        validation: &["echo-sdk-protocol/tests/facade_inventory.rs"],
    },
];

/// Typed extension-bridge kinds resolved by canonical trait source identity.
///
/// Closure rule (plan 07 todo 5b): a consumer-implemented trait is
/// bridge-routed **iff the SDK Host has a live consumption point** for it —
/// the Host itself calls the trait through a proxy while serving a Session
/// (tool execution, LLM traffic, memory, approvals, hooks/callbacks,
/// agent/subagent construction). Every other public consumer trait remains a
/// language-local interface/helper obligation because the SDK Host has no
/// runtime call site for it. This does not imply that an out-of-process source
/// SDK links the Rust framework. Exposing one of those seams remotely is a
/// versioned contract change that adds a typed kind here — never a
/// `bridge:pending` placeholder.
const TYPED_BRIDGE_TRAITS: &[(&str, ExtensionKind)] = &[
    ("echo_core::tools::Tool", ExtensionKind::Tool),
    ("echo_core::llm::LlmClient", ExtensionKind::LlmClient),
    ("echo_core::memory::store::Store", ExtensionKind::Store),
    (
        "echo_orchestration::human_loop::HumanLoopProvider",
        ExtensionKind::HumanLoopProvider,
    ),
    (
        "echo_orchestration::human_loop::batch::BatchApprovalProvider",
        ExtensionKind::HumanLoopProvider,
    ),
    (
        "echo_core::agent::AgentCallback",
        ExtensionKind::AgentCallback,
    ),
    (
        "echo_core::agent::intervention::InterventionCallback",
        ExtensionKind::InterventionCallback,
    ),
    (
        "echo_core::agent::factory::AgentFactory",
        ExtensionKind::AgentFactory,
    ),
    // The subagent registry factory is the Host's live consumption point for
    // kind `AgentFactory` (SubagentFactoryAdapter); the core factory trait is
    // the same config→agent semantic family over the same wire operation.
    (
        "echo_agent::agent::subagent::registry::AgentFactory",
        ExtensionKind::AgentFactory,
    ),
    (
        "echo_integration::channels::types::ChannelPlugin",
        ExtensionKind::ChannelPlugin,
    ),
    (
        "echo_integration::channels::types::MessageHandler",
        ExtensionKind::ChannelMessageHandler,
    ),
    // `Agent` itself is the CustomAgent proxy surface: registered custom
    // agents serve subagent dispatch and direct execution over the bridge.
    ("echo_core::agent::Agent", ExtensionKind::CustomAgent),
    ("echo_core::agent::critic::Critic", ExtensionKind::Critic),
    (
        "echo_core::compression::ContextCompressor",
        ExtensionKind::ContextCompressor,
    ),
    (
        "echo_core::audit::AuditLogger",
        ExtensionKind::AgentComponent,
    ),
    (
        "echo_core::compression::PreModelContextProjector",
        ExtensionKind::AgentComponent,
    ),
    (
        "echo_agent::evolution::triggers::MemoryTriggerSink",
        ExtensionKind::AgentComponent,
    ),
    (
        "echo_core::memory::conversation::ConversationStore",
        ExtensionKind::AgentComponent,
    ),
    (
        "echo_agent::state::RuntimeStateStore",
        ExtensionKind::AgentComponent,
    ),
    ("echo_agent::trace::RunStore", ExtensionKind::AgentComponent),
    ("echo_core::guard::Guard", ExtensionKind::AgentComponent),
    (
        "echo_tools::web::providers::SearchProvider",
        ExtensionKind::AgentComponent,
    ),
    (
        "echo_orchestration::workflow::checkpoint_store::CheckpointStore",
        ExtensionKind::AgentComponent,
    ),
    (
        "echo_orchestration::tasks::revisioned::RevisionedTaskStore",
        ExtensionKind::AgentComponent,
    ),
    (
        "echo_core::sandbox::SandboxExecutor",
        ExtensionKind::AgentComponent,
    ),
    (
        "echo_integration::mcp::transport::McpTransport",
        ExtensionKind::AgentComponent,
    ),
    (
        "echo_integration::mcp::types::JsonRpcNotificationReceiver",
        ExtensionKind::AgentComponent,
    ),
    (
        "echo_core::memory::embedder::Embedder",
        ExtensionKind::AgentComponent,
    ),
    (
        "echo_state::compression::MemoryPromoter",
        ExtensionKind::AgentComponent,
    ),
    (
        "echo_orchestration::workflow::Workflow",
        ExtensionKind::AgentComponent,
    ),
    (
        "echo_agent::intent::IntentClassifier",
        ExtensionKind::AgentComponent,
    ),
    (
        "echo_execution::skills::external::loader::SkillLoadPolicy",
        ExtensionKind::AgentComponent,
    ),
];

/// Consumer traits that are deliberately process-local construction seams.
///
/// These traits carry closures, generic Rust values, or host-owned callback
/// state and have no live Host call site. They are explicitly enumerated so a
/// newly added consumer trait cannot silently become `intrinsic` merely by
/// missing the typed bridge table.
const PROVEN_PROCESS_LOCAL_TRAITS: &[(&str, &str)] = &[
    (
        "echo_agent::acp::runtime::AcpConnectionProfile",
        "process-local-acp-runtime-authority",
    ),
    (
        "echo_agent::acp::session::AcpSessionFactory",
        "process-local-acp-runtime-authority",
    ),
    (
        "echo_agent::acp::runtime::RunEventObserver",
        "process-local-acp-runtime-authority",
    ),
    (
        "echo_core::agent::builder::AgentBuilder",
        "process-local-language-builder-protocol",
    ),
    (
        "echo_core::agent::AgentInputLifecycle",
        "process-local-agent-owned-input-drain-hook",
    ),
    (
        "echo_agent::agent::subagent::isolation::IsolationProvider",
        "process-local-fnonce-isolation-handle",
    ),
    (
        "echo_agent::agent::subagent::isolation::SharedIsolationProvider",
        "process-local-fnonce-isolation-handle",
    ),
    (
        "echo_agent::agent::subagent::hooks::SubagentHooks",
        "process-local-subagent-runtime-hook-registry",
    ),
    (
        "echo_agent::agent::subagent::prompt::SubagentPromptCompiler",
        "process-local-borrowed-subagent-prompt-view",
    ),
    (
        "echo_agent::agent::subagent::team::TeamRuntime",
        "process-local-generic-dag-controller",
    ),
    (
        "echo_agent::agent::subagent::events::SubagentEventListener",
        "process-local-subagent-event-bus-listener",
    ),
    (
        "echo_agent::evolution::audit::ChangeLog",
        "process-local-evolution-storage-construction",
    ),
    (
        "echo_agent::evolution::layer::EvolutionObserver",
        "process-local-evolution-layer-callback",
    ),
    (
        "echo_orchestration::human_loop::classifier::Classifier",
        "process-local-permission-classifier-composition",
    ),
    (
        "echo_orchestration::human_loop::HumanLoopHandler",
        "process-local-human-loop-response-settlement",
    ),
    (
        "echo_orchestration::human_loop::audit::PermissionAuditSink",
        "process-local-permission-audit-composition",
    ),
    (
        "echo_orchestration::human_loop::permission::PermissionRequestHandler",
        "process-local-permission-request-settlement",
    ),
    (
        "echo_core::plugin::lifecycle::PluginLifecycle",
        "process-local-plugin-lifecycle-manager",
    ),
    (
        "echo_orchestration::runtime::turn_driver::EventSink",
        "process-local-driver-owned-event-sink",
    ),
    (
        "echo_core::tools::skill::Skill",
        "process-local-skill-registry-object",
    ),
    (
        "echo_core::tokenizer::Tokenizer",
        "process-local-agent-tokenizer-construction",
    ),
    (
        "echo_core::tools::CommandPolicy",
        "process-local-shell-policy-composition",
    ),
    (
        "echo_core::tools::ScriptExecutionProfileResolver",
        "process-local-script-profile-composition",
    ),
    (
        "echo_core::tools::ToolPack",
        "process-local-tool-pack-registration",
    ),
    (
        "echo_core::tools::ToolRegistrar",
        "process-local-generic-tool-registrar",
    ),
    (
        "echo_core::tools::ToolRunner",
        "process-local-sized-tool-helper",
    ),
    (
        "echo_core::tools::cell::CommandCellRegistry",
        "process-local-command-cell-registry",
    ),
    (
        "echo_core::tools::permission::PermissionPolicy",
        "process-local-permission-service-policy",
    ),
    (
        "echo_core::lsp::client::LspClient",
        "process-local-lsp-manager-client",
    ),
    (
        "echo_integration::channels::session::SessionFactory",
        "process-local-channel-session-factory",
    ),
    (
        "echo_state::journal::CheckpointStore",
        "process-local-generic-journal-checkpoint",
    ),
    (
        "echo_state::journal::EventJournal",
        "process-local-generic-journal-event",
    ),
    (
        "echo_state::journal::EventReducer",
        "process-local-generic-journal-reducer",
    ),
    (
        "echo_state::journal::JournalEvent",
        "process-local-generic-journal-value",
    ),
    (
        "echo_orchestration::tasks::background_task::AnyBackgroundTask",
        "process-local-type-erased-task-handle",
    ),
    (
        "echo_orchestration::tasks::events::AsyncTaskEventListener",
        "process-local-task-event-bus-listener",
    ),
    (
        "echo_orchestration::tasks::background_state::CheckpointStore",
        "process-local-background-task-checkpoint",
    ),
    (
        "echo_orchestration::tasks::runtime_executor::RuntimeDagController",
        "process-local-generic-dag-controller",
    ),
    (
        "echo_orchestration::tasks::events::TaskEventListener",
        "process-local-task-event-bus-listener",
    ),
    (
        "echo_orchestration::tasks::runtime::TaskSubagent",
        "process-local-generic-subagent-dispatch",
    ),
    (
        "echo_orchestration::tasks::revisioned::TaskToolPolicy",
        "process-local-tool-context-policy",
    ),
    (
        "echo_state::delivery::DeliveryPayload",
        "process-local-generic-delivery-value",
    ),
    (
        "echo_state::delivery::DeliveryRoute",
        "process-local-generic-delivery-route",
    ),
    (
        "echo_orchestration::workflow::SharedAgent",
        "process-local-framework-component-construction-seam",
    ),
    (
        "echo_orchestration::workflow::SharedAgentMutex",
        "process-local-framework-component-construction-seam",
    ),
];

/// Construction-only public surfaces are represented by the versioned wire
/// config DTO (`AgentConfigWire`) and language SDK builders. They do not
/// identify a live Host object and must not be invoked through an Agent
/// handle.
const PROVEN_LANGUAGE_LOCAL_SOURCES: &[&str] = &[
    "echo_agent::agent::config::AgentConfig",
    "echo_agent::agent::react::builder::ReactAgentBuilder",
    "echo_core::agent::builder::AgentBuilder",
    "echo_agent::agent::handle::AgentHandle",
    "echo_core::agent::AgentInvocationContext",
    "echo_agent::agent::snapshot::AgentRunSnapshot",
    "echo_agent::agent::snapshot::GuardRuntime",
    "echo_agent::agent::snapshot::RuntimeConfig",
    "echo_agent::agent::snapshot::ToolRuntime",
    "tokio_util::sync::cancellation_token::CancellationToken",
    "echo_core::tools::ToolResult",
    "echo_core::tools::ToolCallParams",
    "echo_core::agent::AgentSteerReceipt",
    "echo_core::agent::AgentSteerState",
    "echo_core::agent::AgentSteerTurnOutcome",
    "echo_orchestration::runtime::turn_driver::TurnRequest",
    "echo_orchestration::runtime::turn_driver::TurnInputReceipt",
    "echo_orchestration::runtime::turn_driver::TurnMode",
    "echo_core::tokenizer::TokenUsageTracker",
    "echo_core::agent::ToolVisibilityPolicy",
    "echo_core::agent::Agent::set_external_context",
    "echo_core::agent::Agent::clear_external_context",
    "echo_core::agent::Agent::tool_visibility_policy",
    "echo_core::agent::factory::AgentFactoryConfig",
    "echo_core::budget::TokenBudget",
    "echo_core::budget::TokenBudgetConfig",
    "echo_core::llm::LlmTimeouts",
    "echo_core::llm::thinking::ThinkingConfig",
    "echo_core::llm::capabilities::ModelProfile",
    "echo_core::llm::capabilities::ModelProfileResolver",
    "echo_core::retry::RetryPolicy",
    "echo_core::sandbox::ResourceLimits",
    "echo_tools::security::PathValidator",
    "echo_tools::security::ResourceLimits",
    "echo_tools::security::SecurityConfig",
    "echo_agent::testing",
    "echo_core::agent::Agent::chat_stream_with_cancel",
    "echo_core::agent::Agent::chat_stream_message_with_cancel",
    "echo_core::agent::Agent::chat_stream_message_with_invocation_context",
    "echo_core::agent::Agent::execute_stream_with_cancel",
    "echo_core::agent::Agent::execute_stream_message_with_cancel",
    "echo_core::agent::Agent::execute_stream_message_with_invocation_context",
    "echo_core::agent::Agent::execute_stream_with_invocation_context",
    "echo_agent::agent::react::ReactAgent::chat_stream_message_with_cancel",
    "echo_agent::agent::react::ReactAgent::config",
    "echo_agent::agent::react::ReactAgent::config_mut",
    "echo_agent::agent::react::ReactAgent::build_permission_service",
    "echo_agent::agent::react::ReactAgent::build_workspace_context_block",
    "echo_agent::agent::react::ReactAgent::tool_visibility_policy",
    "echo_agent::agent::react::ReactAgent::use_tool_visibility_policy",
    "echo_orchestration::human_loop::HumanLoopManager",
    "echo_integration::providers::responses::ResponsesClient",
    "echo_core::sandbox::SandboxExecutor",
    "echo_integration::mcp::client::McpClient::new",
    "echo_integration::mcp::client::McpClient::content_to_text",
    "echo_integration::mcp::client::McpClient::refresh_tools",
    "echo_integration::mcp::client::McpClient::refresh_resources",
    "echo_integration::mcp::client::McpClient::refresh_prompts",
    // No Host-owned authority: these objects retain process-local storage,
    // callbacks, cancellation or plugin/evolution registries.
    "echo_orchestration::scheduler::cron_task::CronTaskStore",
    "echo_orchestration::scheduler::runner::SchedulerRunner",
    "echo_orchestration::scheduler::runner::SchedulerHandle",
    "echo_state::profiles::ProfileStore",
    "echo_core::plugin::registry::PluginRegistry",
    "echo_core::plugin::lifecycle::PluginLifecycleManager",
    "echo_agent::plugin::prepared::PluginIntegrator",
    "echo_agent::plugin::prepared::PreparedPlugin",
    "echo_agent::plugin::prepared::PreparedPluginSet",
    "echo_agent::plugin::prepared::PreparedPluginDocument",
    "echo_agent::plugin::prepared::PreparedPluginSkill",
    "echo_agent::plugin::prepared::PluginPreparationDiagnostic",
    "echo_agent::evolution::audit::JsonlChangeLog",
    "echo_agent::evolution::runtime_integration::MemoryRuntimeIntegrationBuilder",
    "echo_agent::evolution::layer::MemoryLayerManager",
    "echo_agent::evolution::curator::Curator",
    "echo_agent::evolution::background_review::BackgroundReviewer",
    "echo_agent::evolution::dreaming::Dreaming",
    "echo_agent::evolution::draft::SkillDraftGenerator",
    "echo_agent::evolution::merge::SkillMerger",
    "echo_agent::evolution::review::MemoryReviewer",
    "echo_agent::evolution::review::MemoryMerger",
    "echo_agent::evolution::review::ConflictDetector",
    "echo_agent::evolution::health::SkillHealthMonitor",
    "echo_agent::evolution::patch::SkillPatcher",
    "echo_agent::evolution::recall::MemoryRecaller",
    "echo_agent::evolution::security::EvolutionSecurityGuard",
    "echo_agent::evolution::runtime_integration::HookEvolutionObserver",
    "echo_orchestration::human_loop::classifier::RuleClassifier",
    "echo_orchestration::human_loop::classifier::CompositeClassifier",
    "echo_orchestration::human_loop::classifier::DenialTracker",
    "echo_orchestration::human_loop::classifier::LlmClassifier",
    "echo_orchestration::human_loop::audit::CompositePermissionAuditSink",
    "echo_orchestration::human_loop::audit::InMemoryPermissionAuditSink",
    "echo_orchestration::human_loop::audit::LoggingPermissionAuditSink",
    "echo_orchestration::human_loop::ApprovalResponder",
    "echo_orchestration::human_loop::InputResponder",
    "echo_orchestration::human_loop::SelectionResponder",
    "echo_state::audit::file::FileAuditLogger",
    "echo_state::audit::memory::InMemoryAuditLogger",
    "echo_integration::providers::openai::OpenAiClient",
    "echo_integration::providers::anthropic::AnthropicClient",
    "echo_orchestration::human_loop::console::ConsoleHumanLoopProvider",
    "echo_orchestration::human_loop::webhook::WebhookHumanLoopProvider",
    "echo_orchestration::human_loop::websocket::WebSocketHumanLoopProvider",
    "echo_orchestration::human_loop::protected::ProtectedPathChecker",
    "echo_orchestration::human_loop::service::PermissionServiceBuilder",
    // RuleRegistry and SessionApprovalCache are embedded in the
    // Session-owned PermissionService; they have no independently issued
    // Host handles. Their direct Rust methods remain source-local helpers,
    // while the permission family exposes the service-owned projections.
    "echo_core::tools::permission::RuleRegistry",
    "echo_orchestration::human_loop::approval_cache::SessionApprovalCache",
    "echo_core::tools::permission::DefaultPermissionPolicy",
    "echo_core::agent::admission::ExecutionAdmission",
    "echo_core::agent::admission::KeyedExecutionAdmission",
    "echo_core::agent::admission::KeyedExecutionLease",
    "echo_core::agent::admission::KeyedExecutionRetirement",
    "echo_agent::agent::react::capabilities::PreparedAgentModelDeactivation",
    "echo_agent::agent::react::capabilities::PreparedAgentModelGeneration",
    "echo_agent::agent::react::capabilities::PreparedTokenLimit",
    "echo_agent::agent::react::run::pipeline::ToolExecutionPipeline",
    "echo_agent::agent::callbacks::progress_bridge::ProgressBridge",
    "echo_core::agent::critic::composite::CompositeCritic",
    "echo_core::agent::critic::StaticCritic",
    "echo_core::agent::critic::ThresholdCritic",
    "echo_agent::agent::critic::llm_critic::LlmCritic",
    "echo_execution::tools::ToolManager",
    "echo_core::tools::ToolVisibilityState",
    "echo_core::tools::control::ToolControlService",
    "echo_core::tools::artifact::ToolOutputArtifactWriter",
    "echo_tools::registry::StandardToolPack",
    "echo_agent::hooks_bridge::TaskHookBridge",
    "echo_agent::hooks_bridge::SubagentHookBridge",
    "echo_agent::intent::IntentRouter",
    "echo_agent::intent::KeywordClassifier",
    "echo_agent::intent::LlmIntentClassifier",
    "echo_agent::intent::classifier::ChainedClassifier",
    "echo_agent::intent::trigger_supervisor::TriggerSupervisor",
    "echo_core::tokenizer::CalibratedTokenizer",
    "echo_core::circuit_breaker::CircuitBreaker",
    "echo_execution::sandbox::manager::SandboxManager",
    "echo_execution::sandbox::docker::DockerSandbox",
    "echo_execution::sandbox::k8s::K8sSandbox",
    "echo_execution::sandbox::local::LocalSandbox",
];

/// Exact helper identities whose public behavior is local to the language
/// boundary. This stays separate from prefix-based configuration entries so
/// adjacent filesystem, clock, network and persistence authorities remain
/// explicit source operations.
const PROVEN_LANGUAGE_LOCAL_EXACT_SOURCES: &[&str] = &[
    "echo_core::budget::TokenAllocation::ok",
    "echo_core::budget::TokenAllocation::needs_compression",
    "echo_agent::paths::DataRoot",
    "echo_agent::paths::DataRoot::new",
    "echo_agent::paths::DataRoot::as_path",
    "echo_agent::paths::DataRoot::path",
    "echo_core::utils::canonical_json::canonical_json_bytes",
    "echo_core::utils::json_parse::clean_json",
    "echo_core::utils::json_parse::extract_json_from_markdown",
    "echo_core::utils::utf8::IncrementalUtf8Decoder",
    "echo_core::utils::utf8::IncrementalUtf8Decoder::new",
    "echo_core::utils::utf8::IncrementalUtf8Decoder::push",
    "echo_core::utils::utf8::IncrementalUtf8Decoder::finish",
    "echo_core::utils::utf8::split_utf8_chunks",
    // Pure path-value helpers are implemented idiomatically by each source
    // SDK. File I/O and descriptor guards are deliberately excluded and stay
    // on the Host's Rust authority. `atomic_compare_and_swap` is the one I/O
    // exception: its arbitrary `FnOnce(&[u8]) -> bool` callback cannot cross
    // ACP without changing the public contract, so it remains an in-process
    // embedding seam rather than a misleading equality-only adapter.
    "echo_core::utils::fs::ExclusiveFileLease",
    "echo_core::utils::fs::ExistingDirectoryGuard",
    "echo_core::utils::fs::ExistingRegularFileGuard",
    "echo_core::utils::fs::FileDurability",
    "echo_core::utils::fs::encode_path_segment_identity",
    "echo_core::utils::fs::encode_utf8_path_identity",
    "echo_core::utils::fs::join_path_segment",
    "echo_core::utils::fs::validate_path_segment",
    "echo_core::utils::retention::ContentRetentionPolicy::sanitize_text",
    "echo_core::utils::retention::ContentRetentionPolicy::sanitize_json",
    "echo_core::utils::time::local_rfc3339::deserialize",
    "echo_core::utils::time::option_local_rfc3339::deserialize",
    "echo_orchestration::runtime::turn_driver::TurnReceipt::cancelled",
    "echo_orchestration::runtime::turn_driver::TurnReceipt::failed",
    "echo_orchestration::scheduler::cron_task::CronTask::new",
    "echo_orchestration::scheduler::cron_task::CronTask::next_run",
    "echo_orchestration::scheduler::cron_task::CronTask::next_run_after",
    "echo_orchestration::scheduler::cron_task::CronTask::validate_cron",
    "echo_agent::context::ContextAssembler::new",
    "echo_agent::context::ContextAssembler::with_budget",
    "echo_agent::context::ContextAssembler::assemble",
    "echo_agent::context::ContextBudget::new",
    "echo_agent::context::SourcePriority",
    "echo_agent::context::selector::ContextSelector::new",
    "echo_agent::context::selector::ContextSelector::score_files",
    "echo_agent::context::selector::ContextSelector::select_relevant",
    "echo_state::profiles::AgentProfile::new",
    "echo_state::profiles::AgentProfile::update_from_telemetry",
    "echo_state::profiles::AgentProfile::top_capabilities",
    "echo_state::profiles::AgentProfile::top_tools",
    "echo_state::profiles::AgentProfile::to_prompt_block",
    "echo_state::profiles::UserProfile::new",
    "echo_state::profiles::UserProfile::record_task",
    "echo_state::profiles::UserProfile::set_preference",
    "echo_state::profiles::UserProfile::top_tasks",
    "echo_state::profiles::UserProfile::to_prompt_block",
    "echo_core::plugin::scope::InstallSource::parse",
    "echo_core::plugin::scope::InstallSource::is_git",
    "echo_core::plugin::manifest::PluginManifest::validate",
    "echo_core::plugin::manifest::PluginManifest::resolve_user_config",
    "echo_core::plugin::manifest::PluginManifest::user_config_defaults",
    "echo_core::plugin::manifest::PluginManifest::validate_user_config",
    "echo_core::plugin::manifest::PluginManifest::from_json",
    "echo_core::plugin::manifest::PluginManifest::from_file",
    "echo_core::plugin::manifest::PluginManifest::unknown_top_level_fields",
    "echo_core::plugin::manifest::PluginManifest::version_label",
    "echo_core::plugin::manifest::PluginManifest::display_name",
    "echo_core::plugin::capability::PluginCapability::display_name",
    "echo_core::plugin::manifest::PluginDependency::name",
    "echo_core::plugin::manifest::PluginDependency::version_constraint",
    "echo_core::plugin::manifest::PluginDependency::satisfies",
    "echo_core::plugin::registry::PluginEntry::inferred_capabilities",
    "echo_core::plugin::variables::PluginVariables::with_json_user_config",
    "echo_core::plugin::variables::PluginVariables::with_user_config",
    "echo_core::plugin::variables::PluginVariables::new",
    "echo_core::plugin::variables::PluginVariables::with_plugin_data",
    "echo_core::plugin::variables::PluginVariables::substitute",
    "echo_core::plugin::variables::PluginVariables::resolve_path",
    "echo_core::plugin::variables::PluginVariables::ensure_data_dir",
    "echo_agent::evolution::candidate::SkillCandidateDetector::new",
    "echo_agent::evolution::candidate::SkillCandidateDetector::with_thresholds",
    "echo_agent::evolution::candidate::SkillCandidateDetector::with_curator",
    "echo_agent::evolution::candidate::SkillCandidateDetector::with_evolution_observer",
    "echo_agent::evolution::candidate::SkillCandidateDetector::detect",
    "echo_agent::evolution::security::InputTrustLevel::from_source",
    "echo_agent::evolution::security::InputTrustLevel::can_auto_promote",
    "echo_agent::evolution::security::InputTrustLevel::can_auto_promote_to_rule",
    "echo_agent::evolution::health::HealthStatus::from_score",
    "echo_agent::evolution::health::HealthStatus::description",
    "echo_agent::evolution::health::HealthBreakdown::overall_score",
    "echo_agent::evolution::patch::PatchType::label",
    "echo_agent::evolution::security::SecurityVerdict::allow",
    "echo_agent::evolution::security::SecurityVerdict::deny",
    "echo_agent::evolution::audit::ChangeFilter::new",
    "echo_agent::evolution::audit::ChangeFilter::with_entity_type",
    "echo_agent::evolution::audit::ChangeFilter::with_change_type",
    "echo_agent::evolution::audit::ChangeFilter::with_key_prefix",
    "echo_agent::evolution::audit::ChangeFilter::with_limit",
    "echo_agent::evolution::audit::ChangeFilter::matches",
    "echo_agent::evolution::audit::ChangeEntryBuilder::new",
    "echo_agent::evolution::audit::ChangeEntryBuilder::before",
    "echo_agent::evolution::audit::ChangeEntryBuilder::after",
    "echo_agent::evolution::audit::ChangeEntryBuilder::reason",
    "echo_agent::evolution::audit::ChangeEntryBuilder::trigger",
    "echo_agent::evolution::audit::ChangeEntryBuilder::build",
    "echo_agent::evolution::audit::ChangeEntryBuilder::build_with",
    "echo_execution::risk::ToolRiskClassifier",
    "echo_execution::risk::ToolRiskCategory",
    "echo_execution::risk::ToolRiskCategory::description",
    "echo_execution::risk::ToolRiskCategory::level",
    "echo_execution::risk::ToolRiskCategory::permission_label",
    "echo_orchestration::human_loop::HumanLoopEvent",
    "echo_orchestration::human_loop::policy::ApprovalScope::Session",
    "echo_core::audit::AuditEvent::now",
    "echo_core::tools::permission::PermissionDecision::is_allowed",
    "echo_core::tools::permission::PermissionDecision::is_denied",
    "echo_core::tools::permission::PermissionDecision::requires_approval",
    "echo_core::tools::permission::PermissionMode::id",
    "echo_core::tools::permission::PermissionMode::allows_write",
    "echo_core::tools::permission::PermissionMode::requires_interaction",
    "echo_core::tools::permission::PermissionMode::uses_classifier",
    "echo_integration::providers::config::LlmConfig::for_provider",
    "echo_integration::providers::config::LlmConfig::with_input_modalities",
    "echo_integration::providers::config::LlmConfig::with_timeouts",
    "echo_integration::providers::config::LlmConfig::build_client",
    "echo_core::llm::capabilities::ProviderCapabilities::from_provider_name",
    "echo_core::llm::capabilities::ProviderCapabilities::openai_compatible",
    "echo_core::llm::capabilities::ProviderCapabilities::anthropic",
    "echo_core::llm::capabilities::ProviderCapabilities::ollama",
    "echo_core::agent::intervention::InterventionResult::allow",
    "echo_core::agent::intervention::InterventionResult::block",
    "echo_core::agent::intervention::InterventionResult::cancel",
    "echo_core::agent::intervention::InterventionResult::inject",
    "echo_core::agent::intervention::InterventionResult::modify_args",
    "echo_orchestration::human_loop::service::PermissionService::new",
    "echo_orchestration::human_loop::service::PermissionService::from_provider",
    "echo_orchestration::human_loop::service::PermissionService::with_mode",
    "echo_orchestration::human_loop::service::PermissionService::with_classifier",
    "echo_orchestration::human_loop::service::PermissionService::with_request_handler",
    "echo_orchestration::human_loop::service::PermissionService::replace_provider",
    "echo_orchestration::human_loop::service::PermissionService::replace_provider_preserving_cache",
    "echo_orchestration::human_loop::service::PermissionService::with_max_consecutive_denials",
    "echo_orchestration::human_loop::service::PermissionService::with_protected_paths",
    "echo_orchestration::human_loop::service::PermissionService::with_audit_sink",
    "echo_orchestration::human_loop::service::PermissionService::set_mode_sync",
    "echo_agent::intent::Intent::confidence",
    "echo_agent::intent::Intent::skill_name",
    "echo_agent::agent::react::capabilities::PreparedCriticUpdate",
    "echo_agent::agent::react::capabilities::PreparedCriticUpdate::commit",
    "echo_core::tools::ToolPackEntry",
    "echo_core::tools::ToolPackEntry::new",
    "echo_core::tools::ToolPackEntry::read_only",
    "echo_core::agent::ExecutionUsage::duration_millis",
    "echo_core::sandbox::SandboxCommand::shell",
    "echo_core::sandbox::SandboxCommand::program",
    "echo_core::sandbox::SandboxCommand::code",
    "echo_core::sandbox::SandboxCommand::with_env",
    "echo_core::sandbox::SandboxCommand::with_minimum_isolation",
    "echo_core::sandbox::SandboxCommand::with_stdin",
    "echo_core::sandbox::SandboxCommand::with_timeout",
    "echo_core::sandbox::SandboxCommand::with_working_dir",
    "echo_core::sandbox::SandboxStreamFailure::is_cancelled",
    "echo_core::sandbox::SandboxStreamFailure::message",
    "echo_execution::sandbox::policy::SandboxPolicy::trusted",
    "echo_execution::sandbox::policy::SandboxPolicy::local_os",
    "echo_execution::sandbox::policy::SandboxPolicy::local_process",
    "echo_execution::sandbox::policy::SandboxPolicy::strict",
    "echo_execution::skills::external::types::SkillDescriptor::catalog_line",
    "echo_execution::skills::external::types::SkillDescriptor::matches_context_path",
    "echo_execution::skills::external::types::SkillDescriptor::permits_tool",
    "echo_execution::skills::external::types::SkillDescriptor::validate_name",
    "echo_execution::skills::external::types::SkillDescriptor::validate_paths",
    "echo_execution::skills::external::types::SkillSandboxPolicy::is_constraining",
    "echo_execution::skills::external::types::SkillSandboxPolicy::network_allowed",
    "echo_orchestration::human_loop::classifier::ClassifierContext::new",
    "echo_orchestration::human_loop::classifier::ClassifierContext::with_allow_rules",
    "echo_orchestration::human_loop::classifier::ClassifierContext::with_messages",
    "echo_orchestration::human_loop::classifier::ClassifierContext::with_project_type",
    "echo_orchestration::human_loop::classifier::ClassifierContext::with_recent_files",
    "echo_orchestration::human_loop::classifier::ClassifierContext::with_risk_context",
    "echo_orchestration::human_loop::classifier::ClassifierContext::with_soft_deny_rules",
    "echo_orchestration::human_loop::classifier::ClassifierContext::with_workspace_path",
    "echo_orchestration::human_loop::classifier::ClassifierResult::allow",
    "echo_orchestration::human_loop::classifier::ClassifierResult::block",
    "echo_orchestration::human_loop::classifier::ClassifierResult::with_confidence",
    "echo_orchestration::human_loop::permission::RiskLevel::color",
    "echo_orchestration::human_loop::permission::RiskLevel::from_permissions",
    "echo_orchestration::human_loop::permission::RiskLevel::icon",
    "echo_orchestration::human_loop::permission::RiskLevel::requires_confirmation",
    "echo_orchestration::human_loop::permission::Suggestion::allow_always",
    "echo_orchestration::human_loop::permission::Suggestion::allow_for_session",
    "echo_orchestration::human_loop::permission::Suggestion::allow_once",
    "echo_orchestration::human_loop::permission::Suggestion::deny_always",
    "echo_orchestration::human_loop::permission::Suggestion::deny_once",
    "echo_orchestration::human_loop::permission::Suggestion::modify_input",
    "echo_orchestration::human_loop::permission::Suggestion::recommended",
    "echo_orchestration::human_loop::permission::Suggestion::with_shortcut",
    "echo_orchestration::human_loop::permission::PermissionRequest::new",
    "echo_orchestration::human_loop::permission::PermissionRequest::requires_confirmation",
    "echo_orchestration::human_loop::permission::PermissionRequest::with_agent_name",
    "echo_orchestration::human_loop::permission::PermissionRequest::with_context",
    "echo_orchestration::human_loop::permission::PermissionRequest::with_default_suggestions",
    "echo_orchestration::human_loop::permission::PermissionRequest::with_permissions",
    "echo_orchestration::human_loop::permission::PermissionRequest::with_prompt",
    "echo_orchestration::human_loop::permission::PermissionRequest::with_request_id",
    "echo_orchestration::human_loop::permission::PermissionRequest::with_risk_based_suggestions",
    "echo_orchestration::human_loop::permission::PermissionRequest::with_risk_level",
    "echo_orchestration::human_loop::permission::PermissionRequest::with_session_id",
    "echo_orchestration::human_loop::permission::PermissionRequest::with_suggestion",
    "echo_orchestration::human_loop::permission::PermissionRequest::with_suggestions",
    "echo_orchestration::human_loop::permission::PermissionRequest::with_timeout",
    "echo_orchestration::human_loop::permission::PermissionResponse::allowed",
    "echo_orchestration::human_loop::permission::PermissionResponse::denied",
    "echo_orchestration::human_loop::permission::PermissionResponse::from_suggestion",
    "echo_orchestration::human_loop::permission::PermissionResponse::with_feedback",
    "echo_orchestration::human_loop::permission::PermissionUpdate::add_deny_rule",
    "echo_orchestration::human_loop::permission::PermissionUpdate::add_permanent_rule",
    "echo_orchestration::human_loop::permission::PermissionUpdate::add_session_rule",
    "echo_agent::agent::react::ReactAgent",
    "echo_agent::tasks::register_task_tools",
    "echo_agent::tools::is_read_tool",
    "echo_agent::tools::is_write_tool",
    "echo_core::agent::types::critique_output_schema",
    "echo_core::error::AgentFailure::message",
    "echo_core::retry::with_retry",
    "echo_core::retry::with_retry_if",
    "echo_core::tokenizer::HeuristicTokenizer",
    "echo_core::tokenizer::SimpleTokenizer",
    "echo_orchestration::human_loop::HumanLoopKind",
    "echo_orchestration::human_loop::default_provider",
    "echo_orchestration::human_loop::dispatch_event",
    "echo_orchestration::runtime::turn_driver::SinkControl",
    "echo_execution::skills::registry::SharedRegistry",
    "echo_execution::skills::registry::SkillRegistry",
    "echo_execution::skills::registry::SkillRegistry::new",
    "echo_execution::skills::registry::SkillRegistry::set_sandbox_manager",
    "echo_execution::skills::registry::shared_registry",
];

/// Exact value types whose methods only construct, inspect or transform the
/// receiver value. The prefix match stops at the type segment; unlike the old
/// namespace blanket it cannot absorb neighboring I/O services or algorithms.
const PROVEN_LANGUAGE_LOCAL_VALUE_TYPES: &[&str] = &[
    "echo_core::agent::AgentInvocationContext",
    "echo_core::agent::AgentSteerState",
    "echo_core::agent::AgentSteerTurnOutcome",
    "echo_core::agent::ToolVisibilityPolicy",
    "echo_agent::agent::react::run::pipeline::AuditStage",
    "echo_agent::agent::react::run::pipeline::CallbackPhase",
    "echo_agent::agent::react::run::pipeline::CallbackStage",
    "echo_agent::agent::react::run::pipeline::ExecuteStage",
    "echo_agent::agent::react::run::pipeline::InterventionStage",
    "echo_agent::agent::react::run::pipeline::InvocationStage",
    "echo_agent::agent::react::run::pipeline::OutputGuardStage",
    "echo_agent::agent::react::run::pipeline::PermissionStage",
    "echo_agent::agent::react::run::pipeline::PlanModeStage",
    "echo_agent::agent::react::run::pipeline::PostToolUseHookStage",
    "echo_agent::agent::react::run::pipeline::PreToolUseHookStage",
    "echo_agent::agent::react::run::pipeline::ReadBeforeEditStage",
    "echo_agent::agent::react::run::pipeline::SkillPermissionStage",
    "echo_agent::agent::react::run::pipeline::ToolVisibilityStage",
    "echo_agent::agent::react::run::pipeline::TraceRecordingStage",
    "echo_agent::agent::react::run::pipeline::TruncationStage",
    "echo_agent::evolution::audit::ChangeRecordOutcome",
    "echo_agent::evolution::audit::NullChangeLog",
    "echo_agent::evolution::dreaming::DreamingAction",
    "echo_agent::evolution::patch::SkillPatch",
    "echo_agent::evolution::triggers::MemoryTriggerDisposition",
    "echo_agent::headless::HeadlessConfig",
    "echo_agent::headless::HeadlessResult",
    "echo_agent::plugin::prepared::PluginDiagnosticSeverity",
    "echo_agent::plugin::prepared::PluginWiringResult",
    "echo_core::hooks::types::HookContext",
    "echo_core::hooks::types::HookEvent",
    "echo_core::hooks::types::HookEventCategory",
    "echo_core::hooks::types::HookResult",
    "echo_core::hooks::types::SubagentStopStatus",
    "echo_core::hooks::types::TaskTerminalStatus",
    "echo_core::llm::ChatRequest",
    "echo_core::llm::ChatResponse",
    "echo_core::llm::LlmApiProtocol",
    "echo_core::llm::ModelInputModality",
    "echo_core::llm::SimpleChatOptions",
    "echo_core::llm::cache::layout::PromptCacheLayout",
    "echo_core::llm::cache::layout::SegmentRange",
    "echo_core::llm::capabilities::ThinkingProfile",
    "echo_core::llm::thinking::ThinkingLevel",
    "echo_core::llm::thinking::ThinkingProtocol",
    "echo_core::llm::types::ResponseFormat",
    "echo_core::llm::types::ToolDefinition",
    "echo_core::llm::types::Usage",
    "echo_core::plugin::scope::PluginScope",
    "echo_core::sandbox::ExecutionResult",
    "echo_core::tools::ExternalRunContext",
    "echo_core::tools::InvocationResourceGuard",
    "echo_core::tools::NestedDelegationPolicy",
    "echo_core::tools::ParamValue",
    "echo_core::tools::ScriptExecutionProfile",
    "echo_core::tools::SubagentUplinkKind",
    "echo_core::tools::ToolAccess",
    "echo_core::tools::ToolCapabilities",
    "echo_core::tools::ToolContext",
    "echo_core::tools::ToolFailure",
    "echo_core::tools::ToolFailureCategory",
    "echo_core::tools::ToolRecoveryAction",
    "echo_core::tools::ToolRiskLevel",
    "echo_core::tools::artifact::ToolOutputArtifactConfig",
    "echo_core::tools::artifact::ToolOutputArtifactIdentity",
    "echo_core::tools::cell::CommandCellArtifactStatus",
    "echo_core::tools::cell::CommandCellObservationLease",
    "echo_core::tools::cell::CommandCellPhase",
    "echo_core::tools::cell::CommandCellRequest",
    "echo_core::tools::cell::CommandCellTerminalCause",
    "echo_core::tools::control::ToolControlSnapshot",
    "echo_core::tools::pagination::PageInfo",
    "echo_core::tools::pagination::PageRequest",
    "echo_core::tools::permission::PermissionRule",
    "echo_core::tools::permission::RuleBehavior",
    "echo_core::tools::permission::RuleMatcher",
    "echo_core::tools::permission::RuleSource",
    "echo_execution::skills::dependency_probe::DepKind",
    "echo_execution::skills::dependency_probe::ProbeReport",
    "echo_execution::skills::external::prompt_exec::SkillSource",
    "echo_execution::skills::external::types::SkillContent",
    "echo_execution::skills::external::types::SkillDocument",
    "echo_execution::skills::external::validate::SkillValidationReport",
    "echo_execution::skills::hooks::HookAction",
    "echo_execution::skills::hooks::HooksDefinition",
    "echo_orchestration::human_loop::HumanLoopRequest",
    "echo_orchestration::human_loop::audit::PermissionAuditEntry",
    "echo_agent::a2a::types::A2AMessage",
    "echo_agent::a2a::types::A2ATaskStatus",
    "echo_agent::a2a::types::AgentCard",
    "echo_agent::a2a::types::AgentProvider",
    "echo_agent::a2a::types::AgentSkill",
    "echo_agent::a2a::types::TaskState",
    "echo_agent::a2a::auth::JwtClaims",
    "echo_agent::a2a::auth::JwtConfig",
    "echo_core::compression::CanonicalContext",
    "echo_core::compression::CompressionCheckpoint",
    "echo_core::compression::StructuredSummary",
    "echo_core::memory::scope::MemoryScope",
    "echo_core::memory::store::SearchQuery",
    "echo_core::memory::store::StoreItem",
    "echo_core::memory::types::MemoryMeta",
    "echo_core::memory::types::MemorySource",
    "echo_core::memory::types::MemoryType",
    "echo_core::memory::types::TypedMemoryValue",
    "echo_state::compression::CompressionMetrics",
    "echo_state::delivery::DeliveryEnvelope",
    "echo_state::delivery::DeliveryLedgerProjection",
    "echo_state::delivery::DeliverySettlement",
    "echo_state::delivery::DeliveryTransition",
    "echo_state::journal::CheckpointedApplyError",
    "echo_state::journal::Checkpoint",
    "echo_state::journal::CheckpointInfo",
    "echo_state::journal::JournalAppendError",
    "echo_state::journal::JournalAppendReceipt",
    "echo_state::journal::JournalBatchAppendError",
    "echo_state::journal::JournalBatchAppendReceipt",
    "echo_state::journal::JournalBatchPrepareError",
    "echo_state::journal::JournalRecord",
    "echo_state::journal::PreparedJournalBatch",
    "echo_state::memory::typed_store::MemoryFilter",
    "echo_agent::eval::EvalResult",
    "echo_agent::eval::trigger::TriggerAccuracy",
    "echo_agent::improve::RunCritique",
    "echo_agent::telemetry::Metrics",
    "echo_state::skill_telemetry::SkillTelemetry",
    "echo_agent::topology::TopologyNode",
    "echo_agent::trace::LlmContextBreakdown",
    "echo_agent::trace::Run",
    "echo_agent::trace::RunEvent",
    "echo_agent::trace::TokenUsage",
    "echo_core::project_rules::ResolvedInstructions",
    "echo_integration::channels::session::ChannelSessionInstance",
    "echo_integration::channels::session::ChannelSessionRotation",
    "echo_integration::channels::session::SessionConfig",
    "echo_integration::channels::types::InboundMessage",
    "echo_integration::channels::types::MessageAttachment",
    "echo_integration::channels::types::OutboundMessage",
    "echo_integration::channels::channels::feishu::long_poll::WsClientConfig",
    "echo_integration::channels::channels::feishu::proto::HeadersHelper",
    "echo_integration::channels::channels::feishu::proto::ProtoFrame",
    "echo_integration::lsp::config::LspConfig",
    "echo_integration::mcp::config_loader::McpConfigFile",
    "echo_integration::mcp::server_config::McpServerConfig",
    "echo_orchestration::workflow::checkpoint_store::Checkpoint",
    "echo_orchestration::workflow::checkpoint_store::CheckpointFilter",
    "echo_orchestration::workflow::graph::InterruptConfig",
    "echo_orchestration::workflow::graph::InterruptState",
    "echo_orchestration::workflow::pipelines::data_pipeline::DataPipelineConfig",
    "echo_orchestration::workflow::pipelines::data_pipeline::DataPipelineLanguage",
    "echo_orchestration::workflow::pipelines::writing_pipeline::WritingPipelineConfig",
    "echo_agent::agent::subagent::context::ContextInheritance",
    "echo_agent::agent::subagent::context::SubagentContext",
    "echo_agent::agent::subagent::control::SubagentAttemptIdentity",
    "echo_agent::agent::subagent::control::SubagentCommandIdentity",
    "echo_agent::agent::subagent::control::SubagentCommandPhase",
    "echo_agent::agent::subagent::control::SubagentMessageReceipt",
    "echo_agent::agent::subagent::events::SubagentInvocationIdentity",
    "echo_agent::agent::subagent::executor::DispatchRequest",
    "echo_agent::agent::subagent::isolation::IsolationError",
    "echo_agent::agent::subagent::prompt::CompiledSubagentInvocation",
    "echo_agent::agent::subagent::prompt::PromptDiagnostics",
    "echo_agent::agent::subagent::prompt::ToolCapabilitySnapshot",
    "echo_agent::agent::subagent::team::TeamStrategy",
    "echo_agent::agent::subagent::types::ObservedIsolation",
    "echo_agent::agent::subagent::types::SubagentDefinition",
    "echo_agent::agent::subagent::types::SubagentOutcome",
    "echo_agent::agent::subagent::types::SubagentResult",
    "echo_agent::agent::subagent::types::SubagentStatus",
    "echo_agent::agent::subagent::usage::LlmUsageStats",
    "echo_orchestration::tasks::background_state::BackgroundTaskState",
    "echo_orchestration::tasks::background_task::BackgroundTaskStatus",
    "echo_orchestration::tasks::command_cell::BackgroundCommandManagerConfig",
    "echo_orchestration::tasks::command_cell::CommandCellReservation",
    "echo_orchestration::tasks::events::TaskEvent",
    "echo_orchestration::tasks::progress::Phase",
    "echo_orchestration::tasks::progress::PhasePlan",
    "echo_orchestration::tasks::revisioned::TaskDraft",
    "echo_orchestration::tasks::revisioned::TaskSpecPatch",
    "echo_orchestration::tasks::runtime::DagExecutionState",
    "echo_orchestration::tasks::runtime::TaskClaim",
    "echo_orchestration::tasks::runtime::TaskExecution",
    "echo_orchestration::tasks::runtime::TaskSpec",
    "echo_orchestration::tasks::runtime::TaskStatus",
    "echo_orchestration::tasks::runtime::TaskSubagentContext",
    "echo_tools::files::artifact::ArtifactReadError",
    "echo_core::agent::event_envelope::EventId",
    "echo_core::agent::event_envelope::EventIdentity",
    "echo_core::agent::event_envelope::StreamId",
    "echo_agent::state::AgentCheckpoint",
    "echo_core::compression::CompressionInput",
    "echo_core::guard::GuardResult",
    "echo_core::guard::content::ContentGuardResult",
    "echo_integration::mcp::types::JsonRpcNotification",
    "echo_integration::mcp::types::JsonRpcRequest",
    "echo_integration::mcp::types::McpContent",
    "echo_orchestration::workflow::state::SharedState",
    "echo_state::compression::TokenBreakdown",
    "echo_state::delivery::DeliveryOutcome",
    "echo_state::delivery::DeliveryPhase",
    "echo_state::delivery::DeliveryRecord",
    "echo_state::memory::typed_store::TypedMemoryEntry",
];

/// Exact construction types whose executable behavior is reached through an
/// existing Agent, extension or tool-family authority. Language SDK builders
/// may expose their configuration shape, but never reimplement the Rust
/// execution algorithm.
const PROVEN_LANGUAGE_LOCAL_CONSTRUCTION_TYPES: &[&str] = &[
    "echo_core::agent::factory::AgentFactoryConfig",
    "echo_agent::agent::critic::review_tool::ReviewTool",
    "echo_agent::channels::AgentChannelHandler",
    "echo_agent::config::FrameworkConfig",
    "echo_agent::config::ModelConfig",
    "echo_agent::tools::builtin::human_in_loop::HumanInLoop",
    "echo_agent::tools::builtin::think::ThinkTool",
    "echo_agent::tools::lsp::LspDiagnosticsTool",
    "echo_agent::tools::lsp::LspFindReferencesTool",
    "echo_agent::tools::lsp::LspGotoDefinitionTool",
    "echo_agent::tools::lsp::LspHoverTool",
    "echo_agent::tools::lsp::LspStatusTool",
    "echo_core::agent::prompt_template::PromptTemplateManager",
    "echo_execution::tools::ToolSearchTool",
    "echo_orchestration::human_loop::permission::DefaultPermissionRequestHandler",
    "echo_state::audit::AuditCallback",
    "echo_tools::data_quality::ConsistencyCheckTool",
    "echo_tools::data_quality::MissingValueAnalysisTool",
    "echo_tools::data_quality::OutlierDetectionTool",
    "echo_tools::excel::ExcelInfoTool",
    "echo_tools::excel::ExcelLoadTool",
    "echo_tools::excel::ExcelProfileTool",
    "echo_tools::excel::ExcelReadTool",
    "echo_tools::excel::ExcelToCsvTool",
    "echo_tools::excel::ExcelWriteTool",
    "echo_tools::image::ViewImageTool",
    "echo_tools::pdf::PdfExtractTool",
    "echo_tools::pdf::PdfInfoTool",
    "echo_tools::skills::filesystem::FileSystemSkill",
    "echo_tools::skills::shell::ShellSkill",
    "echo_tools::text::TextExportTool",
    "echo_tools::text::TextProcessTool",
    "echo_tools::text::TextSearchTool",
    "echo_tools::text::TextStatsTool",
    "echo_tools::word::WordInfoTool",
    "echo_tools::word::WordReadTool",
    "echo_tools::word::WordStructureTool",
    "echo_tools::worktree_tool::EnterWorktreeTool",
    "echo_tools::worktree_tool::ExitWorktreeTool",
    "echo_tools::worktree_tool::ListWorktreesTool",
    "echo_agent::a2a::types::AgentCardBuilder",
    "echo_agent::workflow::dsl::StateGraph",
    "echo_agent::workflow::loader::WorkflowDefinition",
    "echo_core::guard::rule::RuleGuardBuilder",
    "echo_orchestration::workflow::concurrent::ConcurrentWorkflowBuilder",
    "echo_orchestration::workflow::dag::DagWorkflowBuilder",
    "echo_orchestration::workflow::graph::GraphBuilder",
    "echo_orchestration::workflow::sequential::SequentialWorkflowBuilder",
    "echo_tools::files::apply_patch::ApplyPatchTool",
    "echo_tools::files::code_search::CodeSearchTool",
    "echo_tools::files::diff::DiffTool",
    "echo_tools::files::files::AppendFileTool",
    "echo_tools::files::files::CreateFileTool",
    "echo_tools::files::files::DeleteFileTool",
    "echo_tools::files::files::ListDirTool",
    "echo_tools::files::files::MoveFileTool",
    "echo_tools::files::files::ReadFileTool",
    "echo_tools::files::files::UpdateFileTool",
    "echo_tools::files::files::WriteFileTool",
    "echo_tools::files::glob::GlobTool",
    "echo_tools::files::grep::GrepTool",
    "echo_tools::files::repo_map::RepoMapTool",
    "echo_tools::media::image_fetch::ImageFetchTool",
    "echo_tools::rag::RagIndexTool",
    "echo_tools::rag::RagSearchTool",
    "echo_tools::rag::RagStore",
    "echo_tools::shell::ShellTool",
    "echo_tools::web::fetch::WebFetchTool",
    "echo_tools::web::search::WebSearchTool",
    "echo_state::compression::ContextManagerBuilder",
    "echo_state::compression::compressor::hybrid::HybridCompressor",
    "echo_state::compression::compressor::hybrid::HybridCompressorBuilder",
    "echo_state::compression::compressor::summary::IncrementalSummaryCompressor",
    "echo_state::compression::compressor::summary::SummaryCompressor",
    "echo_state::compression::levels::AdaptiveCompressor",
    "echo_state::compression::compressor::sliding_window::SlidingWindowCompressor",
    "echo_state::compression::horizon::VisibilityHorizonCompressor",
    "echo_agent::a2a::client::A2AClient",
    "echo_integration::channels::types::ChannelContext",
    "echo_integration::lsp::client::StdioLspClient",
    "echo_integration::mcp::config_loader::McpServerEntry",
    "echo_integration::mcp::resource_tool::McpResourceTool",
    "echo_integration::mcp::tool_adapter::McpToolAdapter",
    "echo_integration::mcp::transport::http::HttpTransport",
    "echo_integration::mcp::transport::sse::SseTransport",
    "echo_integration::mcp::transport::stdio::StdioTransport",
    "echo_tools::shell::StandardCommandPolicy",
    "echo_tools::web::providers::brave::BraveSearchProvider",
    "echo_tools::web::providers::duckduckgo::DuckDuckGoProvider",
    "echo_tools::web::providers::tavily::TavilyProvider",
];

/// Bare process object identities and constructors. Only the listed identity
/// is local; methods on the object remain routable and cannot inherit this
/// classification accidentally.
const PROVEN_LANGUAGE_LOCAL_CONSTRUCTION_IDENTITIES: &[&str] = &[
    "echo_agent::evolution::merge::SkillSimilarityDetector",
    "echo_agent::evolution::merge::SkillSimilarityDetector::new",
    "echo_agent::evolution::review::StalenessScorer",
    "echo_agent::evolution::review::StalenessScorer::new",
    "echo_agent::evolution::security::PromptInjectionDetector",
    "echo_agent::evolution::security::SecretScanner",
    "echo_agent::evolution::security::SecretScanner::new",
    "echo_agent::evolution::triggers::TriggerDetector",
    "echo_agent::evolution::triggers::TriggerDetector::new",
    "echo_agent::evolution::triggers::TriggerDetector::with_max_per_turn",
    "echo_agent::intent::classifier::KeywordClassifier",
    "echo_agent::intent::classifier::KeywordClassifier::new",
    "echo_agent::intent::classifier::LlmIntentClassifier",
    "echo_agent::intent::classifier::LlmIntentClassifier::new",
    "echo_execution::skills::external::activate_tool::ActivateSkillTool",
    "echo_execution::skills::external::activate_tool::ActivateSkillTool::new",
    "echo_execution::skills::external::loader::SkillLoader",
    "echo_execution::skills::external::loader::SkillLoader::new",
    "echo_execution::skills::external::resource_tool::ReadSkillResourceTool",
    "echo_execution::skills::external::resource_tool::ReadSkillResourceTool::new",
    "echo_execution::skills::external::resource_tool::ReadSkillResourceTool::with_max_bytes",
    "echo_execution::skills::external::run_script_tool::RunSkillScriptTool",
    "echo_execution::skills::external::run_script_tool::RunSkillScriptTool::new",
    "echo_execution::skills::external::run_script_tool::RunSkillScriptTool::with_sandbox_manager",
    "echo_execution::skills::external::run_script_tool::RunSkillScriptTool::with_timeout",
    "echo_execution::skills::hooks::HookRegistry",
    "echo_execution::skills::hooks::HookRegistry::new",
    "echo_orchestration::planning::validator::PlanValidator::new",
    "echo_orchestration::runtime::turn_driver::AgentTurnDriver",
    "echo_tools::registry::register_all_tools",
    "echo_tools::registry::register_readonly_tools",
    "echo_agent::tools::lsp::register_lsp_tools",
    "echo_core::guard::content::ContentGuard::new",
    "echo_integration::mcp::resource_tool::build_mcp_resource_tools",
    "echo_integration::mcp::client::McpClient::new",
];

const PROVEN_LANGUAGE_LOCAL_PURE_OPERATIONS: &[&str] = &[
    "echo_core::llm::cache::diagnostic::prompt_cache_fingerprint",
    "echo_core::llm::cache::diagnostic::stable_prefix_hash",
    "echo_core::llm::capabilities::infer_context_window",
    "echo_core::llm::capabilities::resolve_thinking_profile",
    "echo_core::plugin::manifest::PluginManifest::is_valid",
    "echo_core::tools::artifact::artifact_scope_component",
    "echo_core::tools::skill::is_path_safe",
    "echo_core::tools::skill::minimal_env",
    "echo_core::tools::skill::minimal_hook_env",
    "echo_core::tools::skill::minimal_hook_env_with_context",
    "echo_execution::sandbox::default_language_image",
    "echo_execution::sandbox::select_image_for_command",
    "echo_execution::skills::dependency_probe::extract_dependencies",
    "echo_execution::skills::external::types::is_skill_control_tool",
    "echo_execution::skills::external::types::skill_allows_tool",
    "echo_execution::skills::external::types::tool_matcher",
    "echo_execution::skills::external::validate::validate_skill_dir",
    "echo_execution::skills::external::validate::validate_skill_markdown",
    "echo_integration::providers::config::resolve_protocol_endpoint",
    "echo_tools::files::apply_patch::existing_file_paths",
    "echo_agent::agent::subagent::types::parse_json_objects",
    "echo_agent::agent::subagent::types::parse_subagent_outcome",
    "echo_agent::agent::subagent::types::render_result_contract",
    "echo_agent::agent::subagent::types::split_subagent_output",
    "echo_core::agent::event_envelope::validate_envelope_trajectory",
    "echo_core::agent::event_envelope::validate_event_trajectory",
    "echo_core::project_rules::inject_rules",
    "echo_core::project_rules::inject_rules_with_root",
    "echo_core::project_rules::rules_injection",
    "echo_core::project_rules::rules_injection_with_root",
    "echo_state::compression::compressor::summary::default_summary_prompt",
    "echo_state::compression::compressor::summary::default_summary_prompt_with_focus",
    "echo_state::compression::compressor::summary::structured_summary_prompt",
    "echo_state::compression::is_context_projection_message",
    "echo_state::compression::levels::tune_for_model",
    "echo_state::compression::verifier::verify_compression",
    "echo_tools::data::detect_format",
    "echo_tools::data::is_numeric",
    "echo_tools::shell::validate_command_safety",
    "echo_tools::web::providers::utils::percent_decode",
    "echo_tools::web::providers::utils::truncate_chars",
    "echo_tools::web::providers::utils::urlencode",
    "echo_integration::mcp::config_loader::validate_stdio_command",
    "echo_integration::mcp::client::McpClient::content_to_text",
];

const PROVEN_LANGUAGE_LOCAL_CLOCK_OPERATIONS: &[&str] = &[
    "echo_core::utils::time::local_rfc3339::serialize",
    "echo_core::utils::time::now_local",
    "echo_core::utils::time::now_millis",
    "echo_core::utils::time::now_secs",
    "echo_core::utils::time::option_local_rfc3339::serialize",
    "echo_core::utils::time::to_local",
];

const PROVEN_PROCESS_LOCAL_OPERATION_REASONS: &[(&str, &str)] = &[
    (
        "echo_agent::evolution::layer::is_stale_memory_proposal_error",
        "process-local-rust-error-classifier",
    ),
    (
        "echo_agent::evolution::merge::SkillSimilarityDetector::scan_and_propose",
        "process-local-store-and-callback-composition",
    ),
    (
        "echo_agent::headless::run_headless",
        "process-local-builder-closure",
    ),
    (
        "echo_execution::skills::dependency_probe::missing_binary_names",
        "process-local-toolchain-probe",
    ),
    (
        "echo_execution::skills::external::prompt_exec::find_git_bash_path",
        "process-local-shell-discovery",
    ),
    (
        "echo_execution::skills::external::prompt_exec::process_skill_content",
        "process-local-shell-and-sandbox-context",
    ),
    (
        "echo_execution::skills::registry::SkillRegistry::activation_handle",
        "process-local-skill-activation-authority",
    ),
    (
        "echo_execution::skills::registry::SkillRegistry::activation_view",
        "process-local-skill-activation-authority",
    ),
    (
        "echo_execution::skills::registry::SkillRegistry::restore_activation_state",
        "process-local-skill-activation-authority",
    ),
    (
        "echo_agent::a2a::auth::get_claims",
        "process-local-http-request-extension",
    ),
    (
        "echo_agent::a2a::serve::serve",
        "process-local-a2a-server-state",
    ),
    (
        "echo_agent::a2a::serve::serve_with_auth",
        "process-local-a2a-server-state",
    ),
    (
        "echo_agent::topology::TopologyCallback::new",
        "process-local-callback-construction",
    ),
    (
        "echo_integration::channels::manager::ChannelManager::register",
        "process-local-channel-plugin-ownership-transfer",
    ),
    (
        "echo_integration::channels::manager::ChannelManager::start_all",
        "process-local-channel-handler-factory",
    ),
    (
        "echo_agent::agent::react::ReactAgent::build_permission_service",
        "process-local-permission-service-construction",
    ),
    (
        "echo_agent::agent::react::ReactAgent::build_workspace_context_block",
        "language-local-agent-context-value",
    ),
    (
        "echo_agent::eval::runner::EvalRunner::run",
        "process-local-agent-consumer-parameter",
    ),
    (
        "echo_agent::workflow::loader::load_graph_from_json",
        "process-local-workflow-agent-construction",
    ),
    (
        "echo_agent::workflow::loader::load_graph_from_json_str",
        "process-local-workflow-agent-construction",
    ),
    (
        "echo_agent::workflow::loader::load_graph_from_yaml",
        "process-local-workflow-agent-construction",
    ),
    (
        "echo_agent::workflow::loader::load_graph_from_yaml_str",
        "process-local-workflow-agent-construction",
    ),
    (
        "echo_integration::mcp::types::NotificationReceiver::recv",
        "process-local-channel-receiver",
    ),
    (
        "echo_integration::mcp::types::NotificationReceiver::new",
        "process-local-channel-receiver-construction",
    ),
    (
        "echo_orchestration::workflow::pipelines::data_pipeline::run_data_pipeline",
        "process-local-workflow-agent-construction",
    ),
    (
        "echo_orchestration::workflow::pipelines::writing_pipeline::run_writing_pipeline",
        "process-local-workflow-agent-construction",
    ),
    (
        "echo_tools::data::load_dataframe",
        "process-local-polars-dataframe",
    ),
    (
        "echo_tools::git_worktree::create_worktree_with_context",
        "process-local-tool-invocation-context",
    ),
    (
        "echo_tools::git_worktree::list_worktrees_with_context",
        "process-local-tool-invocation-context",
    ),
    (
        "echo_tools::git_worktree::merge_worktree_with_context",
        "process-local-tool-invocation-context",
    ),
    (
        "echo_tools::git_worktree::remove_worktree_with_context",
        "process-local-tool-invocation-context",
    ),
    (
        "echo_orchestration::workflow::graph::Graph::set_max_steps",
        "language-local-workflow-definition-max-steps",
    ),
    (
        "echo_orchestration::workflow::graph::Graph::with_cancel_token",
        "host-owned-workflow-cancellation-resource",
    ),
    (
        "echo_orchestration::workflow::graph::Graph::with_checkpoint_store",
        "host-owned-workflow-checkpoint-resource",
    ),
];

const PROVEN_PROCESS_LOCAL_OPERATION_TYPES: &[(&str, &str)] = &[
    (
        "echo_execution::skills::registry::SkillActivationHandle",
        "process-local-skill-activation-authority",
    ),
    (
        "echo_core::agent::AgentSteerReceipt",
        "process-local-turn-input-receipt-state",
    ),
    (
        "echo_agent::agent::react::capabilities::PreparedAgentModelDeactivation",
        "process-local-model-mutation-transaction",
    ),
    (
        "echo_agent::agent::react::capabilities::PreparedAgentModelGeneration",
        "process-local-model-mutation-transaction",
    ),
    (
        "echo_agent::agent::react::capabilities::PreparedTokenLimit",
        "process-local-model-mutation-transaction",
    ),
    (
        "echo_agent::agent::react::run::pipeline::ToolExecutionPipeline",
        "process-local-tool-execution-pipeline",
    ),
    (
        "echo_agent::intent::classifier::ChainedClassifier",
        "process-local-intent-classifier-state",
    ),
    (
        "echo_agent::intent::IntentRouter",
        "process-local-intent-router-state",
    ),
    (
        "echo_agent::intent::trigger_supervisor::TriggerSupervisor",
        "process-local-intent-trigger-state",
    ),
    (
        "echo_core::circuit_breaker::CircuitBreaker",
        "process-local-circuit-breaker-state",
    ),
    (
        "echo_state::profiles::ProfileStore",
        "process-local-profile-filesystem-backend",
    ),
    (
        "echo_core::tokenizer::CalibratedTokenizer",
        "process-local-tokenizer-calibration-state",
    ),
    (
        "echo_core::tokenizer::TokenUsageTracker",
        "process-local-token-usage-state",
    ),
    (
        "echo_tools::registry::StandardToolPack",
        "process-local-tool-registry-composition",
    ),
    (
        "echo_core::tools::ToolVisibilityState",
        "process-local-tool-visibility-state",
    ),
    (
        "echo_core::tools::artifact::ToolOutputArtifactWriter",
        "process-local-artifact-writer-state",
    ),
    (
        "echo_agent::intent::classifier::KeywordClassifier",
        "process-local-intent-classifier-state",
    ),
    (
        "echo_agent::intent::classifier::KeywordClassifierConfig",
        "process-local-intent-classifier-state",
    ),
    (
        "echo_execution::skills::external::loader::SkillLoader",
        "process-local-filesystem-discovery-state",
    ),
    (
        "echo_execution::skills::hooks::HookRegistry",
        "process-local-callback-registry",
    ),
    (
        "echo_agent::agent::subagent::builder::SubagentBuilder",
        "process-local-subagent-coordination-state",
    ),
    (
        "echo_agent::agent::subagent::control::SubagentControlRegistry",
        "process-local-subagent-coordination-state",
    ),
    (
        "echo_agent::agent::subagent::events::SubagentEventBus",
        "process-local-subagent-coordination-state",
    ),
    (
        "echo_agent::agent::subagent::events::SubagentEventPublisher",
        "process-local-subagent-coordination-state",
    ),
    (
        "echo_agent::agent::subagent::executor",
        "process-local-subagent-coordination-state",
    ),
    (
        "echo_agent::agent::subagent::executor::BackgroundSubagentHandle",
        "process-local-subagent-coordination-state",
    ),
    (
        "echo_agent::agent::subagent::executor::SubagentExecutor",
        "process-local-subagent-coordination-state",
    ),
    (
        "echo_agent::agent::subagent::executor::TeammateHandle",
        "process-local-subagent-coordination-state",
    ),
    (
        "echo_agent::agent::subagent::hooks::SubagentHookRegistry",
        "process-local-subagent-coordination-state",
    ),
    (
        "echo_agent::agent::subagent::prompt",
        "process-local-subagent-coordination-state",
    ),
    (
        "echo_agent::agent::subagent::registry::FnAgentFactory",
        "process-local-subagent-coordination-state",
    ),
    (
        "echo_agent::agent::subagent::registry::SubagentRegistry",
        "process-local-subagent-coordination-state",
    ),
    (
        "echo_agent::agent::subagent::team",
        "process-local-subagent-coordination-state",
    ),
    (
        "echo_agent::agent::subagent::team::Team",
        "process-local-subagent-coordination-state",
    ),
    (
        "echo_agent::agent::subagent::team::TeamAgent",
        "process-local-subagent-coordination-state",
    ),
    (
        "echo_agent::agent::subagent::team::TeamAgentBuilder",
        "process-local-subagent-coordination-state",
    ),
    (
        "echo_orchestration::tasks::background_task::BackgroundTask",
        "process-local-task-coordination-state",
    ),
    (
        "echo_orchestration::tasks::background_task::TaskSpawner",
        "process-local-task-coordination-state",
    ),
    (
        "echo_orchestration::tasks::command_cell::BackgroundCommandManager",
        "process-local-task-coordination-state",
    ),
    (
        "echo_orchestration::tasks::command_cell::CommandCellWatcher",
        "process-local-task-coordination-state",
    ),
    (
        "echo_orchestration::tasks::events::TaskEventBus",
        "process-local-task-coordination-state",
    ),
    (
        "echo_orchestration::tasks::progress::ProgressReporter",
        "process-local-task-coordination-state",
    ),
    (
        "echo_orchestration::tasks::revisioned::DefaultTaskToolPolicy",
        "process-local-task-coordination-state",
    ),
    (
        "echo_orchestration::tasks::revisioned::InMemoryRevisionedTaskStore",
        "process-local-task-coordination-state",
    ),
    (
        "echo_orchestration::tasks::revisioned::TaskPatchEngine",
        "process-local-task-coordination-state",
    ),
    (
        "echo_orchestration::tasks::revisioned::TaskRevisionService",
        "process-local-task-coordination-state",
    ),
    (
        "echo_orchestration::tasks::runtime_service",
        "process-local-task-coordination-state",
    ),
    (
        "echo_orchestration::tasks::task_tools",
        "process-local-task-coordination-state",
    ),
    (
        "echo_state::compression::ContextManager",
        "process-local-agent-context-state",
    ),
    (
        "echo_state::memory::snapshot::SnapshotManager",
        "process-local-agent-snapshot-state",
    ),
    (
        "echo_state::delivery::DeliveryLedger",
        "process-local-generic-delivery-transaction",
    ),
    (
        "echo_integration::mcp::server::McpServer",
        "process-local-integration-server-state",
    ),
    (
        "echo_integration::mcp::server::McpServerBuilder",
        "process-local-integration-server-state",
    ),
    (
        "echo_state::journal::CheckpointedReducer",
        "process-local-generic-journal-state",
    ),
    (
        "echo_state::journal::segmented::SegmentedFileEventJournal",
        "process-local-generic-journal-state",
    ),
    (
        "echo_state::journal::file::FileEventJournal",
        "process-local-generic-journal-state",
    ),
    (
        "echo_state::journal::file::FileCheckpointStore",
        "process-local-generic-journal-state",
    ),
    (
        "echo_state::journal::MemoryEventJournal",
        "process-local-generic-journal-state",
    ),
    (
        "echo_state::journal::MemoryCheckpointStore",
        "process-local-generic-journal-state",
    ),
    (
        "echo_state::memory::typed_store::TypedMemoryStore",
        "process-local-memory-backend-state",
    ),
    (
        "echo_state::memory::embedding_store::EmbeddingStore",
        "process-local-memory-backend-state",
    ),
    (
        "echo_state::memory::embedder::HttpEmbedder",
        "process-local-memory-backend-state",
    ),
    (
        "echo_state::memory::store::FileStore",
        "process-local-memory-backend-state",
    ),
    (
        "echo_state::memory::store::InMemoryStore",
        "process-local-memory-backend-state",
    ),
    (
        "echo_state::memory::sqlite_store::SqliteStore",
        "process-local-memory-backend-state",
    ),
    (
        "echo_state::memory::conversation",
        "process-local-conversation-backend-state",
    ),
    (
        "echo_agent::trace::analyzer::TraceAnalyzer",
        "process-local-trace-analysis-state",
    ),
    (
        "echo_agent::trace::JsonlRunStore",
        "process-local-trace-backend-state",
    ),
    (
        "echo_agent::trace::InMemoryRunStore",
        "process-local-trace-backend-state",
    ),
    (
        "echo_orchestration::workflow::concurrent::ConcurrentWorkflow",
        "process-local-workflow-closure-state",
    ),
    (
        "echo_orchestration::workflow::dag::DagWorkflow",
        "process-local-workflow-closure-state",
    ),
    (
        "echo_orchestration::workflow::sequential::SequentialWorkflow",
        "process-local-workflow-closure-state",
    ),
    (
        "echo_orchestration::workflow::shared_agent",
        "process-local-workflow-agent-state",
    ),
    (
        "echo_orchestration::workflow::checkpoint_store",
        "process-local-workflow-checkpoint-state",
    ),
    ("echo_agent::eval", "process-local-evaluation-service-state"),
    (
        "echo_agent::improve",
        "process-local-improvement-service-state",
    ),
    (
        "echo_integration::channels::channels",
        "process-local-channel-transport-state",
    ),
    (
        "echo_integration::channels::manager::ChannelManager",
        "process-local-channel-manager-state",
    ),
    (
        "echo_integration::channels::session",
        "process-local-channel-session-state",
    ),
    (
        "echo_tools::research::clients",
        "process-local-http-transport-type",
    ),
    (
        "echo_core::guard::GuardManager",
        "process-local-guard-registry-state",
    ),
    (
        "echo_core::guard::llm::LlmGuard",
        "process-local-guard-provider-state",
    ),
    (
        "echo_agent::a2a::server::A2AServer",
        "process-local-a2a-server-state",
    ),
    (
        "echo_agent::agent::react::structured::StructuredAgent",
        "process-local-generic-structured-agent",
    ),
    (
        "echo_agent::memory_promoter::StoreMemoryPromoter",
        "process-local-memory-promoter-state",
    ),
    (
        "echo_agent::state::file::FileRuntimeStateStore",
        "process-local-runtime-state-backend",
    ),
    (
        "echo_agent::state::sqlite::SqliteRuntimeStateStore",
        "process-local-runtime-state-backend",
    ),
    (
        "echo_state::memory::file_conversation::FileConversationStore",
        "process-local-conversation-backend-state",
    ),
    (
        "echo_state::memory::sqlite_conversation::SqliteConversationStore",
        "process-local-conversation-backend-state",
    ),
    (
        "echo_state::skill_telemetry::SkillTelemetryStore",
        "process-local-skill-telemetry-backend",
    ),
];

const PROVEN_PROCESS_LOCAL_HTTP_OPERATIONS: &[&str] = &[
    "echo_tools::security::create_safe_http_client",
    "echo_tools::security::create_safe_regex",
    "echo_tools::security::local_http_get",
    "echo_tools::security::local_http_request",
    "echo_tools::security::ssrf_safe_get",
    "echo_tools::security::ssrf_safe_redirect_policy",
    "echo_tools::security::ssrf_safe_request",
    "echo_tools::security::ssrf_safe_request_with_body",
    "echo_tools::security::validate_url",
    "echo_tools::security::validate_url_with_addrs",
];

/// ReactAgent accessors that expose process-local service objects, mutexes or
/// builder/configuration state. These are individually named rather than
/// treating the entire ReactAgent impl as intrinsic: operations that can use
/// the Session Agent authority stay source routes, while these process-local
/// seams require an in-process implementation or language-native helper.
const PROVEN_REACT_AGENT_LOCAL_METHODS: &[&str] = &[
    "add_callback",
    "add_intervention_callback",
    "add_need_appeal_tool",
    "add_skill",
    "add_skills",
    "add_tool",
    "add_tools",
    "calibrated_tokenizer",
    "chat_multimodal",
    "chat_with_image_url",
    "config",
    "config_mut",
    "connect_mcp_from_config",
    "context",
    "conversation_store",
    "create_subagent_hook_bridge",
    "create_task_hook_bridge",
    "critic_owner",
    "execution_mutex",
    "execute_typed",
    "execute_with_image_url",
    "fire_lifecycle_hook",
    "force_compress_with",
    "force_compress_with_hooks",
    "extract",
    "extract_json",
    "hook_activation_cache",
    "hook_registry",
    "install_canonical_context",
    "install_memory_layer_manager",
    "install_memory_store",
    "install_store",
    "llm_client",
    "llm_config",
    "memory_layer_manager",
    "permission_service",
    "register_agent",
    "register_agents",
    "register_prepared_skill",
    "register_mcp_tools",
    "register_prepared_plugin_skills",
    "register_skill_descriptor",
    "register_subagent_definition",
    "register_subagent_factory",
    "register_subagent_with_definition",
    "replace_tool",
    "remove_callbacks_by_type_name",
    "remove_callbacks_by_type_name_and_id",
    // The concrete ReactAgent API returns an opaque Box<dyn Tool>; only the
    // core Agent trait's boolean removal projection is wire-adaptable.
    "remove_tool",
    "record_code_skill_info",
    "replace_system_context_projection",
    "run_store",
    "sandbox_manager",
    "skill_registry",
    "state_store",
    "subagent_executor",
    "subagent_registry",
    "tag_skills_source_with_variables",
    "set_approval_provider",
    "set_canonical_context",
    "set_circuit_breaker",
    "set_critic",
    "set_guard_manager",
    "set_hook_registry",
    "set_human_loop_provider",
    "set_human_loop_provider_preserving_approvals",
    "set_intent_router",
    "set_llm_client",
    "set_llm_config",
    "set_memory_store",
    "set_owned_critic",
    "set_permission_service",
    "set_sandbox_manager",
    "set_skill_curator",
    "set_snapshot_manager",
    "set_subagent_admission",
    "set_token_tracker",
    "set_tool_execution_pipeline",
    "set_tool_manager",
    "set_tool_output_artifacts",
    "set_task_revision_service",
    "task_revision_service",
    "token_tracker",
    "tool_execution_pipeline",
    "tool_manager",
    "tool_output_artifacts",
    "setup_hook_mcp_executor",
    "sync_subagent_dispatch_catalog",
    "store",
    "new",
    "unregister_subagent",
    "with_llm_client",
    "with_llm_config",
];

/// How one facade item is served over the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanonicalRoute {
    /// Losslessly served by a stable ACP v1 method (prompt resources, the
    /// ACP session context projection).
    Standard { method: &'static str },
    /// Served by a typed core-profile method family (task/subagent/…).
    Core { family: FacadeFamily },
    /// Served by a feature-family operation surface (`<family>/op`).
    Family { family: FacadeFamily },
    /// One exact Rust source operation mapped to one closed family handler.
    FamilyOperation {
        family: FacadeFamily,
        source_operation: String,
        handler_operation: &'static str,
    },
    /// Consumer-implemented trait served through the reverse bridge.
    Bridge { kind: ExtensionKind },
    /// Exact facade identity served by the generic typed invoke method.
    Invoke { operation: String },
    /// Exact source identity that still needs a thin Rust authority adapter.
    /// It is kept distinct from the generic fallback family so route
    /// completeness cannot be mistaken for a wildcard handler.
    SourceOperation { operation: String },
    /// Serializable value carried by the extension schema, optionally
    /// owned by a family wire surface.
    Value { family: Option<FacadeFamily> },
    /// Process-local Rust mechanism; language SDKs provide native helpers.
    Intrinsic { reason: &'static str },
}

impl CanonicalRoute {
    /// Stable canonical route id. Contains no wildcards by construction;
    /// [`validate_facade_route_table`] and the manifest tests enforce this.
    pub fn route_id(&self) -> String {
        match self {
            Self::Standard { method } => format!("standard:{method}"),
            Self::Core { family } => format!("core:{}", family.as_str()),
            Self::Family { family } => format!("family:{}", family.as_str()),
            Self::FamilyOperation {
                family,
                source_operation,
                handler_operation,
            } => format!(
                "family-operation:{}:{handler_operation}:{source_operation}",
                family.as_str()
            ),
            Self::Bridge { kind } => format!("bridge:{}", kind.as_str()),
            Self::Invoke { operation } => format!("invoke:{operation}"),
            Self::SourceOperation { operation } => format!("source:{operation}"),
            Self::Value { family } => match family {
                Some(family) => format!("value:{}", family.as_str()),
                None => "value".to_string(),
            },
            Self::Intrinsic { reason } => format!("intrinsic:{reason}"),
        }
    }

    /// Wire surface name for the generated catalog (`surface` field).
    pub fn surface(&self) -> &'static str {
        match self {
            Self::Standard { .. } => "standard",
            Self::Core { .. } => "core",
            Self::Family { .. } => "family",
            Self::FamilyOperation { .. } => "invoke",
            Self::Bridge { .. } => "bridge",
            Self::Invoke { .. } => "invoke",
            Self::SourceOperation { .. } => "invoke",
            Self::Value { .. } => "value",
            Self::Intrinsic { .. } => "intrinsic",
        }
    }

    /// Primary wire method for the route, when it owns one.
    pub fn method(&self) -> Option<&'static str> {
        match self {
            Self::Standard { method } => Some(method),
            Self::Core { family } | Self::Family { family } => family.methods().first().copied(),
            Self::FamilyOperation { .. } => Some("_echo_agent/facade/invoke"),
            Self::Bridge { .. } => Some("_echo_agent/extension/register"),
            Self::Invoke { .. } => Some("_echo_agent/facade/invoke"),
            Self::SourceOperation { .. } => Some("_echo_agent/facade/invoke"),
            Self::Value { .. } | Self::Intrinsic { .. } => None,
        }
    }

    pub fn family(&self) -> Option<FacadeFamily> {
        match self {
            Self::Standard { .. } => Some(FacadeFamily::Standard),
            Self::Core { family } | Self::Family { family } => Some(*family),
            Self::FamilyOperation { family, .. } => Some(*family),
            Self::Bridge { .. } => Some(FacadeFamily::Bridge),
            Self::Invoke { .. } => Some(FacadeFamily::Invoke),
            Self::SourceOperation { .. } => Some(FacadeFamily::SourceOperation),
            Self::Value { family } => Some(family.unwrap_or(FacadeFamily::Value)),
            Self::Intrinsic { .. } => Some(FacadeFamily::Intrinsic),
        }
    }

    /// Exact operation identity for generic invocations; `None` elsewhere.
    pub fn operation(&self) -> Option<&str> {
        match self {
            Self::Invoke { operation } => Some(operation.as_str()),
            Self::SourceOperation { operation } => Some(operation.as_str()),
            Self::FamilyOperation {
                source_operation, ..
            } => Some(source_operation.as_str()),
            _ => None,
        }
    }

    pub fn handler_operation(&self) -> Option<&str> {
        match self {
            Self::FamilyOperation {
                handler_operation, ..
            } => Some(handler_operation),
            _ => None,
        }
    }
}

/// Longest-prefix source-identity match across the family table.
pub fn family_for_source(source: &str) -> Option<FacadeFamily> {
    let mut best: Option<(usize, FacadeFamily)> = None;
    for descriptor in FACADE_FAMILIES {
        for prefix in descriptor.source_prefixes {
            let matches = source == *prefix
                || source
                    .strip_prefix(prefix)
                    .is_some_and(|rest| rest.starts_with("::"));
            if matches {
                let better = best
                    .as_ref()
                    .is_none_or(|(length, _)| prefix.chars().count() > *length);
                if better {
                    best = Some((prefix.chars().count(), descriptor.family));
                }
            }
        }
    }
    best.map(|(_, family)| family)
}

fn family_operation_for_source(
    family: FacadeFamily,
    source_identity: &str,
) -> Option<&'static str> {
    if let Some(operation) = family
        .operations()
        .iter()
        .find(|operation| **operation == source_identity)
    {
        return Some(*operation);
    }
    let method = source_identity
        .rsplit_once("::")
        .map(|(_, method)| method)?;
    match family {
        FacadeFamily::A2a
            if source_identity.starts_with("echo_agent::a2a::client::A2AClient::") =>
        {
            match method {
                "discover" => Some("a2a.discover"),
                "send_task" | "send_task_with_session" => Some("a2a.task.send"),
                "send_task_streaming" | "send_task_streaming_with_session" => {
                    Some("a2a.task.stream.open")
                }
                "get_task" => Some("a2a.task.get"),
                "cancel_task" => Some("a2a.task.cancel"),
                _ => None,
            }
        }
        FacadeFamily::Workflow
            if source_identity.starts_with("echo_orchestration::workflow::graph::Graph::")
                || source_identity
                    .starts_with("echo_orchestration::workflow::state::SharedState::") =>
        {
            match method {
                "run" => Some("workflow.graph.run"),
                "run_stream" => Some("workflow.graph.run_stream"),
                "run_until_interrupt" => Some("workflow.graph.run_until_interrupt"),
                "resume" => Some("workflow.graph.resume_exact"),
                "resume_with_state" => Some("workflow.graph.resume_with_state"),
                "restore_to_checkpoint" => Some("workflow.graph.restore"),
                "branch_from" => Some("workflow.graph.branch"),
                "tag_checkpoint" => Some("workflow.graph.tag_checkpoint"),
                "list_checkpoints" => Some("workflow.graph.list_checkpoints"),
                "list_checkpoints_by_graph" => Some("workflow.graph.list_checkpoints_by_graph"),
                "load_checkpoint" => Some("workflow.graph.load_checkpoint"),
                "cancel" => Some("workflow.graph.cancel"),
                "new" if source_identity.contains("SharedState") => Some("workflow.state.new"),
                "get" if source_identity.contains("SharedState") => Some("workflow.state.get"),
                "insert" | "set" if source_identity.contains("SharedState") => {
                    Some("workflow.state.set")
                }
                "keys" if source_identity.contains("SharedState") => Some("workflow.state.keys"),
                "snapshot" if source_identity.contains("SharedState") => {
                    Some("workflow.state.snapshot")
                }
                _ => None,
            }
        }
        FacadeFamily::Permission
            if source_identity
                .starts_with("echo_orchestration::human_loop::service::PermissionService::") =>
        {
            match method {
                "mode" => Some("permission.mode"),
                "set_mode" => Some("permission.set_mode"),
                "check" => Some("permission.check"),
                "apply_update" => Some("permission.apply_update"),
                "apply_updates" => Some("permission.apply_updates"),
                "check_with_permissions" => Some("permission.check_with_permissions"),
                "check_with_permissions_in_mode" => {
                    Some("permission.check_with_permissions_in_mode")
                }
                "check_with_permissions_result_in_mode" => {
                    Some("permission.check_with_permissions_result_in_mode")
                }
                "check_with_permissions_result_in_mode_and_context" => {
                    Some("permission.check_with_permissions_result_in_mode_and_context")
                }
                "add_rule" => Some("permission.add_rule"),
                "add_rules" => Some("permission.add_rules"),
                "remove_rule" => Some("permission.remove_rule"),
                "clear_rules" => Some("permission.clear_rules"),
                "all_rules" => Some("permission.all_rules"),
                "is_approved" => Some("permission.is_approved"),
                "record_approval" => Some("permission.record_approval"),
                "cleanup_expired" => Some("permission.cleanup_expired"),
                "stats" => Some("permission.stats"),
                "revoke_cache" => Some("permission.revoke_cache"),
                "clear_cache" => Some("permission.clear_cache"),
                "would_request_human" => Some("permission.would_request_human"),
                "would_request_human_for_permissions" => Some("permission.would_request_human"),
                _ => None,
            }
        }
        FacadeFamily::Channels
            if source_identity
                .starts_with("echo_integration::channels::manager::ChannelManager::") =>
        {
            match method {
                "stop" => Some("channels.manager.stop_exact"),
                "stop_all" => Some("channels.manager.stop_all"),
                "channel_ids" => Some("channels.manager.ids"),
                "send" => Some("channels.plugin.send"),
                _ => None,
            }
        }
        FacadeFamily::Mcp
            if source_identity.starts_with("echo_integration::mcp::client::McpClient::")
                || source_identity.starts_with("echo_integration::mcp::McpManager::") =>
        {
            match method {
                "call_tool" => Some("echo_agent::mcp::McpClient::call_tool"),
                "close" => Some("echo_agent::mcp::McpClient::close"),
                "get_prompt" => Some("echo_agent::mcp::McpClient::get_prompt"),
                "list_resource_templates" => {
                    Some("echo_agent::mcp::McpClient::list_resource_templates")
                }
                "list_resources" => Some("echo_agent::mcp::McpClient::list_resources"),
                "ping" => Some("echo_agent::mcp::McpClient::ping"),
                "prompts" => Some("echo_agent::mcp::McpClient::prompts"),
                "protocol_version" => Some("echo_agent::mcp::McpClient::protocol_version"),
                "read_resource" => Some("echo_agent::mcp::McpClient::read_resource"),
                "resources" => Some("echo_agent::mcp::McpClient::resources"),
                "server_capabilities" => Some("echo_agent::mcp::McpClient::server_capabilities"),
                "server_name" => Some("echo_agent::mcp::McpClient::server_name"),
                "supports_prompts" => Some("echo_agent::mcp::McpClient::supports_prompts"),
                "supports_resources" => Some("echo_agent::mcp::McpClient::supports_resources"),
                "tools" => Some("echo_agent::mcp::McpClient::tools"),
                "new"
                    if source_identity
                        .starts_with("echo_integration::mcp::client::McpClient::") =>
                {
                    Some("mcp.client.open_exact")
                }
                "new" => Some("mcp.manager.open_exact"),
                "connect" => Some("mcp.server.connect_exact"),
                "connect_from_config" => Some("mcp.manager.connect_from_config"),
                "reconcile_target" => Some("mcp.manager.reconcile_target"),
                "get_all_tools" => Some("mcp.manager.get_all_tools"),
                "get_client" => Some("mcp.manager.get_client"),
                "get_clients" => Some("mcp.manager.get_clients"),
                "resource_tools" => Some("mcp.manager.resource_tools"),
                "server_names" => Some("mcp.manager.server_names"),
                "close_all" => Some("mcp.manager.close_all"),
                "list" | "list_servers" => Some("mcp.server.list"),
                "disconnect" => Some("mcp.server.disconnect"),
                _ => None,
            }
        }
        FacadeFamily::Delivery
            if source_identity.starts_with("echo_state::delivery::DeliveryLedger::") =>
        {
            match method {
                "enqueue" => Some("delivery.enqueue"),
                "claim_next" => Some("delivery.claim_next"),
                "transition" => Some("delivery.transition"),
                "defer" => Some("delivery.defer"),
                "settle" => Some("delivery.settle"),
                "recover" => Some("delivery.recover"),
                "snapshot" => Some("delivery.snapshot"),
                _ => None,
            }
        }
        FacadeFamily::Topology
            if source_identity.starts_with("echo_agent::topology::TopologyTracker::") =>
        {
            match method {
                "new" => Some("topology.tracker.open_exact"),
                "add_node" => Some("topology.node.add_exact"),
                "record_call" => Some("topology.call.record"),
                "record_call_with_duration" => Some("topology.call.record_with_duration"),
                "nodes" => Some("topology.nodes"),
                "edges" => Some("topology.edges"),
                "stats" => Some("topology.stats"),
                "clear" => Some("topology.clear"),
                "to_mermaid" => Some("topology.to_mermaid"),
                "to_json" => Some("topology.to_json"),
                "to_dot" => Some("topology.to_dot"),
                _ => None,
            }
        }
        FacadeFamily::Lsp
            if source_identity.starts_with("echo_integration::lsp::manager::LspManager::")
                || source_identity.starts_with("echo_core::lsp::client::LspClient::") =>
        {
            match method {
                "new" => Some("lsp.manager.open_exact"),
                "load_config" => Some("lsp.manager.load_config"),
                "set_project_root" => Some("lsp.manager.set_project_root"),
                "start_server" => Some("lsp.manager.start_server"),
                "stop_server" => Some("lsp.manager.stop_server"),
                "restart_server" => Some("lsp.manager.restart_server"),
                "shutdown_all" => Some("lsp.manager.shutdown_all"),
                "get_client" => Some("lsp.manager.get_client"),
                "get_client_for_file" => Some("lsp.manager.get_client_for_file"),
                "status_all" => Some("lsp.server.status_all"),
                "configured_languages" => Some("lsp.language.configured"),
                "running_servers" => Some("lsp.server.running"),
                "language" => Some("lsp.client.language"),
                "is_running" => Some("lsp.client.is_running"),
                "is_initialized" => Some("lsp.client.is_initialized"),
                "initialize" => Some("lsp.client.initialize"),
                "shutdown" => Some("lsp.client.shutdown"),
                "diagnostics" => Some("lsp.client.diagnostics"),
                "goto_definition" => Some("lsp.client.goto_definition"),
                "find_references" => Some("lsp.client.find_references"),
                "hover" => Some("lsp.client.hover"),
                "completion" => Some("lsp.client.completion"),
                "did_open" => Some("lsp.client.did_open"),
                "did_change" => Some("lsp.client.did_change"),
                "did_save" => Some("lsp.client.did_save"),
                "did_close" => Some("lsp.client.did_close"),
                "status" => Some("lsp.client.status"),
                _ => None,
            }
        }
        FacadeFamily::Trace if source_identity.starts_with("echo_agent::trace::RunStore::") => {
            match method {
                "save" => Some("trace.run.save"),
                "load" => Some("trace.run.load"),
                "list_by_session" => Some("trace.run.list_session"),
                "list_all" => Some("trace.run.list_recent"),
                _ => None,
            }
        }
        FacadeFamily::State if source_identity.starts_with("echo_state::RuntimeStateStore::") => {
            match method {
                "load" => Some("state.checkpoint.get"),
                "save" => Some("state.checkpoint.save"),
                "list_runtime_state_ids" => Some("state.runtime.list"),
                "clear_runtime_state" => Some("state.runtime.clear"),
                "clear_scope" => Some("state.scope.clear"),
                _ => None,
            }
        }
        FacadeFamily::Eval => None,
        FacadeFamily::Improve
            if source_identity.starts_with("echo_agent::improve::analyzer::Analyzer::")
                || source_identity.starts_with("echo_agent::improve::trajectory::") =>
        {
            match method {
                "analyze" => Some("improve.run.analyze"),
                "to_sharegpt" | "save_sharegpt" => Some("improve.trajectory.sharegpt"),
                _ => None,
            }
        }
        FacadeFamily::ContentGuard
            if source_identity.starts_with("echo_core::guard::content::ContentGuard::") =>
        {
            match method {
                "detect" => Some("content-guard.detect_exact"),
                "check" => Some("content-guard.check"),
                "redact" => Some("content-guard.redact_exact"),
                "is_clean" => Some("content-guard.is_clean_exact"),
                _ => None,
            }
        }
        FacadeFamily::ProjectRules
            if source_identity.starts_with("echo_core::project_rules::InstructionResolver::")
                || source_identity == "echo_core::project_rules::load_project_rules" =>
        {
            match method {
                "resolve" | "resolve_for_path" | "resolve_instructions" => {
                    Some("project-rules.resolve")
                }
                "load_project_rules" => Some("project-rules.load"),
                _ => None,
            }
        }
        FacadeFamily::Telemetry
            if matches!(
                source_identity,
                "echo_agent::telemetry::init_telemetry"
                    | "echo_agent::telemetry::shutdown_telemetry"
            ) =>
        {
            match method {
                "init_telemetry" => Some("telemetry.init"),
                "shutdown_telemetry" => Some("telemetry.shutdown"),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Typed extension-bridge kinds declared by [`TYPED_BRIDGE_TRAITS`].
/// Kinds without a trait identity here (only `Hook`, which registers by
/// event descriptor, not by trait) still have Host handlers; the closed-set
/// tests reconcile the manifest against this list.
pub fn typed_bridge_kinds() -> Vec<ExtensionKind> {
    TYPED_BRIDGE_TRAITS.iter().map(|(_, kind)| *kind).collect()
}

/// Typed bridge kind for a consumer trait, by canonical trait source
/// identity (exact match on the defining trait path).
fn typed_bridge_kind(source_identity: &str) -> Option<ExtensionKind> {
    TYPED_BRIDGE_TRAITS
        .iter()
        .find(|(trait_path, _)| {
            source_identity == *trait_path
                || source_identity
                    .strip_prefix(trait_path)
                    .is_some_and(|rest| rest.starts_with("::"))
        })
        .map(|(_, kind)| *kind)
}

fn process_local_trait_reason(source_identity: &str) -> Option<&'static str> {
    PROVEN_PROCESS_LOCAL_TRAITS
        .iter()
        .find(|(trait_path, _)| {
            source_identity == *trait_path
                || source_identity
                    .strip_prefix(trait_path)
                    .is_some_and(|rest| rest.starts_with("::"))
        })
        .map(|(_, reason)| *reason)
}

fn language_local_source_reason(source_identity: &str) -> Option<&'static str> {
    if let Some(method) = source_identity.strip_prefix("echo_agent::agent::react::ReactAgent::")
        && PROVEN_REACT_AGENT_LOCAL_METHODS.contains(&method)
    {
        return Some(match method {
            "chat_multimodal" | "chat_with_image_url" | "execute_with_image_url" => {
                "language-local-standard-acp-content-adapter"
            }
            "execute_typed" | "extract" | "extract_json" => {
                "language-local-generic-result-decoding"
            }
            "force_compress_with" | "force_compress_with_hooks" => {
                "process-local-consumer-parameter"
            }
            _ => "process-local-react-agent-composition",
        });
    }
    if source_identity == "echo_core::utils::fs::atomic_compare_and_swap" {
        return Some("process-local-callback-parameter");
    }
    if source_identity
        .strip_prefix("echo_agent::evolution::candidate::SkillCandidateDetector")
        .is_some_and(|rest| rest.is_empty() || rest.starts_with("::"))
    {
        return Some("process-local-store-curator-and-callback-composition");
    }
    if let Some((_, reason)) = PROVEN_PROCESS_LOCAL_OPERATION_REASONS
        .iter()
        .find(|(identity, _)| source_identity == *identity)
    {
        return Some(reason);
    }
    if PROVEN_LANGUAGE_LOCAL_PURE_OPERATIONS.contains(&source_identity) {
        return Some("language-local-pure-value-algorithm");
    }
    if PROVEN_LANGUAGE_LOCAL_CLOCK_OPERATIONS.contains(&source_identity) {
        return Some("language-local-platform-clock");
    }
    if PROVEN_PROCESS_LOCAL_HTTP_OPERATIONS.contains(&source_identity) {
        return Some("process-local-http-transport-type");
    }
    if [
        "echo_core::utils::fs::ExclusiveFileLease",
        "echo_core::utils::fs::ExistingDirectoryGuard",
        "echo_core::utils::fs::ExistingRegularFileGuard",
        "echo_core::utils::fs::FileDurability",
    ]
    .contains(&source_identity)
    {
        return Some("language-local-opaque-resource-type");
    }
    let belongs_to_exact_type = |types: &[&str]| {
        types.iter().any(|type_identity| {
            source_identity == *type_identity
                || source_identity
                    .strip_prefix(*type_identity)
                    .is_some_and(|rest| rest.starts_with("::"))
        })
    };
    if belongs_to_exact_type(PROVEN_LANGUAGE_LOCAL_VALUE_TYPES) {
        return Some("language-local-value-method");
    }
    if belongs_to_exact_type(PROVEN_LANGUAGE_LOCAL_CONSTRUCTION_TYPES) {
        return Some("language-local-authority-construction");
    }
    if let Some((_, reason)) =
        PROVEN_PROCESS_LOCAL_OPERATION_TYPES
            .iter()
            .find(|(type_identity, _)| {
                source_identity == *type_identity
                    || source_identity
                        .strip_prefix(*type_identity)
                        .is_some_and(|rest| rest.starts_with("::"))
            })
    {
        return Some(reason);
    }
    if PROVEN_LANGUAGE_LOCAL_CONSTRUCTION_IDENTITIES.contains(&source_identity) {
        return Some("language-local-authority-construction");
    }
    if PROVEN_LANGUAGE_LOCAL_EXACT_SOURCES.contains(&source_identity) {
        return Some("language-local-exact-helper");
    }
    let matched = PROVEN_LANGUAGE_LOCAL_SOURCES.iter().any(|prefix| {
        source_identity == *prefix
            || source_identity
                .strip_prefix(*prefix)
                .is_some_and(|rest| rest.starts_with("::"))
    });
    if !matched {
        return None;
    }
    if source_identity.starts_with("echo_agent::agent::config::")
        || source_identity.contains("::builder::")
    {
        Some("language-local-agent-config")
    } else if source_identity.contains("with_cancel")
        || source_identity.contains("invocation_context")
        || source_identity.ends_with("CancellationToken")
    {
        Some("language-local-cancellation")
    } else if source_identity.contains("external_context")
        || source_identity.ends_with("tool_visibility_policy")
    {
        Some("language-local-agent-context")
    } else if source_identity.starts_with("echo_core::tools::ToolResult")
        || source_identity.starts_with("echo_core::tools::ToolCallParams")
    {
        Some("language-local-wire-helper")
    } else if source_identity.starts_with("echo_orchestration::runtime::turn_driver::") {
        Some("language-local-turn-context")
    } else if source_identity.starts_with("echo_core::llm::")
        || source_identity.starts_with("echo_core::budget::")
        || source_identity.starts_with("echo_core::retry::")
    {
        Some("language-local-policy-config")
    } else if source_identity.starts_with("echo_integration::mcp::client::McpClient::refresh_") {
        Some("language-local-mcp-cache")
    } else if source_identity.starts_with("echo_agent::plugin::")
        || source_identity.starts_with("echo_core::plugin::")
    {
        Some("process-local-plugin-registry-state")
    } else if source_identity.starts_with("echo_agent::testing::") {
        Some("language-local-test-double")
    } else if source_identity.starts_with("echo_orchestration::human_loop::") {
        Some("process-local-human-loop-policy-state")
    } else if source_identity.starts_with("echo_orchestration::scheduler::") {
        Some("process-local-scheduler-state")
    } else if source_identity.starts_with("echo_execution::sandbox::")
        || source_identity.starts_with("echo_core::sandbox::")
    {
        Some("process-local-sandbox-runtime")
    } else if source_identity.starts_with("echo_agent::evolution::") {
        Some("process-local-evolution-service-state")
    } else if source_identity.starts_with("echo_execution::tools::")
        || source_identity.starts_with("echo_core::tools::control::")
        || source_identity.starts_with("echo_core::tools::permission::")
    {
        Some("process-local-tool-registry-state")
    } else if source_identity.starts_with("echo_agent::agent::handle::")
        || source_identity.starts_with("echo_core::agent::admission::")
        || source_identity.starts_with("tokio_util::sync::cancellation_token::")
    {
        Some("process-local-concurrency-handle")
    } else if source_identity.starts_with("echo_state::audit::") {
        Some("process-local-audit-backend")
    } else if source_identity.starts_with("echo_integration::providers::") {
        Some("process-local-llm-provider-transport")
    } else if source_identity.starts_with("echo_agent::agent::snapshot::") {
        Some("process-local-agent-snapshot-state")
    } else if source_identity.starts_with("echo_core::agent::critic::")
        || source_identity.starts_with("echo_agent::agent::critic::")
    {
        Some("process-local-critic-composition")
    } else if source_identity.starts_with("echo_agent::hooks_bridge::")
        || source_identity.starts_with("echo_agent::agent::callbacks::")
    {
        Some("process-local-callback-bridge-state")
    } else if source_identity.starts_with("echo_tools::security::") {
        Some("language-local-security-policy-value")
    } else {
        Some("process-local-explicit-type-seam")
    }
}

/// Canonical source identity of an inventory entry: the defining rustdoc
/// path when re-exported, else the facade path itself. Deterministic: the
/// lexicographically smallest source path wins when several exist.
pub fn canonical_source_identity(entry: &InventoryEntry) -> String {
    entry
        .source_paths
        .iter()
        .next()
        .cloned()
        .unwrap_or_else(|| entry.path.clone())
}

/// Resolve the one canonical route for an inventory entry. The decision
/// order is fixed: intrinsic → bridge traits → standard ACP projection →
/// source-identity family match → value/invoke fallback.
pub fn resolve_route(
    entry: &InventoryEntry,
    class: SemanticClass,
    relationship: AcpRelationship,
    semantic_rule: &'static str,
) -> CanonicalRoute {
    let identity = canonical_source_identity(entry);
    if [
        "echo_core::tokenizer::Tokenizer::count_tokens",
        "echo_core::guard::content::ContentGuard::new",
        "echo_core::guard::content::ContentGuard::check",
        "echo_core::guard::content::ContentGuard::detect",
        "echo_core::guard::content::ContentGuard::redact",
        "echo_core::guard::content::ContentGuard::is_clean",
        "echo_core::project_rules::InstructionResolver::new",
        "echo_core::project_rules::InstructionResolver::project_root",
        "echo_core::project_rules::InstructionResolver::agents_files_only",
        "echo_core::project_rules::InstructionResolver::resolve",
        "echo_core::project_rules::load_project_rules",
        "echo_integration::mcp::client::McpClient::from_transport",
        "echo_agent::eval::runner::EvalRunner::new",
        "echo_agent::eval::runner::EvalRunner::with_run_store",
        "echo_agent::eval::runner::EvalRunner::with_grader",
        "echo_agent::eval::runner::EvalRunner::timeout_secs",
        "echo_agent::eval::runner::EvalRunner::workspace_root",
        "echo_agent::eval::runner::EvalRunner::run",
        "echo_agent::eval::runner::EvalRunner::run_all",
        "echo_agent::eval::runner::EvalRunner::run_all_async",
        "echo_agent::eval::runner::EvalRunner::evaluate_run_constraints",
        "echo_agent::eval::grader::LlmGrader::new",
        "echo_agent::eval::grader::LlmGrader::grade",
        "echo_agent::eval::grader::LlmGrader::grade_with_trajectory",
        "echo_agent::improve::trajectory::TrajectorySaver::new",
        "echo_agent::improve::trajectory::TrajectorySaver::save",
        "echo_agent::improve::trajectory::TrajectorySaver::list",
        "echo_agent::improve::trajectory::TrajectorySaver::stats",
        "echo_agent::improve::trajectory::TrajectorySaver::convert_run_to_sharegpt",
        "echo_agent::improve::loop::ImprovementLoop::new",
        "echo_agent::improve::loop::ImprovementLoop::run",
        "echo_agent::improve::loop::ImprovementLoop::run_async",
        "echo_agent::improve::loop::ImprovementLoop::max_iterations",
        "echo_agent::improve::loop::ImprovementLoop::improvement_threshold",
        "echo_agent::improve::loop::ImprovementLoop::holdout_ratio",
        "echo_core::plugin::registry::PluginRegistry::new",
        "echo_core::plugin::registry::PluginRegistry::with_paths",
        "echo_core::plugin::registry::PluginRegistry::revision",
        "echo_core::plugin::registry::PluginRegistry::scan_diagnostics",
        "echo_core::plugin::registry::PluginRegistry::scan_all",
        "echo_core::plugin::registry::PluginRegistry::scan_scopes",
        "echo_core::plugin::registry::PluginRegistry::validate_plugin_dir",
        "echo_core::plugin::registry::PluginRegistry::install",
        "echo_core::plugin::registry::PluginRegistry::uninstall",
        "echo_core::plugin::registry::PluginRegistry::enable",
        "echo_core::plugin::registry::PluginRegistry::disable",
        "echo_core::plugin::registry::PluginRegistry::get",
        "echo_core::plugin::registry::PluginRegistry::configure",
        "echo_core::plugin::registry::PluginRegistry::variables_for",
        "echo_core::plugin::registry::PluginRegistry::list",
        "echo_core::plugin::registry::PluginRegistry::list_enabled",
        "echo_core::plugin::registry::PluginRegistry::search",
        "echo_core::plugin::registry::PluginRegistry::count",
        "echo_core::plugin::registry::PluginRegistry::resolve_components",
        "echo_core::plugin::registry::PluginRegistry::resolve_components_async",
        "echo_core::plugin::registry::PluginRegistry::resolve_dependencies",
        "echo_core::plugin::registry::PluginRegistry::resolve_enabled_dependencies",
        "echo_core::plugin::registry::PluginRegistry::data_dir_for",
        "echo_state::memory::store::InMemoryStore::new",
        "echo_state::memory::store::InMemoryStore::put_raw",
        "echo_state::memory::store::FileStore::new",
        "echo_state::memory::store::FileStore::put_batch",
        "echo_state::memory::store::FileStore::flush_public",
        "echo_state::memory::sqlite_store::SqliteStore::new",
        "echo_state::memory::sqlite_store::SqliteStore::with_embedder",
        "echo_execution::skills::external::prompt_exec::PromptContext",
        "echo_execution::skills::external::prompt_exec::process_skill_content",
        "echo_agent::agent::react::ReactAgent::set_intent_router",
    ]
    .contains(&identity.as_str())
    {
        return CanonicalRoute::SourceOperation {
            operation: identity,
        };
    }
    if class == SemanticClass::LanguageIntrinsic
        || relationship == AcpRelationship::LanguageIntrinsic
    {
        if identity.starts_with("echo_agent::acp::") {
            return CanonicalRoute::Intrinsic {
                reason: "process-local-acp-runtime-authority",
            };
        }
        return CanonicalRoute::Intrinsic {
            reason: semantic_rule,
        };
    }
    if identity == "echo_orchestration::runtime::turn_driver::AgentTurnDriver::drive" {
        // The Host's RunStart authority invokes this exact driver and owns
        // cancellation, EventSink projection and the TurnReceipt terminal.
        // It is a core projection, not a language-local reimplementation.
        return CanonicalRoute::Core {
            family: FacadeFamily::Run,
        };
    }
    // Trait members inherit the reverse bridge of their consumer trait. The
    // CustomAgent trait is the one exception: the Host's own Agent facade has
    // direct lifecycle/run authority, so its methods stay on source routes;
    // bridge registration handles CustomAgent construction separately.
    if let Some(kind) = typed_bridge_kind(&identity)
        && kind != ExtensionKind::CustomAgent
    {
        return CanonicalRoute::Bridge { kind };
    }
    // Trait members inherit the process-local classification of their
    // construction seam even when rustdoc classifies the inventory item as a
    // method or value rather than an `Extension`. Without this check, callback
    // traits such as EventSink and AuditLogger become misleading source routes
    // with no Host-owned receiver.
    if let Some(reason) = process_local_trait_reason(&identity) {
        return CanonicalRoute::Intrinsic { reason };
    }
    if class == SemanticClass::Extension {
        // Closed set (plan 07 todo 5b): a consumer trait is served by (1) a
        // typed bridge kind when the Host has a live consumption point and a
        // proxy, (2) its source family surface when the trait is a framework
        // service seam the family adapters address over the wire (the family
        // answers with real operations or a typed feature_unavailable —
        // never method-not-found for a negotiated client), or (3) an
        // in-process library seam served by embedding the Rust framework
        // directly. There is no pending middle state.
        if let Some(kind) = typed_bridge_kind(&identity) {
            return CanonicalRoute::Bridge { kind };
        }
        if let Some(family) = family_for_source(&identity) {
            return if let Some(handler_operation) = family_operation_for_source(family, &identity) {
                CanonicalRoute::FamilyOperation {
                    family,
                    source_operation: identity,
                    handler_operation,
                }
            } else if is_core_family(family) {
                CanonicalRoute::Core { family }
            } else {
                CanonicalRoute::Family { family }
            };
        }
        // An extension trait without typed bridge or explicit process-local
        // evidence remains an executable source operation. Keeping it in the
        // canonical operation catalog makes the missing adapter observable and
        // prevents a silent intrinsic downgrade.
        return CanonicalRoute::SourceOperation {
            operation: identity,
        };
    }
    if relationship == AcpRelationship::StandardProjection {
        let method = standard_projection_method(entry);
        return CanonicalRoute::Standard { method };
    }
    let identity = canonical_source_identity(entry);
    let source_family = family_for_source(&identity);
    if entry.kind != ItemKind::StructField
        && let Some(family) = source_family
        && let Some(handler_operation) = family_operation_for_source(family, &identity)
    {
        return CanonicalRoute::FamilyOperation {
            family,
            source_operation: identity,
            handler_operation,
        };
    }
    if entry.kind != ItemKind::StructField
        && let Some(reason) = language_local_source_reason(&identity)
    {
        return CanonicalRoute::Intrinsic { reason };
    }
    if let Some(family) = source_family {
        return if class == SemanticClass::WireValue {
            CanonicalRoute::Value {
                family: Some(family),
            }
        } else if class == SemanticClass::Operation {
            CanonicalRoute::SourceOperation {
                operation: identity,
            }
        } else if is_core_family(family) {
            CanonicalRoute::Core { family }
        } else {
            CanonicalRoute::Family { family }
        };
    }
    match class {
        SemanticClass::WireValue => CanonicalRoute::Value { family: None },
        _ => CanonicalRoute::SourceOperation {
            operation: identity,
        },
    }
}

fn is_core_family(family: FacadeFamily) -> bool {
    matches!(
        family,
        FacadeFamily::Task
            | FacadeFamily::Subagent
            | FacadeFamily::EventReplay
            | FacadeFamily::StructuredOutput
    )
}

/// Stable ACP method that carries a standard projection, by source family.
fn standard_projection_method(entry: &InventoryEntry) -> &'static str {
    let linked_resource = entry.source_paths.iter().any(|source| {
        source == "echo_core::llm::types::LinkedResource"
            || source
                .strip_prefix("echo_core::llm::types::LinkedResource::")
                .is_some_and(|rest| !rest.is_empty())
    });
    let acp_session_context = entry.path == "echo_agent::acp::AcpSessionContext"
        || entry
            .path
            .strip_prefix("echo_agent::acp::AcpSessionContext::")
            .is_some_and(|rest| !rest.is_empty());
    if linked_resource {
        "session/prompt"
    } else if acp_session_context {
        "initialize+session/new"
    } else {
        "session/update"
    }
}

/// Mechanically validate the route table against the method catalog.
/// Returns every violation; an empty vec is the pass condition used by
/// tests and the contract export.
pub fn validate_facade_route_table() -> Vec<String> {
    let mut problems = Vec::new();
    let catalog_methods: Vec<&str> = METHOD_CATALOG.iter().map(|m| m.name).collect();
    let mut owned_methods: BTreeMap<&str, FacadeFamily> = BTreeMap::new();
    let mut seen_families: Vec<FacadeFamily> = Vec::new();
    for descriptor in FACADE_FAMILIES {
        if seen_families.contains(&descriptor.family) {
            problems.push(format!(
                "duplicate family {} in route table",
                descriptor.family.as_str()
            ));
        }
        seen_families.push(descriptor.family);
        for method in descriptor.methods {
            if method.contains('*') {
                problems.push(format!("wildcard method {method}"));
            }
            if !catalog_methods.contains(method) {
                problems.push(format!(
                    "family {} references unknown method {method}",
                    descriptor.family.as_str()
                ));
            }
            if let Some(owner) = owned_methods.get(method) {
                problems.push(format!(
                    "method {method} owned by both {} and {}",
                    owner.as_str(),
                    descriptor.family.as_str()
                ));
            }
            owned_methods.insert(method, descriptor.family);
        }
        if descriptor.family.is_source_routed() && descriptor.source_prefixes.is_empty() {
            problems.push(format!(
                "source-routed family {} has no source prefixes",
                descriptor.family.as_str()
            ));
        }
        if descriptor.family != FacadeFamily::Intrinsic && descriptor.validation.is_empty() {
            problems.push(format!(
                "family {} has no validation references",
                descriptor.family.as_str()
            ));
        }
    }
    for method in &catalog_methods {
        if !owned_methods.contains_key(method) {
            problems.push(format!("method {method} is not owned by any facade family"));
        }
    }
    problems
}

/// Build the generated `contracts/sdk/facade-operation-catalog.json`
/// document from the parity-manifest route obligations plus the family
/// table. Deterministic: same obligations in, same bytes out. Aliases and
/// canonical items aggregate into one route entry (one handler per route).
pub fn build_facade_operation_catalog(
    extension_protocol_version: u32,
    obligations: &[&crate::inventory::RouteObligation],
) -> serde_json::Value {
    let mut items_by_route: BTreeMap<&str, u64> = BTreeMap::new();
    for obligation in obligations {
        let counter = items_by_route.entry(obligation.route.as_str()).or_insert(0);
        *counter = counter.saturating_add(1);
    }
    let obligation_of_route: BTreeMap<&str, &crate::inventory::RouteObligation> = obligations
        .iter()
        .map(|obligation| (obligation.route.as_str(), *obligation))
        .collect();
    let mut route_values: Vec<serde_json::Value> = obligation_of_route
        .iter()
        .map(|(route_id, obligation)| {
            let family_descriptor = obligation.family.as_deref().and_then(|family| {
                FACADE_FAMILIES
                    .iter()
                    .find(|descriptor| descriptor.family.as_str() == family)
            });
            let is_family_route = obligation.surface == "family";
            let required_feature = if is_family_route {
                family_descriptor.and_then(|descriptor| descriptor.required_feature)
            } else {
                obligation.required_feature.as_deref()
            };
            let required_features = if is_family_route {
                family_descriptor
                    .and_then(|descriptor| descriptor.required_feature)
                    .into_iter()
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            } else {
                obligation.required_features.clone()
            };
            let feature_semantics = if is_family_route {
                if required_features.is_empty() {
                    crate::inventory::FeatureSemantics::Default
                } else {
                    crate::inventory::FeatureSemantics::AnyOf
                }
            } else {
                obligation.feature_semantics
            };
            serde_json::json!({
                "route": route_id,
                "surface": obligation.surface,
                "family": obligation.family,
                "method": obligation.method,
                "operation": obligation.operation,
                "handler_operation": obligation.handler_operation,
                "required_feature": required_feature,
                "required_features": required_features,
                "feature_semantics": feature_semantics,
                "signature_digests": if is_family_route {
                    Vec::<String>::new()
                } else {
                    obligation
                    .signatures
                    .iter()
                    .map(|signature| signature.digest.clone())
                    .collect::<Vec<_>>()
                },
                "input": if matches!(obligation.surface.as_str(), "invoke" | "family") {
                    serde_json::json!({
                        "encoding": "wire_value_array",
                        "description": "FeatureOperationRequest.arguments"
                    })
                } else {
                    serde_json::Value::Null
                },
                "result": if matches!(obligation.surface.as_str(), "invoke" | "family") {
                    serde_json::json!({
                        "encoding": "wire_value",
                        "description": "FeatureOperationResponse.value"
                    })
                } else {
                    serde_json::Value::Null
                },
                "operation_signatures": if is_family_route {
                    family_descriptor
                        .map(|descriptor| {
                            descriptor
                                .family
                                .operations()
                                .iter()
                                .map(|operation| serde_json::json!({
                                    "operation": operation,
                                    "signature_digests": [
                                        family_operation_signature_digest(
                                            descriptor.family.as_str(),
                                            operation,
                                        )
                                    ],
                                    "input": {
                                        "encoding": "wire_value_array",
                                        "description": "FeatureOperationRequest.arguments"
                                    },
                                    "result": {
                                        "encoding": "wire_value",
                                        "description": "FeatureOperationResponse.value"
                                    },
                                }))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default()
                } else {
                    Vec::new()
                },
                "items": items_by_route.get(route_id).copied().unwrap_or(0),
            })
        })
        .collect();
    // Closed family operations are protocol authority, independent of whether
    // a root Rust item currently aggregates onto `family:<name>`. When every
    // construction type of a family is language-local, the executable Host
    // route must retain the same operation/signature contract.
    for descriptor in FACADE_FAMILIES {
        let operations = descriptor.family.operations();
        if operations.is_empty() {
            continue;
        }
        let route_id = format!("family:{}", descriptor.family.as_str());
        if route_values.iter().any(|route| {
            route.get("route").and_then(serde_json::Value::as_str) == Some(route_id.as_str())
        }) {
            continue;
        }
        let Some(method) = descriptor.methods.first() else {
            continue;
        };
        let required_features = descriptor
            .required_feature
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let feature_semantics = if required_features.is_empty() {
            crate::inventory::FeatureSemantics::Default
        } else {
            crate::inventory::FeatureSemantics::AnyOf
        };
        route_values.push(serde_json::json!({
            "route": route_id,
            "surface": "family",
            "family": descriptor.family.as_str(),
            "method": method,
            "operation": serde_json::Value::Null,
            "handler_operation": serde_json::Value::Null,
            "required_feature": descriptor.required_feature,
            "required_features": required_features,
            "feature_semantics": feature_semantics,
            "signature_digests": Vec::<String>::new(),
            "input": {
                "encoding": "wire_value_array",
                "description": "FeatureOperationRequest.arguments"
            },
            "result": {
                "encoding": "wire_value",
                "description": "FeatureOperationResponse.value"
            },
            "operation_signatures": operations
                .iter()
                .map(|operation| serde_json::json!({
                    "operation": operation,
                    "signature_digests": [
                        family_operation_signature_digest(descriptor.family.as_str(), operation)
                    ],
                    "input": {
                        "encoding": "wire_value_array",
                        "description": "FeatureOperationRequest.arguments"
                    },
                    "result": {
                        "encoding": "wire_value",
                        "description": "FeatureOperationResponse.value"
                    },
                }))
                .collect::<Vec<_>>(),
            "items": 0u64,
        }));
    }
    route_values.sort_by(|left, right| {
        left.get("route")
            .and_then(serde_json::Value::as_str)
            .cmp(&right.get("route").and_then(serde_json::Value::as_str))
    });
    let families: Vec<serde_json::Value> = FACADE_FAMILIES
        .iter()
        .map(|descriptor| {
            serde_json::json!({
                "family": descriptor.family.as_str(),
                "source_routed": descriptor.family.is_source_routed(),
                "methods": descriptor.methods,
                "capability": descriptor.capability.as_str(),
                "required_feature": descriptor.required_feature,
                "operations": descriptor.family.operations().to_vec(),
                "source_prefixes": descriptor.source_prefixes,
                "validation": descriptor.validation,
            })
        })
        .collect();
    serde_json::json!({
        "schema_version": 1u64,
        "extension_protocol_version": extension_protocol_version,
        "families": families,
        "routes": route_values,
        "total_items": obligations.len(),
    })
}

/// Handle kinds a facade resource may take (used by the generated catalog
/// documentation and the Host resource ladder in plan 07 todo 2).
pub fn facade_resource_kinds() -> &'static [HandleKind] {
    &[
        HandleKind::TaskRun,
        HandleKind::PlanTask,
        HandleKind::Subagent,
        HandleKind::Stream,
    ]
}

/// Inventory item kinds that can carry a remote operation route. Modules,
/// macros and primitive items are always intrinsic.
pub fn operation_capable(kind: ItemKind) -> bool {
    matches!(
        kind,
        ItemKind::Function | ItemKind::Method | ItemKind::Struct | ItemKind::Enum | ItemKind::Union
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_table_is_mechanically_valid() {
        assert!(
            validate_facade_route_table().is_empty(),
            "route table violations: {:?}",
            validate_facade_route_table()
        );
    }

    #[test]
    fn longest_prefix_wins_over_reexport_module() {
        // `echo_agent::state` re-exports delivery; delivery prefix is
        // longer for delivery items.
        assert_eq!(
            family_for_source("echo_state::delivery::DeliveryLedger"),
            Some(FacadeFamily::Delivery)
        );
        assert_eq!(
            family_for_source("echo_state::journal::segmented::SegmentedJournal"),
            Some(FacadeFamily::State)
        );
        // Prefix must not match partial segments.
        assert_eq!(
            family_for_source("echo_orchestration::tasks_extra::X"),
            None
        );
    }

    #[test]
    fn pure_helper_classification_is_exact_and_excludes_io_neighbors() {
        assert_eq!(
            language_local_source_reason("echo_core::utils::json_parse::clean_json"),
            Some("language-local-exact-helper")
        );
        assert_eq!(
            language_local_source_reason("echo_core::utils::utf8::IncrementalUtf8Decoder::push"),
            Some("language-local-exact-helper")
        );
        assert_eq!(
            language_local_source_reason("echo_core::utils::fs::atomic_write"),
            None
        );
        assert_eq!(
            language_local_source_reason("echo_core::utils::fs::atomic_compare_and_swap"),
            Some("process-local-callback-parameter")
        );
        assert_eq!(
            language_local_source_reason("echo_core::utils::fs::validate_path_segment"),
            Some("language-local-exact-helper")
        );
        assert_eq!(
            language_local_source_reason("echo_core::utils::time::now_secs"),
            Some("language-local-platform-clock")
        );
        assert_eq!(
            language_local_source_reason("echo_core::retry::core::with_retry"),
            None
        );
    }

    #[test]
    fn process_local_trait_members_are_intrinsic_regardless_of_item_class() {
        use std::collections::{BTreeMap, BTreeSet};

        for (identity, reason) in [
            (
                "echo_orchestration::runtime::turn_driver::EventSink::on_event",
                "process-local-driver-owned-event-sink",
            ),
            (
                "echo_agent::evolution::audit::ChangeLog::record",
                "process-local-evolution-storage-construction",
            ),
            (
                "echo_orchestration::human_loop::permission::PermissionRequestHandler::handle",
                "process-local-permission-request-settlement",
            ),
            (
                "echo_core::agent::AgentInputLifecycle::mark_drained",
                "process-local-agent-owned-input-drain-hook",
            ),
        ] {
            assert_eq!(
                process_local_trait_reason(identity),
                Some(reason),
                "missing process-local evidence for {identity}"
            );
            assert!(matches!(
                resolve_route(
                    &InventoryEntry {
                        path: identity.to_string(),
                        kind: ItemKind::Method,
                        source_paths: BTreeSet::from([identity.to_string()]),
                        signatures: BTreeMap::new(),
                        profiles: BTreeSet::new(),
                        declared_feature_requirements: BTreeSet::new(),
                        automatically_derived: false,
                    },
                    SemanticClass::Operation,
                    AcpRelationship::EchoExtension,
                    "test"
                ),
                CanonicalRoute::Intrinsic { .. }
            ));
        }
        assert_eq!(
            process_local_trait_reason("echo_core::audit::AuditLogger::log"),
            None
        );
    }

    #[test]
    fn audited_process_resources_and_helpers_are_intrinsic() {
        for identity in [
            "echo_orchestration::scheduler::cron_task::CronTaskStore::new",
            "echo_orchestration::scheduler::runner::SchedulerRunner::new",
            "echo_state::profiles::ProfileStore::new",
            "echo_core::plugin::registry::PluginRegistry::new",
            "echo_agent::evolution::layer::MemoryLayerManager::new",
            "echo_orchestration::human_loop::classifier::RuleClassifier::new",
            "echo_state::audit::memory::InMemoryAuditLogger::new",
            "echo_integration::providers::openai::OpenAiClient::new",
            "echo_agent::context::ContextAssembler::assemble",
            "echo_core::plugin::manifest::PluginManifest::from_json",
        ] {
            assert!(
                language_local_source_reason(identity).is_some(),
                "missing intrinsic evidence for {identity}"
            );
        }
        assert!(
            language_local_source_reason("echo_core::plugin::manifest::PluginManifest").is_none(),
            "value type itself must not be swallowed by helper classification"
        );
    }

    #[test]
    fn skill_activation_authority_has_an_explicit_process_local_route() {
        for identity in [
            "echo_execution::skills::registry::SkillActivationHandle",
            "echo_execution::skills::registry::SkillActivationHandle::activated_names",
            "echo_execution::skills::registry::SkillRegistry::activation_handle",
            "echo_execution::skills::registry::SkillRegistry::activation_view",
            "echo_execution::skills::registry::SkillRegistry::restore_activation_state",
        ] {
            assert_eq!(
                language_local_source_reason(identity),
                Some("process-local-skill-activation-authority"),
                "missing explicit Skill activation classification for {identity}"
            );
        }
        for identity in [
            "echo_agent::agent::react::ReactAgent::record_code_skill_info",
            "echo_agent::agent::react::ReactAgent::register_prepared_skill",
            "echo_agent::agent::react::ReactAgent::register_skill_descriptor",
            "echo_agent::agent::react::ReactAgent::tag_skills_source_with_variables",
        ] {
            assert_eq!(
                language_local_source_reason(identity),
                Some("process-local-react-agent-composition"),
                "missing explicit Agent reconciliation classification for {identity}"
            );
        }
    }

    #[test]
    fn route_ids_never_contain_wildcards() {
        let routes = [
            CanonicalRoute::Standard {
                method: "session/prompt",
            },
            CanonicalRoute::Core {
                family: FacadeFamily::Task,
            },
            CanonicalRoute::Family {
                family: FacadeFamily::Memory,
            },
            CanonicalRoute::Bridge {
                kind: ExtensionKind::Tool,
            },
            CanonicalRoute::Invoke {
                operation: "echo_agent::evolution::review::ReviewEngine".to_string(),
            },
            CanonicalRoute::SourceOperation {
                operation: "echo_core::agent::Agent::current_run_id".to_string(),
            },
            CanonicalRoute::Value {
                family: Some(FacadeFamily::Task),
            },
            CanonicalRoute::Value { family: None },
            CanonicalRoute::Intrinsic {
                reason: "builder-or-factory",
            },
        ];
        for route in &routes {
            let id = route.route_id();
            assert!(!id.contains('*'), "wildcard in {id}");
            assert!(!id.is_empty(), "empty route id");
        }
        assert_eq!(routes[0].route_id(), "standard:session/prompt".to_string());
        assert_eq!(routes[3].route_id(), "bridge:tool".to_string());
    }

    #[test]
    fn catalog_document_is_deterministic_and_counts_items() {
        use crate::inventory::RouteObligation;
        let obligation = |route: &str, surface: &str, method: Option<&str>| RouteObligation {
            route: route.to_string(),
            surface: surface.to_string(),
            family: Some("task".to_string()),
            method: method.map(str::to_string),
            operation: None,
            handler_operation: None,
            required_feature: None,
            required_features: Vec::new(),
            feature_semantics: crate::inventory::FeatureSemantics::Default,
            signatures: Vec::new(),
            mapping: "echo_extension via long-lived-resource; Rust remains authoritative"
                .to_string(),
            validation: vec!["echo-sdk-protocol/tests/core_rpc_contract.rs".to_string()],
        };
        let obligations = [
            obligation("core:task", "core", Some("_echo_agent/task/create")),
            obligation("core:task", "core", Some("_echo_agent/task/create")),
            obligation(
                "invoke:echo_agent::evolution::review::ReviewEngine",
                "invoke",
                Some("_echo_agent/facade/invoke"),
            ),
        ];
        let referenced: Vec<&RouteObligation> = obligations.iter().collect();
        let doc = build_facade_operation_catalog(1, &referenced);
        let rendered_first = crate::schema::canonical_json(&doc);
        let rendered_second =
            crate::schema::canonical_json(&build_facade_operation_catalog(1, &referenced));
        assert_eq!(rendered_first, rendered_second);
        assert_eq!(doc.get("total_items").and_then(|v| v.as_u64()), Some(3));
        let routes_array = doc
            .get("routes")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let core_task = routes_array
            .iter()
            .find(|route| route.get("route").and_then(|v| v.as_str()) == Some("core:task"))
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            core_task.get("items").and_then(|v| v.as_u64()),
            Some(2),
            "alias and canonical item aggregate into one route"
        );
    }
}
