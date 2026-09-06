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

impl MethodDescriptor {
    /// Request/notification payload definition in the generated extension schema.
    pub fn params_schema(&self) -> &'static str {
        match self.name {
            "_echo_agent/agent/create" => "AgentCreateRequest",
            "_echo_agent/agent/describe" => "AgentDescribeRequest",
            "_echo_agent/agent/close" => "AgentCloseRequest",
            "_echo_agent/session/create" => "SessionCreateRequest",
            "_echo_agent/session/load" => "SessionLoadRequest",
            "_echo_agent/session/close" => "SessionCloseRequest",
            "_echo_agent/run/start" => "RunStartRequest",
            "_echo_agent/run/get" => "RunGetRequest",
            "_echo_agent/run/wait" => "RunWaitRequest",
            "_echo_agent/run/cancel" => "RunCancelRequest",
            "_echo_agent/run/steer" => "RunSteerRequest",
            "_echo_agent/run/replay" => "ReplayRequest",
            "_echo_agent/event" => "EventNotification",
            "_echo_agent/event/ack" => "EventAckNotification",
            "_echo_agent/gap" => "GapNotification",
            "_echo_agent/task/create" => "TaskCreateRequest",
            "_echo_agent/task/update" => "TaskUpdateRequest",
            "_echo_agent/task/list" => "TaskListRequest",
            "_echo_agent/task/execute" => "TaskExecuteRequest",
            "_echo_agent/task/control" => "TaskControlRequest",
            "_echo_agent/subagent/dispatch" => "SubagentDispatchRequest",
            "_echo_agent/subagent/await" => "SubagentAwaitRequest",
            "_echo_agent/subagent/control" => "SubagentControlRequest",
            "_echo_agent/extension/register" => "ExtensionRegisterRequest",
            "_echo_agent/extension/unregister" => "ExtensionUnregisterRequest",
            "_echo_agent/extension/invoke" => "ExtensionInvokeCall",
            "_echo_agent/extension/cancel" => "ExtensionCancelNotice",
            "_echo_agent/extension/stream" => "ExtensionStreamEvent",
            "_echo_agent/structured_output/validate"
            | "_echo_agent/memory/op"
            | "_echo_agent/workflow/op"
            | "_echo_agent/state/op"
            | "_echo_agent/facade/invoke" => "FeatureOperationRequest",
            _ => "",
        }
    }

    /// Successful result definition. Notifications have no result schema.
    pub fn result_schema(&self) -> Option<&'static str> {
        match self.name {
            "_echo_agent/agent/create" => Some("AgentCreateResponse"),
            "_echo_agent/agent/describe" => Some("AgentDescribeResponse"),
            "_echo_agent/agent/close" => Some("AgentCloseResponse"),
            "_echo_agent/session/create" => Some("SessionCreateResponse"),
            "_echo_agent/session/load" => Some("SessionLoadResponse"),
            "_echo_agent/session/close" => Some("SessionCloseResponse"),
            "_echo_agent/run/start" => Some("RunStartResponse"),
            "_echo_agent/run/get" => Some("RunGetResponse"),
            "_echo_agent/run/wait" => Some("RunWaitResponse"),
            "_echo_agent/run/cancel" => Some("RunCancelResponse"),
            "_echo_agent/run/steer" => Some("RunSteerResponse"),
            "_echo_agent/run/replay" => Some("ReplayResponse"),
            "_echo_agent/task/create" => Some("TaskCreateResponse"),
            "_echo_agent/task/update" => Some("TaskUpdateResponse"),
            "_echo_agent/task/list" => Some("TaskListResponse"),
            "_echo_agent/task/execute" => Some("TaskExecuteResponse"),
            "_echo_agent/task/control" => Some("TaskControlResponse"),
            "_echo_agent/subagent/dispatch" => Some("SubagentDispatchResponse"),
            "_echo_agent/subagent/await" => Some("SubagentAwaitResponse"),
            "_echo_agent/subagent/control" => Some("SubagentControlResponse"),
            "_echo_agent/extension/register" => Some("ExtensionRegisterResponse"),
            "_echo_agent/extension/unregister" => Some("ExtensionUnregisterResponse"),
            "_echo_agent/extension/invoke" => Some("ExtensionInvokeOutcome"),
            "_echo_agent/structured_output/validate"
            | "_echo_agent/memory/op"
            | "_echo_agent/workflow/op"
            | "_echo_agent/state/op"
            | "_echo_agent/facade/invoke" => Some("FeatureOperationResponse"),
            "_echo_agent/event"
            | "_echo_agent/gap"
            | "_echo_agent/extension/cancel"
            | "_echo_agent/extension/stream" => None,
            _ => None,
        }
    }

    pub fn error_schema(&self) -> Option<&'static str> {
        matches!(
            self.direction,
            Direction::Request | Direction::ReverseRequest
        )
        .then_some("EchoSdkError")
    }
}

/// Stable ACP v1 method names come directly from the pinned official schema
/// artifact. Keeping the references typed makes an upstream rename a compile
/// failure rather than a silently stale local list.
pub fn official_acp_v1_methods() -> std::collections::BTreeSet<&'static str> {
    use agent_client_protocol_schema::v1::{
        AGENT_METHOD_NAMES, CLIENT_METHOD_NAMES, PROTOCOL_LEVEL_METHOD_NAMES,
    };
    [
        AGENT_METHOD_NAMES.initialize,
        AGENT_METHOD_NAMES.authenticate,
        AGENT_METHOD_NAMES.session_new,
        AGENT_METHOD_NAMES.session_load,
        AGENT_METHOD_NAMES.session_set_mode,
        AGENT_METHOD_NAMES.session_set_config_option,
        AGENT_METHOD_NAMES.session_prompt,
        AGENT_METHOD_NAMES.session_cancel,
        AGENT_METHOD_NAMES.session_list,
        AGENT_METHOD_NAMES.session_delete,
        AGENT_METHOD_NAMES.session_resume,
        AGENT_METHOD_NAMES.session_close,
        AGENT_METHOD_NAMES.logout,
        CLIENT_METHOD_NAMES.session_request_permission,
        CLIENT_METHOD_NAMES.session_update,
        CLIENT_METHOD_NAMES.fs_write_text_file,
        CLIENT_METHOD_NAMES.fs_read_text_file,
        CLIENT_METHOD_NAMES.terminal_create,
        CLIENT_METHOD_NAMES.terminal_output,
        CLIENT_METHOD_NAMES.terminal_release,
        CLIENT_METHOD_NAMES.terminal_wait_for_exit,
        CLIENT_METHOD_NAMES.terminal_kill,
        CLIENT_METHOD_NAMES.elicitation_create,
        CLIENT_METHOD_NAMES.elicitation_complete,
        PROTOCOL_LEVEL_METHOD_NAMES.cancel_request,
    ]
    .into_iter()
    .collect()
}

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
        name: "_echo_agent/event/ack",
        direction: Direction::ClientNotification,
        capability: ExtensionCapability::Runs,
        summary: "Client acknowledgement cursor that retires outstanding live events.",
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
    MethodDescriptor {
        name: "_echo_agent/extension/stream",
        direction: Direction::ClientNotification,
        capability: ExtensionCapability::ExtensionBridge,
        summary: "Chunk or terminal event for a reverse-callback stream.",
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
        summary: "Memory store operations encoded with manifest-identified wire values.",
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
    MethodDescriptor {
        name: "_echo_agent/facade/invoke",
        direction: Direction::Request,
        capability: ExtensionCapability::Runs,
        summary: "Invoke one manifest-identified facade operation with typed wire values.",
    },
];

/// Mechanically validate the catalog against ACP extensibility rules.
/// Returns every violation; an empty vec is the pass condition used by
/// tests and the schema export.
pub fn validate_catalog(catalog: &[MethodDescriptor]) -> Vec<String> {
    let mut problems = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    let official_methods = official_acp_v1_methods();
    for method in catalog {
        if !method.name.starts_with(crate::EXTENSION_NAMESPACE) {
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
        if official_methods.contains(method.name) {
            problems.push(format!(
                "method {} collides with a standard ACP method",
                method.name
            ));
        }
        if method.params_schema().is_empty() {
            problems.push(format!("method {} has no params schema", method.name));
        }
        let is_notification = matches!(
            method.direction,
            Direction::ClientNotification | Direction::HostNotification
        );
        if is_notification && method.result_schema().is_some() {
            problems.push(format!(
                "notification {} must not declare a result schema",
                method.name
            ));
        }
        if !is_notification && method.result_schema().is_none() {
            problems.push(format!("request {} has no result schema", method.name));
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
            "_echo_agent/extension/stream",
            "_echo_agent/facade/invoke",
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
