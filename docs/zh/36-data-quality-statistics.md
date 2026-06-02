# 数据质量与统计分析

## 这篇文档讲什么

本文档介绍 `echo-agent` 中的数据质量评估与统计分析工具。这些工具让 Agent 变成一个数据分析师——可以对数据集做画像、发现质量问题、执行统计检验、生成严谨的数值报告，全程不需要写 Python 或 R 脚本。

工具分布在两个 Cargo feature 下：

| Feature | 提供什么 | 本文涉及的工具数 |
|---------|---------|----------------|
| `data` | 数据画像、描述性统计、缺失值分析、异常值检测、一致性校验、相关性分析 | 6 个（加上更广泛的数据转换工具集） |
| `statistics` | 假设检验、线性回归、高级描述性统计（偏度、峰度、置信区间） | 3 个 |

`statistics` 依赖 `data`，所以启用 `statistics` 会自动引入数据质量工具。

---

## 工具一览

| 目标 | 工具名 | 结构体 | Feature |
|------|--------|--------|---------|
| 快速了解数据集 | `profile_data` | `DataProfileTool` | `data` |
| 逐列详细统计 | `data_stats` | `DataStatsTool` | `data` |
| 缺失值分析 | `missing_value_analysis` | `MissingValueAnalysisTool` | `data` |
| 异常值检测 | `outlier_detection` | `OutlierDetectionTool` | `data` |
| 一致性 / Schema 校验 | `consistency_check` | `ConsistencyCheckTool` | `data` |
| 相关系数矩阵 | `correlate_data` | `CorrelateTool` | `data` |
| 假设检验 | `hypothesis_test` | `HypothesisTestTool` | `statistics` |
| 线性回归 | `regression` | `RegressionTool` | `statistics` |
| 高级描述性统计 | `descriptive_advanced` | `DescriptiveAdvancedTool` | `statistics` |

---

## 1. 数据质量工具（feature = `data`）

### 1.1 `profile_data` — 快速数据画像

当 Agent 遇到一个陌生的数据集时，`profile_data` 是推荐的第一步。它扫描每一列并自动分类为 **维度列（dimension）**、**指标列（metric）** 或 **时间列（temporal）**，然后给出轻量级汇总统计。

每列返回：

- 列类型（`dtype`）和自动检测的分类
- 空值数量和空值占比
- 去重数量和去重占比
- 数值列：最小值、最大值、均值、总和
- 字符串列：最短/最长/平均字符串长度
- 前 5 个样本值
- 维度列、指标列、时间列的计数汇总
- 后续工具建议（如"用 `topn_data` 做排名分析"、"用 `bin_data` 看分布"）

```json
{
  "tool": "profile_data",
  "parameters": {
    "file_path": "/data/sales.csv"
  }
}
```

适用场景：

- 第一次看一个新的 CSV / JSON / Parquet 文件
- 在深入分析之前理解每列的角色
- 决定接下来调用哪些工具

---

### 1.2 `data_stats` — 详细逐列统计

`data_stats` 在不分组的情况下计算每列的详细统计。当你需要精确的分位数或分布细节时用它。

数值列返回：

- 总数、空值数、空值率
- 去重数和去重率
- 均值、标准差、方差
- 最小值、最大值、中位数
- p25、p75、p90、p95 分位数

字符串列返回：

- 最短/最长/平均字符串长度
- 出现频率最高的 3 个值及其计数和占比

```json
{
  "tool": "data_stats",
  "parameters": {
    "file_path": "/data/sales.csv",
    "columns": "revenue,quantity,region"
  }
}
```

与 `aggregate_data` 的区别：`data_stats` 计算的是每列整体统计（不分组），`aggregate_data` 做的是分组聚合。

---

### 1.3 `missing_value_analysis` — 缺失值模式分析

`missing_value_analysis` 不仅统计空值数量，还分析缺失模式。对每一列它会：

1. 报告总数、非空数、缺失数及百分比。
2. 分类缺失模式：
   - `no_missing` — 没有空值
   - `all_missing` — 全部是空值
   - `monotonic_missing` — 空值出现在连续的一段（比如某列中途才开始采集）
   - `random_missing` — 空值随机散布
   - `scattered_missing` — 零星空值
3. 根据列类型和缺失率给出填充建议：
   - 数值列，缺失 <10% → 均值/中位数填充
   - 数值列，缺失 10–30% → 中位数或插值
   - 数值列，缺失 >30% → 考虑模型填充
   - 分类列 → 众数或添加"Unknown"类别
   - 时间列 → 前向/后向填充
   - 缺失 >80% → 考虑直接删除该列

```json
{
  "tool": "missing_value_analysis",
  "parameters": {
    "data_path": "/data/customers.csv"
  }
}
```

输出还包含整个数据集的 `overall_missing_pct`，是一个快速健康指标。

---

### 1.4 `outlier_detection` — 异常值检测

`outlier_detection` 用以下两种方法之一识别数值列中的异常值：

**IQR 方法**（默认，`method = "iqr"`）：
- 计算 Q1、Q3 和 IQR = Q3 − Q1。
- 标记超出 `[Q1 - k*IQR, Q3 + k*IQR]` 的值。
- 默认阈值 `k = 1.5`（Tukey 栅栏）。

**Z-score 方法**（`method = "zscore"`）：
- 计算均值和标准差。
- 标记 `|z| > threshold` 的值。
- 默认阈值 = 3.0。

参数：

| 参数 | 必填 | 默认值 | 说明 |
|------|------|--------|------|
| `data_path` | 是 | — | 数据文件绝对路径 |
| `columns` | 否 | 所有数值列 | 逗号分隔的列名 |
| `method` | 否 | `"iqr"` | `"iqr"` 或 `"zscore"` |
| `threshold` | 否 | 1.5（IQR）/ 3.0（Z） | 检测灵敏度 |

输出包含每列的 Q1、Q3、IQR、上下界、异常值数量、异常值占比，以及最多 10 个异常值样本。

```json
{
  "tool": "outlier_detection",
  "parameters": {
    "data_path": "/data/transactions.csv",
    "columns": "amount,fee",
    "method": "zscore",
    "threshold": 2.5
  }
}
```

最低要求：每列至少 4 个非空值。

---

### 1.5 `consistency_check` — 数据校验与 Schema 检查

`consistency_check` 做两层校验：

**自动检查**（始终执行）：
- **类型不匹配**：字符串列中 >80% 的值实际上是数字（很可能是 schema 错误）
- **空字符串**：字符串列中包含应该是 null 的空字符串
- **负值**：数值列中出现少量意外的负值
- **极端值**：数值偏离均值超过 5 个标准差

**自定义规则校验**（提供 `rules` 参数时执行）：

每条规则是一个 JSON 对象，包含 `column` 字段和 `type`：

| 规则类型 | 额外字段 | 检查内容 |
|----------|---------|---------|
| `range` | `min`、`max`（任一或两者） | 数值是否在范围内 |
| `regex` | `pattern` | 字符串值是否包含该模式 |

规则 JSON 示例：

```json
[
  {"column": "age", "type": "range", "min": 0, "max": 120},
  {"column": "email", "type": "regex", "pattern": "@"},
  {"column": "score", "type": "range", "min": 0, "max": 100}
]
```

每个问题都有严重级别：`high`、`medium` 或 `low`。输出包含 `severity_counts` 供快速了解整体健康度。

```json
{
  "tool": "consistency_check",
  "parameters": {
    "data_path": "/data/users.csv",
    "rules": "[{\"column\":\"age\",\"type\":\"range\",\"min\":0,\"max\":120}]"
  }
}
```

---

### 1.6 `correlate_data` — 相关系数矩阵

`correlate_data` 计算数值列之间的成对相关系数矩阵。

支持的方法：

- **Pearson**（默认）：线性相关，取值 [−1, 1]
- **Spearman**：基于秩的相关，对异常值更鲁棒

```json
{
  "tool": "correlate_data",
  "parameters": {
    "file_path": "/data/metrics.csv",
    "columns": "height,weight,age,income",
    "method": "pearson"
  }
}
```

用途：

- 发现高度相关的特征对（ML 特征冗余）
- 发现变量之间的意外关系
- 决定哪些列纳入回归分析

---

## 2. 统计分析工具（feature = `statistics`）

`statistics` feature 添加推断统计工具。它们构建在 `data` feature 的数据加载基础设施之上，由 `Cargo.toml` 中的 `feature = "statistics"` 控制。

### 2.1 `hypothesis_test` — 假设检验

`hypothesis_test` 支持三种检验类型：

#### t 检验（`test_type = "t_test"`）

Welch t 检验，用于比较两个数值列的均值（或一列与另一列）。

- 返回：t 统计量、自由度（Welch–Satterthwaite 近似）、p 值、结论。
- 最低要求：每列 2 个非空值。

```json
{
  "tool": "hypothesis_test",
  "parameters": {
    "data_path": "/data/experiment.csv",
    "test_type": "t_test",
    "column1": "control_group",
    "column2": "treatment_group",
    "alpha": 0.05
  }
}
```

#### 卡方独立性检验（`test_type = "chi_square"`）

检验两个分类列是否独立。

- 两列在内部被转为字符串。
- 构建观察列联表并计算期望频率。
- 返回：卡方统计量、自由度、p 值、观察表和期望表、结论。
- 最低要求：每列至少 2 个不同值。

```json
{
  "tool": "hypothesis_test",
  "parameters": {
    "data_path": "/data/survey.csv",
    "test_type": "chi_square",
    "column1": "gender",
    "column2": "preference"
  }
}
```

#### 相关性显著性检验（`test_type = "correlation_significance"`）

检验两个数值列之间的 Pearson 相关是否显著不为零。

- 返回：Pearson r、t 统计量、p 值、结论。
- 最低要求：3 对有效值。

```json
{
  "tool": "hypothesis_test",
  "parameters": {
    "data_path": "/data/students.csv",
    "test_type": "correlation_significance",
    "column1": "study_hours",
    "column2": "exam_score"
  }
}
```

三种检验都接受可选的 `alpha` 参数（默认 0.05），并返回人类可读的 `conclusion` 字符串和原始数值。

---

### 2.2 `regression` — 线性回归

`regression` 对一个目标列与一个或多个特征列做普通最小二乘线性回归。

对每个特征计算：

- 斜率（系数）和截距
- R²（决定系数）
- 斜率的标准误
- 斜率的 t 统计量和 p 值

还报告：

- 所有特征组合的总体 R²
- 残差平方和与总平方和
- 有效数据对数量

参数：

| 参数 | 必填 | 说明 |
|------|------|------|
| `data_path` | 是 | 数据文件绝对路径 |
| `target_column` | 是 | 因变量（必须是数值列） |
| `feature_columns` | 是 | 自变量，逗号分隔（至少一个，全部必须是数值列） |
| `output_path` | 否 | 将完整结果保存为 JSON 的路径 |

```json
{
  "tool": "regression",
  "parameters": {
    "data_path": "/data/housing.csv",
    "target_column": "price",
    "feature_columns": "area,bedrooms,age",
    "output_path": "/output/regression_results.json"
  }
}
```

适用场景：

- 量化特征与目标之间的关系
- 构建简单预测模型
- 在报告之前检查关系是否具有统计显著性

---

### 2.3 `descriptive_advanced` — 分布形态与置信区间

`descriptive_advanced` 计算超越均值和标准差的统计量，聚焦于分布形态和估计不确定性。

每个数值列返回：

- **偏度（Skewness）**：衡量不对称性。正值 = 右尾更长；负值 = 左尾更长；0 = 对称。
- **峰度（Kurtosis）**（超额峰度）：衡量尾部厚度。0 = 正态分布；正值 = 尾部更厚；负值 = 尾部更薄。
- **均值的置信区间**：在指定置信水平下的上下界，加均值标准误。

参数：

| 参数 | 必填 | 默认值 | 说明 |
|------|------|--------|------|
| `data_path` | 是 | — | 数据文件绝对路径 |
| `columns` | 否 | 所有数值列 | 逗号分隔的列名 |
| `confidence_level` | 否 | 0.95 | 置信区间的置信水平（如 0.90, 0.99） |

最低要求：每列至少 3 个非空值（偏度/峰度需要）。

```json
{
  "tool": "descriptive_advanced",
  "parameters": {
    "data_path": "/data/response_times.csv",
    "columns": "latency,throughput",
    "confidence_level": 0.99
  }
}
```

适用场景：

- 需要描述分布形态，而不仅是集中趋势
- 需要在报告均值时附带置信区间
- 下游检验对正态性假设有要求

---

## 3. 与数据工具的集成

质量与统计工具被设计为与更广泛的数据工具套件形成流水线：

```
read_data ──▶ profile_data ──▶ missing_value_analysis ──▶ consistency_check
                    │
                    ▼
              data_stats ──▶ outlier_detection
                    │
                    ▼
           correlate_data ──▶ regression
                    │
                    ▼
            hypothesis_test ──▶ descriptive_advanced
```

**数据加载**：所有工具共用同一个 `load_dataframe` 函数，根据文件扩展名自动检测 CSV、JSON 和 Parquet 格式。`data_path`（或 `file_path`）参数始终接受绝对路径，并通过 `SecurityConfig` 沙箱验证。

**Excel 集成**：先用 `excel_to_csv` 把表格转成 CSV，再把 CSV 路径传给任何质量或统计工具。也可以用 `excel_load`（feature `media` + `data`）直接把 Excel 加载到内存中的 Polars DataFrame。

**数据库集成**：用 `sql_query` 将查询结果导出为 CSV，然后用质量/统计工具分析。

**输出串联**：用 `export_data` 写出清洗或过滤后的中间结果，再让质量工具指向导出文件。

---

## 4. 代码示例

### 4.1 注册全部数据质量与统计工具

```rust
use echo_tools::registry::register_all_tools;

// 在你的 Agent 初始化中：
register_all_tools(&mut tool_manager);
// 这会为已启用的 feature 注册所有工具，包括：
// - 数据质量工具（feature = "data" 时）
// - 统计分析工具（feature = "statistics" 时）
```

### 4.2 单独注册工具

```rust
use echo_tools::data_quality::{
    MissingValueAnalysisTool,
    OutlierDetectionTool,
    ConsistencyCheckTool,
};
use echo_tools::statistics::{
    HypothesisTestTool,
    RegressionTool,
    DescriptiveAdvancedTool,
};

tool_manager.register(Box::new(MissingValueAnalysisTool));
tool_manager.register(Box::new(OutlierDetectionTool));
tool_manager.register(Box::new(ConsistencyCheckTool));
tool_manager.register(Box::new(HypothesisTestTool::default()));
tool_manager.register(Box::new(RegressionTool::default()));
tool_manager.register(Box::new(DescriptiveAdvancedTool::default()));
```

### 4.3 典型分析流程

一个常见的 Agent 数据分析工作流：

1. **画像**数据集以了解整体形态：
   ```
   profile_data(file_path='/data/sales.csv')
   ```

2. **检查质量**，确保数据可信：
   ```
   missing_value_analysis(data_path='/data/sales.csv')
   consistency_check(data_path='/data/sales.csv', rules='[{"column":"price","type":"range","min":0}]')
   ```

3. **检测异常值**：
   ```
   outlier_detection(data_path='/data/sales.csv', columns='revenue,quantity')
   ```

4. 在干净数据上**运行统计**：
   ```
   data_stats(file_path='/data/sales_clean.csv', columns='revenue')
   correlate_data(file_path='/data/sales_clean.csv', method='pearson')
   hypothesis_test(data_path='/data/sales_clean.csv', test_type='t_test', column1='region_a', column2='region_b')
   ```

5. **建模关系**：
   ```
   regression(data_path='/data/sales_clean.csv', target_column='revenue', feature_columns='ad_spend,price,season')
   ```

---

## 5. Feature 控制

在 `Cargo.toml` 中：

```toml
[dependencies]
echo_tools = { version = "0.2", features = ["data", "statistics"] }
```

Feature 依赖链：

```
statistics ──依赖──▶ data ──依赖──▶ polars
```

启用 `statistics` 会自动启用 `data` 并引入 Polars。如果只需要数据质量和描述性统计而不需要推断统计，只启用 `data` 即可。

`full` feature 启用全部功能：

```toml
echo_tools = { version = "0.2", features = ["full"] }
```

---

## 6. 安全性

所有数据质量与统计工具：

- 只需要 `ToolPermission::Read` — 它们永远不会修改源数据文件。
- 通过 `SecurityConfig::global()` 验证文件路径，强制执行已配置的安全沙箱边界。
- 在可能的情况下使用 Polars 的惰性求值，避免在不需要时将整个文件加载到内存。

`regression` 工具是唯一例外：如果提供了 `output_path`，它会向该路径写入结果，需要输出位置的 `ToolPermission::Write` 权限。

---

## 相关文档

- `docs/zh/02-tools.md` — 工具系统架构
- `docs/zh/21-common-tools.md` — 常用工具速查（包含数据工具）
- `docs/zh/22-research-tools.md` — 文献检索与研究工具
- `echo-tools/src/data_quality.rs` — 数据质量工具实现
- `echo-tools/src/statistics.rs` — 统计分析工具实现
- `echo-tools/src/data.rs` — 画像、统计与相关性工具实现
