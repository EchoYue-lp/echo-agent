//! Composite evaluator — aggregates evaluation results from multiple Critics

use super::Critic;
use crate::agent::types::Critique;
use crate::error::Result;
use futures::future::BoxFuture;
use tracing::{debug, info};

/// Composite strategy
#[derive(Debug, Clone)]
pub enum CompositeStrategy {
    /// All Critics must pass (AND logic)
    AllMustPass,
    /// Average score
    Average,
    /// Minimum score (pessimistic strategy)
    Minimum,
    /// Weighted average
    Weighted(Vec<f64>),
}

/// Composite evaluator: aggregates multiple Critics
pub struct CompositeCritic {
    critics: Vec<Box<dyn Critic>>,
    strategy: CompositeStrategy,
    pass_threshold: f64,
}

impl CompositeCritic {
    /// Create a composite evaluator
    ///
    /// # Parameters
    /// * `strategy` - Composite strategy, determines how to aggregate evaluation results from multiple Critics
    ///
    /// # Default configuration
    /// * Pass threshold: 7.0
    /// * Initial Critic list is empty (add via `add_critic`)
    pub fn new(strategy: CompositeStrategy) -> Self {
        Self {
            critics: Vec::new(),
            strategy,
            pass_threshold: 7.0,
        }
    }

    /// Add an evaluator to the composite
    ///
    /// # Parameters
    /// * `critic` - Evaluator instance to add, must implement `Critic` trait
    ///
    /// # Description
    /// Can be chained multiple times to add multiple evaluators.
    pub fn add_critic(mut self, critic: impl Critic + 'static) -> Self {
        self.critics.push(Box::new(critic));
        self
    }

    /// Set pass threshold
    ///
    /// # Parameters
    /// * `threshold` - Minimum score for evaluation to pass (range 0.0-10.0)
    ///
    /// # Description
    /// For `CompositeStrategy::Average`, `Minimum`, `Weighted` strategies,
    /// the aggregated score >= threshold means the evaluation passes.
    /// For `CompositeStrategy::AllMustPass`, the threshold only affects score display,
    /// the actual pass logic requires all sub-evaluators to pass.
    pub fn with_pass_threshold(mut self, threshold: f64) -> Self {
        self.pass_threshold = threshold;
        self
    }
}

impl Critic for CompositeCritic {
    fn critique<'a>(
        &'a self,
        task: &'a str,
        answer: &'a str,
        context: &'a str,
    ) -> BoxFuture<'a, Result<Critique>> {
        Box::pin(async move {
            let mut critiques = Vec::with_capacity(self.critics.len());
            for critic in &self.critics {
                let c = critic.critique(task, answer, context).await?;
                debug!(
                    critic = critic.name(),
                    score = c.score,
                    "CompositeCritic sub-result"
                );
                critiques.push(c);
            }

            if critiques.is_empty() {
                return Ok(Critique {
                    score: 0.0,
                    passed: false,
                    feedback: "No critics configured".to_string(),
                    suggestions: vec![],
                });
            }

            let (score, passed) = match &self.strategy {
                CompositeStrategy::AllMustPass => {
                    let all_passed = critiques.iter().all(|c| c.passed);
                    let avg: f64 =
                        critiques.iter().map(|c| c.score).sum::<f64>() / critiques.len() as f64;
                    (avg, all_passed)
                }
                CompositeStrategy::Average => {
                    let avg: f64 =
                        critiques.iter().map(|c| c.score).sum::<f64>() / critiques.len() as f64;
                    (avg, avg >= self.pass_threshold)
                }
                CompositeStrategy::Minimum => {
                    let min = critiques
                        .iter()
                        .map(|c| c.score)
                        .fold(f64::INFINITY, f64::min);
                    (min, min >= self.pass_threshold)
                }
                CompositeStrategy::Weighted(weights) => {
                    let total_weight: f64 = weights.iter().take(critiques.len()).sum();
                    let weighted_sum: f64 = critiques
                        .iter()
                        .enumerate()
                        .map(|(i, c)| c.score * weights.get(i).copied().unwrap_or(1.0))
                        .sum();
                    let avg = if total_weight > 0.0 {
                        weighted_sum / total_weight
                    } else {
                        0.0
                    };
                    (avg, avg >= self.pass_threshold)
                }
            };

            // Merge feedback
            let feedback = critiques
                .iter()
                .map(|c| format!("[{}] {}", c.score, c.feedback))
                .collect::<Vec<_>>()
                .join("\n");

            let suggestions: Vec<String> = critiques
                .iter()
                .flat_map(|c| c.suggestions.clone())
                .collect();

            info!(
                strategy = ?self.strategy,
                score = score,
                passed = passed,
                "CompositeCritic: aggregated result"
            );

            Ok(Critique {
                score,
                passed,
                feedback,
                suggestions,
            })
        })
    }

    fn name(&self) -> &str {
        "composite"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::StaticCritic;

    #[tokio::test]
    async fn test_composite_all_must_pass() {
        let critic = CompositeCritic::new(CompositeStrategy::AllMustPass)
            .add_critic(StaticCritic::new(9.0, true, "Good"))
            .add_critic(StaticCritic::new(5.0, false, "Bad"));

        let result = critic.critique("task", "answer", "").await.unwrap();
        assert!(!result.passed); // Second one did not pass
        assert_eq!(result.score, 7.0); // (9 + 5) / 2
    }

    #[tokio::test]
    async fn test_composite_all_pass() {
        let critic = CompositeCritic::new(CompositeStrategy::AllMustPass)
            .add_critic(StaticCritic::new(9.0, true, "Good"))
            .add_critic(StaticCritic::new(8.0, true, "Also good"));

        let result = critic.critique("task", "answer", "").await.unwrap();
        assert!(result.passed);
    }

    #[tokio::test]
    async fn test_composite_average() {
        let critic = CompositeCritic::new(CompositeStrategy::Average)
            .add_critic(StaticCritic::new(8.0, true, "Good"))
            .add_critic(StaticCritic::new(6.0, false, "Mediocre"));

        let result = critic.critique("task", "answer", "").await.unwrap();
        assert_eq!(result.score, 7.0);
        assert!(result.passed); // 7.0 >= 7.0
    }

    #[tokio::test]
    async fn test_composite_minimum() {
        let critic = CompositeCritic::new(CompositeStrategy::Minimum)
            .add_critic(StaticCritic::new(9.0, true, "Good"))
            .add_critic(StaticCritic::new(4.0, false, "Bad"));

        let result = critic.critique("task", "answer", "").await.unwrap();
        assert_eq!(result.score, 4.0);
        assert!(!result.passed);
    }

    #[tokio::test]
    async fn test_composite_weighted() {
        let critic = CompositeCritic::new(CompositeStrategy::Weighted(vec![0.7, 0.3]))
            .add_critic(StaticCritic::new(10.0, true, "Perfect"))
            .add_critic(StaticCritic::new(0.0, false, "Failed"));

        let result = critic.critique("task", "answer", "").await.unwrap();
        // (10.0 * 0.7 + 0.0 * 0.3) / (0.7 + 0.3) = 7.0
        assert!((result.score - 7.0).abs() < 0.01);
    }

    #[tokio::test]
    async fn test_composite_empty() {
        let critic = CompositeCritic::new(CompositeStrategy::Average);
        let result = critic.critique("task", "answer", "").await.unwrap();
        assert!(!result.passed);
    }
}
