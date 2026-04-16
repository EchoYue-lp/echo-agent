//! 组合评估器 — 聚合多个 Critic 的评估结果

use crate::agent::self_reflection::critic::Critic;
use crate::agent::self_reflection::types::Critique;
use crate::error::Result;
use futures::future::BoxFuture;
use tracing::{debug, info};

/// 组合策略
#[derive(Debug, Clone)]
pub enum CompositeStrategy {
    /// 所有 Critic 都必须通过（AND 逻辑）
    AllMustPass,
    /// 取平均分
    Average,
    /// 取最低分（悲观策略）
    Minimum,
    /// 加权平均
    Weighted(Vec<f64>),
}

/// 组合评估器：聚合多个 Critic
pub struct CompositeCritic {
    critics: Vec<Box<dyn Critic>>,
    strategy: CompositeStrategy,
    pass_threshold: f64,
}

impl CompositeCritic {
    pub fn new(strategy: CompositeStrategy) -> Self {
        Self {
            critics: Vec::new(),
            strategy,
            pass_threshold: 7.0,
        }
    }

    pub fn add_critic(mut self, critic: impl Critic + 'static) -> Self {
        self.critics.push(Box::new(critic));
        self
    }

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

            // 合并反馈
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
    use crate::agent::self_reflection::critic::StaticCritic;

    #[tokio::test]
    async fn test_composite_all_must_pass() {
        let critic = CompositeCritic::new(CompositeStrategy::AllMustPass)
            .add_critic(StaticCritic::new(9.0, true, "好"))
            .add_critic(StaticCritic::new(5.0, false, "差"));

        let result = critic.critique("task", "answer", "").await.unwrap();
        assert!(!result.passed); // 第二个未通过
        assert_eq!(result.score, 7.0); // (9 + 5) / 2
    }

    #[tokio::test]
    async fn test_composite_all_pass() {
        let critic = CompositeCritic::new(CompositeStrategy::AllMustPass)
            .add_critic(StaticCritic::new(9.0, true, "好"))
            .add_critic(StaticCritic::new(8.0, true, "也不错"));

        let result = critic.critique("task", "answer", "").await.unwrap();
        assert!(result.passed);
    }

    #[tokio::test]
    async fn test_composite_average() {
        let critic = CompositeCritic::new(CompositeStrategy::Average)
            .add_critic(StaticCritic::new(8.0, true, "好"))
            .add_critic(StaticCritic::new(6.0, false, "一般"));

        let result = critic.critique("task", "answer", "").await.unwrap();
        assert_eq!(result.score, 7.0);
        assert!(result.passed); // 7.0 >= 7.0
    }

    #[tokio::test]
    async fn test_composite_minimum() {
        let critic = CompositeCritic::new(CompositeStrategy::Minimum)
            .add_critic(StaticCritic::new(9.0, true, "好"))
            .add_critic(StaticCritic::new(4.0, false, "差"));

        let result = critic.critique("task", "answer", "").await.unwrap();
        assert_eq!(result.score, 4.0);
        assert!(!result.passed);
    }

    #[tokio::test]
    async fn test_composite_weighted() {
        let critic = CompositeCritic::new(CompositeStrategy::Weighted(vec![0.7, 0.3]))
            .add_critic(StaticCritic::new(10.0, true, "完美"))
            .add_critic(StaticCritic::new(0.0, false, "失败"));

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
