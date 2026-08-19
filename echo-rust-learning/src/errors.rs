//! `Option`, `Result`, error conversion, and the `?` operator.

use thiserror::Error;

/// Errors shared by the learning examples.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum LearningError {
    #[error("task title cannot be empty")]
    EmptyTitle,
    #[error("limit must be a positive integer, got {0}")]
    InvalidLimit(String),
    #[error("task already exists: {0}")]
    DuplicateTask(String),
    #[error("required field is missing: {0}")]
    MissingField(&'static str),
    #[error("shared state is temporarily borrowed")]
    BorrowConflict,
    #[error("shared state lock was poisoned")]
    PoisonedLock,
    #[error("subagent task failed: {0}")]
    SubagentJoin(String),
    #[error("channel closed before the result was delivered")]
    ChannelClosed,
    #[error("operation was cancelled")]
    Cancelled,
    #[error("operation timed out after {0} ms")]
    TimedOut(u64),
}

/// Parse a strictly positive limit without panicking on malformed input.
pub fn parse_positive_limit(input: &str) -> Result<usize, LearningError> {
    let value = input
        .parse::<usize>()
        .map_err(|_| LearningError::InvalidLimit(input.to_string()))?;
    if value == 0 {
        return Err(LearningError::InvalidLimit(input.to_string()));
    }
    Ok(value)
}

/// Convert an optional borrowed field into a validated owned value.
pub fn required_text(field: &'static str, value: Option<&str>) -> Result<String, LearningError> {
    let value = value.ok_or(LearningError::MissingField(field))?.trim();
    if value.is_empty() {
        return Err(LearningError::MissingField(field));
    }
    Ok(value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_positive_limit() {
        assert_eq!(parse_positive_limit("3"), Ok(3));
        assert!(parse_positive_limit("0").is_err());
        assert!(parse_positive_limit("三").is_err());
    }

    #[test]
    fn validates_required_text() {
        assert_eq!(
            required_text("name", Some(" Echo ")),
            Ok("Echo".to_string())
        );
        assert_eq!(
            required_text("name", None),
            Err(LearningError::MissingField("name"))
        );
    }
}
