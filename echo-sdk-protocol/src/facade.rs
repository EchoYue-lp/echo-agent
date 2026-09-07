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
//! family method are frozen together with the family handlers (plan 07
//! todos 3–5); until then every item still carries its exact identity —
//! generic invocations use the exact source identity as the operation id
//! (`invoke:<source-path>`), never a wildcard.

use std::collections::BTreeMap;

use crate::capability::ExtensionCapability;
use crate::catalog::METHOD_CATALOG;
use crate::handle::HandleKind;
use crate::inventory::{AcpRelationship, InventoryEntry, ItemKind, SemanticClass};
use crate::methods::ExtensionKind;

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
                | Self::Invoke
                | Self::Value
                | Self::Bridge
                | Self::Standard
                | Self::Intrinsic
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
        source_prefixes: &["echo_core::lsp", "echo_integration::lsp"],
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
        source_prefixes: &["echo_tools::files"],
        validation: FAMILY_VALIDATION,
    },
    FamilyDescriptor {
        family: FacadeFamily::Shell,
        methods: &["_echo_agent/shell/op"],
        capability: ExtensionCapability::FeatureSurfaces,
        required_feature: Some("shell"),
        source_prefixes: &["echo_tools::shell"],
        validation: FAMILY_VALIDATION,
    },
    FamilyDescriptor {
        family: FacadeFamily::Git,
        methods: &["_echo_agent/git/op"],
        capability: ExtensionCapability::FeatureSurfaces,
        required_feature: Some("git"),
        source_prefixes: &["echo_tools::git"],
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
        source_prefixes: &["echo_tools::media"],
        validation: FAMILY_VALIDATION,
    },
    FamilyDescriptor {
        family: FacadeFamily::Data,
        methods: &["_echo_agent/data/op"],
        capability: ExtensionCapability::FeatureSurfaces,
        required_feature: Some("data"),
        source_prefixes: &["echo_tools::data"],
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
/// Traits without an entry stay on the bridge with `typed_kind: None` until
/// plan 07 todo 5 closes the remaining trait set; the catalog records them
/// as `bridge:pending` so the obligation is visible, never hidden.
const TYPED_BRIDGE_TRAITS: &[(&str, ExtensionKind)] = &[
    ("echo_core::tools::Tool", ExtensionKind::Tool),
    ("echo_core::llm::LlmClient", ExtensionKind::LlmClient),
    ("echo_core::memory::store::Store", ExtensionKind::Store),
    (
        "echo_orchestration::human_loop::HumanLoopProvider",
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
    /// Consumer-implemented trait served through the reverse bridge.
    Bridge { kind: Option<ExtensionKind> },
    /// Exact facade identity served by the generic typed invoke method.
    Invoke { operation: String },
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
            Self::Bridge { kind } => match kind {
                Some(kind) => format!("bridge:{}", kind.as_str()),
                None => "bridge:pending".to_string(),
            },
            Self::Invoke { operation } => format!("invoke:{operation}"),
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
            Self::Bridge { .. } => "bridge",
            Self::Invoke { .. } => "invoke",
            Self::Value { .. } => "value",
            Self::Intrinsic { .. } => "intrinsic",
        }
    }

    /// Primary wire method for the route, when it owns one.
    pub fn method(&self) -> Option<&'static str> {
        match self {
            Self::Standard { method } => Some(method),
            Self::Core { family } | Self::Family { family } => family.methods().first().copied(),
            Self::Bridge { .. } => Some("_echo_agent/extension/register"),
            Self::Invoke { .. } => Some("_echo_agent/facade/invoke"),
            Self::Value { .. } | Self::Intrinsic { .. } => None,
        }
    }

    pub fn family(&self) -> Option<FacadeFamily> {
        match self {
            Self::Standard { .. } => Some(FacadeFamily::Standard),
            Self::Core { family } | Self::Family { family } => Some(*family),
            Self::Bridge { .. } => Some(FacadeFamily::Bridge),
            Self::Invoke { .. } => Some(FacadeFamily::Invoke),
            Self::Value { family } => Some(family.unwrap_or(FacadeFamily::Value)),
            Self::Intrinsic { .. } => Some(FacadeFamily::Intrinsic),
        }
    }

    /// Exact operation identity for generic invocations; `None` elsewhere.
    pub fn operation(&self) -> Option<&str> {
        match self {
            Self::Invoke { operation } => Some(operation.as_str()),
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
    if class == SemanticClass::LanguageIntrinsic
        || relationship == AcpRelationship::LanguageIntrinsic
    {
        return CanonicalRoute::Intrinsic {
            reason: semantic_rule,
        };
    }
    if class == SemanticClass::Extension {
        return CanonicalRoute::Bridge {
            kind: typed_bridge_kind(&canonical_source_identity(entry)),
        };
    }
    if relationship == AcpRelationship::StandardProjection {
        let method = standard_projection_method(entry);
        return CanonicalRoute::Standard { method };
    }
    let identity = canonical_source_identity(entry);
    if let Some(family) = family_for_source(&identity) {
        return if class == SemanticClass::WireValue {
            CanonicalRoute::Value {
                family: Some(family),
            }
        } else if is_core_family(family) {
            CanonicalRoute::Core { family }
        } else {
            CanonicalRoute::Family { family }
        };
    }
    match class {
        SemanticClass::WireValue => CanonicalRoute::Value { family: None },
        _ => CanonicalRoute::Invoke {
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
    let route_values: Vec<serde_json::Value> = obligation_of_route
        .iter()
        .map(|(route_id, obligation)| {
            serde_json::json!({
                "route": route_id,
                "surface": obligation.surface,
                "family": obligation.family,
                "method": obligation.method,
                "operation": obligation.operation,
                "required_feature": obligation.required_feature,
                "items": items_by_route.get(route_id).copied().unwrap_or(0),
            })
        })
        .collect();
    let families: Vec<serde_json::Value> = FACADE_FAMILIES
        .iter()
        .map(|descriptor| {
            serde_json::json!({
                "family": descriptor.family.as_str(),
                "source_routed": descriptor.family.is_source_routed(),
                "methods": descriptor.methods,
                "capability": descriptor.capability.as_str(),
                "required_feature": descriptor.required_feature,
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
                kind: Some(ExtensionKind::Tool),
            },
            CanonicalRoute::Bridge { kind: None },
            CanonicalRoute::Invoke {
                operation: "echo_agent::evolution::review::ReviewEngine".to_string(),
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
        assert_eq!(routes[4].route_id(), "bridge:pending".to_string());
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
            required_feature: None,
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
