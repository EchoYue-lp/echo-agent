# Data Tool Output Format Reference

**Feature**: `data` | **Engine**: Polars | **Formats**: CSV, JSON, Parquet

## Standardized Output Envelope

All standardized data tools use a unified output envelope:

```json
{
  "tool": "tool_name",
  "rows": 100,
  "columns": 5,
  "column_names": ["col_a", "col_b", "col_c", "col_d", "col_e"],
  "truncated": false,
  "data": [
    {"col_a": 1, "col_b": "x"},
    {"col_a": 2, "col_b": "y"}
  ]
}
```

| Field | Type | Description |
|-------|------|-------------|
| `tool` | string | Tool name |
| `rows` | integer | Total row count |
| `columns` | integer | Column count |
| `column_names` | string[] | List of column names |
| `truncated` | boolean | Whether data was truncated due to row limit |
| `data` | object[] | Row data array (up to `max_preview_rows` rows) |

Standardized tools: `filter_data`, `aggregate_data`, `transform_data`, `topn_data`, `correlate_data`

## Tool-Specific Output

### read_data

```json
{
  "file": "/path/data.csv",
  "format": "csv",
  "rows": 1000,
  "columns": 5,
  "column_info": [
    {"name": "id", "dtype": "Int64", "nulls": 0}
  ],
  "preview_rows": 50,
  "preview": [{"id": 1, "name": "Alice"}]
}
```

### filter_data

Standardized envelope + extra fields:
- `filter`: Filter expression string
- `matched_rows`: Number of matching rows

### aggregate_data

Standardized envelope + extra fields:
- `group_by`: Group-by columns (optional)

### correlate_data

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

### contribution_data

```json
{
  "dimension_column": "region",
  "metric_column": "revenue",
  "total": 1000000.0,
  "items": [
    {"value": "East", "metric": 450000.0, "pct": 45.0, "cumulative_pct": 45.0}
  ],
  "other": {"count": 3, "metric": 250000.0, "pct": 25.0}
}
```

### bin_data

```json
{
  "column": "age",
  "method": "equal_width",
  "num_bins": 5,
  "range": {"min": 18, "max": 78},
  "total_count": 1000,
  "bins": [
    {"label": "[18.0, 30.0)", "count": 250, "pct": 25.0}
  ]
}
```

## Data Quality Tool Output (feature: `data`)

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
    {"column": "salary", "q1": 30000, "q3": 60000, "iqr": 30000, "outlier_count": 5, "outlier_samples": [150000]}
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
