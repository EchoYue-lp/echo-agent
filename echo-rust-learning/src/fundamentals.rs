//! Values, scalar and compound types, expressions, control flow, and slices.

/// A typed summary demonstrates integer accumulation and floating-point output.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScoreSummary {
    pub minimum: u32,
    pub maximum: u32,
    pub total: u64,
    pub average: f64,
}

/// Summarize a borrowed slice. An empty slice has no meaningful minimum.
pub fn summarize_scores(scores: &[u32]) -> Option<ScoreSummary> {
    let (&first, rest) = scores.split_first()?;
    let mut minimum = first;
    let mut maximum = first;
    let mut total = u64::from(first);

    for &score in rest {
        minimum = minimum.min(score);
        maximum = maximum.max(score);
        total = total.checked_add(u64::from(score))?;
    }

    Some(ScoreSummary {
        minimum,
        maximum,
        total,
        average: total as f64 / scores.len() as f64,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptDecision {
    Start,
    Retry { remaining: u32 },
    Stop,
}

/// `if` is an expression: every branch produces the returned enum value.
pub fn decide_attempt(attempt: u32, max_attempts: u32) -> AttemptDecision {
    if max_attempts == 0 || attempt >= max_attempts {
        AttemptDecision::Stop
    } else if attempt == 0 {
        AttemptDecision::Start
    } else {
        AttemptDecision::Retry {
            remaining: max_attempts.saturating_sub(attempt),
        }
    }
}

/// Arrays have fixed length; a slice borrows any contiguous sequence.
pub fn safe_item<T>(items: &[T], index: usize) -> Option<&T> {
    items.get(index)
}

/// Tuples group a fixed set of values with potentially different meanings.
pub fn swap_coordinates((x, y): (i32, i32)) -> (i32, i32) {
    (y, x)
}

/// A range plus iterator adapters is often clearer than a mutable while loop.
pub fn countdown(start: u32) -> Vec<u32> {
    (1..=start).rev().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarizes_non_empty_slice() {
        let summary = summarize_scores(&[10, 20, 30]);
        assert_eq!(summary.map(|value| value.total), Some(60));
        assert_eq!(summary.map(|value| value.average), Some(20.0));
        assert_eq!(summarize_scores(&[]), None);
    }

    #[test]
    fn control_flow_returns_domain_values() {
        assert_eq!(decide_attempt(0, 3), AttemptDecision::Start);
        assert_eq!(
            decide_attempt(1, 3),
            AttemptDecision::Retry { remaining: 2 }
        );
        assert_eq!(decide_attempt(3, 3), AttemptDecision::Stop);
    }

    #[test]
    fn slice_access_never_indexes_out_of_bounds() {
        let values = ["one", "two"];
        assert_eq!(safe_item(&values, 1), Some(&"two"));
        assert_eq!(safe_item(&values, 2), None);
    }
}
