//! Memory decay — importance-weighted forgetting for long-term memory.
//!
//! Low-importance memories that haven't been accessed in a long time
//! have their effective score reduced via exponential decay.
//!
//! ## Formula
//!
//! ```text
//! effective_score = importance * e^(-λ * days_since_access)
//! where λ = 0.05 (half-life ≈ 14 days)
//! ```

use super::store::StoreItem;
use crate::utils::time;

/// Default decay rate λ. A value of 0.05 gives a half-life of ~14 days.
pub const DEFAULT_DECAY_LAMBDA: f64 = 0.05;

/// Decay threshold: items with effective_score below this are candidates for pruning.
pub const PRUNE_THRESHOLD: f32 = 1.0;

/// Compute the decayed importance score for a memory item.
///
/// Returns `importance` unchanged if `last_accessed` is `None` (never accessed).
pub fn decay_score(item: &StoreItem) -> f32 {
    decay_score_with_lambda(item, DEFAULT_DECAY_LAMBDA)
}

/// Compute the decayed score with a custom decay rate.
pub fn decay_score_with_lambda(item: &StoreItem, lambda: f64) -> f32 {
    let Some(last_access) = item.last_accessed else {
        return item.importance;
    };

    let now = time::now_secs();
    let days_since = (now.saturating_sub(last_access)) as f64 / 86400.0;
    let decay = (-lambda * days_since).exp();
    (item.importance as f64 * decay) as f32
}

/// Whether an item should be pruned based on its decayed score.
pub fn should_prune(item: &StoreItem) -> bool {
    decay_score(item) < PRUNE_THRESHOLD
}

/// Sort items by decayed score descending, truncating to `limit`.
pub fn sort_by_decayed_score(items: &mut Vec<StoreItem>, limit: usize) {
    items.sort_by(|a, b| {
        decay_score(b)
            .partial_cmp(&decay_score(a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    items.truncate(limit);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::store::StoreItem;
    use serde_json::json;

    #[test]
    fn test_no_access_no_decay() {
        let item = StoreItem::new(
            vec!["test".into()],
            "k1".into(),
            json!({"content": "test"}),
        );
        assert_eq!(decay_score(&item), 5.0); // default importance
    }

    #[test]
    fn test_recent_no_decay() {
        let now = time::now_secs();
        let item = StoreItem {
            last_accessed: Some(now), // just now
            importance: 10.0,
            ..StoreItem::new(vec!["t".into()], "k".into(), json!({}))
        };
        let score = decay_score(&item);
        assert!(score > 9.9); // barely any decay
    }

    #[test]
    fn test_old_access_decays() {
        let now = time::now_secs();
        let thirty_days = now - 30 * 86400;
        let item = StoreItem {
            last_accessed: Some(thirty_days),
            importance: 10.0,
            ..StoreItem::new(vec!["t".into()], "k".into(), json!({}))
        };
        let score = decay_score(&item);
        // e^(-0.05 * 30) ≈ 0.223, so score ≈ 2.23
        assert!(score < 3.0 && score > 1.5);
    }

    #[test]
    fn test_should_prune() {
        let now = time::now_secs();
        let old = now - 90 * 86400; // 90 days ago
        let item = StoreItem {
            last_accessed: Some(old),
            importance: 5.0,
            ..StoreItem::new(vec!["t".into()], "k".into(), json!({}))
        };
        // e^(-0.05 * 90) ≈ 0.011, score ≈ 0.055
        assert!(should_prune(&item));
    }
}
