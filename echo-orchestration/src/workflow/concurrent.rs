//! Concurrent workflow: all agents execute the same input in parallel; results are aggregated via a merge function.

use super::{SharedAgent, StepOutput, Workflow, WorkflowOutput, shared_agent};
use echo_core::agent::Agent;
use echo_core::error::Result;
use futures::future::BoxFuture;
use std::time::Instant;
use tracing::{debug, info};

/// Result merge function
type MergeFn = Box<dyn Fn(Vec<String>) -> String + Send + Sync>;

fn default_merge(results: Vec<String>) -> String {
    results.join("\n---\n")
}

/// Concurrent workflow: all registered agents execute in parallel; results are merged via a `merge` function.
///
/// # Example
///
/// ```rust,no_run
/// use echo_core::agent::{Agent, AgentEvent};
/// use echo_core::error::Result;
/// use echo_orchestration::workflow::{ConcurrentWorkflow, Workflow};
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
/// let agent_x = DummyAgent::new("tech");
/// let agent_y = DummyAgent::new("business");
///
/// let mut wf = ConcurrentWorkflow::builder()
///     .agent(agent_x)
///     .agent(agent_y)
///     .merge(|results| {
///         format!("Combined analysis:\n{}", results.join("\n\n"))
///     })
///     .build();
///
/// let output = wf.run("Analyze AI Agent development trends").await?;
/// println!("{}", output.result);
/// # Ok(())
/// # }
/// ```
pub struct ConcurrentWorkflow {
    agents: Vec<SharedAgent>,
    merge: MergeFn,
}

impl ConcurrentWorkflow {
    pub fn builder() -> ConcurrentWorkflowBuilder {
        ConcurrentWorkflowBuilder {
            agents: Vec::new(),
            merge: None,
        }
    }
}

impl Workflow for ConcurrentWorkflow {
    fn run<'a>(&'a mut self, input: &'a str) -> BoxFuture<'a, Result<WorkflowOutput>> {
        Box::pin(async move {
            let total_start = Instant::now();
            let agent_count = self.agents.len();

            info!(
                workflow = "concurrent",
                agents = agent_count,
                "⚡ Executing {} agents concurrently",
                agent_count
            );

            let mut handles = Vec::with_capacity(agent_count);

            for agent_handle in &self.agents {
                let agent_handle = agent_handle.clone();
                let input = input.to_string();
                handles.push(tokio::spawn(async move {
                    let step_start = Instant::now();
                    let agent = agent_handle.as_ref();
                    let agent_name = agent.name().to_string();
                    debug!(workflow = "concurrent", agent = %agent_name, "▶ Starting execution");
                    let result = agent.execute(&input).await;
                    let elapsed = step_start.elapsed();
                    (agent_name, input, result, elapsed)
                }));
            }

            let mut step_outputs = Vec::with_capacity(agent_count);
            let mut results = Vec::with_capacity(agent_count);

            let mut first_error = None;
            for handle in &mut handles {
                let joined = handle.await;
                let (agent_name, step_input, result, elapsed) = match joined {
                    Ok(value) => value,
                    Err(error) => {
                        first_error.get_or_insert_with(|| {
                            echo_core::error::ReactError::Other(format!("task join error: {error}"))
                        });
                        continue;
                    }
                };
                let output = match result {
                    Ok(output) => output,
                    Err(error) => {
                        first_error.get_or_insert(error);
                        continue;
                    }
                };
                info!(
                    workflow = "concurrent",
                    agent = %agent_name,
                    elapsed_ms = elapsed.as_millis(),
                    "✓ Agent completed"
                );

                step_outputs.push(StepOutput {
                    agent_name,
                    input: step_input,
                    output: output.clone(),
                    elapsed,
                });
                results.push(output);
            }
            if let Some(error) = first_error {
                for handle in &handles {
                    if !handle.is_finished() {
                        handle.abort();
                    }
                }
                return Err(error);
            }

            let merged = (self.merge)(results);

            Ok(WorkflowOutput {
                result: merged,
                steps: step_outputs,
                elapsed: total_start.elapsed(),
            })
        })
    }
}

/// [`ConcurrentWorkflow`] builder
pub struct ConcurrentWorkflowBuilder {
    agents: Vec<SharedAgent>,
    merge: Option<MergeFn>,
}

impl ConcurrentWorkflowBuilder {
    /// Add an agent to execute concurrently
    pub fn agent(mut self, agent: impl Agent + 'static) -> Self {
        self.agents.push(shared_agent(agent));
        self
    }

    /// Add an already-wrapped SharedAgent
    pub fn agent_shared(mut self, agent: SharedAgent) -> Self {
        self.agents.push(agent);
        self
    }

    /// Set the result merge function (default joins with `\n---\n`)
    pub fn merge(mut self, f: impl Fn(Vec<String>) -> String + Send + Sync + 'static) -> Self {
        self.merge = Some(Box::new(f));
        self
    }

    pub fn build(self) -> ConcurrentWorkflow {
        ConcurrentWorkflow {
            agents: self.agents,
            merge: self.merge.unwrap_or_else(|| Box::new(default_merge)),
        }
    }
}
