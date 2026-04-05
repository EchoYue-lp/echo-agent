//! Critic trait — 评估 Agent 输出的质量

use crate::agents::self_reflection::types::Critique;
use crate::error::Result;
use futures::future::BoxFuture;

/// Critic trait — 评估 Agent 生成的回答
///
/// 与 `Planner` 对称，是 Self-Reflection 模式中可插拔的评估组件。
/// 实现 `Critic` 即可自定义评估策略。
pub trait Critic: Send + Sync {
    /// 评估一个回答
    ///
    /// - `task`: 原始任务描述
    /// - `answer`: Agent 生成的回答
    /// - `context`: 额外上下文（如情景记忆、已有结果等）
    fn critique<'a>(
        &'a self,
        task: &'a str,
        answer: &'a str,
        context: &'a str,
    ) -> BoxFuture<'a, Result<Critique>>;

    /// Critic 的可读名称
    fn name(&self) -> &str {
        "anonymous"
    }
}

// ── StaticCritic ────────────────────────────────────────────────────────────

/// 静态 Critic：始终返回固定结果（适用于测试）
pub struct StaticCritic {
    score: f64,
    passed: bool,
    feedback: String,
    suggestions: Vec<String>,
}

impl StaticCritic {
    pub fn new(score: f64, passed: bool, feedback: impl Into<String>) -> Self {
        Self {
            score,
            passed,
            feedback: feedback.into(),
            suggestions: Vec::new(),
        }
    }

    pub fn with_suggestions(mut self, suggestions: Vec<String>) -> Self {
        self.suggestions = suggestions;
        self
    }

    /// 创建一个始终通过的 Critic
    pub fn always_pass() -> Self {
        Self::new(9.0, true, "回答质量优秀")
    }

    /// 创建一个始终失败的 Critic
    pub fn always_fail() -> Self {
        Self::new(3.0, false, "回答不符合要求")
    }
}

impl Critic for StaticCritic {
    fn critique<'a>(
        &'a self,
        _task: &'a str,
        _answer: &'a str,
        _context: &'a str,
    ) -> BoxFuture<'a, Result<Critique>> {
        Box::pin(async move {
            Ok(Critique {
                score: self.score,
                passed: self.passed,
                feedback: self.feedback.clone(),
                suggestions: self.suggestions.clone(),
            })
        })
    }

    fn name(&self) -> &str {
        "static"
    }
}

// ── ThresholdCritic ──────────────────────────────────────────────────────────

/// 阈值 Critic：通过另一个 Critic 评估，然后根据阈值判断是否通过
///
/// 用于包装其他 Critic，覆盖其 `passed` 判定。
pub struct ThresholdCritic<C: Critic> {
    inner: C,
    threshold: f64,
}

impl<C: Critic> ThresholdCritic<C> {
    pub fn new(inner: C, threshold: f64) -> Self {
        Self { inner, threshold }
    }
}

impl<C: Critic> Critic for ThresholdCritic<C> {
    fn critique<'a>(
        &'a self,
        task: &'a str,
        answer: &'a str,
        context: &'a str,
    ) -> BoxFuture<'a, Result<Critique>> {
        Box::pin(async move {
            let mut critique = self.inner.critique(task, answer, context).await?;
            critique.passed = critique.score >= self.threshold;
            Ok(critique)
        })
    }

    fn name(&self) -> &str {
        self.inner.name()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_static_critic_always_pass() {
        let critic = StaticCritic::always_pass();
        let critique = critic.critique("task", "answer", "").await.unwrap();
        assert!(critique.passed);
        assert!(critique.score >= 8.0);
    }

    #[tokio::test]
    async fn test_static_critic_always_fail() {
        let critic = StaticCritic::always_fail();
        let critique = critic.critique("task", "answer", "").await.unwrap();
        assert!(!critique.passed);
    }

    #[tokio::test]
    async fn test_threshold_critic() {
        let inner = StaticCritic::new(6.0, true, "还行");
        let critic = ThresholdCritic::new(inner, 8.0);
        let critique = critic.critique("task", "answer", "").await.unwrap();
        assert!(!critique.passed); // 6.0 < 8.0
        assert_eq!(critique.score, 6.0);
    }

    #[tokio::test]
    async fn test_threshold_critic_passes() {
        let inner = StaticCritic::new(9.0, false, "很好");
        let critic = ThresholdCritic::new(inner, 8.0);
        let critique = critic.critique("task", "answer", "").await.unwrap();
        assert!(critique.passed); // 9.0 >= 8.0
    }
}
