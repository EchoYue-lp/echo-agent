# 数据工具输出格式参考

**Feature**: `data` | **引擎**: Polars | **支持格式**: CSV, JSON, Parquet

## 标准化输出 Envelope

所有数据工具（已标准化部分）使用统一的输出信封格式：

```json
{
  "tool": "tool_name",
  "rows": 100,
  "columns": 5,
  "column_names": ["col_a", "col_b", "col_c", "col_d", "col_e"],
  "truncated": false,
  "data": [
    {"col_a": 1, "col_b": "x", ...},
    {"col_a": 2, "col_b": "y", ...}
  ]
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `tool` | string | 工具名称 |
| `rows` | integer | 数据总行数 |
| `columns` | integer | 列数 |
| `column_names` | string[] | 列名列表 |
| `truncated` | boolean | 数据是否因行数限制而被截断 |
| `data` | object[] | 数据行数组（最多 `max_preview_rows` 行） |

已标准化工具：`filter_data`, `aggregate_data`, `transform_data`, `topn_data`, `correlate_data`

## 各工具详细输出

### read_data — 数据读取

```json
{
  "file": "/path/data.csv",
  "format": "csv",
  "rows": 1000,
  "columns": 5,
  "column_info": [
    {"name": "id", "dtype": "Int64", "nulls": 0},
    {"name": "name", "dtype": "String", "nulls": 5}
  ],
  "preview_rows": 50,
  "preview": [{...}, ...]
}
```

### filter_data — 数据过滤

标准化 envelope + 额外字段:
- `filter`: 过滤表达式字符串
- `matched_rows`: 匹配行数

### aggregate_data — 聚合

标准化 envelope + 额外字段:
- `group_by`: 分组列（可选）

### transform_data — 转换

标准化 envelope + 额外字段:
- `operation`: 操作类型（sort/select/drop/rename）
- `params`: 操作参数

### data_stats — 列统计

```json
{
  "file": "/path/data.csv",
  "total_rows": 1000,
  "total_cols": 5,
  "columns": [
    {
      "name": "age",
      "type": "Int64",
      "null_count": 10,
      "null_pct": 1.0,
      "distinct_count": 85,
      "numeric_stats": {
        "mean": 35.2, "std": 12.1, "min": 18, "max": 78,
        "median": 34, "p25": 26, "p75": 44, "p90": 55, "p95": 63
      }
    }
  ]
}
```

### profile_data — 数据画像

```json
{
  "file": "/path/data.csv",
  "rows": 1000,
  "cols": 10,
  "columns": [...],
  "summary": {
    "dimensions": 5,
    "metrics": 3,
    "temporal": 1,
    "other": 1
  },
  "suggestions": ["Consider encoding 'region' as categorical", ...]
}
```

### topn_data — TopN 分析

标准化 envelope + 额外字段:
- `top_n`: N 值
- `metric_column`: 排序指标列
- `ascending`: 排序方向
- `dimension_columns`: 维度列（可选）

### contribution_data — 贡献率分析

```json
{
  "dimension_column": "region",
  "metric_column": "revenue",
  "total": 1000000.0,
  "items": [
    {"value": "East", "metric": 450000.0, "pct": 45.0, "cumulative_pct": 45.0},
    {"value": "West", "metric": 300000.0, "pct": 30.0, "cumulative_pct": 75.0}
  ],
  "other": {"count": 3, "metric": 250000.0, "pct": 25.0}
}
```

### bin_data — 数值分箱

```json
{
  "column": "age",
  "method": "equal_width",
  "num_bins": 5,
  "range": {"min": 18, "max": 78},
  "total_count": 1000,
  "bins": [
    {"label": "[18.0, 30.0)", "count": 250, "pct": 25.0},
    {"label": "[30.0, 42.0)", "count": 350, "pct": 35.0}
  ]
}
```

### correlate_data — 相关性矩阵

```json
{
  "tool": "correlate_data",
  "method": "pearson",
  "columns": ["age", "income", "score"],
  "matrix": [
    [1.0, 0.45, -0.12],
    [0.45, 1.0, 0.33],
    [-0.12, 0.33, 1.0]
  ]
}
```

### join_data — 数据连接

```json
{
  "left_file": "orders.csv",
  "right_file": "customers.csv",
  "join_keys": ["customer_id"],
  "join_type": "left",
  "total_rows": 5000,
  "columns": 8,
  "preview": [{...}, ...]
}
```

### pivot_data — 数据透视

```json
{
  "pivot_result": {
    "index": ["region"],
    "columns": "product_type",
    "pivot_values": ["Electronics", "Clothing"],
    "values": "revenue",
    "agg_function": "sum",
    "shape": [5, 3],
    "data": [{...}, ...]
  }
}
```

## 数据质量工具输出 (feature: `data`)

### missing_value_analysis

```json
{
  "file": "data.csv",
  "rows": 1000,
  "columns": 5,
  "total_missing_cells": 150,
  "overall_missing_pct": 3.0,
  "per_column": [
    {"column": "age", "missing_count": 10, "missing_pct": 1.0, "pattern": "scattered", "suggestion": "median_imputation"}
  ]
}
```

### outlier_detection

```json
{
  "file": "data.csv",
  "method": "iqr",
  "threshold": 1.5,
  "columns": [
    {"column": "salary", "q1": 30000, "q3": 60000, "iqr": 30000, "outlier_count": 5, "outlier_samples": [150000, 200000]}
  ]
}
```

### consistency_check

```json
{
  "file": "data.csv",
  "total_issues": 3,
  "severity_counts": {"high": 1, "medium": 1, "low": 1},
  "issues": [
    {"column": "age", "type": "range_violation", "detail": "2 values outside range", "severity": "high"}
  ]
}
```
