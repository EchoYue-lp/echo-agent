//! Session-owned PermissionService family adapter.
//!
//! Permission decisions are part of the live Session Agent pipeline. This
//! module only projects the existing PermissionService; it does not create a
//! second policy, classifier, approval cache or audit authority.

use echo_agent::human_loop::{
    ClassifierContext, PermissionContext, PermissionInvocationContext, PermissionUpdate,
    RiskContext,
};
use echo_agent::tools::permission::{PermissionRule, RuleBehavior, RuleMatcher, RuleSource};
use echo_sdk_protocol::error::{EchoSdkError, ExtensionErrorCode, Retryability};
use echo_sdk_protocol::methods::FeatureOperationRequest;
use echo_sdk_protocol::scalar::WireValue;
use std::sync::Arc;
use std::time::Duration;

use super::super::state::CoreProfileState;
use super::WireResult;

const METHOD: &str = "_echo_agent/permission/op";

fn invalid(message: impl Into<String>) -> EchoSdkError {
    super::wire::sdk_error(
        ExtensionErrorCode::InvalidValue,
        message,
        Retryability::Never,
        METHOD,
    )
}

fn framework(message: impl Into<String>) -> EchoSdkError {
    super::wire::sdk_error(
        ExtensionErrorCode::FrameworkError,
        message,
        Retryability::Never,
        METHOD,
    )
}

fn json_argument(
    request: &FeatureOperationRequest,
    position: usize,
    name: &str,
) -> Result<serde_json::Value, EchoSdkError> {
    let value = request
        .arguments
        .get(position)
        .cloned()
        .ok_or_else(|| invalid(format!("{name} is required at argument {position}")))?;
    value
        .into_json()
        .map_err(|error| invalid(format!("{name} is not a lossless wire value: {error}")))
}

fn string_argument(
    request: &FeatureOperationRequest,
    position: usize,
    name: &str,
) -> Result<String, EchoSdkError> {
    match request.arguments.get(position) {
        Some(WireValue::String(value)) if !value.trim().is_empty() => Ok(value.clone()),
        Some(_) => Err(invalid(format!("{name} must be a non-empty string"))),
        None => Err(invalid(format!(
            "{name} is required at argument {position}"
        ))),
    }
}

async fn service_of(
    state: &CoreProfileState,
    request: &FeatureOperationRequest,
) -> Result<Arc<echo_agent::human_loop::PermissionService>, EchoSdkError> {
    let session = request
        .handle
        .as_ref()
        .ok_or_else(|| invalid("permission operation requires a Session handle"))?;
    state.handles.check_shape_and_generation(
        session,
        echo_sdk_protocol::handle::HandleKind::Session,
        METHOD,
    )?;
    let record = state.handles.session(session)?;
    let authorities = state
        .session_factory
        .session_services(&record.acp_session_id)
        .ok_or_else(|| framework("Session authority is unavailable"))?;
    authorities
        .agent_handle
        .read(|agent| agent.permission_service().cloned())
        .await
        .ok_or_else(|| framework("Session Agent has no PermissionService authority"))
}

fn value<T: serde::Serialize>(value: T) -> Result<WireValue, EchoSdkError> {
    WireValue::from_json(serde_json::to_value(value).map_err(|error| framework(error.to_string()))?)
        .map_err(|error| framework(error.to_string()))
}

fn decision_value(
    decision: echo_agent::tools::permission::PermissionDecision,
) -> Result<WireValue, EchoSdkError> {
    let value = match decision {
        echo_agent::tools::permission::PermissionDecision::Allow => {
            serde_json::json!({"decision": "allow"})
        }
        echo_agent::tools::permission::PermissionDecision::Deny { reason } => {
            serde_json::json!({"decision": "deny", "reason": reason})
        }
        echo_agent::tools::permission::PermissionDecision::RequireApproval => {
            serde_json::json!({"decision": "require_approval"})
        }
        echo_agent::tools::permission::PermissionDecision::Ask { suggestions } => {
            serde_json::json!({"decision": "ask", "suggestions": suggestions})
        }
    };
    WireValue::from_json(value).map_err(|error| framework(error.to_string()))
}

/// Permission values are intentionally parsed here instead of relying on
/// `ToolPermission`'s Rust serde spelling.  The facade contract uses stable
/// lower-case identifiers (`read`, `write`, `network`, `execute`,
/// `sensitive`) across all language SDKs.
fn permission_value(
    value: &serde_json::Value,
    name: &str,
) -> Result<echo_agent::tools::permission::ToolPermission, EchoSdkError> {
    let text = value
        .as_str()
        .ok_or_else(|| invalid(format!("{name} must be a lower-case permission string")))?;
    match text {
        "read" => Ok(echo_agent::tools::permission::ToolPermission::Read),
        "write" => Ok(echo_agent::tools::permission::ToolPermission::Write),
        "network" => Ok(echo_agent::tools::permission::ToolPermission::Network),
        "execute" => Ok(echo_agent::tools::permission::ToolPermission::Execute),
        "sensitive" => Ok(echo_agent::tools::permission::ToolPermission::Sensitive),
        _ => Err(invalid(format!(
            "{name} has unsupported permission '{text}'"
        ))),
    }
}

fn permissions_value(
    value: &serde_json::Value,
    name: &str,
) -> Result<Vec<echo_agent::tools::permission::ToolPermission>, EchoSdkError> {
    let values = value
        .as_array()
        .ok_or_else(|| invalid(format!("{name} must be an array of permission strings")))?;
    values
        .iter()
        .enumerate()
        .map(|(index, value)| permission_value(value, &format!("{name}[{index}]")))
        .collect()
}

fn permission_mode_value(
    value: &serde_json::Value,
    name: &str,
) -> Result<echo_agent::tools::permission::PermissionMode, EchoSdkError> {
    let text = value
        .as_str()
        .ok_or_else(|| invalid(format!("{name} must be a permission mode string")))?;
    text.parse().map_err(|error: String| invalid(error))
}

fn optional_mode_argument(
    request: &FeatureOperationRequest,
    position: usize,
) -> Result<Option<echo_agent::tools::permission::PermissionMode>, EchoSdkError> {
    match request.arguments.get(position) {
        Some(WireValue::Null) => Ok(None),
        Some(value) => {
            let value = value
                .clone()
                .into_json()
                .map_err(|error| invalid(format!("permission mode is not lossless: {error}")))?;
            permission_mode_value(&value, "permission mode override").map(Some)
        }
        None => Err(invalid(format!(
            "permission mode override is required at argument {position}"
        ))),
    }
}

fn approval_scope_value(
    value: &serde_json::Value,
) -> Result<echo_agent::human_loop::ApprovalScope, EchoSdkError> {
    let text = value
        .as_str()
        .ok_or_else(|| invalid("approval scope must be a string"))?;
    match text.trim().to_ascii_lowercase().as_str() {
        "once" => Ok(echo_agent::human_loop::ApprovalScope::Once),
        "session" => Ok(echo_agent::human_loop::ApprovalScope::Session),
        "session_tool" | "session-tool" | "sessiontool" => {
            Ok(echo_agent::human_loop::ApprovalScope::SessionTool)
        }
        _ => Err(invalid(format!(
            "unsupported approval scope '{text}'; expected once, session, or session_tool"
        ))),
    }
}

fn rule_behavior_value(value: &serde_json::Value) -> Result<RuleBehavior, EchoSdkError> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid("permission rule behavior must be an object"))?;
    let kind = object
        .get("type")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| invalid("permission rule behavior requires a type"))?;
    match kind {
        "allow" => Ok(RuleBehavior::Allow),
        "deny" => {
            let reason = object
                .get("reason")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| invalid("deny permission rule requires a reason"))?;
            if reason.trim().is_empty() {
                return Err(invalid("deny permission rule reason must be non-empty"));
            }
            Ok(RuleBehavior::Deny {
                reason: reason.to_string(),
            })
        }
        "ask" => {
            let suggestions = object
                .get("suggestions")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| invalid("ask permission rule requires suggestions"))?
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    value
                        .as_str()
                        .filter(|text| !text.trim().is_empty())
                        .map(str::to_string)
                        .ok_or_else(|| {
                            invalid(format!(
                                "permission suggestions[{index}] must be a non-empty string"
                            ))
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(RuleBehavior::Ask { suggestions })
        }
        other => Err(invalid(format!(
            "unsupported permission rule behavior '{other}'"
        ))),
    }
}

fn rule_source_value(value: &serde_json::Value) -> Result<RuleSource, EchoSdkError> {
    let source = value
        .as_str()
        .ok_or_else(|| invalid("permission rule source must be a string"))?;
    match source {
        "default" => Ok(RuleSource::Default),
        "local_settings" | "localsettings" | "localSettings" => Ok(RuleSource::LocalSettings),
        "project_settings" | "projectsettings" | "projectSettings" => {
            Ok(RuleSource::ProjectSettings)
        }
        "user_settings" | "usersettings" | "userSettings" | "manual" => {
            Ok(RuleSource::UserSettings)
        }
        "managed" => Ok(RuleSource::Managed),
        "cli_arg" | "cliarg" | "cliArg" => Ok(RuleSource::CliArg),
        "session" => Ok(RuleSource::Session),
        other => Err(invalid(format!(
            "unsupported permission rule source '{other}'"
        ))),
    }
}

fn rule_matcher_value(value: &serde_json::Value) -> Result<RuleMatcher, EchoSdkError> {
    if let Some(text) = value.as_str() {
        if text.trim().is_empty() {
            return Err(invalid("permission rule matcher must be non-empty"));
        }
        return text
            .parse::<RuleMatcher>()
            .or_else(|_| {
                Ok::<RuleMatcher, String>(RuleMatcher::Pattern {
                    pattern: text.to_string(),
                })
            })
            .map_err(invalid);
    }
    let object = value
        .as_object()
        .ok_or_else(|| invalid("permission rule matcher must be an object or string"))?;
    let kind = object
        .get("type")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| invalid("permission rule matcher requires a type"))?;
    match kind {
        "tool" => {
            let name = object
                .get("name")
                .and_then(serde_json::Value::as_str)
                .filter(|text| !text.trim().is_empty())
                .ok_or_else(|| invalid("tool permission matcher requires a non-empty name"))?;
            Ok(RuleMatcher::Tool {
                name: name.to_string(),
            })
        }
        "pattern" => {
            let pattern = object
                .get("pattern")
                .and_then(serde_json::Value::as_str)
                .filter(|text| !text.trim().is_empty())
                .ok_or_else(|| {
                    invalid("pattern permission matcher requires a non-empty pattern")
                })?;
            Ok(RuleMatcher::Pattern {
                pattern: pattern.to_string(),
            })
        }
        "permission" => Ok(RuleMatcher::Permission {
            permission: permission_value(
                object
                    .get("permission")
                    .ok_or_else(|| invalid("permission matcher requires permission"))?,
                "permission matcher",
            )?,
        }),
        "all" => Ok(RuleMatcher::All),
        other => Err(invalid(format!(
            "unsupported permission rule matcher type '{other}'"
        ))),
    }
}

fn rule_value(
    request: &FeatureOperationRequest,
    position: usize,
) -> Result<PermissionRule, EchoSdkError> {
    rule_from_json(&json_argument(request, position, "permission rule")?)
}

fn rule_from_json(value: &serde_json::Value) -> Result<PermissionRule, EchoSdkError> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid("permission rule must be an object"))?;
    let matcher = rule_matcher_value(
        object
            .get("matcher")
            .ok_or_else(|| invalid("permission rule requires matcher"))?,
    )?;
    let behavior = rule_behavior_value(
        object
            .get("behavior")
            .ok_or_else(|| invalid("permission rule requires behavior"))?,
    )?;
    let source = rule_source_value(
        object
            .get("source")
            .ok_or_else(|| invalid("permission rule requires source"))?,
    )?;
    let description = object
        .get("description")
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| invalid("permission rule description must be a string"))
        })
        .transpose()?;
    Ok(PermissionRule {
        matcher,
        behavior,
        source,
        description,
    })
}

fn rule_to_json(rule: &PermissionRule) -> serde_json::Value {
    let matcher = match &rule.matcher {
        RuleMatcher::Tool { name } => serde_json::json!({"type": "tool", "name": name}),
        RuleMatcher::Pattern { pattern } => {
            serde_json::json!({"type": "pattern", "pattern": pattern})
        }
        RuleMatcher::Permission { permission } => {
            serde_json::json!({"type": "permission", "permission": permission.to_string()})
        }
        RuleMatcher::All => serde_json::json!({"type": "all"}),
    };
    let behavior = match &rule.behavior {
        RuleBehavior::Allow => serde_json::json!({"type": "allow"}),
        RuleBehavior::Deny { reason } => serde_json::json!({"type": "deny", "reason": reason}),
        RuleBehavior::Ask { suggestions } => {
            serde_json::json!({"type": "ask", "suggestions": suggestions})
        }
    };
    let source = match rule.source {
        RuleSource::Default => "default",
        RuleSource::LocalSettings => "local_settings",
        RuleSource::ProjectSettings => "project_settings",
        RuleSource::UserSettings => "user_settings",
        RuleSource::Managed => "managed",
        RuleSource::CliArg => "cli_arg",
        RuleSource::Session => "session",
    };
    let mut result = serde_json::Map::new();
    result.insert("matcher".to_string(), matcher);
    result.insert("behavior".to_string(), behavior);
    result.insert(
        "source".to_string(),
        serde_json::Value::String(source.to_string()),
    );
    if let Some(description) = &rule.description {
        result.insert(
            "description".to_string(),
            serde_json::Value::String(description.clone()),
        );
    }
    serde_json::Value::Object(result)
}

fn rules_value(rules: Vec<PermissionRule>) -> Result<WireValue, EchoSdkError> {
    WireValue::from_json(serde_json::Value::Array(
        rules.iter().map(rule_to_json).collect(),
    ))
    .map_err(|error| framework(error.to_string()))
}

#[derive(Debug, serde::Deserialize)]
#[serde(tag = "type")]
enum PermissionUpdateDto {
    #[serde(rename = "add_rule", alias = "addrule", alias = "addRule")]
    AddRule {
        matcher: String,
        behavior: String,
        source: String,
    },
    #[serde(rename = "remove_rule", alias = "removerule", alias = "removeRule")]
    RemoveRule { matcher: String },
    #[serde(rename = "set_mode", alias = "setmode", alias = "setMode")]
    SetMode { mode: String },
}

impl PermissionUpdateDto {
    fn into_framework(self) -> Result<PermissionUpdate, EchoSdkError> {
        match self {
            Self::AddRule {
                matcher,
                behavior,
                source,
            } => {
                let behavior = match behavior.as_str() {
                    "allow" | "deny" | "ask" => behavior,
                    other => {
                        return Err(invalid(format!(
                            "unsupported permission rule behavior '{other}'"
                        )));
                    }
                };
                let source = match source.as_str() {
                    "default" => "default",
                    "local_settings" | "localsettings" | "localSettings" => "local_settings",
                    "project_settings" | "projectsettings" | "projectSettings" => {
                        "project_settings"
                    }
                    "user_settings" | "usersettings" | "userSettings" | "manual" => "user_settings",
                    "managed" => "managed",
                    "cli_arg" | "cliarg" | "cliArg" => "cli_arg",
                    "session" => "session",
                    other => {
                        return Err(invalid(format!(
                            "unsupported permission rule source '{other}'"
                        )));
                    }
                }
                .to_string();
                Ok(PermissionUpdate::AddRule {
                    matcher,
                    behavior,
                    source,
                })
            }
            Self::RemoveRule { matcher } => Ok(PermissionUpdate::RemoveRule { matcher }),
            Self::SetMode { mode } => Ok(PermissionUpdate::SetMode {
                mode: permission_mode_value(
                    &serde_json::Value::String(mode),
                    "permission update mode",
                )?,
            }),
        }
    }
}

fn update_value(
    request: &FeatureOperationRequest,
    position: usize,
) -> Result<PermissionUpdate, EchoSdkError> {
    let value = json_argument(request, position, "permission update")?;
    let dto = serde_json::from_value::<PermissionUpdateDto>(value)
        .map_err(|error| invalid(format!("permission update is invalid: {error}")))?;
    dto.into_framework()
}

fn updates_value(
    request: &FeatureOperationRequest,
    position: usize,
) -> Result<Vec<PermissionUpdate>, EchoSdkError> {
    let value = json_argument(request, position, "permission updates")?;
    let values = value
        .as_array()
        .ok_or_else(|| invalid("permission updates must be an array"))?;
    values
        .iter()
        .cloned()
        .map(|value| {
            let dto = serde_json::from_value::<PermissionUpdateDto>(value)
                .map_err(|error| invalid(format!("permission update is invalid: {error}")))?;
            dto.into_framework()
        })
        .collect()
}

#[derive(Debug, Default, serde::Deserialize)]
struct ClassifierContextDto {
    #[serde(default)]
    messages: Vec<echo_agent::llm::LlmMessage>,
    #[serde(default)]
    allow_rules: Vec<String>,
    #[serde(default)]
    soft_deny_rules: Vec<String>,
    #[serde(default)]
    workspace_path: Option<String>,
    #[serde(default)]
    project_type: Option<String>,
    #[serde(default)]
    recent_files: Vec<String>,
    #[serde(default)]
    risk_context: Option<RiskContextDto>,
}

#[derive(Debug, serde::Deserialize)]
struct RiskContextDto {
    #[serde(default)]
    has_sensitive_files: bool,
    #[serde(default)]
    is_destructive: bool,
    #[serde(default)]
    directory_depth: usize,
    #[serde(default)]
    repetition_count: u32,
}

impl From<ClassifierContextDto> for ClassifierContext {
    fn from(value: ClassifierContextDto) -> Self {
        Self {
            messages: value.messages,
            allow_rules: value.allow_rules,
            soft_deny_rules: value.soft_deny_rules,
            workspace_path: value.workspace_path,
            project_type: value.project_type,
            recent_files: value.recent_files,
            risk_context: value.risk_context.map(|risk| RiskContext {
                has_sensitive_files: risk.has_sensitive_files,
                is_destructive: risk.is_destructive,
                directory_depth: risk.directory_depth,
                repetition_count: risk.repetition_count,
            }),
        }
    }
}

#[derive(Debug, Default, serde::Deserialize)]
struct InvocationContextDto {
    #[serde(default)]
    scope_id: Option<String>,
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    agent_name: Option<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    permission: Option<PermissionContext>,
    #[serde(default)]
    classifier: Option<ClassifierContextDto>,
}

impl From<InvocationContextDto> for PermissionInvocationContext {
    fn from(value: InvocationContextDto) -> Self {
        Self {
            scope_id: value.scope_id,
            request_id: value.request_id,
            session_id: value.session_id,
            agent_name: value.agent_name,
            timeout: value.timeout_ms.map(Duration::from_millis),
            permission: value.permission.unwrap_or_default(),
            classifier: value.classifier.unwrap_or_default().into(),
        }
    }
}

fn invocation_value(
    request: &FeatureOperationRequest,
    position: usize,
) -> Result<Option<PermissionInvocationContext>, EchoSdkError> {
    match request.arguments.get(position) {
        Some(WireValue::Null) => Ok(None),
        Some(value) => {
            let value = value.clone().into_json().map_err(|error| {
                invalid(format!("permission invocation is not lossless: {error}"))
            })?;
            let dto = serde_json::from_value::<InvocationContextDto>(value)
                .map_err(|error| invalid(format!("permission invocation is invalid: {error}")))?;
            Ok(Some(dto.into()))
        }
        None => Err(invalid(format!(
            "permission invocation is required at argument {position}"
        ))),
    }
}

fn check_value(check: echo_agent::human_loop::PermissionCheck) -> Result<WireValue, EchoSdkError> {
    let decision = match check.decision {
        echo_agent::tools::permission::PermissionDecision::Allow => {
            serde_json::json!({"decision": "allow"})
        }
        echo_agent::tools::permission::PermissionDecision::Deny { reason } => {
            serde_json::json!({"decision": "deny", "reason": reason})
        }
        echo_agent::tools::permission::PermissionDecision::RequireApproval => {
            serde_json::json!({"decision": "require_approval"})
        }
        echo_agent::tools::permission::PermissionDecision::Ask { suggestions } => {
            serde_json::json!({"decision": "ask", "suggestions": suggestions})
        }
    };
    let mut output = match decision {
        serde_json::Value::Object(object) => object,
        _ => {
            return Err(framework(
                "permission decision did not serialize as an object",
            ));
        }
    };
    output.insert(
        "updated_input".to_string(),
        check.updated_input.unwrap_or(serde_json::Value::Null),
    );
    WireValue::from_json(serde_json::Value::Object(output))
        .map_err(|error| framework(error.to_string()))
}

fn cache_stats_value(stats: echo_agent::human_loop::CacheStats) -> Result<WireValue, EchoSdkError> {
    let count = |value: usize, name: &str| {
        u64::try_from(value)
            .map(echo_sdk_protocol::scalar::WireU64::from_u64)
            .map_err(|_| framework(format!("{name} exceeds the WireU64 range")))
    };
    let per_tool_entries = count(stats.per_tool_entries, "per_tool_entries")?;
    let global_entries = count(stats.global_entries, "global_entries")?;
    let tools_cached = count(stats.tools_cached, "tools_cached")?;
    let ttl_ms = stats.ttl.map(|ttl| {
        echo_sdk_protocol::scalar::WireU64::from_u64(ttl.as_millis().min(u64::MAX as u128) as u64)
    });
    WireValue::from_json(serde_json::json!({
        "per_tool_entries": per_tool_entries,
        "global_entries": global_entries,
        "tools_cached": tools_cached,
        "ttl_ms": ttl_ms,
    }))
    .map_err(|error| framework(error.to_string()))
}

fn usize_value(value: usize, name: &str) -> Result<WireValue, EchoSdkError> {
    let value =
        u64::try_from(value).map_err(|_| framework(format!("{name} exceeds the WireU64 range")))?;
    Ok(WireValue::U64(
        echo_sdk_protocol::scalar::WireU64::from_u64(value),
    ))
}

pub(crate) async fn dispatch(
    state: &CoreProfileState,
    request: &FeatureOperationRequest,
) -> WireResult {
    let service = service_of(state, request).await?;
    match request.operation.as_str() {
        "permission.mode" => {
            if !request.arguments.is_empty() {
                return Err(invalid("PermissionService::mode accepts no arguments"));
            }
            value(service.mode().await)
        }
        "permission.set_mode" => {
            if request.arguments.len() != 1 {
                return Err(invalid("PermissionService::set_mode accepts one mode"));
            }
            let mode = string_argument(request, 0, "permission mode")?;
            let mode = permission_mode_value(&serde_json::Value::String(mode), "permission mode")?;
            service.set_mode(mode).await;
            value(())
        }
        "permission.check" => {
            if request.arguments.len() != 2 {
                return Err(invalid(
                    "PermissionService::check accepts tool name and input",
                ));
            }
            let tool_name = string_argument(request, 0, "tool name")?;
            let tool_input = json_argument(request, 1, "tool input")?;
            let decision = service
                .check(&tool_name, &tool_input)
                .await
                .map_err(|error| framework(error.to_string()))?;
            decision_value(decision)
        }
        "permission.apply_update" => {
            if request.arguments.len() != 1 {
                return Err(invalid(
                    "PermissionService::apply_update accepts one update",
                ));
            }
            service.apply_update(update_value(request, 0)?).await;
            value(())
        }
        "permission.apply_updates" => {
            if request.arguments.len() != 1 {
                return Err(invalid(
                    "PermissionService::apply_updates accepts one update array",
                ));
            }
            service.apply_updates(updates_value(request, 0)?).await;
            value(())
        }
        "permission.check_with_permissions" => {
            if request.arguments.len() != 3 {
                return Err(invalid(
                    "PermissionService::check_with_permissions accepts tool, input and permissions",
                ));
            }
            let tool_name = string_argument(request, 0, "tool name")?;
            let tool_input = json_argument(request, 1, "tool input")?;
            let permissions =
                permissions_value(&json_argument(request, 2, "permissions")?, "permissions")?;
            let decision = service
                .check_with_permissions(&tool_name, &tool_input, &permissions)
                .await
                .map_err(|error| framework(error.to_string()))?;
            decision_value(decision)
        }
        "permission.check_with_permissions_in_mode" => {
            if request.arguments.len() != 4 {
                return Err(invalid(
                    "PermissionService::check_with_permissions_in_mode accepts tool, input, permissions and mode",
                ));
            }
            let tool_name = string_argument(request, 0, "tool name")?;
            let tool_input = json_argument(request, 1, "tool input")?;
            let permissions =
                permissions_value(&json_argument(request, 2, "permissions")?, "permissions")?;
            let mode = optional_mode_argument(request, 3)?;
            let decision = service
                .check_with_permissions_in_mode(&tool_name, &tool_input, &permissions, mode)
                .await
                .map_err(|error| framework(error.to_string()))?;
            decision_value(decision)
        }
        "permission.check_with_permissions_result_in_mode" => {
            if request.arguments.len() != 4 {
                return Err(invalid(
                    "PermissionService::check_with_permissions_result_in_mode accepts tool, input, permissions and mode",
                ));
            }
            let tool_name = string_argument(request, 0, "tool name")?;
            let tool_input = json_argument(request, 1, "tool input")?;
            let permissions =
                permissions_value(&json_argument(request, 2, "permissions")?, "permissions")?;
            let mode = optional_mode_argument(request, 3)?;
            let check = service
                .check_with_permissions_result_in_mode(&tool_name, &tool_input, &permissions, mode)
                .await
                .map_err(|error| framework(error.to_string()))?;
            check_value(check)
        }
        "permission.check_with_permissions_result_in_mode_and_context" => {
            if request.arguments.len() != 5 {
                return Err(invalid(
                    "PermissionService::check_with_permissions_result_in_mode_and_context accepts tool, input, permissions, mode and invocation",
                ));
            }
            let tool_name = string_argument(request, 0, "tool name")?;
            let tool_input = json_argument(request, 1, "tool input")?;
            let permissions =
                permissions_value(&json_argument(request, 2, "permissions")?, "permissions")?;
            let mode = optional_mode_argument(request, 3)?;
            let invocation = invocation_value(request, 4)?;
            let check = service
                .check_with_permissions_result_in_mode_and_context(
                    &tool_name,
                    &tool_input,
                    &permissions,
                    mode,
                    invocation.as_ref(),
                )
                .await
                .map_err(|error| framework(error.to_string()))?;
            check_value(check)
        }
        "permission.add_rule" => {
            if request.arguments.len() != 1 {
                return Err(invalid("PermissionService::add_rule accepts one rule"));
            }
            let rule = rule_value(request, 0)?;
            service.add_rule(rule).await;
            value(())
        }
        "permission.add_rules" => {
            if request.arguments.len() != 1 {
                return Err(invalid("PermissionService::add_rules accepts one list"));
            }
            let rules_value = json_argument(request, 0, "permission rules")?;
            let rules = rules_value
                .as_array()
                .ok_or_else(|| invalid("permission rules must be an array"))?
                .iter()
                .map(rule_from_json)
                .collect::<Result<Vec<_>, _>>()?;
            service.add_rules(rules).await;
            value(())
        }
        "permission.remove_rule" => {
            if request.arguments.len() != 1 {
                return Err(invalid("PermissionService::remove_rule accepts one rule"));
            }
            let rule = rule_value(request, 0)?;
            value(service.remove_rule(&rule).await)
        }
        "permission.clear_rules" => {
            if !request.arguments.is_empty() {
                return Err(invalid(
                    "PermissionService::clear_rules accepts no arguments",
                ));
            }
            service.clear_rules().await;
            value(())
        }
        "permission.all_rules" => {
            if !request.arguments.is_empty() {
                return Err(invalid("PermissionService::all_rules accepts no arguments"));
            }
            rules_value(service.all_rules().await)
        }
        "permission.is_approved" => {
            if request.arguments.len() != 3 {
                return Err(invalid(
                    "SessionApprovalCache::is_approved accepts scope, tool and input",
                ));
            }
            let scope = string_argument(request, 0, "scope id")?;
            let tool = string_argument(request, 1, "tool name")?;
            let input = json_argument(request, 2, "tool input")?;
            value(service.is_approved(&scope, &tool, &input))
        }
        "permission.record_approval" => {
            if request.arguments.len() != 4 {
                return Err(invalid(
                    "SessionApprovalCache::record_approval accepts scope, tool, input and approval scope",
                ));
            }
            let scope = string_argument(request, 0, "scope id")?;
            let tool = string_argument(request, 1, "tool name")?;
            let input = json_argument(request, 2, "tool input")?;
            let approval_scope =
                approval_scope_value(&json_argument(request, 3, "approval scope")?)?;
            service.record_approval(&scope, &tool, &input, approval_scope);
            value(())
        }
        "permission.cleanup_expired" => {
            if !request.arguments.is_empty() {
                return Err(invalid(
                    "SessionApprovalCache::cleanup_expired accepts no arguments",
                ));
            }
            usize_value(service.cleanup_expired(), "expired cache entry count")
        }
        "permission.stats" => {
            if !request.arguments.is_empty() {
                return Err(invalid("SessionApprovalCache::stats accepts no arguments"));
            }
            cache_stats_value(service.stats())
        }
        "permission.revoke_cache" => {
            if request.arguments.len() != 2 {
                return Err(invalid(
                    "PermissionService::revoke_cache accepts scope and tool",
                ));
            }
            let scope = string_argument(request, 0, "scope id")?;
            let tool = string_argument(request, 1, "tool name")?;
            service.revoke_cache(&scope, &tool);
            value(())
        }
        "permission.clear_cache" => {
            if !request.arguments.is_empty() {
                return Err(invalid(
                    "PermissionService::clear_cache accepts no arguments",
                ));
            }
            service.clear_cache();
            value(())
        }
        "permission.would_request_human" => {
            if request.arguments.len() != 2 {
                return Err(invalid(
                    "PermissionService::would_request_human_for_permissions accepts tool and permissions",
                ));
            }
            let tool = string_argument(request, 0, "tool name")?;
            let permissions =
                permissions_value(&json_argument(request, 1, "permissions")?, "permissions")?;
            value(
                service
                    .would_request_human_for_permissions(&tool, &permissions)
                    .await,
            )
        }
        _ => Err(framework(format!(
            "permission operation {} has no adapter in this Host build",
            request.operation
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn wire_permissions_are_lowercase_and_explicit() {
        assert_eq!(
            permission_value(&json!("read"), "permission").ok(),
            Some(echo_agent::tools::permission::ToolPermission::Read)
        );
        assert!(permission_value(&json!("SENSITIVE"), "permission").is_err());
        assert!(permission_value(&json!("unknown"), "permission").is_err());
        assert!(permissions_value(&json!(["read", "write"]), "permissions").is_ok());
        assert!(permissions_value(&json!(["Read"]), "permissions").is_err());
    }

    #[test]
    fn rule_dto_rejects_unknown_behavior_and_source() {
        let valid = json!({
            "matcher": {"type": "permission", "permission": "execute"},
            "behavior": {"type": "deny", "reason": "blocked"},
            "source": "user_settings"
        });
        let parsed = rule_from_json(&valid);
        assert!(parsed.is_ok());
        if let Ok(parsed) = parsed {
            assert!(matches!(parsed.matcher, RuleMatcher::Permission { .. }));
            assert!(matches!(parsed.behavior, RuleBehavior::Deny { .. }));
            assert_eq!(parsed.source, RuleSource::UserSettings);
        }

        let unknown_behavior = json!({
            "matcher": "Bash",
            "behavior": {"type": "maybe"},
            "source": "session"
        });
        assert!(rule_from_json(&unknown_behavior).is_err());
        let unknown_source = json!({
            "matcher": "Bash",
            "behavior": {"type": "allow"},
            "source": "operator"
        });
        assert!(rule_from_json(&unknown_source).is_err());
    }

    #[test]
    fn invocation_dto_accepts_nullable_context_components() {
        let dto = serde_json::from_value::<InvocationContextDto>(json!({
            "scope_id": "scope",
            "permission": null,
            "classifier": null,
            "timeout_ms": 25
        }));
        assert!(dto.is_ok());
        let Some(dto) = dto.ok() else {
            return;
        };
        let context: PermissionInvocationContext = dto.into();
        assert_eq!(context.scope_id.as_deref(), Some("scope"));
        assert_eq!(context.timeout, Some(Duration::from_millis(25)));
        assert!(context.permission.affected_files.is_empty());
    }
}
