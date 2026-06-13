//! Data analysis pipeline — load_data -> profile -> analyze -> visualize -> summarize
//!
//! A 5-stage graph workflow that uses a single `SharedAgent` to perform
//! end-to-end data analysis. Each stage is an `agent.chat()` call with a
//! purpose-specific prompt, and intermediate results are stored in
//! [`SharedState`] for downstream stages to consume.
//!
//! # Example
//!
//! ```rust,no_run
//! use echo_agent::workflow::pipelines::data_pipeline::{
//!     DataPipelineConfig, run_data_pipeline,
//! };
//! use echo_agent::workflow::SharedAgent;
//! use echo_agent::testing::MockAgent;
//!
//! # async fn example() -> echo_core::error::Result<()> {
//! let agent = MockAgent::new("data_analyst")
//!     .with_response("Analysis complete. Key finding: positive trend.");
//!
//! let config = DataPipelineConfig {
//!     dataset_path: "/data/sales_2024.csv".to_string(),
//!     objective: Some("Identify revenue trends".to_string()),
//!     max_charts: 3,
//! };
//!
//! let result = run_data_pipeline(&agent.into(), config).await?;
//! println!("Summary: {}", result.state.get::<String>("summary").unwrap_or_default());
//! # Ok(())
//! # }
//! ```

use crate::workflow::SharedAgent;
use crate::workflow::graph::{Graph, GraphBuilder, GraphResult};
use crate::workflow::state::SharedState;
use echo_core::error::Result;

// ── Configuration ──────────────────────────────────────────────────────────────

/// Configuration for the data analysis pipeline.
#[derive(Debug, Clone)]
pub struct DataPipelineConfig {
    /// Path to the dataset file (CSV, JSON, Parquet, etc.).
    pub dataset_path: String,
    /// Optional analysis objective / question to focus on.
    pub objective: Option<String>,
    /// Maximum number of charts/visualizations to produce.
    pub max_charts: u32,
}

impl Default for DataPipelineConfig {
    fn default() -> Self {
        Self {
            dataset_path: String::new(),
            objective: None,
            max_charts: 3,
        }
    }
}

impl DataPipelineConfig {
    /// Create a config with the dataset path and default values for other fields.
    pub fn new(dataset_path: impl Into<String>) -> Self {
        Self {
            dataset_path: dataset_path.into(),
            objective: None,
            max_charts: 3,
        }
    }

    /// Set the analysis objective.
    pub fn with_objective(mut self, objective: impl Into<String>) -> Self {
        self.objective = Some(objective.into());
        self
    }

    /// Set the maximum number of charts.
    pub fn with_max_charts(mut self, max: u32) -> Self {
        self.max_charts = max;
        self
    }
}

// ── Pipeline Stages ────────────────────────────────────────────────────────────

/// Build the data analysis graph.
///
/// Constructs a linear pipeline: `load_data -> profile -> analyze -> visualize -> summarize`.
/// Each stage reads from `SharedState` and writes its output back, so subsequent
/// stages can reference prior results.
///
/// All configuration and prompt templates are injected into state via the `init`
/// node, so downstream function nodes only read from state — no closure
/// captures of local variables are needed.
fn build_data_graph(agent: &SharedAgent) -> Result<Graph> {
    let agent_clone = agent.clone();

    let graph = GraphBuilder::new("data_pipeline")
        // ── Init: store config values in state ──
        .add_function_node("init", |state: &SharedState| {
            Box::pin(async move {
                // Config values are set in state by run_data_pipeline() before
                // graph execution starts. Read them here to build prompt templates.
                let dataset_path: String = state.get("dataset_path").unwrap_or_default();
                let objective: String = state
                    .get("objective")
                    .unwrap_or_else(|| "general exploration and insight extraction".to_string());
                let max_charts: i64 = state.get("max_charts").unwrap_or(3);

                // Store prompt templates in state for downstream nodes
                let _ = state.set(
                    "tpl_load",
                    format!(
                        "You are a data loader. Read the dataset at path '{}' and describe \
                         its contents: number of rows, columns, column names, and a sample \
                         of the first few rows. Store the full dataset description as your output.",
                        dataset_path,
                    ),
                );
                let _ = state.set(
                    "tpl_profile",
                    format!(
                        "You are a data profiler. Given the dataset description, compute a \
                         detailed profile: statistics (mean, median, std, min, max) for numeric \
                         columns, missing value counts per column, and data type classification. \
                         Objective: {}. Output a structured data profile.",
                        objective,
                    ),
                );
                let _ = state.set(
                    "tpl_analyze",
                    format!(
                        "You are a statistical analyst. Given the data profile and dataset \
                         description, perform a thorough statistical analysis: correlations, \
                         outliers, distributions, and significant patterns. Focus on the \
                         objective: {}. Output a detailed analysis report.",
                        objective,
                    ),
                );
                let _ = state.set(
                    "tpl_visualize",
                    format!(
                        "You are a data visualization specialist. Based on the analysis results, \
                         propose up to {} charts/visualizations that best illustrate the key findings. \
                         For each chart, specify: chart type, axes, title, and the insight it reveals. \
                         Focus on the objective: {}. Output structured visualization specifications.",
                        max_charts, objective,
                    ),
                );
                let _ = state.set(
                    "tpl_summarize",
                    format!(
                        "You are a data insight summarizer. Combine the data profile, analysis, \
                         and visualization specifications into a concise executive summary. \
                         Highlight the top 3-5 key insights, actionable recommendations, and \
                         any caveats or limitations. Objective: {}. Output a final summary report.",
                        objective,
                    ),
                );
                Ok(())
            })
        })
        // ── Stage 1: load_data ──
        // Function node constructs prompt from template + state, agent executes it.
        .add_function_node("load_prompt", |state: &SharedState| {
            Box::pin(async move {
                let tpl: String = state.get("tpl_load").unwrap_or_default();
                let _ = state.set("load_prompt", tpl);
                Ok(())
            })
        })
        .add_shared_agent_node_with_mode(
            "load_data",
            agent_clone.clone(),
            "load_prompt",
            "loaded_data",
            false, // chat mode (single-turn)
        )
        // ── Stage 2: profile ──
        .add_function_node("profile_prompt", |state: &SharedState| {
            Box::pin(async move {
                let tpl: String = state.get("tpl_profile").unwrap_or_default();
                let loaded: String = state.get("loaded_data").unwrap_or_default();
                let prompt = format!(
                    "{}\n\nHere is the loaded dataset description:\n{}",
                    tpl, loaded,
                );
                let _ = state.set("profile_prompt", prompt);
                Ok(())
            })
        })
        .add_shared_agent_node_with_mode(
            "profile",
            agent_clone.clone(),
            "profile_prompt",
            "data_profile",
            false,
        )
        // ── Stage 3: analyze ──
        .add_function_node("analyze_prompt", |state: &SharedState| {
            Box::pin(async move {
                let tpl: String = state.get("tpl_analyze").unwrap_or_default();
                let loaded: String = state.get("loaded_data").unwrap_or_default();
                let profile: String = state.get("data_profile").unwrap_or_default();
                let prompt = format!(
                    "{}\n\nDataset description:\n{}\n\nData profile:\n{}",
                    tpl, loaded, profile,
                );
                let _ = state.set("analyze_prompt", prompt);
                Ok(())
            })
        })
        .add_shared_agent_node_with_mode(
            "analyze",
            agent_clone.clone(),
            "analyze_prompt",
            "analysis",
            false,
        )
        // ── Stage 4: visualize ──
        .add_function_node("visualize_prompt", |state: &SharedState| {
            Box::pin(async move {
                let tpl: String = state.get("tpl_visualize").unwrap_or_default();
                let profile: String = state.get("data_profile").unwrap_or_default();
                let analysis: String = state.get("analysis").unwrap_or_default();
                let prompt = format!(
                    "{}\n\nData profile:\n{}\n\nAnalysis:\n{}",
                    tpl, profile, analysis,
                );
                let _ = state.set("visualize_prompt", prompt);
                Ok(())
            })
        })
        .add_shared_agent_node_with_mode(
            "visualize",
            agent_clone.clone(),
            "visualize_prompt",
            "visualizations",
            false,
        )
        // ── Stage 5: summarize ──
        .add_function_node("summarize_prompt", |state: &SharedState| {
            Box::pin(async move {
                let tpl: String = state.get("tpl_summarize").unwrap_or_default();
                let profile: String = state.get("data_profile").unwrap_or_default();
                let analysis: String = state.get("analysis").unwrap_or_default();
                let viz: String = state.get("visualizations").unwrap_or_default();
                let prompt = format!(
                    "{}\n\nData profile:\n{}\n\nAnalysis:\n{}\n\nVisualizations:\n{}",
                    tpl, profile, analysis, viz,
                );
                let _ = state.set("summarize_prompt", prompt);
                Ok(())
            })
        })
        .add_shared_agent_node_with_mode(
            "summarize",
            agent_clone,
            "summarize_prompt",
            "summary",
            false,
        )
        // ── Edges: linear pipeline ──
        .set_entry("init")
        .add_edge("init", "load_prompt")
        .add_edge("load_prompt", "load_data")
        .add_edge("load_data", "profile_prompt")
        .add_edge("profile_prompt", "profile")
        .add_edge("profile", "analyze_prompt")
        .add_edge("analyze_prompt", "analyze")
        .add_edge("analyze", "visualize_prompt")
        .add_edge("visualize_prompt", "visualize")
        .add_edge("visualize", "summarize_prompt")
        .add_edge("summarize_prompt", "summarize")
        .set_finish("summarize")
        .build()?;

    Ok(graph)
}

// ── Pipeline Execution ─────────────────────────────────────────────────────────

/// Run the data analysis pipeline.
///
/// Returns a [`GraphResult`] containing the final [`SharedState`] with keys:
/// - `loaded_data` — dataset description
/// - `data_profile` — statistical profile
/// - `analysis` — analysis report
/// - `visualizations` — chart specifications
/// - `summary` — final executive summary
pub async fn run_data_pipeline(
    agent: &SharedAgent,
    config: DataPipelineConfig,
) -> Result<GraphResult> {
    let graph = build_data_graph(agent)?;
    let state = SharedState::new();

    tracing::info!(
        pipeline = "data",
        dataset = %config.dataset_path,
        objective = ?config.objective,
        max_charts = config.max_charts,
        "Starting data analysis pipeline"
    );

    // Store config values in state before graph execution starts.
    // The init node reads these to build prompt templates.
    let _ = state.set("dataset_path", config.dataset_path);
    let _ = state.set(
        "objective",
        config
            .objective
            .unwrap_or_else(|| "general exploration and insight extraction".to_string()),
    );
    let _ = state.set("max_charts", config.max_charts as i64);

    let result = graph.run(state).await?;

    tracing::info!(
        pipeline = "data",
        steps = result.steps,
        path = ?result.path,
        "Data analysis pipeline completed"
    );

    Ok(result)
}

// ── Unit Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::shared_agent;

    /// A minimal mock that returns a fixed string for any chat() call.
    struct StageMock {
        name: String,
        response: String,
    }

    impl StageMock {
        fn new(name: &str, response: &str) -> Self {
            Self {
                name: name.to_string(),
                response: response.to_string(),
            }
        }
    }

    impl echo_core::agent::Agent for StageMock {
        fn name(&self) -> &str {
            &self.name
        }

        fn model_name(&self) -> &str {
            "mock-model"
        }

        fn system_prompt(&self) -> &str {
            "You are a mock agent"
        }

        fn execute<'a>(&'a self, _task: &'a str) -> futures::future::BoxFuture<'a, Result<String>> {
            Box::pin(async move { Ok(self.response.clone()) })
        }

        fn execute_stream<'a>(
            &'a self,
            _task: &'a str,
        ) -> futures::future::BoxFuture<
            'a,
            Result<futures::stream::BoxStream<'a, Result<echo_core::agent::AgentEvent>>>,
        > {
            Box::pin(async move {
                let s: futures::stream::BoxStream<'a, Result<echo_core::agent::AgentEvent>> =
                    Box::pin(futures::stream::empty());
                Ok(s)
            })
        }
    }

    #[tokio::test]
    async fn test_data_pipeline_runs_all_stages() {
        let agent = shared_agent(StageMock::new(
            "data_analyst",
            "Mock analysis result: positive trend detected.",
        ));

        let config = DataPipelineConfig {
            dataset_path: "/test/data.csv".to_string(),
            objective: Some("Find revenue trends".to_string()),
            max_charts: 3,
        };

        let result = run_data_pipeline(&agent, config).await.unwrap();

        // Verify all stages ran
        assert!(result.path.contains(&"init".to_string()));
        assert!(result.path.contains(&"load_prompt".to_string()));
        assert!(result.path.contains(&"load_data".to_string()));
        assert!(result.path.contains(&"profile_prompt".to_string()));
        assert!(result.path.contains(&"profile".to_string()));
        assert!(result.path.contains(&"analyze_prompt".to_string()));
        assert!(result.path.contains(&"analyze".to_string()));
        assert!(result.path.contains(&"visualize_prompt".to_string()));
        assert!(result.path.contains(&"visualize".to_string()));
        assert!(result.path.contains(&"summarize_prompt".to_string()));
        assert!(result.path.contains(&"summarize".to_string()));

        // Verify final state has expected keys
        assert!(result.state.contains("loaded_data"));
        assert!(result.state.contains("data_profile"));
        assert!(result.state.contains("analysis"));
        assert!(result.state.contains("visualizations"));
        assert!(result.state.contains("summary"));
        assert!(result.state.contains("dataset_path"));
    }

    #[tokio::test]
    async fn test_data_pipeline_default_config() {
        let agent = shared_agent(StageMock::new("analyst", "result"));

        let config = DataPipelineConfig::new("/data/test.csv");
        assert_eq!(config.dataset_path, "/data/test.csv");
        assert_eq!(config.max_charts, 3);
        assert!(config.objective.is_none());

        let result = run_data_pipeline(&agent, config).await.unwrap();
        assert!(result.state.contains("summary"));
    }
}
