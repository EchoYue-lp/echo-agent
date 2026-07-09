//! A2A protocol type definitions
//!
//! Agent Card, Task, and other types defined according to the Google A2A protocol specification.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ── JSON-RPC / A2A constants ─────────────────────────────────────────────────

/// JSON-RPC protocol version
pub const JSONRPC_VERSION: &str = "2.0";

/// A2A method names
pub const METHOD_SEND: &str = "tasks/send";
/// Subscribe to task updates.
pub const METHOD_SEND_SUBSCRIBE: &str = "tasks/sendSubscribe";
/// Get task status.
pub const METHOD_GET: &str = "tasks/get";
/// Cancel a running task.
pub const METHOD_CANCEL: &str = "tasks/cancel";

/// A2A error codes
pub const ERROR_CODE_PARSE: i64 = -32700;
/// Method not found.
pub const ERROR_CODE_METHOD_NOT_FOUND: i64 = -32601;
/// Invalid parameters.
pub const ERROR_CODE_INVALID_PARAMS: i64 = -32602;
/// Task execution failed.
pub const ERROR_CODE_TASK_FAILED: i64 = -32000;
/// Task not found.
pub const ERROR_CODE_TASK_NOT_FOUND: i64 = -32001;
/// Task is already in terminal state.
pub const ERROR_CODE_TERMINAL_STATE: i64 = -32002;
/// Invalid state transition.
pub const ERROR_CODE_INVALID_TRANSITION: i64 = -32003;

// ── Agent Card ───────────────────────────────────────────────────────────────

/// Agent Card — describes an Agent's capabilities and interfaces (A2A spec core type)
///
/// Published via the `/.well-known/agent.json` endpoint for discovery and invocation by other Agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCard {
    /// Agent name
    pub name: String,
    /// Agent description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Agent service URL endpoint
    pub url: String,
    /// Agent version
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Agent provider information
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<AgentProvider>,
    /// Agent skills list
    #[serde(default)]
    pub skills: Vec<AgentSkill>,
    /// Supported input content types
    #[serde(default = "default_content_types")]
    pub default_input_modes: Vec<String>,
    /// Supported output content types
    #[serde(default = "default_content_types")]
    pub default_output_modes: Vec<String>,
    /// Authentication methods
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authentication: Option<AgentAuthentication>,
    /// Extra capability flags
    #[serde(default)]
    pub capabilities: AgentCapabilities,
}

fn default_content_types() -> Vec<String> {
    vec!["text/plain".to_string()]
}

impl AgentCard {
    /// Create an AgentCard builder
    pub fn builder(name: impl Into<String>, url: impl Into<String>) -> AgentCardBuilder {
        AgentCardBuilder {
            name: name.into(),
            url: url.into(),
            description: None,
            version: None,
            provider: None,
            skills: Vec::new(),
            default_input_modes: default_content_types(),
            default_output_modes: default_content_types(),
            authentication: None,
            capabilities: AgentCapabilities::default(),
        }
    }

    /// Auto-generate an Agent Card from an existing Agent trait object
    pub fn from_agent(agent: &dyn crate::agent::Agent, url: impl Into<String>) -> Self {
        let skills: Vec<AgentSkill> = agent
            .tool_definitions()
            .into_iter()
            .map(|td| AgentSkill::new(&td.function.name, &td.function.description))
            .collect();

        AgentCard {
            name: agent.name().to_string(),
            description: Some(agent.system_prompt().to_string()),
            url: url.into(),
            version: Some("1.0.0".to_string()),
            provider: None,
            skills,
            default_input_modes: default_content_types(),
            default_output_modes: default_content_types(),
            authentication: None,
            capabilities: AgentCapabilities::default(),
        }
    }
}

/// Agent Card builder
pub struct AgentCardBuilder {
    name: String,
    url: String,
    description: Option<String>,
    version: Option<String>,
    provider: Option<AgentProvider>,
    skills: Vec<AgentSkill>,
    default_input_modes: Vec<String>,
    default_output_modes: Vec<String>,
    authentication: Option<AgentAuthentication>,
    capabilities: AgentCapabilities,
}

impl AgentCardBuilder {
    /// Set the description of the agent.
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Set the version of the agent.
    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    /// Set the provider information of the agent.
    pub fn provider(mut self, provider: AgentProvider) -> Self {
        self.provider = Some(provider);
        self
    }

    /// Add a single skill to the agent.
    pub fn skill(mut self, skill: AgentSkill) -> Self {
        self.skills.push(skill);
        self
    }

    /// Add multiple skills to the agent.
    pub fn skills(mut self, skills: Vec<AgentSkill>) -> Self {
        self.skills.extend(skills);
        self
    }

    /// Set the default input modes (content types) supported by the agent.
    pub fn input_modes(mut self, modes: Vec<impl Into<String>>) -> Self {
        self.default_input_modes = modes.into_iter().map(|m| m.into()).collect();
        self
    }

    /// Set the default output modes (content types) supported by the agent.
    pub fn output_modes(mut self, modes: Vec<impl Into<String>>) -> Self {
        self.default_output_modes = modes.into_iter().map(|m| m.into()).collect();
        self
    }

    /// Set authentication configuration for the agent.
    pub fn authentication(mut self, auth: AgentAuthentication) -> Self {
        self.authentication = Some(auth);
        self
    }

    /// Enable streaming capability for the agent.
    pub fn streaming(mut self) -> Self {
        self.capabilities.streaming = true;
        self
    }

    /// Enable push notifications capability for the agent.
    pub fn push_notifications(mut self) -> Self {
        self.capabilities.push_notifications = true;
        self
    }

    /// Build the AgentCard with the configured fields.
    pub fn build(self) -> AgentCard {
        AgentCard {
            name: self.name,
            description: self.description,
            url: self.url,
            version: self.version,
            provider: self.provider,
            skills: self.skills,
            default_input_modes: self.default_input_modes,
            default_output_modes: self.default_output_modes,
            authentication: self.authentication,
            capabilities: self.capabilities,
        }
    }
}

/// Agent provider information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProvider {
    /// Organization/company name
    pub organization: String,
    /// Contact URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl AgentProvider {
    /// Create a new AgentProvider with the given organization name.
    pub fn new(organization: impl Into<String>) -> Self {
        Self {
            organization: organization.into(),
            url: None,
        }
    }

    /// Set the URL for the agent provider.
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }
}

/// Agent skill description
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSkill {
    /// Skill ID
    pub id: String,
    /// Skill name
    pub name: String,
    /// Skill description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Example inputs
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<String>,
    /// Supported input types
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input_modes: Vec<String>,
    /// Supported output types
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub output_modes: Vec<String>,
    /// Custom tags
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

impl AgentSkill {
    /// Create a new AgentSkill with the given name and description.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        let name_str: String = name.into();
        Self {
            id: name_str.clone(),
            name: name_str,
            description: Some(description.into()),
            examples: Vec::new(),
            input_modes: Vec::new(),
            output_modes: Vec::new(),
            tags: Vec::new(),
        }
    }

    /// Add examples to the skill.
    pub fn with_examples(mut self, examples: Vec<impl Into<String>>) -> Self {
        self.examples = examples.into_iter().map(|e| e.into()).collect();
        self
    }

    /// Add tags to the skill.
    pub fn with_tags(mut self, tags: Vec<impl Into<String>>) -> Self {
        self.tags = tags.into_iter().map(|t| t.into()).collect();
        self
    }
}

/// Agent authentication configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentAuthentication {
    /// Authentication scheme list
    pub schemes: Vec<AuthenticationScheme>,
}

/// Authentication scheme
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthenticationScheme {
    /// Scheme type: "apiKey", "bearer", "oauth2", etc.
    pub scheme: String,
    /// Additional configuration
    #[serde(flatten)]
    pub config: HashMap<String, serde_json::Value>,
}

/// Agent capability flags
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCapabilities {
    /// Whether streaming output is supported
    #[serde(default)]
    pub streaming: bool,
    /// Whether push notifications are supported
    #[serde(default)]
    pub push_notifications: bool,
    /// Whether session state is supported
    #[serde(default)]
    pub state_transition_history: bool,
}

// ── A2A Task state machine ───────────────────────────────────────────────────
//
//  submitted → working → [input-required] → completed / failed
//                       ↑___________________↓
//
//  Terminal states: completed, failed, canceled

/// Task lifecycle state (A2A spec state machine)
///
/// ```text
/// submitted → working → completed
///                     → failed
///                     → input-required ⇄ working
///
/// Any non-terminal state → canceled
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskState {
    /// Task submitted, awaiting processing
    Submitted,
    /// Agent is executing
    Working,
    /// Agent needs more input to continue
    InputRequired,
    /// Task completed successfully
    Completed,
    /// Task execution failed
    Failed,
    /// Task was canceled
    Canceled,
}

impl TaskState {
    /// Whether this is a terminal state (completed / failed / canceled)
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Canceled)
    }

    /// Check whether a state transition is valid
    pub fn can_transition_to(self, next: Self) -> bool {
        if self.is_terminal() {
            return false;
        }
        matches!(
            (self, next),
            (Self::Submitted, Self::Working)
                | (Self::Submitted, Self::Canceled)
                | (Self::Working, Self::Completed)
                | (Self::Working, Self::Failed)
                | (Self::Working, Self::InputRequired)
                | (Self::Working, Self::Canceled)
                | (Self::InputRequired, Self::Working)
                | (Self::InputRequired, Self::Canceled)
        )
    }
}

impl std::fmt::Display for TaskState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Submitted => write!(f, "submitted"),
            Self::Working => write!(f, "working"),
            Self::InputRequired => write!(f, "input-required"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
            Self::Canceled => write!(f, "canceled"),
        }
    }
}

// ── A2A Task types ───────────────────────────────────────────────────────────

/// A2A task request
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A2ATaskRequest {
    /// JSON-RPC version
    pub jsonrpc: String,
    /// Request ID
    pub id: String,
    /// Method name
    pub method: String,
    /// Parameters
    pub params: A2ATaskParams,
}

/// A2A task parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A2ATaskParams {
    /// Task ID (optional; omitted for new tasks)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Session ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Message content
    pub message: A2AMessage,
}

/// A2A message
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A2AMessage {
    /// Role: "user" or "agent"
    pub role: String,
    /// Message parts list
    pub parts: Vec<A2APart>,
}

/// Message content part
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum A2APart {
    /// Text content
    #[serde(rename = "text")]
    Text {
        /// Text content
        text: String,
    },
    /// File content
    #[serde(rename = "file")]
    File {
        /// MIME type of the file.
        #[serde(rename = "mimeType")]
        mime_type: String,
        /// Base64-encoded file data.
        data: String,
    },
}

impl A2AMessage {
    /// Create a user text message
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            parts: vec![A2APart::Text { text: text.into() }],
        }
    }

    /// Create an Agent text message
    pub fn agent_text(text: impl Into<String>) -> Self {
        Self {
            role: "agent".to_string(),
            parts: vec![A2APart::Text { text: text.into() }],
        }
    }

    /// Get all text content
    pub fn text_content(&self) -> String {
        self.parts
            .iter()
            .filter_map(|p| match p {
                A2APart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// A2A task response
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A2ATaskResponse {
    /// JSON-RPC version
    pub jsonrpc: String,
    /// Request ID (None when parsing failed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Task result
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<A2ATask>,
    /// Error information
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<A2AError>,
}

/// A2A task
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A2ATask {
    /// Task ID
    pub id: String,
    /// Session ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Task status
    pub status: A2ATaskStatus,
    /// Message history
    #[serde(default)]
    pub history: Vec<A2AMessage>,
    /// Agent-produced artifacts
    #[serde(default)]
    pub artifacts: Vec<A2AArtifact>,
}

/// Task status
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A2ATaskStatus {
    /// State enum
    pub state: TaskState,
    /// Status message (may contain Agent reply or error description)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<A2AMessage>,
    /// Status change timestamp (ISO 8601)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
}

impl A2ATaskStatus {
    /// Create a new task status with the given state and current timestamp.
    pub fn new(state: TaskState) -> Self {
        Self {
            state,
            message: None,
            timestamp: Some(crate::utils::time::now_local().to_rfc3339()),
        }
    }

    /// Create a new task status with the given state and message.
    pub fn with_message(state: TaskState, message: A2AMessage) -> Self {
        Self {
            state,
            message: Some(message),
            timestamp: Some(crate::utils::time::now_local().to_rfc3339()),
        }
    }
}

/// Agent artifact (output)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A2AArtifact {
    /// Artifact name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Artifact index in the list (identifies the same artifact during streaming appends)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
    /// Artifact content parts
    pub parts: Vec<A2APart>,
    /// Whether to append to an existing artifact with the same index (streaming scenario)
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub append: bool,
}

/// A2A error
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct A2AError {
    /// Error code
    pub code: i32,
    /// Error message
    pub message: String,
}

// ── A2A streaming event types ─────────────────────────────────────────────────
//
// Used for tasks/sendSubscribe SSE streaming responses.
// Each event is one line of JSON, format: `data: <json>\n\n`

/// Event type in streaming responses
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum A2AStreamEvent {
    /// Task status change event
    #[serde(rename = "status")]
    StatusUpdate(TaskStatusUpdateEvent),
    /// Artifact update event (streaming output)
    #[serde(rename = "artifact")]
    ArtifactUpdate(TaskArtifactUpdateEvent),
}

/// Task status change event
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskStatusUpdateEvent {
    /// Task ID
    pub task_id: String,
    /// New status
    pub status: A2ATaskStatus,
    /// Whether this is the final event for the task
    #[serde(rename = "final", default)]
    pub is_final: bool,
}

/// Artifact update event (incremental output)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskArtifactUpdateEvent {
    /// Task ID
    pub task_id: String,
    /// Updated artifact
    pub artifact: A2AArtifact,
    /// Whether this is the final event for the task
    #[serde(rename = "final", default)]
    pub is_final: bool,
}

/// Streaming JSON-RPC response wrapper (carrier for SSE `data:` lines)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A2AStreamResponse {
    /// JSON-RPC version
    pub jsonrpc: String,
    /// Request ID
    pub id: String,
    /// Event result
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<A2AStreamEvent>,
    /// Error information
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<A2AError>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_card_builder() {
        let card = AgentCard::builder("test-agent", "http://localhost:8080")
            .description("Test agent")
            .version("1.0.0")
            .skill(AgentSkill::new("calc", "Math calculation"))
            .streaming()
            .build();

        assert_eq!(card.name, "test-agent");
        assert_eq!(card.description.as_deref(), Some("Test agent"));
        assert_eq!(card.skills.len(), 1);
        assert!(card.capabilities.streaming);
    }

    #[test]
    fn test_agent_card_serialization() {
        let card = AgentCard::builder("test", "http://localhost")
            .skill(AgentSkill::new("echo", "Echo"))
            .build();

        let json = serde_json::to_string_pretty(&card).unwrap();
        assert!(json.contains("\"name\":"));
        assert!(json.contains("\"skills\":"));

        let parsed: AgentCard = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "test");
    }

    #[test]
    fn test_a2a_message() {
        let msg = A2AMessage::user_text("Hello");
        assert_eq!(msg.role, "user");
        assert_eq!(msg.text_content(), "Hello");
    }

    #[test]
    fn test_agent_skill() {
        let skill = AgentSkill::new("translate", "Translate")
            .with_tags(vec!["nlp", "translation"])
            .with_examples(vec!["Translate 'hello' to Chinese"]);

        assert_eq!(skill.id, "translate");
        assert_eq!(skill.tags.len(), 2);
        assert_eq!(skill.examples.len(), 1);
    }

    // ── TaskState state machine tests ─────────────────────────

    #[test]
    fn test_task_state_terminal() {
        assert!(!TaskState::Submitted.is_terminal());
        assert!(!TaskState::Working.is_terminal());
        assert!(!TaskState::InputRequired.is_terminal());
        assert!(TaskState::Completed.is_terminal());
        assert!(TaskState::Failed.is_terminal());
        assert!(TaskState::Canceled.is_terminal());
    }

    #[test]
    fn test_task_state_transitions() {
        assert!(TaskState::Submitted.can_transition_to(TaskState::Working));
        assert!(TaskState::Submitted.can_transition_to(TaskState::Canceled));
        assert!(!TaskState::Submitted.can_transition_to(TaskState::Completed));

        assert!(TaskState::Working.can_transition_to(TaskState::Completed));
        assert!(TaskState::Working.can_transition_to(TaskState::Failed));
        assert!(TaskState::Working.can_transition_to(TaskState::InputRequired));
        assert!(TaskState::Working.can_transition_to(TaskState::Canceled));
        assert!(!TaskState::Working.can_transition_to(TaskState::Submitted));

        // input-required ⇄ working cycle
        assert!(TaskState::InputRequired.can_transition_to(TaskState::Working));
        assert!(TaskState::InputRequired.can_transition_to(TaskState::Canceled));
        assert!(!TaskState::InputRequired.can_transition_to(TaskState::Completed));

        // terminal states cannot transition
        assert!(!TaskState::Completed.can_transition_to(TaskState::Working));
        assert!(!TaskState::Failed.can_transition_to(TaskState::Working));
        assert!(!TaskState::Canceled.can_transition_to(TaskState::Working));
    }

    #[test]
    fn test_task_state_serde_kebab_case() {
        let json = serde_json::to_string(&TaskState::InputRequired).unwrap();
        assert_eq!(json, "\"input-required\"");

        let parsed: TaskState = serde_json::from_str("\"input-required\"").unwrap();
        assert_eq!(parsed, TaskState::InputRequired);

        let parsed: TaskState = serde_json::from_str("\"working\"").unwrap();
        assert_eq!(parsed, TaskState::Working);
    }

    #[test]
    fn test_task_status_with_timestamp() {
        let status = A2ATaskStatus::new(TaskState::Working);
        assert_eq!(status.state, TaskState::Working);
        assert!(status.timestamp.is_some());
        assert!(status.message.is_none());

        let status =
            A2ATaskStatus::with_message(TaskState::Completed, A2AMessage::agent_text("done"));
        assert_eq!(status.state, TaskState::Completed);
        assert!(status.message.is_some());
    }

    #[test]
    fn test_artifact_with_streaming_fields() {
        let artifact = A2AArtifact {
            name: Some("output".to_string()),
            index: Some(0),
            parts: vec![A2APart::Text {
                text: "chunk".into(),
            }],
            append: true,
        };
        let json = serde_json::to_string(&artifact).unwrap();
        assert!(json.contains("\"index\":0"));
        assert!(json.contains("\"append\":true"));

        let non_append = A2AArtifact {
            name: None,
            index: None,
            parts: vec![A2APart::Text {
                text: "full".into(),
            }],
            append: false,
        };
        let json = serde_json::to_string(&non_append).unwrap();
        assert!(!json.contains("index"));
        assert!(!json.contains("append"));
    }

    #[test]
    fn test_stream_event_serialization() {
        let event = A2AStreamEvent::StatusUpdate(TaskStatusUpdateEvent {
            task_id: "t1".into(),
            status: A2ATaskStatus::new(TaskState::Working),
            is_final: false,
        });
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"status\""));
        assert!(json.contains("\"working\""));

        let event = A2AStreamEvent::ArtifactUpdate(TaskArtifactUpdateEvent {
            task_id: "t1".into(),
            artifact: A2AArtifact {
                name: None,
                index: Some(0),
                parts: vec![A2APart::Text { text: "hi".into() }],
                append: true,
            },
            is_final: false,
        });
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"artifact\""));
    }
}
