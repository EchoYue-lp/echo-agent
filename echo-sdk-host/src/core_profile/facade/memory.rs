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
use std::sync::Arc;

use super::super::wire;
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
    })
}

/// Dispatch one memory family operation. Returns the typed wire result.
pub(crate) async fn dispatch(
    authorities: &Arc<SessionAuthorityServices>,
    request: &FeatureOperationRequest,
    page_limit: usize,
) -> Result<WireValue, EchoSdkError> {
    let Some(store) = authorities.memory_store.clone() else {
        return Err(invalid("session has no memory store"));
    };
    let arguments: Vec<serde_json::Value> = request
        .arguments
        .iter()
        .enumerate()
        .map(|(position, value)| json_of(value, position))
        .collect::<Result<_, _>>()?;
    let namespace_owned = arguments
        .first()
        .ok_or_else(|| invalid("memory operations require a namespace argument"))?;
    let namespace = namespace_of(namespace_owned)?;
    let namespace_refs: Vec<&str> = namespace.iter().map(String::as_str).collect();
    match request.operation.as_str() {
        "memory.store.put" => {
            let key = string_at(&arguments, 1, "a key")?;
            let value = arguments
                .get(2)
                .cloned()
                .ok_or_else(|| invalid("memory.store.put requires a value at argument 2"))?;
            store
                .put(&namespace_refs, &key, value)
                .await
                .map_err(framework)?;
            Ok(WireValue::from_json(serde_json::json!({"ok": true})).unwrap_or(WireValue::Null))
        }
        "memory.store.get" => {
            let key = string_at(&arguments, 1, "a key")?;
            let found = store.get(&namespace_refs, &key).await.map_err(framework)?;
            let value = found
                .as_ref()
                .map(item_to_json)
                .unwrap_or(serde_json::Value::Null);
            WireValue::from_json(value).map_err(|error| invalid(error.to_string()))
        }
        "memory.store.delete" => {
            let key = string_at(&arguments, 1, "a key")?;
            let deleted = store
                .delete(&namespace_refs, &key)
                .await
                .map_err(framework)?;
            WireValue::from_json(serde_json::json!({"deleted": deleted}))
                .map_err(|error| invalid(error.to_string()))
        }
        "memory.store.list" => {
            let items = store.list(&namespace_refs).await.map_err(framework)?;
            let page: Vec<serde_json::Value> =
                items.iter().take(page_limit).map(item_to_json).collect();
            WireValue::from_json(serde_json::json!({
                "items": page,
                "truncated": items.len() > page_limit.min(items.len()),
            }))
            .map_err(|error| invalid(error.to_string()))
        }
        "memory.store.search" => {
            let query = string_at(&arguments, 1, "a query")?;
            let limit = arguments
                .get(2)
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
