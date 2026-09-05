//! Feature-neutral quantile helper shared by `statistics` and `data_quality`.
//!
//! `data_quality` (gated on `data`) needs the same linear-interpolation
//! quantile the `statistics` tool uses, while Cargo features declare
//! `statistics = ["data"]` — the dependency points the wrong way for a
//! `crate::statistics` call. Living outside both feature gates, this module
//! keeps `--no-default-features --features data` compiling without inverting
//! the feature graph or duplicating the math.

/// Linear-interpolation quantile of pre-sorted-or-unsorted values; the caller
/// sorts when order matters. Returns `None` for empty input. Probability is
/// clamped to `[0, 1]`; indexing stays bounds-checked.
pub(crate) fn quantile(values: &[f64], probability: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let bounded_probability = probability.clamp(0.0, 1.0);
    let max_index = values.len().saturating_sub(1);
    let position = bounded_probability * max_index as f64;
    let lower_index = position.floor() as usize;
    let upper_index = position.ceil() as usize;
    let lower = values.get(lower_index).copied()?;
    let upper = values.get(upper_index).copied()?;
    let fraction = position - lower_index as f64;
    Some(lower + (upper - lower) * fraction)
}

#[cfg(test)]
mod tests {
    use super::quantile;

    #[test]
    fn quantiles_use_linear_interpolation() {
        let values = [1.0, 2.0, 3.0, 4.0];
        assert_eq!(quantile(&values, 0.25), Some(1.75));
        assert_eq!(quantile(&values, 0.5), Some(2.5));
        assert_eq!(quantile(&values, 0.75), Some(3.25));
    }

    #[test]
    fn quantile_bounds_and_empty() {
        let values = [1.0, 2.0];
        assert_eq!(quantile(&values, 0.0), Some(1.0));
        assert_eq!(quantile(&values, 1.0), Some(2.0));
        assert_eq!(quantile(&values, -1.0), Some(1.0));
        assert_eq!(quantile(&values, 2.0), Some(2.0));
        let empty: [f64; 0] = [];
        assert_eq!(quantile(&empty, 0.5), None);
    }
}
