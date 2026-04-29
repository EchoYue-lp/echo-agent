//! Self-Reflection 引擎
//!
//! 在 ReAct 基础上引入"评估 → 反思 → 修正"闭环，通过语言反馈让 Agent 从错误中学习。
//!
//! # 三组件模型 + 情景记忆
//!
//! ```text
//! Actor(生成) → Evaluator(评估) → Reflector(反思) → 修正 → 循环
//!                     ↓                              ↓
//!               Episodic Memory（跨任务经验存储）
//! ```
//!
//! # 示例
//!
//! ```rust,no_run
//! use echo_agent::prelude::*;
//! use echo_agent::agent::self_reflection::{LlmCritic, SelfReflectionAgent};
//!
//! # #[tokio::main]
//! # async fn main() -> echo_agent::error::Result<()> {
//! let generator = ReactAgentBuilder::new()
//!     .model("qwen3-max")
//!     .name("writer")
//!     .system_prompt("你是一个技术文档撰写专家。")
//!     .build()?;
//!
//! let critic = LlmCritic::new("qwen3-max").with_pass_threshold(8.0);
//!
//! let mut agent = SelfReflectionAgent::new("reflection_agent", generator, critic)
//!     .max_reflections(3);
//!
//! let result = agent
//!     .execute("解释 Rust 所有权的核心概念，要求通俗易懂且准确。")
//!     .await?;
//!
//! println!("最终结果:\n{}", result);
//! # Ok(())
//! # }
//! ```

mod composite;
mod critic;
mod llm_critic;
mod store;
mod types;

pub use composite::{CompositeCritic, CompositeStrategy};
pub use critic::{Critic, StaticCritic, ThresholdCritic};
pub use llm_critic::LlmCritic;
pub use store::{InMemoryReflectionStore, ReflectionStore};
pub use types::{
    Critique, CritiqueOutput, DefaultRefinementPromptBuilder, DefaultReflectionPromptBuilder,
    RefinementPromptBuilder, ReflectionExperience, ReflectionPromptBuilder, ReflectionRecord,
    critique_output_schema,
};

use crate::agent::{Agent, AgentEvent};
use crate::error::Result;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use std::sync::{Arc, RwLock};
use tracing::{debug, info, warn};

#[cfg(feature = "plan-execute")]
use crate::agent::plan_execute::Executor;

/// Self-Reflection Agent
///
/// 三阶段循环：生成 → 评估 → 反思修正，配合情景记忆实现跨任务学习。
pub struct SelfReflectionAgent<C: Critic> {
    name: String,
    generator: Box<dyn Agent>,
    critic: C,
    max_reflections: usize,
    pass_threshold: f64,
    refinement_prompt_builder: Box<dyn RefinementPromptBuilder>,
    reflection_prompt_builder: Box<dyn ReflectionPromptBuilder>,
    episodic_memory: RwLock<std::collections::VecDeque<ReflectionExperience>>,
    memory_limit: usize,
    store: Option<Arc<dyn ReflectionStore>>,
    /// Pending records for batch persistence
    pending_records: RwLock<Vec<(String, Vec<ReflectionRecord>)>>,
}

impl<C: Critic> SelfReflectionAgent<C> {
    /// 创建 Self-Reflection Agent
    ///
    /// # 参数
    /// * `name` - Agent 名称（用于标识和日志记录）
    /// * `generator` - 生成器 Agent（如 ReactAgent），负责生成初始响应
    /// * `critic` - 评估器（Critic），负责评估生成质量并提供反馈
    ///
    /// # 默认配置
    /// * 最大反思次数：3
    /// * 通过阈值：7.0（评分 ≥ 7.0 视为通过）
    /// * 情景记忆容量：10 条经验
    pub fn new(name: impl Into<String>, generator: impl Agent + 'static, critic: C) -> Self {
        Self {
            name: name.into(),
            generator: Box::new(generator),
            critic,
            max_reflections: 3,
            pass_threshold: 7.0,
            refinement_prompt_builder: Box::new(DefaultRefinementPromptBuilder),
            reflection_prompt_builder: Box::new(DefaultReflectionPromptBuilder),
            episodic_memory: RwLock::new(std::collections::VecDeque::with_capacity(10)),
            memory_limit: 10,
            store: None,
            pending_records: RwLock::new(Vec::new()),
        }
    }

    /// 最大反思迭代次数（默认 3）
    pub fn max_reflections(mut self, n: usize) -> Self {
        self.max_reflections = n;
        self
    }

    /// 通过阈值（0.0 - 10.0，默认 7.0）
    pub fn pass_threshold(mut self, threshold: f64) -> Self {
        self.pass_threshold = threshold;
        self
    }

    /// 自定义修正提示词构建器
    pub fn refinement_prompt_builder(
        mut self,
        builder: impl RefinementPromptBuilder + 'static,
    ) -> Self {
        self.refinement_prompt_builder = Box::new(builder);
        self
    }

    /// 自定义反思提示词构建器
    pub fn reflection_prompt_builder(
        mut self,
        builder: impl ReflectionPromptBuilder + 'static,
    ) -> Self {
        self.reflection_prompt_builder = Box::new(builder);
        self
    }

    /// 设置情景记忆容量上限（默认 10）
    pub fn memory_limit(mut self, limit: usize) -> Self {
        self.memory_limit = limit;
        self
    }

    /// 设置持久化存储
    pub fn with_store(mut self, store: Arc<dyn ReflectionStore>) -> Self {
        self.store = Some(store);
        self
    }

    /// 核心执行循环
    async fn run_reflection_loop(&self, task: &str) -> Result<String> {
        let agent = self.name.clone();

        // ── 阶段 1: 生成初始响应 ──────────────────────────────────────
        info!(agent = %agent, "🎯 Self-Reflection: 生成初始响应");

        let memory_context = self.build_memory_context();
        let enhanced_task = if memory_context.is_empty() {
            task.to_string()
        } else {
            format!("{}\n\n参考以下过往经验教训：\n{}", task, memory_context)
        };

        let mut current_answer = self.generator.execute(&enhanced_task).await?;
        let mut records: Vec<ReflectionRecord> = Vec::new();

        // ── 阶段 2: 评估 → 反思 → 修正循环 ──────────────────────────
        for iteration in 0..self.max_reflections {
            info!(
                agent = %agent,
                iteration = iteration + 1,
                max = self.max_reflections,
                "🔍 Self-Reflection: 第 {}/{} 轮评估",
                iteration + 1,
                self.max_reflections
            );

            // 评估
            let context = self.build_critique_context(&records);
            let critique = self
                .critic
                .critique(task, &current_answer, &context)
                .await?;

            info!(
                agent = %agent,
                score = critique.score,
                passed = critique.passed,
                "📊 评估结果: {:.1}/10.0 ({})",
                critique.score,
                if critique.passed { "通过" } else { "未通过" }
            );

            // 通过质量阈值
            if critique.passed && critique.score >= self.pass_threshold {
                records.push(ReflectionRecord {
                    iteration,
                    answer: current_answer.clone(),
                    critique: critique.clone(),
                    reflection_text: String::new(),
                    refined_answer: None,
                });

                info!(agent = %agent, "✅ Self-Reflection: 评估通过");

                // 存储成功经验
                self.persist_records(task, &records).await;
                return Ok(current_answer);
            }

            // 反思：分析失败原因
            let reflection_prompt = self.reflection_prompt_builder.build_reflection_prompt(
                task,
                &current_answer,
                &critique,
            );

            let reflection_text = self.generator.execute(&reflection_prompt).await?;
            debug!(agent = %agent, reflection = %reflection_text, "💡 反思文本");

            // 构建修正提示词
            let refinement_prompt = self.refinement_prompt_builder.build_prompt(
                task,
                &current_answer,
                &critique,
                &reflection_text,
                iteration,
            );

            // 修正
            info!(agent = %agent, iteration = iteration + 1, "🔧 Self-Reflection: 修正回答");
            let refined = self.generator.execute(&refinement_prompt).await?;

            records.push(ReflectionRecord {
                iteration,
                answer: current_answer.clone(),
                critique,
                reflection_text,
                refined_answer: Some(refined.clone()),
            });

            current_answer = refined;

            // 提取并存储经验教训
            self.extract_experience(&records);
        }

        info!(
            agent = %agent,
            "🏁 Self-Reflection: 达到最大反思次数 {}",
            self.max_reflections
        );

        self.persist_records(task, &records).await;
        Ok(current_answer)
    }

    /// 构建情景记忆上下文文本
    fn build_memory_context(&self) -> String {
        if self.episodic_memory.read().unwrap().is_empty() {
            return String::new();
        }

        self.episodic_memory
            .read()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(i, exp)| format!("{}. {}", i + 1, exp.lesson))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 构建评估上下文（之前轮次的记录）
    fn build_critique_context(&self, records: &[ReflectionRecord]) -> String {
        if records.is_empty() {
            return String::new();
        }

        let mut parts = vec!["之前轮次的评估记录：".to_string()];
        for r in records {
            parts.push(format!(
                "  轮次 {}: 评分 {:.1} — {}",
                r.iteration + 1,
                r.critique.score,
                r.critique.feedback
            ));
        }
        parts.join("\n")
    }

    /// 从反思记录中提取经验教训
    fn extract_experience(&self, records: &[ReflectionRecord]) {
        for r in records {
            if r.critique.passed {
                continue;
            }
            // 从反馈中提取简短教训
            let feedback = &r.critique.feedback;
            let lesson = if feedback.len() > 100 {
                // 使用 char_indices 安全截断
                let end = feedback
                    .char_indices()
                    .take_while(|(idx, _)| *idx < 100)
                    .last()
                    .map(|(idx, c)| idx + c.len_utf8())
                    .unwrap_or(0);
                format!("{}...", &feedback[..end])
            } else {
                feedback.clone()
            };

            let error_pattern = r
                .critique
                .suggestions
                .first()
                .cloned()
                .unwrap_or_else(|| "未识别具体错误模式".to_string());

            // 去重：如果已有相似经验则增加引用计数
            let _found = {
                let mut memory = self.episodic_memory.write().unwrap();
                let similar = memory.iter_mut().find(|e| e.lesson == lesson);
                if let Some(existing) = similar {
                    existing.use_count += 1;
                    true
                } else {
                    // If at capacity, remove the entry with the lowest use_count
                    if memory.len() >= self.memory_limit
                        && let Some((min_idx, _)) =
                            memory.iter().enumerate().min_by_key(|(_, e)| e.use_count)
                    {
                        memory.remove(min_idx);
                    }
                    memory.push_back(ReflectionExperience::new(lesson, error_pattern));
                    false
                }
            };
        }
    }

    /// 持久化记录 — 缓冲后批量写入
    ///
    /// 将记录添加到待写入缓冲区，当缓冲区大小达到阈值时自动 flush。
    /// 这减少了频繁的 I/O 操作，提升性能。
    async fn persist_records(&self, task: &str, records: &[ReflectionRecord]) {
        if self.store.is_none() {
            return;
        }

        // Buffer the records
        if !records.is_empty() {
            self.pending_records
                .write()
                .unwrap()
                .push((task.to_string(), records.to_vec()));
        }

        // Flush when buffer exceeds threshold (batch size of 5)
        if self.pending_records.read().unwrap().len() >= 5 {
            self.flush_pending_records().await;
        }
    }

    /// 立即刷新所有待写入的记录
    pub async fn flush_pending_records(&self) {
        if self.pending_records.read().unwrap().is_empty() {
            return;
        }

        if let Some(ref store) = self.store {
            // Save all pending reflection records
            let pending: Vec<_> = self.pending_records.write().unwrap().drain(..).collect();
            for (task, records) in pending {
                if let Err(e) = store.save_reflections(&task, &records).await {
                    warn!(error = %e, task = %task, "Failed to persist reflection records");
                }
            }

            // Save experiences (convert VecDeque to Vec for slice reference)
            let experiences: Vec<_> = self
                .episodic_memory
                .read()
                .unwrap()
                .iter()
                .cloned()
                .collect();
            if let Err(e) = store.save_experiences(&experiences).await {
                warn!(error = %e, "Failed to persist experiences");
            }
        }
    }
}

// ── impl Agent ───────────────────────────────────────────────────────────────

impl<C: Critic + Send + Sync> Agent for SelfReflectionAgent<C> {
    fn name(&self) -> &str {
        &self.name
    }

    fn model_name(&self) -> &str {
        "self-reflection"
    }

    fn system_prompt(&self) -> &str {
        ""
    }

    fn execute<'a>(&'a self, task: &'a str) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move { self.run_reflection_loop(task).await })
    }

    fn execute_stream<'a>(
        &'a self,
        task: &'a str,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        Box::pin(async move {
            let task_owned = task.to_string();
            let stream = async_stream::try_stream! {
                let agent = self.name.clone();

                // ── Phase 1: 生成初始响应 ──
                info!(agent = %agent, "🎯 Self-Reflection (stream): 生成初始响应");
                let memory_context = self.build_memory_context();
                let enhanced_task = if memory_context.is_empty() {
                    task_owned.clone()
                } else {
                    format!("{}\n\n参考以下过往经验教训：\n{}", task_owned, memory_context)
                };

                let mut current_answer = self.generator.execute(&enhanced_task).await?;
                let mut records: Vec<ReflectionRecord> = Vec::new();

                // ── Phase 2: 评估 → 反思 → 修正循环 ──
                for iteration in 0..self.max_reflections {
                    yield AgentEvent::ReflectionStart { iteration };

                    let context = self.build_critique_context(&records);
                    let critique = self.critic.critique(
                        &task_owned,
                        &current_answer,
                        &context,
                    ).await?;

                    yield AgentEvent::CritiqueGenerated {
                        score: critique.score,
                        passed: critique.passed,
                        feedback: critique.feedback.clone(),
                    };

                    // 通过
                    if critique.passed && critique.score >= self.pass_threshold {
                        let score = critique.score;
                        records.push(ReflectionRecord {
                            iteration,
                            answer: current_answer.clone(),
                            critique,
                            reflection_text: String::new(),
                            refined_answer: None,
                        });

                        yield AgentEvent::ReflectionEnd {
                            iteration,
                            score,
                            passed: true,
                        };

                        self.extract_experience(&records);
                        self.persist_records(&task_owned, &records).await;
                        yield AgentEvent::FinalAnswer(current_answer);
                        return;
                    }

                    // 反思
                    let reflection_prompt = self.reflection_prompt_builder
                        .build_reflection_prompt(&task_owned, &current_answer, &critique);
                    let reflection_text = self.generator.execute(&reflection_prompt).await?;

                    // 修正
                    yield AgentEvent::Refining { iteration };
                    let refinement_prompt = self.refinement_prompt_builder.build_prompt(
                        &task_owned,
                        &current_answer,
                        &critique,
                        &reflection_text,
                        iteration,
                    );
                    let refined = self.generator.execute(&refinement_prompt).await?;

                    records.push(ReflectionRecord {
                        iteration,
                        answer: current_answer.clone(),
                        critique,
                        reflection_text,
                        refined_answer: Some(refined.clone()),
                    });

                    yield AgentEvent::ReflectionEnd {
                        iteration,
                        score: records.last().map(|r| r.critique.score).unwrap_or(0.0),
                        passed: false,
                    };

                    current_answer = refined;
                }

                self.extract_experience(&records);
                self.persist_records(&task_owned, &records).await;
                yield AgentEvent::FinalAnswer(current_answer);
            };
            Ok(Box::pin(stream) as BoxStream<'a, Result<AgentEvent>>)
        })
    }

    fn reset(&self) {
        self.generator.reset();
        self.episodic_memory.write().unwrap().clear();
    }
}

// ── ReflectiveExecutor ──────────────────────────────────────────────────────

#[cfg(feature = "plan-execute")]
/// 反思执行器：将 Self-Reflection 作为 Plan-and-Execute 的 Executor 使用
///
/// 每个 Plan 步骤都经过"生成 → 评估 → 修正"闭环。
///
/// # 示例
///
/// ```rust,no_run
/// use echo_agent::prelude::*;
/// use echo_agent::advanced::{LlmPlanner, PlanExecuteAgent};
/// use echo_agent::agent::self_reflection::{LlmCritic, SelfReflectionAgent, ReflectiveExecutor};
///
/// # #[tokio::main]
/// # async fn main() -> echo_agent::error::Result<()> {
/// let generator = ReactAgentBuilder::new()
///     .model("qwen3-max")
///     .name("step_executor")
///     .system_prompt("你是任务执行助手")
///     .build()?;
///
/// let critic = LlmCritic::new("qwen3-max");
/// let reflective_agent = SelfReflectionAgent::new("reflective", generator, critic)
///     .max_reflections(2);
///
/// let executor = ReflectiveExecutor::new(reflective_agent);
///
/// let planner = LlmPlanner::new("qwen3-max");
/// let mut agent = PlanExecuteAgent::new("plan_agent", planner, executor);
/// let result = agent.execute("分析并优化代码性能").await?;
/// # Ok(())
/// # }
/// ```
pub struct ReflectiveExecutor {
    agent: SelfReflectionAgent<LlmCritic>,
}

#[cfg(feature = "plan-execute")]
impl ReflectiveExecutor {
    /// 创建 ReflectiveExecutor
    ///
    /// # 参数
    /// * `agent` - 已配置好的 SelfReflectionAgent（需使用 LlmCritic 作为评估器）
    ///
    /// # 说明
    /// 用于将 Self-Reflection Agent 适配为 Plan-and-Execute 架构中的 Executor，
    /// 使其能够作为 PlanStep 的执行器使用。
    pub fn new(agent: SelfReflectionAgent<LlmCritic>) -> Self {
        Self { agent }
    }

    /// 使用默认配置快速创建
    pub fn simple(model: &str, system_prompt: &str) -> Result<Self> {
        let generator = crate::agent::ReactAgentBuilder::new()
            .model(model)
            .name("reflective_executor")
            .system_prompt(system_prompt)
            .build()?;
        let critic = LlmCritic::new(model);
        let agent = SelfReflectionAgent::new("reflective", generator, critic).max_reflections(2);
        Ok(Self { agent })
    }
}

#[cfg(feature = "plan-execute")]
impl Executor for ReflectiveExecutor {
    fn execute_step<'a>(
        &'a mut self,
        step_description: &'a str,
        context: &'a str,
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let task = if context.is_empty() {
                step_description.to_string()
            } else {
                format!("{}\n\n{}", context, step_description)
            };

            info!(
                agent = %self.agent.name(),
                step = %step_description,
                "⚡ ReflectiveExecutor 执行步骤（含反思）"
            );

            self.agent.execute(&task).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_self_reflection_passes_immediately() {
        let generator = crate::testing::MockAgent::new("mock").with_response("这是回答");
        let critic = StaticCritic::always_pass();

        let agent = SelfReflectionAgent::new("test", generator, critic).max_reflections(3);

        let result = agent.execute("测试任务").await.unwrap();
        assert_eq!(result, "这是回答");
    }

    #[tokio::test]
    async fn test_self_reflection_always_fails() {
        // MockAgent 有多个响应，用于生成 + 反思 + 修正
        let generator = crate::testing::MockAgent::new("mock").with_responses([
            "answer1",
            "reflection",
            "refined",
        ]);
        let critic = StaticCritic::always_fail();

        let agent = SelfReflectionAgent::new("test", generator, critic).max_reflections(2);

        let result = agent.execute("测试任务").await.unwrap();
        // 即使始终失败也返回最后的回答
        assert!(!result.is_empty());
    }

    #[tokio::test]
    async fn test_self_reflection_reset() {
        let generator = crate::testing::MockAgent::new("mock").with_response("answer");
        let critic = StaticCritic::always_pass();

        let agent = SelfReflectionAgent::new("test", generator, critic);

        // 先执行一次积累经验
        agent.execute("任务1").await.unwrap();

        // 重置
        agent.reset();
        assert!(agent.episodic_memory.read().unwrap().is_empty());
    }

    #[test]
    fn test_agent_name() {
        let generator = crate::testing::MockAgent::new("mock").with_response("answer");
        let critic = StaticCritic::always_pass();
        let agent = SelfReflectionAgent::new("my_agent", generator, critic);
        assert_eq!(agent.name(), "my_agent");
        assert_eq!(agent.model_name(), "self-reflection");
    }
}
