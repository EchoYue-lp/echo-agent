//! Lossless extension scalars.
//!
//! Stable ACP v1 limits certain wire shapes: numbers must survive JavaScript,
//! paths are absolute UTF-8 strings, and binary has no native encoding. The
//! full SDK profile therefore carries these facts through `_echo_agent/*`
//! extension payloads only (design §10.5) — standard ACP methods never see
//! these types, and standard projection losses are never repaired by
//! reinterpreting standard fields.
//!
//! All integers travel as decimal strings so a `u64::MAX` survives every
//! language runtime unchanged; binary and raw path units travel as base64.

use serde::{Deserialize, Serialize};

/// A `u64`/`usize` carried as a non-empty decimal string. Parses back exactly;
/// leading zeros and non-digit content are rejected by the round-trip used in
/// contract fixtures.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(try_from = "String", into = "String")]
pub struct WireU64(String);

impl WireU64 {
    pub fn from_u64(value: u64) -> Self {
        Self(value.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Lossless conversion back to `u64`; invalid digits are a contract
    /// violation, surfaced as `None` rather than a panic.
    pub fn to_u64(&self) -> Option<u64> {
        self.0.parse().ok()
    }
}

impl TryFrom<String> for WireU64 {
    type Error = ScalarError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty() || !value.chars().all(|c| c.is_ascii_digit()) {
            return Err(ScalarError::InvalidInteger(value));
        }
        // Reject non-canonical spellings (leading zeros) so two
        // serializations of the same value can never differ.
        if value.len() > 1 && value.starts_with('0') {
            return Err(ScalarError::InvalidInteger(value));
        }
        if value
            .parse::<u128>()
            .map(|v| v > u64::MAX as u128)
            .unwrap_or(true)
        {
            return Err(ScalarError::InvalidInteger(value));
        }
        Ok(Self(value))
    }
}

impl From<WireU64> for String {
    fn from(value: WireU64) -> Self {
        value.0
    }
}

/// Errors produced when extension scalars fail validation. These become
/// `invalid_value` extension errors on the wire; they never panic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScalarError {
    InvalidInteger(String),
    EmptyDomainId,
}

impl std::fmt::Display for ScalarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScalarError::InvalidInteger(value) => {
                write!(f, "not a canonical u64 decimal string: {value:?}")
            }
            ScalarError::EmptyDomainId => write!(f, "domain identity must be non-empty"),
        }
    }
}

impl std::error::Error for ScalarError {}

/// A duration carried losslessly as nanoseconds.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WireDuration {
    pub nanos: WireU64,
}

impl WireDuration {
    pub fn from_nanos(nanos: u64) -> Self {
        Self {
            nanos: WireU64::from_u64(nanos),
        }
    }
}

/// A timestamp carried losslessly as nanoseconds since the Unix epoch, plus
/// the RFC 3339 rendering for display. `rfc3339` is informational only;
/// `unix_nanos` is the authority.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WireTimestamp {
    pub unix_nanos: WireU64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rfc3339: Option<String>,
}

impl WireTimestamp {
    pub fn from_nanos(unix_nanos: u64) -> Self {
        Self {
            unix_nanos: WireU64::from_u64(unix_nanos),
            rfc3339: None,
        }
    }
}

/// A local filesystem path that standard ACP UTF-8 rules cannot express
/// losslessly (design §10.5). Exactly one physical representation is set;
/// `display` is a human-readable convenience and never the authority.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "encoding", rename_all = "snake_case")]
pub enum WirePath {
    /// Absolute Unix path with its original bytes (base64). Preserves
    /// non-UTF-8 byte sequences.
    Unix {
        bytes_base64: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
    },
    /// Absolute Windows path with original UTF-16 units (base64 of the
    /// little-endian unit sequence).
    Windows {
        utf16_base64: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
    },
    /// Path that is exactly representable as an absolute UTF-8 string.
    Utf8 { path: String },
}

/// Opaque binary payload, base64-encoded.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WireBytes {
    pub base64: String,
}

/// A typed view over an unknown additive extension value (design §18): older
/// SDKs must observe unknown events without crashing, retaining the original
/// type tag and a bounded payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WireUnknown {
    pub type_tag: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u64_round_trip_lossless() {
        for value in [0u64, 1, 42, u32::MAX as u64, u64::MAX] {
            let wire = WireU64::from_u64(value);
            let json = serde_json::to_string(&wire).unwrap_or_default();
            let back: WireU64 = serde_json::from_str(&json).unwrap_or(WireU64::from_u64(0));
            assert_eq!(back.to_u64(), Some(value));
        }
    }

    #[test]
    fn rejects_non_canonical_integers() {
        assert!(WireU64::try_from("12".to_string()).is_ok());
        assert!(WireU64::try_from("".to_string()).is_err());
        assert!(WireU64::try_from("012".to_string()).is_err());
        assert!(WireU64::try_from("-1".to_string()).is_err());
        assert!(WireU64::try_from("1.5".to_string()).is_err());
        assert!(WireU64::try_from("18446744073709551616".to_string()).is_err());
    }

    #[test]
    fn path_variants_are_disjoint() {
        let unix = WirePath::Unix {
            bytes_base64: "L3RtcC9mb28".to_string(),
            display: Some("/tmp/foo".to_string()),
        };
        let json = serde_json::to_string(&unix).unwrap_or_default();
        assert!(json.contains("\"encoding\":\"unix\""));
        let back: Result<WirePath, _> = serde_json::from_str(&json);
        assert_eq!(
            back.unwrap_or(WirePath::Utf8 {
                path: String::new()
            }),
            unix
        );
    }
}
