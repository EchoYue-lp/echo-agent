//! Namespaced echo-agent capability for ACP `initialize` `_meta`.
//!
//! ACP's extensibility rule (https://agentclientprotocol.com/protocol/v1/extensibility)
//! puts vendor data under `_meta` and vendor methods behind underscore
//! prefixes declared as capabilities. The Host publishes this structure under
//! the `echo_agent` key of its `initialize` `_meta`; a standard ACP client
//! ignores it and keeps the standard profile, while an echo-agent SDK client
//! validates it and fails closed (design §10.2) when the extension version,
//! digest, required capability or feature set does not match.
//!
//! Standard ACP compliance and full SDK negotiation stay independent: this
//! capability never modifies a standard ACP field.

use serde::{Deserialize, Serialize};

use crate::scalar::WireDuration;

/// Extension capabilities the Host may expose. Each maps to a family of
/// `_echo_agent/*` methods; the method catalog asserts that every method's
/// required capability is declared here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionCapability {
    /// `agent/*` construction, description and close.
    AgentLifecycle,
    /// `session/*` extension handles beyond standard ACP sessions.
    SessionHandles,
    /// `run/*` start, wait, steer, cancel and receipts.
    Runs,
    /// `run/replay` bounded event replay and gap reporting.
    EventReplay,
    /// `task/*` TaskRun/PlanTask graph operations.
    TaskGraph,
    /// `subagent/*` dispatch and control.
    Subagents,
    /// `extension/*` registration and reverse invocation bridge.
    ExtensionBridge,
    /// Structured output contracts on runs.
    StructuredOutput,
}

impl ExtensionCapability {
    pub fn as_str(&self) -> &'static str {
        match self {
            ExtensionCapability::AgentLifecycle => "agent_lifecycle",
            ExtensionCapability::SessionHandles => "session_handles",
            ExtensionCapability::Runs => "runs",
            ExtensionCapability::EventReplay => "event_replay",
            ExtensionCapability::TaskGraph => "task_graph",
            ExtensionCapability::Subagents => "subagents",
            ExtensionCapability::ExtensionBridge => "extension_bridge",
            ExtensionCapability::StructuredOutput => "structured_output",
        }
    }
}

/// Resource bounds the Host enforces (design §10.2/§16). All bounds are
/// inclusive maxima; exceeding them fails with the matching typed extension
/// error instead of unbounded growth.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EchoLimits {
    /// Maximum serialized request/response payload in bytes.
    pub max_message_bytes: u64,
    /// Maximum events buffered per subscriber before backpressure/gap.
    pub max_stream_buffer_events: u64,
    /// Maximum concurrently executing reverse callbacks.
    pub max_callback_concurrency: u32,
    /// Default reverse-callback deadline.
    pub callback_timeout: WireDuration,
    /// Maximum events returned by one replay request.
    pub max_replay_events: u64,
    /// Maximum structured output payload in bytes.
    pub max_structured_output_bytes: u64,
}

/// The capability object published under `initialize._meta.echo_agent`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EchoAgentCapability {
    /// `_echo_agent` extension protocol version (governed independently from
    /// the ACP wire version and crate versions, design §18).
    pub extension_protocol_version: u32,
    /// Digest of the extension contract (schema + catalog), computed by the
    /// schema export tool and pinned in the contract artifacts.
    pub contract_digest: String,
    /// Leaf Cargo features compiled into the Host, sorted. Operations
    /// requiring absent features fail with `feature_unavailable`.
    pub features: Vec<String>,
    /// Declared extension capabilities with required/optional flag.
    pub capabilities: Vec<CapabilityDeclaration>,
    pub limits: EchoLimits,
}

/// One declared capability. Required capabilities must be understood by the
/// SDK client for initialization to succeed; optional ones may be ignored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CapabilityDeclaration {
    pub capability: ExtensionCapability,
    pub required: bool,
}

impl EchoAgentCapability {
    /// Validate negotiation preconditions on the client side: non-empty
    /// digest, sorted deduplicated features, at least one declared
    /// capability. Returns the list of unsatisfied preconditions.
    pub fn validate_shape(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if self.contract_digest.trim().is_empty() {
            problems.push("contract_digest must be non-empty".to_string());
        }
        let mut sorted = self.features.clone();
        sorted.sort();
        sorted.dedup();
        if sorted != self.features {
            problems.push("features must be sorted and deduplicated".to_string());
        }
        if self.capabilities.is_empty() {
            problems.push("at least one capability must be declared".to_string());
        }
        if self.limits.max_message_bytes == 0 {
            problems.push("max_message_bytes must be positive".to_string());
        }
        problems
    }

    /// Whether a capability is declared (required or optional).
    pub fn declares(&self, capability: ExtensionCapability) -> bool {
        self.capabilities.iter().any(|d| d.capability == capability)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capability() -> EchoAgentCapability {
        EchoAgentCapability {
            extension_protocol_version: 1,
            contract_digest: "sha256:abc".to_string(),
            features: vec!["mcp".to_string(), "subagent".to_string()],
            capabilities: vec![CapabilityDeclaration {
                capability: ExtensionCapability::Runs,
                required: true,
            }],
            limits: EchoLimits {
                max_message_bytes: 1_048_576,
                max_stream_buffer_events: 1024,
                max_callback_concurrency: 8,
                callback_timeout: WireDuration::from_nanos(30_000_000_000),
                max_replay_events: 512,
                max_structured_output_bytes: 262_144,
            },
        }
    }

    #[test]
    fn valid_capability_has_no_problems() {
        assert!(capability().validate_shape().is_empty());
    }

    #[test]
    fn unsorted_features_are_reported() {
        let mut cap = capability();
        cap.features = vec!["subagent".to_string(), "mcp".to_string()];
        assert!(cap.validate_shape().iter().any(|p| p.contains("sorted")));
    }

    #[test]
    fn round_trip() {
        let cap = capability();
        let json = serde_json::to_string(&cap).unwrap_or_default();
        let back: EchoAgentCapability =
            serde_json::from_str(&json).unwrap_or_else(|_| EchoAgentCapability {
                extension_protocol_version: 0,
                contract_digest: String::new(),
                features: vec![],
                capabilities: vec![],
                limits: EchoLimits {
                    max_message_bytes: 0,
                    max_stream_buffer_events: 0,
                    max_callback_concurrency: 0,
                    callback_timeout: WireDuration::from_nanos(0),
                    max_replay_events: 0,
                    max_structured_output_bytes: 0,
                },
            });
        assert!(back.declares(ExtensionCapability::Runs));
        assert_eq!(back.extension_protocol_version, 1);
    }
}
