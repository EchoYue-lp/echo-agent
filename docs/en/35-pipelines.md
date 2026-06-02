# Pipelines — Pre-built Workflow Pipelines

## What It Is

Pipelines are **pre-built multi-stage workflows** built on top of Graph Workflow, encapsulating common Agent task patterns:

| Pipeline | Stages | Description |
|----------|--------|-------------|
| [`data_pipeline`] | load_data → profile → analyze → visualize → summarize | End-to-end data analysis |
| [`writing_pipeline`] | outline → draft → review → revise (loop) → finalize | Content creation with quality loop |

Each pipeline takes a `SharedAgent` and a config, internally builds a Graph and executes it, returning a `GraphResult` with all intermediate results.

---

## Data Pipeline — Data Analysis

### Flow Diagram

```
┌──────────────────────────────────────────────────────────────────┐
│                     Data Pipeline Flow                            │
│                                                                   │
│   ┌──────┐    ┌─────────┐    ┌─────────┐    ┌───────────┐      │
│   │ init │───▶│load_data│───▶│ profile │───▶│ analyze   │      │
│   └──────┘    └─────────┘    └─────────┘    └───────────┘      │
│                                                   │              │
│                                                   ▼              │
│                                              ┌──────────┐        │
│                                              │summarize │        │
│                                              └──────────┘        │
│                                                   ▲              │
│                                                   │              │
│                                            ┌─────────────┐      │
│                                            │  visualize  │      │
│                                            └─────────────┘      │
└──────────────────────────────────────────────────────────────────┘

  Each stage writes output to SharedState; downstream stages read upstream results
```

### Stage Descriptions

| Stage | Responsibility | State Key (output) |
|-------|---------------|-------------------|
| `init` | Generate prompt templates from config | `tpl_load`, `tpl_profile`, ... |
| `load_data` | Read dataset, describe rows, columns, samples | `loaded_data` |
| `profile` | Statistical overview: mean/median/std/missing/types | `data_profile` |
| `analyze` | Correlations, outliers, distributions, patterns | `analysis` |
| `visualize` | Chart specifications (type, axes, title) | `visualizations` |
| `summarize` | Executive summary: top 3-5 insights + recommendations | `summary` |

### DataPipelineConfig

```rust
pub struct DataPipelineConfig {
    /// Path to the dataset file (CSV, JSON, Parquet, etc.)
    pub dataset_path: String,
    /// Optional analysis objective / question to focus on
    pub objective: Option<String>,
    /// Maximum number of charts/visualizations to produce
    pub max_charts: u32,
}
```

**Builder methods:**

| Method | Description |
|--------|-------------|
| `new(path)` | Create with dataset path, defaults for other fields |
| `with_objective(obj)` | Set the analysis objective |
| `with_max_charts(n)` | Set maximum chart count |

**Defaults:** `objective = None` (general exploration), `max_charts = 3`

### API: run_data_pipeline

```rust
pub async fn run_data_pipeline(
    agent: &SharedAgent,
    config: DataPipelineConfig,
) -> Result<GraphResult>
```

Returns `GraphResult` containing:
- `state` — `SharedState` with access to all intermediate and final results via keys
- `steps` — number of execution steps
- `path` — execution path (list of node names traversed)

### Code Examples

#### Basic Usage

```rust
use echo_agent::workflow::pipelines::{DataPipelineConfig, run_data_pipeline};
use echo_agent::workflow::shared_agent;
use echo_agent::agent::ReactAgentBuilder;

# async fn example() -> echo_core::error::Result<()> {
let agent = shared_agent(
    ReactAgentBuilder::simple("qwen3-max", "Data Analyst")?
);

let config = DataPipelineConfig::new("/data/sales_2024.csv")
    .with_objective("Identify revenue trends and seasonal patterns")
    .with_max_charts(5);

let result = run_data_pipeline(&agent, config).await?;

// Get the final summary
let summary: String = result.state.get("summary").unwrap_or_default();
println!("Analysis Summary:\n{}", summary);

// Access intermediate results
let profile: String = result.state.get("data_profile").unwrap_or_default();
let viz: String = result.state.get("visualizations").unwrap_or_default();
println!("Data Profile:\n{}", profile);
println!("Visualization Plan:\n{}", viz);
# Ok(())
# }
```

#### Integration with Data Tools

When the Agent is configured with data tools (Excel, CSV, Statistics), each pipeline stage can invoke tools for actual computation:

```rust
use echo_agent::tools::{ExcelTool, CsvTool, StatisticsTool};

let agent = shared_agent(
    ReactAgentBuilder::simple("qwen3-max", "Data Analyst")?
        .with_tool(ExcelTool::new())
        .with_tool(CsvTool::new())
        .with_tool(StatisticsTool::new())
);

let config = DataPipelineConfig::new("/data/experiment_results.xlsx")
    .with_objective("Compare A/B test groups for statistical significance");

let result = run_data_pipeline(&agent, config).await?;
// The Agent automatically invokes StatisticsTool during the analyze stage
```

---

## Writing Pipeline — Content Creation

### Flow Diagram

```
┌───────────────────────────────────────────────────────────────────┐
│                     Writing Pipeline Flow                          │
│                                                                    │
│   ┌──────┐    ┌─────────┐    ┌───────┐    ┌────────┐            │
│   │ init │───▶│ outline │───▶│ draft │───▶│ review │            │
│   └──────┘    └─────────┘    └───────┘    └────────┘            │
│                                               │                  │
│                                               ▼                  │
│                                      ┌────────────────┐          │
│                                      │evaluate_quality│          │
│                                      └────────┬───────┘          │
│                                               │                  │
│                        ┌──────────────────────┼──────────────┐   │
│                        ▼                      │              ▼   │
│              score < threshold        score ≥ threshold   ┌──────────┐
│              & revs < max             or revs = max       │ finalize │
│                        │                      │           └──────────┘
│                        ▼                      │
│                   ┌─────────┐                 │
│                   │ revise  │                 │
│                   └────┬────┘                 │
│                        │                      │
│                        ▼                      │
│              ┌──────────────────┐             │
│              │increment_revision│             │
│              └────────┬─────────┘             │
│                       │                       │
│                       └──▶ review (loop back) │
└───────────────────────────────────────────────────────────────────┘

  Quality loop: review → evaluate → revise → review until threshold met or max reached
```

### Stage Descriptions

| Stage | Responsibility | State Key (output) |
|-------|---------------|-------------------|
| `init` | Generate prompt templates from config | `tpl_outline`, `tpl_draft`, ... |
| `outline` | Create structured outline (title, sections, key points) | `outline` |
| `draft` | Write full draft following the outline | `draft` |
| `review` | Review and score (0-100), output `QUALITY_SCORE: <n>` | `review` |
| `evaluate_quality` | Extract score from review text, decide whether to loop | `quality_score` |
| `revise` | Revise draft based on review feedback (overwrites `draft`) | `draft` (overwritten) |
| `increment_revision` | Increment revision counter | `revision_count` |
| `finalize` | Final polish, produce publication-ready content | `final_output` |

### Quality Loop Mechanism

The review stage asks the Agent to prefix its output with `QUALITY_SCORE: <number>`. The `evaluate_quality` node extracts this score and decides:

| Condition | Action |
|-----------|--------|
| `score ≥ threshold` | Proceed to finalize |
| `score < threshold` and `revision_count < max_revisions` | Loop to revise → re-review |
| `revision_count ≥ max_revisions` | Proceed to finalize (even if below threshold) |

**Score extraction priority:**
1. `QUALITY_SCORE: <number>` (exact match)
2. `Score: <number>` (fallback match)
3. Default 60 (when no score pattern found)

### WritingPipelineConfig

```rust
pub struct WritingPipelineConfig {
    /// Topic or subject of the writing
    pub topic: String,
    /// Target audience for the content
    pub audience: String,
    /// Desired format (e.g. "blog post", "essay", "report", "white paper")
    pub format: String,
    /// Maximum number of revision iterations (review → revise loops)
    pub max_revisions: u32,
    /// Quality score threshold (0-100). Below this triggers revision
    pub quality_threshold: u32,
}
```

**Builder methods:**

| Method | Description |
|--------|-------------|
| `new(topic)` | Create with topic, defaults for other fields |
| `with_audience(aud)` | Set target audience |
| `with_format(fmt)` | Set output format |
| `with_max_revisions(n)` | Set maximum revision count |
| `with_quality_threshold(t)` | Set quality threshold |

**Defaults:** `audience = "general readers"`, `format = "blog post"`, `max_revisions = 2`, `quality_threshold = 80`

### API: run_writing_pipeline

```rust
pub async fn run_writing_pipeline(
    agent: &SharedAgent,
    config: WritingPipelineConfig,
) -> Result<GraphResult>
```

Returns `GraphResult` containing:
- `state` — `SharedState` with access to all intermediate and final results via keys
- `steps` — number of execution steps (including loop iterations)
- `path` — execution path

### Code Examples

#### Basic Usage

```rust
use echo_agent::workflow::pipelines::{WritingPipelineConfig, run_writing_pipeline};
use echo_agent::workflow::shared_agent;
use echo_agent::agent::ReactAgentBuilder;

# async fn example() -> echo_core::error::Result<()> {
let agent = shared_agent(
    ReactAgentBuilder::simple("qwen3-max", "Writing Assistant")?
);

let config = WritingPipelineConfig::new("The Rise of AI Agents")
    .with_audience("technical professionals")
    .with_format("blog post")
    .with_max_revisions(3)
    .with_quality_threshold(85);

let result = run_writing_pipeline(&agent, config).await?;

// Get the final content
let final_output: String = result.state.get("final_output").unwrap_or_default();
println!("Final Draft:\n{}", final_output);

// Check quality metrics
let score: i64 = result.state.get("quality_score").unwrap_or(0);
let revisions: i64 = result.state.get("revision_count").unwrap_or(0);
println!("Quality score: {}, Revisions: {}", score, revisions);
# Ok(())
# }
```

#### Academic Paper Writing

```rust
let config = WritingPipelineConfig::new("Survey of Transformer-based Multimodal Fusion Methods")
    .with_audience("computer science researchers")
    .with_format("academic paper")
    .with_max_revisions(5)
    .with_quality_threshold(90);

let result = run_writing_pipeline(&agent, config).await?;

let outline: String = result.state.get("outline").unwrap_or_default();
let final_paper: String = result.state.get("final_output").unwrap_or_default();
```

---

## Customization & Extension

### Accessing Intermediate Results

Both pipelines return a `GraphResult` with full `SharedState`, allowing direct access to any stage's output:

```rust
let result = run_data_pipeline(&agent, config).await?;

// List all state keys
for key in result.state.keys() {
    println!("State key: {}", key);
}

// Read a specific stage result
let analysis: String = result.state.get("analysis").unwrap_or_default();
```

### Building Custom Workflows Based on Pipeline Patterns

Pipelines use `GraphBuilder` internally. You can follow the same pattern to build custom pipelines. The core pattern:

```rust
use echo_agent::workflow::{GraphBuilder, SharedState, shared_agent};

let agent = shared_agent(/* ... */);

let graph = GraphBuilder::new("custom_pipeline")
    // 1. Init node: inject config and prompt templates into state
    .add_function_node("init", |state: &SharedState| {
        Box::pin(async move {
            state.set("tpl_step1", "Your prompt template...".to_string());
            Ok(())
        })
    })
    // 2. Prompt node: construct full prompt from state
    .add_function_node("step1_prompt", |state: &SharedState| {
        Box::pin(async move {
            let tpl: String = state.get("tpl_step1").unwrap_or_default();
            state.set("step1_prompt", tpl);
            Ok(())
        })
    })
    // 3. Agent node: execute the task
    .add_shared_agent_node_with_mode(
        "step1", agent.clone(), "step1_prompt", "step1_output", false,
    )
    // 4. Connect edges
    .set_entry("init")
    .add_edge("init", "step1_prompt")
    .add_edge("step1_prompt", "step1")
    .set_finish("step1")
    .build()?;

let state = SharedState::new();
let result = graph.run(state).await?;
```

### Adding Custom Stages

To extend beyond the built-in pipelines:

1. **Add custom nodes before/after a pipeline**: Build a larger Graph that contains the pipeline as a subgraph
2. **Customize via Agent system prompt**: Influence stage behavior through the Agent's system prompt
3. **Compose multiple pipelines**: e.g., run data pipeline first, feed `summary` as the writing pipeline's `topic`

```rust
// Compose: Data Analysis → Report Writing
let data_result = run_data_pipeline(&analyst, data_config).await?;
let summary: String = data_result.state.get("summary").unwrap_or_default();

let writing_config = WritingPipelineConfig::new(format!("Data Analysis Report: {}", summary))
    .with_audience("management")
    .with_format("executive report");

let report = run_writing_pipeline(&writer, writing_config).await?;
```

---

## Best Practices

1. **Configure relevant tools for the Agent**: data pipeline pairs with data tools (CSV, Excel, Statistics); writing pipeline pairs with search tools
2. **Set quality thresholds wisely**: too low lets poor quality through; too high causes unnecessary loops
3. **Leverage intermediate results**: pipelines produce rich intermediate data useful for debugging, display, and post-processing
4. **Monitor execution paths**: use `result.path` to see which nodes were actually traversed (especially writing pipeline loop count)
5. **Set reasonable max_revisions**: prevents infinite loops while allowing enough optimization room (recommended: 2-5)

---

## When to Use

| Scenario | Pipeline | Reason |
|----------|----------|--------|
| Dataset exploration & insights | Data | Automated end-to-end analysis |
| A/B test analysis report | Data | Combines with statistical tools |
| Technical blog post | Writing | Quality loop ensures content quality |
| Research report writing | Writing | Multi-revision iteration toward target quality |
| Custom ETL pipeline | Custom | Follow pipeline patterns to build your own |

See: `echo-orchestration/src/workflow/pipelines/data_pipeline.rs`, `echo-orchestration/src/workflow/pipelines/writing_pipeline.rs`
