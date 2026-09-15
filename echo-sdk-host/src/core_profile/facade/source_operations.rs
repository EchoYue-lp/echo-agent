//! Canonical source-identity operation adapter.
//!
//! Source identities are not wildcard routes: every entry is admitted by the
//! generated catalog with its exact signature and this adapter is the only
//! dispatch boundary for source-operation items that are not already owned by
//! a typed family. The adapter deliberately handles only operations whose
//! receiver semantics are available from the Host's existing authorities;
//! unknown source identities fail with a typed framework error rather than a
//! misleading `feature_unavailable` response.

use base64::Engine as _;
use echo_agent::agent::Agent;
use echo_sdk_protocol::capability::ExtensionCapability;
use echo_sdk_protocol::error::AgentFailureWire;
use echo_sdk_protocol::error::{EchoSdkError, ExtensionErrorCode, Retryability};
use echo_sdk_protocol::handle::{HandleKind, WireHandle};
#[cfg(feature = "sdk-extension-bridge")]
use echo_sdk_protocol::methods::AgentComponentKindWire;
use echo_sdk_protocol::methods::{
    AgentCloseRequest, AgentEventWire, FeatureOperationRequest, LlmMessageWire, RunInput,
    RunStartRequest, RunSteerRequest,
};
use echo_sdk_protocol::scalar::{WireBytes, WireU64, WireValue};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use super::super::handles::{HandleRegistry, RunRecord};
use super::super::state::CoreProfileState;
use super::super::wire;
use super::registry::CompiledOperationCatalog;

const METHOD: &str = "_echo_agent/facade/invoke";

/// Rust file authorities retained behind the unified facade resource
/// registry. The enum owns the real lease/descriptor guard; adapters only
/// resolve its handle and call the canonical `echo_core::utils::fs` function.
pub(crate) enum FileAuthorityRecord {
    Lease(echo_agent::utils::fs::ExclusiveFileLease),
    Regular(echo_agent::utils::fs::ExistingRegularFileGuard),
    Directory(echo_agent::utils::fs::ExistingDirectoryGuard),
}

/// One concrete memory backend retained behind a facade resource. The memory
/// family consumes the same object through `Store`, while source adapters can
/// still preserve backend-specific atomic operations.
pub(crate) enum MemoryStoreAuthority {
    InMemory(Arc<echo_agent::memory::InMemoryStore>),
    File(Arc<echo_agent::memory::FileStore>),
    #[cfg(feature = "framework-sqlite")]
    Sqlite(Arc<echo_agent::memory::SqliteStore>),
}

impl MemoryStoreAuthority {
    pub(crate) fn store(&self) -> Arc<dyn echo_agent::memory::Store> {
        match self {
            Self::InMemory(store) => store.clone(),
            Self::File(store) => store.clone(),
            #[cfg(feature = "framework-sqlite")]
            Self::Sqlite(store) => store.clone(),
        }
    }
}

#[cfg(all(
    any(feature = "framework-eval", feature = "framework-improve"),
    feature = "sdk-extension-bridge"
))]
struct FailedEvalAgent {
    message: String,
}

#[cfg(all(
    any(feature = "framework-eval", feature = "framework-improve"),
    feature = "sdk-extension-bridge"
))]
impl echo_agent::agent::Agent for FailedEvalAgent {
    fn name(&self) -> &str {
        "sdk-eval-exhausted"
    }

    fn model_name(&self) -> &str {
        "unavailable"
    }

    fn system_prompt(&self) -> &str {
        ""
    }

    fn execute<'a>(
        &'a self,
        _task: &'a str,
    ) -> futures::future::BoxFuture<'a, echo_agent::error::Result<String>> {
        Box::pin(async { Err(echo_agent::error::ReactError::Other(self.message.clone())) })
    }

    fn execute_stream<'a>(
        &'a self,
        _task: &'a str,
    ) -> futures::future::BoxFuture<
        'a,
        echo_agent::error::Result<
            futures::stream::BoxStream<
                'a,
                echo_agent::error::Result<echo_agent::agent::AgentEvent>,
            >,
        >,
    > {
        Box::pin(async {
            use futures::StreamExt as _;
            Ok(futures::stream::once(async {
                Err(echo_agent::error::ReactError::Other(self.message.clone()))
            })
            .boxed())
        })
    }
}

#[cfg(feature = "framework-content-guard")]
pub(crate) struct ContentGuardAuthorityRecord {
    mode: echo_agent::guard::content::ContentGuardMode,
}

#[cfg(feature = "framework-project-rules")]
#[derive(Clone)]
pub(crate) struct InstructionResolverAuthorityRecord {
    working_dir: PathBuf,
    project_root: Option<PathBuf>,
    agents_files_only: bool,
}

pub(crate) fn drop_file_authorities(
    authorities: &std::sync::Mutex<HashMap<String, Arc<FileAuthorityRecord>>>,
    closed: &[String],
) {
    let mut authorities = authorities
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    for id in closed {
        authorities.remove(id);
    }
}

pub(crate) fn drop_tokenizer_authorities(
    authorities: &std::sync::Mutex<HashMap<String, Arc<dyn echo_agent::tokenizer::Tokenizer>>>,
    closed: &[String],
) {
    let mut authorities = authorities
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    for id in closed {
        authorities.remove(id);
    }
}

#[cfg(any(
    feature = "framework-content-guard",
    feature = "framework-project-rules"
))]
pub(crate) fn drop_value_authorities<T>(
    authorities: &std::sync::Mutex<HashMap<String, T>>,
    closed: &[String],
) {
    let mut authorities = authorities
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    for id in closed {
        authorities.remove(id);
    }
}

fn register_source_resource(
    state: &CoreProfileState,
    owner: &str,
    family: &str,
    resource_kind: &str,
) -> Result<WireHandle, EchoSdkError> {
    state
        .handles
        .register_facade_resource(
            state.limits.max_facade_resources,
            family,
            resource_kind,
            Some(owner),
            METHOD,
        )
        .map(|(handle, _)| handle)
}

fn source_resource_at(
    state: &CoreProfileState,
    request: &FeatureOperationRequest,
    owner: &str,
    position: usize,
    family: &str,
    resource_kind: &str,
) -> Result<WireHandle, EchoSdkError> {
    let handle = match request.arguments.get(position) {
        Some(WireValue::Handle(handle)) => handle.clone(),
        _ => {
            return Err(invalid(format!(
                "argument {position} must be a {resource_kind} resource handle"
            )));
        }
    };
    let record = super::owned_resource(&state.handles, &handle, owner, &request.operation)?;
    if record.family != family || record.resource_type != resource_kind {
        return Err(invalid(format!(
            "argument {position} does not address {resource_kind}"
        )));
    }
    Ok(handle)
}

#[cfg(feature = "sdk-extension-bridge")]
pub(crate) fn register_tokenizer_authority(
    state: &CoreProfileState,
    owner: &str,
    tokenizer: Arc<dyn echo_agent::tokenizer::Tokenizer>,
) -> Result<WireHandle, EchoSdkError> {
    let (resource, _) = state.handles.register_facade_resource(
        state.limits.max_facade_resources,
        "tokenizer",
        "tokenizer.reference",
        Some(owner),
        "_echo_agent/extension/invoke",
    )?;
    state
        .facade
        .tokenizer_authorities
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(resource.id.clone(), tokenizer);
    Ok(resource)
}

#[cfg(feature = "sdk-extension-bridge")]
fn tokenizer_authority(
    state: &CoreProfileState,
    request: &FeatureOperationRequest,
) -> Result<Arc<dyn echo_agent::tokenizer::Tokenizer>, EchoSdkError> {
    let handle = request
        .handle
        .as_ref()
        .ok_or_else(|| invalid("Tokenizer::count_tokens requires a tokenizer resource receiver"))?;
    state.handles.check_shape_and_generation(
        handle,
        HandleKind::FacadeResource,
        &request.operation,
    )?;
    let owner = string_argument(request, 0, "owner_session_id")?;
    let record = super::owned_resource(&state.handles, handle, &owner, &request.operation)?;
    if record.family != "tokenizer" || record.resource_type != "tokenizer.reference" {
        return Err(invalid(
            "Tokenizer::count_tokens receiver is not a tokenizer resource",
        ));
    }
    state
        .facade
        .tokenizer_authorities
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&handle.id)
        .cloned()
        .ok_or_else(|| invalid("Tokenizer::count_tokens receiver is closed"))
}

#[cfg(feature = "sdk-extension-bridge")]
pub(crate) fn release_tokenizer_authority(
    state: &CoreProfileState,
    owner: &str,
    handle: &WireHandle,
) {
    if super::owned_resource(&state.handles, handle, owner, METHOD).is_err() {
        return;
    }
    state
        .facade
        .tokenizer_authorities
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(&handle.id);
    let _ = state.handles.close_facade_resource(handle, METHOD);
}

fn invalid(message: impl Into<String>) -> EchoSdkError {
    wire::sdk_error(
        ExtensionErrorCode::InvalidValue,
        message,
        Retryability::Never,
        METHOD,
    )
}

#[cfg(feature = "framework-content-guard")]
fn content_guard_mode(
    value: &str,
) -> Result<echo_agent::guard::content::ContentGuardMode, EchoSdkError> {
    match value {
        "detect" => Ok(echo_agent::guard::content::ContentGuardMode::Detect),
        "redact" => Ok(echo_agent::guard::content::ContentGuardMode::Redact),
        "reject" => Ok(echo_agent::guard::content::ContentGuardMode::Reject),
        _ => Err(invalid(
            "content guard mode must be detect, redact, or reject",
        )),
    }
}

#[cfg(feature = "framework-content-guard")]
fn content_guard_result_value(result: echo_agent::guard::content::ContentGuardResult) -> WireValue {
    match result {
        echo_agent::guard::content::ContentGuardResult::Pass => WireValue::Variant {
            type_id: "echo_core::guard::content::ContentGuardResult".to_string(),
            variant: "pass".to_string(),
            fields: Vec::new(),
        },
        echo_agent::guard::content::ContentGuardResult::Detected { pii_types } => {
            WireValue::Variant {
                type_id: "echo_core::guard::content::ContentGuardResult".to_string(),
                variant: "detected".to_string(),
                fields: vec![echo_sdk_protocol::scalar::WireField {
                    name: "pii_types".to_string(),
                    value: WireValue::List(pii_types.into_iter().map(WireValue::String).collect()),
                }],
            }
        }
        echo_agent::guard::content::ContentGuardResult::Rejected { pii_types } => {
            WireValue::Variant {
                type_id: "echo_core::guard::content::ContentGuardResult".to_string(),
                variant: "rejected".to_string(),
                fields: vec![echo_sdk_protocol::scalar::WireField {
                    name: "pii_types".to_string(),
                    value: WireValue::List(pii_types.into_iter().map(WireValue::String).collect()),
                }],
            }
        }
        echo_agent::guard::content::ContentGuardResult::Redacted(content) => WireValue::Variant {
            type_id: "echo_core::guard::content::ContentGuardResult".to_string(),
            variant: "redacted".to_string(),
            fields: vec![echo_sdk_protocol::scalar::WireField {
                name: "content".to_string(),
                value: WireValue::String(content),
            }],
        },
    }
}

#[cfg(feature = "framework-project-rules")]
fn resolved_instructions_value(
    resolved: echo_agent::project_rules::ResolvedInstructions,
) -> Result<WireValue, EchoSdkError> {
    let sources = resolved
        .sources
        .into_iter()
        .map(|source| {
            Ok(WireValue::Record {
                type_id: "echo_core::project_rules::InstructionSource".to_string(),
                fields: vec![
                    echo_sdk_protocol::scalar::WireField {
                        name: "path".to_string(),
                        value: WireValue::Path(wire::path_to_wire(&source.path).map_err(invalid)?),
                    },
                    echo_sdk_protocol::scalar::WireField {
                        name: "kind".to_string(),
                        value: WireValue::String(source.kind),
                    },
                    echo_sdk_protocol::scalar::WireField {
                        name: "precedence".to_string(),
                        value: WireValue::U64(WireU64::from_u64(
                            u64::try_from(source.precedence).unwrap_or(u64::MAX),
                        )),
                    },
                ],
            })
        })
        .collect::<Result<Vec<_>, EchoSdkError>>()?;
    let project_root = match resolved.project_root {
        Some(path) => WireValue::Variant {
            type_id: "core::option::Option<PathBuf>".to_string(),
            variant: "some".to_string(),
            fields: vec![echo_sdk_protocol::scalar::WireField {
                name: "value".to_string(),
                value: WireValue::Path(wire::path_to_wire(&path).map_err(invalid)?),
            }],
        },
        None => WireValue::Variant {
            type_id: "core::option::Option<PathBuf>".to_string(),
            variant: "none".to_string(),
            fields: Vec::new(),
        },
    };
    Ok(WireValue::Record {
        type_id: "echo_core::project_rules::ResolvedInstructions".to_string(),
        fields: vec![
            echo_sdk_protocol::scalar::WireField {
                name: "content".to_string(),
                value: WireValue::String(resolved.content),
            },
            echo_sdk_protocol::scalar::WireField {
                name: "sources".to_string(),
                value: WireValue::List(sources),
            },
            echo_sdk_protocol::scalar::WireField {
                name: "project_root".to_string(),
                value: project_root,
            },
        ],
    })
}

fn plugin_install_source(
    value: &WireValue,
) -> Result<echo_agent::plugin::InstallSource, EchoSdkError> {
    let WireValue::Variant {
        variant, fields, ..
    } = value
    else {
        return Err(invalid("plugin install source must be a Variant"));
    };
    let field = |name: &str| {
        fields
            .iter()
            .find(|field| field.name == name)
            .map(|field| &field.value)
    };
    match variant.as_str() {
        "local" => match field("path") {
            Some(WireValue::Path(path)) => wire::path_from_wire(path)
                .map(echo_agent::plugin::InstallSource::Local)
                .map_err(invalid),
            _ => Err(invalid("local plugin source requires a Path")),
        },
        "git" => {
            let url = match field("url") {
                Some(WireValue::String(url)) if !url.trim().is_empty() => url.clone(),
                _ => return Err(invalid("git plugin source requires a URL")),
            };
            let subdir = match field("subdir") {
                None | Some(WireValue::Null) => None,
                Some(WireValue::String(value)) => Some(value.clone()),
                _ => return Err(invalid("git plugin subdir must be String or null")),
            };
            Ok(echo_agent::plugin::InstallSource::Git { url, subdir })
        }
        _ => Err(invalid("plugin install source must be local or git")),
    }
}

fn resolved_plugin_components_value(
    value: echo_agent::plugin::ResolvedComponents,
) -> Result<WireValue, EchoSdkError> {
    let paths = |items: Vec<PathBuf>| {
        items
            .into_iter()
            .map(|path| {
                wire::path_to_wire(&path)
                    .map(WireValue::Path)
                    .map_err(invalid)
            })
            .collect::<Result<Vec<_>, EchoSdkError>>()
    };
    let optional_path = |path: Option<PathBuf>| -> Result<WireValue, EchoSdkError> {
        match path {
            Some(path) => Ok(WireValue::Variant {
                type_id: "core::option::Option<PathBuf>".to_string(),
                variant: "some".to_string(),
                fields: vec![echo_sdk_protocol::scalar::WireField {
                    name: "value".to_string(),
                    value: WireValue::Path(wire::path_to_wire(&path).map_err(invalid)?),
                }],
            }),
            None => Ok(WireValue::Variant {
                type_id: "core::option::Option<PathBuf>".to_string(),
                variant: "none".to_string(),
                fields: Vec::new(),
            }),
        }
    };
    Ok(WireValue::Record {
        type_id: "echo_core::plugin::registry::ResolvedComponents".to_string(),
        fields: vec![
            echo_sdk_protocol::scalar::WireField {
                name: "skill_dirs".to_string(),
                value: WireValue::List(paths(value.skill_dirs)?),
            },
            echo_sdk_protocol::scalar::WireField {
                name: "agent_files".to_string(),
                value: WireValue::List(paths(value.agent_files)?),
            },
            echo_sdk_protocol::scalar::WireField {
                name: "hooks_file".to_string(),
                value: optional_path(value.hooks_file)?,
            },
            echo_sdk_protocol::scalar::WireField {
                name: "mcp_config_file".to_string(),
                value: optional_path(value.mcp_config_file)?,
            },
            echo_sdk_protocol::scalar::WireField {
                name: "lsp_config_file".to_string(),
                value: optional_path(value.lsp_config_file)?,
            },
            echo_sdk_protocol::scalar::WireField {
                name: "diagnostics".to_string(),
                value: WireValue::List(
                    value
                        .diagnostics
                        .into_iter()
                        .map(WireValue::String)
                        .collect(),
                ),
            },
        ],
    })
}

fn sandbox_policy_argument(
    request: &FeatureOperationRequest,
    position: usize,
) -> Result<echo_agent::sandbox::SandboxPolicy, EchoSdkError> {
    let value = json_argument(request, position, "sandbox policy")?;
    let object = value
        .as_object()
        .ok_or_else(|| invalid("sandbox policy must be an object"))?;
    let default_level = match object
        .get("default_level")
        .and_then(serde_json::Value::as_str)
    {
        Some("trusted") => echo_agent::sandbox::SecurityLevel::Trusted,
        Some("standard") => echo_agent::sandbox::SecurityLevel::Standard,
        Some("strict") => echo_agent::sandbox::SecurityLevel::Strict,
        Some("maximum") => echo_agent::sandbox::SecurityLevel::Maximum,
        _ => return Err(invalid("sandbox policy default_level is invalid")),
    };
    let isolation = |value: Option<&serde_json::Value>| match value {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(value)) => match value.as_str() {
            "none" => Ok(Some(echo_agent::sandbox::IsolationLevel::None)),
            "process" => Ok(Some(echo_agent::sandbox::IsolationLevel::Process)),
            "os-sandbox" => Ok(Some(echo_agent::sandbox::IsolationLevel::OsSandbox)),
            "container" => Ok(Some(echo_agent::sandbox::IsolationLevel::Container)),
            "orchestrated" => Ok(Some(echo_agent::sandbox::IsolationLevel::Orchestrated)),
            _ => Err(invalid("sandbox policy max_isolation_level is invalid")),
        },
        _ => Err(invalid(
            "sandbox policy max_isolation_level must be string or null",
        )),
    };
    let string_set = |field: &str| -> Result<HashSet<String>, EchoSdkError> {
        object
            .get(field)
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| invalid(format!("sandbox policy {field} must be an array")))?
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .filter(|value| !value.trim().is_empty())
                    .map(str::to_string)
                    .ok_or_else(|| invalid(format!("sandbox policy {field} entry is invalid")))
            })
            .collect()
    };
    Ok(echo_agent::sandbox::SandboxPolicy {
        default_level,
        auto_escalate: object
            .get("auto_escalate")
            .and_then(serde_json::Value::as_bool)
            .ok_or_else(|| invalid("sandbox policy auto_escalate must be boolean"))?,
        max_isolation_level: isolation(object.get("max_isolation_level"))?,
        container_required_languages: string_set("container_required_languages")?,
        trusted_commands: string_set("trusted_commands")?,
    })
}

fn facade_capability_error() -> EchoSdkError {
    wire::sdk_error(
        ExtensionErrorCode::ExtensionCapabilityMismatch,
        "capability feature_surfaces is not advertised",
        Retryability::Never,
        METHOD,
    )
}

fn framework(operation: &str, message: impl Into<String>) -> EchoSdkError {
    let mut error = wire::sdk_error(
        ExtensionErrorCode::FrameworkError,
        message,
        Retryability::Never,
        METHOD,
    );
    error.operation = Some(operation.to_string());
    error
}

fn agent_handle(request: &FeatureOperationRequest) -> Result<&WireHandle, EchoSdkError> {
    let handle = request
        .handle
        .as_ref()
        .ok_or_else(|| invalid("source operation requires an Agent handle"))?;
    if handle.kind != HandleKind::Agent {
        return Err(invalid("source operation requires an Agent handle"));
    }
    Ok(handle)
}

fn no_arguments(request: &FeatureOperationRequest) -> Result<(), EchoSdkError> {
    if request.arguments.is_empty() {
        Ok(())
    } else {
        Err(invalid("source accessor does not accept arguments"))
    }
}

fn execution_usage_value(
    request: &FeatureOperationRequest,
    duration_ms: WireU64,
    tokens_used: Option<WireU64>,
    iterations: Option<WireU64>,
) -> Result<WireValue, EchoSdkError> {
    snapshot_value(
        request,
        serde_json::json!({
            "duration_ms": duration_ms,
            "tokens_used": tokens_used,
            "iterations": iterations,
        }),
    )
}

fn recovered_usage_value(
    request: &FeatureOperationRequest,
    receipt: &echo_sdk_protocol::methods::RunReceiptWire,
) -> Result<WireValue, EchoSdkError> {
    let tokens_used = match receipt.llm_calls.to_u64() {
        Some(0) => None,
        Some(_) => {
            let prompt_tokens = receipt.prompt_tokens.to_u64().ok_or_else(|| {
                framework(
                    &request.operation,
                    "persisted prompt token count is invalid",
                )
            })?;
            let completion_tokens = receipt.completion_tokens.to_u64().ok_or_else(|| {
                framework(
                    &request.operation,
                    "persisted completion token count is invalid",
                )
            })?;
            Some(WireU64::from_u64(
                prompt_tokens.saturating_add(completion_tokens),
            ))
        }
        None => {
            return Err(framework(
                &request.operation,
                "persisted LLM call count is invalid",
            ));
        }
    };
    execution_usage_value(request, receipt.elapsed_ms.clone(), tokens_used, None)
}

fn snapshot_value(
    request: &FeatureOperationRequest,
    value: serde_json::Value,
) -> Result<WireValue, EchoSdkError> {
    WireValue::from_json(value).map_err(|error| framework(&request.operation, error.to_string()))
}

const TURN_OUTCOME_TYPE_ID: &str = "echo_orchestration::runtime::turn_driver::TurnOutcome";

fn turn_outcome_value(outcome: echo_agent::runtime::TurnOutcome) -> WireValue {
    match outcome {
        echo_agent::runtime::TurnOutcome::Completed => WireValue::Variant {
            type_id: TURN_OUTCOME_TYPE_ID.to_string(),
            variant: "completed".to_string(),
            fields: Vec::new(),
        },
        echo_agent::runtime::TurnOutcome::Cancelled => WireValue::Variant {
            type_id: TURN_OUTCOME_TYPE_ID.to_string(),
            variant: "cancelled".to_string(),
            fields: Vec::new(),
        },
        echo_agent::runtime::TurnOutcome::Failed(failure) => WireValue::Variant {
            type_id: TURN_OUTCOME_TYPE_ID.to_string(),
            variant: "failed".to_string(),
            fields: vec![echo_sdk_protocol::scalar::WireField {
                name: "failure".to_string(),
                value: AgentFailureWire::from(&failure).into_wire_value(),
            }],
        },
    }
}

fn turn_outcome_classify(request: &FeatureOperationRequest) -> Result<WireValue, EchoSdkError> {
    if request.handle.is_some() {
        return Err(invalid(
            "TurnOutcome::classify does not accept a receiver handle",
        ));
    }
    let value = request
        .arguments
        .first()
        .ok_or_else(|| invalid("TurnOutcome::classify requires one AgentEventWire argument"))?;
    if request.arguments.len() != 1 {
        return Err(invalid(
            "TurnOutcome::classify accepts exactly one AgentEventWire argument",
        ));
    }
    let wire = AgentEventWire::from_wire_value(value)
        .map_err(|error| invalid(format!("AgentEventWire payload is invalid: {error}")))?;
    let event = match wire {
        AgentEventWire::FinalAnswer { text } => {
            Some(echo_agent::agent::AgentEvent::FinalAnswer(text))
        }
        AgentEventWire::Cancelled => Some(echo_agent::agent::AgentEvent::Cancelled),
        AgentEventWire::Error {
            source,
            message,
            failure,
        } => {
            let failure = AgentFailureWire::from_wire_value(&failure)
                .map_err(|error| invalid(format!("AgentFailureWire payload is invalid: {error}")))?
                .into_framework()
                .map_err(|error| {
                    invalid(format!("AgentFailureWire payload is invalid: {error}"))
                })?;
            Some(echo_agent::agent::AgentEvent::Error {
                source,
                message,
                failure,
            })
        }
        _ => None,
    };
    Ok(event
        .as_ref()
        .and_then(echo_agent::runtime::TurnOutcome::classify)
        .map(turn_outcome_value)
        .unwrap_or(WireValue::Null))
}

fn skill_info_value(skill: &echo_agent::skills::SkillInfo) -> serde_json::Value {
    serde_json::json!({
        "name": skill.name,
        "description": skill.description,
        "tool_names": skill.tool_names,
        "has_prompt_injection": skill.has_prompt_injection,
    })
}

#[cfg(feature = "framework-subagent")]
async fn dispatch_subagent_with_depth(
    state: &CoreProfileState,
    request: &FeatureOperationRequest,
    target: Option<String>,
    task: String,
    depth: u32,
) -> Result<String, EchoSdkError> {
    let agent = session_agent(state, request).await?;
    let session_handle = match request.arguments.first() {
        Some(WireValue::Handle(handle)) if handle.kind == HandleKind::Session => handle,
        Some(WireValue::Handle(_)) => {
            return Err(invalid("delegation requires a Session handle"));
        }
        Some(_) | None => return Err(invalid("delegation requires a Session handle")),
    };
    let session_record = state.handles.session(session_handle)?;
    let authorities = state
        .session_factory
        .session_services(&session_record.acp_session_id)
        .ok_or_else(|| {
            framework(
                &request.operation,
                "Session subagent authority is unavailable",
            )
        })?;
    let definitions = authorities.subagent_registry.list_available().await;
    let target = match target {
        Some(target) => target,
        None => match definitions.first() {
            Some(definition) => definition.name.clone(),
            None => {
                return agent
                    .chat(&task)
                    .await
                    .map_err(|error| framework(&request.operation, error.to_string()));
            }
        },
    };
    let dispatch = echo_agent::agent::subagent::executor::DispatchRequest {
        agent_name: target,
        task,
        mode_override: Some(echo_agent::agent::subagent::types::ExecutionMode::Fork),
        cancel: tokio_util::sync::CancellationToken::new(),
        parent_agent: agent.name().to_string(),
        parent_context: Some(
            echo_agent::agent::subagent::context::SubagentContext::from_parent(
                &agent.tool_definitions(),
                &agent.messages(),
                None,
                &echo_agent::agent::subagent::context::ContextInheritance::fresh_default(),
            ),
        ),
        delegation_policy:
            echo_agent::agent::subagent::executor::DispatchRequest::policy_from_depth(depth),
        // Preserve the stable ACP conversation/run identity for the nested
        // dispatch.  The executor will derive execution/isolation ids at its
        // own safe point; leaving this context absent would detach the child
        // from the parent Session's runtime lineage.
        runtime_context: Some(echo_agent::tools::ExternalRunContext {
            conversation_id: Some(session_record.acp_session_id.clone()),
            run_id: agent.current_run_id(),
            ..Default::default()
        }),
        message: None,
        prompt_payload: None,
        prompt_context: None,
        constraints: Vec::new(),
        background: false,
    };
    let result = authorities
        .subagent_executor
        .dispatch(dispatch)
        .await
        .map_err(|error| framework(&request.operation, error.to_string()))?;
    Ok(result.output)
}

#[cfg(feature = "framework-subagent")]
fn json_argument(
    request: &FeatureOperationRequest,
    position: usize,
    what: &str,
) -> Result<serde_json::Value, EchoSdkError> {
    request
        .arguments
        .get(position)
        .cloned()
        .ok_or_else(|| invalid(format!("{what} is required at argument {position}")))?
        .into_json()
        .map_err(|error| invalid(format!("{what} is not a lossless wire value: {error}")))
}

#[cfg(feature = "framework-subagent")]
fn optional_json_argument(
    request: &FeatureOperationRequest,
    position: usize,
    what: &str,
) -> Result<Option<serde_json::Value>, EchoSdkError> {
    match request.arguments.get(position) {
        Some(WireValue::Null) => Ok(None),
        Some(_) => json_argument(request, position, what).map(Some),
        None => Ok(None),
    }
}

#[cfg(feature = "framework-subagent")]
fn string_field(
    object: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<Option<String>, EchoSdkError> {
    match object.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(invalid(format!(
            "delegation runtime field {key} must be a string"
        ))),
    }
}

#[cfg(feature = "framework-subagent")]
fn runtime_context_from_json(
    value: Option<serde_json::Value>,
) -> Result<Option<echo_agent::tools::ExternalRunContext>, EchoSdkError> {
    let Some(value) = value else { return Ok(None) };
    if value.is_null() {
        return Ok(None);
    }
    let object = value
        .as_object()
        .ok_or_else(|| invalid("runtime_context must be a record"))?;
    let allowed = [
        "conversation_id",
        "run_id",
        "turn_id",
        "execution_id",
        "isolation_id",
        "message_id",
    ];
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(invalid("runtime_context contains an unknown field"));
    }
    Ok(Some(echo_agent::tools::ExternalRunContext {
        conversation_id: string_field(object, "conversation_id")?,
        run_id: string_field(object, "run_id")?,
        turn_id: string_field(object, "turn_id")?,
        execution_id: string_field(object, "execution_id")?,
        isolation_id: string_field(object, "isolation_id")?,
        message_id: string_field(object, "message_id")?,
        ..Default::default()
    }))
}

#[cfg(feature = "framework-subagent")]
fn string_list(
    value: Option<serde_json::Value>,
    what: &str,
) -> Result<Option<Vec<String>>, EchoSdkError> {
    let Some(value) = value else { return Ok(None) };
    if value.is_null() {
        return Ok(None);
    }
    let list = value
        .as_array()
        .ok_or_else(|| invalid(format!("{what} must be an array of strings")))?;
    list.iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| invalid(format!("{what} must contain only strings")))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

#[cfg(feature = "framework-subagent")]
fn prompt_context_from_json(
    value: Option<serde_json::Value>,
) -> Result<Option<echo_agent::agent::subagent::prompt::SubagentTaskContext>, EchoSdkError> {
    let Some(value) = value else { return Ok(None) };
    if value.is_null() {
        return Ok(None);
    }
    let object = value
        .as_object()
        .ok_or_else(|| invalid("prompt_context must be a record"))?;
    let string_value = |key: &str| -> Result<Option<String>, EchoSdkError> {
        match object.get(key) {
            None | Some(serde_json::Value::Null) => Ok(None),
            Some(serde_json::Value::String(value)) => Ok(Some(value.clone())),
            Some(_) => Err(invalid(format!(
                "prompt_context field {key} must be a string"
            ))),
        }
    };
    let list_value = |key: &str| -> Result<Vec<String>, EchoSdkError> {
        string_list(object.get(key).cloned(), key).map(|value| value.unwrap_or_default())
    };
    Ok(Some(
        echo_agent::agent::subagent::prompt::SubagentTaskContext {
            task_title: string_value("task_title")?,
            user_goal: string_value("user_goal")?,
            workspace: string_value("workspace")?.map(std::path::PathBuf::from),
            files: list_value("files")?,
            execution_checks: list_value("execution_checks")?,
            acceptance_criteria: list_value("acceptance_criteria")?,
            required_artifacts: list_value("required_artifacts")?,
            constraints: list_value("constraints")?,
        },
    ))
}

#[cfg(feature = "framework-subagent")]
fn result_wire(
    result: echo_agent::agent::subagent::SubagentResult,
    operation: &str,
) -> Result<WireValue, EchoSdkError> {
    let outcome = serde_json::to_value(&result.outcome)
        .map_err(|error| framework(operation, error.to_string()))?;
    let mode = serde_json::to_value(&result.mode)
        .map_err(|error| framework(operation, error.to_string()))?;
    let isolation = serde_json::to_value(&result.isolation_observed)
        .map_err(|error| framework(operation, error.to_string()))?;
    let llm_usage = result
        .llm_usage
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|error| framework(operation, error.to_string()))?;
    snapshot_value(
        &FeatureOperationRequest {
            operation: operation.to_string(),
            signature_digest: String::new(),
            handle: None,
            arguments: Vec::new(),
        },
        serde_json::json!({
            "agent_name": result.agent_name,
            "output": result.output,
            "outcome": outcome,
            "duration_ms": u64::try_from(result.duration.as_millis()).unwrap_or(u64::MAX),
            "iterations": result.iterations,
            "tokens_used": result.tokens_used,
            "was_truncated": result.was_truncated,
            "mode": mode,
            "isolation_observed": isolation,
            "llm_usage": llm_usage,
        }),
    )
}

#[cfg(feature = "framework-subagent")]
#[allow(clippy::too_many_arguments)]
async fn dispatch_rich_subagent(
    state: &CoreProfileState,
    request: &FeatureOperationRequest,
    target: String,
    task: String,
    parent_label: Option<String>,
    depth: u32,
    runtime_context: Option<echo_agent::tools::ExternalRunContext>,
    allowed_tools: Option<Vec<String>>,
    prompt_payload: Option<serde_json::Value>,
    prompt_context: Option<echo_agent::agent::subagent::prompt::SubagentTaskContext>,
    message: Option<echo_agent::llm::types::Message>,
    require_active_cancel: bool,
    attempt_identity: Option<echo_agent::agent::subagent::SubagentAttemptIdentity>,
) -> Result<WireValue, EchoSdkError> {
    let agent = session_agent(state, request).await?;
    let session_handle = match request.arguments.first() {
        Some(WireValue::Handle(handle)) if handle.kind == HandleKind::Session => handle,
        _ => return Err(invalid("delegation requires a Session handle")),
    };
    let session_record = state.handles.session(session_handle)?;
    let services = state.services().map_err(|error| {
        framework(
            &request.operation,
            format!("connection services unavailable: {error}"),
        )
    })?;
    let cancel = match services
        .active_run_cancellation(&session_record.acp_session_id)
        .await
    {
        Some(cancel) => cancel,
        None if require_active_cancel => {
            return Err(framework(
                &request.operation,
                "delegation cancellation requires an active Session run",
            ));
        }
        None => tokio_util::sync::CancellationToken::new(),
    };
    let mut runtime_context = runtime_context;
    if let Some(context) = runtime_context.as_mut() {
        context.cancel = Some(std::sync::Arc::new(cancel.clone()));
    }
    let mut parent_context = echo_agent::agent::subagent::context::SubagentContext::from_parent(
        &agent.tool_definitions(),
        &agent.messages(),
        None,
        &echo_agent::agent::subagent::context::ContextInheritance::fresh_default(),
    );
    parent_context.allowed_tools = allowed_tools;
    let dispatch = echo_agent::agent::subagent::executor::DispatchRequest {
        agent_name: target,
        task,
        mode_override: Some(echo_agent::agent::subagent::types::ExecutionMode::Fork),
        cancel,
        parent_agent: parent_label.unwrap_or_else(|| agent.name().to_string()),
        parent_context: Some(parent_context),
        delegation_policy:
            echo_agent::agent::subagent::executor::DispatchRequest::policy_from_depth(depth),
        runtime_context,
        message,
        prompt_payload,
        prompt_context,
        constraints: Vec::new(),
        background: false,
    };
    let executor = state
        .session_factory
        .session_services(&session_record.acp_session_id)
        .ok_or_else(|| framework(&request.operation, "Session subagent authority unavailable"))?
        .subagent_executor
        .clone();
    let result = match attempt_identity {
        Some(identity) => executor.dispatch_attempt(dispatch, identity).await,
        None => executor.dispatch(dispatch).await,
    }
    .map_err(|error| framework(&request.operation, error.to_string()))?;
    result_wire(result, &request.operation)
}

/// Resolve the concrete ACP Session Agent for accessors whose Rust receiver
/// owns live conversation state. The facade Agent handle addresses the
/// prepared definition; the explicit first argument selects the Session that
/// owns the same Agent instance used by Prompt/Run authority.
async fn session_agent(
    state: &CoreProfileState,
    request: &FeatureOperationRequest,
) -> Result<Arc<dyn echo_agent::agent::Agent>, EchoSdkError> {
    let agent = agent_handle(request)?;
    state
        .handles
        .check_shape_and_generation(agent, HandleKind::Agent, METHOD)?;
    let session = match request.arguments.first() {
        Some(WireValue::Handle(handle)) if handle.kind == HandleKind::Session => handle,
        Some(WireValue::Handle(_)) => {
            return Err(invalid("live Agent accessor requires a Session handle"));
        }
        Some(_) => {
            return Err(invalid(
                "live Agent accessor argument 0 must be a Session handle",
            ));
        }
        None => {
            return Err(invalid(
                "live Agent accessor requires a Session handle argument",
            ));
        }
    };
    state
        .handles
        .check_shape_and_generation(session, HandleKind::Session, METHOD)?;
    let session_record = state.handles.session(session)?;
    if session_record.agent_handle_id != agent.id {
        return Err(invalid("Session does not belong to the requested Agent"));
    }
    let services = state.services().map_err(|error| {
        framework(
            &request.operation,
            format!("connection services are unavailable: {error}"),
        )
    })?;
    let session_id =
        agent_client_protocol::schema::v1::SessionId::new(session_record.acp_session_id.clone());
    services
        .sessions()
        .get(&session_id)
        .await
        .map(|session| session.agent.clone())
        .ok_or_else(|| framework(&request.operation, "Session Agent is no longer available"))
}

#[cfg(feature = "framework-eval")]
async fn explicit_agent_argument(
    state: &CoreProfileState,
    request: &FeatureOperationRequest,
    _owner: &str,
    position: usize,
) -> Result<Arc<dyn echo_agent::agent::Agent>, EchoSdkError> {
    let handle = match request.arguments.get(position) {
        Some(WireValue::Handle(handle)) => handle,
        _ => {
            return Err(invalid(format!(
                "argument {position} must be an Agent handle"
            )));
        }
    };
    match handle.kind {
        HandleKind::Session => {
            state.handles.check_shape_and_generation(
                handle,
                HandleKind::Session,
                &request.operation,
            )?;
            let session = state.handles.session(handle)?;
            let services = state
                .services()
                .map_err(|error| framework(&request.operation, error.to_string()))?;
            services
                .sessions()
                .get(&agent_client_protocol::schema::v1::SessionId::new(
                    session.acp_session_id.clone(),
                ))
                .await
                .map(|session| session.agent.clone())
                .ok_or_else(|| invalid("explicit Session Agent is no longer available"))
        }
        HandleKind::Extension => {
            #[cfg(feature = "sdk-extension-bridge")]
            {
                crate::core_profile::extension_bridge::ExtensionCustomAgentProxy::for_registration(
                    state.extension_bridge.clone(),
                    handle.clone(),
                    _owner.to_string(),
                )
                .map(|agent| Arc::new(agent) as Arc<dyn Agent>)
                .ok_or_else(|| invalid("explicit Agent extension is not a CustomAgent"))
            }
            #[cfg(not(feature = "sdk-extension-bridge"))]
            {
                Err(wire::sdk_error(
                    ExtensionErrorCode::FeatureUnavailable,
                    "CustomAgent arguments require the SDK extension bridge",
                    Retryability::Never,
                    METHOD,
                ))
            }
        }
        _ => Err(invalid(
            "explicit Agent must be a Session or CustomAgent extension handle",
        )),
    }
}

#[cfg(all(
    any(feature = "framework-eval", feature = "framework-improve"),
    feature = "sdk-extension-bridge"
))]
fn agent_factory_argument(
    state: &CoreProfileState,
    request: &FeatureOperationRequest,
    owner: &str,
    position: usize,
) -> Result<Arc<crate::core_profile::extension_bridge::SubagentFactoryAdapter>, EchoSdkError> {
    let extension = match request.arguments.get(position) {
        Some(WireValue::Handle(handle)) if handle.kind == HandleKind::Extension => handle.clone(),
        _ => return Err(invalid("Agent factory must be an Extension handle")),
    };
    let name = state
        .handles
        .extension(&extension)?
        .implementation_id
        .clone();
    crate::core_profile::extension_bridge::SubagentFactoryAdapter::for_registration(
        state.extension_bridge.clone(),
        extension,
        name,
        owner.to_string(),
    )
    .map(Arc::new)
    .ok_or_else(|| invalid("extension is not an AgentFactory"))
}

#[cfg(any(feature = "framework-eval", feature = "framework-improve"))]
fn run_store_argument(
    state: &CoreProfileState,
    request: &FeatureOperationRequest,
    owner: &str,
    position: usize,
) -> Result<Option<Arc<dyn echo_agent::trace::RunStore>>, EchoSdkError> {
    let Some(argument) = request.arguments.get(position) else {
        return Err(invalid("RunStore argument is missing"));
    };
    let WireValue::Handle(handle) = argument else {
        return matches!(argument, WireValue::Null)
            .then_some(None)
            .ok_or_else(|| invalid("RunStore must be a trace resource, extension, or null"));
    };
    match handle.kind {
        HandleKind::FacadeResource => {
            let resource =
                source_resource_at(state, request, owner, position, "trace", "trace.store")?;
            state
                .facade
                .trace_stores
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .get(&resource.id)
                .map(|record| Some(record.store.clone()))
                .ok_or_else(|| invalid("trace RunStore resource is closed"))
        }
        HandleKind::Extension => {
            #[cfg(feature = "sdk-extension-bridge")]
            {
                let proxy =
                    crate::core_profile::extension_bridge::ExtensionAgentComponentProxy::new(
                        state.extension_bridge.clone(),
                        handle.clone(),
                        owner.to_string(),
                    )
                    .filter(|proxy| proxy.component() == AgentComponentKindWire::RunStore)
                    .ok_or_else(|| invalid("extension is not a RunStore component"))?;
                Ok(Some(Arc::new(proxy)))
            }
            #[cfg(not(feature = "sdk-extension-bridge"))]
            {
                Err(wire::sdk_error(
                    ExtensionErrorCode::FeatureUnavailable,
                    "RunStore extensions require the SDK extension bridge",
                    Retryability::Never,
                    METHOD,
                ))
            }
        }
        _ => Err(invalid(
            "RunStore must be a trace resource, extension, or null",
        )),
    }
}

async fn session_authorities(
    state: &CoreProfileState,
    request: &FeatureOperationRequest,
) -> Result<std::sync::Arc<crate::factory::SessionAuthorityServices>, EchoSdkError> {
    let _ = session_agent(state, request).await?;
    let session = match request.arguments.first() {
        Some(WireValue::Handle(handle)) if handle.kind == HandleKind::Session => handle,
        _ => return Err(invalid("operation requires a Session handle")),
    };
    let session_record = state.handles.session(session)?;
    state
        .session_factory
        .session_services(&session_record.acp_session_id)
        .ok_or_else(|| framework(&request.operation, "Session authority is unavailable"))
}

async fn source_session_owner(
    state: &CoreProfileState,
    request: &FeatureOperationRequest,
) -> Result<String, EchoSdkError> {
    let _ = session_agent(state, request).await?;
    let session = match request.arguments.first() {
        Some(WireValue::Handle(handle)) if handle.kind == HandleKind::Session => handle,
        _ => return Err(invalid("operation requires a Session handle at argument 0")),
    };
    Ok(state.handles.session(session)?.acp_session_id.clone())
}

#[cfg(feature = "sdk-extension-bridge")]
async fn reject_reentrant_session_mutation(
    state: &CoreProfileState,
    request: &FeatureOperationRequest,
) -> Result<(), EchoSdkError> {
    let owner = source_session_owner(state, request).await?;
    if state.extension_shared.has_active_session_callback(&owner) {
        return Err(wire::sdk_error(
            ExtensionErrorCode::ExtensionConflict,
            "an extension callback cannot mutate its active Session",
            Retryability::Never,
            METHOD,
        )
        .with_operation(request.operation.clone()));
    }
    Ok(())
}

#[cfg(not(feature = "sdk-extension-bridge"))]
async fn reject_reentrant_session_mutation(
    _state: &CoreProfileState,
    _request: &FeatureOperationRequest,
) -> Result<(), EchoSdkError> {
    Ok(())
}

fn is_session_agent_mutation(operation: &str) -> bool {
    matches!(
        operation.rsplit_once("::").map(|(_, method)| method),
        Some(
            "set_plan_mode"
                | "set_permission_mode"
                | "set_conversation_id"
                | "set_model"
                | "set_temperature"
                | "set_max_tokens"
                | "set_thinking"
                | "set_token_limit"
                | "set_disabled_tools"
                | "set_store"
                | "set_max_iterations"
                | "set_compressor"
                | "set_memory_promoter"
                | "set_sandbox_executor"
                | "set_intent_router"
                | "set_skill_load_policy"
                | "reconcile_skill_load_policy"
                | "set_audit_logger"
                | "set_conversation_store"
                | "set_memory_trigger_sink"
                | "set_pre_model_context_projector"
                | "set_run_store"
                | "set_state_store"
                | "set_system_prompt"
                | "replace_system_prompt"
                | "set_working_dir"
                | "clear_working_dir"
                | "reset"
        )
    )
}

fn path_argument(
    request: &FeatureOperationRequest,
    position: usize,
) -> Result<PathBuf, EchoSdkError> {
    let path = match request.arguments.get(position) {
        Some(WireValue::Path(path)) => wire::path_from_wire(path)
            .map_err(|error| invalid(format!("path argument {position} is invalid: {error}")))?,
        _ => return Err(invalid(format!("argument {position} must be a Path"))),
    };
    if !path.is_absolute() {
        return Err(invalid(format!(
            "path argument {position} must be absolute across the SDK process boundary"
        )));
    }
    Ok(path)
}

fn bytes_argument(
    request: &FeatureOperationRequest,
    position: usize,
) -> Result<Vec<u8>, EchoSdkError> {
    let bytes = match request.arguments.get(position) {
        Some(WireValue::Bytes(bytes)) => bytes,
        _ => return Err(invalid(format!("argument {position} must be Bytes"))),
    };
    bytes
        .validate()
        .map_err(|error| invalid(format!("bytes argument {position} is invalid: {error}")))?;
    base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(&bytes.base64)
        .map_err(|error| invalid(format!("bytes argument {position} is invalid: {error}")))
}

fn string_argument(
    request: &FeatureOperationRequest,
    position: usize,
    what: &str,
) -> Result<String, EchoSdkError> {
    match request.arguments.get(position) {
        Some(WireValue::String(value)) => Ok(value.clone()),
        _ => Err(invalid(format!("argument {position} must be a {what}"))),
    }
}

fn u64_argument(request: &FeatureOperationRequest, position: usize) -> Result<u64, EchoSdkError> {
    match request.arguments.get(position) {
        Some(WireValue::U64(value)) => value
            .to_u64()
            .ok_or_else(|| invalid(format!("argument {position} is outside u64"))),
        _ => Err(invalid(format!("argument {position} must be U64"))),
    }
}

fn usize_argument(
    request: &FeatureOperationRequest,
    position: usize,
) -> Result<usize, EchoSdkError> {
    usize::try_from(u64_argument(request, position)?)
        .map_err(|_| invalid(format!("argument {position} is outside usize")))
}

fn durability_argument(
    request: &FeatureOperationRequest,
    position: usize,
) -> Result<echo_agent::utils::fs::FileDurability, EchoSdkError> {
    match request.arguments.get(position) {
        Some(WireValue::String(value)) if value.eq_ignore_ascii_case("flush") => {
            Ok(echo_agent::utils::fs::FileDurability::Flush)
        }
        Some(WireValue::String(value))
            if value.eq_ignore_ascii_case("sync_data")
                || value.eq_ignore_ascii_case("syncdata") =>
        {
            Ok(echo_agent::utils::fs::FileDurability::SyncData)
        }
        _ => Err(invalid(format!(
            "argument {position} must be 'flush' or 'sync_data'"
        ))),
    }
}

fn bytes_value(bytes: Vec<u8>) -> WireValue {
    WireValue::Bytes(WireBytes {
        base64: base64::engine::general_purpose::STANDARD_NO_PAD.encode(bytes),
    })
}

fn file_error(operation: &str, error: std::io::Error) -> EchoSdkError {
    framework(operation, format!("filesystem operation failed: {error}"))
}

fn register_file_authority(
    state: &CoreProfileState,
    owner: &str,
    resource_type: &str,
    authority: FileAuthorityRecord,
    operation: &str,
) -> Result<WireValue, EchoSdkError> {
    let (handle, _) = state.handles.register_facade_resource(
        state.limits.max_facade_resources,
        "source_operation",
        resource_type,
        Some(owner),
        operation,
    )?;
    state
        .facade
        .file_authorities
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(handle.id.clone(), Arc::new(authority));
    Ok(WireValue::Handle(handle))
}

fn file_authority(
    state: &CoreProfileState,
    request: &FeatureOperationRequest,
    owner: &str,
    position: usize,
    resource_type: &str,
) -> Result<Arc<FileAuthorityRecord>, EchoSdkError> {
    let handle = match request.arguments.get(position) {
        Some(WireValue::Handle(handle)) => handle,
        _ => {
            return Err(invalid(format!(
                "argument {position} must be a FacadeResource handle"
            )));
        }
    };
    let resource = super::owned_resource(&state.handles, handle, owner, &request.operation)?;
    if resource.family != "source_operation" || resource.resource_type != resource_type {
        return Err(invalid(format!(
            "argument {position} is not a {resource_type} resource"
        )));
    }
    state
        .facade
        .file_authorities
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&handle.id)
        .cloned()
        .ok_or_else(|| framework(&request.operation, "file authority is no longer available"))
}

fn wire_string_list_argument(
    request: &FeatureOperationRequest,
    position: usize,
    what: &str,
) -> Result<Vec<String>, EchoSdkError> {
    match request.arguments.get(position) {
        Some(WireValue::List(values)) => values
            .iter()
            .map(|value| match value {
                WireValue::String(value) => Ok(value.clone()),
                _ => Err(invalid(format!("{what} must contain only strings"))),
            })
            .collect(),
        _ => Err(invalid(format!(
            "argument {position} must be a {what} list"
        ))),
    }
}

fn skill_content_value(
    request: &FeatureOperationRequest,
    content: echo_agent::skills::external::SkillContent,
) -> Result<WireValue, EchoSdkError> {
    snapshot_value(
        request,
        serde_json::json!({
            "descriptor": content.descriptor,
            "instructions": content.instructions,
            "resources": content.resources,
        }),
    )
}

fn plugin_variables_argument(
    request: &FeatureOperationRequest,
    position: usize,
) -> Result<Option<echo_agent::plugin::PluginVariables>, EchoSdkError> {
    let value = match request.arguments.get(position) {
        None | Some(WireValue::Null) => return Ok(None),
        Some(WireValue::Record { fields, .. }) => fields,
        _ => return Err(invalid("plugin variables must be a Record or null")),
    };
    let field = |name: &str| {
        value
            .iter()
            .find(|field| field.name == name)
            .map(|field| &field.value)
    };
    let path = |name: &str| -> Result<PathBuf, EchoSdkError> {
        match field(name) {
            Some(WireValue::Path(path)) => wire::path_from_wire(path)
                .map_err(|error| invalid(format!("plugin variable {name} is invalid: {error}"))),
            _ => Err(invalid(format!("plugin variable {name} must be a Path"))),
        }
    };
    let mut user_config = HashMap::new();
    if let Some(WireValue::Map(entries)) = field("user_config") {
        for entry in entries {
            let (WireValue::String(key), WireValue::String(value)) = (&entry.key, &entry.value)
            else {
                return Err(invalid(
                    "plugin variable user_config must map strings to strings",
                ));
            };
            user_config.insert(key.clone(), value.clone());
        }
    } else if field("user_config").is_some() {
        return Err(invalid("plugin variable user_config must be a Map"));
    }
    Ok(Some(
        echo_agent::plugin::PluginVariables::new(
            path("plugin_root")?,
            path("plugin_data")?,
            path("project_dir")?,
        )
        .with_user_config(user_config),
    ))
}

fn trigger_context_argument(
    request: &FeatureOperationRequest,
    position: usize,
) -> Result<echo_agent::evolution::TriggerContext, EchoSdkError> {
    let value = json_argument(request, position, "trigger context")?;
    let object = value
        .as_object()
        .ok_or_else(|| invalid("trigger context must be a record"))?;
    let optional_string = |name: &str| -> Result<Option<String>, EchoSdkError> {
        match object.get(name) {
            None | Some(serde_json::Value::Null) => Ok(None),
            Some(serde_json::Value::String(value)) => Ok(Some(value.clone())),
            Some(_) => Err(invalid(format!("trigger context {name} must be a string"))),
        }
    };
    let record_string =
        |record: &serde_json::Map<String, serde_json::Value>, name: &str, kind: &str| {
            record
                .get(name)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| invalid(format!("{kind} requires string field {name}")))
        };
    let failure = match object.get("last_tool_failure") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::Object(record)) => Some(echo_agent::evolution::ToolFailureRecord {
            tool_name: record_string(record, "tool_name", "tool failure")?,
            input_summary: record_string(record, "input_summary", "tool failure")?,
            error: record_string(record, "error", "tool failure")?,
            timestamp: record_string(record, "timestamp", "tool failure")?,
        }),
        Some(_) => return Err(invalid("last_tool_failure must be a record or null")),
    };
    let success = match object.get("last_tool_success") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::Object(record)) => Some(echo_agent::evolution::ToolSuccessRecord {
            tool_name: record_string(record, "tool_name", "tool success")?,
            input_summary: record_string(record, "input_summary", "tool success")?,
            output_summary: record_string(record, "output_summary", "tool success")?,
            timestamp: record_string(record, "timestamp", "tool success")?,
        }),
        Some(_) => return Err(invalid("last_tool_success must be a record or null")),
    };
    let explicit_save = match object.get("explicit_save") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::Object(record)) => {
            Some(echo_agent::evolution::ExplicitSaveRecord {
                content: record_string(record, "content", "explicit save")?,
                topic: match record.get("topic") {
                    None | Some(serde_json::Value::Null) => None,
                    Some(serde_json::Value::String(value)) => Some(value.clone()),
                    Some(_) => return Err(invalid("explicit save topic must be a string")),
                },
            })
        }
        Some(_) => return Err(invalid("explicit_save must be a record or null")),
    };
    let tool_sequences = match object.get("tool_sequences") {
        None => Vec::new(),
        Some(serde_json::Value::Array(records)) => records
            .iter()
            .map(|record| {
                let record = record
                    .as_object()
                    .ok_or_else(|| invalid("tool sequence must contain records"))?;
                Ok(echo_agent::evolution::ToolSequenceRecord {
                    tool_name: record_string(record, "tool_name", "tool sequence")?,
                    session_id: record_string(record, "session_id", "tool sequence")?,
                    timestamp: record_string(record, "timestamp", "tool sequence")?,
                })
            })
            .collect::<Result<Vec<_>, EchoSdkError>>()?,
        Some(_) => return Err(invalid("tool_sequences must be a list")),
    };
    Ok(echo_agent::evolution::TriggerContext {
        user_message: optional_string("user_message")?,
        assistant_message: optional_string("assistant_message")?,
        last_tool_failure: failure,
        last_tool_success: success,
        explicit_save,
        tool_sequences,
    })
}

fn artifact_config_argument(
    request: &FeatureOperationRequest,
    position: usize,
) -> Result<echo_agent::tools::artifact::ToolOutputArtifactConfig, EchoSdkError> {
    let fields = match request.arguments.get(position) {
        Some(WireValue::Record { fields, .. }) => fields,
        _ => return Err(invalid("artifact config must be a Record")),
    };
    let field = |name: &str| {
        fields
            .iter()
            .find(|field| field.name == name)
            .map(|field| &field.value)
    };
    let root_dir = match field("root_dir") {
        Some(WireValue::Path(path)) => wire::path_from_wire(path)
            .map_err(|error| invalid(format!("artifact root_dir is invalid: {error}")))?,
        _ => return Err(invalid("artifact config requires a Path root_dir")),
    };
    let retention = match field("retention") {
        Some(WireValue::String(value)) if !value.trim().is_empty() => value.clone(),
        _ => return Err(invalid("artifact config requires retention")),
    };
    let threshold_bytes = match field("threshold_bytes") {
        Some(WireValue::U64(value)) => value
            .to_u64()
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| invalid("artifact threshold_bytes exceeds usize"))?,
        _ => return Err(invalid("artifact config requires U64 threshold_bytes")),
    };
    let max_age_secs = match field("max_age_secs") {
        None | Some(WireValue::Null) => None,
        Some(WireValue::U64(value)) => Some(
            value
                .to_u64()
                .ok_or_else(|| invalid("artifact max_age_secs exceeds u64"))?,
        ),
        _ => return Err(invalid("artifact max_age_secs must be U64 or null")),
    };
    Ok(
        echo_agent::tools::artifact::ToolOutputArtifactConfig::new(root_dir, retention)
            .threshold_bytes(threshold_bytes)
            .max_age_secs(max_age_secs),
    )
}

fn artifact_identity_argument(
    request: &FeatureOperationRequest,
    position: usize,
) -> Result<echo_agent::tools::artifact::ToolOutputArtifactIdentity, EchoSdkError> {
    let fields = match request.arguments.get(position) {
        Some(WireValue::Record { fields, .. }) => fields,
        _ => return Err(invalid("artifact identity must be a Record")),
    };
    let string = |name: &str, required: bool| -> Result<Option<String>, EchoSdkError> {
        match fields
            .iter()
            .find(|field| field.name == name)
            .map(|field| &field.value)
        {
            None | Some(WireValue::Null) if !required => Ok(None),
            Some(WireValue::String(value)) if !value.trim().is_empty() => Ok(Some(value.clone())),
            _ => Err(invalid(format!(
                "artifact identity field {name} is invalid"
            ))),
        }
    };
    Ok(echo_agent::tools::artifact::ToolOutputArtifactIdentity {
        conversation_id: string("conversation_id", false)?,
        run_id: string("run_id", false)?,
        call_id: string("call_id", true)?.ok_or_else(|| invalid("call_id is required"))?,
        tool_name: string("tool_name", true)?.ok_or_else(|| invalid("tool_name is required"))?,
    })
}

#[cfg(feature = "framework-git")]
fn worktree_config_argument(
    request: &FeatureOperationRequest,
    position: usize,
) -> Result<echo_agent::tools::git_worktree::WorktreeConfig, EchoSdkError> {
    let value = json_argument(request, position, "worktree config")?;
    serde_json::from_value(value)
        .map_err(|error| invalid(format!("worktree config is malformed: {error}")))
}

#[cfg(feature = "framework-git")]
fn managed_worktree_argument(
    request: &FeatureOperationRequest,
    position: usize,
) -> Result<echo_agent::tools::git_worktree::ManagedWorktree, EchoSdkError> {
    let fields = match request.arguments.get(position) {
        Some(WireValue::Record { fields, .. }) => fields,
        _ => return Err(invalid("managed worktree must be a Record")),
    };
    let field = |name: &str| {
        fields
            .iter()
            .find(|field| field.name == name)
            .map(|field| &field.value)
    };
    let path = match field("path") {
        Some(WireValue::Path(path)) => wire::path_from_wire(path)
            .map_err(|error| invalid(format!("managed worktree path is invalid: {error}")))?,
        _ => return Err(invalid("managed worktree requires a Path")),
    };
    let branch = match field("branch") {
        Some(WireValue::String(value)) if !value.trim().is_empty() => value.clone(),
        _ => return Err(invalid("managed worktree requires a branch")),
    };
    let managed = match field("managed") {
        Some(WireValue::Bool(value)) => *value,
        _ => return Err(invalid("managed worktree requires a managed flag")),
    };
    Ok(echo_agent::tools::git_worktree::ManagedWorktree {
        path,
        branch,
        managed,
    })
}

#[cfg(feature = "framework-git")]
fn managed_worktree_value(
    operation: &str,
    worktree: echo_agent::tools::git_worktree::ManagedWorktree,
) -> Result<WireValue, EchoSdkError> {
    let path = wire::path_to_wire(&worktree.path).map_err(|error| framework(operation, error))?;
    Ok(WireValue::Record {
        type_id: "echo_tools::git_worktree::ManagedWorktree".to_string(),
        fields: vec![
            echo_sdk_protocol::scalar::WireField {
                name: "path".to_string(),
                value: WireValue::Path(path),
            },
            echo_sdk_protocol::scalar::WireField {
                name: "branch".to_string(),
                value: WireValue::String(worktree.branch),
            },
            echo_sdk_protocol::scalar::WireField {
                name: "managed".to_string(),
                value: WireValue::Bool(worktree.managed),
            },
        ],
    })
}

#[cfg(feature = "framework-files")]
fn artifact_ref_argument(
    request: &FeatureOperationRequest,
    position: usize,
) -> Result<echo_agent::tools::artifact::ToolOutputArtifactRef, EchoSdkError> {
    let fields = match request.arguments.get(position) {
        Some(WireValue::Record { fields, .. }) => fields,
        _ => return Err(invalid("artifact reference must be a Record")),
    };
    let field = |name: &str| {
        fields
            .iter()
            .find(|field| field.name == name)
            .map(|field| &field.value)
    };
    let u64_field = |name: &str| match field(name) {
        Some(WireValue::U64(value)) => value
            .to_u64()
            .ok_or_else(|| invalid(format!("artifact {name} exceeds u64"))),
        _ => Err(invalid(format!("artifact reference requires U64 {name}"))),
    };
    let string_field = |name: &str| match field(name) {
        Some(WireValue::String(value)) if !value.trim().is_empty() => Ok(value.clone()),
        _ => Err(invalid(format!("artifact reference requires {name}"))),
    };
    let path = match field("path") {
        Some(WireValue::Path(path)) => wire::path_from_wire(path)
            .map_err(|error| invalid(format!("artifact path is invalid: {error}")))?,
        _ => return Err(invalid("artifact reference requires a Path")),
    };
    Ok(echo_agent::tools::artifact::ToolOutputArtifactRef {
        path,
        artifact_bytes: u64_field("artifact_bytes")?,
        payload_bytes: u64_field("payload_bytes")?,
        sha256: string_field("sha256")?,
        retention: string_field("retention")?,
    })
}

#[cfg(feature = "framework-files")]
fn artifact_page_limit_argument(
    request: &FeatureOperationRequest,
    position: usize,
) -> Result<echo_agent::tools::files::artifact::ArtifactPageLimit, EchoSdkError> {
    let (variant, fields) = match request.arguments.get(position) {
        Some(WireValue::Variant {
            variant, fields, ..
        }) => (variant, fields),
        _ => return Err(invalid("artifact page limit must be a Variant")),
    };
    let value = fields
        .iter()
        .find(|field| field.name == "value" || field.name == "limit")
        .and_then(|field| match &field.value {
            WireValue::U64(value) => value.to_u64(),
            _ => None,
        })
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| invalid("artifact page limit requires a U64 value"))?;
    if variant.eq_ignore_ascii_case("bytes") {
        Ok(echo_agent::tools::files::artifact::ArtifactPageLimit::Bytes(value))
    } else if variant.eq_ignore_ascii_case("tokens") {
        Ok(echo_agent::tools::files::artifact::ArtifactPageLimit::Tokens(value))
    } else {
        Err(invalid("artifact page limit must be bytes or tokens"))
    }
}

fn discovery_scopes_from_wire(
    value: &WireValue,
) -> Result<Vec<echo_agent::skills::external::DiscoveryScope>, EchoSdkError> {
    let values = match value {
        WireValue::List(values) => values,
        _ => return Err(invalid("discover_skills requires a list of scopes")),
    };
    values
        .iter()
        .map(|value| {
            let (kind, path) = match value {
                WireValue::Path(path) => (
                    "custom".to_string(),
                    Some(wire::path_from_wire(path).map_err(|error| {
                        invalid(format!("skill discovery path is invalid: {error}"))
                    })?),
                ),
                WireValue::String(scope) => (scope.trim().to_ascii_lowercase(), None),
                WireValue::Record { fields, .. } => {
                    let field = |name: &str| {
                        fields
                            .iter()
                            .find(|field| field.name == name)
                            .map(|field| &field.value)
                    };
                    let kind = field("scope")
                        .or_else(|| field("kind"))
                        .and_then(|value| match value {
                            WireValue::String(value) => Some(value.trim().to_ascii_lowercase()),
                            _ => None,
                        })
                        .unwrap_or_else(|| "custom".to_string());
                    let path = field("path")
                        .or_else(|| field("directory"))
                        .ok_or_else(|| invalid("skill discovery scope requires a path"))?;
                    let path = match path {
                        WireValue::Path(path) => wire::path_from_wire(path).map_err(|error| {
                            invalid(format!("skill discovery path is invalid: {error}"))
                        })?,
                        WireValue::String(path) if !path.trim().is_empty() => {
                            std::path::PathBuf::from(path)
                        }
                        _ => return Err(invalid("skill discovery path must be a Path or String")),
                    };
                    (kind, Some(path))
                }
                WireValue::Variant {
                    variant, fields, ..
                } => {
                    if variant.eq_ignore_ascii_case("user") {
                        return Ok(echo_agent::skills::external::DiscoveryScope::User);
                    }
                    let path = fields
                        .iter()
                        .find(|field| field.name == "path" || field.name == "directory")
                        .map(|field| &field.value)
                        .ok_or_else(|| invalid("skill discovery scope requires a path"))?;
                    let path = match path {
                        WireValue::Path(path) => wire::path_from_wire(path).map_err(|error| {
                            invalid(format!("skill discovery path is invalid: {error}"))
                        })?,
                        WireValue::String(path) if !path.trim().is_empty() => {
                            std::path::PathBuf::from(path)
                        }
                        _ => return Err(invalid("skill discovery path must be a Path or String")),
                    };
                    (variant.trim().to_ascii_lowercase(), Some(path))
                }
                _ => return Err(invalid("skill discovery scope has an unsupported shape")),
            };
            match kind.as_str() {
                "user" => Ok(echo_agent::skills::external::DiscoveryScope::User),
                "project" => Ok(echo_agent::skills::external::DiscoveryScope::Project(
                    path.ok_or_else(|| invalid("project skill discovery requires a path"))?,
                )),
                "custom" => Ok(echo_agent::skills::external::DiscoveryScope::Custom(
                    path.ok_or_else(|| invalid("custom skill discovery requires a path"))?,
                )),
                _ => Err(invalid(format!("unknown skill discovery scope: {kind}"))),
            }
        })
        .collect()
}

#[cfg(feature = "framework-mcp")]
async fn register_mcp_client_resource(
    state: &CoreProfileState,
    session: &WireHandle,
    client: std::sync::Arc<echo_agent::mcp::McpClient>,
    operation: &str,
) -> Result<serde_json::Value, EchoSdkError> {
    let owner = state
        .handles
        .session(session)
        .map_err(|error| error.with_operation(operation))?
        .acp_session_id
        .clone();
    let server_name = client.server_name().to_string();
    let existing_clients = {
        let clients = state
            .facade
            .integrations
            .mcp_clients
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        clients
            .iter()
            .map(|(id, client)| (id.clone(), client.server_name().to_string()))
            .collect::<Vec<_>>()
    };
    let old_ids = existing_clients
        .into_iter()
        .filter(|(id, existing_name)| {
            existing_name == &server_name
                && state
                    .handles
                    .facade_resource(
                        &WireHandle {
                            id: id.clone(),
                            generation: WireU64::from_u64(state.handles.generation()),
                            kind: HandleKind::FacadeResource,
                        },
                        operation,
                    )
                    .ok()
                    .and_then(|record| record.owner_session.clone())
                    .as_deref()
                    == Some(owner.as_str())
        })
        .map(|(id, _)| id)
        .collect::<Vec<_>>();
    for id in old_ids {
        let old_client = state
            .facade
            .integrations
            .mcp_clients
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&id);
        let old_handle = WireHandle {
            id,
            generation: WireU64::from_u64(state.handles.generation()),
            kind: HandleKind::FacadeResource,
        };
        let _ = state.handles.close_facade_resource(&old_handle, operation);
        if let Some(client) = old_client {
            client.close().await;
        }
    }
    let (resource, _) = state.handles.register_facade_resource(
        state.limits.max_facade_resources,
        "mcp",
        "mcp.client",
        Some(&owner),
        operation,
    )?;
    let tools = client
        .tools()
        .iter()
        .map(|tool| tool.name.clone())
        .collect::<Vec<_>>();
    state
        .facade
        .integrations
        .mcp_clients
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(resource.id.clone(), client);
    Ok(serde_json::json!({
        "resource": resource,
        "server_name": server_name,
        "tools": tools,
    }))
}

/// Translate the two direct Agent execution methods into the existing typed
/// RunStart authority. The source-operation wire shape is explicit:
/// `handle` is the issued Agent handle and `arguments` contains a Session
/// handle followed by the text/task input. This preserves the Rust Agent
/// receiver identity while giving the remote call the Session context that a
/// process-local method receives from its owning Agent instance.
pub(crate) fn run_start_request(
    state: &CoreProfileState,
    method: &str,
    params: &serde_json::Value,
) -> Result<Option<RunStartRequest>, EchoSdkError> {
    if method != "_echo_agent/facade/invoke" {
        return Ok(None);
    }
    if !state
        .advertisement
        .declares(ExtensionCapability::FeatureSurfaces)
    {
        return Err(facade_capability_error());
    }
    let request: FeatureOperationRequest = serde_json::from_value(params.clone())
        .map_err(|error| invalid(format!("facade request payload is malformed: {error}")))?;
    enum SourceInputKind {
        Text { execute: bool },
        Message { execute: bool },
    }
    let input_kind = match request.operation.as_str() {
        "echo_core::agent::Agent::chat" | "echo_core::agent::Agent::chat_stream" => {
            SourceInputKind::Text { execute: false }
        }
        "echo_core::agent::Agent::execute" | "echo_core::agent::Agent::execute_stream" => {
            SourceInputKind::Text { execute: true }
        }
        "echo_agent::agent::react::ReactAgent::chat_stream_message" => {
            SourceInputKind::Message { execute: false }
        }
        "echo_agent::agent::react::ReactAgent::execute_stream_message" => {
            SourceInputKind::Message { execute: true }
        }
        _ => return Ok(None),
    };
    request
        .validate()
        .map_err(|reason| invalid(reason.to_string()))?;
    if request.arguments.len() != 2 {
        return Err(invalid(
            "Agent execution accepts exactly [Session handle, input]",
        ));
    }
    let catalog = CompiledOperationCatalog::global()
        .map_err(|reason| framework(&request.operation, reason))?;
    let route = catalog
        .invoke_route(&request.operation)
        .ok_or_else(|| invalid("source operation is not in the canonical catalog"))?;
    if route.family != "source_operation"
        || !route
            .signature_digests
            .iter()
            .any(|digest| digest == &request.signature_digest)
    {
        return Err(invalid(
            "source operation signature does not match the canonical route",
        ));
    }
    let agent = request
        .handle
        .as_ref()
        .ok_or_else(|| invalid("Agent execution requires an Agent handle"))?;
    state.handles.check_shape_and_generation(
        agent,
        HandleKind::Agent,
        "_echo_agent/facade/invoke",
    )?;
    let agent_record = state.handles.agent(agent)?;
    let session = match request.arguments.first() {
        Some(WireValue::Handle(handle)) => handle.clone(),
        Some(_) => {
            return Err(invalid(
                "Agent execution argument 0 must be a Session handle",
            ));
        }
        None => {
            return Err(invalid(
                "Agent execution requires a Session handle argument",
            ));
        }
    };
    state.handles.check_shape_and_generation(
        &session,
        HandleKind::Session,
        "_echo_agent/facade/invoke",
    )?;
    let session_record = state.handles.session(&session)?;
    if session_record.agent_handle_id != agent.id {
        return Err(invalid("Session does not belong to the requested Agent"));
    }
    let input = match input_kind {
        SourceInputKind::Text { execute } => {
            let text = match request.arguments.get(1) {
                Some(WireValue::String(value)) if !value.trim().is_empty() => value.clone(),
                Some(_) => return Err(invalid("Agent input must be a non-empty string")),
                None => return Err(invalid("Agent execution requires text at argument 1")),
            };
            if execute {
                RunInput::Execute { task: text }
            } else {
                RunInput::Chat { text }
            }
        }
        SourceInputKind::Message { execute } => {
            let value = request
                .arguments
                .get(1)
                .cloned()
                .ok_or_else(|| invalid("Agent message stream requires a Message argument"))?
                .into_json()
                .map_err(|error| invalid(format!("Message is not a wire value: {error}")))?;
            let message: LlmMessageWire = serde_json::from_value(value)
                .map_err(|error| invalid(format!("Message payload is malformed: {error}")))?;
            if execute {
                RunInput::ExecuteMessage { message }
            } else {
                RunInput::ChatMessage { message }
            }
        }
    };
    let _ = agent_record;
    Ok(Some(RunStartRequest {
        session,
        input,
        idempotency_id: None,
    }))
}

/// Translate Agent steering methods onto the typed RunSteer authority. The
/// source-operation wire shape is `handle = Agent`, followed by
/// `arguments = [Handle(Run), String(text)]`.
pub(crate) fn run_steer_request(
    state: &CoreProfileState,
    method: &str,
    params: &serde_json::Value,
) -> Result<Option<RunSteerRequest>, EchoSdkError> {
    if method != "_echo_agent/facade/invoke" {
        return Ok(None);
    }
    if !state
        .advertisement
        .declares(ExtensionCapability::FeatureSurfaces)
    {
        return Err(facade_capability_error());
    }
    let request: FeatureOperationRequest = serde_json::from_value(params.clone())
        .map_err(|error| invalid(format!("facade request payload is malformed: {error}")))?;
    match request.operation.as_str() {
        "echo_core::agent::Agent::steer_input"
        | "echo_core::agent::Agent::steer_input_tracked"
        | "echo_agent::agent::react::ReactAgent::steer_input"
        | "echo_agent::agent::react::ReactAgent::steer_input_tracked" => {}
        _ => return Ok(None),
    }
    request
        .validate()
        .map_err(|reason| invalid(reason.to_string()))?;
    if request.arguments.len() != 2 {
        return Err(invalid(
            "Agent steering accepts exactly [Run handle, String]",
        ));
    }
    let catalog = CompiledOperationCatalog::global()
        .map_err(|reason| framework(&request.operation, reason))?;
    let route = catalog
        .invoke_route(&request.operation)
        .ok_or_else(|| invalid("source operation is not in the canonical catalog"))?;
    if route.family != "source_operation"
        || !route
            .signature_digests
            .iter()
            .any(|digest| digest == &request.signature_digest)
    {
        return Err(invalid(
            "source operation signature does not match the canonical route",
        ));
    }
    let agent = request
        .handle
        .as_ref()
        .ok_or_else(|| invalid("Agent steering requires an Agent handle"))?;
    state.handles.check_shape_and_generation(
        agent,
        HandleKind::Agent,
        "_echo_agent/facade/invoke",
    )?;
    let run = match request.arguments.first() {
        Some(WireValue::Handle(handle)) => handle.clone(),
        Some(_) => return Err(invalid("Agent steering argument 0 must be a Run handle")),
        None => return Err(invalid("Agent steering requires a Run handle argument")),
    };
    state
        .handles
        .check_shape_and_generation(&run, HandleKind::Run, "_echo_agent/facade/invoke")?;
    let run_record = state.handles.run(&run)?;
    let run_session_id = match run_record.as_ref() {
        RunRecord::Live { entry } => entry.session_id.to_string(),
        RunRecord::Recovered(recovered) => recovered.session_id.clone(),
    };
    let belongs_to_agent = state
        .handles
        .sessions_for_agent(&agent.id)
        .iter()
        .any(|(_, session_id)| session_id == &run_session_id);
    if !belongs_to_agent {
        return Err(invalid("Run does not belong to the requested Agent"));
    }
    let text = match request.arguments.get(1) {
        Some(WireValue::String(value)) if !value.trim().is_empty() => value.clone(),
        Some(_) => return Err(invalid("steer input must be a non-empty string")),
        None => return Err(invalid("Agent steering requires text at argument 1")),
    };
    Ok(Some(RunSteerRequest { run, text }))
}

pub(crate) fn agent_close_request(
    state: &CoreProfileState,
    method: &str,
    params: &serde_json::Value,
) -> Result<Option<AgentCloseRequest>, EchoSdkError> {
    if method != "_echo_agent/facade/invoke" {
        return Ok(None);
    }
    if !state
        .advertisement
        .declares(ExtensionCapability::FeatureSurfaces)
    {
        return Err(facade_capability_error());
    }
    let request: FeatureOperationRequest = serde_json::from_value(params.clone())
        .map_err(|error| invalid(format!("facade request payload is malformed: {error}")))?;
    if request.operation != "echo_core::agent::Agent::close" {
        return Ok(None);
    }
    request
        .validate()
        .map_err(|reason| invalid(reason.to_string()))?;
    let catalog = CompiledOperationCatalog::global()
        .map_err(|reason| framework(&request.operation, reason))?;
    let route = catalog
        .invoke_route(&request.operation)
        .ok_or_else(|| invalid("source operation is not in the canonical catalog"))?;
    if route.family != "source_operation"
        || !route
            .signature_digests
            .iter()
            .any(|digest| digest == &request.signature_digest)
    {
        return Err(invalid(
            "source operation signature does not match the canonical route",
        ));
    }
    no_arguments(&request)?;
    let agent = request
        .handle
        .as_ref()
        .ok_or_else(|| invalid("Agent close requires an Agent handle"))?;
    state.handles.check_shape_and_generation(
        agent,
        HandleKind::Agent,
        "_echo_agent/facade/invoke",
    )?;
    state.handles.agent(agent)?;
    Ok(Some(AgentCloseRequest {
        agent: agent.clone(),
    }))
}

/// Dispatch one exact source identity. Definition accessors read the
/// Host-issued Agent record; Session-aware accessors resolve the same live
/// ACP Session Agent used by the standard Prompt/Run authority. Operations
/// without a bound framework authority remain explicit framework errors.
pub(crate) async fn dispatch(
    state: &CoreProfileState,
    handles: &HandleRegistry,
    request: &FeatureOperationRequest,
) -> Result<WireValue, EchoSdkError> {
    if is_session_agent_mutation(&request.operation) {
        reject_reentrant_session_mutation(state, request).await?;
    }
    match request.operation.as_str() {
        #[cfg(feature = "framework-content-guard")]
        "echo_core::guard::content::ContentGuard::new" => {
            let owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 2 {
                return Err(invalid("ContentGuard::new accepts [Session, mode]"));
            }
            let mode = content_guard_mode(&string_argument(request, 1, "mode")?)?;
            let resource =
                register_source_resource(state, &owner, "content_guard", "content_guard.instance")?;
            state
                .facade
                .content_guard_authorities
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(resource.id.clone(), ContentGuardAuthorityRecord { mode });
            Ok(WireValue::Handle(resource))
        }
        #[cfg(feature = "framework-content-guard")]
        "echo_core::guard::content::ContentGuard::check"
        | "echo_core::guard::content::ContentGuard::detect"
        | "echo_core::guard::content::ContentGuard::redact"
        | "echo_core::guard::content::ContentGuard::is_clean" => {
            let owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 3 {
                return Err(invalid(
                    "ContentGuard methods accept [Session, ContentGuard, content]",
                ));
            }
            let resource = source_resource_at(
                state,
                request,
                &owner,
                1,
                "content_guard",
                "content_guard.instance",
            )?;
            let mode = state
                .facade
                .content_guard_authorities
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .get(&resource.id)
                .map(|record| record.mode)
                .ok_or_else(|| invalid("ContentGuard receiver is closed"))?;
            let content = string_argument(request, 2, "content")?;
            let guard = echo_agent::guard::content::ContentGuard::new(mode);
            match request
                .operation
                .rsplit_once("::")
                .map(|(_, method)| method)
            {
                Some("check") => guard
                    .check(&content)
                    .map(content_guard_result_value)
                    .map_err(|error| framework(&request.operation, error.to_string())),
                Some("detect") => snapshot_value(
                    request,
                    serde_json::to_value(guard.detect(&content))
                        .map_err(|error| framework(&request.operation, error.to_string()))?,
                ),
                Some("redact") => Ok(WireValue::String(guard.redact(&content))),
                Some("is_clean") => Ok(WireValue::Bool(guard.is_clean(&content))),
                _ => Err(invalid("unknown ContentGuard operation")),
            }
        }
        #[cfg(feature = "framework-project-rules")]
        "echo_core::project_rules::InstructionResolver::new" => {
            let owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 2 {
                return Err(invalid(
                    "InstructionResolver::new accepts [Session, working_dir]",
                ));
            }
            let working_dir = path_argument(request, 1)?;
            let resource =
                register_source_resource(state, &owner, "project_rules", "project_rules.resolver")?;
            state
                .facade
                .instruction_resolver_authorities
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(
                    resource.id.clone(),
                    InstructionResolverAuthorityRecord {
                        working_dir,
                        project_root: None,
                        agents_files_only: false,
                    },
                );
            Ok(WireValue::Handle(resource))
        }
        #[cfg(feature = "framework-project-rules")]
        "echo_core::project_rules::InstructionResolver::project_root"
        | "echo_core::project_rules::InstructionResolver::agents_files_only"
        | "echo_core::project_rules::InstructionResolver::resolve" => {
            let owner = source_session_owner(state, request).await?;
            let expected = if request.operation.ends_with("::project_root") {
                3
            } else {
                2
            };
            if request.arguments.len() != expected {
                return Err(invalid(
                    "InstructionResolver method received the wrong argument count",
                ));
            }
            let resource = source_resource_at(
                state,
                request,
                &owner,
                1,
                "project_rules",
                "project_rules.resolver",
            )?;
            if request.operation.ends_with("::project_root") {
                let root = path_argument(request, 2)?;
                let mut records = state
                    .facade
                    .instruction_resolver_authorities
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                let record = records
                    .get_mut(&resource.id)
                    .ok_or_else(|| invalid("InstructionResolver receiver is closed"))?;
                record.project_root = Some(root);
                return Ok(WireValue::Handle(resource));
            }
            if request.operation.ends_with("::agents_files_only") {
                let mut records = state
                    .facade
                    .instruction_resolver_authorities
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                let record = records
                    .get_mut(&resource.id)
                    .ok_or_else(|| invalid("InstructionResolver receiver is closed"))?;
                record.agents_files_only = true;
                return Ok(WireValue::Handle(resource));
            }
            let record = state
                .facade
                .instruction_resolver_authorities
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .get(&resource.id)
                .cloned()
                .ok_or_else(|| invalid("InstructionResolver receiver is closed"))?;
            let resolver = echo_agent::project_rules::InstructionResolver::new(record.working_dir);
            let resolver = match record.project_root {
                Some(root) => resolver.project_root(root),
                None => resolver,
            };
            let resolver = if record.agents_files_only {
                resolver.agents_files_only()
            } else {
                resolver
            };
            resolved_instructions_value(resolver.resolve())
        }
        #[cfg(feature = "framework-project-rules")]
        "echo_core::project_rules::load_project_rules" => {
            let _owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 2 {
                return Err(invalid("load_project_rules accepts [Session, working_dir]"));
            }
            let working_dir = path_argument(request, 1)?;
            match echo_agent::project_rules::load_project_rules(&working_dir) {
                Some((path, content)) => Ok(WireValue::Variant {
                    type_id: "core::option::Option<(PathBuf,String)>".to_string(),
                    variant: "some".to_string(),
                    fields: vec![
                        echo_sdk_protocol::scalar::WireField {
                            name: "path".to_string(),
                            value: WireValue::Path(wire::path_to_wire(&path).map_err(invalid)?),
                        },
                        echo_sdk_protocol::scalar::WireField {
                            name: "content".to_string(),
                            value: WireValue::String(content),
                        },
                    ],
                }),
                None => Ok(WireValue::Variant {
                    type_id: "core::option::Option<(PathBuf,String)>".to_string(),
                    variant: "none".to_string(),
                    fields: Vec::new(),
                }),
            }
        }
        #[cfg(all(feature = "framework-mcp", feature = "sdk-extension-bridge"))]
        "echo_integration::mcp::client::McpClient::from_transport" => {
            let owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 3 {
                return Err(invalid(
                    "McpClient::from_transport accepts [Session, server_name, transport extension]",
                ));
            }
            let server_name = string_argument(request, 1, "server_name")?;
            let extension = match request.arguments.get(2) {
                Some(WireValue::Handle(handle)) if handle.kind == HandleKind::Extension => {
                    handle.clone()
                }
                _ => return Err(invalid("MCP transport must be an Extension handle")),
            };
            let proxy = crate::core_profile::extension_bridge::ExtensionAgentComponentProxy::new(
                state.extension_bridge.clone(),
                extension,
                owner.clone(),
            )
            .filter(|proxy| proxy.component() == AgentComponentKindWire::McpTransport)
            .ok_or_else(|| invalid("extension is not an McpTransport component"))?;
            let client = echo_agent::mcp::McpClient::from_transport(server_name, Arc::new(proxy))
                .await
                .map_err(|error| framework(&request.operation, error.to_string()))?;
            let handle = match super::integrations::register_mcp_client(
                &state.handles,
                &state.facade.integrations,
                client.clone(),
                &owner,
                state.limits.max_facade_resources,
            ) {
                Ok(handle) => handle,
                Err(error) => {
                    client.close().await;
                    return Err(error);
                }
            };
            Ok(WireValue::Handle(handle))
        }
        #[cfg(feature = "framework-eval")]
        "echo_agent::eval::runner::EvalRunner::new" => {
            let owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 2 {
                return Err(invalid("EvalRunner::new accepts [Session, workspace_root]"));
            }
            let workspace_root = path_argument(request, 1)?;
            let resource = register_source_resource(state, &owner, "eval", "eval.runner")?;
            state
                .facade
                .eval_runners
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(
                    resource.id.clone(),
                    Arc::new(echo_agent::eval::EvalRunner::new(workspace_root)),
                );
            Ok(WireValue::Handle(resource))
        }
        #[cfg(feature = "framework-eval")]
        "echo_agent::eval::runner::EvalRunner::with_run_store"
        | "echo_agent::eval::runner::EvalRunner::with_grader" => {
            let owner = source_session_owner(state, request).await?;
            let resource = source_resource_at(state, request, &owner, 1, "eval", "eval.runner")?;
            let runner = state
                .facade
                .eval_runners
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .get(&resource.id)
                .cloned()
                .ok_or_else(|| invalid("EvalRunner receiver is closed"))?;
            let configured = if request.operation.ends_with("::with_run_store") {
                if request.arguments.len() != 3 {
                    return Err(invalid(
                        "with_run_store accepts [Session, runner, RunStore]",
                    ));
                }
                runner.as_ref().clone().with_run_store(
                    run_store_argument(state, request, &owner, 2)?
                        .ok_or_else(|| invalid("with_run_store does not accept null"))?,
                )
            } else {
                if request.arguments.len() != 4 {
                    return Err(invalid(
                        "with_grader accepts [Session, runner, grader, Agent]",
                    ));
                }
                let grader_resource =
                    source_resource_at(state, request, &owner, 2, "eval", "eval.grader")?;
                let grader = state
                    .facade
                    .llm_graders
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .get(&grader_resource.id)
                    .cloned()
                    .ok_or_else(|| invalid("LlmGrader resource is closed"))?;
                runner.as_ref().clone().with_grader(
                    grader.as_ref().clone(),
                    explicit_agent_argument(state, request, &owner, 3).await?,
                )
            };
            let configured_resource =
                register_source_resource(state, &owner, "eval", "eval.runner")?;
            state
                .facade
                .eval_runners
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(configured_resource.id.clone(), Arc::new(configured));
            Ok(WireValue::Handle(configured_resource))
        }
        #[cfg(feature = "framework-eval")]
        "echo_agent::eval::runner::EvalRunner::timeout_secs"
        | "echo_agent::eval::runner::EvalRunner::workspace_root" => {
            let owner = source_session_owner(state, request).await?;
            let resource = source_resource_at(state, request, &owner, 1, "eval", "eval.runner")?;
            let runner = state
                .facade
                .eval_runners
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .get(&resource.id)
                .cloned()
                .ok_or_else(|| invalid("EvalRunner receiver is closed"))?;
            if request.arguments.len() == 2 {
                return if request.operation.ends_with("::timeout_secs") {
                    Ok(WireValue::U64(WireU64::from_u64(runner.timeout_secs)))
                } else {
                    Ok(WireValue::Path(
                        wire::path_to_wire(&runner.workspace_root).map_err(invalid)?,
                    ))
                };
            }
            if request.arguments.len() != 3 {
                return Err(invalid(
                    "EvalRunner property accepts [Session, runner, value?]",
                ));
            }
            let mut configured = runner.as_ref().clone();
            if request.operation.ends_with("::timeout_secs") {
                configured.timeout_secs = u64_argument(request, 2)?;
            } else {
                configured.workspace_root = path_argument(request, 2)?;
            }
            let configured_resource =
                register_source_resource(state, &owner, "eval", "eval.runner")?;
            state
                .facade
                .eval_runners
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(configured_resource.id.clone(), Arc::new(configured));
            Ok(WireValue::Handle(configured_resource))
        }
        #[cfg(feature = "framework-eval")]
        "echo_agent::eval::runner::EvalRunner::run"
        | "echo_agent::eval::runner::EvalRunner::evaluate_run_constraints" => {
            let owner = source_session_owner(state, request).await?;
            let resource = source_resource_at(state, request, &owner, 1, "eval", "eval.runner")?;
            let runner = state
                .facade
                .eval_runners
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .get(&resource.id)
                .cloned()
                .ok_or_else(|| invalid("EvalRunner receiver is closed"))?;
            if request.operation.ends_with("::run") {
                if request.arguments.len() != 4 {
                    return Err(invalid(
                        "EvalRunner::run accepts [Session, runner, case, Agent]",
                    ));
                }
                let case: echo_agent::eval::EvalCase =
                    serde_json::from_value(json_argument(request, 2, "eval case")?)
                        .map_err(|error| invalid(format!("eval case is malformed: {error}")))?;
                let agent = explicit_agent_argument(state, request, &owner, 3).await?;
                let result = runner.run(&case, agent.as_ref()).await;
                return snapshot_value(
                    request,
                    serde_json::to_value(result)
                        .map_err(|error| framework(&request.operation, error.to_string()))?,
                );
            }
            if request.operation.ends_with("::evaluate_run_constraints") {
                if request.arguments.len() != 4 {
                    return Err(invalid(
                        "evaluate_run_constraints accepts [Session, runner, constraints, run]",
                    ));
                }
                let constraints =
                    serde_json::from_value(json_argument(request, 2, "eval constraints")?)
                        .map_err(|error| {
                            invalid(format!("eval constraints are malformed: {error}"))
                        })?;
                let run = serde_json::from_value(json_argument(request, 3, "run")?)
                    .map_err(|error| invalid(format!("run is malformed: {error}")))?;
                return snapshot_value(
                    request,
                    serde_json::to_value(runner.evaluate_run_constraints(&constraints, &run))
                        .map_err(|error| framework(&request.operation, error.to_string()))?,
                );
            }
            Err(invalid("unknown EvalRunner operation"))
        }
        #[cfg(all(feature = "framework-eval", feature = "sdk-extension-bridge"))]
        "echo_agent::eval::runner::EvalRunner::run_all"
        | "echo_agent::eval::runner::EvalRunner::run_all_async" => {
            let owner = source_session_owner(state, request).await?;
            let resource = source_resource_at(state, request, &owner, 1, "eval", "eval.runner")?;
            if request.arguments.len() != 4 {
                return Err(invalid(
                    "EvalRunner::run_all accepts [Session, runner, cases, AgentFactory]",
                ));
            }
            let runner = state
                .facade
                .eval_runners
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .get(&resource.id)
                .cloned()
                .ok_or_else(|| invalid("EvalRunner receiver is closed"))?;
            let cases: Vec<echo_agent::eval::EvalCase> =
                serde_json::from_value(json_argument(request, 2, "eval cases")?)
                    .map_err(|error| invalid(format!("eval cases are malformed: {error}")))?;
            use echo_agent::agent::subagent::AgentFactory as _;
            let factory = agent_factory_argument(state, request, &owner, 3)?;
            let report = runner
                .run_all_async(&cases, || {
                    let factory = factory.clone();
                    async move {
                        match factory.create().await {
                            Ok(agent) => agent,
                            Err(error) => Box::new(FailedEvalAgent {
                                message: error.to_string(),
                            }) as Box<dyn Agent>,
                        }
                    }
                })
                .await;
            snapshot_value(
                request,
                serde_json::to_value(report)
                    .map_err(|error| framework(&request.operation, error.to_string()))?,
            )
        }
        #[cfg(feature = "framework-eval")]
        "echo_agent::eval::grader::LlmGrader::new" => {
            let owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 1 {
                return Err(invalid("LlmGrader::new accepts [Session]"));
            }
            let resource = register_source_resource(state, &owner, "eval", "eval.grader")?;
            state
                .facade
                .llm_graders
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(
                    resource.id.clone(),
                    Arc::new(echo_agent::eval::LlmGrader::new()),
                );
            Ok(WireValue::Handle(resource))
        }
        #[cfg(feature = "framework-eval")]
        "echo_agent::eval::grader::LlmGrader::grade"
        | "echo_agent::eval::grader::LlmGrader::grade_with_trajectory" => {
            let owner = source_session_owner(state, request).await?;
            let resource = source_resource_at(state, request, &owner, 1, "eval", "eval.grader")?;
            let grader = state
                .facade
                .llm_graders
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .get(&resource.id)
                .cloned()
                .ok_or_else(|| invalid("LlmGrader receiver is closed"))?;
            let task = string_argument(request, 2, "task")?;
            let output = string_argument(request, 3, "output")?;
            let assertions: Vec<echo_agent::eval::grader::Assertion> =
                serde_json::from_value(json_argument(request, 4, "assertions")?)
                    .map_err(|error| invalid(format!("assertions are malformed: {error}")))?;
            let report = if request.operation.ends_with("::grade_with_trajectory") {
                if request.arguments.len() != 7 {
                    return Err(invalid(
                        "grade_with_trajectory accepts [Session, grader, task, output, assertions, trajectory, Agent]",
                    ));
                }
                let trajectory = match request.arguments.get(5) {
                    Some(WireValue::Null) => None,
                    Some(WireValue::String(value)) => Some(value.as_str()),
                    _ => return Err(invalid("trajectory must be String or null")),
                };
                grader
                    .grade_with_trajectory(
                        explicit_agent_argument(state, request, &owner, 6)
                            .await?
                            .as_ref(),
                        &task,
                        &output,
                        &assertions,
                        trajectory,
                    )
                    .await
            } else {
                if request.arguments.len() != 6 {
                    return Err(invalid(
                        "LlmGrader::grade accepts [Session, grader, task, output, assertions, Agent]",
                    ));
                }
                grader
                    .grade(
                        explicit_agent_argument(state, request, &owner, 5)
                            .await?
                            .as_ref(),
                        &task,
                        &output,
                        &assertions,
                    )
                    .await
            };
            snapshot_value(
                request,
                serde_json::to_value(report)
                    .map_err(|error| framework(&request.operation, error.to_string()))?,
            )
        }
        #[cfg(feature = "framework-improve")]
        "echo_agent::improve::loop::ImprovementLoop::new" => {
            let owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 1 {
                return Err(invalid("ImprovementLoop::new accepts [Session]"));
            }
            let resource = register_source_resource(state, &owner, "improve", "improve.loop")?;
            state
                .facade
                .improvement_loops
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(
                    resource.id.clone(),
                    Arc::new(echo_agent::improve::ImprovementLoop::new()),
                );
            Ok(WireValue::Handle(resource))
        }
        #[cfg(feature = "framework-improve")]
        "echo_agent::improve::loop::ImprovementLoop::max_iterations"
        | "echo_agent::improve::loop::ImprovementLoop::improvement_threshold"
        | "echo_agent::improve::loop::ImprovementLoop::holdout_ratio" => {
            let owner = source_session_owner(state, request).await?;
            let resource =
                source_resource_at(state, request, &owner, 1, "improve", "improve.loop")?;
            let loop_authority = state
                .facade
                .improvement_loops
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .get(&resource.id)
                .cloned()
                .ok_or_else(|| invalid("ImprovementLoop receiver is closed"))?;
            if request.arguments.len() == 2 {
                return match request
                    .operation
                    .rsplit_once("::")
                    .map(|(_, property)| property)
                {
                    Some("max_iterations") => Ok(WireValue::U64(WireU64::from_u64(
                        u64::try_from(loop_authority.max_iterations).unwrap_or(u64::MAX),
                    ))),
                    Some("improvement_threshold") => {
                        Ok(WireValue::F64(loop_authority.improvement_threshold))
                    }
                    _ => Ok(WireValue::F64(loop_authority.holdout_ratio)),
                };
            }
            if request.arguments.len() != 3 {
                return Err(invalid(
                    "ImprovementLoop property accepts [Session, loop, value?]",
                ));
            }
            let mut configured = loop_authority.as_ref().clone();
            match request
                .operation
                .rsplit_once("::")
                .map(|(_, property)| property)
            {
                Some("max_iterations") => configured.max_iterations = usize_argument(request, 2)?,
                Some("improvement_threshold") => {
                    let WireValue::F64(value) = request
                        .arguments
                        .get(2)
                        .ok_or_else(|| invalid("improvement_threshold is required"))?
                    else {
                        return Err(invalid("improvement_threshold must be F64"));
                    };
                    if !value.is_finite() {
                        return Err(invalid("improvement_threshold must be finite"));
                    }
                    configured.improvement_threshold = *value;
                }
                _ => {
                    let WireValue::F64(value) = request
                        .arguments
                        .get(2)
                        .ok_or_else(|| invalid("holdout_ratio is required"))?
                    else {
                        return Err(invalid("holdout_ratio must be F64"));
                    };
                    if !value.is_finite() {
                        return Err(invalid("holdout_ratio must be finite"));
                    }
                    configured.holdout_ratio = *value;
                }
            }
            let configured_resource =
                register_source_resource(state, &owner, "improve", "improve.loop")?;
            state
                .facade
                .improvement_loops
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(configured_resource.id.clone(), Arc::new(configured));
            Ok(WireValue::Handle(configured_resource))
        }
        #[cfg(all(feature = "framework-improve", feature = "sdk-extension-bridge"))]
        "echo_agent::improve::loop::ImprovementLoop::run"
        | "echo_agent::improve::loop::ImprovementLoop::run_async" => {
            let owner = source_session_owner(state, request).await?;
            let resource =
                source_resource_at(state, request, &owner, 1, "improve", "improve.loop")?;
            if request.arguments.len() != 5 {
                return Err(invalid(
                    "ImprovementLoop::run accepts [Session, loop, cases, AgentFactory, RunStore|null]",
                ));
            }
            let loop_authority = state
                .facade
                .improvement_loops
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .get(&resource.id)
                .cloned()
                .ok_or_else(|| invalid("ImprovementLoop receiver is closed"))?;
            let cases = serde_json::from_value::<Vec<echo_agent::eval::EvalCase>>(json_argument(
                request, 2, "cases",
            )?)
            .map_err(|error| invalid(format!("eval cases are malformed: {error}")))?;
            let run_store = run_store_argument(state, request, &owner, 4)?;
            {
                use echo_agent::agent::subagent::AgentFactory as _;
                let factory = agent_factory_argument(state, request, &owner, 3)?;
                let result = loop_authority
                    .run_async(
                        &cases,
                        || {
                            let factory = factory.clone();
                            async move {
                                match factory.create().await {
                                    Ok(agent) => agent,
                                    Err(error) => Box::new(FailedEvalAgent {
                                        message: error.to_string(),
                                    })
                                        as Box<dyn Agent>,
                                }
                            }
                        },
                        &run_store,
                    )
                    .await;
                snapshot_value(
                    request,
                    serde_json::to_value(result)
                        .map_err(|error| framework(&request.operation, error.to_string()))?,
                )
            }
        }
        #[cfg(feature = "framework-improve")]
        "echo_agent::improve::trajectory::TrajectorySaver::convert_run_to_sharegpt" => {
            let _owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 2 {
                return Err(invalid(
                    "TrajectorySaver::convert_run_to_sharegpt accepts [Session, run]",
                ));
            }
            let run = serde_json::from_value(json_argument(request, 1, "run")?)
                .map_err(|error| invalid(format!("run is malformed: {error}")))?;
            snapshot_value(
                request,
                serde_json::to_value(
                    echo_agent::improve::TrajectorySaver::convert_run_to_sharegpt(&run),
                )
                .map_err(|error| framework(&request.operation, error.to_string()))?,
            )
        }
        #[cfg(feature = "framework-improve")]
        "echo_agent::improve::trajectory::TrajectorySaver::new" => {
            let owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 2 {
                return Err(invalid("TrajectorySaver::new accepts [Session, base_dir]"));
            }
            let base_dir = path_argument(request, 1)?;
            let saver = echo_agent::improve::TrajectorySaver::new(base_dir)
                .map_err(|error| framework(&request.operation, error.to_string()))?;
            let resource =
                register_source_resource(state, &owner, "improve", "improve.trajectory_saver")?;
            state
                .facade
                .trajectory_savers
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(resource.id.clone(), Arc::new(saver));
            Ok(WireValue::Handle(resource))
        }
        #[cfg(feature = "framework-improve")]
        "echo_agent::improve::trajectory::TrajectorySaver::save"
        | "echo_agent::improve::trajectory::TrajectorySaver::list"
        | "echo_agent::improve::trajectory::TrajectorySaver::stats" => {
            let owner = source_session_owner(state, request).await?;
            let resource = source_resource_at(
                state,
                request,
                &owner,
                1,
                "improve",
                "improve.trajectory_saver",
            )?;
            let saver = state
                .facade
                .trajectory_savers
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .get(&resource.id)
                .cloned()
                .ok_or_else(|| invalid("TrajectorySaver receiver is closed"))?;
            let value = if request.operation.ends_with("::save") {
                if request.arguments.len() != 4 {
                    return Err(invalid(
                        "TrajectorySaver::save accepts [Session, saver, run, model]",
                    ));
                }
                let run = serde_json::from_value(json_argument(request, 2, "run")?)
                    .map_err(|error| invalid(format!("run is malformed: {error}")))?;
                let model = string_argument(request, 3, "model")?;
                serde_json::to_value(
                    saver
                        .save(&run, &model)
                        .await
                        .map_err(|error| framework(&request.operation, error.to_string()))?,
                )
            } else if request.operation.ends_with("::list") {
                if request.arguments.len() != 3 {
                    return Err(invalid(
                        "TrajectorySaver::list accepts [Session, saver, date_prefix|null]",
                    ));
                }
                let prefix = match request.arguments.get(2) {
                    Some(WireValue::Null) => None,
                    Some(WireValue::String(value)) => Some(value.as_str()),
                    _ => return Err(invalid("date_prefix must be String or null")),
                };
                serde_json::to_value(
                    saver
                        .list(prefix)
                        .await
                        .map_err(|error| framework(&request.operation, error.to_string()))?,
                )
            } else {
                if request.arguments.len() != 2 {
                    return Err(invalid("TrajectorySaver::stats accepts [Session, saver]"));
                }
                serde_json::to_value(
                    saver
                        .stats()
                        .await
                        .map_err(|error| framework(&request.operation, error.to_string()))?,
                )
            }
            .map_err(|error| framework(&request.operation, error.to_string()))?;
            snapshot_value(request, value)
        }
        "echo_core::plugin::registry::PluginRegistry::new" => {
            let owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 3 {
                return Err(invalid(
                    "PluginRegistry::new accepts [Session, data_root, project_root|null]",
                ));
            }
            let data_root = path_argument(request, 1)?;
            let project_root = match request.arguments.get(2) {
                Some(WireValue::Null) => None,
                Some(WireValue::Path(path)) => Some(wire::path_from_wire(path).map_err(invalid)?),
                _ => return Err(invalid("project_root must be Path or null")),
            };
            let resource = register_source_resource(state, &owner, "plugin", "plugin.registry")?;
            state
                .facade
                .plugin_registries
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(
                    resource.id.clone(),
                    Arc::new(tokio::sync::Mutex::new(
                        echo_agent::plugin::PluginRegistry::new(data_root, project_root),
                    )),
                );
            Ok(WireValue::Handle(resource))
        }
        "echo_core::plugin::registry::PluginRegistry::with_paths" => {
            let owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 4 {
                return Err(invalid(
                    "PluginRegistry::with_paths accepts [Session, state_file, data_dir, project_root|null]",
                ));
            }
            let project_root = match request.arguments.get(3) {
                Some(WireValue::Null) => None,
                Some(WireValue::Path(path)) => Some(wire::path_from_wire(path).map_err(invalid)?),
                _ => return Err(invalid("project_root must be Path or null")),
            };
            let registry = echo_agent::plugin::PluginRegistry::with_paths(
                path_argument(request, 1)?,
                path_argument(request, 2)?,
                project_root,
            );
            let resource = register_source_resource(state, &owner, "plugin", "plugin.registry")?;
            state
                .facade
                .plugin_registries
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(
                    resource.id.clone(),
                    Arc::new(tokio::sync::Mutex::new(registry)),
                );
            Ok(WireValue::Handle(resource))
        }
        "echo_core::plugin::registry::PluginRegistry::validate_plugin_dir" => {
            let _owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 2 {
                return Err(invalid(
                    "PluginRegistry::validate_plugin_dir accepts [Session, root]",
                ));
            }
            let (manifest, resolved) = echo_agent::plugin::PluginRegistry::validate_plugin_dir(
                &path_argument(request, 1)?,
            )
            .map_err(|errors| framework(&request.operation, errors.join("; ")))?;
            Ok(WireValue::List(vec![
                snapshot_value(
                    request,
                    serde_json::to_value(manifest)
                        .map_err(|error| framework(&request.operation, error.to_string()))?,
                )?,
                resolved_plugin_components_value(resolved)?,
            ]))
        }
        "echo_core::plugin::registry::PluginRegistry::scan_all"
        | "echo_core::plugin::registry::PluginRegistry::scan_scopes"
        | "echo_core::plugin::registry::PluginRegistry::install"
        | "echo_core::plugin::registry::PluginRegistry::uninstall"
        | "echo_core::plugin::registry::PluginRegistry::enable"
        | "echo_core::plugin::registry::PluginRegistry::disable"
        | "echo_core::plugin::registry::PluginRegistry::get"
        | "echo_core::plugin::registry::PluginRegistry::configure"
        | "echo_core::plugin::registry::PluginRegistry::variables_for"
        | "echo_core::plugin::registry::PluginRegistry::list"
        | "echo_core::plugin::registry::PluginRegistry::list_enabled"
        | "echo_core::plugin::registry::PluginRegistry::search"
        | "echo_core::plugin::registry::PluginRegistry::count"
        | "echo_core::plugin::registry::PluginRegistry::revision"
        | "echo_core::plugin::registry::PluginRegistry::scan_diagnostics"
        | "echo_core::plugin::registry::PluginRegistry::resolve_components"
        | "echo_core::plugin::registry::PluginRegistry::resolve_components_async" => {
            let owner = source_session_owner(state, request).await?;
            let resource =
                source_resource_at(state, request, &owner, 1, "plugin", "plugin.registry")?;
            let registry = state
                .facade
                .plugin_registries
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .get(&resource.id)
                .cloned()
                .ok_or_else(|| invalid("PluginRegistry receiver is closed"))?;
            let mut registry = registry.lock().await;
            match request
                .operation
                .rsplit_once("::")
                .map(|(_, method)| method)
            {
                Some("scan_all") => {
                    if request.arguments.len() != 2 {
                        return Err(invalid(
                            "PluginRegistry::scan_all accepts [Session, registry]",
                        ));
                    }
                    let count = registry
                        .scan_all()
                        .map_err(|error| framework(&request.operation, error.to_string()))?;
                    Ok(WireValue::U64(WireU64::from_u64(
                        u64::try_from(count).unwrap_or(u64::MAX),
                    )))
                }
                Some("scan_scopes") => {
                    if request.arguments.len() != 3 {
                        return Err(invalid(
                            "PluginRegistry::scan_scopes accepts [Session, registry, scopes]",
                        ));
                    }
                    let scopes = serde_json::from_value::<Vec<echo_agent::plugin::PluginScope>>(
                        json_argument(request, 2, "scopes")?,
                    )
                    .map_err(|error| invalid(format!("plugin scopes are invalid: {error}")))?;
                    let count = registry
                        .scan_scopes(&scopes)
                        .map_err(|error| framework(&request.operation, error.to_string()))?;
                    Ok(WireValue::U64(WireU64::from_u64(
                        u64::try_from(count).unwrap_or(u64::MAX),
                    )))
                }
                Some("install") => {
                    if request.arguments.len() != 4 {
                        return Err(invalid(
                            "PluginRegistry::install accepts [Session, registry, source, scope]",
                        ));
                    }
                    let source = plugin_install_source(
                        request
                            .arguments
                            .get(2)
                            .ok_or_else(|| invalid("plugin source is required"))?,
                    )?;
                    let scope: echo_agent::plugin::PluginScope = serde_json::from_value(
                        serde_json::Value::String(string_argument(request, 3, "scope")?),
                    )
                    .map_err(|error| invalid(format!("plugin scope is invalid: {error}")))?;
                    registry
                        .install(&source, scope)
                        .map(WireValue::String)
                        .map_err(|error| framework(&request.operation, error))
                }
                Some("enable") => {
                    if request.arguments.len() != 3 {
                        return Err(invalid(
                            "PluginRegistry::enable accepts [Session, registry, plugin_id]",
                        ));
                    }
                    let plugin_id = string_argument(request, 2, "plugin_id")?;
                    registry
                        .enable(&plugin_id)
                        .map(|_| WireValue::Null)
                        .map_err(|error| framework(&request.operation, error))
                }
                Some("uninstall") => {
                    if request.arguments.len() != 4 {
                        return Err(invalid(
                            "PluginRegistry::uninstall accepts [Session, registry, plugin_id, keep_data]",
                        ));
                    }
                    let plugin_id = string_argument(request, 2, "plugin_id")?;
                    let keep_data = match request.arguments.get(3) {
                        Some(WireValue::Bool(value)) => *value,
                        _ => return Err(invalid("keep_data must be Bool")),
                    };
                    registry
                        .uninstall(&plugin_id, keep_data)
                        .map(|_| WireValue::Null)
                        .map_err(|error| framework(&request.operation, error))
                }
                Some("disable") => {
                    if request.arguments.len() != 3 {
                        return Err(invalid(
                            "PluginRegistry::disable accepts [Session, registry, plugin_id]",
                        ));
                    }
                    let plugin_id = string_argument(request, 2, "plugin_id")?;
                    registry
                        .disable(&plugin_id)
                        .map(|_| WireValue::Null)
                        .map_err(|error| framework(&request.operation, error))
                }
                Some("get") => {
                    if request.arguments.len() != 3 {
                        return Err(invalid(
                            "PluginRegistry::get accepts [Session, registry, plugin_id]",
                        ));
                    }
                    let plugin_id = string_argument(request, 2, "plugin_id")?;
                    snapshot_value(
                        request,
                        serde_json::to_value(registry.get(&plugin_id))
                            .map_err(|error| framework(&request.operation, error.to_string()))?,
                    )
                }
                Some("configure") => {
                    if request.arguments.len() != 4 {
                        return Err(invalid(
                            "PluginRegistry::configure accepts [Session, registry, plugin_id, values]",
                        ));
                    }
                    let plugin_id = string_argument(request, 2, "plugin_id")?;
                    let values = serde_json::from_value(json_argument(request, 3, "values")?)
                        .map_err(|error| invalid(format!("plugin values are invalid: {error}")))?;
                    registry
                        .configure(&plugin_id, values)
                        .map(|_| WireValue::Null)
                        .map_err(|error| framework(&request.operation, error))
                }
                Some("variables_for") => {
                    if request.arguments.len() != 3 {
                        return Err(invalid(
                            "PluginRegistry::variables_for accepts [Session, registry, plugin_id]",
                        ));
                    }
                    let plugin_id = string_argument(request, 2, "plugin_id")?;
                    let variables = registry
                        .variables_for(&plugin_id)
                        .map_err(|error| framework(&request.operation, error))?;
                    let value = serde_json::json!({
                        "plugin_root": wire::path_to_wire(&variables.plugin_root).map_err(invalid)?,
                        "plugin_data": wire::path_to_wire(&variables.plugin_data).map_err(invalid)?,
                        "project_dir": wire::path_to_wire(&variables.project_dir).map_err(invalid)?,
                        "user_config": variables.user_config,
                    });
                    snapshot_value(request, value)
                }
                Some("list") | Some("list_enabled") | Some("search") => {
                    let entries = match request
                        .operation
                        .rsplit_once("::")
                        .map(|(_, method)| method)
                    {
                        Some("list") => {
                            if request.arguments.len() != 2 {
                                return Err(invalid(
                                    "PluginRegistry::list accepts [Session, registry]",
                                ));
                            }
                            registry.list()
                        }
                        Some("list_enabled") => {
                            if request.arguments.len() != 2 {
                                return Err(invalid(
                                    "PluginRegistry::list_enabled accepts [Session, registry]",
                                ));
                            }
                            registry.list_enabled()
                        }
                        _ => {
                            if request.arguments.len() != 3 {
                                return Err(invalid(
                                    "PluginRegistry::search accepts [Session, registry, query]",
                                ));
                            }
                            registry.search(&string_argument(request, 2, "query")?)
                        }
                    };
                    snapshot_value(
                        request,
                        serde_json::to_value(entries)
                            .map_err(|error| framework(&request.operation, error.to_string()))?,
                    )
                }
                Some("count") => {
                    if request.arguments.len() != 2 {
                        return Err(invalid("PluginRegistry::count accepts [Session, registry]"));
                    }
                    Ok(WireValue::U64(WireU64::from_u64(
                        u64::try_from(registry.count()).unwrap_or(u64::MAX),
                    )))
                }
                Some("revision") => {
                    if request.arguments.len() != 2 {
                        return Err(invalid(
                            "PluginRegistry::revision accepts [Session, registry]",
                        ));
                    }
                    Ok(WireValue::U64(WireU64::from_u64(registry.revision())))
                }
                Some("scan_diagnostics") => {
                    if request.arguments.len() != 2 {
                        return Err(invalid(
                            "PluginRegistry::scan_diagnostics accepts [Session, registry]",
                        ));
                    }
                    let diagnostics = registry
                        .scan_diagnostics()
                        .iter()
                        .map(|diagnostic| {
                            Ok(serde_json::json!({
                                "path": wire::path_to_wire(&diagnostic.path).map_err(invalid)?,
                                "message": diagnostic.message,
                                "is_error": diagnostic.is_error,
                            }))
                        })
                        .collect::<Result<Vec<_>, EchoSdkError>>()?;
                    snapshot_value(request, serde_json::Value::Array(diagnostics))
                }
                Some("resolve_components") => {
                    if request.arguments.len() != 3 {
                        return Err(invalid(
                            "resolve_components accepts [Session, registry, plugin_id]",
                        ));
                    }
                    let plugin_id = string_argument(request, 2, "plugin_id")?;
                    let resolved = registry
                        .resolve_components(&plugin_id)
                        .map_err(|error| framework(&request.operation, error))?;
                    resolved_plugin_components_value(resolved)
                }
                Some("resolve_components_async") => {
                    if request.arguments.len() != 3 {
                        return Err(invalid(
                            "resolve_components_async accepts [Session, registry, plugin_id]",
                        ));
                    }
                    let plugin_id = string_argument(request, 2, "plugin_id")?;
                    let resolved = registry
                        .resolve_components_async(&plugin_id)
                        .await
                        .map_err(|error| framework(&request.operation, error))?;
                    resolved_plugin_components_value(resolved)
                }
                _ => Err(invalid("unknown PluginRegistry operation")),
            }
        }
        "echo_core::plugin::registry::PluginRegistry::resolve_dependencies"
        | "echo_core::plugin::registry::PluginRegistry::resolve_enabled_dependencies"
        | "echo_core::plugin::registry::PluginRegistry::data_dir_for" => {
            let owner = source_session_owner(state, request).await?;
            let resource =
                source_resource_at(state, request, &owner, 1, "plugin", "plugin.registry")?;
            let registry = state
                .facade
                .plugin_registries
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .get(&resource.id)
                .cloned()
                .ok_or_else(|| invalid("PluginRegistry receiver is closed"))?;
            let registry = registry.lock().await;
            match request
                .operation
                .rsplit_once("::")
                .map(|(_, method)| method)
            {
                Some("resolve_dependencies") => {
                    if request.arguments.len() != 2 {
                        return Err(invalid("resolve_dependencies accepts [Session, registry]"));
                    }
                    let ids = registry
                        .resolve_dependencies()
                        .map_err(|error| framework(&request.operation, error))?;
                    Ok(WireValue::List(
                        ids.into_iter().map(WireValue::String).collect(),
                    ))
                }
                Some("resolve_enabled_dependencies") => {
                    if request.arguments.len() != 2 {
                        return Err(invalid(
                            "resolve_enabled_dependencies accepts [Session, registry]",
                        ));
                    }
                    let ids = registry
                        .resolve_enabled_dependencies()
                        .map_err(|error| framework(&request.operation, error))?;
                    Ok(WireValue::List(
                        ids.into_iter().map(WireValue::String).collect(),
                    ))
                }
                Some("data_dir_for") => {
                    if request.arguments.len() != 3 {
                        return Err(invalid(
                            "data_dir_for accepts [Session, registry, plugin_id]",
                        ));
                    }
                    let path = registry.data_dir_for(&string_argument(request, 2, "plugin_id")?);
                    Ok(WireValue::Path(wire::path_to_wire(&path).map_err(invalid)?))
                }
                _ => Err(invalid("unknown PluginRegistry query operation")),
            }
        }
        "echo_state::memory::store::InMemoryStore::new"
        | "echo_state::memory::store::FileStore::new"
        | "echo_state::memory::sqlite_store::SqliteStore::new" => {
            let owner = source_session_owner(state, request).await?;
            let store = match request.operation.as_str() {
                "echo_state::memory::store::InMemoryStore::new" => {
                    if request.arguments.len() != 1 {
                        return Err(invalid("InMemoryStore::new accepts [Session]"));
                    }
                    MemoryStoreAuthority::InMemory(Arc::new(
                        echo_agent::memory::InMemoryStore::new(),
                    ))
                }
                "echo_state::memory::store::FileStore::new" => {
                    if request.arguments.len() != 2 {
                        return Err(invalid("FileStore::new accepts [Session, path]"));
                    }
                    MemoryStoreAuthority::File(Arc::new(
                        echo_agent::memory::FileStore::new(path_argument(request, 1)?)
                            .map_err(|error| framework(&request.operation, error.to_string()))?,
                    ))
                }
                _ => {
                    #[cfg(feature = "framework-sqlite")]
                    {
                        if request.arguments.len() != 2 {
                            return Err(invalid("SqliteStore::new accepts [Session, path]"));
                        }
                        MemoryStoreAuthority::Sqlite(Arc::new(
                            echo_agent::memory::SqliteStore::new(path_argument(request, 1)?)
                                .map_err(|error| {
                                    framework(&request.operation, error.to_string())
                                })?,
                        ))
                    }
                    #[cfg(not(feature = "framework-sqlite"))]
                    {
                        return Err(wire::sdk_error(
                            ExtensionErrorCode::FeatureUnavailable,
                            "SqliteStore requires the sqlite feature",
                            Retryability::Never,
                            METHOD,
                        )
                        .with_operation(request.operation.clone()));
                    }
                }
            };
            let resource = register_source_resource(state, &owner, "memory", "memory.store")?;
            state
                .facade
                .memory_store_resources
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(resource.id.clone(), Arc::new(store));
            Ok(WireValue::Handle(resource))
        }
        "echo_state::memory::store::InMemoryStore::put_raw" => {
            let owner = source_session_owner(state, request).await?;
            let resource = source_resource_at(state, request, &owner, 1, "memory", "memory.store")?;
            if request.arguments.len() != 3 {
                return Err(invalid(
                    "InMemoryStore::put_raw accepts [Session, store, item]",
                ));
            }
            let item = serde_json::from_value::<echo_agent::memory::StoreItem>(json_argument(
                request, 2, "item",
            )?)
            .map_err(|error| invalid(format!("memory item is malformed: {error}")))?;
            let store = state
                .facade
                .memory_store_resources
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .get(&resource.id)
                .cloned()
                .ok_or_else(|| invalid("memory Store resource is closed"))?;
            let MemoryStoreAuthority::InMemory(store) = store.as_ref() else {
                return Err(invalid("put_raw requires an InMemoryStore resource"));
            };
            store.put_raw(item).await;
            Ok(WireValue::Null)
        }
        "echo_state::memory::store::FileStore::put_batch"
        | "echo_state::memory::store::FileStore::flush_public" => {
            let owner = source_session_owner(state, request).await?;
            let resource = source_resource_at(state, request, &owner, 1, "memory", "memory.store")?;
            let store = state
                .facade
                .memory_store_resources
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .get(&resource.id)
                .cloned()
                .ok_or_else(|| invalid("memory Store resource is closed"))?;
            let MemoryStoreAuthority::File(store) = store.as_ref() else {
                return Err(invalid("FileStore operation requires a FileStore resource"));
            };
            if request.operation.ends_with("::flush_public") {
                if request.arguments.len() != 2 {
                    return Err(invalid("FileStore::flush_public accepts [Session, store]"));
                }
                store
                    .flush_public()
                    .await
                    .map_err(|error| framework(&request.operation, error.to_string()))?;
            } else {
                if request.arguments.len() != 3 {
                    return Err(invalid(
                        "FileStore::put_batch accepts [Session, store, entries]",
                    ));
                }
                #[derive(serde::Deserialize)]
                struct BatchEntry {
                    namespace: Vec<String>,
                    key: String,
                    value: serde_json::Value,
                }
                let entries = serde_json::from_value::<Vec<BatchEntry>>(json_argument(
                    request, 2, "entries",
                )?)
                .map_err(|error| invalid(format!("memory batch is malformed: {error}")))?;
                store
                    .put_batch(entries.iter().map(|entry| {
                        (
                            entry.namespace.iter().map(String::as_str).collect(),
                            entry.key.as_str(),
                            entry.value.clone(),
                        )
                    }))
                    .await
                    .map_err(|error| framework(&request.operation, error.to_string()))?;
            }
            Ok(WireValue::Null)
        }
        #[cfg(all(feature = "framework-sqlite", feature = "sdk-extension-bridge"))]
        "echo_state::memory::sqlite_store::SqliteStore::with_embedder" => {
            let owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 3 {
                return Err(invalid(
                    "SqliteStore::with_embedder accepts [Session, path, Embedder extension]",
                ));
            }
            let extension = match request.arguments.get(2) {
                Some(WireValue::Handle(handle)) if handle.kind == HandleKind::Extension => {
                    handle.clone()
                }
                _ => return Err(invalid("with_embedder requires an Extension handle")),
            };
            let proxy = crate::core_profile::extension_bridge::ExtensionAgentComponentProxy::new(
                state.extension_bridge.clone(),
                extension,
                owner.clone(),
            )
            .filter(|proxy| proxy.component() == AgentComponentKindWire::Embedder)
            .ok_or_else(|| invalid("with_embedder requires an Embedder extension"))?;
            let store = MemoryStoreAuthority::Sqlite(Arc::new(
                echo_agent::memory::SqliteStore::with_embedder(
                    path_argument(request, 1)?,
                    Arc::new(proxy),
                )
                .map_err(|error| framework(&request.operation, error.to_string()))?,
            ));
            let resource = register_source_resource(state, &owner, "memory", "memory.store")?;
            state
                .facade
                .memory_store_resources
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(resource.id.clone(), Arc::new(store));
            Ok(WireValue::Handle(resource))
        }
        "echo_execution::skills::external::prompt_exec::PromptContext" => {
            let owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 8 {
                return Err(invalid(
                    "PromptContext accepts [Session, skill_dir, session_id, arguments, shell|null, timeout, source, sandbox|null]",
                ));
            }
            let arguments = match request.arguments.get(3) {
                Some(WireValue::List(values)) => values
                    .iter()
                    .map(|value| match value {
                        WireValue::String(value) => Ok(value.clone()),
                        _ => Err(invalid("PromptContext arguments must be strings")),
                    })
                    .collect::<Result<Vec<_>, EchoSdkError>>()?,
                _ => return Err(invalid("PromptContext arguments must be a List")),
            };
            let shell = match request.arguments.get(4) {
                Some(WireValue::Null) => None,
                Some(WireValue::String(value)) => Some(value.clone()),
                _ => return Err(invalid("PromptContext shell must be String or null")),
            };
            let timeout = match request.arguments.get(5) {
                Some(WireValue::Duration(value)) => {
                    value
                        .validate()
                        .map_err(|error| invalid(error.to_string()))?;
                    std::time::Duration::new(
                        value
                            .seconds
                            .to_u64()
                            .ok_or_else(|| invalid("PromptContext timeout is invalid"))?,
                        value.nanos,
                    )
                }
                _ => return Err(invalid("PromptContext timeout must be a Duration")),
            };
            let source = match string_argument(request, 6, "source")?.as_str() {
                "local" => echo_agent::skills::external::SkillSource::Local,
                "mcp" => echo_agent::skills::external::SkillSource::Mcp,
                _ => return Err(invalid("PromptContext source must be local or mcp")),
            };
            let sandbox: Option<Arc<dyn echo_agent::sandbox::SandboxExecutor>> = match request
                .arguments
                .get(7)
            {
                Some(WireValue::Null) => None,
                #[cfg(feature = "sdk-extension-bridge")]
                Some(WireValue::Handle(extension)) if extension.kind == HandleKind::Extension => {
                    let proxy =
                        crate::core_profile::extension_bridge::ExtensionAgentComponentProxy::new(
                            state.extension_bridge.clone(),
                            extension.clone(),
                            owner.clone(),
                        )
                        .filter(|proxy| {
                            proxy.component() == AgentComponentKindWire::SandboxExecutor
                        })
                        .ok_or_else(|| {
                            invalid("PromptContext sandbox must be a SandboxExecutor extension")
                        })?;
                    Some(Arc::new(proxy))
                }
                _ => {
                    return Err(invalid(
                        "PromptContext sandbox must be an Extension handle or null",
                    ));
                }
            };
            let context = echo_agent::skills::external::PromptContext {
                skill_dir: string_argument(request, 1, "skill_dir")?,
                session_id: string_argument(request, 2, "session_id")?,
                arguments,
                shell,
                timeout,
                source,
                sandbox,
            };
            let resource =
                register_source_resource(state, &owner, "skills", "skills.prompt_context")?;
            state
                .facade
                .prompt_contexts
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(resource.id.clone(), Arc::new(context));
            Ok(WireValue::Handle(resource))
        }
        "echo_execution::skills::external::prompt_exec::process_skill_content" => {
            let owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 3 {
                return Err(invalid(
                    "process_skill_content accepts [Session, content, PromptContext]",
                ));
            }
            let context =
                source_resource_at(state, request, &owner, 2, "skills", "skills.prompt_context")?;
            let context = state
                .facade
                .prompt_contexts
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .get(&context.id)
                .cloned()
                .ok_or_else(|| invalid("PromptContext resource is closed"))?;
            Ok(WireValue::String(
                echo_agent::skills::external::process_skill_content(
                    &string_argument(request, 1, "content")?,
                    context.as_ref(),
                )
                .await,
            ))
        }
        #[cfg(feature = "sdk-extension-bridge")]
        "echo_core::tokenizer::Tokenizer::count_tokens" => {
            if request.arguments.len() != 2 {
                return Err(invalid(
                    "Tokenizer::count_tokens accepts owner context and one text argument",
                ));
            }
            let text = string_argument(request, 1, "text")?;
            let count = tokenizer_authority(state, request)?.count_tokens(&text);
            Ok(WireValue::U64(WireU64::from_u64(
                u64::try_from(count).unwrap_or(u64::MAX),
            )))
        }
        "echo_agent::security::contains_secrets"
        | "echo_agent::security::redact_secrets"
        | "echo_agent::security::scan_secrets"
        | "echo_agent::security::scan_summary" => {
            let _owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 2 {
                return Err(invalid("security helper accepts [Session, text]"));
            }
            let text = string_argument(request, 1, "text")?;
            match request.operation.as_str() {
                "echo_agent::security::contains_secrets" => Ok(WireValue::Bool(
                    echo_agent::security::contains_secrets(&text),
                )),
                "echo_agent::security::redact_secrets" => Ok(WireValue::String(
                    echo_agent::security::redact_secrets(&text),
                )),
                "echo_agent::security::scan_summary" => snapshot_value(
                    request,
                    serde_json::to_value(echo_agent::security::scan_summary(&text))
                        .map_err(|error| framework(&request.operation, error.to_string()))?,
                ),
                _ => {
                    let matches = echo_agent::security::scan_secrets(&text)
                        .into_iter()
                        .map(|found| {
                            let position = u64::try_from(found.position).map_err(|_| {
                                framework(&request.operation, "secret position exceeds WireU64")
                            })?;
                            Ok(serde_json::json!({
                                "secret_type": found.secret_type,
                                "matched": found.matched,
                                "position": WireU64::from_u64(position),
                            }))
                        })
                        .collect::<Result<Vec<_>, EchoSdkError>>()?;
                    snapshot_value(request, serde_json::Value::Array(matches))
                }
            }
        }
        "echo_execution::risk::ToolRiskClassifier::classify"
        | "echo_execution::risk::ToolRiskClassifier::safety_notice" => {
            let _owner = source_session_owner(state, request).await?;
            let tool_name = string_argument(request, 1, "tool name")?;
            if request.operation.ends_with("::classify") {
                if request.arguments.len() != 2 {
                    return Err(invalid(
                        "ToolRiskClassifier::classify accepts [Session, name]",
                    ));
                }
                let category = echo_agent::tools::risk::ToolRiskClassifier::classify(&tool_name);
                let variant = match category {
                    echo_agent::tools::risk::ToolRiskCategory::ReadOnly => "read_only",
                    echo_agent::tools::risk::ToolRiskCategory::FileWrite => "file_write",
                    echo_agent::tools::risk::ToolRiskCategory::ShellExec => "shell_exec",
                    echo_agent::tools::risk::ToolRiskCategory::GitWrite => "git_write",
                    echo_agent::tools::risk::ToolRiskCategory::DatabaseWrite => "database_write",
                    echo_agent::tools::risk::ToolRiskCategory::NetworkCall => "network_call",
                    echo_agent::tools::risk::ToolRiskCategory::Destructive => "destructive",
                };
                Ok(WireValue::Variant {
                    type_id: "echo_execution::risk::ToolRiskCategory".to_string(),
                    variant: variant.to_string(),
                    fields: Vec::new(),
                })
            } else {
                if request.arguments.len() != 3 {
                    return Err(invalid(
                        "ToolRiskClassifier::safety_notice accepts [Session, name, params]",
                    ));
                }
                let params: echo_agent::tools::ToolParameters = serde_json::from_value(
                    json_argument(request, 2, "tool parameters")?,
                )
                .map_err(|error| invalid(format!("tool parameters are invalid: {error}")))?;
                Ok(WireValue::String(
                    echo_agent::tools::risk::ToolRiskClassifier::safety_notice(&tool_name, &params),
                ))
            }
        }
        "echo_execution::sandbox::policy::SandboxPolicy::evaluate"
        | "echo_execution::sandbox::policy::SandboxPolicy::evaluate_with_limits" => {
            let _owner = source_session_owner(state, request).await?;
            let expected = if request.operation.ends_with("evaluate_with_limits") {
                4
            } else {
                3
            };
            if request.arguments.len() != expected {
                return Err(invalid(
                    "SandboxPolicy evaluation accepts [Session, policy, command, optional limits]",
                ));
            }
            let policy = sandbox_policy_argument(request, 1)?;
            let command: echo_agent::sandbox::SandboxCommand =
                serde_json::from_value(json_argument(request, 2, "sandbox command")?)
                    .map_err(|error| invalid(format!("sandbox command is invalid: {error}")))?;
            let level = if expected == 4 {
                let limits: Option<echo_agent::sandbox::ResourceLimits> =
                    match request.arguments.get(3) {
                        Some(WireValue::Null) => None,
                        Some(_) => Some(
                            serde_json::from_value(json_argument(
                                request,
                                3,
                                "sandbox resource limits",
                            )?)
                            .map_err(|error| {
                                invalid(format!("sandbox resource limits are invalid: {error}"))
                            })?,
                        ),
                        None => None,
                    };
                policy.evaluate_with_limits(&command, limits.as_ref())
            } else {
                policy.evaluate(&command)
            };
            Ok(WireValue::String(level.to_string()))
        }
        "echo_agent::evolution::security::PromptInjectionDetector::detect"
        | "echo_agent::evolution::security::SecretScanner::contains_secrets"
        | "echo_agent::evolution::security::SecretScanner::scan" => {
            let _owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 2 {
                return Err(invalid("evolution security helper accepts [Session, text]"));
            }
            let text = string_argument(request, 1, "text")?;
            match request.operation.as_str() {
                "echo_agent::evolution::security::PromptInjectionDetector::detect" => {
                    Ok(WireValue::Bool(
                        echo_agent::evolution::security::PromptInjectionDetector.detect(&text),
                    ))
                }
                "echo_agent::evolution::security::SecretScanner::contains_secrets" => {
                    Ok(WireValue::Bool(
                        echo_agent::evolution::security::SecretScanner::new()
                            .contains_secrets(&text),
                    ))
                }
                _ => snapshot_value(
                    request,
                    serde_json::to_value(
                        echo_agent::evolution::security::SecretScanner::new().scan(&text),
                    )
                    .map_err(|error| framework(&request.operation, error.to_string()))?,
                ),
            }
        }
        "echo_agent::evolution::curator::CuratorState::try_load" => {
            let _owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 2 {
                return Err(invalid("CuratorState::try_load accepts [Session, Path]"));
            }
            let path = path_argument(request, 1)?;
            let state = echo_agent::evolution::CuratorState::try_load(&path)
                .map_err(|error| framework(&request.operation, error.to_string()))?;
            snapshot_value(
                request,
                serde_json::to_value(state)
                    .map_err(|error| framework(&request.operation, error.to_string()))?,
            )
        }
        "echo_agent::evolution::curator::CuratorState::save" => {
            let _owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 3 {
                return Err(invalid("CuratorState::save accepts [Session, state, Path]"));
            }
            let curator: echo_agent::evolution::CuratorState =
                serde_json::from_value(json_argument(request, 1, "curator state")?)
                    .map_err(|error| invalid(format!("curator state is malformed: {error}")))?;
            let path = path_argument(request, 2)?;
            curator
                .save(&path)
                .map_err(|error| framework(&request.operation, error.to_string()))?;
            Ok(WireValue::Null)
        }
        "echo_agent::evolution::merge::SkillSimilarityDetector::compute_similarity" => {
            let _owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 3 {
                return Err(invalid(
                    "SkillSimilarityDetector::compute_similarity accepts [Session, descriptor_a, descriptor_b]",
                ));
            }
            let descriptor = |position, label| {
                serde_json::from_value::<echo_agent::skills::external::SkillDescriptor>(
                    json_argument(request, position, label)?,
                )
                .map_err(|error| invalid(format!("{label} is malformed: {error}")))
            };
            let left = descriptor(1, "left skill descriptor")?;
            let right = descriptor(2, "right skill descriptor")?;
            let result =
                echo_agent::evolution::SkillSimilarityDetector::compute_similarity(&left, &right);
            snapshot_value(
                request,
                serde_json::to_value(result)
                    .map_err(|error| framework(&request.operation, error.to_string()))?,
            )
        }
        "echo_agent::evolution::merge::SkillSimilarityDetector::overall_score" => {
            let _owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 2 {
                return Err(invalid(
                    "SkillSimilarityDetector::overall_score accepts [Session, breakdown]",
                ));
            }
            let breakdown: echo_agent::evolution::SimilarityBreakdown =
                serde_json::from_value(json_argument(request, 1, "similarity breakdown")?)
                    .map_err(|error| {
                        invalid(format!("similarity breakdown is malformed: {error}"))
                    })?;
            Ok(WireValue::F64(
                echo_agent::evolution::SkillSimilarityDetector::overall_score(&breakdown),
            ))
        }
        "echo_agent::evolution::review::StalenessScorer::score" => {
            let _owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 4 {
                return Err(invalid(
                    "StalenessScorer::score accepts [Session, entry, RFC3339 timestamp, contradiction]",
                ));
            }
            let value = json_argument(request, 1, "typed memory entry")?;
            let object = value
                .as_object()
                .ok_or_else(|| invalid("typed memory entry must be a record"))?;
            let key = object
                .get("key")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| invalid("typed memory entry requires key"))?
                .to_string();
            let content = object
                .get("content")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| invalid("typed memory entry requires content"))?
                .to_string();
            let meta = serde_json::from_value(
                object
                    .get("meta")
                    .cloned()
                    .ok_or_else(|| invalid("typed memory entry requires meta"))?,
            )
            .map_err(|error| invalid(format!("typed memory metadata is malformed: {error}")))?;
            let raw = serde_json::from_value(
                object
                    .get("raw")
                    .cloned()
                    .ok_or_else(|| invalid("typed memory entry requires raw StoreItem"))?,
            )
            .map_err(|error| invalid(format!("typed memory StoreItem is malformed: {error}")))?;
            let entry = echo_agent::memory::TypedMemoryEntry {
                key,
                content,
                meta,
                raw,
            };
            let now = string_argument(request, 2, "RFC3339 timestamp")?;
            let now = chrono::DateTime::parse_from_rfc3339(&now)
                .map_err(|error| invalid(format!("staleness timestamp is invalid: {error}")))?
                .with_timezone(&chrono::Utc);
            let contradiction = match request.arguments.get(3) {
                Some(WireValue::Bool(value)) => *value,
                _ => return Err(invalid("staleness contradiction flag must be Bool")),
            };
            let report =
                echo_agent::evolution::StalenessScorer::new().score(&entry, now, contradiction);
            snapshot_value(
                request,
                serde_json::json!({
                    "key": report.key,
                    "staleness": report.staleness,
                    "age_factor": report.age_factor,
                    "usage_factor": report.usage_factor,
                    "instability_factor": report.instability_factor,
                    "contradiction_factor": report.contradiction_factor,
                    "source_factor": report.source_factor,
                    "recommended_status": report.recommended_status,
                }),
            )
        }
        "echo_agent::evolution::triggers::TriggerDetector::detect" => {
            let _owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 3 {
                return Err(invalid(
                    "TriggerDetector::detect accepts [Session, max_per_turn, context]",
                ));
            }
            let max_per_turn = usize_argument(request, 1)?;
            let context = trigger_context_argument(request, 2)?;
            let detected = echo_agent::evolution::TriggerDetector::with_max_per_turn(max_per_turn)
                .detect(&context)
                .into_iter()
                .map(|trigger| {
                    serde_json::json!({
                        "content": trigger.content,
                        "memory_type": trigger.memory_type,
                        "source": trigger.source,
                        "confidence": trigger.confidence,
                        "topic": trigger.topic,
                        "trust_level": trigger.trust_level,
                        "suggested_key": trigger.suggested_key,
                        "evidence": trigger.evidence.into_iter().map(|evidence| serde_json::json!({
                            "source_role": evidence.source_role,
                            "quote": evidence.quote,
                        })).collect::<Vec<_>>(),
                    })
                })
                .collect::<Vec<_>>();
            snapshot_value(request, serde_json::Value::Array(detected))
        }
        "echo_orchestration::planning::validator::PlanValidator::validate_task_specs"
        | "echo_orchestration::planning::validator::PlanValidator::validate_task_snapshot" => {
            let _owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 5 {
                return Err(invalid(
                    "PlanValidator operation accepts [Session, max_tasks, max_depth, max_retries, tasks]",
                ));
            }
            let max_retries = u32::try_from(u64_argument(request, 3)?)
                .map_err(|_| invalid("PlanValidator max_retries exceeds u32"))?;
            let validator = echo_agent::tasks::PlanValidator {
                max_tasks: usize_argument(request, 1)?,
                max_depth: usize_argument(request, 2)?,
                max_retries,
            };
            let tasks = json_argument(request, 4, "tasks")?;
            let errors = if request.operation.ends_with("validate_task_specs") {
                let tasks: Vec<echo_agent::tasks::TaskSpec> = serde_json::from_value(tasks)
                    .map_err(|error| invalid(format!("task specs are malformed: {error}")))?;
                validator.validate_task_specs(&tasks).err()
            } else {
                let tasks: Vec<echo_agent::tasks::Task> = serde_json::from_value(tasks)
                    .map_err(|error| invalid(format!("task snapshot is malformed: {error}")))?;
                validator.validate_task_snapshot(&tasks).err()
            };
            snapshot_value(
                request,
                serde_json::json!({
                    "valid": errors.is_none(),
                    "errors": errors.unwrap_or_default(),
                }),
            )
        }
        "echo_core::tools::artifact::persist_tool_output" => {
            let _owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 4 {
                return Err(invalid(
                    "persist_tool_output accepts [Session, config, identity, output]",
                ));
            }
            let config = artifact_config_argument(request, 1)?;
            let identity = artifact_identity_argument(request, 2)?;
            let output = match request.arguments.get(3) {
                Some(WireValue::String(value)) => value.clone(),
                _ => return Err(invalid("tool output must be String")),
            };
            let artifact =
                echo_agent::tools::artifact::persist_tool_output(config, identity, &output)
                    .map_err(|error| file_error(&request.operation, error))?;
            match artifact {
                Some(artifact) => {
                    let path = wire::path_to_wire(&artifact.path)
                        .map_err(|error| framework(&request.operation, error))?;
                    Ok(WireValue::Record {
                        type_id: "echo_core::tools::artifact::ToolOutputArtifactRef".to_string(),
                        fields: vec![
                            echo_sdk_protocol::scalar::WireField {
                                name: "path".to_string(),
                                value: WireValue::Path(path),
                            },
                            echo_sdk_protocol::scalar::WireField {
                                name: "artifact_bytes".to_string(),
                                value: WireValue::U64(WireU64::from_u64(artifact.artifact_bytes)),
                            },
                            echo_sdk_protocol::scalar::WireField {
                                name: "payload_bytes".to_string(),
                                value: WireValue::U64(WireU64::from_u64(artifact.payload_bytes)),
                            },
                            echo_sdk_protocol::scalar::WireField {
                                name: "sha256".to_string(),
                                value: WireValue::String(artifact.sha256),
                            },
                            echo_sdk_protocol::scalar::WireField {
                                name: "retention".to_string(),
                                value: WireValue::String(artifact.retention),
                            },
                        ],
                    })
                }
                None => Ok(WireValue::Null),
            }
        }
        "echo_core::tools::artifact::cleanup_tool_output_scope" => {
            let _owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 4 {
                return Err(invalid(
                    "cleanup_tool_output_scope accepts [Session, config, conversation_id, run_id|null]",
                ));
            }
            let config = artifact_config_argument(request, 1)?;
            let conversation_id = string_argument(request, 2, "conversation id")?;
            let run_id = match request.arguments.get(3) {
                Some(WireValue::Null) => None,
                Some(WireValue::String(value)) if !value.trim().is_empty() => Some(value.as_str()),
                _ => return Err(invalid("artifact run_id must be String or null")),
            };
            echo_agent::tools::artifact::cleanup_tool_output_scope(
                &config,
                &conversation_id,
                run_id,
            )
            .map_err(|error| file_error(&request.operation, error))?;
            Ok(WireValue::Null)
        }
        #[cfg(feature = "framework-files")]
        "echo_tools::files::artifact::read_artifact_page" => {
            let _owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 5 {
                return Err(invalid(
                    "read_artifact_page accepts [Session, config, artifact, cursor|null, limit]",
                ));
            }
            let config = artifact_config_argument(request, 1)?;
            let artifact = artifact_ref_argument(request, 2)?;
            let cursor = match request.arguments.get(3) {
                Some(WireValue::Null) => None,
                Some(WireValue::String(value)) if !value.trim().is_empty() => Some(value.as_str()),
                _ => return Err(invalid("artifact cursor must be String or null")),
            };
            let limit = artifact_page_limit_argument(request, 4)?;
            let page = echo_agent::tools::files::artifact::read_artifact_page(
                &config, &artifact, cursor, limit,
            )
            .map_err(|error| framework(&request.operation, error.to_string()))?;
            snapshot_value(
                request,
                serde_json::json!({
                    "content": page.content,
                    "next_cursor": page.next_cursor,
                    "complete": page.complete,
                    "sha256": page.sha256,
                    "start_byte": WireU64::from_u64(page.start_byte),
                    "end_byte": WireU64::from_u64(page.end_byte),
                    "total_bytes": WireU64::from_u64(page.total_bytes),
                }),
            )
        }
        #[cfg(feature = "framework-git")]
        "echo_tools::git_checkpoint::create_checkpoint"
        | "echo_tools::git_checkpoint::rollback_to_checkpoint"
        | "echo_tools::git_checkpoint::cleanup_old_checkpoints" => {
            let _owner = source_session_owner(state, request).await?;
            let path = path_argument(request, 1)?;
            match request.operation.as_str() {
                "echo_tools::git_checkpoint::create_checkpoint" => {
                    if request.arguments.len() != 2 {
                        return Err(invalid("create_checkpoint accepts [Session, Path]"));
                    }
                    let checkpoint = echo_agent::tools::git_checkpoint::create_checkpoint(&path)
                        .map_err(|error| framework(&request.operation, error))?;
                    snapshot_value(
                        request,
                        serde_json::to_value(checkpoint)
                            .map_err(|error| framework(&request.operation, error.to_string()))?,
                    )
                }
                "echo_tools::git_checkpoint::rollback_to_checkpoint" => {
                    if request.arguments.len() != 3 {
                        return Err(invalid(
                            "rollback_to_checkpoint accepts [Session, Path, checkpoint_id]",
                        ));
                    }
                    let checkpoint_id = string_argument(request, 2, "checkpoint id")?;
                    echo_agent::tools::git_checkpoint::rollback_to_checkpoint(
                        &path,
                        &checkpoint_id,
                    )
                    .map_err(|error| framework(&request.operation, error))?;
                    Ok(WireValue::Null)
                }
                _ => {
                    if request.arguments.len() != 3 {
                        return Err(invalid(
                            "cleanup_old_checkpoints accepts [Session, Path, keep]",
                        ));
                    }
                    let keep = usize_argument(request, 2)?;
                    echo_agent::tools::git_checkpoint::cleanup_old_checkpoints(&path, keep);
                    Ok(WireValue::Null)
                }
            }
        }
        #[cfg(feature = "framework-git")]
        "echo_tools::git_worktree::create_worktree" => {
            let _owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 3 {
                return Err(invalid(
                    "create_worktree accepts [Session, repo Path, config]",
                ));
            }
            let repo = path_argument(request, 1)?;
            let config = worktree_config_argument(request, 2)?;
            let worktree = echo_agent::tools::git_worktree::create_worktree(&repo, &config)
                .await
                .map_err(|error| framework(&request.operation, error))?;
            managed_worktree_value(&request.operation, worktree)
        }
        #[cfg(feature = "framework-git")]
        "echo_tools::git_worktree::list_worktrees" => {
            let _owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 2 {
                return Err(invalid("list_worktrees accepts [Session, repo Path]"));
            }
            let repo = path_argument(request, 1)?;
            let worktrees = echo_agent::tools::git_worktree::list_worktrees(&repo)
                .await
                .map_err(|error| framework(&request.operation, error))?
                .into_iter()
                .map(|worktree| managed_worktree_value(&request.operation, worktree))
                .collect::<Result<Vec<_>, EchoSdkError>>()?;
            Ok(WireValue::List(worktrees))
        }
        #[cfg(feature = "framework-git")]
        "echo_tools::git_worktree::merge_worktree" => {
            let _owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 4 {
                return Err(invalid(
                    "merge_worktree accepts [Session, repo Path, worktree, target_branch]",
                ));
            }
            let repo = path_argument(request, 1)?;
            let worktree = managed_worktree_argument(request, 2)?;
            let target = string_argument(request, 3, "target branch")?;
            let output = echo_agent::tools::git_worktree::merge_worktree(&repo, &worktree, &target)
                .await
                .map_err(|error| framework(&request.operation, error))?;
            Ok(WireValue::String(output))
        }
        #[cfg(feature = "framework-git")]
        "echo_tools::git_worktree::remove_worktree" => {
            let _owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 3 {
                return Err(invalid(
                    "remove_worktree accepts [Session, repo Path, worktree]",
                ));
            }
            let repo = path_argument(request, 1)?;
            let worktree = managed_worktree_argument(request, 2)?;
            echo_agent::tools::git_worktree::remove_worktree(&repo, &worktree)
                .await
                .map_err(|error| framework(&request.operation, error))?;
            Ok(WireValue::Null)
        }
        "echo_agent::state::clear_persisted_runtime_incarnation"
        | "echo_agent::state::delete_persisted_conversation" => {
            let authorities = session_authorities(state, request).await?;
            let scope_id = string_argument(request, 1, "conversation scope id")?;
            if request
                .operation
                .ends_with("clear_persisted_runtime_incarnation")
            {
                if request.arguments.len() != 3 {
                    return Err(invalid(
                        "clear_persisted_runtime_incarnation accepts [Session, scope_id, runtime_state_id]",
                    ));
                }
                let runtime_state_id = string_argument(request, 2, "runtime state id")?;
                let receipt = authorities
                    .agent_handle
                    .read_async(move |agent| {
                        let conversation_store = agent.conversation_store().clone();
                        let runtime_state_store = agent.state_store().clone();
                        Box::pin(async move {
                            let conversation_store = conversation_store.ok_or_else(|| {
                                echo_agent::error::ReactError::Other(
                                    "Session Agent has no ConversationStore".to_string(),
                                )
                            })?;
                            let runtime_state_store = runtime_state_store.ok_or_else(|| {
                                echo_agent::error::ReactError::Other(
                                    "Session Agent has no RuntimeStateStore".to_string(),
                                )
                            })?;
                            echo_agent::state::clear_persisted_runtime_incarnation(
                                conversation_store.as_ref(),
                                runtime_state_store.as_ref(),
                                &scope_id,
                                &runtime_state_id,
                            )
                            .await
                        })
                    })
                    .await
                    .map_err(|error| framework(&request.operation, error.to_string()))?;
                snapshot_value(
                    request,
                    serde_json::json!({
                        "scope_id": receipt.scope_id,
                        "runtime_state_id": receipt.runtime_state_id,
                        "checkpoint_removed": receipt.checkpoint_removed,
                    }),
                )
            } else {
                if request.arguments.len() != 2 {
                    return Err(invalid(
                        "delete_persisted_conversation accepts [Session, conversation_id]",
                    ));
                }
                let receipt = authorities
                    .agent_handle
                    .read_async(move |agent| {
                        let conversation_store = agent.conversation_store().clone();
                        let runtime_state_store = agent.state_store().clone();
                        Box::pin(async move {
                            let conversation_store = conversation_store.ok_or_else(|| {
                                echo_agent::error::ReactError::Other(
                                    "Session Agent has no ConversationStore".to_string(),
                                )
                            })?;
                            let runtime_state_store = runtime_state_store.ok_or_else(|| {
                                echo_agent::error::ReactError::Other(
                                    "Session Agent has no RuntimeStateStore".to_string(),
                                )
                            })?;
                            echo_agent::state::delete_persisted_conversation(
                                conversation_store.as_ref(),
                                runtime_state_store.as_ref(),
                                &scope_id,
                            )
                            .await
                        })
                    })
                    .await
                    .map_err(|error| framework(&request.operation, error.to_string()))?;
                snapshot_value(
                    request,
                    serde_json::json!({
                        "conversation_id": receipt.conversation_id,
                        "runtime_state_ids": receipt.runtime_state_ids,
                    }),
                )
            }
        }
        "echo_core::utils::fs::create_dir_all_durable" => {
            let _owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 2 {
                return Err(invalid("create_dir_all_durable accepts [Session, Path]"));
            }
            let path = path_argument(request, 1)?;
            echo_agent::utils::fs::create_dir_all_durable(&path)
                .map_err(|error| file_error(&request.operation, error))?;
            Ok(WireValue::Null)
        }
        "echo_core::utils::fs::try_exclusive_file_lease" => {
            let owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 2 {
                return Err(invalid("try_exclusive_file_lease accepts [Session, Path]"));
            }
            let path = path_argument(request, 1)?;
            let lease = echo_agent::utils::fs::try_exclusive_file_lease(&path)
                .map_err(|error| file_error(&request.operation, error))?;
            register_file_authority(
                state,
                &owner,
                "fs.exclusive_file_lease",
                FileAuthorityRecord::Lease(lease),
                &request.operation,
            )
        }
        "echo_core::utils::fs::ExclusiveFileLease::path" => {
            let owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 2 {
                return Err(invalid(
                    "ExclusiveFileLease::path accepts [Session, resource]",
                ));
            }
            let authority = file_authority(state, request, &owner, 1, "fs.exclusive_file_lease")?;
            let path = match authority.as_ref() {
                FileAuthorityRecord::Lease(lease) => lease.path(),
                _ => {
                    return Err(framework(
                        &request.operation,
                        "file authority type mismatch",
                    ));
                }
            };
            let path =
                wire::path_to_wire(path).map_err(|error| framework(&request.operation, error))?;
            Ok(WireValue::Path(path))
        }
        "echo_core::utils::fs::open_existing_regular_guard" => {
            let owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 2 {
                return Err(invalid(
                    "open_existing_regular_guard accepts [Session, Path]",
                ));
            }
            let path = path_argument(request, 1)?;
            let guard = echo_agent::utils::fs::open_existing_regular_guard(&path)
                .map_err(|error| file_error(&request.operation, error))?;
            register_file_authority(
                state,
                &owner,
                "fs.existing_regular_guard",
                FileAuthorityRecord::Regular(guard),
                &request.operation,
            )
        }
        "echo_core::utils::fs::open_existing_directory_guard" => {
            let owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 2 {
                return Err(invalid(
                    "open_existing_directory_guard accepts [Session, Path]",
                ));
            }
            let path = path_argument(request, 1)?;
            let guard = echo_agent::utils::fs::open_existing_directory_guard(&path)
                .map_err(|error| file_error(&request.operation, error))?;
            register_file_authority(
                state,
                &owner,
                "fs.existing_directory_guard",
                FileAuthorityRecord::Directory(guard),
                &request.operation,
            )
        }
        "echo_core::utils::fs::ExistingRegularFileGuard::len"
        | "echo_core::utils::fs::ExistingRegularFileGuard::is_empty" => {
            let owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 2 {
                return Err(invalid(
                    "ExistingRegularFileGuard accessor accepts [Session, resource]",
                ));
            }
            let authority = file_authority(state, request, &owner, 1, "fs.existing_regular_guard")?;
            let guard = match authority.as_ref() {
                FileAuthorityRecord::Regular(guard) => guard,
                _ => {
                    return Err(framework(
                        &request.operation,
                        "file authority type mismatch",
                    ));
                }
            };
            if request.operation.ends_with("::is_empty") {
                guard
                    .is_empty()
                    .map(WireValue::Bool)
                    .map_err(|error| file_error(&request.operation, error))
            } else {
                guard
                    .len()
                    .map(|len| WireValue::U64(WireU64::from_u64(len)))
                    .map_err(|error| file_error(&request.operation, error))
            }
        }
        "echo_core::utils::fs::verify_existing_directory"
        | "echo_core::utils::fs::sync_existing_directory_matching" => {
            let owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 3 {
                return Err(invalid(
                    "directory guard operation accepts [Session, Path, resource]",
                ));
            }
            let path = path_argument(request, 1)?;
            let authority =
                file_authority(state, request, &owner, 2, "fs.existing_directory_guard")?;
            let guard = match authority.as_ref() {
                FileAuthorityRecord::Directory(guard) => guard,
                _ => {
                    return Err(framework(
                        &request.operation,
                        "file authority type mismatch",
                    ));
                }
            };
            let result = if request.operation.ends_with("::verify_existing_directory") {
                echo_agent::utils::fs::verify_existing_directory(&path, guard)
            } else {
                echo_agent::utils::fs::sync_existing_directory_matching(&path, guard)
            };
            result.map_err(|error| file_error(&request.operation, error))?;
            Ok(WireValue::Null)
        }
        "echo_core::utils::fs::matching_existing_regular_len" => {
            let owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 3 {
                return Err(invalid(
                    "matching_existing_regular_len accepts [Session, Path, resource]",
                ));
            }
            let path = path_argument(request, 1)?;
            let authority = file_authority(state, request, &owner, 2, "fs.existing_regular_guard")?;
            let guard = match authority.as_ref() {
                FileAuthorityRecord::Regular(guard) => guard,
                _ => {
                    return Err(framework(
                        &request.operation,
                        "file authority type mismatch",
                    ));
                }
            };
            echo_agent::utils::fs::matching_existing_regular_len(&path, guard)
                .map(|len| WireValue::U64(WireU64::from_u64(len)))
                .map_err(|error| file_error(&request.operation, error))
        }
        "echo_core::utils::fs::append_existing_matching" => {
            let owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 6 {
                return Err(invalid(
                    "append_existing_matching accepts [Session, Path, resource, expected_len, Bytes, durability]",
                ));
            }
            let path = path_argument(request, 1)?;
            let authority = file_authority(state, request, &owner, 2, "fs.existing_regular_guard")?;
            let guard = match authority.as_ref() {
                FileAuthorityRecord::Regular(guard) => guard,
                _ => {
                    return Err(framework(
                        &request.operation,
                        "file authority type mismatch",
                    ));
                }
            };
            let expected_len = u64_argument(request, 3)?;
            let bytes = bytes_argument(request, 4)?;
            let durability = durability_argument(request, 5)?;
            echo_agent::utils::fs::append_existing_matching(
                &path,
                guard,
                expected_len,
                &bytes,
                durability,
            )
            .map_err(|error| file_error(&request.operation, error))?;
            Ok(WireValue::Null)
        }
        "echo_core::utils::fs::read_existing_matching" => {
            let owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 3 {
                return Err(invalid(
                    "read_existing_matching accepts [Session, Path, resource]",
                ));
            }
            let path = path_argument(request, 1)?;
            let authority = file_authority(state, request, &owner, 2, "fs.existing_regular_guard")?;
            let guard = match authority.as_ref() {
                FileAuthorityRecord::Regular(guard) => guard,
                _ => {
                    return Err(framework(
                        &request.operation,
                        "file authority type mismatch",
                    ));
                }
            };
            echo_agent::utils::fs::read_existing_matching(&path, guard)
                .map(bytes_value)
                .map_err(|error| file_error(&request.operation, error))
        }
        "echo_core::utils::fs::read_existing_from_matching" => {
            let owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 4 {
                return Err(invalid(
                    "read_existing_from_matching accepts [Session, Path, resource, offset]",
                ));
            }
            let path = path_argument(request, 1)?;
            let authority = file_authority(state, request, &owner, 2, "fs.existing_regular_guard")?;
            let guard = match authority.as_ref() {
                FileAuthorityRecord::Regular(guard) => guard,
                _ => {
                    return Err(framework(
                        &request.operation,
                        "file authority type mismatch",
                    ));
                }
            };
            let offset = u64_argument(request, 3)?;
            echo_agent::utils::fs::read_existing_from_matching(&path, guard, offset)
                .map(bytes_value)
                .map_err(|error| file_error(&request.operation, error))
        }
        "echo_core::utils::fs::read_existing_lines_from_matching" => {
            let owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 6 {
                return Err(invalid(
                    "read_existing_lines_from_matching accepts [Session, Path, resource, expected_len, offset, limit]",
                ));
            }
            let path = path_argument(request, 1)?;
            let authority = file_authority(state, request, &owner, 2, "fs.existing_regular_guard")?;
            let guard = match authority.as_ref() {
                FileAuthorityRecord::Regular(guard) => guard,
                _ => {
                    return Err(framework(
                        &request.operation,
                        "file authority type mismatch",
                    ));
                }
            };
            let expected_len = u64_argument(request, 3)?;
            let offset = u64_argument(request, 4)?;
            let limit = usize_argument(request, 5)?;
            echo_agent::utils::fs::read_existing_lines_from_matching(
                &path,
                guard,
                expected_len,
                offset,
                limit,
            )
            .map(bytes_value)
            .map_err(|error| file_error(&request.operation, error))
        }
        "echo_core::utils::fs::truncate_existing_matching" => {
            let owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 6 {
                return Err(invalid(
                    "truncate_existing_matching accepts [Session, Path, resource, expected_len, len, durability]",
                ));
            }
            let path = path_argument(request, 1)?;
            let authority = file_authority(state, request, &owner, 2, "fs.existing_regular_guard")?;
            let guard = match authority.as_ref() {
                FileAuthorityRecord::Regular(guard) => guard,
                _ => {
                    return Err(framework(
                        &request.operation,
                        "file authority type mismatch",
                    ));
                }
            };
            let expected_len = u64_argument(request, 3)?;
            let len = u64_argument(request, 4)?;
            let durability = durability_argument(request, 5)?;
            echo_agent::utils::fs::truncate_existing_matching(
                &path,
                guard,
                expected_len,
                len,
                durability,
            )
            .map_err(|error| file_error(&request.operation, error))?;
            Ok(WireValue::Null)
        }
        "echo_core::utils::fs::read_existing_lines_from_exact_len" => {
            let _owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 5 {
                return Err(invalid(
                    "read_existing_lines_from_exact_len accepts [Session, Path, expected_len, offset, limit]",
                ));
            }
            let path = path_argument(request, 1)?;
            let expected_len = u64_argument(request, 2)?;
            let offset = u64_argument(request, 3)?;
            let limit = usize_argument(request, 4)?;
            echo_agent::utils::fs::read_existing_lines_from_exact_len(
                &path,
                expected_len,
                offset,
                limit,
            )
            .map(bytes_value)
            .map_err(|error| file_error(&request.operation, error))
        }
        "echo_core::utils::fs::atomic_write" => {
            let _owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 3 {
                return Err(invalid("atomic_write accepts [Session, Path, Bytes]"));
            }
            let path = path_argument(request, 1)?;
            let bytes = bytes_argument(request, 2)?;
            echo_agent::utils::fs::atomic_write(&path, &bytes)
                .map_err(|error| file_error(&request.operation, error))?;
            Ok(WireValue::Null)
        }
        "echo_core::utils::fs::remove_file_durable" => {
            let _owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 2 {
                return Err(invalid("remove_file_durable accepts [Session, Path]"));
            }
            let path = path_argument(request, 1)?;
            echo_agent::utils::fs::remove_file_durable(&path)
                .map(WireValue::Bool)
                .map_err(|error| file_error(&request.operation, error))
        }
        "echo_core::utils::fs::append_existing" => {
            let _owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 4 {
                return Err(invalid(
                    "append_existing accepts [Session, Path, Bytes, durability]",
                ));
            }
            let path = path_argument(request, 1)?;
            let bytes = bytes_argument(request, 2)?;
            let durability = durability_argument(request, 3)?;
            echo_agent::utils::fs::append_existing(&path, &bytes, durability)
                .map_err(|error| file_error(&request.operation, error))?;
            Ok(WireValue::Null)
        }
        "echo_core::utils::fs::read_existing" => {
            let _owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 2 {
                return Err(invalid("read_existing accepts [Session, Path]"));
            }
            let path = path_argument(request, 1)?;
            echo_agent::utils::fs::read_existing(&path)
                .map(bytes_value)
                .map_err(|error| file_error(&request.operation, error))
        }
        "echo_core::utils::fs::read_existing_from" => {
            let _owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 3 {
                return Err(invalid(
                    "read_existing_from accepts [Session, Path, offset]",
                ));
            }
            let path = path_argument(request, 1)?;
            let offset = u64_argument(request, 2)?;
            echo_agent::utils::fs::read_existing_from(&path, offset)
                .map(bytes_value)
                .map_err(|error| file_error(&request.operation, error))
        }
        "echo_core::utils::fs::read_existing_lines_from" => {
            let _owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 4 {
                return Err(invalid(
                    "read_existing_lines_from accepts [Session, Path, offset, limit]",
                ));
            }
            let path = path_argument(request, 1)?;
            let offset = u64_argument(request, 2)?;
            let limit = usize_argument(request, 3)?;
            echo_agent::utils::fs::read_existing_lines_from(&path, offset, limit)
                .map(bytes_value)
                .map_err(|error| file_error(&request.operation, error))
        }
        "echo_core::utils::fs::truncate_existing" => {
            let _owner = source_session_owner(state, request).await?;
            if request.arguments.len() != 4 {
                return Err(invalid(
                    "truncate_existing accepts [Session, Path, len, durability]",
                ));
            }
            let path = path_argument(request, 1)?;
            let len = u64_argument(request, 2)?;
            let durability = durability_argument(request, 3)?;
            echo_agent::utils::fs::truncate_existing(&path, len, durability)
                .map_err(|error| file_error(&request.operation, error))?;
            Ok(WireValue::Null)
        }
        "echo_core::agent::Agent::name"
        | "echo_agent::agent::Agent::name"
        | "echo_agent::agent::react::ReactAgent::name"
        | "echo_core::agent::Agent::model_name"
        | "echo_agent::agent::Agent::model_name"
        | "echo_agent::agent::react::ReactAgent::model_name"
        | "echo_core::agent::Agent::system_prompt"
        | "echo_agent::agent::Agent::system_prompt"
        | "echo_agent::agent::react::ReactAgent::current_system_prompt"
        | "echo_core::agent::Agent::tool_names"
        | "echo_agent::agent::Agent::tool_names"
        | "echo_agent::agent::react::ReactAgent::tool_names"
        | "echo_agent::agent::react::ReactAgent::list_tools"
        | "echo_core::agent::Agent::skill_names"
        | "echo_agent::agent::Agent::skill_names"
        | "echo_core::agent::Agent::mcp_server_names"
        | "echo_agent::agent::Agent::mcp_server_names"
        | "echo_agent::agent::react::ReactAgent::mcp_server_names"
        | "echo_agent::agent::react::ReactAgent::list_mcp_servers"
        | "echo_core::agent::Agent::working_dir"
        | "echo_agent::agent::Agent::working_dir"
        | "echo_agent::agent::react::ReactAgent::working_dir" => {
            let handle = agent_handle(request)?;
            handles.check_shape_and_generation(handle, HandleKind::Agent, METHOD)?;
            let snapshot = handles.agent(handle)?.definition.snapshot();
            let live = match request.arguments.len() {
                0 => None,
                1 => Some(session_agent(state, request).await?),
                _ => return Err(invalid("Agent accessor accepts zero or one Session handle")),
            };
            let value = match request.operation.rsplit_once("::").map(|(_, name)| name) {
                Some("name") => live
                    .as_ref()
                    .map(|agent| serde_json::json!(agent.name()))
                    .unwrap_or_else(|| serde_json::json!(snapshot.name)),
                Some("model_name") => live
                    .as_ref()
                    .map(|agent| serde_json::json!(agent.model_name()))
                    .unwrap_or_else(|| serde_json::json!(snapshot.model_name)),
                Some("system_prompt") => live
                    .as_ref()
                    .map(|agent| serde_json::json!(agent.system_prompt()))
                    .unwrap_or_else(|| serde_json::json!(snapshot.system_prompt)),
                Some("current_system_prompt") => {
                    if live.is_some() {
                        let authorities = session_authorities(state, request).await?;
                        serde_json::json!(
                            authorities
                                .agent_handle
                                .read(|agent| agent.current_system_prompt())
                                .await
                        )
                    } else {
                        serde_json::json!(snapshot.system_prompt)
                    }
                }
                Some("tool_names") | Some("list_tools") => live
                    .as_ref()
                    .map(|agent| serde_json::json!(agent.tool_names()))
                    .unwrap_or_else(|| serde_json::json!(snapshot.tool_names)),
                Some("skill_names") => live
                    .as_ref()
                    .map(|agent| serde_json::json!(agent.skill_names()))
                    .unwrap_or_else(|| serde_json::json!(snapshot.skill_names)),
                Some("mcp_server_names") | Some("list_mcp_servers") => live
                    .as_ref()
                    .map(|agent| serde_json::json!(agent.mcp_server_names()))
                    .unwrap_or_else(|| serde_json::json!(snapshot.mcp_server_names)),
                Some("working_dir") => live
                    .as_ref()
                    .map(|agent| serde_json::json!(agent.working_dir()))
                    .unwrap_or_else(|| serde_json::json!(snapshot.working_dir)),
                _ => return Err(framework(&request.operation, "unknown Agent accessor")),
            };
            snapshot_value(request, value)
        }
        "echo_agent::agent::react::ReactAgent::skill_names" => {
            let authorities = session_authorities(state, request).await?;
            if request.arguments.len() != 1 {
                return Err(invalid("skill_names accepts exactly one Session handle"));
            }
            let names = authorities
                .agent_handle
                .read(|agent| agent.skill_names())
                .await;
            snapshot_value(
                request,
                serde_json::to_value(names)
                    .map_err(|error| framework(&request.operation, error.to_string()))?,
            )
        }
        // `Agent::current_run_id` is an observational accessor. An Agent
        // definition has no active run; a Session argument reads the live
        // Agent instance used by the ACP runtime.
        "echo_core::agent::Agent::current_run_id" | "echo_agent::agent::Agent::current_run_id" => {
            if request.arguments.is_empty() {
                let handle = agent_handle(request)?;
                handles.check_shape_and_generation(handle, HandleKind::Agent, METHOD)?;
                handles.agent(handle)?;
                snapshot_value(request, serde_json::Value::Null)
            } else {
                let agent = session_agent(state, request).await?;
                if request.arguments.len() != 1 {
                    return Err(invalid("current_run_id accepts zero or one Session handle"));
                }
                snapshot_value(request, serde_json::json!(agent.current_run_id()))
            }
        }
        "echo_agent::agent::react::ReactAgent::current_run_id" => {
            if request.arguments.is_empty() {
                let handle = agent_handle(request)?;
                handles.check_shape_and_generation(handle, HandleKind::Agent, METHOD)?;
                handles.agent(handle)?;
                snapshot_value(request, serde_json::Value::Null)
            } else {
                let agent = session_agent(state, request).await?;
                if request.arguments.len() != 1 {
                    return Err(invalid("current_run_id accepts zero or one Session handle"));
                }
                snapshot_value(request, serde_json::json!(agent.current_run_id()))
            }
        }
        "echo_core::agent::Agent::disabled_tool_names"
        | "echo_agent::agent::Agent::disabled_tool_names" => {
            let agent = session_agent(state, request).await?;
            if request.arguments.len() != 1 {
                return Err(invalid(
                    "live Agent accessor accepts exactly one Session handle",
                ));
            }
            snapshot_value(
                request,
                serde_json::to_value(agent.disabled_tool_names())
                    .map_err(|error| framework(&request.operation, error.to_string()))?,
            )
        }
        "echo_agent::agent::react::ReactAgent::disabled_tool_names" => {
            let agent = session_agent(state, request).await?;
            if request.arguments.len() != 1 {
                return Err(invalid(
                    "live Agent accessor accepts exactly one Session handle",
                ));
            }
            snapshot_value(
                request,
                serde_json::to_value(agent.disabled_tool_names())
                    .map_err(|error| framework(&request.operation, error.to_string()))?,
            )
        }
        "echo_core::agent::Agent::token_usage_summary"
        | "echo_agent::agent::Agent::token_usage_summary" => {
            let agent = session_agent(state, request).await?;
            if request.arguments.len() != 1 {
                return Err(invalid(
                    "live Agent accessor accepts exactly one Session handle",
                ));
            }
            let usage = agent.token_usage_summary();
            snapshot_value(
                request,
                serde_json::json!({
                    "model_name": usage.model_name,
                    "total_prompt_tokens": WireU64::from_u64(usage.total_prompt_tokens),
                    "total_completion_tokens": WireU64::from_u64(usage.total_completion_tokens),
                    "total_tokens": WireU64::from_u64(usage.total_tokens),
                    "total_cached_prompt_tokens": WireU64::from_u64(usage.total_cached_prompt_tokens),
                    "total_cache_creation_prompt_tokens": WireU64::from_u64(usage.total_cache_creation_prompt_tokens),
                    "request_count": WireU64::from_u64(usage.request_count),
                }),
            )
        }
        "echo_agent::agent::react::ReactAgent::token_usage_summary" => {
            let agent = session_agent(state, request).await?;
            if request.arguments.len() != 1 {
                return Err(invalid(
                    "live Agent accessor accepts exactly one Session handle",
                ));
            }
            let usage = agent.token_usage_summary();
            snapshot_value(
                request,
                serde_json::json!({
                    "model_name": usage.model_name,
                    "total_prompt_tokens": WireU64::from_u64(usage.total_prompt_tokens),
                    "total_completion_tokens": WireU64::from_u64(usage.total_completion_tokens),
                    "total_tokens": WireU64::from_u64(usage.total_tokens),
                    "total_cached_prompt_tokens": WireU64::from_u64(usage.total_cached_prompt_tokens),
                    "total_cache_creation_prompt_tokens": WireU64::from_u64(usage.total_cache_creation_prompt_tokens),
                    "request_count": WireU64::from_u64(usage.request_count),
                }),
            )
        }
        "echo_agent::agent::ReactAgent::max_iterations"
        | "echo_agent::agent::react::ReactAgent::max_iterations" => {
            let authorities = session_authorities(state, request).await?;
            if request.arguments.len() != 1 {
                return Err(invalid("max_iterations accepts exactly one Session handle"));
            }
            let max_iterations = authorities
                .agent_handle
                .read(|agent| agent.max_iterations())
                .await;
            let max_iterations = u64::try_from(max_iterations)
                .map_err(|_| framework(&request.operation, "max_iterations exceeds WireU64"))?;
            snapshot_value(
                request,
                serde_json::json!(WireU64::from_u64(max_iterations)),
            )
        }
        "echo_agent::agent::ReactAgent::is_plan_mode"
        | "echo_agent::agent::react::ReactAgent::is_plan_mode" => {
            let authorities = session_authorities(state, request).await?;
            if request.arguments.len() != 1 {
                return Err(invalid("is_plan_mode accepts exactly one Session handle"));
            }
            snapshot_value(
                request,
                serde_json::json!(
                    authorities
                        .agent_handle
                        .read(|agent| agent.is_plan_mode())
                        .await
                ),
            )
        }
        "echo_agent::agent::ReactAgent::get_permission_mode"
        | "echo_agent::agent::react::ReactAgent::get_permission_mode" => {
            let authorities = session_authorities(state, request).await?;
            if request.arguments.len() != 1 {
                return Err(invalid(
                    "get_permission_mode accepts exactly one Session handle",
                ));
            }
            let mode = authorities
                .agent_handle
                .read(|agent| agent.get_permission_mode())
                .await;
            snapshot_value(
                request,
                serde_json::to_value(mode)
                    .map_err(|error| framework(&request.operation, error.to_string()))?,
            )
        }
        "echo_agent::agent::ReactAgent::context_stats"
        | "echo_agent::agent::react::ReactAgent::context_stats" => {
            let authorities = session_authorities(state, request).await?;
            if request.arguments.len() != 1 {
                return Err(invalid("context_stats accepts exactly one Session handle"));
            }
            let stats = authorities
                .agent_handle
                .read_async(|agent| Box::pin(agent.context_stats()))
                .await;
            snapshot_value(
                request,
                serde_json::to_value(stats)
                    .map_err(|error| framework(&request.operation, error.to_string()))?,
            )
        }
        "echo_agent::agent::ReactAgent::snapshot"
        | "echo_agent::agent::react::ReactAgent::snapshot" => {
            let authorities = session_authorities(state, request).await?;
            if request.arguments.len() != 1 {
                return Err(invalid("snapshot accepts exactly one Session handle"));
            }
            let snapshot = authorities
                .agent_handle
                .read_async(|agent| Box::pin(agent.snapshot()))
                .await;
            snapshot_value(
                request,
                serde_json::to_value(snapshot)
                    .map_err(|error| framework(&request.operation, error.to_string()))?,
            )
        }
        "echo_agent::agent::ReactAgent::snapshots"
        | "echo_agent::agent::react::ReactAgent::snapshots" => {
            let authorities = session_authorities(state, request).await?;
            if request.arguments.len() != 1 {
                return Err(invalid("snapshots accepts exactly one Session handle"));
            }
            let snapshots = authorities
                .agent_handle
                .read(|agent| agent.snapshots())
                .await;
            snapshot_value(
                request,
                serde_json::to_value(snapshots)
                    .map_err(|error| framework(&request.operation, error.to_string()))?,
            )
        }
        "echo_agent::agent::ReactAgent::latest_snapshot"
        | "echo_agent::agent::react::ReactAgent::latest_snapshot" => {
            let authorities = session_authorities(state, request).await?;
            if request.arguments.len() != 1 {
                return Err(invalid(
                    "latest_snapshot accepts exactly one Session handle",
                ));
            }
            let snapshot = authorities
                .agent_handle
                .read(|agent| agent.latest_snapshot())
                .await;
            snapshot_value(
                request,
                serde_json::to_value(snapshot)
                    .map_err(|error| framework(&request.operation, error.to_string()))?,
            )
        }
        "echo_agent::agent::ReactAgent::conversation_id"
        | "echo_agent::agent::react::ReactAgent::conversation_id" => {
            let authorities = session_authorities(state, request).await?;
            if request.arguments.len() != 1 {
                return Err(invalid(
                    "conversation_id accepts exactly one Session handle",
                ));
            }
            snapshot_value(
                request,
                serde_json::to_value(
                    authorities
                        .agent_handle
                        .read(|agent| agent.conversation_id().map(str::to_string))
                        .await,
                )
                .map_err(|error| framework(&request.operation, error.to_string()))?,
            )
        }
        "echo_agent::agent::ReactAgent::has_memory_layer_manager"
        | "echo_agent::agent::react::ReactAgent::has_memory_layer_manager" => {
            let authorities = session_authorities(state, request).await?;
            if request.arguments.len() != 1 {
                return Err(invalid(
                    "has_memory_layer_manager accepts exactly one Session handle",
                ));
            }
            snapshot_value(
                request,
                serde_json::json!(
                    authorities
                        .agent_handle
                        .read(|agent| agent.has_memory_layer_manager())
                        .await
                ),
            )
        }
        "echo_agent::agent::ReactAgent::thinking"
        | "echo_agent::agent::react::ReactAgent::thinking" => {
            let authorities = session_authorities(state, request).await?;
            if request.arguments.len() != 1 {
                return Err(invalid("thinking accepts exactly one Session handle"));
            }
            snapshot_value(
                request,
                serde_json::to_value(
                    authorities
                        .agent_handle
                        .read(|agent| agent.thinking().cloned())
                        .await,
                )
                .map_err(|error| framework(&request.operation, error.to_string()))?,
            )
        }
        "echo_agent::agent::ReactAgent::set_plan_mode"
        | "echo_agent::agent::react::ReactAgent::set_plan_mode" => {
            let authorities = session_authorities(state, request).await?;
            let enabled = match request.arguments.get(1) {
                Some(WireValue::Bool(value)) => *value,
                Some(_) => return Err(invalid("set_plan_mode requires a boolean value")),
                None => return Err(invalid("set_plan_mode requires a boolean value")),
            };
            if request.arguments.len() != 2 {
                return Err(invalid("set_plan_mode accepts [Session handle, Bool]"));
            }
            authorities
                .agent_handle
                .write(|agent| agent.set_plan_mode(enabled))
                .await;
            snapshot_value(request, serde_json::Value::Null)
        }
        "echo_agent::agent::ReactAgent::set_permission_mode"
        | "echo_agent::agent::react::ReactAgent::set_permission_mode" => {
            let authorities = session_authorities(state, request).await?;
            let mode_value = request
                .arguments
                .get(1)
                .cloned()
                .ok_or_else(|| invalid("set_permission_mode requires a mode value"))?
                .into_json()
                .map_err(|error| {
                    invalid(format!("permission mode is not a wire value: {error}"))
                })?;
            let mode: echo_agent::tools::permission::PermissionMode =
                serde_json::from_value(mode_value)
                    .map_err(|error| invalid(format!("permission mode is invalid: {error}")))?;
            if request.arguments.len() != 2 {
                return Err(invalid(
                    "set_permission_mode accepts [Session handle, PermissionMode]",
                ));
            }
            authorities
                .agent_handle
                .write(|agent| agent.set_permission_mode(mode))
                .await;
            snapshot_value(request, serde_json::Value::Null)
        }
        "echo_agent::agent::ReactAgent::set_conversation_id"
        | "echo_agent::agent::react::ReactAgent::set_conversation_id" => {
            let authorities = session_authorities(state, request).await?;
            let conversation_id = match request.arguments.get(1) {
                Some(WireValue::String(value)) if !value.trim().is_empty() => value.clone(),
                Some(_) => return Err(invalid("set_conversation_id requires a non-empty string")),
                None => return Err(invalid("set_conversation_id requires a string value")),
            };
            if request.arguments.len() != 2 {
                return Err(invalid(
                    "set_conversation_id accepts [Session handle, String]",
                ));
            }
            authorities
                .agent_handle
                .write(|agent| agent.set_conversation_id(conversation_id))
                .await;
            snapshot_value(request, serde_json::Value::Null)
        }
        "echo_agent::agent::ReactAgent::set_model"
        | "echo_agent::agent::react::ReactAgent::set_model" => {
            let authorities = session_authorities(state, request).await?;
            let model = match request.arguments.get(1) {
                Some(WireValue::String(value)) if !value.trim().is_empty() => value.clone(),
                Some(_) => return Err(invalid("set_model requires a non-empty string")),
                None => return Err(invalid("set_model requires a model name")),
            };
            if request.arguments.len() != 2 {
                return Err(invalid("set_model accepts [Session handle, String]"));
            }
            authorities
                .agent_handle
                .write(|agent| agent.set_model(&model))
                .await;
            snapshot_value(request, serde_json::Value::Null)
        }
        "echo_agent::agent::ReactAgent::set_temperature"
        | "echo_agent::agent::react::ReactAgent::set_temperature" => {
            let authorities = session_authorities(state, request).await?;
            let temperature = match request.arguments.get(1) {
                Some(WireValue::Null) => None,
                Some(value) => {
                    let json = value.clone().into_json().map_err(|error| {
                        invalid(format!("temperature is not a wire value: {error}"))
                    })?;
                    let parsed: f32 = serde_json::from_value(json)
                        .map_err(|error| invalid(format!("temperature is invalid: {error}")))?;
                    if !parsed.is_finite() {
                        return Err(invalid("temperature must be finite"));
                    }
                    Some(parsed)
                }
                None => return Err(invalid("set_temperature requires a value")),
            };
            if request.arguments.len() != 2 {
                return Err(invalid(
                    "set_temperature accepts [Session handle, F64|null]",
                ));
            }
            authorities
                .agent_handle
                .write(|agent| agent.set_temperature(temperature))
                .await;
            snapshot_value(request, serde_json::Value::Null)
        }
        "echo_agent::agent::ReactAgent::set_max_tokens"
        | "echo_agent::agent::react::ReactAgent::set_max_tokens" => {
            let authorities = session_authorities(state, request).await?;
            let max_tokens = match request.arguments.get(1) {
                Some(WireValue::Null) => None,
                Some(WireValue::U64(value)) => Some(
                    value
                        .to_u64()
                        .ok_or_else(|| invalid("max_tokens value is invalid"))
                        .and_then(|value| {
                            u32::try_from(value).map_err(|_| invalid("max_tokens exceeds u32"))
                        })?,
                ),
                Some(_) => return Err(invalid("set_max_tokens requires a U64|null value")),
                None => return Err(invalid("set_max_tokens requires a value")),
            };
            if request.arguments.len() != 2 {
                return Err(invalid("set_max_tokens accepts [Session handle, U64|null]"));
            }
            authorities
                .agent_handle
                .write(|agent| agent.set_max_tokens(max_tokens))
                .await;
            snapshot_value(request, serde_json::Value::Null)
        }
        "echo_agent::agent::ReactAgent::set_thinking"
        | "echo_agent::agent::react::ReactAgent::set_thinking" => {
            let authorities = session_authorities(state, request).await?;
            let value = request
                .arguments
                .get(1)
                .cloned()
                .ok_or_else(|| invalid("set_thinking requires a value"))?;
            let thinking =
                match value {
                    WireValue::Null => None,
                    value => {
                        let json = value.into_json().map_err(|error| {
                            invalid(format!("thinking is not a wire value: {error}"))
                        })?;
                        if let serde_json::Value::String(spec) = &json {
                            echo_agent::llm::ThinkingConfig::parse_spec(spec)
                                .map_err(|error| invalid(format!("thinking is invalid: {error}")))?
                        } else {
                            Some(serde_json::from_value(json).map_err(|error| {
                                invalid(format!("thinking is invalid: {error}"))
                            })?)
                        }
                    }
                };
            if request.arguments.len() != 2 {
                return Err(invalid(
                    "set_thinking accepts [Session handle, ThinkingConfig|null]",
                ));
            }
            authorities
                .agent_handle
                .write(|agent| agent.set_thinking(thinking))
                .await;
            snapshot_value(request, serde_json::Value::Null)
        }
        "echo_agent::agent::ReactAgent::set_token_limit"
        | "echo_agent::agent::react::ReactAgent::set_token_limit" => {
            let authorities = session_authorities(state, request).await?;
            let token_limit = match request.arguments.get(1) {
                Some(WireValue::U64(value)) => value
                    .to_u64()
                    .ok_or_else(|| invalid("token_limit value is invalid"))
                    .and_then(|value| {
                        usize::try_from(value)
                            .map_err(|_| invalid("token_limit exceeds platform usize"))
                    })?,
                Some(_) => return Err(invalid("set_token_limit requires a U64 value")),
                None => return Err(invalid("set_token_limit requires a U64 value")),
            };
            if request.arguments.len() != 2 {
                return Err(invalid("set_token_limit accepts [Session handle, U64]"));
            }
            authorities
                .agent_handle
                .write(|agent| agent.set_token_limit(token_limit))
                .await
                .map_err(|error| framework(&request.operation, error.to_string()))?;
            snapshot_value(request, serde_json::Value::Null)
        }
        "echo_agent::agent::ReactAgent::set_disabled_tools"
        | "echo_agent::agent::react::ReactAgent::set_disabled_tools" => {
            let authorities = session_authorities(state, request).await?;
            let disabled = match request.arguments.get(1) {
                Some(WireValue::Null) => None,
                Some(WireValue::List(values)) => {
                    let mut names = std::collections::HashSet::with_capacity(values.len());
                    for value in values {
                        let WireValue::String(name) = value else {
                            return Err(invalid("set_disabled_tools list must contain strings"));
                        };
                        if name.trim().is_empty() {
                            return Err(invalid("set_disabled_tools names must be non-empty"));
                        }
                        names.insert(name.clone());
                    }
                    Some(names)
                }
                Some(_) => {
                    return Err(invalid("set_disabled_tools requires a string list|null"));
                }
                None => return Err(invalid("set_disabled_tools requires a value")),
            };
            if request.arguments.len() != 2 {
                return Err(invalid(
                    "set_disabled_tools accepts [Session handle, List<String>|Null]",
                ));
            }
            authorities
                .agent_handle
                .write(|agent| agent.set_disabled_tools(disabled))
                .await;
            snapshot_value(request, serde_json::Value::Null)
        }
        "echo_agent::agent::ReactAgent::set_store"
        | "echo_agent::agent::react::ReactAgent::set_store" => {
            #[cfg(feature = "sdk-extension-bridge")]
            {
                let authorities = session_authorities(state, request).await?;
                let session_handle = match request.arguments.first() {
                    Some(WireValue::Handle(handle)) if handle.kind == HandleKind::Session => handle,
                    Some(WireValue::Handle(_)) => {
                        return Err(invalid("set_store requires a Session handle"));
                    }
                    Some(_) | None => return Err(invalid("set_store requires a Session handle")),
                };
                let extension = match request.arguments.get(1) {
                    Some(WireValue::Handle(handle)) if handle.kind == HandleKind::Extension => {
                        handle.clone()
                    }
                    Some(WireValue::Handle(_)) => {
                        return Err(invalid("set_store requires an Extension handle"));
                    }
                    Some(_) | None => {
                        return Err(invalid("set_store requires an Extension handle"));
                    }
                };
                if request.arguments.len() != 2 {
                    return Err(invalid(
                        "set_store accepts [Session handle, Extension handle]",
                    ));
                }
                let record = handles.extension(&extension)?;
                if record.kind != echo_sdk_protocol::methods::ExtensionKind::Store {
                    return Err(invalid("set_store requires a Store extension handle"));
                }
                let session_record = handles.session(session_handle)?;
                let proxy = crate::core_profile::extension_bridge::ExtensionStoreProxy::new(
                    state.extension_bridge.clone(),
                    extension,
                    session_record.acp_session_id.clone(),
                )
                .ok_or_else(|| {
                    framework(&request.operation, "Store extension proxy is unavailable")
                })?;
                authorities
                    .agent_handle
                    .write(|agent| agent.set_store(Arc::new(proxy)))
                    .await;
                snapshot_value(request, serde_json::Value::Null)
            }
            #[cfg(not(feature = "sdk-extension-bridge"))]
            {
                let _ = (state, handles, request);
                Err(framework(
                    "echo_agent::agent::react::ReactAgent::set_store",
                    "set_store requires the SDK extension bridge",
                ))
            }
        }
        "echo_agent::agent::ReactAgent::set_max_iterations"
        | "echo_agent::agent::react::ReactAgent::set_max_iterations" => {
            let authorities = session_authorities(state, request).await?;
            let max = match request.arguments.get(1) {
                Some(WireValue::U64(value)) => value
                    .to_u64()
                    .ok_or_else(|| invalid("max_iterations value is invalid"))
                    .and_then(|value| {
                        usize::try_from(value)
                            .map_err(|_| invalid("max_iterations exceeds platform usize"))
                    })?,
                Some(_) => return Err(invalid("set_max_iterations requires a U64 value")),
                None => return Err(invalid("set_max_iterations requires a U64 value")),
            };
            if request.arguments.len() != 2 {
                return Err(invalid("set_max_iterations accepts [Session handle, U64]"));
            }
            authorities
                .agent_handle
                .write(|agent| agent.set_max_iterations(max))
                .await
                .map_err(|error| framework(&request.operation, error.to_string()))?;
            snapshot_value(request, serde_json::Value::Null)
        }
        "echo_agent::agent::react::ReactAgent::force_checkpoint" => {
            let authorities = session_authorities(state, request).await?;
            if request.arguments.len() != 1 {
                return Err(invalid(
                    "force_checkpoint accepts exactly one Session handle",
                ));
            }
            authorities
                .agent_handle
                .read_async(|agent| Box::pin(agent.force_checkpoint()))
                .await
                .map_err(|error| framework(&request.operation, error.to_string()))?;
            snapshot_value(request, serde_json::Value::Null)
        }
        "echo_agent::agent::react::ReactAgent::resume_from_state_store" => {
            let authorities = session_authorities(state, request).await?;
            if request.arguments.len() != 1 {
                return Err(invalid(
                    "resume_from_state_store accepts exactly one Session handle",
                ));
            }
            let checkpoint = authorities
                .agent_handle
                .read_async(|agent| Box::pin(agent.resume_from_state_store()))
                .await
                .map_err(|error| framework(&request.operation, error.to_string()))?;
            snapshot_value(
                request,
                serde_json::to_value(checkpoint)
                    .map_err(|error| framework(&request.operation, error.to_string()))?,
            )
        }
        "echo_agent::agent::react::ReactAgent::rollback" => {
            let authorities = session_authorities(state, request).await?;
            let steps = match request.arguments.get(1) {
                Some(WireValue::U64(value)) => value
                    .to_u64()
                    .ok_or_else(|| invalid("rollback steps are invalid"))
                    .and_then(|value| {
                        usize::try_from(value).map_err(|_| invalid("rollback steps exceed usize"))
                    })?,
                _ => return Err(invalid("rollback requires a U64 steps argument")),
            };
            if request.arguments.len() != 2 {
                return Err(invalid("rollback accepts [Session handle, U64]"));
            }
            let snapshot = authorities
                .agent_handle
                .read_async(|agent| Box::pin(agent.rollback(steps)))
                .await;
            snapshot_value(
                request,
                serde_json::to_value(snapshot)
                    .map_err(|error| framework(&request.operation, error.to_string()))?,
            )
        }
        "echo_agent::agent::react::ReactAgent::rollback_to" => {
            let authorities = session_authorities(state, request).await?;
            let snapshot_id = match request.arguments.get(1) {
                Some(WireValue::String(value)) if !value.trim().is_empty() => value.clone(),
                _ => return Err(invalid("rollback_to requires a snapshot id")),
            };
            if request.arguments.len() != 2 {
                return Err(invalid("rollback_to accepts [Session handle, String]"));
            }
            let snapshot_id_for_call = std::sync::Arc::new(snapshot_id);
            let snapshot = authorities
                .agent_handle
                .read_async(move |agent| {
                    let snapshot_id = snapshot_id_for_call.clone();
                    Box::pin(async move { agent.rollback_to(&snapshot_id).await })
                })
                .await;
            snapshot_value(
                request,
                serde_json::to_value(snapshot)
                    .map_err(|error| framework(&request.operation, error.to_string()))?,
            )
        }
        "echo_agent::agent::react::ReactAgent::shutdown" => {
            let authorities = session_authorities(state, request).await?;
            if request.arguments.len() != 1 {
                return Err(invalid("shutdown accepts exactly one Session handle"));
            }
            authorities
                .agent_handle
                .read_async(|agent| Box::pin(agent.shutdown()))
                .await;
            snapshot_value(request, serde_json::Value::Null)
        }
        "echo_agent::agent::react::ReactAgent::force_compress_context" => {
            let authorities = session_authorities(state, request).await?;
            if request.arguments.len() != 1 {
                return Err(invalid(
                    "force_compress_context accepts exactly one Session handle",
                ));
            }
            let (stats, checkpoint) = authorities
                .agent_handle
                .read_async(|agent| Box::pin(agent.force_compress_context()))
                .await
                .map_err(|error| framework(&request.operation, error.to_string()))?;
            snapshot_value(
                request,
                serde_json::json!({
                    "stats": {
                        "before_count": stats.before_count,
                        "after_count": stats.after_count,
                        "evicted": stats.evicted,
                        "before_tokens": stats.before_tokens,
                        "after_tokens": stats.after_tokens,
                    },
                    "checkpoint": serde_json::to_value(checkpoint)
                        .map_err(|error| framework(&request.operation, error.to_string()))?,
                }),
            )
        }
        #[cfg(feature = "sdk-extension-bridge")]
        "echo_agent::agent::react::ReactAgent::set_compressor" => {
            let authorities = session_authorities(state, request).await?;
            if request.arguments.len() != 2 {
                return Err(invalid(
                    "set_compressor accepts [Session, ContextCompressor extension]",
                ));
            }
            let session = match request.arguments.first() {
                Some(WireValue::Handle(handle)) if handle.kind == HandleKind::Session => handle,
                _ => return Err(invalid("set_compressor requires a Session handle")),
            };
            let extension = match request.arguments.get(1) {
                Some(WireValue::Handle(handle)) if handle.kind == HandleKind::Extension => {
                    handle.clone()
                }
                _ => {
                    return Err(invalid(
                        "set_compressor requires a ContextCompressor extension handle",
                    ));
                }
            };
            let session_id = state.handles.session(session)?.acp_session_id.clone();
            let proxy =
                crate::core_profile::extension_bridge::ExtensionContextCompressorProxy::new(
                    state.extension_bridge.clone(),
                    extension,
                    session_id,
                )
                .ok_or_else(|| invalid("extension is not a ContextCompressor"))?;
            authorities
                .agent_handle
                .read_async(move |agent| Box::pin(agent.set_compressor(proxy)))
                .await;
            Ok(WireValue::Null)
        }
        #[cfg(feature = "sdk-extension-bridge")]
        "echo_agent::agent::react::ReactAgent::set_intent_router" => {
            let authorities = session_authorities(state, request).await?;
            if !(2..=4).contains(&request.arguments.len()) {
                return Err(invalid(
                    "set_intent_router accepts [Session, IntentClassifier extension, config?, available_skills?]",
                ));
            }
            let session = match request.arguments.first() {
                Some(WireValue::Handle(handle)) if handle.kind == HandleKind::Session => handle,
                _ => return Err(invalid("set_intent_router requires a Session handle")),
            };
            let extension = match request.arguments.get(1) {
                Some(WireValue::Handle(handle)) if handle.kind == HandleKind::Extension => {
                    handle.clone()
                }
                _ => return Err(invalid("set_intent_router requires an Extension handle")),
            };
            let config = match request.arguments.get(2) {
                None | Some(WireValue::Null) => echo_agent::intent::IntentRouterConfig::default(),
                Some(value) => serde_json::from_value(
                    value
                        .clone()
                        .into_json()
                        .map_err(|error| invalid(error.to_string()))?,
                )
                .map_err(|error| invalid(format!("intent router config is malformed: {error}")))?,
            };
            let available_skills = match request.arguments.get(3) {
                None | Some(WireValue::Null) => None,
                Some(WireValue::List(values)) => Some(
                    values
                        .iter()
                        .map(|value| match value {
                            WireValue::String(value) => Ok(value.clone()),
                            _ => Err(invalid("available skill names must be strings")),
                        })
                        .collect::<Result<Vec<_>, EchoSdkError>>()?,
                ),
                _ => return Err(invalid("available_skills must be a List or null")),
            };
            let session_id = state.handles.session(session)?.acp_session_id.clone();
            let proxy = crate::core_profile::extension_bridge::ExtensionAgentComponentProxy::new(
                state.extension_bridge.clone(),
                extension,
                session_id,
            )
            .filter(|proxy| proxy.component() == AgentComponentKindWire::IntentClassifier)
            .ok_or_else(|| invalid("extension is not an IntentClassifier"))?;
            let mut router = echo_agent::intent::IntentRouter::new(Box::new(proxy), config);
            if let Some(skills) = available_skills {
                router = router.with_available_skills(skills);
            }
            authorities
                .agent_handle
                .write(move |agent| agent.set_intent_router(router))
                .await;
            Ok(WireValue::Null)
        }
        #[cfg(feature = "sdk-extension-bridge")]
        "echo_agent::agent::react::ReactAgent::set_skill_load_policy" => {
            let authorities = session_authorities(state, request).await?;
            let session_id = source_session_owner(state, request).await?;
            if request.arguments.len() != 2 {
                return Err(invalid(
                    "set_skill_load_policy accepts [Session, SkillLoadPolicy extension or null]",
                ));
            }
            let policy = match request.arguments.get(1) {
                Some(WireValue::Null) => None,
                Some(WireValue::Handle(extension)) if extension.kind == HandleKind::Extension => {
                    let proxy =
                        crate::core_profile::extension_bridge::ExtensionAgentComponentProxy::new(
                            state.extension_bridge.clone(),
                            extension.clone(),
                            session_id.clone(),
                        )
                        .filter(|proxy| {
                            proxy.component() == AgentComponentKindWire::SkillLoadPolicy
                        })
                        .ok_or_else(|| invalid("extension is not a SkillLoadPolicy component"))?;
                    Some(Arc::new(proxy) as Arc<dyn echo_agent::skills::external::SkillLoadPolicy>)
                }
                _ => {
                    return Err(invalid(
                        "set_skill_load_policy requires an Extension handle or null",
                    ));
                }
            };
            authorities
                .agent_handle
                .write(move |agent| agent.set_skill_load_policy(policy))
                .await;
            Ok(WireValue::Null)
        }
        #[cfg(feature = "sdk-extension-bridge")]
        "echo_agent::agent::react::ReactAgent::set_audit_logger"
        | "echo_agent::agent::react::ReactAgent::set_conversation_store"
        | "echo_agent::agent::react::ReactAgent::set_memory_trigger_sink"
        | "echo_agent::agent::react::ReactAgent::set_memory_promoter"
        | "echo_agent::agent::react::ReactAgent::set_pre_model_context_projector"
        | "echo_agent::agent::react::ReactAgent::set_run_store"
        | "echo_agent::agent::react::ReactAgent::set_sandbox_executor"
        | "echo_agent::agent::react::ReactAgent::set_state_store" => {
            let authorities = session_authorities(state, request).await?;
            if request.arguments.len() != 2 {
                return Err(invalid(
                    "Agent component setter accepts [Session, AgentComponent extension]",
                ));
            }
            let session = match request.arguments.first() {
                Some(WireValue::Handle(handle)) if handle.kind == HandleKind::Session => handle,
                _ => return Err(invalid("Agent component setter requires a Session handle")),
            };
            let extension = match request.arguments.get(1) {
                Some(WireValue::Handle(handle)) if handle.kind == HandleKind::Extension => {
                    handle.clone()
                }
                _ => {
                    return Err(invalid(
                        "Agent component setter requires an Extension handle",
                    ));
                }
            };
            let expected = match request.operation.as_str() {
                "echo_agent::agent::react::ReactAgent::set_audit_logger" => {
                    AgentComponentKindWire::AuditLogger
                }
                "echo_agent::agent::react::ReactAgent::set_conversation_store" => {
                    AgentComponentKindWire::ConversationStore
                }
                "echo_agent::agent::react::ReactAgent::set_memory_trigger_sink" => {
                    AgentComponentKindWire::MemoryTriggerSink
                }
                "echo_agent::agent::react::ReactAgent::set_memory_promoter" => {
                    AgentComponentKindWire::MemoryPromoter
                }
                "echo_agent::agent::react::ReactAgent::set_pre_model_context_projector" => {
                    AgentComponentKindWire::ContextProjector
                }
                "echo_agent::agent::react::ReactAgent::set_run_store" => {
                    AgentComponentKindWire::RunStore
                }
                "echo_agent::agent::react::ReactAgent::set_sandbox_executor" => {
                    AgentComponentKindWire::SandboxExecutor
                }
                _ => AgentComponentKindWire::RuntimeStateStore,
            };
            let session_id = state.handles.session(session)?.acp_session_id.clone();
            let proxy = crate::core_profile::extension_bridge::ExtensionAgentComponentProxy::new(
                state.extension_bridge.clone(),
                extension,
                session_id,
            )
            .filter(|proxy| proxy.component() == expected)
            .ok_or_else(|| invalid("extension component does not match the Agent setter"))?;
            if expected == AgentComponentKindWire::MemoryPromoter {
                authorities
                    .agent_handle
                    .read_async(move |agent| Box::pin(agent.set_memory_promoter(Arc::new(proxy))))
                    .await;
                return Ok(WireValue::Null);
            }
            authorities
                .agent_handle
                .write(move |agent| match expected {
                    AgentComponentKindWire::AuditLogger => agent.set_audit_logger(Arc::new(proxy)),
                    AgentComponentKindWire::ConversationStore => {
                        agent.set_conversation_store(Arc::new(proxy));
                    }
                    AgentComponentKindWire::MemoryTriggerSink => {
                        agent.set_memory_trigger_sink(Some(Arc::new(proxy)));
                    }
                    AgentComponentKindWire::ContextProjector => {
                        agent.set_pre_model_context_projector(Some(Arc::new(proxy)));
                    }
                    AgentComponentKindWire::RunStore => agent.set_run_store(Arc::new(proxy)),
                    AgentComponentKindWire::SandboxExecutor => {
                        agent.set_sandbox_executor(Arc::new(proxy));
                    }
                    AgentComponentKindWire::RuntimeStateStore => {
                        agent.set_state_store(Arc::new(proxy));
                    }
                    AgentComponentKindWire::Guard => {
                        agent.set_guard_manager(echo_agent::guard::GuardManager::from_guards(vec![
                            Arc::new(proxy),
                        ]))
                    }
                    AgentComponentKindWire::SearchProvider
                    | AgentComponentKindWire::WorkflowCheckpointStore
                    | AgentComponentKindWire::RevisionedTaskStore
                    | AgentComponentKindWire::McpTransport
                    | AgentComponentKindWire::Embedder
                    | AgentComponentKindWire::MemoryPromoter
                    | AgentComponentKindWire::Workflow
                    | AgentComponentKindWire::IntentClassifier
                    | AgentComponentKindWire::SkillLoadPolicy => drop(proxy),
                })
                .await;
            Ok(WireValue::Null)
        }
        "echo_agent::agent::react::ReactAgent::force_compress_with_focus_and_hooks" => {
            let authorities = session_authorities(state, request).await?;
            if request.arguments.len() != 4 {
                return Err(invalid(
                    "force_compress_with_focus_and_hooks accepts [Session, focus, window, matcher]",
                ));
            }
            let focus = std::sync::Arc::new(string_argument(request, 1, "focus instructions")?);
            let window = usize_argument(request, 2)?;
            let matcher = std::sync::Arc::new(string_argument(request, 3, "hook matcher")?);
            let (stats, checkpoint) = authorities
                .agent_handle
                .read_async(move |agent| {
                    let focus = focus.clone();
                    let matcher = matcher.clone();
                    Box::pin(async move {
                        agent
                            .force_compress_with_focus_and_hooks(&focus, window, &matcher)
                            .await
                    })
                })
                .await
                .map_err(|error| framework(&request.operation, error.to_string()))?;
            snapshot_value(
                request,
                serde_json::json!({
                    "stats": {
                        "before_count": stats.before_count,
                        "after_count": stats.after_count,
                        "evicted": stats.evicted,
                        "before_tokens": stats.before_tokens,
                        "after_tokens": stats.after_tokens,
                    },
                    "checkpoint": serde_json::to_value(checkpoint)
                        .map_err(|error| framework(&request.operation, error.to_string()))?,
                }),
            )
        }
        "echo_agent::agent::react::ReactAgent::load_mcp_config" => {
            #[cfg(feature = "framework-mcp")]
            {
                let authorities = session_authorities(state, request).await?;
                let session = match request.arguments.first() {
                    Some(WireValue::Handle(handle)) if handle.kind == HandleKind::Session => handle,
                    _ => return Err(invalid("load_mcp_config requires a Session handle")),
                };
                if request.arguments.len() != 2 {
                    return Err(invalid(
                        "load_mcp_config accepts [Session handle, ConfigFile]",
                    ));
                }
                let config_value = json_argument(request, 1, "MCP config file")?;
                let config: echo_agent::mcp::McpConfigFile = serde_json::from_value(config_value)
                    .map_err(|error| {
                    invalid(format!("MCP config file is malformed: {error}"))
                })?;
                let clients = authorities
                    .agent_handle
                    .write_async(move |agent| Box::pin(agent.load_mcp_config(config)))
                    .await
                    .map_err(|error| framework(&request.operation, error.to_string()))?;
                let mut resources = Vec::with_capacity(clients.len());
                for client in clients {
                    resources.push(
                        register_mcp_client_resource(state, session, client, &request.operation)
                            .await?,
                    );
                }
                snapshot_value(request, serde_json::json!({"clients": resources}))
            }
            #[cfg(not(feature = "framework-mcp"))]
            {
                Err(framework(
                    &request.operation,
                    "load_mcp_config requires the framework-mcp feature",
                ))
            }
        }
        "echo_agent::agent::react::ReactAgent::load_mcp_from_file" => {
            #[cfg(feature = "framework-mcp")]
            {
                let authorities = session_authorities(state, request).await?;
                let session = match request.arguments.first() {
                    Some(WireValue::Handle(handle)) if handle.kind == HandleKind::Session => handle,
                    _ => return Err(invalid("load_mcp_from_file requires a Session handle")),
                };
                let path = match request.arguments.get(1) {
                    Some(WireValue::Path(path)) => wire::path_from_wire(path)
                        .map_err(|error| invalid(format!("MCP config path is invalid: {error}")))?,
                    _ => return Err(invalid("load_mcp_from_file requires a Path")),
                };
                if request.arguments.len() != 2 {
                    return Err(invalid("load_mcp_from_file accepts [Session handle, Path]"));
                }
                let clients = authorities
                    .agent_handle
                    .write_async(move |agent| Box::pin(agent.load_mcp_from_file(path)))
                    .await
                    .map_err(|error| framework(&request.operation, error.to_string()))?;
                let mut resources = Vec::with_capacity(clients.len());
                for client in clients {
                    resources.push(
                        register_mcp_client_resource(state, session, client, &request.operation)
                            .await?,
                    );
                }
                snapshot_value(request, serde_json::json!({"clients": resources}))
            }
            #[cfg(not(feature = "framework-mcp"))]
            {
                Err(framework(
                    &request.operation,
                    "load_mcp_from_file requires the framework-mcp feature",
                ))
            }
        }
        "echo_agent::agent::react::ReactAgent::reconcile_mcp_entry" => {
            #[cfg(feature = "framework-mcp")]
            {
                let authorities = session_authorities(state, request).await?;
                let session = match request.arguments.first() {
                    Some(WireValue::Handle(handle)) if handle.kind == HandleKind::Session => handle,
                    _ => return Err(invalid("reconcile_mcp_entry requires a Session handle")),
                };
                let name = match request.arguments.get(1) {
                    Some(WireValue::String(value)) if !value.trim().is_empty() => value.clone(),
                    _ => return Err(invalid("reconcile_mcp_entry requires a server name")),
                };
                let entry = optional_json_argument(request, 2, "MCP server entry")?
                    .map(serde_json::from_value)
                    .transpose()
                    .map_err(|error| invalid(format!("MCP server entry is malformed: {error}")))?;
                if request.arguments.len() != 3 {
                    return Err(invalid(
                        "reconcile_mcp_entry accepts [Session handle, name, Entry|null]",
                    ));
                }
                let name_for_call = name.clone();
                let change = authorities
                    .agent_handle
                    .write_async(move |agent| {
                        Box::pin(
                            async move { agent.reconcile_mcp_entry(&name_for_call, entry).await },
                        )
                    })
                    .await
                    .map_err(|error| framework(&request.operation, error.to_string()))?;
                let name_for_read = name.clone();
                let client = authorities
                    .agent_handle
                    .read_async(move |agent| {
                        Box::pin(async move { agent.mcp_client(&name_for_read) })
                    })
                    .await;
                let client_resource = match client {
                    Some(client) => Some(
                        register_mcp_client_resource(state, session, client, &request.operation)
                            .await?,
                    ),
                    None => None,
                };
                snapshot_value(
                    request,
                    serde_json::json!({
                        "change": format!("{change:?}"),
                        "client": client_resource,
                    }),
                )
            }
            #[cfg(not(feature = "framework-mcp"))]
            {
                Err(framework(
                    &request.operation,
                    "reconcile_mcp_entry requires the framework-mcp feature",
                ))
            }
        }
        "echo_agent::agent::react::ReactAgent::disconnect_mcp" => {
            #[cfg(feature = "framework-mcp")]
            {
                let authorities = session_authorities(state, request).await?;
                let session = match request.arguments.first() {
                    Some(WireValue::Handle(handle)) if handle.kind == HandleKind::Session => handle,
                    _ => return Err(invalid("disconnect_mcp requires a Session handle")),
                };
                let owner = state.handles.session(session)?.acp_session_id.clone();
                let name = match request.arguments.get(1) {
                    Some(WireValue::String(value)) if !value.trim().is_empty() => value.clone(),
                    _ => return Err(invalid("disconnect_mcp requires a server name")),
                };
                if request.arguments.len() != 2 {
                    return Err(invalid("disconnect_mcp accepts [Session handle, String]"));
                }
                let name = std::sync::Arc::new(name);
                let name_for_cleanup = name.clone();
                let disconnected = authorities
                    .agent_handle
                    .write_async(move |agent| {
                        let name = name.clone();
                        Box::pin(async move { agent.disconnect_mcp(&name).await })
                    })
                    .await;
                if disconnected {
                    let clients = {
                        let clients = state
                            .facade
                            .integrations
                            .mcp_clients
                            .lock()
                            .unwrap_or_else(|error| error.into_inner());
                        clients
                            .iter()
                            .map(|(id, client)| (id.clone(), client.server_name().to_string()))
                            .collect::<Vec<_>>()
                    };
                    let resource_ids = clients
                        .into_iter()
                        .filter(|(id, client_name)| {
                            client_name == name_for_cleanup.as_str()
                                && state
                                    .handles
                                    .facade_resource(
                                        &WireHandle {
                                            id: id.clone(),
                                            generation: WireU64::from_u64(
                                                state.handles.generation(),
                                            ),
                                            kind: HandleKind::FacadeResource,
                                        },
                                        &request.operation,
                                    )
                                    .ok()
                                    .and_then(|record| record.owner_session.clone())
                                    .as_deref()
                                    == Some(owner.as_str())
                        })
                        .map(|(id, _)| id)
                        .collect::<Vec<_>>();
                    for id in resource_ids {
                        state
                            .facade
                            .integrations
                            .mcp_clients
                            .lock()
                            .unwrap_or_else(|error| error.into_inner())
                            .remove(&id);
                        let handle = WireHandle {
                            id,
                            generation: WireU64::from_u64(state.handles.generation()),
                            kind: HandleKind::FacadeResource,
                        };
                        let _ = state
                            .handles
                            .close_facade_resource(&handle, &request.operation);
                    }
                }
                snapshot_value(request, serde_json::Value::Bool(disconnected))
            }
            #[cfg(not(feature = "framework-mcp"))]
            {
                Err(framework(
                    &request.operation,
                    "disconnect_mcp requires the framework-mcp feature",
                ))
            }
        }
        "echo_agent::agent::react::ReactAgent::connect_mcp_from_json" => {
            #[cfg(feature = "framework-mcp")]
            {
                let authorities = session_authorities(state, request).await?;
                let session = match request.arguments.first() {
                    Some(WireValue::Handle(handle)) if handle.kind == HandleKind::Session => handle,
                    _ => return Err(invalid("connect_mcp_from_json requires a Session handle")),
                };
                let name = match request.arguments.get(1) {
                    Some(WireValue::String(value)) if !value.trim().is_empty() => value.clone(),
                    _ => return Err(invalid("connect_mcp_from_json requires a server name")),
                };
                let config = match request.arguments.get(2) {
                    Some(WireValue::String(value)) => value.clone(),
                    _ => {
                        return Err(invalid(
                            "connect_mcp_from_json requires a JSON config string",
                        ));
                    }
                };
                if request.arguments.len() != 3 {
                    return Err(invalid(
                        "connect_mcp_from_json accepts [Session handle, name, JSON config]",
                    ));
                }
                let client = authorities
                    .agent_handle
                    .write_async(move |agent| {
                        let name = name.clone();
                        let config = config.clone();
                        Box::pin(async move { agent.connect_mcp_from_json(&name, &config).await })
                    })
                    .await
                    .map_err(|error| framework(&request.operation, error.to_string()))?;
                snapshot_value(
                    request,
                    register_mcp_client_resource(state, session, client, METHOD).await?,
                )
            }
            #[cfg(not(feature = "framework-mcp"))]
            {
                Err(framework(
                    &request.operation,
                    "connect_mcp_from_json requires the framework-mcp feature",
                ))
            }
        }
        "echo_agent::agent::react::ReactAgent::mcp_client" => {
            #[cfg(feature = "framework-mcp")]
            {
                // Resolve the receiver and Session together before looking up
                // the resource.  A client from another Agent/Session must not
                // become addressable merely because its server name matches.
                let _authorities = session_authorities(state, request).await?;
                let session = match request.arguments.first() {
                    Some(WireValue::Handle(handle)) if handle.kind == HandleKind::Session => handle,
                    _ => return Err(invalid("mcp_client requires a Session handle")),
                };
                let owner = state.handles.session(session)?.acp_session_id.clone();
                let name = match request.arguments.get(1) {
                    Some(WireValue::String(value)) if !value.trim().is_empty() => value,
                    _ => return Err(invalid("mcp_client requires a server name")),
                };
                if request.arguments.len() != 2 {
                    return Err(invalid("mcp_client accepts [Session handle, name]"));
                }
                let clients = {
                    let clients = state
                        .facade
                        .integrations
                        .mcp_clients
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    clients
                        .iter()
                        .map(|(id, client)| (id.clone(), client.server_name().to_string()))
                        .collect::<Vec<_>>()
                };
                let id = clients
                    .into_iter()
                    .find(|(id, client_name)| {
                        client_name == name
                            && state
                                .handles
                                .facade_resource(
                                    &WireHandle {
                                        id: id.clone(),
                                        generation: WireU64::from_u64(state.handles.generation()),
                                        kind: HandleKind::FacadeResource,
                                    },
                                    &request.operation,
                                )
                                .ok()
                                .and_then(|record| record.owner_session.clone())
                                .as_deref()
                                == Some(owner.as_str())
                    })
                    .map(|(id, _)| id);
                let value = id.and_then(|id| {
                    let handle = WireHandle {
                        id,
                        generation: WireU64::from_u64(state.handles.generation()),
                        kind: HandleKind::FacadeResource,
                    };
                    let record = state
                        .handles
                        .facade_resource(&handle, &request.operation)
                        .ok()?;
                    if record.owner_session.as_deref() != Some(owner.as_str()) {
                        return None;
                    }
                    Some(serde_json::json!(handle))
                });
                snapshot_value(request, value.unwrap_or(serde_json::Value::Null))
            }
            #[cfg(not(feature = "framework-mcp"))]
            {
                Err(framework(
                    &request.operation,
                    "mcp_client requires the framework-mcp feature",
                ))
            }
        }
        "echo_agent::agent::react::ReactAgent::activate_skill" => {
            let authorities = session_authorities(state, request).await?;
            let name = match request.arguments.get(1) {
                Some(WireValue::String(value)) if !value.trim().is_empty() => value.clone(),
                _ => return Err(invalid("activate_skill requires a skill name")),
            };
            if request.arguments.len() != 2 {
                return Err(invalid("activate_skill accepts [Session handle, String]"));
            }
            authorities
                .agent_handle
                .read_async(move |agent| {
                    let name = name.clone();
                    Box::pin(async move { agent.activate_skill(&name).await })
                })
                .await
                .map_err(|error| framework(&request.operation, error.to_string()))?;
            snapshot_value(request, serde_json::Value::Null)
        }
        "echo_agent::agent::react::ReactAgent::has_skill" => {
            let authorities = session_authorities(state, request).await?;
            let name = match request.arguments.get(1) {
                Some(WireValue::String(value)) if !value.trim().is_empty() => value,
                _ => return Err(invalid("has_skill requires a skill name")),
            };
            if request.arguments.len() != 2 {
                return Err(invalid("has_skill accepts [Session handle, String]"));
            }
            let name = name.clone();
            let present = authorities
                .agent_handle
                .read(|agent| agent.has_skill(&name))
                .await;
            snapshot_value(request, serde_json::Value::Bool(present))
        }
        "echo_agent::agent::react::ReactAgent::skill_count" => {
            let authorities = session_authorities(state, request).await?;
            if request.arguments.len() != 1 {
                return Err(invalid("skill_count accepts exactly one Session handle"));
            }
            let count = authorities
                .agent_handle
                .read(|agent| agent.skill_count())
                .await;
            let count = u64::try_from(count)
                .map_err(|_| framework(&request.operation, "skill count exceeds WireU64"))?;
            snapshot_value(
                request,
                serde_json::to_value(WireU64::from_u64(count))
                    .map_err(|error| framework(&request.operation, error.to_string()))?,
            )
        }
        "echo_agent::agent::react::ReactAgent::list_skills" => {
            let authorities = session_authorities(state, request).await?;
            if request.arguments.len() != 1 {
                return Err(invalid("list_skills accepts exactly one Session handle"));
            }
            let skills = authorities
                .agent_handle
                .read(|agent| agent.list_skills())
                .await;
            let skills = skills
                .into_iter()
                .map(|skill| {
                    serde_json::json!({
                        "name": skill.name,
                        "description": skill.description,
                        "tool_names": skill.tool_names,
                        "has_prompt_injection": skill.has_prompt_injection,
                    })
                })
                .collect::<Vec<_>>();
            snapshot_value(request, serde_json::Value::Array(skills))
        }
        "echo_agent::agent::react::ReactAgent::skill_descriptors" => {
            let authorities = session_authorities(state, request).await?;
            if request.arguments.len() != 1 {
                return Err(invalid(
                    "skill_descriptors accepts exactly one Session handle",
                ));
            }
            let descriptors = authorities
                .agent_handle
                .read(|agent| agent.skill_descriptors())
                .await;
            snapshot_value(
                request,
                serde_json::to_value(descriptors)
                    .map_err(|error| framework(&request.operation, error.to_string()))?,
            )
        }
        "echo_execution::skills::registry::SkillRegistry::activate"
        | "echo_agent::skills::SkillRegistry::activate"
        | "echo_core::skills::registry::SkillRegistry::activate" => {
            let authorities = session_authorities(state, request).await?;
            if request.arguments.len() != 2 {
                return Err(invalid("SkillRegistry::activate accepts [Session, name]"));
            }
            let name = string_argument(request, 1, "skill name")?;
            let content = authorities
                .agent_handle
                .read_async(move |agent| {
                    Box::pin(async move { agent.skill_registry().activate(&name).await })
                })
                .await
                .map_err(|error| framework(&request.operation, error.to_string()))?;
            skill_content_value(request, content)
        }
        "echo_execution::skills::registry::SkillRegistry::activate_with_args"
        | "echo_agent::skills::SkillRegistry::activate_with_args"
        | "echo_core::skills::registry::SkillRegistry::activate_with_args" => {
            let authorities = session_authorities(state, request).await?;
            if request.arguments.len() != 4 {
                return Err(invalid(
                    "SkillRegistry::activate_with_args accepts [Session, name, args, source]",
                ));
            }
            let name = string_argument(request, 1, "skill name")?;
            let args = wire_string_list_argument(request, 2, "skill arguments")?;
            let source = match request.arguments.get(3) {
                Some(WireValue::String(value)) if value.eq_ignore_ascii_case("local") => {
                    echo_agent::skills::external::SkillSource::Local
                }
                Some(WireValue::String(value)) if value.eq_ignore_ascii_case("mcp") => {
                    echo_agent::skills::external::SkillSource::Mcp
                }
                _ => return Err(invalid("skill source must be 'local' or 'mcp'")),
            };
            let content = authorities
                .agent_handle
                .read_async(move |agent| {
                    Box::pin(async move {
                        agent
                            .skill_registry()
                            .activate_with_args(&name, &args, source)
                            .await
                    })
                })
                .await
                .map_err(|error| framework(&request.operation, error.to_string()))?;
            skill_content_value(request, content)
        }
        "echo_execution::skills::registry::SkillRegistry::mark_activated"
        | "echo_agent::skills::SkillRegistry::mark_activated"
        | "echo_core::skills::registry::SkillRegistry::mark_activated" => {
            let authorities = session_authorities(state, request).await?;
            if request.arguments.len() != 2 {
                return Err(invalid(
                    "SkillRegistry::mark_activated accepts [Session, name]",
                ));
            }
            let name = string_argument(request, 1, "skill name")?;
            let activated = authorities
                .agent_handle
                .read(|agent| agent.skill_registry().mark_activated(&name))
                .await;
            Ok(WireValue::Bool(activated))
        }
        "echo_execution::skills::registry::SkillRegistry::reset_activation_state"
        | "echo_agent::skills::SkillRegistry::reset_activation_state"
        | "echo_core::skills::registry::SkillRegistry::reset_activation_state" => {
            let authorities = session_authorities(state, request).await?;
            if request.arguments.len() != 1 {
                return Err(invalid(
                    "SkillRegistry::reset_activation_state accepts [Session]",
                ));
            }
            authorities
                .agent_handle
                .read(|agent| agent.skill_registry().reset_activation_state())
                .await;
            Ok(WireValue::Null)
        }
        "echo_execution::skills::registry::SkillRegistry::record_code_skill"
        | "echo_agent::skills::SkillRegistry::record_code_skill"
        | "echo_core::skills::registry::SkillRegistry::record_code_skill" => {
            let authorities = session_authorities(state, request).await?;
            if request.arguments.len() != 5 {
                return Err(invalid(
                    "SkillRegistry::record_code_skill accepts [Session, name, description, tool_names, has_prompt_injection]",
                ));
            }
            let info = echo_agent::skills::SkillInfo {
                name: string_argument(request, 1, "skill name")?,
                description: string_argument(request, 2, "skill description")?,
                tool_names: wire_string_list_argument(request, 3, "tool names")?,
                has_prompt_injection: match request.arguments.get(4) {
                    Some(WireValue::Bool(value)) => *value,
                    _ => return Err(invalid("has_prompt_injection must be Bool")),
                },
            };
            authorities
                .agent_handle
                .write(|agent| agent.skill_registry_mut().record_code_skill(info))
                .await;
            Ok(WireValue::Null)
        }
        "echo_execution::skills::registry::SkillRegistry::inject_methodology_baseline"
        | "echo_agent::skills::SkillRegistry::inject_methodology_baseline"
        | "echo_core::skills::registry::SkillRegistry::inject_methodology_baseline" => {
            let authorities = session_authorities(state, request).await?;
            if request.arguments.len() != 3 {
                return Err(invalid(
                    "SkillRegistry::inject_methodology_baseline accepts [Session, system_prompt, enabled_names]",
                ));
            }
            let mut prompt = match request.arguments.get(1) {
                Some(WireValue::String(value)) => value.clone(),
                _ => return Err(invalid("system_prompt must be String")),
            };
            let enabled = wire_string_list_argument(request, 2, "enabled baseline names")?;
            authorities
                .agent_handle
                .read(|agent| {
                    let enabled = enabled.iter().map(String::as_str).collect::<Vec<_>>();
                    agent
                        .skill_registry()
                        .inject_methodology_baseline(&mut prompt, &enabled);
                })
                .await;
            Ok(WireValue::String(prompt))
        }
        "echo_execution::skills::registry::SkillRegistry::register_descriptor"
        | "echo_agent::skills::SkillRegistry::register_descriptor"
        | "echo_core::skills::registry::SkillRegistry::register_descriptor" => {
            let authorities = session_authorities(state, request).await?;
            if request.arguments.len() != 5 {
                return Err(invalid(
                    "SkillRegistry::register_descriptor accepts [Session, descriptor, location, source|null, hooks|null]",
                ));
            }
            let mut descriptor: echo_agent::skills::external::SkillDescriptor =
                serde_json::from_value(json_argument(request, 1, "skill descriptor")?)
                    .map_err(|error| invalid(format!("skill descriptor is malformed: {error}")))?;
            descriptor.location = path_argument(request, 2)?;
            descriptor.source = match request.arguments.get(3) {
                Some(WireValue::Null) => None,
                Some(WireValue::String(value)) if !value.trim().is_empty() => Some(value.clone()),
                _ => return Err(invalid("descriptor source must be String or null")),
            };
            descriptor.hooks = optional_json_argument(request, 4, "skill hooks")?
                .map(serde_json::from_value)
                .transpose()
                .map_err(|error| invalid(format!("skill hooks are malformed: {error}")))?;
            authorities
                .agent_handle
                .write(|agent| agent.skill_registry_mut().register_descriptor(descriptor))
                .await;
            Ok(WireValue::Null)
        }
        "echo_execution::skills::registry::SkillRegistry::register_prepared"
        | "echo_agent::skills::SkillRegistry::register_prepared"
        | "echo_core::skills::registry::SkillRegistry::register_prepared" => {
            let authorities = session_authorities(state, request).await?;
            if request.arguments.len() != 4 {
                return Err(invalid(
                    "SkillRegistry::register_prepared accepts [Session, SKILL.md, location, source|null]",
                ));
            }
            let markdown = string_argument(request, 1, "SKILL.md document")?;
            let location = path_argument(request, 2)?;
            let mut document =
                echo_agent::skills::external::SkillDocument::parse_at(&markdown, location)
                    .map_err(|error| invalid(format!("SKILL.md is invalid: {error}")))?;
            match request.arguments.get(3) {
                Some(WireValue::Null) => {}
                Some(WireValue::String(value)) if !value.trim().is_empty() => {
                    document.set_registration_source(value.clone());
                }
                _ => return Err(invalid("prepared skill source must be String or null")),
            }
            authorities
                .agent_handle
                .write(|agent| agent.skill_registry_mut().register_prepared(document))
                .await;
            Ok(WireValue::Null)
        }
        "echo_execution::skills::registry::SkillRegistry::tag_source_with_variables"
        | "echo_agent::skills::SkillRegistry::tag_source_with_variables"
        | "echo_core::skills::registry::SkillRegistry::tag_source_with_variables" => {
            let authorities = session_authorities(state, request).await?;
            if request.arguments.len() != 4 {
                return Err(invalid(
                    "SkillRegistry::tag_source_with_variables accepts [Session, names, source, variables|null]",
                ));
            }
            let names = wire_string_list_argument(request, 1, "skill names")?;
            let source = string_argument(request, 2, "source")?;
            let variables = plugin_variables_argument(request, 3)?;
            authorities
                .agent_handle
                .write(|agent| {
                    agent.skill_registry_mut().tag_source_with_variables(
                        &names,
                        &source,
                        variables.as_ref(),
                    )
                })
                .await;
            Ok(WireValue::Null)
        }
        "echo_execution::skills::registry::SkillRegistry::activated_count"
        | "echo_agent::skills::SkillRegistry::activated_count"
        | "echo_core::skills::registry::SkillRegistry::activated_count" => {
            let authorities = session_authorities(state, request).await?;
            if request.arguments.len() != 1 {
                return Err(invalid(
                    "SkillRegistry::activated_count accepts exactly one Session handle",
                ));
            }
            let count = authorities
                .agent_handle
                .read(|agent| agent.skill_registry().activated_count())
                .await;
            let count = u64::try_from(count)
                .map_err(|_| framework(&request.operation, "activated count exceeds WireU64"))?;
            snapshot_value(request, serde_json::json!(WireU64::from_u64(count)))
        }
        "echo_execution::skills::registry::SkillRegistry::activated_names"
        | "echo_agent::skills::SkillRegistry::activated_names"
        | "echo_core::skills::registry::SkillRegistry::activated_names" => {
            let authorities = session_authorities(state, request).await?;
            if request.arguments.len() != 1 {
                return Err(invalid(
                    "SkillRegistry::activated_names accepts exactly one Session handle",
                ));
            }
            let names = authorities
                .agent_handle
                .read(|agent| agent.skill_registry().activated_names())
                .await;
            snapshot_value(
                request,
                serde_json::to_value(names)
                    .map_err(|error| framework(&request.operation, error.to_string()))?,
            )
        }
        "echo_execution::skills::registry::SkillRegistry::available_names"
        | "echo_agent::skills::SkillRegistry::available_names"
        | "echo_core::skills::registry::SkillRegistry::available_names" => {
            let authorities = session_authorities(state, request).await?;
            if request.arguments.len() != 1 {
                return Err(invalid(
                    "SkillRegistry::available_names accepts exactly one Session handle",
                ));
            }
            let names = authorities
                .agent_handle
                .read(|agent| agent.skill_registry().available_names())
                .await;
            snapshot_value(
                request,
                serde_json::to_value(names)
                    .map_err(|error| framework(&request.operation, error.to_string()))?,
            )
        }
        "echo_execution::skills::registry::SkillRegistry::count"
        | "echo_agent::skills::SkillRegistry::count"
        | "echo_core::skills::registry::SkillRegistry::count" => {
            let authorities = session_authorities(state, request).await?;
            if request.arguments.len() != 1 {
                return Err(invalid(
                    "SkillRegistry::count accepts exactly one Session handle",
                ));
            }
            let count = authorities
                .agent_handle
                .read(|agent| agent.skill_registry().count())
                .await;
            let count = u64::try_from(count)
                .map_err(|_| framework(&request.operation, "skill count exceeds WireU64"))?;
            snapshot_value(request, serde_json::json!(WireU64::from_u64(count)))
        }
        "echo_execution::skills::registry::SkillRegistry::descriptor_count"
        | "echo_agent::skills::SkillRegistry::descriptor_count"
        | "echo_core::skills::registry::SkillRegistry::descriptor_count" => {
            let authorities = session_authorities(state, request).await?;
            if request.arguments.len() != 1 {
                return Err(invalid(
                    "SkillRegistry::descriptor_count accepts exactly one Session handle",
                ));
            }
            let count = authorities
                .agent_handle
                .read(|agent| agent.skill_registry().descriptor_count())
                .await;
            let count = u64::try_from(count)
                .map_err(|_| framework(&request.operation, "descriptor count exceeds WireU64"))?;
            snapshot_value(request, serde_json::json!(WireU64::from_u64(count)))
        }
        "echo_execution::skills::registry::SkillRegistry::list"
        | "echo_agent::skills::SkillRegistry::list"
        | "echo_core::skills::registry::SkillRegistry::list" => {
            let authorities = session_authorities(state, request).await?;
            if request.arguments.len() != 1 {
                return Err(invalid(
                    "SkillRegistry::list accepts exactly one Session handle",
                ));
            }
            let skills = authorities
                .agent_handle
                .read(|agent| {
                    agent
                        .skill_registry()
                        .list()
                        .into_iter()
                        .map(skill_info_value)
                        .collect::<Vec<_>>()
                })
                .await;
            snapshot_value(request, serde_json::Value::Array(skills))
        }
        "echo_execution::skills::registry::SkillRegistry::list_descriptors"
        | "echo_agent::skills::SkillRegistry::list_descriptors"
        | "echo_core::skills::registry::SkillRegistry::list_descriptors" => {
            let authorities = session_authorities(state, request).await?;
            if request.arguments.len() != 1 {
                return Err(invalid(
                    "SkillRegistry::list_descriptors accepts exactly one Session handle",
                ));
            }
            let descriptors = authorities
                .agent_handle
                .read(|agent| {
                    agent
                        .skill_registry()
                        .list_descriptors()
                        .into_iter()
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .await;
            snapshot_value(
                request,
                serde_json::to_value(descriptors)
                    .map_err(|error| framework(&request.operation, error.to_string()))?,
            )
        }
        "echo_execution::skills::registry::SkillRegistry::list_code_skills"
        | "echo_agent::skills::SkillRegistry::list_code_skills"
        | "echo_core::skills::registry::SkillRegistry::list_code_skills" => {
            let authorities = session_authorities(state, request).await?;
            if request.arguments.len() != 1 {
                return Err(invalid(
                    "SkillRegistry::list_code_skills accepts exactly one Session handle",
                ));
            }
            let skills = authorities
                .agent_handle
                .read(|agent| {
                    agent
                        .skill_registry()
                        .list_code_skills()
                        .into_iter()
                        .map(skill_info_value)
                        .collect::<Vec<_>>()
                })
                .await;
            snapshot_value(request, serde_json::Value::Array(skills))
        }
        "echo_execution::skills::registry::SkillRegistry::get_descriptor"
        | "echo_agent::skills::SkillRegistry::get_descriptor"
        | "echo_core::skills::registry::SkillRegistry::get_descriptor" => {
            let authorities = session_authorities(state, request).await?;
            let name = match request.arguments.get(1) {
                Some(WireValue::String(value)) if !value.trim().is_empty() => value.clone(),
                _ => {
                    return Err(invalid(
                        "SkillRegistry::get_descriptor requires a skill name",
                    ));
                }
            };
            if request.arguments.len() != 2 {
                return Err(invalid(
                    "SkillRegistry::get_descriptor accepts [Session handle, String]",
                ));
            }
            let descriptor = authorities
                .agent_handle
                .read(|agent| agent.skill_registry().get_descriptor(&name).cloned())
                .await;
            snapshot_value(
                request,
                serde_json::to_value(descriptor)
                    .map_err(|error| framework(&request.operation, error.to_string()))?,
            )
        }
        "echo_execution::skills::registry::SkillRegistry::get_code_skill"
        | "echo_agent::skills::SkillRegistry::get_code_skill"
        | "echo_core::skills::registry::SkillRegistry::get_code_skill" => {
            let authorities = session_authorities(state, request).await?;
            let name = match request.arguments.get(1) {
                Some(WireValue::String(value)) if !value.trim().is_empty() => value.clone(),
                _ => {
                    return Err(invalid(
                        "SkillRegistry::get_code_skill requires a skill name",
                    ));
                }
            };
            if request.arguments.len() != 2 {
                return Err(invalid(
                    "SkillRegistry::get_code_skill accepts [Session handle, String]",
                ));
            }
            let skill = authorities
                .agent_handle
                .read(|agent| agent.skill_registry().get_code_skill(&name).cloned())
                .await;
            let skill = skill.as_ref().map(skill_info_value);
            snapshot_value(
                request,
                serde_json::to_value(skill)
                    .map_err(|error| framework(&request.operation, error.to_string()))?,
            )
        }
        "echo_execution::skills::registry::SkillRegistry::active_skill_allowed_tools"
        | "echo_agent::skills::SkillRegistry::active_skill_allowed_tools"
        | "echo_core::skills::registry::SkillRegistry::active_skill_allowed_tools" => {
            let authorities = session_authorities(state, request).await?;
            if request.arguments.len() != 1 {
                return Err(invalid(
                    "SkillRegistry::active_skill_allowed_tools accepts exactly one Session handle",
                ));
            }
            let allowed_tools = authorities
                .agent_handle
                .read(|agent| {
                    agent
                        .skill_registry()
                        .active_skill_allowed_tools()
                        .map(|tools| {
                            let mut tools = tools.into_iter().collect::<Vec<_>>();
                            tools.sort();
                            tools
                        })
                })
                .await;
            snapshot_value(
                request,
                serde_json::to_value(allowed_tools)
                    .map_err(|error| framework(&request.operation, error.to_string()))?,
            )
        }
        "echo_execution::skills::registry::SkillRegistry::catalog_prompt"
        | "echo_agent::skills::SkillRegistry::catalog_prompt"
        | "echo_core::skills::registry::SkillRegistry::catalog_prompt" => {
            let authorities = session_authorities(state, request).await?;
            if request.arguments.len() != 1 {
                return Err(invalid(
                    "SkillRegistry::catalog_prompt accepts exactly one Session handle",
                ));
            }
            let catalog = authorities
                .agent_handle
                .read(|agent| agent.skill_registry().catalog_prompt())
                .await;
            snapshot_value(
                request,
                serde_json::to_value(catalog)
                    .map_err(|error| framework(&request.operation, error.to_string()))?,
            )
        }
        "echo_execution::skills::registry::SkillRegistry::get_active_sandbox_policy"
        | "echo_agent::skills::SkillRegistry::get_active_sandbox_policy"
        | "echo_core::skills::registry::SkillRegistry::get_active_sandbox_policy" => {
            let authorities = session_authorities(state, request).await?;
            let name = match request.arguments.get(1) {
                Some(WireValue::String(value)) if !value.trim().is_empty() => value.clone(),
                _ => {
                    return Err(invalid(
                        "SkillRegistry::get_active_sandbox_policy requires a skill name",
                    ));
                }
            };
            if request.arguments.len() != 2 {
                return Err(invalid(
                    "SkillRegistry::get_active_sandbox_policy accepts [Session handle, String]",
                ));
            }
            let policy = authorities
                .agent_handle
                .read(|agent| agent.skill_registry().get_active_sandbox_policy(&name))
                .await;
            snapshot_value(
                request,
                serde_json::to_value(policy)
                    .map_err(|error| framework(&request.operation, error.to_string()))?,
            )
        }
        "echo_execution::skills::registry::SkillRegistry::get_dependency_tree"
        | "echo_agent::skills::SkillRegistry::get_dependency_tree"
        | "echo_core::skills::registry::SkillRegistry::get_dependency_tree" => {
            let authorities = session_authorities(state, request).await?;
            let name = match request.arguments.get(1) {
                Some(WireValue::String(value)) if !value.trim().is_empty() => value.clone(),
                _ => {
                    return Err(invalid(
                        "SkillRegistry::get_dependency_tree requires a skill name",
                    ));
                }
            };
            if request.arguments.len() != 2 {
                return Err(invalid(
                    "SkillRegistry::get_dependency_tree accepts [Session handle, String]",
                ));
            }
            let dependencies = authorities
                .agent_handle
                .read(|agent| agent.skill_registry().get_dependency_tree(&name))
                .await;
            snapshot_value(
                request,
                serde_json::to_value(dependencies)
                    .map_err(|error| framework(&request.operation, error.to_string()))?,
            )
        }
        "echo_execution::skills::registry::SkillRegistry::remove_descriptor"
        | "echo_agent::skills::SkillRegistry::remove_descriptor"
        | "echo_core::skills::registry::SkillRegistry::remove_descriptor" => {
            let authorities = session_authorities(state, request).await?;
            let name = match request.arguments.get(1) {
                Some(WireValue::String(value)) if !value.trim().is_empty() => value.clone(),
                _ => {
                    return Err(invalid(
                        "SkillRegistry::remove_descriptor requires a skill name",
                    ));
                }
            };
            if request.arguments.len() != 2 {
                return Err(invalid(
                    "SkillRegistry::remove_descriptor accepts [Session handle, String]",
                ));
            }
            let removed = authorities
                .agent_handle
                .write(|agent| agent.skill_registry_mut().remove_descriptor(&name))
                .await;
            snapshot_value(request, serde_json::Value::Bool(removed))
        }
        "echo_execution::skills::registry::SkillRegistry::tag_source"
        | "echo_agent::skills::SkillRegistry::tag_source"
        | "echo_core::skills::registry::SkillRegistry::tag_source" => {
            let authorities = session_authorities(state, request).await?;
            let names = match request.arguments.get(1) {
                Some(WireValue::List(values)) => values
                    .iter()
                    .map(|value| match value {
                        WireValue::String(name) => Ok(name.clone()),
                        _ => Err(invalid("SkillRegistry::tag_source names must be strings")),
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                _ => return Err(invalid("SkillRegistry::tag_source requires a string list")),
            };
            let source = match request.arguments.get(2) {
                Some(WireValue::String(value)) if !value.trim().is_empty() => value.clone(),
                _ => return Err(invalid("SkillRegistry::tag_source requires a source")),
            };
            if request.arguments.len() != 3 {
                return Err(invalid(
                    "SkillRegistry::tag_source accepts [Session handle, List, String]",
                ));
            }
            authorities
                .agent_handle
                .write(|agent| agent.skill_registry_mut().tag_source(&names, &source))
                .await;
            snapshot_value(request, serde_json::Value::Null)
        }
        "echo_execution::skills::registry::SkillRegistry::unregister_by_source"
        | "echo_agent::skills::SkillRegistry::unregister_by_source"
        | "echo_core::skills::registry::SkillRegistry::unregister_by_source" => {
            let authorities = session_authorities(state, request).await?;
            let source = match request.arguments.get(1) {
                Some(WireValue::String(value)) if !value.trim().is_empty() => value.clone(),
                _ => {
                    return Err(invalid(
                        "SkillRegistry::unregister_by_source requires a source",
                    ));
                }
            };
            if request.arguments.len() != 2 {
                return Err(invalid(
                    "SkillRegistry::unregister_by_source accepts [Session handle, String]",
                ));
            }
            let removed = authorities
                .agent_handle
                .write(|agent| agent.skill_registry_mut().unregister_by_source(&source))
                .await;
            let removed = u64::try_from(removed)
                .map_err(|_| framework(&request.operation, "removed count exceeds WireU64"))?;
            snapshot_value(request, serde_json::json!(WireU64::from_u64(removed)))
        }
        "echo_execution::skills::registry::SkillRegistry::unregister_names_by_source"
        | "echo_agent::skills::SkillRegistry::unregister_names_by_source"
        | "echo_core::skills::registry::SkillRegistry::unregister_names_by_source" => {
            let authorities = session_authorities(state, request).await?;
            let source = match request.arguments.get(1) {
                Some(WireValue::String(value)) if !value.trim().is_empty() => value.clone(),
                _ => {
                    return Err(invalid(
                        "SkillRegistry::unregister_names_by_source requires a source",
                    ));
                }
            };
            if request.arguments.len() != 2 {
                return Err(invalid(
                    "SkillRegistry::unregister_names_by_source accepts [Session handle, String]",
                ));
            }
            let removed = authorities
                .agent_handle
                .write(|agent| {
                    agent
                        .skill_registry_mut()
                        .unregister_names_by_source(&source)
                })
                .await;
            snapshot_value(
                request,
                serde_json::to_value(removed)
                    .map_err(|error| framework(&request.operation, error.to_string()))?,
            )
        }
        "echo_execution::skills::registry::SkillRegistry::has_code_skill"
        | "echo_agent::skills::SkillRegistry::has_code_skill"
        | "echo_core::skills::registry::SkillRegistry::has_code_skill"
        | "echo_execution::skills::registry::SkillRegistry::is_activated"
        | "echo_agent::skills::SkillRegistry::is_activated"
        | "echo_core::skills::registry::SkillRegistry::is_activated"
        | "echo_execution::skills::registry::SkillRegistry::is_installed"
        | "echo_agent::skills::SkillRegistry::is_installed"
        | "echo_core::skills::registry::SkillRegistry::is_installed" => {
            let authorities = session_authorities(state, request).await?;
            let name = match request.arguments.get(1) {
                Some(WireValue::String(value)) if !value.trim().is_empty() => value.clone(),
                _ => return Err(invalid("SkillRegistry query requires a skill name")),
            };
            if request.arguments.len() != 2 {
                return Err(invalid(
                    "SkillRegistry query accepts [Session handle, String]",
                ));
            }
            let present = authorities
                .agent_handle
                .read(|agent| match request.operation.rsplit_once("::") {
                    Some((_, "has_code_skill")) => agent.skill_registry().has_code_skill(&name),
                    Some((_, "is_activated")) => agent.skill_registry().is_activated(&name),
                    Some((_, "is_installed")) => agent.skill_registry().is_installed(&name),
                    _ => false,
                })
                .await;
            snapshot_value(request, serde_json::Value::Bool(present))
        }
        "echo_agent::agent::react::ReactAgent::discover_skills" => {
            let authorities = session_authorities(state, request).await?;
            let scopes = discovery_scopes_from_wire(
                request
                    .arguments
                    .get(1)
                    .ok_or_else(|| invalid("discover_skills requires a scope list"))?,
            )?;
            if request.arguments.len() != 2 {
                return Err(invalid("discover_skills accepts [Session handle, List]"));
            }
            let names = authorities
                .agent_handle
                .write_async(move |agent| {
                    Box::pin(async move { agent.discover_skills(&scopes).await })
                })
                .await
                .map_err(|error| framework(&request.operation, error.to_string()))?;
            snapshot_value(
                request,
                serde_json::to_value(names)
                    .map_err(|error| framework(&request.operation, error.to_string()))?,
            )
        }
        "echo_agent::agent::react::ReactAgent::reconcile_skill_load_policy" => {
            let authorities = session_authorities(state, request).await?;
            if request.arguments.len() != 1 {
                return Err(invalid(
                    "reconcile_skill_load_policy accepts [Session handle]",
                ));
            }
            let removed = authorities
                .agent_handle
                .write_async(|agent| Box::pin(agent.reconcile_skill_load_policy()))
                .await;
            snapshot_value(
                request,
                serde_json::to_value(removed)
                    .map_err(|error| framework(&request.operation, error.to_string()))?,
            )
        }
        "echo_agent::agent::react::ReactAgent::load_skills_from_dir"
        | "echo_agent::agent::react::ReactAgent::reload_skills_from_dir" => {
            let authorities = session_authorities(state, request).await?;
            let path = match request.arguments.get(1) {
                Some(WireValue::Path(path)) => wire::path_from_wire(path)
                    .map_err(|error| invalid(format!("skill directory is invalid: {error}")))?,
                _ => return Err(invalid("skill loader requires a Path directory")),
            };
            if request.arguments.len() != 2 {
                return Err(invalid("skill loader accepts [Session handle, Path]"));
            }
            let reload = request.operation.ends_with("reload_skills_from_dir");
            let names = authorities
                .agent_handle
                .write_async(move |agent| {
                    Box::pin(async move {
                        if reload {
                            agent.reload_skills_from_dir(path).await
                        } else {
                            agent.load_skills_from_dir(path).await
                        }
                    })
                })
                .await
                .map_err(|error| framework(&request.operation, error.to_string()))?;
            snapshot_value(
                request,
                serde_json::to_value(names)
                    .map_err(|error| framework(&request.operation, error.to_string()))?,
            )
        }
        "echo_agent::agent::react::ReactAgent::load_plugin_skills_from_dir" => {
            let authorities = session_authorities(state, request).await?;
            let path = match request.arguments.get(1) {
                Some(WireValue::Path(path)) => wire::path_from_wire(path).map_err(|error| {
                    invalid(format!("plugin skill directory is invalid: {error}"))
                })?,
                _ => return Err(invalid("load_plugin_skills_from_dir requires a Path")),
            };
            let source = match request.arguments.get(2) {
                Some(WireValue::String(value)) if !value.trim().is_empty() => value.clone(),
                _ => return Err(invalid("load_plugin_skills_from_dir requires a source")),
            };
            let variables = json_argument(request, 3, "plugin variables")?;
            let object = variables
                .as_object()
                .ok_or_else(|| invalid("plugin variables must be a record"))?;
            let path_field = |key: &str| -> Result<std::path::PathBuf, EchoSdkError> {
                object
                    .get(key)
                    .and_then(serde_json::Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .map(std::path::PathBuf::from)
                    .ok_or_else(|| invalid(format!("plugin variables require {key}")))
            };
            let user_config = object
                .get("user_config")
                .and_then(serde_json::Value::as_object)
                .map(|values| {
                    values
                        .iter()
                        .map(|(key, value)| {
                            value
                                .as_str()
                                .map(|value| (key.clone(), value.to_string()))
                                .ok_or_else(|| invalid("plugin user_config values must be strings"))
                        })
                        .collect::<Result<std::collections::HashMap<_, _>, _>>()
                })
                .transpose()?
                .unwrap_or_default();
            let plugin_variables = echo_agent::plugin::PluginVariables::new(
                path_field("plugin_root")?,
                path_field("plugin_data")?,
                path_field("project_dir")?,
            )
            .with_user_config(user_config);
            if request.arguments.len() != 4 {
                return Err(invalid(
                    "load_plugin_skills_from_dir accepts [Session handle, Path, source, variables]",
                ));
            }
            let names = authorities
                .agent_handle
                .write_async(move |agent| {
                    Box::pin(async move {
                        agent
                            .load_plugin_skills_from_dir(path, &source, &plugin_variables)
                            .await
                    })
                })
                .await
                .map_err(|error| framework(&request.operation, error.to_string()))?;
            snapshot_value(
                request,
                serde_json::to_value(names)
                    .map_err(|error| framework(&request.operation, error.to_string()))?,
            )
        }
        "echo_agent::agent::react::ReactAgent::unregister_skill_names" => {
            let authorities = session_authorities(state, request).await?;
            let names = match request.arguments.get(1) {
                Some(WireValue::List(values)) => values
                    .iter()
                    .map(|value| match value {
                        WireValue::String(name) => Ok(name.clone()),
                        _ => Err(invalid("skill names must be strings")),
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                _ => return Err(invalid("unregister_skill_names requires a string list")),
            };
            if request.arguments.len() != 2 {
                return Err(invalid(
                    "unregister_skill_names accepts [Session handle, List]",
                ));
            }
            let names = std::sync::Arc::new(names);
            let removed = authorities
                .agent_handle
                .write_async(move |agent| {
                    let names = names.clone();
                    Box::pin(async move { agent.unregister_skill_names(&names).await })
                })
                .await;
            snapshot_value(
                request,
                serde_json::to_value(removed)
                    .map_err(|error| framework(&request.operation, error.to_string()))?,
            )
        }
        "echo_agent::agent::react::ReactAgent::unregister_skills_by_source" => {
            let authorities = session_authorities(state, request).await?;
            let source = match request.arguments.get(1) {
                Some(WireValue::String(value)) if !value.trim().is_empty() => value.clone(),
                _ => return Err(invalid("unregister_skills_by_source requires a source")),
            };
            if request.arguments.len() != 2 {
                return Err(invalid(
                    "unregister_skills_by_source accepts [Session handle, String]",
                ));
            }
            let source = std::sync::Arc::new(source);
            let removed = authorities
                .agent_handle
                .write_async(move |agent| {
                    let source = source.clone();
                    Box::pin(async move { agent.unregister_skills_by_source(&source).await })
                })
                .await;
            snapshot_value(
                request,
                serde_json::to_value(removed)
                    .map_err(|error| framework(&request.operation, error.to_string()))?,
            )
        }
        "echo_agent::agent::react::ReactAgent::tag_skills_source" => {
            let authorities = session_authorities(state, request).await?;
            let names = match request.arguments.get(1) {
                Some(WireValue::List(values)) => values
                    .iter()
                    .map(|value| match value {
                        WireValue::String(name) => Ok(name.clone()),
                        _ => Err(invalid("skill names must be strings")),
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                _ => return Err(invalid("tag_skills_source requires a string list")),
            };
            let source = match request.arguments.get(2) {
                Some(WireValue::String(value)) if !value.trim().is_empty() => value.clone(),
                _ => return Err(invalid("tag_skills_source requires a source")),
            };
            if request.arguments.len() != 3 {
                return Err(invalid(
                    "tag_skills_source accepts [Session handle, List, String]",
                ));
            }
            let names = std::sync::Arc::new(names);
            let source = std::sync::Arc::new(source);
            authorities
                .agent_handle
                .write_async(move |agent| {
                    let names = names.clone();
                    let source = source.clone();
                    Box::pin(async move { agent.tag_skills_source(&names, &source).await })
                })
                .await;
            snapshot_value(request, serde_json::Value::Null)
        }
        "echo_core::agent::Agent::reset" | "echo_agent::agent::Agent::reset" => {
            let agent = session_agent(state, request).await?;
            if request.arguments.len() != 1 {
                return Err(invalid("Agent::reset accepts exactly one Session handle"));
            }
            agent.reset().await;
            snapshot_value(request, serde_json::Value::Null)
        }
        "echo_core::agent::Agent::set_system_prompt"
        | "echo_agent::agent::Agent::set_system_prompt" => {
            let agent = session_agent(state, request).await?;
            let prompt = match request.arguments.get(1) {
                Some(WireValue::String(value)) => value.clone(),
                Some(_) => return Err(invalid("set_system_prompt requires a string prompt")),
                None => return Err(invalid("set_system_prompt requires a prompt argument")),
            };
            if request.arguments.len() != 2 {
                return Err(invalid(
                    "set_system_prompt accepts [Session handle, String]",
                ));
            }
            agent.set_system_prompt(&prompt);
            snapshot_value(request, serde_json::Value::Null)
        }
        "echo_agent::agent::ReactAgent::set_system_prompt"
        | "echo_agent::agent::react::ReactAgent::set_system_prompt" => {
            let authorities = session_authorities(state, request).await?;
            let prompt = match request.arguments.get(1) {
                Some(WireValue::String(value)) => value.clone(),
                Some(_) => return Err(invalid("set_system_prompt requires a string prompt")),
                None => return Err(invalid("set_system_prompt requires a prompt argument")),
            };
            if request.arguments.len() != 2 {
                return Err(invalid(
                    "set_system_prompt accepts [Session handle, String]",
                ));
            }
            authorities
                .agent_handle
                .write_async(move |agent| {
                    Box::pin(async move { agent.set_system_prompt(prompt).await })
                })
                .await;
            snapshot_value(request, serde_json::Value::Null)
        }
        "echo_agent::agent::ReactAgent::replace_system_prompt"
        | "echo_agent::agent::react::ReactAgent::replace_system_prompt" => {
            let authorities = session_authorities(state, request).await?;
            let prompt = match request.arguments.get(1) {
                Some(WireValue::String(value)) => value.clone(),
                Some(_) => return Err(invalid("replace_system_prompt requires a string prompt")),
                None => return Err(invalid("replace_system_prompt requires a prompt argument")),
            };
            if request.arguments.len() != 2 {
                return Err(invalid(
                    "replace_system_prompt accepts [Session handle, String]",
                ));
            }
            authorities
                .agent_handle
                .write(|agent| agent.replace_system_prompt(prompt))
                .await;
            snapshot_value(request, serde_json::Value::Null)
        }
        "echo_core::agent::Agent::set_working_dir"
        | "echo_agent::agent::Agent::set_working_dir"
        | "echo_agent::agent::ReactAgent::set_working_dir"
        | "echo_agent::agent::react::ReactAgent::set_working_dir" => {
            let agent = session_agent(state, request).await?;
            let path = match request.arguments.get(1) {
                Some(WireValue::Path(path)) => {
                    Some(wire::path_from_wire(path).map_err(|error| {
                        invalid(format!("working directory is invalid: {error}"))
                    })?)
                }
                Some(WireValue::Null) => None,
                Some(_) => return Err(invalid("set_working_dir requires a Path or Null value")),
                None => return Err(invalid("set_working_dir requires a Path argument")),
            };
            if request.arguments.len() != 2 {
                return Err(invalid("set_working_dir accepts [Session handle, Path]"));
            }
            agent.set_working_dir(path);
            snapshot_value(request, serde_json::Value::Null)
        }
        "echo_core::agent::Agent::clear_working_dir"
        | "echo_agent::agent::Agent::clear_working_dir"
        | "echo_agent::agent::ReactAgent::clear_working_dir"
        | "echo_agent::agent::react::ReactAgent::clear_working_dir" => {
            let agent = session_agent(state, request).await?;
            if request.arguments.len() != 1 {
                return Err(invalid(
                    "clear_working_dir accepts exactly one Session handle",
                ));
            }
            agent.clear_working_dir();
            snapshot_value(request, serde_json::Value::Null)
        }
        "echo_core::agent::Agent::remove_tool"
        | "echo_agent::agent::Agent::remove_tool"
        | "echo_agent::agent::ReactAgent::remove_tool"
        | "echo_agent::agent::react::ReactAgent::remove_tool" => {
            let agent = session_agent(state, request).await?;
            let name = match request.arguments.get(1) {
                Some(WireValue::String(value)) if !value.trim().is_empty() => value,
                Some(_) => return Err(invalid("remove_tool requires a non-empty tool name")),
                None => return Err(invalid("remove_tool requires a tool name")),
            };
            if request.arguments.len() != 2 {
                return Err(invalid("remove_tool accepts [Session handle, String]"));
            }
            snapshot_value(request, serde_json::Value::Bool(agent.remove_tool(name)))
        }
        "echo_core::agent::Agent::register_tool" | "echo_agent::agent::Agent::register_tool" => {
            #[cfg(feature = "sdk-extension-bridge")]
            {
                let agent = session_agent(state, request).await?;
                let session_handle = match request.arguments.first() {
                    Some(WireValue::Handle(handle)) => handle,
                    _ => return Err(invalid("register_tool requires a Session handle")),
                };
                let extension = match request.arguments.get(1) {
                    Some(WireValue::Handle(handle)) if handle.kind == HandleKind::Extension => {
                        handle.clone()
                    }
                    Some(WireValue::Handle(_)) => {
                        return Err(invalid("register_tool requires an Extension handle"));
                    }
                    Some(_) => return Err(invalid("register_tool requires an Extension handle")),
                    None => return Err(invalid("register_tool requires a Tool extension handle")),
                };
                if request.arguments.len() != 2 {
                    return Err(invalid(
                        "register_tool accepts [Session handle, Extension handle]",
                    ));
                }
                let record = state.handles.extension(&extension)?;
                if record.kind != echo_sdk_protocol::methods::ExtensionKind::Tool {
                    return Err(invalid("register_tool requires a Tool extension handle"));
                }
                let session_record = state.handles.session(session_handle)?;
                let proxy = crate::core_profile::extension_bridge::ExtensionToolProxy::new(
                    state.extension_bridge.clone(),
                    extension,
                    session_record.acp_session_id.clone(),
                )
                .ok_or_else(|| {
                    framework(&request.operation, "Tool extension proxy is unavailable")
                })?;
                agent.register_tool(Box::new(proxy));
                snapshot_value(request, serde_json::Value::Null)
            }
            #[cfg(not(feature = "sdk-extension-bridge"))]
            {
                let _ = state;
                let _ = handles;
                let _ = request;
                Err(framework(
                    "echo_core::agent::Agent::register_tool",
                    "register_tool requires the SDK extension bridge",
                ))
            }
        }
        "echo_core::agent::Agent::delegate_to"
        | "echo_agent::agent::Agent::delegate_to"
        | "echo_agent::agent::react::ReactAgent::delegate_to_agent" => {
            let agent = session_agent(state, request).await?;
            let target = match request.arguments.get(1) {
                Some(WireValue::String(value)) if !value.trim().is_empty() => value,
                Some(_) => return Err(invalid("delegate_to requires a non-empty target")),
                None => return Err(invalid("delegate_to requires a target argument")),
            };
            let task = match request.arguments.get(2) {
                Some(WireValue::String(value)) if !value.trim().is_empty() => value,
                Some(_) => return Err(invalid("delegate_to requires a non-empty task")),
                None => return Err(invalid("delegate_to requires a task argument")),
            };
            if request.arguments.len() != 3 {
                return Err(invalid(
                    "delegate_to accepts [Session handle, String target, String task]",
                ));
            }
            let result = agent
                .delegate_to(target, task)
                .await
                .map_err(|error| framework(&request.operation, error.to_string()))?;
            snapshot_value(request, serde_json::Value::String(result))
        }
        "echo_agent::agent::react::ReactAgent::delegate_to_agent_with_cancel"
        | "echo_agent::agent::react::ReactAgent::delegate_to_agent_with_parent_and_cancel"
        | "echo_agent::agent::react::ReactAgent::delegate_to_agent_with_parent_context_and_cancel"
        | "echo_agent::agent::react::ReactAgent::delegate_to_agent_with_parent_context_cancel_and_tools"
        | "echo_agent::agent::react::ReactAgent::delegate_to_agent_with_prompt_payload"
        | "echo_agent::agent::react::ReactAgent::delegate_to_agent_attempt_with_prompt_payload"
        | "echo_agent::agent::react::ReactAgent::delegate_to_agent_with_parent_cancel_and_message"
        | "echo_agent::agent::react::ReactAgent::delegate_to_agent_with_parent_context_cancel_and_message"
        | "echo_agent::agent::react::ReactAgent::delegate_to_agent_with_parent_context_cancel_message_and_tools"
        | "echo_agent::agent::react::ReactAgent::delegate_to_agent_with_message_and_prompt_payload"
        | "echo_agent::agent::react::ReactAgent::delegate_to_agent_attempt_with_message_and_prompt_payload" =>
        {
            #[cfg(feature = "framework-subagent")]
            {
                let operation = request.operation.as_str();
                let short_cancel = operation.ends_with("::delegate_to_agent_with_cancel");
                let has_message = operation.contains("_message");
                let has_runtime_context =
                    operation.contains("parent_context") || operation.contains("prompt_payload");
                let has_allowed_tools =
                    operation.contains("and_tools") || operation.contains("prompt_payload");
                let has_prompt_payload = operation.contains("prompt_payload");
                let is_attempt = operation.contains("delegate_to_agent_attempt_");
                let parent_index = if has_message { 4 } else { 3 };
                let depth_index = if has_message { 5 } else { 4 };
                let runtime_index = if has_message { 6 } else { 5 };
                let tools_index = if has_message { 7 } else { 6 };
                let payload_index = if has_message { 8 } else { 7 };
                let context_index = if has_message { 9 } else { 8 };
                let identity_index = if has_message { 10 } else { 9 };
                let expected = 3usize
                    .saturating_add(usize::from(has_message))
                    .saturating_add(if short_cancel { 0 } else { 2 })
                    .saturating_add(usize::from(has_runtime_context))
                    .saturating_add(usize::from(has_allowed_tools))
                    .saturating_add(if has_prompt_payload { 2 } else { 0 })
                    .saturating_add(usize::from(is_attempt));
                if request.arguments.len() != expected {
                    return Err(invalid(format!(
                        "{operation} expects {expected} arguments including Session handle"
                    )));
                }
                let target = match request.arguments.get(1) {
                    Some(WireValue::String(value)) if !value.trim().is_empty() => value.clone(),
                    Some(_) => return Err(invalid("delegation target must be non-empty")),
                    None => return Err(invalid("delegation target is required")),
                };
                let task = match request.arguments.get(2) {
                    Some(WireValue::String(value)) if !value.trim().is_empty() => value.clone(),
                    Some(_) => return Err(invalid("delegation task must be non-empty")),
                    None => return Err(invalid("delegation task is required")),
                };
                let message = if has_message {
                    let value = json_argument(request, 3, "delegation message")?;
                    let wire: LlmMessageWire = serde_json::from_value(value).map_err(|error| {
                        invalid(format!("delegation message is malformed: {error}"))
                    })?;
                    Some(wire::message_from_wire(wire).map_err(invalid)?)
                } else {
                    None
                };
                let parent_label = if short_cancel {
                    None
                } else {
                    Some(
                        request
                            .arguments
                            .get(parent_index)
                            .and_then(|value| match value {
                                WireValue::String(value) => Some(value.clone()),
                                _ => None,
                            })
                            .filter(|value| !value.trim().is_empty())
                            .ok_or_else(|| invalid("delegation parent_label must be non-empty"))?,
                    )
                };
                let depth = if short_cancel {
                    0
                } else {
                    match request.arguments.get(depth_index) {
                        Some(WireValue::U64(value)) => u32::try_from(
                            value
                                .to_u64()
                                .ok_or_else(|| invalid("delegation depth is invalid"))?,
                        )
                        .map_err(|_| invalid("delegation depth exceeds u32"))?,
                        _ => return Err(invalid("delegation depth must be a U64")),
                    }
                };
                let runtime_context = if has_runtime_context {
                    runtime_context_from_json(optional_json_argument(
                        request,
                        runtime_index,
                        "runtime_context",
                    )?)?
                } else {
                    None
                };
                let allowed_tools = if has_allowed_tools {
                    string_list(
                        optional_json_argument(request, tools_index, "allowed_tools")?,
                        "allowed_tools",
                    )?
                } else {
                    None
                };
                let prompt_payload = if has_prompt_payload {
                    optional_json_argument(request, payload_index, "prompt_payload")?
                } else {
                    None
                };
                let prompt_context = if has_prompt_payload {
                    prompt_context_from_json(optional_json_argument(
                        request,
                        context_index,
                        "prompt_context",
                    )?)?
                } else {
                    None
                };
                let attempt_identity = if is_attempt {
                    Some(
                        serde_json::from_value(json_argument(
                            request,
                            identity_index,
                            "attempt_identity",
                        )?)
                        .map_err(|error| {
                            invalid(format!("attempt_identity is malformed: {error}"))
                        })?,
                    )
                } else {
                    None
                };
                let result = dispatch_rich_subagent(
                    state,
                    request,
                    target,
                    task,
                    parent_label,
                    depth,
                    runtime_context,
                    allowed_tools,
                    prompt_payload,
                    prompt_context,
                    message,
                    true,
                    attempt_identity,
                )
                .await?;
                Ok(result)
            }
            #[cfg(not(feature = "framework-subagent"))]
            {
                Err(framework(
                    &request.operation,
                    "delegation requires the framework-subagent feature",
                ))
            }
        }
        "echo_agent::agent::react::ReactAgent::delegate_task" => {
            #[cfg(feature = "framework-subagent")]
            {
                let agent = session_agent(state, request).await?;
                let session_handle = match request.arguments.first() {
                    Some(WireValue::Handle(handle)) if handle.kind == HandleKind::Session => handle,
                    Some(WireValue::Handle(_)) => {
                        return Err(invalid("delegate_task requires a Session handle"));
                    }
                    Some(_) => return Err(invalid("delegate_task requires a Session handle")),
                    None => return Err(invalid("delegate_task requires a Session handle")),
                };
                let task = match request.arguments.get(1) {
                    Some(WireValue::String(value)) if !value.trim().is_empty() => value,
                    Some(_) => return Err(invalid("delegate_task requires a non-empty task")),
                    None => return Err(invalid("delegate_task requires a task argument")),
                };
                if request.arguments.len() != 2 {
                    return Err(invalid("delegate_task accepts [Session handle, String]"));
                }
                let session_record = state.handles.session(session_handle)?;
                let authorities = state
                    .session_factory
                    .session_services(&session_record.acp_session_id)
                    .ok_or_else(|| {
                        framework(
                            &request.operation,
                            "Session subagent authority is unavailable",
                        )
                    })?;
                let definitions = authorities.subagent_registry.list_available().await;
                let result = if let Some(definition) = definitions.first() {
                    agent
                        .delegate_to(&definition.name, task)
                        .await
                        .map_err(|error| framework(&request.operation, error.to_string()))?
                } else {
                    agent
                        .chat(task)
                        .await
                        .map_err(|error| framework(&request.operation, error.to_string()))?
                };
                snapshot_value(request, serde_json::Value::String(result))
            }
            #[cfg(not(feature = "framework-subagent"))]
            {
                let _ = (state, handles, request);
                Err(framework(
                    "echo_agent::agent::react::ReactAgent::delegate_task",
                    "delegate_task requires the framework-subagent feature",
                ))
            }
        }
        "echo_agent::agent::react::ReactAgent::delegate_task_with_depth" => {
            #[cfg(feature = "framework-subagent")]
            {
                let task = match request.arguments.get(1) {
                    Some(WireValue::String(value)) if !value.trim().is_empty() => value.clone(),
                    Some(_) => {
                        return Err(invalid(
                            "delegate_task_with_depth requires a non-empty task",
                        ));
                    }
                    None => return Err(invalid("delegate_task_with_depth requires a task")),
                };
                let depth = match request.arguments.get(2) {
                    Some(WireValue::U64(value)) => value
                        .to_u64()
                        .ok_or_else(|| invalid("delegate depth is invalid"))
                        .and_then(|depth| {
                            u32::try_from(depth).map_err(|_| invalid("delegate depth exceeds u32"))
                        })?,
                    Some(_) => return Err(invalid("delegate depth must be a U64")),
                    None => return Err(invalid("delegate_task_with_depth requires a depth")),
                };
                if request.arguments.len() != 3 {
                    return Err(invalid(
                        "delegate_task_with_depth accepts [Session handle, String, U64]",
                    ));
                }
                let result =
                    dispatch_subagent_with_depth(state, request, None, task, depth).await?;
                snapshot_value(request, serde_json::Value::String(result))
            }
            #[cfg(not(feature = "framework-subagent"))]
            {
                Err(framework(
                    &request.operation,
                    "delegation requires the framework-subagent feature",
                ))
            }
        }
        "echo_agent::agent::react::ReactAgent::delegate_to_agent_with_depth" => {
            #[cfg(feature = "framework-subagent")]
            {
                let target = match request.arguments.get(1) {
                    Some(WireValue::String(value)) if !value.trim().is_empty() => value.clone(),
                    Some(_) => return Err(invalid("delegation target must be non-empty")),
                    None => return Err(invalid("delegation target is required")),
                };
                let task = match request.arguments.get(2) {
                    Some(WireValue::String(value)) if !value.trim().is_empty() => value.clone(),
                    Some(_) => return Err(invalid("delegation task must be non-empty")),
                    None => return Err(invalid("delegation task is required")),
                };
                let depth = match request.arguments.get(3) {
                    Some(WireValue::U64(value)) => value
                        .to_u64()
                        .ok_or_else(|| invalid("delegate depth is invalid"))
                        .and_then(|depth| {
                            u32::try_from(depth).map_err(|_| invalid("delegate depth exceeds u32"))
                        })?,
                    Some(_) => return Err(invalid("delegate depth must be a U64")),
                    None => return Err(invalid("delegate_to_agent_with_depth requires a depth")),
                };
                if request.arguments.len() != 4 {
                    return Err(invalid(
                        "delegate_to_agent_with_depth accepts [Session handle, target, task, depth]",
                    ));
                }
                let result =
                    dispatch_subagent_with_depth(state, request, Some(target), task, depth).await?;
                snapshot_value(request, serde_json::Value::String(result))
            }
            #[cfg(not(feature = "framework-subagent"))]
            {
                Err(framework(
                    &request.operation,
                    "delegation requires the framework-subagent feature",
                ))
            }
        }
        "echo_core::agent::Agent::tool_definitions"
        | "echo_agent::agent::Agent::tool_definitions"
        | "echo_agent::agent::react::ReactAgent::tool_definitions"
        | "echo_core::agent::Agent::messages"
        | "echo_agent::agent::Agent::messages"
        | "echo_agent::agent::react::ReactAgent::get_messages"
        | "echo_agent::agent::react::ReactAgent::load_messages" => {
            let authorities = session_authorities(state, request).await?;
            if request.operation.ends_with("load_messages") {
                if request.arguments.len() != 2 {
                    return Err(invalid(
                        "load_messages accepts [Session handle, List<Message>]",
                    ));
                }
                let values = match request.arguments.get(1) {
                    Some(WireValue::List(values)) => values,
                    Some(_) => return Err(invalid("load_messages requires a message list")),
                    None => return Err(invalid("load_messages requires a message list")),
                };
                let mut messages = Vec::with_capacity(values.len());
                for (index, value) in values.iter().enumerate() {
                    let json = value.clone().into_json().map_err(|error| {
                        invalid(format!(
                            "load_messages message {index} is not a wire value: {error}"
                        ))
                    })?;
                    let wire: LlmMessageWire = serde_json::from_value(json).map_err(|error| {
                        invalid(format!(
                            "load_messages message {index} is malformed: {error}"
                        ))
                    })?;
                    messages.push(wire::message_from_wire(wire).map_err(|error| {
                        invalid(format!("load_messages message {index} is invalid: {error}"))
                    })?);
                }
                authorities
                    .agent_handle
                    .write_async(move |agent| {
                        Box::pin(async move { agent.load_messages(messages).await })
                    })
                    .await;
                return snapshot_value(request, serde_json::Value::Null);
            }
            if request.arguments.len() != 1 {
                return Err(invalid(
                    "live Agent accessor accepts exactly one Session handle",
                ));
            }
            let value = if request.operation.ends_with("tool_definitions") {
                serde_json::to_value(
                    authorities
                        .agent_handle
                        .read(|agent| agent.tool_definitions())
                        .await,
                )
            } else if request.operation.ends_with("::get_messages") {
                serde_json::to_value(
                    authorities
                        .agent_handle
                        .read_async(|agent| Box::pin(agent.get_messages()))
                        .await,
                )
            } else {
                serde_json::to_value(authorities.agent_handle.read(Agent::messages).await)
            }
            .map_err(|error| framework(&request.operation, error.to_string()))?;
            snapshot_value(request, value)
        }
        "echo_orchestration::runtime::turn_driver::TurnOutcome::classify" => {
            turn_outcome_classify(request)
        }
        "echo_orchestration::runtime::turn_driver::TurnOutcome::status"
        | "echo_orchestration::runtime::turn_driver::TurnReceipt::status"
        | "echo_orchestration::runtime::turn_driver::TurnReceipt::usage" => {
            let run = request
                .handle
                .as_ref()
                .ok_or_else(|| invalid("TurnReceipt operation requires a Run handle"))?;
            handles.check_shape_and_generation(run, HandleKind::Run, METHOD)?;
            no_arguments(request)?;
            let record = handles.run(run)?;
            match record.as_ref() {
                RunRecord::Live { entry } => {
                    let receipt = entry.receipt().ok_or_else(|| {
                        framework(
                            &request.operation,
                            "TurnReceipt operation requires a settled Run receipt",
                        )
                    })?;
                    if request.operation.ends_with("::status") {
                        snapshot_value(request, serde_json::json!(receipt.status()))
                    } else {
                        let usage = receipt.usage();
                        execution_usage_value(
                            request,
                            WireU64::from_u64(usage.duration_ms.ok_or_else(|| {
                                framework(&request.operation, "TurnReceipt usage has no duration")
                            })?),
                            usage.tokens_used.map(WireU64::from_u64),
                            usage.iterations.map(WireU64::from_u64),
                        )
                    }
                }
                RunRecord::Recovered(recovered) => {
                    let receipt = recovered.receipt.as_ref().ok_or_else(|| {
                        framework(
                            &request.operation,
                            "TurnReceipt operation requires a persisted Run receipt",
                        )
                    })?;
                    if request.operation.ends_with("::status") {
                        snapshot_value(request, serde_json::json!(receipt.outcome.clone()))
                    } else {
                        recovered_usage_value(request, receipt)
                    }
                }
            }
        }
        #[cfg(not(feature = "sdk-extension-bridge"))]
        "echo_integration::mcp::client::McpClient::from_transport"
        | "echo_state::memory::sqlite_store::SqliteStore::with_embedder"
        | "echo_core::tokenizer::Tokenizer::count_tokens"
        | "echo_agent::agent::react::ReactAgent::set_compressor"
        | "echo_agent::agent::react::ReactAgent::set_audit_logger"
        | "echo_agent::agent::react::ReactAgent::set_conversation_store"
        | "echo_agent::agent::react::ReactAgent::set_memory_trigger_sink"
        | "echo_agent::agent::react::ReactAgent::set_memory_promoter"
        | "echo_agent::agent::react::ReactAgent::set_pre_model_context_projector"
        | "echo_agent::agent::react::ReactAgent::set_run_store"
        | "echo_agent::agent::react::ReactAgent::set_sandbox_executor"
        | "echo_agent::agent::react::ReactAgent::set_state_store"
        | "echo_agent::agent::react::ReactAgent::set_intent_router"
        | "echo_agent::agent::react::ReactAgent::set_skill_load_policy"
        | "echo_agent::eval::runner::EvalRunner::run_all"
        | "echo_agent::eval::runner::EvalRunner::run_all_async"
        | "echo_agent::improve::loop::ImprovementLoop::run"
        | "echo_agent::improve::loop::ImprovementLoop::run_async" => Err(wire::sdk_error(
            ExtensionErrorCode::FeatureUnavailable,
            "operation requires the SDK extension bridge capability",
            Retryability::Never,
            METHOD,
        )
        .with_operation(request.operation.clone())),
        #[cfg(not(feature = "framework-git"))]
        "echo_tools::git_checkpoint::cleanup_old_checkpoints"
        | "echo_tools::git_checkpoint::create_checkpoint"
        | "echo_tools::git_checkpoint::rollback_to_checkpoint" => Err(wire::sdk_error(
            ExtensionErrorCode::FeatureUnavailable,
            "operation requires the git feature",
            Retryability::Never,
            METHOD,
        )
        .with_operation(request.operation.clone())),
        _ => Err(framework(
            &request.operation,
            "source operation is canonical but has no Host authority adapter",
        )),
    }
}

#[cfg(test)]
mod classify_tests {
    use super::*;
    use echo_sdk_protocol::scalar::{WireField, WireMapEntry};

    const CLASSIFY: &str = "echo_orchestration::runtime::turn_driver::TurnOutcome::classify";

    fn request(value: WireValue) -> FeatureOperationRequest {
        FeatureOperationRequest {
            operation: CLASSIFY.to_string(),
            signature_digest: format!("sha256:{}", "0".repeat(64)),
            handle: None,
            arguments: vec![value],
        }
    }

    fn event(variant: &str, fields: Vec<WireField>) -> WireValue {
        WireValue::Variant {
            type_id: echo_sdk_protocol::methods::AGENT_EVENT_WIRE_TYPE_ID.to_string(),
            variant: variant.to_string(),
            fields,
        }
    }

    fn string_field(name: &str, value: &str) -> WireField {
        WireField {
            name: name.to_string(),
            value: WireValue::String(value.to_string()),
        }
    }

    fn failure_map() -> WireValue {
        WireValue::Map(vec![
            WireMapEntry {
                key: WireValue::String("category".to_string()),
                value: WireValue::String("llm".to_string()),
            },
            WireMapEntry {
                key: WireValue::String("terminal_kind".to_string()),
                value: WireValue::String("failed".to_string()),
            },
            WireMapEntry {
                key: WireValue::String("retryable".to_string()),
                value: WireValue::Bool(false),
            },
            WireMapEntry {
                key: WireValue::String("code".to_string()),
                value: WireValue::String("llm_invalid_response".to_string()),
            },
            WireMapEntry {
                key: WireValue::String("http_status".to_string()),
                value: WireValue::Null,
            },
            WireMapEntry {
                key: WireValue::String("message".to_string()),
                value: WireValue::String("bad response".to_string()),
            },
        ])
    }

    #[test]
    fn classify_projects_completed_cancelled_and_non_terminal() -> Result<(), String> {
        let completed = match turn_outcome_classify(&request(event(
            "final_answer",
            vec![string_field("text", "done")],
        ))) {
            Ok(value) => value,
            Err(error) => return Err(format!("final answer classification failed: {error:?}")),
        };
        assert!(matches!(
            completed,
            WireValue::Variant { variant, fields, .. }
                if variant == "completed" && fields.is_empty()
        ));

        let cancelled = match turn_outcome_classify(&request(event("cancelled", Vec::new()))) {
            Ok(value) => value,
            Err(error) => return Err(format!("cancelled classification failed: {error:?}")),
        };
        assert!(matches!(
            cancelled,
            WireValue::Variant { variant, fields, .. }
                if variant == "cancelled" && fields.is_empty()
        ));

        let pending = match turn_outcome_classify(&request(event(
            "token",
            vec![string_field("text", "partial")],
        ))) {
            Ok(value) => value,
            Err(error) => return Err(format!("non-terminal classification failed: {error:?}")),
        };
        assert_eq!(pending, WireValue::Null);
        Ok(())
    }

    #[test]
    fn classify_projects_typed_failure_and_rejects_malformed_input() -> Result<(), String> {
        let failed = match turn_outcome_classify(&request(event(
            "error",
            vec![
                string_field("source", "llm"),
                string_field("message", "bad response"),
                WireField {
                    name: "failure".to_string(),
                    value: failure_map(),
                },
            ],
        ))) {
            Ok(value) => value,
            Err(error) => return Err(format!("failure classification failed: {error:?}")),
        };
        if let WireValue::Variant {
            variant, fields, ..
        } = failed
        {
            assert_eq!(variant, "failed");
            assert!(fields.iter().any(|field| {
                field.name == "failure"
                    && matches!(field.value, WireValue::Record { ref type_id, .. } if type_id == echo_sdk_protocol::error::AGENT_FAILURE_WIRE_TYPE_ID)
            }));
        } else {
            return Err("failure classification must return a typed variant".to_string());
        }

        let malformed = request(WireValue::Map(Vec::new()));
        let error = match turn_outcome_classify(&malformed) {
            Ok(value) => return Err(format!("plain map unexpectedly classified as {value:?}")),
            Err(error) => error,
        };
        assert_eq!(error.code, ExtensionErrorCode::InvalidValue);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_sdk_protocol::methods::RunReceiptWire;

    fn request(operation: &str) -> FeatureOperationRequest {
        FeatureOperationRequest {
            operation: operation.to_string(),
            signature_digest:
                "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                    .to_string(),
            handle: None,
            arguments: Vec::new(),
        }
    }

    #[test]
    fn recovered_usage_preserves_nonzero_wire_u64_counters() -> Result<(), String> {
        let request = request("echo_orchestration::runtime::turn_driver::TurnReceipt::usage");
        let receipt = RunReceiptWire {
            turn_id: "turn-1".to_string(),
            outcome: "completed".to_string(),
            delivery: Some("delivered".to_string()),
            delivery_error: None,
            final_answer: Some("done".to_string()),
            final_message_id: None,
            prompt_tokens: WireU64::from_u64(9_007_199_254_740_993),
            completion_tokens: WireU64::from_u64(7),
            llm_calls: WireU64::from_u64(1),
            compaction_count: WireU64::from_u64(0),
            last_event_sequence: WireU64::from_u64(3),
            elapsed_ms: WireU64::from_u64(12),
        };

        let value = recovered_usage_value(&request, &receipt)
            .map_err(|error| format!("valid receipt projection failed: {error:?}"))?;
        let json = value
            .into_json()
            .map_err(|error| format!("usage projection is not JSON-compatible: {error:?}"))?;
        assert_eq!(
            json,
            serde_json::json!({
                "duration_ms": "12",
                "tokens_used": "9007199254741000",
                "iterations": null,
            })
        );
        Ok(())
    }
}
