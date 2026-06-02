# Pipelines —— 预置工作流管道

## 是什么

Pipelines 是基于 Graph Workflow 构建的**预置多阶段工作流**，封装常见 Agent 任务模式：

| Pipeline | 阶段 | 描述 |
|----------|------|------|
| [`data_pipeline`] | load_data → profile → analyze → visualize → summarize | 端到端数据分析 |
| [`writing_pipeline`] | outline → draft → review → revise（循环）→ finalize | 带质量循环的内容创作 |

每个 pipeline 接收一个 `SharedAgent` 和配置，内部构建 Graph 并执行，返回包含所有中间结果的 `GraphResult`。

---

## Data Pipeline —— 数据分析管道

### 流程图

```
┌──────────────────────────────────────────────────────────────────┐
│                     Data Pipeline 流程                           │
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

  每阶段输出写入 SharedState，下游阶段读取上游结果
```

### 阶段说明

| 阶段 | 职责 | State Key（输出） |
|------|------|-------------------|
| `init` | 从配置生成 prompt 模板 | `tpl_load`, `tpl_profile`, ... |
| `load_data` | 读取数据集，描述行数、列名、样例 | `loaded_data` |
| `profile` | 统计概览：均值/中位数/标准差/缺失值/类型 | `data_profile` |
| `analyze` | 相关性、异常值、分布、显著模式 | `analysis` |
| `visualize` | 生成图表规格（类型、坐标轴、标题） | `visualizations` |
| `summarize` | 执行摘要：Top 3-5 洞察 + 建议 + 限制 | `summary` |

### DataPipelineConfig

```rust
pub struct DataPipelineConfig {
    /// 数据集文件路径（CSV、JSON、Parquet 等）
    pub dataset_path: String,
    /// 可选的分析目标/问题
    pub objective: Option<String>,
    /// 最多生成的图表数量
    pub max_charts: u32,
}
```

**Builder 方法：**

| 方法 | 描述 |
|------|------|
| `new(path)` | 用路径创建，其余取默认值 |
| `with_objective(obj)` | 设置分析目标 |
| `with_max_charts(n)` | 设置最大图表数 |

**默认值：** `objective = None`（通用探索），`max_charts = 3`

### API：run_data_pipeline

```rust
pub async fn run_data_pipeline(
    agent: &SharedAgent,
    config: DataPipelineConfig,
) -> Result<GraphResult>
```

返回值 `GraphResult` 包含：
- `state` — `SharedState`，可通过 key 访问所有中间和最终结果
- `steps` — 执行步骤数
- `path` — 执行路径（经过的节点名列表）

### 代码示例

#### 基本用法

```rust
use echo_agent::workflow::pipelines::{DataPipelineConfig, run_data_pipeline};
use echo_agent::workflow::shared_agent;
use echo_agent::agent::ReactAgentBuilder;

# async fn example() -> echo_core::error::Result<()> {
let agent = shared_agent(
    ReactAgentBuilder::simple("qwen3-max", "数据分析师")?
);

let config = DataPipelineConfig::new("/data/sales_2024.csv")
    .with_objective("识别收入趋势和季节性模式")
    .with_max_charts(5);

let result = run_data_pipeline(&agent, config).await?;

// 获取最终摘要
let summary: String = result.state.get("summary").unwrap_or_default();
println!("分析摘要:\n{}", summary);

// 获取中间结果
let profile: String = result.state.get("data_profile").unwrap_or_default();
let viz: String = result.state.get("visualizations").unwrap_or_default();
println!("数据画像:\n{}", profile);
println!("可视化方案:\n{}", viz);
# Ok(())
# }
```

#### 结合 Data Tools

当 Agent 配置了数据工具（Excel、CSV、统计等）时，pipeline 各阶段可自动调用工具完成实际计算：

```rust
use echo_agent::tools::{ExcelTool, CsvTool, StatisticsTool};

let agent = shared_agent(
    ReactAgentBuilder::simple("qwen3-max", "数据分析师")?
        .with_tool(ExcelTool::new())
        .with_tool(CsvTool::new())
        .with_tool(StatisticsTool::new())
);

let config = DataPipelineConfig::new("/data/experiment_results.xlsx")
    .with_objective("比较 A/B 测试组的显著性差异");

let result = run_data_pipeline(&agent, config).await?;
// Agent 在 analyze 阶段会自动调用 StatisticsTool 进行统计检验
```

---

## Writing Pipeline —— 内容创作管道

### 流程图

```
┌───────────────────────────────────────────────────────────────────┐
│                     Writing Pipeline 流程                         │
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
│                       └──▶ review（循环）      │
└───────────────────────────────────────────────────────────────────┘

  质量循环：review → evaluate → revise → review，直到达标或达到最大次数
```

### 阶段说明

| 阶段 | 职责 | State Key（输出） |
|------|------|-------------------|
| `init` | 从配置生成 prompt 模板 | `tpl_outline`, `tpl_draft`, ... |
| `outline` | 创建结构化大纲（标题、章节、要点） | `outline` |
| `draft` | 按大纲撰写完整初稿 | `draft` |
| `review` | 评审并打分（0-100），输出 `QUALITY_SCORE: <n>` | `review` |
| `evaluate_quality` | 从 review 文本提取分数，判断是否循环 | `quality_score` |
| `revise` | 根据评审反馈修订初稿（覆盖 `draft`） | `draft`（覆写） |
| `increment_revision` | 修订计数器 +1 | `revision_count` |
| `finalize` | 最终润色，输出发表级内容 | `final_output` |

### 质量循环机制

review 阶段要求 Agent 在输出开头标注 `QUALITY_SCORE: <number>`，`evaluate_quality` 节点提取该分数后决策：

| 条件 | 动作 |
|------|------|
| `score ≥ threshold` | 进入 finalize |
| `score < threshold` 且 `revision_count < max_revisions` | 进入 revise → 重新 review |
| `revision_count ≥ max_revisions` | 进入 finalize（即使未达标） |

**分数提取优先级：**
1. `QUALITY_SCORE: <number>`（精确匹配）
2. `Score: <number>`（回退匹配）
3. 默认 60 分（未找到分数时）

### WritingPipelineConfig

```rust
pub struct WritingPipelineConfig {
    /// 写作主题
    pub topic: String,
    /// 目标读者
    pub audience: String,
    /// 输出格式（如 "博客文章"、"论文"、"报告"、"白皮书"）
    pub format: String,
    /// 最大修订次数（review → revise 循环）
    pub max_revisions: u32,
    /// 质量分数阈值（0-100），低于此值触发修订
    pub quality_threshold: u32,
}
```

**Builder 方法：**

| 方法 | 描述 |
|------|------|
| `new(topic)` | 用主题创建，其余取默认值 |
| `with_audience(aud)` | 设置目标读者 |
| `with_format(fmt)` | 设置输出格式 |
| `with_max_revisions(n)` | 设置最大修订次数 |
| `with_quality_threshold(t)` | 设置质量阈值 |

**默认值：** `audience = "general readers"`，`format = "blog post"`，`max_revisions = 2`，`quality_threshold = 80`

### API：run_writing_pipeline

```rust
pub async fn run_writing_pipeline(
    agent: &SharedAgent,
    config: WritingPipelineConfig,
) -> Result<GraphResult>
```

返回值 `GraphResult` 包含：
- `state` — `SharedState`，可通过 key 访问所有中间和最终结果
- `steps` — 执行步骤数（含循环步骤）
- `path` — 执行路径

### 代码示例

#### 基本用法

```rust
use echo_agent::workflow::pipelines::{WritingPipelineConfig, run_writing_pipeline};
use echo_agent::workflow::shared_agent;
use echo_agent::agent::ReactAgentBuilder;

# async fn example() -> echo_core::error::Result<()> {
let agent = shared_agent(
    ReactAgentBuilder::simple("qwen3-max", "写作助手")?
);

let config = WritingPipelineConfig::new("AI Agent 的崛起")
    .with_audience("技术从业者")
    .with_format("博客文章")
    .with_max_revisions(3)
    .with_quality_threshold(85);

let result = run_writing_pipeline(&agent, config).await?;

// 获取最终内容
let final_output: String = result.state.get("final_output").unwrap_or_default();
println!("最终稿件:\n{}", final_output);

// 检查质量指标
let score: i64 = result.state.get("quality_score").unwrap_or(0);
let revisions: i64 = result.state.get("revision_count").unwrap_or(0);
println!("质量分数: {}, 修订次数: {}", score, revisions);
# Ok(())
# }
```

#### 学术论文写作

```rust
let config = WritingPipelineConfig::new("基于 Transformer 的多模态融合方法综述")
    .with_audience("计算机科学研究者")
    .with_format("学术论文")
    .with_max_revisions(5)
    .with_quality_threshold(90);

let result = run_writing_pipeline(&agent, config).await?;

let outline: String = result.state.get("outline").unwrap_or_default();
let final_paper: String = result.state.get("final_output").unwrap_or_default();
```

---

## 定制与扩展

### 访问中间结果

两个 pipeline 返回的 `GraphResult` 都包含完整的 `SharedState`，可直接读取任意阶段的输出：

```rust
let result = run_data_pipeline(&agent, config).await?;

// 列出所有 state keys
for key in result.state.keys() {
    println!("State key: {}", key);
}

// 读取特定阶段结果
let analysis: String = result.state.get("analysis").unwrap_or_default();
```

### 基于 Pipeline 构建自定义工作流

Pipeline 内部使用 `GraphBuilder`，你可以参考相同模式构建自定义管道。核心模式：

```rust
use echo_agent::workflow::{GraphBuilder, SharedState, shared_agent};

let agent = shared_agent(/* ... */);

let graph = GraphBuilder::new("custom_pipeline")
    // 1. init 节点：注入配置和 prompt 模板到 state
    .add_function_node("init", |state: &SharedState| {
        Box::pin(async move {
            state.set("tpl_step1", "你的 prompt 模板...".to_string());
            Ok(())
        })
    })
    // 2. prompt 节点：从 state 构造完整 prompt
    .add_function_node("step1_prompt", |state: &SharedState| {
        Box::pin(async move {
            let tpl: String = state.get("tpl_step1").unwrap_or_default();
            state.set("step1_prompt", tpl);
            Ok(())
        })
    })
    // 3. agent 节点：执行任务
    .add_shared_agent_node_with_mode(
        "step1", agent.clone(), "step1_prompt", "step1_output", false,
    )
    // 4. 连接边
    .set_entry("init")
    .add_edge("init", "step1_prompt")
    .add_edge("step1_prompt", "step1")
    .set_finish("step1")
    .build()?;

let state = SharedState::new();
let result = graph.run(state).await?;
```

### 添加自定义阶段

要在现有 pipeline 基础上扩展，可以：

1. **在 pipeline 前后添加自定义节点**：构建包含 pipeline 子图的更大 Graph
2. **修改 prompt 模板**：通过自定义 Agent 的 system prompt 影响各阶段行为
3. **组合多个 pipeline**：例如先跑 data pipeline，将 `summary` 作为 writing pipeline 的 `topic`

```rust
// 组合：数据分析 → 报告撰写
let data_result = run_data_pipeline(&analyst, data_config).await?;
let summary: String = data_result.state.get("summary").unwrap_or_default();

let writing_config = WritingPipelineConfig::new(format!("数据分析报告：{}", summary))
    .with_audience("管理层")
    .with_format("执行报告");

let report = run_writing_pipeline(&writer, writing_config).await?;
```

---

## 最佳实践

1. **为 Agent 配置相关工具**：data pipeline 搭配数据工具（CSV、Excel、统计），writing pipeline 搭配搜索工具
2. **合理设置质量阈值**：过低导致质量不达标，过高导致不必要的循环
3. **利用中间结果**：pipeline 产出丰富的中间数据，可用于调试、展示、二次加工
4. **监控执行路径**：通过 `result.path` 了解实际执行了哪些节点（特别是 writing pipeline 的循环次数）
5. **设置合理的 max_revisions**：避免无限循环，同时给足优化空间（推荐 2-5 次）

---

## 适用场景

| 场景 | Pipeline | 原因 |
|------|----------|------|
| 数据集探索与洞察 | Data | 自动完成从加载到摘要全流程 |
| A/B 测试分析报告 | Data | 结合统计工具自动分析 |
| 技术博客创作 | Writing | 质量循环保证内容质量 |
| 研究报告撰写 | Writing | 多轮修订逼近目标质量 |
| 自定义 ETL 管道 | 自定义 | 参考 pipeline 模式构建 |

参见：`echo-orchestration/src/workflow/pipelines/data_pipeline.rs`、`echo-orchestration/src/workflow/pipelines/writing_pipeline.rs`
