//! Memory family adapter (plan 07, todo 4).
//!
//! `_echo_agent/memory/op` routes the closed store-operation set onto the
//! Session Agent's own [`echo_agent::memory::Store`] authority — the same
//! instance the in-conversation memory tools use. Operations are exact
//! identities (`memory.store.put`, …), never wildcards; results keep the
//! framework's shapes and errors verbatim, bounded by the negotiated page
//! limit.

use echo_sdk_protocol::error::{EchoSdkError, ExtensionErrorCode, Retryability};
use echo_sdk_protocol::methods::FeatureOperationRequest;
use echo_sdk_protocol::scalar::WireValue;
use std::collections::HashMap;
use std::sync::Arc;

use super::super::wire;
use super::owned_resource;
use super::source_operations::MemoryStoreAuthority;
use crate::core_profile::handles::HandleRegistry;
use crate::factory::SessionAuthorityServices;

const METHOD: &str = "_echo_agent/memory/op";

fn invalid(message: impl Into<String>) -> EchoSdkError {
    wire::sdk_error(
        ExtensionErrorCode::InvalidValue,
        message,
        Retryability::Never,
        METHOD,
    )
}

fn json_of(value: &WireValue, position: usize) -> Result<serde_json::Value, EchoSdkError> {
    value.clone().into_json().map_err(|error| {
        invalid(format!(
            "memory argument {position} is not a lossless wire value: {error}"
        ))
    })
}

fn namespace_of(value: &serde_json::Value) -> Result<Vec<String>, EchoSdkError> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .filter(|items| !items.is_empty())
        .ok_or_else(|| invalid("memory namespace must be a non-empty string array"))
}

fn string_at(
    arguments: &[serde_json::Value],
    position: usize,
    what: &str,
) -> Result<String, EchoSdkError> {
    arguments
        .get(position)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| {
            invalid(format!(
                "memory operation requires {what} at argument {position}"
            ))
        })
}

fn item_to_json(item: &echo_agent::memory::StoreItem) -> serde_json::Value {
    serde_json::json!({
        "namespace": item.namespace,
        "key": item.key,
        "value": item.value,
        "created_at": item.created_at,
        "updated_at": item.updated_at,
        "score": item.score,
        "importance": item.importance,
        "last_accessed": item.last_accessed,
        "expires_at": item.expires_at,
    })
}

fn store_for_request<'a>(
    authorities: &Arc<SessionAuthorityServices>,
    resource_stores: &std::sync::Mutex<HashMap<String, Arc<MemoryStoreAuthority>>>,
    handles: &HandleRegistry,
    owner: &str,
    request: &'a FeatureOperationRequest,
) -> Result<(Arc<dyn echo_agent::memory::Store>, usize, &'a str), EchoSdkError> {
    if !request.operation.starts_with("memory.resource.") {
        let store = authorities
            .memory_store
            .clone()
            .ok_or_else(|| invalid("session has no memory store"))?;
        return Ok((store, 0, request.operation.as_str()));
    }
    let resource = match request.arguments.first() {
        Some(WireValue::Handle(handle)) => handle,
        _ => return Err(invalid("memory.resource operation requires a Store handle")),
    };
    let record = owned_resource(handles, resource, owner, METHOD)?;
    if record.family != "memory" || record.resource_type != "memory.store" {
        return Err(invalid("memory resource handle is not a Store"));
    }
    let store = resource_stores
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&resource.id)
        .cloned()
        .ok_or_else(|| invalid("memory Store resource is closed"))?
        .store();
    let operation = match request.operation.as_str() {
        "memory.resource.put" => "memory.store.put",
        "memory.resource.get" => "memory.store.get",
        "memory.resource.search" => "memory.store.search",
        "memory.resource.search_with" => "memory.store.search_with",
        "memory.resource.delete" => "memory.store.delete",
        "memory.resource.list_namespaces" => "memory.store.list_namespaces",
        "memory.resource.list" => "memory.store.list",
        "memory.resource.prune_expired" => "memory.store.prune_expired",
        "memory.resource.dedup_by_content" => "memory.store.dedup_by_content",
        _ => return Err(invalid("unknown memory resource operation")),
    };
    Ok((store, 1, operation))
}

/// Dispatch one memory family operation. Returns the typed wire result.
pub(crate) async fn dispatch(
    authorities: &Arc<SessionAuthorityServices>,
    resource_stores: &std::sync::Mutex<HashMap<String, Arc<MemoryStoreAuthority>>>,
    handles: &HandleRegistry,
    owner: &str,
    request: &FeatureOperationRequest,
    page_limit: usize,
) -> Result<WireValue, EchoSdkError> {
    let (store, offset, operation) =
        store_for_request(authorities, resource_stores, handles, owner, request)?;
    let arguments: Vec<serde_json::Value> = request
        .arguments
        .iter()
        .enumerate()
        .map(|(position, value)| {
            if position < offset {
                Ok(serde_json::Value::Null)
            } else {
                json_of(value, position)
            }
        })
        .collect::<Result<_, _>>()?;
    if operation == "memory.store.list_namespaces" {
        let prefix = match arguments.get(offset) {
            None | Some(serde_json::Value::Null) => None,
            Some(value) => Some(namespace_of(value)?),
        };
        let prefix_refs = prefix
            .as_ref()
            .map(|parts| parts.iter().map(String::as_str).collect::<Vec<_>>());
        let namespaces = store
            .list_namespaces(prefix_refs.as_deref())
            .await
            .map_err(framework)?;
        let truncated = namespaces.len() > page_limit;
        return WireValue::from_json(serde_json::json!({
            "namespaces": namespaces.into_iter().take(page_limit).collect::<Vec<_>>(),
            "truncated": truncated,
        }))
        .map_err(|error| invalid(error.to_string()));
    }
    let namespace_owned = arguments
        .get(offset)
        .ok_or_else(|| invalid("memory operations require a namespace argument"))?;
    let namespace = namespace_of(namespace_owned)?;
    let namespace_refs: Vec<&str> = namespace.iter().map(String::as_str).collect();
    match operation {
        "memory.store.put" => {
            let key = string_at(&arguments, offset.saturating_add(1), "a key")?;
            let value = arguments
                .get(offset.saturating_add(2))
                .cloned()
                .ok_or_else(|| invalid("memory.store.put requires a value"))?;
            store
                .put(&namespace_refs, &key, value)
                .await
                .map_err(framework)?;
            Ok(WireValue::from_json(serde_json::json!({"ok": true})).unwrap_or(WireValue::Null))
        }
        "memory.store.get" => {
            let key = string_at(&arguments, offset.saturating_add(1), "a key")?;
            let found = store.get(&namespace_refs, &key).await.map_err(framework)?;
            let value = found
                .as_ref()
                .map(item_to_json)
                .unwrap_or(serde_json::Value::Null);
            WireValue::from_json(value).map_err(|error| invalid(error.to_string()))
        }
        "memory.store.delete" => {
            let key = string_at(&arguments, offset.saturating_add(1), "a key")?;
            let deleted = store
                .delete(&namespace_refs, &key)
                .await
                .map_err(framework)?;
            WireValue::from_json(serde_json::json!({"deleted": deleted}))
                .map_err(|error| invalid(error.to_string()))
        }
        "memory.store.list" => {
            let items = store.list(&namespace_refs).await.map_err(framework)?;
            let truncated = items.len() > page_limit;
            let page: Vec<serde_json::Value> =
                items.iter().take(page_limit).map(item_to_json).collect();
            WireValue::from_json(serde_json::json!({
                "items": page,
                "truncated": truncated,
            }))
            .map_err(|error| invalid(error.to_string()))
        }
        "memory.store.search" => {
            let query = string_at(&arguments, offset.saturating_add(1), "a query")?;
            let limit = arguments
                .get(offset.saturating_add(2))
                .and_then(serde_json::Value::as_u64)
                .map(|limit| limit.min(page_limit as u64))
                .unwrap_or(page_limit.min(64) as u64)
                .min(page_limit as u64) as usize;
            let items = store
                .search_with(
                    &namespace_refs,
                    echo_agent::memory::SearchQuery {
                        text: query.as_str(),
                        limit,
                        mode: echo_agent::memory::SearchMode::Keyword,
                    },
                )
                .await
                .map_err(framework)?;
            let page: Vec<serde_json::Value> = items.iter().take(limit).map(item_to_json).collect();
            WireValue::from_json(serde_json::json!({"items": page}))
                .map_err(|error| invalid(error.to_string()))
        }
        "memory.store.search_with" => {
            let query = arguments
                .get(offset.saturating_add(1))
                .and_then(serde_json::Value::as_object)
                .ok_or_else(|| invalid("memory.store.search_with requires a query object"))?;
            let text = query
                .get("text")
                .and_then(serde_json::Value::as_str)
                .filter(|text| !text.trim().is_empty())
                .ok_or_else(|| invalid("memory search query requires non-empty text"))?;
            let limit = query
                .get("limit")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(page_limit.min(64) as u64)
                .min(page_limit as u64) as usize;
            let mode = query
                .get("mode")
                .cloned()
                .map(serde_json::from_value::<echo_agent::memory::SearchMode>)
                .transpose()
                .map_err(|error| invalid(format!("memory search mode is malformed: {error}")))?
                .unwrap_or(echo_agent::memory::SearchMode::Keyword);
            let items = store
                .search_with(
                    &namespace_refs,
                    echo_agent::memory::SearchQuery { text, limit, mode },
                )
                .await
                .map_err(framework)?;
            let page: Vec<serde_json::Value> = items.iter().take(limit).map(item_to_json).collect();
            WireValue::from_json(serde_json::json!({"items": page}))
                .map_err(|error| invalid(error.to_string()))
        }
        "memory.store.prune_expired" => {
            let removed = store
                .prune_expired(&namespace_refs)
                .await
                .map_err(framework)?;
            Ok(WireValue::U64(
                echo_sdk_protocol::scalar::WireU64::from_u64(removed),
            ))
        }
        "memory.store.dedup_by_content" => {
            let removed = store
                .dedup_by_content(&namespace_refs)
                .await
                .map_err(framework)?;
            Ok(WireValue::U64(
                echo_sdk_protocol::scalar::WireU64::from_u64(removed),
            ))
        }
        other => Err(invalid(format!(
            "unknown memory operation {other}; the family surface is closed"
        ))),
    }
}

fn framework(error: echo_agent::error::ReactError) -> EchoSdkError {
    wire::sdk_error(
        ExtensionErrorCode::FrameworkError,
        wire::bounded_framework_message(&error.to_string()),
        Retryability::Never,
        METHOD,
    )
}
