# Pipelines —— 预置工作流管道

## 是什么

Pipelines 是基于 Graph Workflow 构建的预置工作流，封装常见 Agent 任务模式：

| Pipeline | 阶段 | 描述 |
|----------|------|------|
| [`data_pipeline`] | 检查数据 → 保存脚本 → 执行 → 验证产物 | 代码优先的可复现数据分析 |
| [`writing_pipeline`] | outline → draft → review → revise（循环）→ finalize | 带质量循环的内容创作 |

每个 pipeline 接收一个 `SharedAgent` 和配置，构建 Graph 并返回 `GraphResult`。数据管道只使用一次具备工具能力的 Agent 执行，让已保存脚本和真实运行产物成为事实源。

---

## Data Pipeline —— 数据分析管道

### 分析合同

```
检查真实数据集
  → 写入 <artifact_dir>/manifest.json 与 analysis.py|analysis.R
  → 通过 run_code(script_path=...) 执行已保存文件
  → 生成 environment.json、result.json、outputs/、runs/、latest-run.json
  → 验证哈希、诊断、假设和限制
  → 返回基于产物的摘要
```

管道不再让多个纯文本阶段分别猜测画像、统计结论和图表方案。具备工具能力的 Agent 写出完整可审阅脚本，执行同一文件，只报告已观察到的结果。正式推断必须使用 SciPy、statsmodels 或成熟 R 包；依赖缺失时返回失败，不允许回退到框架自研近似。

### 必需产物

| 路径 | 用途 |
|------|------|
| `manifest.json` | 合同版本、输入、目标、语言、参数、随机种子和时间戳 |
| `analysis.py` 或 `analysis.R` | 完整可审阅分析代码 |
| `environment.json` | 运行时和包版本 |
| `result.json` | 结构化分析结果 |
| `outputs/` | 表格、图表和报告 |
| `runs/<run-id>.json` | 不可变的运行终态、哈希、诊断、告警和限制 |
| `latest-run.json` | 最近一次运行的投影 |

### DataPipelineConfig

```rust
pub struct DataPipelineConfig {
    /// 工作区相对输入路径
    pub dataset_path: String,
    /// 可选分析目标
    pub objective: Option<String>,
    /// 工作区相对产物目录
    pub artifact_dir: String,
    /// Python 或 R
    pub language: DataPipelineLanguage,
    /// 最大图表数量
    pub max_charts: u32,
    /// 可复现随机种子
    pub random_seed: Option<u64>,
    /// 写入 manifest 的结构化参数
    pub parameters: serde_json::Value,
}
```

**Builder 方法：**

| 方法 | 描述 |
|------|------|
| `new(path)` | 用路径创建，其余取默认值 |
| `with_objective(obj)` | 设置分析目标 |
| `with_artifact_dir(path)` | 设置工作区相对产物目录 |
| `with_language(language)` | 选择 Python 或 R |
| `with_max_charts(n)` | 设置最大图表数 |
| `with_random_seed(seed)` | 设置随机种子 |
| `without_random_seed()` | 标记严格确定性分析 |
| `with_parameters(value)` | 保存结构化参数 |

`new(path)` 默认派生 `analysis/<dataset-stem>` 产物目录。默认使用 Python、最多三张图、随机种子 42、空参数对象。输入和产物路径必须位于 Agent 工作目录内。

### API：run_data_pipeline

```rust
pub async fn run_data_pipeline(
    agent: &SharedAgent,
    config: DataPipelineConfig,
) -> Result<GraphResult>
```

State 包含 `analysis_execution`、`summary`、`dataset_path`、`artifact_dir`、`analysis_language`、`script_path`、`contract_version`、`parameters`、`random_seed` 和 `max_charts`。

### 代码示例

#### 基本用法

```rust
use echo_agent::workflow::pipelines::{
    DataPipelineConfig, DataPipelineLanguage, run_data_pipeline,
};
use echo_agent::workflow::shared_agent;
use echo_agent::agent::ReactAgentBuilder;

# async fn example() -> echo_agent::error::Result<()> {
let agent = shared_agent(
    ReactAgentBuilder::simple("qwen3-max", "数据分析师")?
);

let config = DataPipelineConfig::new("data/sales_2024.csv")
    .with_objective("识别收入趋势和季节性模式")
    .with_artifact_dir("analysis/revenue-trends")
    .with_language(DataPipelineLanguage::Python)
    .with_max_charts(5)
    .with_random_seed(42)
    .with_parameters(serde_json::json!({"group_by": "region"}));

let result = run_data_pipeline(&agent, config).await?;

let summary: String = result.state.get("summary").unwrap_or_default();
println!("分析摘要:\n{}", summary);
# Ok(())
# }
```

#### Agent 能力要求

Agent 必须具备文件读写工具和 `run_code`。CSV、Excel、Parquet 或探索性统计工具可以辅助检查输入，但所有转换和正式推断都由已保存的 Python/R 脚本承担。`run_code` 必须执行保存后的 `script_path`，不能执行一份重复的内联代码。标准 registry 中最小相关工具名是 `read_file`、`write_file` 和 `run_code`。

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

# async fn example() -> echo_agent::error::Result<()> {
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

### 访问结果

两个 pipeline 返回的 `GraphResult` 都包含完整的 `SharedState`。数据管道暴露分析合同元数据和基于产物的执行报告：

```rust
let result = run_data_pipeline(&agent, config).await?;

// 列出所有 state keys
for key in result.state.keys() {
    println!("State key: {}", key);
}

let artifact_dir: String = result.state.get("artifact_dir").unwrap_or_default();
let summary: String = result.state.get("summary").unwrap_or_default();
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
2. **修改 Agent system prompt**：补充领域要求，但不削弱文件合同
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

1. **配置必需工具**：data pipeline 必须有文件读写和 `run_code`，格式读取器只作为可选辅助
2. **合理设置质量阈值**：过低导致质量不达标，过高导致不必要的循环
3. **审阅真实产物**：信任结论前检查已保存脚本、manifest、环境、结果、哈希和诊断
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
