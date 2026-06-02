# Data Quality & Statistics

## What This Page Covers

This page documents the data quality assessment and statistical analysis tools in `echo-agent`. These tools turn the Agent into a data analyst that can profile a dataset, detect quality issues, run statistical tests, and produce rigorous numeric summaries — all without writing custom Python or R scripts.

The tools are split across two Cargo features:

| Feature | What it adds | Tool count |
|---------|-------------|------------|
| `data` | Profiling, descriptive stats, missing-value analysis, outlier detection, consistency checks, correlation | 6 tools documented here (plus the broader data-transform suite) |
| `statistics` | Hypothesis testing, linear regression, advanced descriptive stats (skewness, kurtosis, CI) | 3 tools |

`statistics` depends on `data`, so enabling `statistics` automatically pulls in the data-quality tools as well.

---

## Tool Map

| Goal | Tool name | Struct | Feature |
|------|-----------|--------|---------|
| Quick dataset overview | `profile_data` | `DataProfileTool` | `data` |
| Per-column descriptive stats | `data_stats` | `DataStatsTool` | `data` |
| Missing value analysis | `missing_value_analysis` | `MissingValueAnalysisTool` | `data` |
| Outlier detection | `outlier_detection` | `OutlierDetectionTool` | `data` |
| Consistency / schema validation | `consistency_check` | `ConsistencyCheckTool` | `data` |
| Correlation matrix | `correlate_data` | `CorrelateTool` | `data` |
| Hypothesis tests | `hypothesis_test` | `HypothesisTestTool` | `statistics` |
| Linear regression | `regression` | `RegressionTool` | `statistics` |
| Advanced descriptive stats | `descriptive_advanced` | `DescriptiveAdvancedTool` | `statistics` |

---

## 1. Data Quality Tools (feature = `data`)

### 1.1 `profile_data` — Quick Dataset Profiling

`profile_data` is the recommended first step when the Agent encounters an unfamiliar dataset. It scans every column and automatically classifies it as a **dimension**, **metric**, or **temporal** column, then computes lightweight summary statistics.

What it returns per column:

- Column type (`dtype`) and auto-detected category
- Null count and null percentage
- Distinct count and distinct percentage
- Numeric columns: min, max, mean, sum
- String columns: min/max/average string length
- Top 5 sample values
- Summary counts of dimensions, metrics, and temporal columns
- Follow-up tool suggestions (e.g. "use `topn_data` for ranking", "use `bin_data` for distributions")

```json
{
  "tool": "profile_data",
  "parameters": {
    "file_path": "/data/sales.csv"
  }
}
```

When to use it:

- First look at a new CSV / JSON / Parquet file
- Understand column roles before running deeper analysis
- Decide which downstream tools to call

---

### 1.2 `data_stats` — Detailed Per-Column Statistics

`data_stats` computes thorough per-column statistics without grouping. It is the right tool when you need exact percentiles or distribution details.

Numeric columns get:

- count, null count, null rate
- distinct count and distinct rate
- mean, standard deviation, variance
- min, max, median
- p25, p75, p90, p95 percentiles

String columns get:

- min / max / average string length
- Top 3 most frequent values with counts and percentages

```json
{
  "tool": "data_stats",
  "parameters": {
    "file_path": "/data/sales.csv",
    "columns": "revenue,quantity,region"
  }
}
```

Difference from `aggregate_data`: `data_stats` produces per-column overall stats with no grouping; `aggregate_data` computes grouped aggregations.

---

### 1.3 `missing_value_analysis` — Missing Data Patterns

`missing_value_analysis` goes beyond raw null counts. For each column it:

1. Reports total, non-null, and missing counts with percentages.
2. Classifies the missing pattern:
   - `no_missing` — no nulls
   - `all_missing` — every value is null
   - `monotonic_missing` — nulls appear in a contiguous block (e.g. a column added mid-stream)
   - `random_missing` — nulls scattered without pattern
   - `scattered_missing` — isolated nulls
3. Suggests an imputation strategy based on column type and missing rate:
   - Numeric, <10% missing → mean/median imputation
   - Numeric, 10–30% → median or interpolation
   - Numeric, >30% → model-based imputation
   - Categorical → mode or "Unknown" category
   - Datetime → forward/backward fill
   - >80% missing → consider dropping the column

```json
{
  "tool": "missing_value_analysis",
  "parameters": {
    "data_path": "/data/customers.csv"
  }
}
```

The output also includes an `overall_missing_pct` across the entire dataset, which is a quick health indicator.

---

### 1.4 `outlier_detection` — Statistical Outlier Detection

`outlier_detection` identifies anomalous values in numeric columns using one of two methods:

**IQR method** (default, `method = "iqr"`):
- Computes Q1, Q3, and IQR = Q3 − Q1.
- Flags values outside `[Q1 - k*IQR, Q3 + k*IQR]`.
- Default threshold `k = 1.5` (Tukey's fences).

**Z-score method** (`method = "zscore"`):
- Computes mean and standard deviation.
- Flags values with `|z| > threshold`.
- Default threshold = 3.0.

Parameters:

| Parameter | Required | Default | Description |
|-----------|----------|---------|-------------|
| `data_path` | yes | — | Absolute path to the data file |
| `columns` | no | all numeric columns | Comma-separated column names |
| `method` | no | `"iqr"` | `"iqr"` or `"zscore"` |
| `threshold` | no | 1.5 (IQR) / 3.0 (Z) | Detection sensitivity |

The result includes Q1, Q3, IQR, bounds, outlier count, outlier percentage, and up to 10 sample outlier values per column.

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

Minimum requirement: at least 4 non-null values per column.

---

### 1.5 `consistency_check` — Data Validation and Schema Checks

`consistency_check` performs two layers of validation:

**Automatic checks** (always run):
- **Type mismatches**: string columns where >80% of values are actually numeric (likely a schema error)
- **Empty strings**: string columns containing empty strings that should be null
- **Negative values**: numeric columns with a small number of unexpected negatives
- **Extreme values**: numeric values beyond 5 standard deviations from the mean

**Custom rule validation** (when `rules` parameter is provided):

Each rule is a JSON object with a `column` field and a `type`:

| Rule type | Extra fields | What it checks |
|-----------|-------------|----------------|
| `range` | `min`, `max` (either or both) | Numeric values within bounds |
| `regex` | `pattern` | String values contain the pattern |

Example rules JSON:

```json
[
  {"column": "age", "type": "range", "min": 0, "max": 120},
  {"column": "email", "type": "regex", "pattern": "@"},
  {"column": "score", "type": "range", "min": 0, "max": 100}
]
```

Every issue carries a severity: `high`, `medium`, or `low`. The output includes `severity_counts` for a quick health overview.

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

### 1.6 `correlate_data` — Correlation Matrix

`correlate_data` computes a pairwise correlation matrix across numeric columns.

Supported methods:

- **Pearson** (default): linear correlation, values in [−1, 1]
- **Spearman**: rank-based correlation, more robust to outliers

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

The output is a formatted text matrix. Use it to:

- Spot highly correlated feature pairs (redundancy in ML features)
- Find unexpected relationships between variables
- Decide which columns to include in regression

---

## 2. Statistics Tools (feature = `statistics`)

The `statistics` feature adds inferential statistics tools. They build on the data loading infrastructure from the `data` feature and are gated behind `feature = "statistics"` in `Cargo.toml`.

### 2.1 `hypothesis_test` — Statistical Hypothesis Testing

`hypothesis_test` supports three test types:

#### t-test (`test_type = "t_test"`)

Welch's t-test for comparing means of two numeric columns (or one column against itself with a second).

- Returns: t-statistic, degrees of freedom (Welch–Satterthwaite), p-value, conclusion.
- Minimum: 2 non-null values per column.

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

#### Chi-square test of independence (`test_type = "chi_square"`)

Tests whether two categorical columns are independent.

- Both columns are cast to string internally.
- Builds an observed contingency table and computes expected frequencies.
- Returns: chi-square statistic, degrees of freedom, p-value, observed and expected tables, conclusion.
- Minimum: 2 unique values per column.

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

#### Correlation significance (`test_type = "correlation_significance"`)

Tests whether the Pearson correlation between two numeric columns is significantly different from zero.

- Returns: Pearson r, t-statistic, p-value, conclusion.
- Minimum: 3 valid pairs.

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

All three tests accept an optional `alpha` parameter (default 0.05) and return a human-readable `conclusion` string alongside the raw numbers.

---

### 2.2 `regression` — Linear Regression

`regression` performs ordinary least-squares linear regression between a target column and one or more feature columns.

For each feature it computes:

- Slope (coefficient) and intercept
- R² (coefficient of determination)
- Standard error of the slope
- t-statistic and p-value for the slope

It also reports:

- Overall R² across all features combined
- Residual sum of squares and total sum of squares
- Valid pair count

Parameters:

| Parameter | Required | Description |
|-----------|----------|-------------|
| `data_path` | yes | Absolute path to the data file |
| `target_column` | yes | Dependent variable (must be numeric) |
| `feature_columns` | yes | Independent variables, comma-separated (at least one, all numeric) |
| `output_path` | no | Path to save the full result as JSON |

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

Use it for:

- Quantifying how features relate to a target
- Building simple predictive models
- Checking whether a relationship is statistically significant before reporting it

---

### 2.3 `descriptive_advanced` — Distribution Shape & Confidence Intervals

`descriptive_advanced` computes statistics beyond mean and standard deviation, focusing on distribution shape and estimation uncertainty.

Per numeric column it returns:

- **Skewness**: measures asymmetry. Positive = right tail longer; negative = left tail longer; 0 = symmetric.
- **Kurtosis** (excess kurtosis): measures tail weight. 0 = normal distribution; positive = heavier tails; negative = lighter tails.
- **Confidence interval for the mean**: lower and upper bounds at the specified confidence level, plus standard error of the mean.

Parameters:

| Parameter | Required | Default | Description |
|-----------|----------|---------|-------------|
| `data_path` | yes | — | Absolute path to the data file |
| `columns` | no | all numeric columns | Comma-separated column names |
| `confidence_level` | no | 0.95 | Confidence level for the CI (e.g. 0.90, 0.99) |

Minimum: 3 non-null values per column (needed for skewness/kurtosis).

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

Use it when:

- You need to describe distribution shape, not just central tendency
- You want to report confidence intervals alongside means
- Normality assumptions matter for downstream tests

---

## 3. Integration with Data Tools

The quality and statistics tools are designed to work as a pipeline with the broader data tool suite:

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

**Data loading**: All tools share the same `load_dataframe` function, which auto-detects CSV, JSON, and Parquet formats from the file extension. The `data_path` (or `file_path`) parameter always takes an absolute path and is validated against the `SecurityConfig` sandbox.

**Excel integration**: Convert spreadsheets to CSV first with `excel_to_csv`, then feed the CSV path into any quality or statistics tool. Alternatively, `excel_load` (feature `media` + `data`) loads Excel directly into a Polars DataFrame in memory.

**Database integration**: Export query results to CSV using `sql_query` with an output path, then analyze with the quality/statistics tools.

**Output chaining**: Use `export_data` to write cleaned or filtered intermediate results, then point quality tools at the exported file.

---

## 4. Code Examples

### 4.1 Register all data quality and statistics tools

```rust
use echo_tools::registry::register_all_tools;

// In your agent setup:
register_all_tools(&mut tool_manager);
// This registers all tools for enabled features, including:
// - data quality tools (when feature = "data")
// - statistics tools (when feature = "statistics")
```

### 4.2 Register individual tools

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

### 4.3 Typical analysis workflow

A common Agent workflow for data analysis:

1. **Profile** the dataset to understand its shape:
   ```
   profile_data(file_path='/data/sales.csv')
   ```

2. **Check quality** before trusting results:
   ```
   missing_value_analysis(data_path='/data/sales.csv')
   consistency_check(data_path='/data/sales.csv', rules='[{"column":"price","type":"range","min":0}]')
   ```

3. **Detect outliers** in key metrics:
   ```
   outlier_detection(data_path='/data/sales.csv', columns='revenue,quantity')
   ```

4. **Run statistics** on clean data:
   ```
   data_stats(file_path='/data/sales_clean.csv', columns='revenue')
   correlate_data(file_path='/data/sales_clean.csv', method='pearson')
   hypothesis_test(data_path='/data/sales_clean.csv', test_type='t_test', column1='region_a', column2='region_b')
   ```

5. **Model relationships**:
   ```
   regression(data_path='/data/sales_clean.csv', target_column='revenue', feature_columns='ad_spend,price,season')
   ```

---

## 5. Feature Gates

In `Cargo.toml`:

```toml
[dependencies]
echo_tools = { version = "0.2", features = ["data", "statistics"] }
```

Feature dependency chain:

```
statistics ──depends on──▶ data ──depends on──▶ polars
```

Enabling `statistics` automatically enables `data` and pulls in Polars. If you only need data quality and descriptive stats without inferential statistics, enabling `data` alone is sufficient.

The `full` feature enables everything:

```toml
echo_tools = { version = "0.2", features = ["full"] }
```

---

## 6. Security

All data quality and statistics tools:

- Require `ToolPermission::Read` only — they never modify the source data file.
- Validate file paths through `SecurityConfig::global()`, which enforces the configured sandbox boundaries.
- Use Polars' lazy evaluation where possible to avoid loading entire files into memory when not needed.

The `regression` tool is the one exception: it can write results to `output_path` if provided, which requires `ToolPermission::Write` on the output location.

---

## Related Docs

- `docs/en/02-tools.md` — tool system architecture
- `docs/en/21-common-tools.md` — quick selection guide including data tools
- `docs/en/22-research-tools.md` — literature search and research tools
- `echo-tools/src/data_quality.rs` — data quality tool implementations
- `echo-tools/src/statistics.rs` — statistics tool implementations
- `echo-tools/src/data.rs` — profiling, stats, and correlation implementations
