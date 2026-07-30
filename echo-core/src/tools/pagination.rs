//! Shared cursor pagination contract for collection-returning tools.

use super::{ToolParameters, ToolResult};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::io::{self, Write};

const CURSOR_VERSION: u8 = 1;
const MAX_CURSOR_CHARS: usize = 1_024;

/// Pagination input shared by collection-returning tools.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageRequest {
    pub limit: usize,
    pub cursor: Option<String>,
}

/// Pagination metadata shared by collection-returning tools.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PageInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub truncated: bool,
    pub total_known: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<usize>,
    pub returned: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageError {
    InvalidLimit { limit: usize, max: usize },
    InvalidParameter(String),
    InvalidCursor(String),
    CursorQueryMismatch,
    CursorOutOfRange { offset: usize, total: usize },
}

impl fmt::Display for PageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLimit { limit, max } => {
                write!(
                    formatter,
                    "limit must be between 1 and {max}, received {limit}"
                )
            }
            Self::InvalidParameter(message) | Self::InvalidCursor(message) => {
                formatter.write_str(message)
            }
            Self::CursorQueryMismatch => formatter.write_str(
                "cursor does not match the current query or result snapshot; restart without cursor",
            ),
            Self::CursorOutOfRange { offset, total } => write!(
                formatter,
                "cursor offset {offset} is beyond the current result count {total}; restart without cursor"
            ),
        }
    }
}

impl std::error::Error for PageError {}

#[derive(Debug, Serialize, Deserialize)]
struct CursorEnvelope {
    version: u8,
    query_fingerprint: String,
    offset: usize,
}

#[derive(Debug, Serialize)]
struct FingerprintInput<'a, Q, T> {
    query: &'a Q,
    limit: usize,
    items: &'a [T],
}

impl PageRequest {
    pub fn from_parameters(
        parameters: &ToolParameters,
        default_limit: usize,
        max_limit: usize,
    ) -> Result<Self, PageError> {
        if default_limit == 0 || default_limit > max_limit {
            return Err(PageError::InvalidParameter(
                "pagination defaults are invalid".to_string(),
            ));
        }
        let limit = match parameters.get("limit") {
            Some(value) => value
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .ok_or_else(|| {
                    PageError::InvalidParameter("limit must be a positive integer".to_string())
                })?,
            None => default_limit,
        };
        if limit == 0 || limit > max_limit {
            return Err(PageError::InvalidLimit {
                limit,
                max: max_limit,
            });
        }
        let cursor = match parameters.get("cursor") {
            Some(value) => Some(
                value
                    .as_str()
                    .filter(|cursor| !cursor.is_empty())
                    .ok_or_else(|| {
                        PageError::InvalidParameter("cursor must be a non-empty string".to_string())
                    })?
                    .to_string(),
            ),
            None => None,
        };
        Ok(Self { limit, cursor })
    }

    pub fn paginate<T, Q>(&self, items: Vec<T>, query: &Q) -> Result<(Vec<T>, PageInfo), PageError>
    where
        T: Serialize,
        Q: Serialize,
    {
        let fingerprint = query_fingerprint(query, self.limit, &items)?;
        let offset = match self.cursor.as_deref() {
            Some(cursor) => {
                let envelope = decode_cursor(cursor)?;
                if envelope.version != CURSOR_VERSION {
                    return Err(PageError::InvalidCursor(format!(
                        "unsupported cursor version {}",
                        envelope.version
                    )));
                }
                if envelope.query_fingerprint != fingerprint {
                    return Err(PageError::CursorQueryMismatch);
                }
                envelope.offset
            }
            None => 0,
        };

        let total = items.len();
        if offset > total {
            return Err(PageError::CursorOutOfRange { offset, total });
        }
        let end = offset.saturating_add(self.limit).min(total);
        let returned = end.saturating_sub(offset);
        let page = items.into_iter().skip(offset).take(returned).collect();
        let next_cursor = if end < total {
            Some(encode_cursor(&CursorEnvelope {
                version: CURSOR_VERSION,
                query_fingerprint: fingerprint,
                offset: end,
            })?)
        } else {
            None
        };
        Ok((
            page,
            PageInfo {
                truncated: next_cursor.is_some(),
                next_cursor,
                total_known: true,
                total: Some(total),
                returned,
            },
        ))
    }
}

impl PageInfo {
    /// Attach the common page contract to a tool result without exposing content.
    pub fn apply_to(&self, result: &mut ToolResult) {
        result.truncated = result.truncated || self.truncated;
        result
            .metadata
            .insert("page.truncated".to_string(), self.truncated.to_string());
        result
            .metadata
            .insert("page.total_known".to_string(), self.total_known.to_string());
        result
            .metadata
            .insert("page.returned".to_string(), self.returned.to_string());
        if let Some(total) = self.total {
            result
                .metadata
                .insert("page.total".to_string(), total.to_string());
        }
        if let Some(cursor) = &self.next_cursor {
            result
                .metadata
                .insert("page.next_cursor".to_string(), cursor.clone());
        }
    }
}

fn query_fingerprint<Q: Serialize, T: Serialize>(
    query: &Q,
    limit: usize,
    items: &[T],
) -> Result<String, PageError> {
    let mut writer = Sha256Writer::default();
    serde_json::to_writer(
        &mut writer,
        &FingerprintInput {
            query,
            limit,
            items,
        },
    )
    .map_err(|error| {
        PageError::InvalidParameter(format!("cannot fingerprint pagination snapshot: {error}"))
    })?;
    Ok(format!("{:x}", writer.0.finalize()))
}

#[derive(Default)]
struct Sha256Writer(Sha256);

impl Write for Sha256Writer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn encode_cursor(envelope: &CursorEnvelope) -> Result<String, PageError> {
    let bytes = serde_json::to_vec(envelope)
        .map_err(|error| PageError::InvalidCursor(format!("cannot encode cursor: {error}")))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn decode_cursor(cursor: &str) -> Result<CursorEnvelope, PageError> {
    if cursor.chars().count() > MAX_CURSOR_CHARS || !cursor.len().is_multiple_of(2) {
        return Err(PageError::InvalidCursor(
            "cursor has an invalid length".to_string(),
        ));
    }
    let chars: Vec<char> = cursor.chars().collect();
    let mut bytes = Vec::with_capacity(chars.len() / 2);
    for pair in chars.chunks_exact(2) {
        let high = pair
            .first()
            .and_then(|value| value.to_digit(16))
            .ok_or_else(|| PageError::InvalidCursor("cursor is not valid hex".to_string()))?;
        let low = pair
            .get(1)
            .and_then(|value| value.to_digit(16))
            .ok_or_else(|| PageError::InvalidCursor("cursor is not valid hex".to_string()))?;
        let byte = high
            .checked_mul(16)
            .and_then(|value| value.checked_add(low))
            .and_then(|value| u8::try_from(value).ok())
            .ok_or_else(|| PageError::InvalidCursor("cursor byte is invalid".to_string()))?;
        bytes.push(byte);
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| PageError::InvalidCursor(format!("cursor is malformed: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn params(limit: u64, cursor: Option<&str>) -> ToolParameters {
        let mut parameters = ToolParameters::new();
        parameters.insert("limit".to_string(), json!(limit));
        if let Some(cursor) = cursor {
            parameters.insert("cursor".to_string(), json!(cursor));
        }
        parameters
    }

    #[test]
    fn pages_without_duplicates_and_preserves_unicode() -> Result<(), String> {
        let items = vec!["中文", "emoji🙂", "third", "fourth"];
        let first = PageRequest::from_parameters(&params(2, None), 2, 10)
            .map_err(|error| error.to_string())?;
        let (first_items, first_info) = first
            .paginate(items.clone(), &json!({"query": "same"}))
            .map_err(|error| error.to_string())?;
        assert_eq!(first_items, vec!["中文", "emoji🙂"]);
        let cursor = first_info
            .next_cursor
            .as_deref()
            .ok_or_else(|| "first page must have a cursor".to_string())?;
        let second = PageRequest::from_parameters(&params(2, Some(cursor)), 2, 10)
            .map_err(|error| error.to_string())?;
        let (second_items, second_info) = second
            .paginate(items, &json!({"query": "same"}))
            .map_err(|error| error.to_string())?;
        assert_eq!(second_items, vec!["third", "fourth"]);
        assert!(!second_info.truncated);
        assert_eq!(second_info.total, Some(4));
        Ok(())
    }

    #[test]
    fn rejects_cursor_after_query_or_limit_changes() -> Result<(), String> {
        let items = vec![1, 2, 3];
        let first = PageRequest::from_parameters(&params(1, None), 1, 10)
            .map_err(|error| error.to_string())?;
        let (_, info) = first
            .paginate(items.clone(), &json!({"query": "old"}))
            .map_err(|error| error.to_string())?;
        let cursor = info
            .next_cursor
            .as_deref()
            .ok_or_else(|| "first page must have a cursor".to_string())?;

        let changed_query = PageRequest::from_parameters(&params(1, Some(cursor)), 1, 10)
            .map_err(|error| error.to_string())?;
        assert_eq!(
            changed_query.paginate(items.clone(), &json!({"query": "new"})),
            Err(PageError::CursorQueryMismatch)
        );
        let changed_limit = PageRequest::from_parameters(&params(2, Some(cursor)), 1, 10)
            .map_err(|error| error.to_string())?;
        assert_eq!(
            changed_limit.paginate(items, &json!({"query": "old"})),
            Err(PageError::CursorQueryMismatch)
        );
        Ok(())
    }

    #[test]
    fn rejects_cursor_after_result_snapshot_changes() -> Result<(), String> {
        let first = PageRequest::from_parameters(&params(1, None), 1, 10)
            .map_err(|error| error.to_string())?;
        let (_, info) = first
            .paginate(vec!["first", "second"], &json!({"query": "same"}))
            .map_err(|error| error.to_string())?;
        let cursor = info
            .next_cursor
            .as_deref()
            .ok_or_else(|| "first page must have a cursor".to_string())?;
        let next = PageRequest::from_parameters(&params(1, Some(cursor)), 1, 10)
            .map_err(|error| error.to_string())?;

        assert_eq!(
            next.paginate(
                vec!["inserted", "first", "second"],
                &json!({"query": "same"})
            ),
            Err(PageError::CursorQueryMismatch)
        );
        Ok(())
    }

    #[test]
    fn applying_page_info_preserves_existing_truncation() {
        let mut result = ToolResult::success("preview").with_truncated(true);
        PageInfo {
            next_cursor: None,
            truncated: false,
            total_known: true,
            total: Some(1),
            returned: 1,
        }
        .apply_to(&mut result);

        assert!(result.truncated);
        assert_eq!(
            result.metadata.get("page.truncated").map(String::as_str),
            Some("false")
        );
    }
}
