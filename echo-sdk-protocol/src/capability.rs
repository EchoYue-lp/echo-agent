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

use crate::scalar::{WireDuration, WireU64};

/// `_meta` key under which the Host publishes [`EchoAgentCapability`] during
/// `initialize` (`agentCapabilities._meta.echo_agent`) and under which the
/// SDK Client publishes its [`EchoAgentClientHello`]
/// (`clientCapabilities._meta.echo_agent`). ACP Extensibility keeps vendor
/// data inside `_meta`; standard ACP fields are never modified.
pub const ECHO_AGENT_META_KEY: &str = "echo_agent";

/// Build the `_meta` object that carries one echo-agent capability value.
pub fn meta_with_entry(
    value: serde_json::Value,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let entry = serde_json::to_value(value)
        .map_err(|error| format!("failed to encode echo-agent capability meta: {error}"))?;
    let mut meta = serde_json::Map::new();
    meta.insert(ECHO_AGENT_META_KEY.to_string(), entry);
    Ok(meta)
}

/// Read the raw echo-agent entry from a `_meta` object. `None` means the peer
/// did not send the key at all (a plain standard ACP peer), which is distinct
/// from `Some(Err(..))` (present but malformed).
pub fn meta_entry(
    meta: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Option<Result<&serde_json::Value, String>> {
    meta.map(|meta| {
        meta.get(ECHO_AGENT_META_KEY)
            .ok_or_else(|| "echo-agent meta entry is missing".to_string())
    })
}

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
    pub max_message_bytes: WireU64,
    /// Maximum events buffered per subscriber before backpressure/gap.
    pub max_stream_buffer_events: WireU64,
    /// Maximum concurrently executing reverse callbacks.
    #[schemars(range(min = 1))]
    pub max_callback_concurrency: u32,
    /// Default reverse-callback deadline.
    pub callback_timeout: WireDuration,
    /// Maximum events returned by one replay request.
    pub max_replay_events: WireU64,
    /// Maximum structured output payload in bytes.
    pub max_structured_output_bytes: WireU64,
    /// Maximum not-yet-acknowledged live events per stream. Reaching this
    /// bound emits one gap notification and pauses live delivery until the
    /// Client acknowledges a cursor (`_echo_agent/event/ack`).
    pub max_outstanding_live_events: WireU64,
    /// Maximum cumulative serialized live event bytes per stream before the
    /// gap/pause behavior above applies.
    pub max_stream_buffer_bytes: WireU64,
    /// Maximum cumulative serialized event bytes returned by one replay.
    pub max_replay_bytes: WireU64,
    /// Maximum simultaneously issued (open) Agent/Session/Run/Stream handles
    /// per connection.
    pub max_open_handles: WireU64,
}

/// The capability object published under `initialize._meta.echo_agent`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EchoAgentCapability {
    /// `_echo_agent` extension protocol version (governed independently from
    /// the ACP wire version and crate versions, design §18).
    pub extension_protocol_version: u32,
    /// Digest of the extension contract (schema + catalog), computed by the
    /// schema export tool and pinned in the contract artifacts.
    #[schemars(regex(pattern = "^sha256:[0-9a-fA-F]{64}$"))]
    pub contract_digest: String,
    /// Digest of the generated source-compatibility inputs
    /// (`contracts/sdk/source-contract.json`: Cargo.lock + facade inventory +
    /// parity manifest). Negotiated separately from `contract_digest`; both
    /// must match for the SDK Client to enter Extended mode.
    #[schemars(regex(pattern = "^sha256:[0-9a-fA-F]{64}$"))]
    pub source_contract_digest: String,
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
    /// digests, sorted deduplicated features, at least one declared
    /// capability. Returns the list of unsatisfied preconditions.
    pub fn validate_shape(&self) -> Vec<String> {
        let mut problems = Vec::new();
        for (name, digest) in [
            ("contract_digest", &self.contract_digest),
            ("source_contract_digest", &self.source_contract_digest),
        ] {
            let digest_is_valid = digest.strip_prefix("sha256:").is_some_and(|hex| {
                hex.chars().count() == 64
                    && hex.chars().all(|character| character.is_ascii_hexdigit())
            });
            if !digest_is_valid {
                problems.push(format!("{name} must be sha256 plus 64 hex characters"));
            }
        }
        if self.extension_protocol_version != crate::EXTENSION_PROTOCOL_VERSION {
            problems.push(format!(
                "unsupported extension_protocol_version {}",
                self.extension_protocol_version
            ));
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
        let mut seen_capabilities = std::collections::BTreeSet::new();
        for declaration in &self.capabilities {
            if !seen_capabilities.insert(declaration.capability.as_str()) {
                problems.push(format!(
                    "duplicate capability {}",
                    declaration.capability.as_str()
                ));
            }
        }
        for (name, value) in [
            ("max_message_bytes", &self.limits.max_message_bytes),
            (
                "max_stream_buffer_events",
                &self.limits.max_stream_buffer_events,
            ),
            ("max_replay_events", &self.limits.max_replay_events),
            (
                "max_structured_output_bytes",
                &self.limits.max_structured_output_bytes,
            ),
            (
                "max_outstanding_live_events",
                &self.limits.max_outstanding_live_events,
            ),
            (
                "max_stream_buffer_bytes",
                &self.limits.max_stream_buffer_bytes,
            ),
            ("max_replay_bytes", &self.limits.max_replay_bytes),
            ("max_open_handles", &self.limits.max_open_handles),
        ] {
            if value.to_u64() == Some(0) {
                problems.push(format!("{name} must be positive"));
            }
        }
        if self.limits.max_callback_concurrency == 0 {
            problems.push("max_callback_concurrency must be positive".to_string());
        }
        if let Err(error) = self.limits.callback_timeout.validate() {
            problems.push(error.to_string());
        }
        problems
    }

    /// Whether a capability is declared (required or optional).
    pub fn declares(&self, capability: ExtensionCapability) -> bool {
        self.capabilities.iter().any(|d| d.capability == capability)
    }

    /// Negotiate this Host advertisement against a parsed Client hello.
    /// Returns every unsatisfied requirement; an empty vec means the
    /// connection enters Extended mode. The plain-client case (no hello at
    /// all) never reaches this function — it stays Standard unconditionally.
    pub fn negotiate_hello(&self, hello: &EchoAgentClientHello) -> Vec<String> {
        let mut problems = Vec::new();
        if hello.extension_protocol_version != self.extension_protocol_version {
            problems.push(format!(
                "hello extension_protocol_version {} does not match host {}",
                hello.extension_protocol_version, self.extension_protocol_version
            ));
        }
        if hello.contract_digest != self.contract_digest {
            problems.push("hello contract_digest does not match host".to_string());
        }
        if hello.source_contract_digest != self.source_contract_digest {
            problems.push("hello source_contract_digest does not match host".to_string());
        }
        for feature in &hello.required_features {
            if !self.features.iter().any(|host| host == feature) {
                problems.push(format!("required feature {feature} is not compiled in"));
            }
        }
        for capability in &hello.required_capabilities {
            if !self.declares(*capability) {
                problems.push(format!(
                    "required capability {} is not advertised",
                    capability.as_str()
                ));
            }
        }
        problems
    }
}

/// The echo-agent SDK Client hello published under
/// `clientCapabilities._meta.echo_agent`. A Client that sends this object
/// requests Extended mode; the Host enters Extended mode only when every
/// field matches its own [`EchoAgentCapability`]. A missing key means a
/// plain standard ACP Client; a malformed or mismatched hello degrades the
/// connection to Standard mode without failing `initialize` and without
/// creating any extension handles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EchoAgentClientHello {
    /// Extension protocol version the Client was built against.
    pub extension_protocol_version: u32,
    /// Extension contract digest the Client validates.
    #[schemars(regex(pattern = "^sha256:[0-9a-fA-F]{64}$"))]
    pub contract_digest: String,
    /// Source-contract digest the Client validates.
    #[schemars(regex(pattern = "^sha256:[0-9a-fA-F]{64}$"))]
    pub source_contract_digest: String,
    /// Leaf features the Client requires on the Host, sorted.
    pub required_features: Vec<String>,
    /// Extension capabilities the Client requires, sorted by declaration
    /// order-free identity; duplicates are a shape violation.
    pub required_capabilities: Vec<ExtensionCapability>,
}

impl EchoAgentClientHello {
    /// Structural validation before negotiation: digest formats, sorted
    /// deduplicated feature list, non-empty request, unique capabilities.
    pub fn validate_shape(&self) -> Vec<String> {
        let mut problems = Vec::new();
        for (name, digest) in [
            ("contract_digest", &self.contract_digest),
            ("source_contract_digest", &self.source_contract_digest),
        ] {
            let digest_is_valid = digest.strip_prefix("sha256:").is_some_and(|hex| {
                hex.chars().count() == 64
                    && hex.chars().all(|character| character.is_ascii_hexdigit())
            });
            if !digest_is_valid {
                problems.push(format!("{name} must be sha256 plus 64 hex characters"));
            }
        }
        if self.extension_protocol_version != crate::EXTENSION_PROTOCOL_VERSION {
            problems.push(format!(
                "unsupported hello extension_protocol_version {}",
                self.extension_protocol_version
            ));
        }
        let mut sorted = self.required_features.clone();
        sorted.sort();
        sorted.dedup();
        if sorted != self.required_features {
            problems.push("required_features must be sorted and deduplicated".to_string());
        }
        let mut seen = std::collections::BTreeSet::new();
        for capability in &self.required_capabilities {
            if !seen.insert(capability.as_str()) {
                problems.push(format!(
                    "duplicate required capability {}",
                    capability.as_str()
                ));
            }
        }
        problems
    }

    /// Decode a hello from the raw `_meta` entry value.
    pub fn from_meta_value(value: &serde_json::Value) -> Result<Self, String> {
        serde_json::from_value(value.clone())
            .map_err(|error| format!("malformed echo-agent client hello: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(byte: u8) -> String {
        let hex_char = char::from_digit(u32::from(byte) % 16, 16).unwrap_or('0');
        format!(
            "sha256:{}",
            std::iter::repeat_n(hex_char, 64).collect::<String>()
        )
    }

    fn capability() -> EchoAgentCapability {
        EchoAgentCapability {
            extension_protocol_version: 1,
            contract_digest: digest(0),
            source_contract_digest: digest(1),
            features: vec!["mcp".to_string(), "subagent".to_string()],
            capabilities: vec![CapabilityDeclaration {
                capability: ExtensionCapability::Runs,
                required: true,
            }],
            limits: EchoLimits {
                max_message_bytes: WireU64::from_u64(1_048_576),
                max_stream_buffer_events: WireU64::from_u64(1024),
                max_callback_concurrency: 8,
                callback_timeout: WireDuration::from_nanos(30_000_000_000),
                max_replay_events: WireU64::from_u64(512),
                max_structured_output_bytes: WireU64::from_u64(262_144),
                max_outstanding_live_events: WireU64::from_u64(256),
                max_stream_buffer_bytes: WireU64::from_u64(4_194_304),
                max_replay_bytes: WireU64::from_u64(4_194_304),
                max_open_handles: WireU64::from_u64(512),
            },
        }
    }

    fn hello() -> EchoAgentClientHello {
        EchoAgentClientHello {
            extension_protocol_version: 1,
            contract_digest: digest(0),
            source_contract_digest: digest(1),
            required_features: vec!["mcp".to_string()],
            required_capabilities: vec![ExtensionCapability::Runs],
        }
    }

    #[test]
    fn valid_capability_has_no_problems() {
        assert!(capability().validate_shape().is_empty());
        assert!(hello().validate_shape().is_empty());
    }

    #[test]
    fn unsorted_features_are_reported() {
        let mut cap = capability();
        cap.features = vec!["subagent".to_string(), "mcp".to_string()];
        assert!(cap.validate_shape().iter().any(|p| p.contains("sorted")));
    }

    #[test]
    fn matching_hello_negotiates_and_mismatches_fail_closed() {
        let cap = capability();
        assert!(cap.negotiate_hello(&hello()).is_empty());

        let mut wrong_version = hello();
        wrong_version.extension_protocol_version = 99;
        assert!(
            cap.negotiate_hello(&wrong_version)
                .iter()
                .any(|p| p.contains("extension_protocol_version"))
        );

        let mut wrong_digest = hello();
        wrong_digest.source_contract_digest = digest(2);
        assert!(
            cap.negotiate_hello(&wrong_digest)
                .iter()
                .any(|p| p.contains("source_contract_digest"))
        );

        let mut missing_feature = hello();
        missing_feature.required_features = vec!["chart".to_string()];
        assert!(
            cap.negotiate_hello(&missing_feature)
                .iter()
                .any(|p| p.contains("required feature chart"))
        );

        let mut missing_capability = hello();
        missing_capability.required_capabilities = vec![ExtensionCapability::Subagents];
        assert!(
            cap.negotiate_hello(&missing_capability)
                .iter()
                .any(|p| p.contains("subagents"))
        );
    }

    #[test]
    fn meta_helpers_round_trip() {
        let cap = capability();
        let meta = meta_with_entry(serde_json::to_value(&cap).unwrap_or(serde_json::Value::Null))
            .unwrap_or_default();
        let entry = meta_entry(Some(&meta))
            .and_then(|result| result.ok())
            .unwrap_or(&serde_json::Value::Null);
        let back =
            EchoAgentCapability::deserialize(entry).unwrap_or_else(|_| EchoAgentCapability {
                extension_protocol_version: 0,
                contract_digest: String::new(),
                source_contract_digest: String::new(),
                features: vec![],
                capabilities: vec![],
                limits: EchoLimits {
                    max_message_bytes: WireU64::from_u64(0),
                    max_stream_buffer_events: WireU64::from_u64(0),
                    max_callback_concurrency: 0,
                    callback_timeout: WireDuration::from_nanos(0),
                    max_replay_events: WireU64::from_u64(0),
                    max_structured_output_bytes: WireU64::from_u64(0),
                    max_outstanding_live_events: WireU64::from_u64(0),
                    max_stream_buffer_bytes: WireU64::from_u64(0),
                    max_replay_bytes: WireU64::from_u64(0),
                    max_open_handles: WireU64::from_u64(0),
                },
            });
        assert_eq!(back, cap);
        assert!(meta_entry(None).is_none());
    }

    #[test]
    fn hello_decodes_from_meta_value_and_rejects_garbage() {
        let value = serde_json::to_value(hello()).unwrap_or(serde_json::Value::Null);
        assert!(EchoAgentClientHello::from_meta_value(&value).is_ok());
        assert!(EchoAgentClientHello::from_meta_value(&serde_json::Value::Null).is_err());
    }

    #[test]
    fn round_trip() {
        let cap = capability();
        let json = serde_json::to_string(&cap).unwrap_or_default();
        let fallback = EchoAgentCapability {
            extension_protocol_version: 0,
            contract_digest: String::new(),
            source_contract_digest: String::new(),
            features: vec![],
            capabilities: vec![],
            limits: EchoLimits {
                max_message_bytes: WireU64::from_u64(0),
                max_stream_buffer_events: WireU64::from_u64(0),
                max_callback_concurrency: 0,
                callback_timeout: WireDuration::from_nanos(0),
                max_replay_events: WireU64::from_u64(0),
                max_structured_output_bytes: WireU64::from_u64(0),
                max_outstanding_live_events: WireU64::from_u64(0),
                max_stream_buffer_bytes: WireU64::from_u64(0),
                max_replay_bytes: WireU64::from_u64(0),
                max_open_handles: WireU64::from_u64(0),
            },
        };
        let back: EchoAgentCapability = serde_json::from_str(&json).unwrap_or(fallback);
        assert!(back.declares(ExtensionCapability::Runs));
        assert_eq!(back.extension_protocol_version, 1);
        assert_eq!(back.source_contract_digest, digest(1));
    }
}
