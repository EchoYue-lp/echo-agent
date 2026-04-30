//! Timestamp utility functions
//!
//! Provides unified, safe timestamp retrieval, avoiding `unwrap()` panics.

/// Get the current Unix timestamp in seconds, returns 0 on failure.
///
/// Replaces scattered `SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()` calls.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Get the current Unix timestamp in milliseconds, returns 0 on failure.
pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_now_secs_reasonable() {
        let ts = now_secs();
        // Should be after 2024-01-01 and before 2100-01-01
        assert!(ts > 1_704_067_200, "timestamp should be after 2024");
        assert!(ts < 4_102_444_800, "timestamp should be before 2100");
    }

    #[test]
    fn test_now_millis_monotonic() {
        let a = now_millis();
        let b = now_millis();
        assert!(b >= a);
    }
}
