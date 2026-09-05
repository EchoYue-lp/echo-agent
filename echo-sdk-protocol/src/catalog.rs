//! Canonical `_echo_agent/*` operation catalog.
//!
//! The catalog is the single authority for the extension method surface
//! (design §10.4): every method name, direction, required capability and the
//! feature surface families it exposes. The contract generator embeds it in
//! the exported schema, and `validate_catalog` enforces the ACP
//! extensibility rules mechanically:
//!
//! - every method name starts with the `_echo_agent/` namespace;
//! - no method collides with a standard ACP method;
//! - every method's required capability exists in the capability taxonomy;
//! - reverse (Host -> SDK) calls are marked, because they ride the same
//!   connection as client-initiated requests;
//! - notifications never carry a response payload type.
//!
//! The catalog binds to the parity manifest: manifest entries classified as
//! `echo_extension` are the Rust facade items these methods are obligated to
//! project; the adapter plans consume this table, they do not invent one.

use crate::capability::ExtensionCapability;

/// Direction of an extension operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Client -> Host request with a response.
    Request,
    /// Client -> Host notification (no response).
    ClientNotification,
    /// Host -> Client reverse request (extension bridge, design §12).
    ReverseRequest,
    /// Host -> Client notification (event stream, gaps).
    HostNotification,
}

impl Direction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Direction::Request => "request",
            Direction::ClientNotification => "client_notification",
            Direction::ReverseRequest => "reverse_request",
            Direction::HostNotification => "host_notification",
        }
    }
}

/// One catalog entry.
#[derive(Debug, Clone)]
pub struct MethodDescriptor {
    /// Full method name, e.g. `_echo_agent/run/start`.
    pub name: &'static str,
    pub direction: Direction,
    /// Capability that must be declared for the method to be callable.
    pub capability: ExtensionCapability,
    /// One-line contract summary embedded in the exported schema.
    pub summary: &'static str,
}

/// Standard ACP v1 method names. The catalog must never define any of
/// these; the official schema owns them.
pub const STANDARD_ACP_METHODS: &[&str] = &[
    "initialize",
    "authenticate",
    "session/new",
    "session/load",
    "session/prompt",
    "session/cancel",
    "session/update",
    "session/request_permission",
    "session/set_mode",
    "session/set_suggestions",
    "session/show_command",
    "session/read_clipboard",
    "session/write_text",
    "fs/read_text_file",
    "fs/write_text_file",
    "terminal/create",
    "terminal/restart",
    "terminal/write",
    "terminal/kill",
    "terminal/read",
    "session/request_auth",
    "session/sign_out",
    "session/list",
    "session/delete",
    "session/close",
];

/// The frozen extension method catalog.
pub const METHOD_CATALOG: &[MethodDescriptor] = &[
    // ── Agent lifecycle ───────────────────────────────────────────────
    MethodDescriptor {
        name: "_echo_agent/agent/create",
        direction: Direction::Request,
        capability: ExtensionCapability::AgentLifecycle,
        summary: "Construct an agent instance from a typed config; framework validates.",
    },
    MethodDescriptor {
        name: "_echo_agent/agent/describe",
        direction: Direction::Request,
        capability: ExtensionCapability::AgentLifecycle,
        summary: "Capability snapshot and immutable construction facts of an agent.",
    },
    MethodDescriptor {
        name: "_echo_agent/agent/close",
        direction: Direction::Request,
        capability: ExtensionCapability::AgentLifecycle,
        summary: "Release an agent; in-flight runs settle via framework semantics.",
    },
    // ── Session handles ───────────────────────────────────────────────
    MethodDescriptor {
        name: "_echo_agent/session/create",
        direction: Direction::Request,
        capability: ExtensionCapability::SessionHandles,
        summary: "Create an extension session handle on an agent.",
    },
    MethodDescriptor {
        name: "_echo_agent/session/load",
        direction: Direction::Request,
        capability: ExtensionCapability::SessionHandles,
        summary: "Resume a persisted session by framework identity.",
    },
    MethodDescriptor {
        name: "_echo_agent/session/close",
        direction: Direction::Request,
        capability: ExtensionCapability::SessionHandles,
        summary: "Release a session handle; idempotent.",
    },
    // ── Runs ──────────────────────────────────────────────────────────
    MethodDescriptor {
        name: "_echo_agent/run/start",
        direction: Direction::Request,
        capability: ExtensionCapability::Runs,
        summary: "Start a chat/execute run; returns run handle and first event.",
    },
    MethodDescriptor {
        name: "_echo_agent/run/get",
        direction: Direction::Request,
        capability: ExtensionCapability::Runs,
        summary: "Snapshot run status, last sequence and settled terminal/receipt.",
    },
    MethodDescriptor {
        name: "_echo_agent/run/wait",
        direction: Direction::Request,
        capability: ExtensionCapability::Runs,
        summary: "Bounded wait for the single authoritative terminal.",
    },
    MethodDescriptor {
        name: "_echo_agent/run/cancel",
        direction: Direction::Request,
        capability: ExtensionCapability::Runs,
        summary: "Request cancellation; framework CAS decides the unique outcome.",
    },
    MethodDescriptor {
        name: "_echo_agent/run/steer",
        direction: Direction::Request,
        capability: ExtensionCapability::Runs,
        summary: "Mid-flight steering on supporting runs.",
    },
    // ── Replay ────────────────────────────────────────────────────────
    MethodDescriptor {
        name: "_echo_agent/run/replay",
        direction: Direction::Request,
        capability: ExtensionCapability::EventReplay,
        summary: "Bounded event replay strictly after a cursor; gap-aware.",
    },
    MethodDescriptor {
        name: "_echo_agent/event",
        direction: Direction::HostNotification,
        capability: ExtensionCapability::Runs,
        summary: "Full EventEnvelope notification for accepted framework events.",
    },
    MethodDescriptor {
        name: "_echo_agent/gap",
        direction: Direction::HostNotification,
        capability: ExtensionCapability::EventReplay,
        summary: "Retention-floor breach notice with snapshot watermark.",
    },
    // ── Task graph ────────────────────────────────────────────────────
    MethodDescriptor {
        name: "_echo_agent/task/create",
        direction: Direction::Request,
        capability: ExtensionCapability::TaskGraph,
        summary: "Create a PlanTask in a revisioned TaskRun graph.",
    },
    MethodDescriptor {
        name: "_echo_agent/task/update",
        direction: Direction::Request,
        capability: ExtensionCapability::TaskGraph,
        summary: "Patch a task at an expected revision (optimistic concurrency).",
    },
    MethodDescriptor {
        name: "_echo_agent/task/list",
        direction: Direction::Request,
        capability: ExtensionCapability::TaskGraph,
        summary: "List task identity/status/revision summaries of a TaskRun.",
    },
    MethodDescriptor {
        name: "_echo_agent/task/execute",
        direction: Direction::Request,
        capability: ExtensionCapability::TaskGraph,
        summary: "Drive one PlanTask through the framework executor.",
    },
    MethodDescriptor {
        name: "_echo_agent/task/control",
        direction: Direction::Request,
        capability: ExtensionCapability::TaskGraph,
        summary: "Pause/resume/cancel a task via the framework service.",
    },
    // ── Subagents ─────────────────────────────────────────────────────
    MethodDescriptor {
        name: "_echo_agent/subagent/dispatch",
        direction: Direction::Request,
        capability: ExtensionCapability::Subagents,
        summary: "Dispatch a subagent execution; framework scheduler owns it.",
    },
    MethodDescriptor {
        name: "_echo_agent/subagent/await",
        direction: Direction::Request,
        capability: ExtensionCapability::Subagents,
        summary: "Bounded wait for a subagent result.",
    },
    MethodDescriptor {
        name: "_echo_agent/subagent/control",
        direction: Direction::Request,
        capability: ExtensionCapability::Subagents,
        summary: "Control verbs for a running subagent.",
    },
    // ── Extension bridge ──────────────────────────────────────────────
    MethodDescriptor {
        name: "_echo_agent/extension/register",
        direction: Direction::Request,
        capability: ExtensionCapability::ExtensionBridge,
        summary: "Register a host-language implementation of a public trait.",
    },
    MethodDescriptor {
        name: "_echo_agent/extension/unregister",
        direction: Direction::Request,
        capability: ExtensionCapability::ExtensionBridge,
        summary: "Release a registered extension; idempotent.",
    },
    MethodDescriptor {
        name: "_echo_agent/extension/invoke",
        direction: Direction::ReverseRequest,
        capability: ExtensionCapability::ExtensionBridge,
        summary: "Host -> SDK reverse call into a registered implementation.",
    },
    MethodDescriptor {
        name: "_echo_agent/extension/cancel",
        direction: Direction::HostNotification,
        capability: ExtensionCapability::ExtensionBridge,
        summary: "Host -> SDK cancellation notice for an in-flight invocation.",
    },
    // ── Structured output ─────────────────────────────────────────────
    MethodDescriptor {
        name: "_echo_agent/structured_output/validate",
        direction: Direction::Request,
        capability: ExtensionCapability::StructuredOutput,
        summary: "Validate a structured-output contract against the facade schema.",
    },
    // ── Feature surfaces ──────────────────────────────────────────────
    MethodDescriptor {
        name: "_echo_agent/memory/op",
        direction: Direction::Request,
        capability: ExtensionCapability::Runs,
        summary: "Memory store operations projected verbatim until typed DTOs land.",
    },
    MethodDescriptor {
        name: "_echo_agent/workflow/op",
        direction: Direction::Request,
        capability: ExtensionCapability::Runs,
        summary: "Workflow definition/execution operations.",
    },
    MethodDescriptor {
        name: "_echo_agent/state/op",
        direction: Direction::Request,
        capability: ExtensionCapability::Runs,
        summary: "State/delivery/trace operations over framework stores.",
    },
];

/// Mechanically validate the catalog against ACP extensibility rules.
/// Returns every violation; an empty vec is the pass condition used by
/// tests and the schema export.
pub fn validate_catalog(catalog: &[MethodDescriptor]) -> Vec<String> {
    let mut problems = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    for method in catalog {
        if !method
            .name
            .starts_with(crate::EXTENSION_NAMESPACE.to_string().as_str())
        {
            problems.push(format!(
                "method {} is outside the extension namespace",
                method.name
            ));
        }
        // ACP requires vendor methods to start with an underscore; the
        // namespace constant itself begins with `_echo_agent`, but assert it
        // explicitly so a rename cannot silently break compliance.
        if !method.name.starts_with('_') {
            problems.push(format!(
                "method {} must start with an underscore",
                method.name
            ));
        }
        if STANDARD_ACP_METHODS.contains(&method.name) {
            problems.push(format!(
                "method {} collides with a standard ACP method",
                method.name
            ));
        }
        if seen.contains(&method.name) {
            problems.push(format!("duplicate catalog entry {}", method.name));
        }
        seen.push(method.name);
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_valid() {
        assert!(validate_catalog(METHOD_CATALOG).is_empty());
    }

    #[test]
    fn catalog_covers_design_families() {
        let names: Vec<&str> = METHOD_CATALOG.iter().map(|m| m.name).collect();
        for family in [
            "_echo_agent/agent/create",
            "_echo_agent/session/create",
            "_echo_agent/run/start",
            "_echo_agent/run/replay",
            "_echo_agent/task/execute",
            "_echo_agent/subagent/dispatch",
            "_echo_agent/extension/register",
            "_echo_agent/extension/invoke",
            "_echo_agent/event",
            "_echo_agent/gap",
        ] {
            assert!(names.contains(&family), "missing {family}");
        }
    }

    #[test]
    fn detects_namespace_violation() {
        let bad = vec![MethodDescriptor {
            name: "run/start",
            direction: Direction::Request,
            capability: ExtensionCapability::Runs,
            summary: "",
        }];
        assert!(!validate_catalog(&bad).is_empty());
    }
}
