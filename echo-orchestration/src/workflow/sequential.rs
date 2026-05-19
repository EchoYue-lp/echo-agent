//! Sequential workflow: agents execute one by one in registration order, with each step's output becoming the next step's input.

use super::{SharedAgent, StepOutput, Workflow, WorkflowOutput, shared_agent};
use echo_core::agent::Agent;
use echo_core::error::Result;
use futures::future::BoxFuture;
use std::time::Instant;
use tracing::{debug, info};

/// Inter-step input transform function
type TransformFn = Box<dyn Fn(&str) -> String + Send + Sync>;

/// One step in a sequential workflow
pub struct WorkflowStep {
    agent: SharedAgent,
    transform: Option<TransformFn>,
}

/// Sequential workflow: each agent runs in registration order; the former's output flows into the latter's input.
///
/// # Example
///
/// ```rust,no_run
/// use echo_core::agent::{Agent, AgentEvent};
/// use echo_core::error::Result;
/// use echo_orchestration::workflow::{SequentialWorkflow, Workflow};
/// use futures::future::BoxFuture;
/// use futures::stream::{self, BoxStream};
///
/// # struct DummyAgent {
/// #     name: String,
/// # }
/// #
/// # impl DummyAgent {
/// #     fn new(name: impl Into<String>) -> Self {
/// #         Self { name: name.into() }
/// #     }
/// # }
/// #
/// # impl Agent for DummyAgent {
/// #     fn name(&self) -> &str { &self.name }
/// #     fn model_name(&self) -> &str { "mock-model" }
/// #     fn system_prompt(&self) -> &str { "You are a mock agent" }
/// #     fn execute<'a>(&'a self, task: &'a str) -> BoxFuture<'a, Result<String>> {
/// #         Box::pin(async move { Ok(format!("{}: {task}", self.name)) })
/// #     }
/// #     fn execute_stream<'a>(&'a self, _task: &'a str) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
/// #         Box::pin(async move {
/// #             let s: BoxStream<'a, Result<AgentEvent>> = Box::pin(stream::empty());
/// #             Ok(s)
/// #         })
/// #     }
/// # }
///
/// # async fn example() -> Result<()> {
/// let agent_a = DummyAgent::new("translator");
/// let agent_b = DummyAgent::new("reviewer");
///
/// let mut wf = SequentialWorkflow::builder()
///     .step(agent_a)
///     .step(agent_b)
///     .build();
///
/// let output = wf.run("Hello world").await?;
/// println!("Final result: {}", output.result);
/// for s in &output.steps {
///     println!("  {} ({:?}): {}", s.agent_name, s.elapsed, &s.output[..s.output.len().min(80)]);
/// }
/// # Ok(())
/// # }
/// ```
pub struct SequentialWorkflow {
    steps: Vec<WorkflowStep>,
}

impl SequentialWorkflow {
    pub fn builder() -> SequentialWorkflowBuilder {
        SequentialWorkflowBuilder { steps: Vec::new() }
    }
}

impl Workflow for SequentialWorkflow {
    fn run<'a>(&'a mut self, input: &'a str) -> BoxFuture<'a, Result<WorkflowOutput>> {
        Box::pin(async move {
            let total_start = Instant::now();
            let mut current_input = input.to_string();
            let mut step_outputs = Vec::with_capacity(self.steps.len());

            for (i, step) in self.steps.iter().enumerate() {
                let actual_input = if let Some(transform) = &step.transform {
                    transform(&current_input)
                } else {
                    current_input.clone()
                };

                let agent = step.agent.as_ref();
                let agent_name = agent.name().to_string();

                info!(
                    workflow = "sequential",
                    step = i + 1,
                    agent = %agent_name,
                    "▶ Executing step"
                );
                debug!(
                    workflow = "sequential",
                    step = i + 1,
                    input_len = actual_input.len()
                );

                let step_start = Instant::now();
                let output = agent.execute(&actual_input).await?;
                let step_elapsed = step_start.elapsed();

                info!(
                    workflow = "sequential",
                    step = i + 1,
                    agent = %agent_name,
                    elapsed_ms = step_elapsed.as_millis(),
                    "✓ Step completed"
                );

                step_outputs.push(StepOutput {
                    agent_name,
                    input: actual_input,
                    output: output.clone(),
                    elapsed: step_elapsed,
                });

                current_input = output;
            }

            let result = current_input;
            Ok(WorkflowOutput {
                result,
                steps: step_outputs,
                elapsed: total_start.elapsed(),
            })
        })
    }
}

/// [`SequentialWorkflow`] builder
pub struct SequentialWorkflowBuilder {
    steps: Vec<WorkflowStep>,
}

impl SequentialWorkflowBuilder {
    /// Add a step (uses previous step's output directly as input)
    pub fn step(mut self, agent: impl Agent + 'static) -> Self {
        self.steps.push(WorkflowStep {
            agent: shared_agent(agent),
            transform: None,
        });
        self
    }

    /// Add a step with a transform function applied to the input before execution
    pub fn step_with_transform(
        mut self,
        agent: impl Agent + 'static,
        transform: impl Fn(&str) -> String + Send + Sync + 'static,
    ) -> Self {
        self.steps.push(WorkflowStep {
            agent: shared_agent(agent),
            transform: Some(Box::new(transform)),
        });
        self
    }

    /// Add a step (using an already-wrapped SharedAgent)
    pub fn step_shared(mut self, agent: SharedAgent) -> Self {
        self.steps.push(WorkflowStep {
            agent,
            transform: None,
        });
        self
    }

    pub fn build(self) -> SequentialWorkflow {
        SequentialWorkflow { steps: self.steps }
    }
}
