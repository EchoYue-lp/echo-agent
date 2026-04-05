//! 时间戳工具函数
//!
//! 提供统一、安全的时间戳获取，避免 `unwrap()` panic。

/// 获取当前 Unix 时间戳（秒），失败时返回 0。
///
/// 替代散落在各处的 `SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()`。
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// 获取当前 Unix 时间戳（毫秒），失败时返回 0。
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
