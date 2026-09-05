//! Lossless scalar and structural values used only by `_echo_agent/*`.

use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::handle::WireHandle;

const NANOS_PER_SECOND: u64 = 1_000_000_000;
const MAX_WIRE_VALUE_DEPTH: usize = 64;
const MAX_WIRE_COLLECTION_ITEMS: usize = 4096;
const MAX_WIRE_TEXT_CHARS: usize = 1_048_576;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct WireU64(String);

impl WireU64 {
    pub fn from_u64(value: u64) -> Self {
        Self(value.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn to_u64(&self) -> Option<u64> {
        self.0.parse().ok()
    }
}

impl TryFrom<String> for WireU64 {
    type Error = ScalarError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        validate_unsigned_decimal(&value)?;
        if value
            .parse::<u128>()
            .map(|number| number > u64::MAX as u128)
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

impl schemars::JsonSchema for WireU64 {
    fn schema_name() -> String {
        "WireU64".to_string()
    }

    fn json_schema(_: &mut schemars::r#gen::SchemaGenerator) -> schemars::schema::Schema {
        string_schema(&format!(
            "^(0|{})$",
            bounded_positive_pattern("18446744073709551615")
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct WireNonZeroU64(WireU64);

impl WireNonZeroU64 {
    pub fn to_u64(&self) -> Option<u64> {
        self.0.to_u64()
    }
}

impl TryFrom<String> for WireNonZeroU64 {
    type Error = ScalarError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let value = WireU64::try_from(value)?;
        if value.to_u64() == Some(0) {
            return Err(ScalarError::InvalidInteger("0".to_string()));
        }
        Ok(Self(value))
    }
}

impl From<WireNonZeroU64> for String {
    fn from(value: WireNonZeroU64) -> Self {
        value.0.into()
    }
}

impl schemars::JsonSchema for WireNonZeroU64 {
    fn schema_name() -> String {
        "WireNonZeroU64".to_string()
    }

    fn json_schema(_: &mut schemars::r#gen::SchemaGenerator) -> schemars::schema::Schema {
        string_schema(&format!(
            "^({})$",
            bounded_positive_pattern("18446744073709551615")
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct WireI64(String);

impl WireI64 {
    pub fn from_i64(value: i64) -> Self {
        Self(value.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn to_i64(&self) -> Option<i64> {
        self.0.parse().ok()
    }
}

impl TryFrom<String> for WireI64 {
    type Error = ScalarError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let unsigned = value.strip_prefix('-').unwrap_or(&value);
        validate_unsigned_decimal(unsigned)?;
        if value == "-0" || value.parse::<i64>().is_err() {
            return Err(ScalarError::InvalidInteger(value));
        }
        Ok(Self(value))
    }
}

impl From<WireI64> for String {
    fn from(value: WireI64) -> Self {
        value.0
    }
}

impl schemars::JsonSchema for WireI64 {
    fn schema_name() -> String {
        "WireI64".to_string()
    }

    fn json_schema(_: &mut schemars::r#gen::SchemaGenerator) -> schemars::schema::Schema {
        string_schema(&format!(
            "^(0|{}|-{})$",
            bounded_positive_pattern("9223372036854775807"),
            bounded_positive_pattern("9223372036854775808")
        ))
    }
}

fn string_schema(pattern: &str) -> schemars::schema::Schema {
    use schemars::schema::{InstanceType, Schema, SchemaObject, SingleOrVec, StringValidation};
    Schema::Object(SchemaObject {
        instance_type: Some(SingleOrVec::Single(Box::new(InstanceType::String))),
        string: Some(Box::new(StringValidation {
            pattern: Some(pattern.to_string()),
            ..StringValidation::default()
        })),
        ..SchemaObject::default()
    })
}

pub const BASE64_NO_PAD_FORMAT: &str = "echo-base64-no-pad";
pub const ABSOLUTE_UNIX_PATH_FORMAT: &str = "echo-absolute-unix-path-base64";
pub const ABSOLUTE_WINDOWS_PATH_FORMAT: &str = "echo-absolute-windows-path-utf16-base64";
pub const ABSOLUTE_UTF8_PATH_FORMAT: &str = "echo-absolute-utf8-path";

const BASE64_NO_PAD_PATTERN: &str =
    "^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/][AQgw]|[A-Za-z0-9+/]{2}[AEIMQUYcgkosw048])?$";

fn formatted_string_schema(pattern: &str, format: &str) -> schemars::schema::Schema {
    let mut schema = match string_schema(pattern) {
        schemars::schema::Schema::Object(schema) => schema,
        schema => return schema,
    };
    schema.format = Some(format.to_string());
    schemars::schema::Schema::Object(schema)
}

fn base64_no_pad_schema(_: &mut schemars::r#gen::SchemaGenerator) -> schemars::schema::Schema {
    formatted_string_schema(BASE64_NO_PAD_PATTERN, BASE64_NO_PAD_FORMAT)
}

fn absolute_unix_path_schema(_: &mut schemars::r#gen::SchemaGenerator) -> schemars::schema::Schema {
    formatted_string_schema(BASE64_NO_PAD_PATTERN, ABSOLUTE_UNIX_PATH_FORMAT)
}

fn absolute_windows_path_schema(
    _: &mut schemars::r#gen::SchemaGenerator,
) -> schemars::schema::Schema {
    formatted_string_schema(BASE64_NO_PAD_PATTERN, ABSOLUTE_WINDOWS_PATH_FORMAT)
}

fn absolute_utf8_path_schema(_: &mut schemars::r#gen::SchemaGenerator) -> schemars::schema::Schema {
    formatted_string_schema(
        "^(?:/.*|\\\\\\\\.*|[A-Za-z]:[\\\\/].*)$",
        ABSOLUTE_UTF8_PATH_FORMAT,
    )
}

fn bounded_positive_pattern(maximum: &str) -> String {
    let digits: Vec<char> = maximum.chars().collect();
    let mut alternatives = Vec::new();
    if digits.len() > 1 {
        alternatives.push(format!(
            "[1-9][0-9]{{0,{}}}",
            digits.len().saturating_sub(2)
        ));
    }
    for (index, digit) in digits.iter().enumerate() {
        let Some(maximum_digit) = digit.to_digit(10) else {
            continue;
        };
        let minimum_digit = if index == 0 { 1 } else { 0 };
        if maximum_digit > minimum_digit {
            let prefix: String = digits.iter().take(index).collect();
            let upper = maximum_digit.saturating_sub(1);
            let remaining = digits.len().saturating_sub(index.saturating_add(1));
            let suffix = if remaining == 0 {
                String::new()
            } else {
                format!("[0-9]{{{remaining}}}")
            };
            alternatives.push(format!("{prefix}[{minimum_digit}-{upper}]{suffix}"));
        }
    }
    alternatives.push(maximum.to_string());
    format!("(?:{})", alternatives.join("|"))
}

fn validate_unsigned_decimal(value: &str) -> Result<(), ScalarError> {
    if value.is_empty() || !value.chars().all(|character| character.is_ascii_digit()) {
        return Err(ScalarError::InvalidInteger(value.to_string()));
    }
    if value.chars().count() > 1 && value.starts_with('0') {
        return Err(ScalarError::InvalidInteger(value.to_string()));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScalarError {
    InvalidInteger(String),
    InvalidDuration,
    InvalidTimestamp,
    InvalidPath(&'static str),
    InvalidBase64,
    EmptyDomainId,
    ValueLimit(&'static str),
}

impl std::fmt::Display for ScalarError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInteger(value) => {
                write!(formatter, "not a canonical integer string: {value:?}")
            }
            Self::InvalidDuration => write!(formatter, "duration nanos must be below one second"),
            Self::InvalidTimestamp => write!(formatter, "timestamp nanos must be below one second"),
            Self::InvalidPath(reason) => write!(formatter, "invalid lossless path: {reason}"),
            Self::InvalidBase64 => write!(formatter, "invalid canonical base64 payload"),
            Self::EmptyDomainId => write!(formatter, "domain identity must be non-empty"),
            Self::ValueLimit(reason) => write!(formatter, "wire value exceeds limit: {reason}"),
        }
    }
}

impl std::error::Error for ScalarError {}

/// Rust `Duration` represented without collapsing the full `u64` seconds
/// range into an overflowing nanosecond count.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WireDuration {
    pub seconds: WireU64,
    #[schemars(range(max = 999999999))]
    pub nanos: u32,
}

impl WireDuration {
    pub fn from_nanos(nanos: u64) -> Self {
        Self {
            seconds: WireU64::from_u64(nanos / NANOS_PER_SECOND),
            nanos: u32::try_from(nanos % NANOS_PER_SECOND).unwrap_or_default(),
        }
    }

    pub fn validate(&self) -> Result<(), ScalarError> {
        if u64::from(self.nanos) >= NANOS_PER_SECOND {
            Err(ScalarError::InvalidDuration)
        } else {
            Ok(())
        }
    }
}

/// Signed Unix seconds plus a positive sub-second component. This represents
/// instants before and after the epoch without relying on JSON safe integers.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WireTimestamp {
    pub unix_seconds: WireI64,
    #[schemars(range(max = 999999999))]
    pub nanos: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rfc3339: Option<String>,
}

impl WireTimestamp {
    pub fn from_unix_parts(unix_seconds: i64, nanos: u32) -> Result<Self, ScalarError> {
        let timestamp = Self {
            unix_seconds: WireI64::from_i64(unix_seconds),
            nanos,
            rfc3339: None,
        };
        timestamp.validate()?;
        Ok(timestamp)
    }

    pub fn from_nanos(unix_nanos: u64) -> Self {
        Self {
            unix_seconds: WireI64::from_i64(
                i64::try_from(unix_nanos / NANOS_PER_SECOND).unwrap_or(i64::MAX),
            ),
            nanos: u32::try_from(unix_nanos % NANOS_PER_SECOND).unwrap_or_default(),
            rfc3339: None,
        }
    }

    pub fn validate(&self) -> Result<(), ScalarError> {
        if u64::from(self.nanos) >= NANOS_PER_SECOND {
            Err(ScalarError::InvalidTimestamp)
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "encoding", rename_all = "snake_case")]
pub enum WirePath {
    Unix {
        #[schemars(schema_with = "absolute_unix_path_schema")]
        bytes_base64: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
    },
    Windows {
        #[schemars(schema_with = "absolute_windows_path_schema")]
        utf16_base64: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
    },
    Utf8 {
        #[schemars(schema_with = "absolute_utf8_path_schema")]
        path: String,
    },
}

impl WirePath {
    pub fn validate(&self) -> Result<(), ScalarError> {
        match self {
            Self::Unix { bytes_base64, .. } => {
                if !is_absolute_unix_path_base64(bytes_base64) {
                    return Err(ScalarError::InvalidPath("Unix path must be absolute"));
                }
            }
            Self::Windows { utf16_base64, .. } => {
                if !is_absolute_windows_path_base64(utf16_base64) {
                    return Err(ScalarError::InvalidPath("Windows path must be absolute"));
                }
            }
            Self::Utf8 { path } => {
                if !is_absolute_utf8_path(path) {
                    return Err(ScalarError::InvalidPath("UTF-8 path must be absolute"));
                }
            }
        }
        Ok(())
    }
}

fn decode_base64(value: &str) -> Result<Vec<u8>, ScalarError> {
    base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(value)
        .map_err(|_| ScalarError::InvalidBase64)
}

pub fn is_base64_no_pad(value: &str) -> bool {
    decode_base64(value).is_ok()
}

pub fn is_absolute_unix_path_base64(value: &str) -> bool {
    decode_base64(value)
        .is_ok_and(|bytes| bytes.first().copied() == Some(b'/') && !bytes.contains(&0))
}

pub fn is_absolute_windows_path_base64(value: &str) -> bool {
    decode_base64(value).is_ok_and(|bytes| {
        if bytes.is_empty() || bytes.len() % 2 != 0 {
            return false;
        }
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .filter_map(|pair| {
                let low = pair.first().copied()?;
                let high = pair.get(1).copied()?;
                Some(u16::from_le_bytes([low, high]))
            })
            .collect();
        !units.contains(&0) && windows_units_are_absolute(&units)
    })
}

pub fn is_absolute_utf8_path(path: &str) -> bool {
    if path.contains('\0') {
        return false;
    }
    if path.starts_with('/') || path.starts_with("\\\\") {
        return true;
    }
    let mut characters = path.chars();
    matches!(
        (characters.next(), characters.next(), characters.next()),
        (Some(letter), Some(':'), Some('/' | '\\')) if letter.is_ascii_alphabetic()
    )
}

fn windows_units_are_absolute(units: &[u16]) -> bool {
    let first = units.first().copied();
    let second = units.get(1).copied();
    let third = units.get(2).copied();
    let drive_absolute = first
        .and_then(|unit| char::from_u32(u32::from(unit)))
        .is_some_and(|letter| letter.is_ascii_alphabetic())
        && second == Some(u16::from(b':'))
        && matches!(third, Some(value) if value == u16::from(b'\\') || value == u16::from(b'/'));
    let unc = matches!(
        (first, second),
        (Some(left), Some(right))
            if left == u16::from(b'\\') && right == u16::from(b'\\')
    );
    drive_absolute || unc
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WireBytes {
    #[schemars(schema_with = "base64_no_pad_schema")]
    pub base64: String,
}

impl WireBytes {
    pub fn validate(&self) -> Result<(), ScalarError> {
        if is_base64_no_pad(&self.base64) {
            Ok(())
        } else {
            Err(ScalarError::InvalidBase64)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WireField {
    #[schemars(length(min = 1, max = 256))]
    pub name: String,
    pub value: WireValue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WireMapEntry {
    pub key: WireValue,
    pub value: WireValue,
}

/// Closed structural value algebra used by manifest-identified facade
/// operations. It avoids an unbounded, schema-free `serde_json::Value`
/// escape hatch while still preserving records, variants and unknown values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum WireValue {
    Null,
    Bool(bool),
    String(String),
    I64(WireI64),
    U64(WireU64),
    F64(f64),
    Bytes(WireBytes),
    Duration(WireDuration),
    Timestamp(WireTimestamp),
    Path(WirePath),
    Handle(WireHandle),
    List(Vec<WireValue>),
    Map(Vec<WireMapEntry>),
    Record {
        #[schemars(length(min = 1, max = 512))]
        type_id: String,
        fields: Vec<WireField>,
    },
    Variant {
        #[schemars(length(min = 1, max = 512))]
        type_id: String,
        #[schemars(length(min = 1, max = 256))]
        variant: String,
        fields: Vec<WireField>,
    },
    Unknown {
        #[schemars(length(min = 1, max = 512))]
        type_tag: String,
        payload: Option<Box<WireValue>>,
    },
}

impl WireValue {
    pub fn validate(&self) -> Result<(), ScalarError> {
        self.validate_at_depth(0)
    }

    fn validate_at_depth(&self, depth: usize) -> Result<(), ScalarError> {
        if depth > MAX_WIRE_VALUE_DEPTH {
            return Err(ScalarError::ValueLimit("maximum nesting depth"));
        }
        match self {
            Self::String(value) => validate_text(value),
            Self::Bytes(value) => value.validate(),
            Self::Duration(value) => value.validate(),
            Self::Timestamp(value) => value.validate(),
            Self::Path(value) => value.validate(),
            Self::Handle(value) => value.validate().map_err(|_| ScalarError::EmptyDomainId),
            Self::List(values) => {
                validate_collection_len(values.len())?;
                for value in values {
                    value.validate_at_depth(depth.saturating_add(1))?;
                }
                Ok(())
            }
            Self::Map(entries) => {
                validate_collection_len(entries.len())?;
                for entry in entries {
                    entry.key.validate_at_depth(depth.saturating_add(1))?;
                    entry.value.validate_at_depth(depth.saturating_add(1))?;
                }
                Ok(())
            }
            Self::Record { type_id, fields } => {
                validate_identity(type_id)?;
                validate_fields(fields, depth)
            }
            Self::Variant {
                type_id,
                variant,
                fields,
            } => {
                validate_identity(type_id)?;
                validate_identity(variant)?;
                validate_fields(fields, depth)
            }
            Self::Unknown { type_tag, payload } => {
                validate_identity(type_tag)?;
                if let Some(payload) = payload {
                    payload.validate_at_depth(depth.saturating_add(1))?;
                }
                Ok(())
            }
            Self::F64(value) if !value.is_finite() => {
                Err(ScalarError::ValueLimit("non-finite floating point value"))
            }
            _ => Ok(()),
        }
    }

    pub fn from_json(value: serde_json::Value) -> Result<Self, ScalarError> {
        match value {
            serde_json::Value::Null => Ok(Self::Null),
            serde_json::Value::Bool(value) => Ok(Self::Bool(value)),
            serde_json::Value::String(value) => Ok(Self::String(value)),
            serde_json::Value::Number(value) => {
                if let Some(value) = value.as_u64() {
                    Ok(Self::U64(WireU64::from_u64(value)))
                } else if let Some(value) = value.as_i64() {
                    Ok(Self::I64(WireI64::from_i64(value)))
                } else if let Some(value) = value.as_f64().filter(|value| value.is_finite()) {
                    Ok(Self::F64(value))
                } else {
                    Err(ScalarError::ValueLimit("unsupported JSON number"))
                }
            }
            serde_json::Value::Array(values) => values
                .into_iter()
                .map(Self::from_json)
                .collect::<Result<Vec<_>, _>>()
                .map(Self::List),
            serde_json::Value::Object(values) => values
                .into_iter()
                .map(|(key, value)| {
                    Ok(WireMapEntry {
                        key: Self::String(key),
                        value: Self::from_json(value)?,
                    })
                })
                .collect::<Result<Vec<_>, ScalarError>>()
                .map(Self::Map),
        }
    }

    pub fn into_json(self) -> Result<serde_json::Value, ScalarError> {
        match self {
            Self::Null => Ok(serde_json::Value::Null),
            Self::Bool(value) => Ok(serde_json::Value::Bool(value)),
            Self::String(value) => Ok(serde_json::Value::String(value)),
            Self::I64(value) => value
                .to_i64()
                .map(serde_json::Value::from)
                .ok_or_else(|| ScalarError::InvalidInteger(value.as_str().to_string())),
            Self::U64(value) => value
                .to_u64()
                .map(serde_json::Value::from)
                .ok_or_else(|| ScalarError::InvalidInteger(value.as_str().to_string())),
            Self::F64(value) => serde_json::Number::from_f64(value)
                .map(serde_json::Value::Number)
                .ok_or(ScalarError::ValueLimit("non-finite floating point value")),
            Self::List(values) => values
                .into_iter()
                .map(Self::into_json)
                .collect::<Result<Vec<_>, _>>()
                .map(serde_json::Value::Array),
            Self::Map(entries) => {
                let mut object = serde_json::Map::new();
                for entry in entries {
                    let Self::String(key) = entry.key else {
                        return Err(ScalarError::ValueLimit("JSON object key must be text"));
                    };
                    if object.insert(key, entry.value.into_json()?).is_some() {
                        return Err(ScalarError::ValueLimit("duplicate JSON object key"));
                    }
                }
                Ok(serde_json::Value::Object(object))
            }
            _ => Err(ScalarError::ValueLimit(
                "typed wire value has no implicit JSON projection",
            )),
        }
    }
}

fn validate_fields(fields: &[WireField], depth: usize) -> Result<(), ScalarError> {
    validate_collection_len(fields.len())?;
    let mut names = std::collections::BTreeSet::new();
    for field in fields {
        validate_identity(&field.name)?;
        if !names.insert(field.name.as_str()) {
            return Err(ScalarError::ValueLimit("duplicate record field"));
        }
        field.value.validate_at_depth(depth.saturating_add(1))?;
    }
    Ok(())
}

fn validate_collection_len(len: usize) -> Result<(), ScalarError> {
    if len > MAX_WIRE_COLLECTION_ITEMS {
        Err(ScalarError::ValueLimit("maximum collection items"))
    } else {
        Ok(())
    }
}

fn validate_text(value: &str) -> Result<(), ScalarError> {
    if value.chars().count() > MAX_WIRE_TEXT_CHARS {
        Err(ScalarError::ValueLimit("maximum text characters"))
    } else {
        Ok(())
    }
}

fn validate_identity(value: &str) -> Result<(), ScalarError> {
    if value.trim().is_empty() {
        Err(ScalarError::EmptyDomainId)
    } else {
        validate_text(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers_round_trip_without_json_precision_loss() {
        assert_eq!(WireU64::from_u64(u64::MAX).to_u64(), Some(u64::MAX));
        assert_eq!(WireI64::from_i64(i64::MIN).to_i64(), Some(i64::MIN));
        assert!(WireU64::try_from("012".to_string()).is_err());
        assert!(WireI64::try_from("-0".to_string()).is_err());
    }

    #[test]
    fn duration_preserves_full_seconds_range() {
        let duration = WireDuration {
            seconds: WireU64::from_u64(u64::MAX),
            nanos: 999_999_999,
        };
        assert!(duration.validate().is_ok());
    }

    #[test]
    fn paths_reject_relative_and_invalid_encoded_values() {
        assert!(
            WirePath::Utf8 {
                path: "relative/file".to_string()
            }
            .validate()
            .is_err()
        );
        assert!(
            WirePath::Unix {
                bytes_base64: "not base64".to_string(),
                display: None
            }
            .validate()
            .is_err()
        );
    }
}
