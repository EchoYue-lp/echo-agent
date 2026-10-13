//! JSON-RPC 2.0 message types for LSP communication.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC request.
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcRequest {
    pub fn new(id: u64, method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            method: method.into(),
            params,
        }
    }
}

/// JSON-RPC notification (no id, no response expected).
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcNotification {
    pub fn new(method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            method: method.into(),
            params,
        }
    }
}

/// JSON-RPC response.
#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_present_result")]
    pub result: Option<Value>,
    #[serde(default, deserialize_with = "deserialize_present_error")]
    pub error: Option<JsonRpcError>,
}

// A present JSON null is a successful nullable LSP result. Only an absent
// member means that the response did not provide a result.
fn deserialize_present_result<'de, D>(deserializer: D) -> Result<Option<Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Value::deserialize(deserializer).map(Some)
}

// A present error member must be an object; null does not mean "no error".
fn deserialize_present_error<'de, D>(deserializer: D) -> Result<Option<JsonRpcError>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    if !value.is_object() {
        return Err(serde::de::Error::custom("JSON-RPC error must be an object"));
    }
    serde_json::from_value(value)
        .map(Some)
        .map_err(serde::de::Error::custom)
}

/// JSON-RPC error object.
#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(default)]
    pub data: Option<Value>,
}

impl std::fmt::Display for JsonRpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "JSON-RPC error {}: {}", self.code, self.message)
    }
}

/// Encode a JSON-RPC message as an LSP content-length framed message.
///
/// LSP uses the format:
/// ```text
/// Content-Length: <length>\r\n
/// \r\n
/// <json body>
/// ```
pub fn encode_message(msg: &impl Serialize) -> Result<Vec<u8>, serde_json::Error> {
    let body = serde_json::to_vec(msg)?;
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    let mut bytes = Vec::with_capacity(header.len() + body.len());
    bytes.extend_from_slice(header.as_bytes());
    bytes.extend_from_slice(&body);
    Ok(bytes)
}

/// Parse a Content-Length header value from a header line.
pub fn parse_content_length(line: &str) -> Option<usize> {
    let line = line.trim();
    let (name, value) = line.split_once(':')?;
    name.eq_ignore_ascii_case("content-length")
        .then(|| value.trim().parse().ok())
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_message() -> Result<(), String> {
        let req = JsonRpcRequest::new(1, "initialize", None);
        let bytes = encode_message(&req).map_err(|error| error.to_string())?;
        let msg = String::from_utf8(bytes).map_err(|error| error.to_string())?;
        assert!(msg.starts_with("Content-Length:"));
        assert!(msg.contains("\"method\":\"initialize\""));
        Ok(())
    }

    #[test]
    fn test_parse_content_length() {
        assert_eq!(parse_content_length("Content-Length: 42"), Some(42));
        assert_eq!(parse_content_length("content-length: 100"), Some(100));
        assert_eq!(parse_content_length("Other-Header: foo"), None);
        assert_eq!(parse_content_length("Content-Length: 中文"), None);
        assert_eq!(parse_content_length("Ｃontent-Length: 4"), None);
    }

    #[test]
    fn test_parse_response() -> Result<(), String> {
        let json = r#"{"jsonrpc":"2.0","id":1,"result":{"capabilities":{}}}"#;
        let resp: JsonRpcResponse =
            serde_json::from_str(json).map_err(|error| error.to_string())?;
        assert_eq!(resp.id, Some(1));
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
        Ok(())
    }

    #[test]
    fn response_fidelity_null_result_is_present_but_missing_result_is_not() -> Result<(), String> {
        let present: JsonRpcResponse =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":1,"result":null}"#)
                .map_err(|e| e.to_string())?;
        let absent: JsonRpcResponse =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":2}"#).map_err(|e| e.to_string())?;
        assert_eq!(present.result, Some(Value::Null));
        assert_eq!(absent.result, None);
        Ok(())
    }

    #[test]
    fn test_parse_error_response() -> Result<(), String> {
        let json =
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"Method not found"}}"#;
        let resp: JsonRpcResponse =
            serde_json::from_str(json).map_err(|error| error.to_string())?;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.map(|error| error.code), Some(-32601));
        Ok(())
    }
}
