//! Self-Reflection engine
//!
//! Introduces an "Evaluate → Reflect → Refine" closed loop on top of ReAct,
//! enabling the Agent to learn from mistakes through linguistic feedback.
//!
//! # Three-component model + Episodic Memory
//!
//! ```text
//! Actor(generate) → Evaluator(evaluate) → Reflector(reflect) → Refine → loop
//!                         ↓                                ↓
//!               Episodic Memory (cross-task experience storage)
//! ```
//!
//! # Example
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
//!     .system_prompt("You are a technical documentation expert.")
//!     .build()?;
//!
//! let critic = LlmCritic::new("qwen3-max").with_pass_threshold(8.0);
//!
//! let mut agent = SelfReflectionAgent::new("reflection_agent", generator, critic)
//!     .max_reflections(3);
//!
//! let result = agent
//!     .execute("Explain the core concepts of Rust ownership — make it accessible and accurate.")
//!     .await?;
//!
//! println!("Final result:\n{}", result);
//! # Ok(())
//! # }
//! ```

mod llm_critic;

pub use llm_critic::LlmCritic;

// Re-export core types from echo_core for convenience
pub use echo_core::agent::{
    CompositeCritic, CompositeStrategy, Critic, Critique, CritiqueOutput, InMemoryReflectionStore,
    ReflectionExperience, ReflectionRecord, ReflectionStore, StaticCritic, ThresholdCritic,
    critique_output_schema, default_refinement_prompt, default_reflection_prompt,
};

use crate::agent::{Agent, AgentEvent};
use crate::error::Result;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use std::sync::{Arc, RwLock};
use tracing::{debug, info, warn};

/// Type alias for refinement prompt builder closures
type RefinementPromptFn = Box<dyn Fn(&str, &str, &Critique, &str, usize) -> String + Send + Sync>;
/// Type alias for reflection prompt builder closures
type ReflectionPromptFn = Box<dyn Fn(&str, &str, &Critique) -> String + Send + Sync>;

#[cfg(feature = "plan-execute")]
use echo_core::agent::Executor;

/// Self-Reflection Agent
///
/// Three-phase loop: Generate → Evaluate → Reflect & Refine, with episodic memory for cross-task learning.
pub struct SelfReflectionAgent {
    name: String,
    generator: Box<dyn Agent>,
    critic: Box<dyn Critic>,
    max_reflections: usize,
    pass_threshold: f64,
    refinement_prompt_builder: RefinementPromptFn,
    reflection_prompt_builder: ReflectionPromptFn,
    episodic_memory: RwLock<std::collections::VecDeque<ReflectionExperience>>,
    memory_limit: usize,
    store: Option<Arc<dyn ReflectionStore>>,
    /// Pending records for batch persistence
    pending_records: RwLock<Vec<(String, Vec<ReflectionRecord>)>>,
}

impl SelfReflectionAgent {
    /// Create a Self-Reflection Agent
    ///
    /// # Parameters
    /// * `name` - Agent name (used for identification and logging)
    /// * `generator` - Generator Agent (e.g. ReactAgent), responsible for generating initial responses
    /// * `critic` - Critic (evaluator), responsible for evaluating generation quality and providing feedback
    ///
    /// # Default configuration
    /// * Max reflections: 3
    /// * Pass threshold: 7.0 (score ≥ 7.0 is considered pass)
    /// * Episodic memory capacity: 10 experiences
    pub fn new(
        name: impl Into<String>,
        generator: impl Agent + 'static,
        critic: impl Critic + 'static,
    ) -> Self {
        Self {
            name: name.into(),
            generator: Box::new(generator),
            critic: Box::new(critic),
            max_reflections: 3,
            pass_threshold: 7.0,
            refinement_prompt_builder: Box::new(default_refinement_prompt),
            reflection_prompt_builder: Box::new(default_reflection_prompt),
            episodic_memory: RwLock::new(std::collections::VecDeque::with_capacity(10)),
            memory_limit: 10,
            store: None,
            pending_records: RwLock::new(Vec::new()),
        }
    }

    /// Maximum reflection iterations (default 3)
    pub fn max_reflections(mut self, n: usize) -> Self {
        self.max_reflections = n;
        self
    }

    /// Pass threshold (0.0 - 10.0, default 7.0)
    pub fn pass_threshold(mut self, threshold: f64) -> Self {
        self.pass_threshold = threshold;
        self
    }

    /// Custom refinement prompt builder
    pub fn refinement_prompt_builder(
        mut self,
        f: impl Fn(&str, &str, &Critique, &str, usize) -> String + Send + Sync + 'static,
    ) -> Self {
        self.refinement_prompt_builder = Box::new(f);
        self
    }

    /// Custom reflection prompt builder
    pub fn reflection_prompt_builder(
        mut self,
        f: impl Fn(&str, &str, &Critique) -> String + Send + Sync + 'static,
    ) -> Self {
        self.reflection_prompt_builder = Box::new(f);
        self
    }

    /// Set episodic memory capacity limit (default 10)
    pub fn memory_limit(mut self, limit: usize) -> Self {
        self.memory_limit = limit;
        self
    }

    /// Set persistent storage
    pub fn with_store(mut self, store: Arc<dyn ReflectionStore>) -> Self {
        self.store = Some(store);
        self
    }

    /// Core execution loop
    async fn run_reflection_loop(&self, task: &str) -> Result<String> {
        let agent = self.name.clone();

        // ── Phase 1: Generate initial response ──────────────────────────────────────
        info!(agent = %agent, "🎯 Self-Reflection: generating initial response");

        let memory_context = self.build_memory_context();
        let enhanced_task = if memory_context.is_empty() {
            task.to_string()
        } else {
            format!(
                "{}\n\nRefer to the following past experiences and lessons:\n{}",
                task, memory_context
            )
        };

        let mut current_answer = self.generator.execute(&enhanced_task).await?;
        let mut records: Vec<ReflectionRecord> = Vec::new();

        // ── Phase 2: Evaluate → Reflect → Refine loop ──────────────────────────
        for iteration in 0..self.max_reflections {
            info!(
                agent = %agent,
                iteration = iteration + 1,
                max = self.max_reflections,
                "🔍 Self-Reflection: round {}/{} evaluation",
                iteration + 1,
                self.max_reflections
            );

            // Evaluate
            let context = self.build_critique_context(&records);
            let critique = self
                .critic
                .critique(task, &current_answer, &context)
                .await?;

            info!(
                agent = %agent,
                score = critique.score,
                passed = critique.passed,
                "📊 Evaluation result: {:.1}/10.0 ({})",
                critique.score,
                if critique.passed { "pass" } else { "fail" }
            );

            // Passed quality threshold
            if critique.passed && critique.score >= self.pass_threshold {
                records.push(ReflectionRecord {
                    iteration,
                    answer: current_answer.clone(),
                    critique: critique.clone(),
                    reflection_text: String::new(),
                    refined_answer: None,
                });

                info!(agent = %agent, "✅ Self-Reflection: evaluation passed");

                // Store successful experience
                self.persist_records(task, &records).await;
                return Ok(current_answer);
            }

            // Reflect: analyze failure reasons
            let reflection_prompt =
                (self.reflection_prompt_builder)(task, &current_answer, &critique);

            let reflection_text = self.generator.execute(&reflection_prompt).await?;
            debug!(agent = %agent, reflection = %reflection_text, "💡 Reflection text");

            // Build refinement prompt
            let refinement_prompt = (self.refinement_prompt_builder)(
                task,
                &current_answer,
                &critique,
                &reflection_text,
                iteration,
            );

            // Refine
            info!(agent = %agent, iteration = iteration + 1, "🔧 Self-Reflection: refining answer");
            let refined = self.generator.execute(&refinement_prompt).await?;

            records.push(ReflectionRecord {
                iteration,
                answer: current_answer.clone(),
                critique,
                reflection_text,
                refined_answer: Some(refined.clone()),
            });

            current_answer = refined;

            // Extract and store lessons learned
            self.extract_experience(&records);
        }

        info!(
            agent = %agent,
            "🏁 Self-Reflection: reached max reflection iterations {}",
            self.max_reflections
        );

        self.persist_records(task, &records).await;
        Ok(current_answer)
    }

    /// Build episodic memory context text
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

    /// Build evaluation context (records from previous rounds)
    fn build_critique_context(&self, records: &[ReflectionRecord]) -> String {
        if records.is_empty() {
            return String::new();
        }

        let mut parts = vec!["Evaluation records from previous rounds:".to_string()];
        for r in records {
            parts.push(format!(
                "  Round {}: score {:.1} — {}",
                r.iteration + 1,
                r.critique.score,
                r.critique.feedback
            ));
        }
        parts.join("\n")
    }

    /// Extract lessons learned from reflection records
    fn extract_experience(&self, records: &[ReflectionRecord]) {
        for r in records {
            if r.critique.passed {
                continue;
            }
            // Extract brief lessons from feedback
            let feedback = &r.critique.feedback;
            let lesson = if feedback.len() > 100 {
                // Safe truncation using char_indices
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
                .unwrap_or_else(|| "Unrecognized error pattern".to_string());

            // Dedup: increment use count if similar experience already exists
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

    /// Persist records — buffer then batch write
    ///
    /// Add records to a pending write buffer and auto-flush when the buffer reaches the threshold.
    /// This reduces frequent I/O operations and improves performance.
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

    /// Immediately flush all pending records
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

impl Agent for SelfReflectionAgent {
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

                // ── Phase 1: Generate initial response ──
                info!(agent = %agent, "🎯 Self-Reflection (stream): generating initial response");
                let memory_context = self.build_memory_context();
                let enhanced_task = if memory_context.is_empty() {
                    task_owned.clone()
                } else {
                    format!("{}\n\nRefer to the following past experiences and lessons:\n{}", task_owned, memory_context)
                };

                let mut current_answer = self.generator.execute(&enhanced_task).await?;
                let mut records: Vec<ReflectionRecord> = Vec::new();

                // ── Phase 2: Evaluate → Reflect → Refine loop ──
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

                    // Passed
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

                    // Reflect
                    let reflection_prompt = (self.reflection_prompt_builder)(
                        &task_owned, &current_answer, &critique);
                    let reflection_text = self.generator.execute(&reflection_prompt).await?;

                    // Refine
                    yield AgentEvent::Refining { iteration };
                    let refinement_prompt = (self.refinement_prompt_builder)(
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
/// Reflective executor: use Self-Reflection as a Plan-and-Execute Executor
///
/// Each Plan step goes through the "Generate → Evaluate → Refine" closed loop.
///
/// # Example
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
///     .system_prompt("You are a task execution assistant")
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
/// let result = agent.execute("Analyze and optimize the code performance").await?;
/// # Ok(())
/// # }
/// ```
pub struct ReflectiveExecutor {
    agent: SelfReflectionAgent,
}

#[cfg(feature = "plan-execute")]
impl ReflectiveExecutor {
    /// Create a ReflectiveExecutor
    ///
    /// # Parameters
    /// * `agent` - A pre-configured SelfReflectionAgent
    ///
    /// # Description
    /// Adapts the Self-Reflection Agent as an Executor in the Plan-and-Execute architecture,
    /// allowing it to be used as a PlanStep executor.
    pub fn new(agent: SelfReflectionAgent) -> Self {
        Self { agent }
    }

    /// Quick creation with default configuration
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
                "⚡ ReflectiveExecutor executing step (with reflection)"
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
        let generator = crate::testing::MockAgent::new("mock").with_response("This is an answer");
        let critic = StaticCritic::always_pass();

        let agent = SelfReflectionAgent::new("test", generator, critic).max_reflections(3);

        let result = agent.execute("Test task").await.unwrap();
        assert_eq!(result, "This is an answer");
    }

    #[tokio::test]
    async fn test_self_reflection_always_fails() {
        // MockAgent has multiple responses for generate + reflect + refine
        let generator = crate::testing::MockAgent::new("mock").with_responses([
            "answer1",
            "reflection",
            "refined",
        ]);
        let critic = StaticCritic::always_fail();

        let agent = SelfReflectionAgent::new("test", generator, critic).max_reflections(2);

        let result = agent.execute("Test task").await.unwrap();
        // Return the last answer even if always failing
        assert!(!result.is_empty());
    }

    #[tokio::test]
    async fn test_self_reflection_reset() {
        let generator = crate::testing::MockAgent::new("mock").with_response("answer");
        let critic = StaticCritic::always_pass();

        let agent = SelfReflectionAgent::new("test", generator, critic);

        // Execute once to accumulate experience
        agent.execute("Task1").await.unwrap();

        // Reset
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
