//! Data processing tools
//!
//! Data processing capabilities based on Polars, supporting:
//! - CSV/JSON/Parquet file reading
//! - Data filtering, aggregation, sorting
//! - Statistical computation
//! - Data transformation
//! - Data profiling (dimension/metric identification)
//! - TopN / contribution analysis / numeric binning

use std::path::Path;

use futures::future::BoxFuture;
use polars::prelude::*;
use serde_json::Value;

use crate::security::SecurityConfig;
use echo_core::error::{Result, ToolError};
use echo_core::tools::permission::ToolPermission;
use echo_core::tools::{Tool, ToolParameters, ToolResult};

const TOOL_NAME: &str = "data_tools";

// ── Shared data loading helpers ──────────────────────────────────────

/// Detect format based on file extension
pub fn detect_format<'a>(path: &'a Path, hint: Option<&'a str>) -> &'a str {
    hint.unwrap_or_else(|| match path.extension().and_then(|e| e.to_str()) {
        Some("csv") | Some("txt") | Some("tsv") => "csv",
        Some("json") | Some("jsonl") => "json",
        Some("parquet") | Some("pq") => "parquet",
        _ => "csv",
    })
}

/// Load DataFrame (eager), supports CSV/JSON/Parquet
pub fn load_dataframe(path: &Path, format: Option<&str>) -> Result<DataFrame> {
    let fmt = detect_format(path, format);

    let file = std::fs::File::open(path).map_err(|e| ToolError::ExecutionFailed {
        tool: TOOL_NAME.to_string(),
        message: format!("Failed to open file: {}", e),
    })?;

    match fmt {
        "csv" => Ok(CsvReader::new(file)
            .finish()
            .map_err(|e| ToolError::ExecutionFailed {
                tool: TOOL_NAME.to_string(),
                message: format!("Failed to read CSV: {}", e),
            })?),
        "json" => {
            let file2 = std::fs::File::open(path).map_err(|e| ToolError::ExecutionFailed {
                tool: TOOL_NAME.to_string(),
                message: format!("Failed to open JSON file: {}", e),
            })?;
            Ok(JsonReader::new(file2)
                .finish()
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("Failed to read JSON: {}", e),
                })?)
        }
        "parquet" => {
            let file2 = std::fs::File::open(path).map_err(|e| ToolError::ExecutionFailed {
                tool: TOOL_NAME.to_string(),
                message: format!("Failed to open Parquet file: {}", e),
            })?;
            Ok(ParquetReader::new(file2)
                .finish()
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("Failed to read Parquet: {}", e),
                })?)
        }
        _ => Err(ToolError::InvalidParameter {
            name: "format".to_string(),
            message: format!("Unsupported file format: '{}'", fmt),
        }
        .into()),
    }
}

/// Load LazyFrame, supports CSV/JSON/Parquet
fn load_lazyframe(path: &Path, format: Option<&str>) -> Result<LazyFrame> {
    let fmt = detect_format(path, format);
    let path_str = path.to_string_lossy().to_string();

    match fmt {
        "csv" => Ok(LazyCsvReader::new(PlRefPath::from(path_str.as_str()))
            .finish()
            .map_err(|e| ToolError::ExecutionFailed {
                tool: TOOL_NAME.to_string(),
                message: format!("Failed to read CSV: {}", e),
            })?),
        "json" => {
            // For JSON, we use the eager reader and convert to LazyFrame
            let df = load_dataframe(path, Some("json"))?;
            Ok(df.lazy())
        }
        "parquet" => {
            let file = std::fs::File::open(path).map_err(|e| ToolError::ExecutionFailed {
                tool: TOOL_NAME.to_string(),
                message: format!("Failed to open Parquet file: {}", e),
            })?;
            let df = ParquetReader::new(file)
                .finish()
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("Failed to read Parquet: {}", e),
                })?;
            Ok(df.lazy())
        }
        _ => Err(ToolError::InvalidParameter {
            name: "format".to_string(),
            message: format!("Unsupported file format: '{}'", fmt),
        }
        .into()),
    }
}

/// Build a standardized data tool response with consistent metadata.
///
/// Output envelope format:
/// ```json
/// {
///   "tool": "tool_name",
///   "rows": 100,
///   "columns": 5,
///   "column_names": ["a", "b"],
///   "truncated": false,
///   "data": [ ... ]
/// }
/// ```
fn data_tool_response(tool_name: &str, df: &DataFrame, max_rows: usize) -> Value {
    let total_rows = df.height();
    let columns = df.width();
    let column_names: Vec<String> = df
        .get_column_names()
        .iter()
        .map(|s| s.to_string())
        .collect();
    let truncated = total_rows > max_rows;

    let display_df = if truncated {
        df.head(Some(max_rows))
    } else {
        df.clone()
    };

    let data = df_to_json(&display_df).unwrap_or(Value::Array(vec![]));

    serde_json::json!({
        "tool": tool_name,
        "rows": total_rows,
        "columns": columns,
        "column_names": column_names,
        "truncated": truncated,
        "data": data,
    })
}

/// Build a standardized data tool response with additional metadata fields.
fn data_tool_response_with_meta(
    tool_name: &str,
    df: &DataFrame,
    max_rows: usize,
    extra: Vec<(&str, Value)>,
) -> Value {
    let mut base = data_tool_response(tool_name, df, max_rows);
    if let Value::Object(ref mut map) = base {
        for (key, val) in extra {
            map.insert(key.to_string(), val);
        }
    }
    base
}

/// Check if a column type is numeric
pub fn is_numeric(dtype: &DataType) -> bool {
    matches!(
        dtype,
        DataType::Int8
            | DataType::Int16
            | DataType::Int32
            | DataType::Int64
            | DataType::UInt8
            | DataType::UInt16
            | DataType::UInt32
            | DataType::UInt64
            | DataType::Float32
            | DataType::Float64
    )
}

/// Check if a column type is temporal
fn is_temporal(dtype: &DataType) -> bool {
    matches!(
        dtype,
        DataType::Date | DataType::Datetime(_, _) | DataType::Time | DataType::Duration(_)
    )
}

/// Column category type
#[derive(Debug, PartialEq)]
enum ColumnCategory {
    Dimension,
    Metric,
    Temporal,
    Unknown,
}

fn classify_column(dtype: &DataType, distinct_count: usize, row_count: usize) -> ColumnCategory {
    if is_temporal(dtype) {
        return ColumnCategory::Temporal;
    }

    let distinct_ratio = if row_count > 0 {
        distinct_count as f64 / row_count as f64
    } else {
        0.0
    };

    // Low cardinality (< 10% or < 50 distinct values) → dimension
    // String type → dimension
    if is_numeric(dtype) {
        if distinct_ratio < 0.1 || distinct_count < 50 {
            ColumnCategory::Dimension
        } else {
            ColumnCategory::Metric
        }
    } else {
        if matches!(
            dtype,
            DataType::String | DataType::Categorical(_, _) | DataType::Enum(_, _)
        ) || distinct_ratio < 0.3
        {
            ColumnCategory::Dimension
        } else {
            ColumnCategory::Unknown
        }
    }
}

// ── Data reader tool ─────────────────────────────────────────────────

pub struct DataReadTool;

impl Tool for DataReadTool {
    fn name(&self) -> &str {
        "read_data"
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Read]
    }

    fn description(&self) -> &str {
        "Read data files (CSV, JSON, Parquet), returning basic info and a preview of the first rows."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the data file"
                },
                "format": {
                    "type": "string",
                    "description": "File format: 'csv', 'json', or 'parquet' (optional, auto-detected)"
                },
                "preview_rows": {
                    "type": "integer",
                    "description": "Number of preview rows (default 10)"
                }
            },
            "required": ["file_path"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let file_path = parameters
                .get("file_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("file_path".to_string()))?;

            let format = parameters.get("format").and_then(|v| v.as_str());

            let preview_rows = parameters
                .get("preview_rows")
                .and_then(|v| v.as_u64())
                .unwrap_or(10) as usize;

            let security = SecurityConfig::global();
            let path = security.validate_file(file_path)?;

            let detected_format = detect_format(&path, format);
            let df = load_dataframe(&path, format)?;

            let effective_preview_rows = preview_rows.min(security.limits.max_preview_rows);

            // Basic info
            let shape = df.shape();
            let columns: Vec<String> = df
                .get_column_names()
                .iter()
                .map(|s| s.to_string())
                .collect();

            let preview = df.head(Some(effective_preview_rows));
            let preview_json = df_to_json(&preview)?;

            let result = serde_json::json!({
                "file": file_path,
                "format": detected_format,
                "rows": shape.0,
                "columns": shape.1,
                "column_info": columns.iter().map(|col| {
                    if let Ok(c) = df.column(col.as_str()) {
                        serde_json::json!({"name": col, "dtype": c.dtype().to_string()})
                    } else {
                        serde_json::json!({"name": col, "dtype": "unknown"})
                    }
                }).collect::<Vec<_>>(),
                "preview_rows": effective_preview_rows,
                "preview": preview_json,
            });

            Ok(ToolResult::success_json(result))
        })
    }
}

// ── Data filter tool ─────────────────────────────────────────────────

pub struct DataFilterTool;

impl Tool for DataFilterTool {
    fn name(&self) -> &str {
        "filter_data"
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Read]
    }

    fn description(&self) -> &str {
        "Filter a data file, supporting conditional expressions (comparisons, AND/OR combinations, contains matching, etc.). Returns a preview of the filtered data."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the data file"
                },
                "filter": {
                    "type": "string",
                    "description": "Filter condition. Supports: 'col > 100', 'col == \"value\"', 'col contains \"text\"', 'A > 10 AND B < 5', 'col starts_with \"prefix\"'"
                },
                "limit": {
                    "type": "integer",
                    "description": "Result row count limit (optional)"
                }
            },
            "required": ["file_path", "filter"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let file_path = parameters
                .get("file_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("file_path".to_string()))?;

            let filter_expr = parameters
                .get("filter")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("filter".to_string()))?;

            let limit = parameters
                .get("limit")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize);

            let security = SecurityConfig::global();
            let path = security.validate_file(file_path)?;
            let format = parameters.get("format").and_then(|v| v.as_str());

            let lf = load_lazyframe(&path, format)?;

            let expr = parse_filter_expression(filter_expr)?;
            let filtered_lf = lf.filter(expr);
            let df = filtered_lf
                .collect()
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("Filter execution failed: {}", e),
                })?;

            let max_rows = security.limits.max_preview_rows;
            let effective_limit = limit.map(|n| n.min(max_rows)).unwrap_or(max_rows);
            let result_df = df.head(Some(effective_limit));

            let result = data_tool_response_with_meta(
                "filter_data",
                &result_df,
                effective_limit,
                vec![
                    ("filter", Value::String(filter_expr.to_string())),
                    ("matched_rows", serde_json::json!(df.shape().0)),
                ],
            );

            Ok(ToolResult::success_json(result))
        })
    }
}

// ── Data aggregation tool ────────────────────────────────────────────

pub struct DataAggregateTool;

impl Tool for DataAggregateTool {
    fn name(&self) -> &str {
        "aggregate_data"
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Read]
    }

    fn description(&self) -> &str {
        "Group aggregation operations on data: group stats, sum, mean, count, distinct count, variance, stddev, median, p25/p75/p90/p95/arbitrary percentile, etc. Supported operations: sum, mean/avg, min, max, count, count_distinct/n_unique, variance/var, stddev/std, median, p25/p75/p90/p95, percentile:N/pct:N, first, last. Example: aggregate_data(file_path='sales.csv', group_by='region', aggregations='sales:sum,profit:mean,users:count_distinct,revenue:p95')"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the data file"
                },
                "group_by": {
                    "type": "string",
                    "description": "Group-by column name (optional, comma-separated for multiple)"
                },
                "aggregations": {
                    "type": "string",
                    "description": "Aggregation operations, format: 'column:op', comma-separated for multiple. Ops: sum, mean/avg, min, max, count, count_distinct, variance, stddev, median, p90, p95, percentile:N, etc."
                }
            },
            "required": ["file_path", "aggregations"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let file_path = parameters
                .get("file_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("file_path".to_string()))?;

            let group_by = parameters.get("group_by").and_then(|v| v.as_str());

            let aggregations_str = parameters
                .get("aggregations")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("aggregations".to_string()))?;

            let security = SecurityConfig::global();
            let path = security.validate_file(file_path)?;
            let format = parameters.get("format").and_then(|v| v.as_str());

            let lf = load_lazyframe(&path, format)?;
            let agg_exprs = parse_aggregations(aggregations_str)?;

            let result_lf = if let Some(gb) = group_by {
                let group_cols: Vec<Expr> = gb.split(',').map(|s| col(s.trim())).collect();
                lf.group_by(group_cols).agg(agg_exprs)
            } else {
                lf.select(agg_exprs)
            };

            let df = result_lf
                .collect()
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("Aggregation execution failed: {}", e),
                })?;

            let max_rows = security.limits.max_preview_rows;
            let result = data_tool_response_with_meta(
                "aggregate_data",
                &df,
                max_rows,
                vec![("group_by", serde_json::json!(group_by))],
            );

            Ok(ToolResult::success_json(result))
        })
    }
}

// ── Data stats tool ──────────────────────────────────────────────────

pub struct DataStatsTool;

impl Tool for DataStatsTool {
    fn name(&self) -> &str {
        "data_stats"
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Read]
    }

    fn description(&self) -> &str {
        "Compute detailed per-column statistics (no grouping): count, nulls and null rate, distinct count and distinct rate, mean, stddev, variance, min, max, median, p25/p75/p90/p95 percentiles; for string columns also shows shortest/longest/average length and most frequent value. Difference from aggregate_data: data_stats is per-column overall stats (no grouping), aggregate_data is grouped aggregation. Example: data_stats(file_path='data.csv', columns='age,income,region')"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the data file"
                },
                "columns": {
                    "type": "string",
                    "description": "Column names to compute statistics for, comma-separated (optional, defaults to all numeric columns)"
                }
            },
            "required": ["file_path"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let file_path = parameters
                .get("file_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("file_path".to_string()))?;

            let column_filter: Option<Vec<&str>> = parameters
                .get("columns")
                .and_then(|v| v.as_str())
                .map(|s| s.split(',').map(|c| c.trim()).collect());

            let security = SecurityConfig::global();
            let path = security.validate_file(file_path)?;
            let format = parameters.get("format").and_then(|v| v.as_str());

            let df = load_dataframe(&path, format)?;
            let shape = df.shape();
            let all_columns: Vec<String> = df
                .get_column_names()
                .iter()
                .map(|s| s.to_string())
                .collect();

            let mut columns_json = Vec::new();

            // Filter columns to compute stats for
            let target_cols: Vec<String> = if let Some(ref filter) = column_filter {
                filter.iter().map(|s| s.to_string()).collect()
            } else {
                all_columns.clone()
            };

            for col_name in &target_cols {
                let c = match df.column(col_name.as_str()) {
                    Ok(c) => c,
                    Err(_) => {
                        columns_json.push(serde_json::json!({
                            "name": col_name,
                            "error": "Column not found",
                        }));
                        continue;
                    }
                };

                let dtype = c.dtype();
                let null_count = c.null_count();
                let total = c.len();
                let non_null_count = total - null_count;
                let null_pct = if total > 0 {
                    (null_count as f64 / total as f64) * 100.0
                } else {
                    0.0
                };

                let mut col_json = serde_json::json!({
                    "name": col_name,
                    "dtype": dtype.to_string(),
                    "total": total,
                    "non_null": non_null_count,
                    "null_count": null_count,
                    "null_pct": (null_pct * 100.0).round() / 100.0,
                });

                // Distinct count
                if let Ok(unique_count) = c.n_unique() {
                    let unique_pct = if total > 0 {
                        (unique_count as f64 / total as f64) * 100.0
                    } else {
                        0.0
                    };
                    col_json["unique_count"] = serde_json::json!(unique_count);
                    col_json["unique_pct"] =
                        serde_json::json!((unique_pct * 100.0).round() / 100.0);
                }

                // Numeric statistics
                if is_numeric(dtype) && non_null_count > 0 {
                    let series = c.as_materialized_series();
                    let chunked = match dtype {
                        DataType::Int64 => {
                            let ca: &polars::prelude::Int64Chunked =
                                series.i64().map_err(|e| ToolError::ExecutionFailed {
                                    tool: TOOL_NAME.to_string(),
                                    message: format!("Expected Int64 series: {e}"),
                                })?;
                            let v: Vec<Option<f64>> =
                                ca.iter().map(|opt| opt.map(|x| x as f64)).collect();
                            polars::prelude::Float64Chunked::from_slice_options(
                                PlSmallStr::from_static("tmp"),
                                &v,
                            )
                        }
                        DataType::Float64 => series
                            .f64()
                            .map_err(|e| ToolError::ExecutionFailed {
                                tool: TOOL_NAME.to_string(),
                                message: format!("Expected Float64 series: {e}"),
                            })?
                            .clone(),
                        _ => series
                            .cast(&DataType::Float64)
                            .unwrap_or_default()
                            .f64()
                            .unwrap_or(&polars::prelude::Float64Chunked::full(
                                PlSmallStr::from_static("tmp"),
                                0.0,
                                0,
                            ))
                            .clone(),
                    };

                    let values: Vec<f64> = chunked.iter().flatten().collect();

                    if !values.is_empty() {
                        let mut sorted = values.clone();
                        sorted
                            .sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

                        let n = sorted.len();
                        let min_val = sorted[0];
                        let max_val = sorted[n - 1];
                        let sum: f64 = sorted.iter().sum();
                        let mean = sum / n as f64;

                        let variance: f64 =
                            sorted.iter().map(|x| (x - mean) * (x - mean)).sum::<f64>()
                                / (n - 1) as f64;
                        let stddev = variance.sqrt();

                        let median = if n % 2 == 0 {
                            (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
                        } else {
                            sorted[n / 2]
                        };

                        let p25_idx = (n as f64 * 0.25).round() as usize;
                        let p75_idx = (n as f64 * 0.75).round() as usize;
                        let p90_idx = (n as f64 * 0.90).round() as usize;
                        let p95_idx = (n as f64 * 0.95).round() as usize;

                        let p25 = sorted[p25_idx.min(n - 1)];
                        let p75 = sorted[p75_idx.min(n - 1)];
                        let p90 = sorted[p90_idx.min(n - 1)];
                        let p95 = sorted[p95_idx.min(n - 1)];

                        col_json["numeric_stats"] = serde_json::json!({
                            "min": min_val,
                            "max": max_val,
                            "mean": mean,
                            "median": median,
                            "stddev": stddev,
                            "variance": variance,
                            "p25": p25,
                            "p75": p75,
                            "p90": p90,
                            "p95": p95,
                        });
                    }
                }

                // String column statistics
                if matches!(dtype, DataType::String) && non_null_count > 0 {
                    let series = c.as_materialized_series();
                    let ca = series.str().map_err(|e| ToolError::ExecutionFailed {
                        tool: TOOL_NAME.to_string(),
                        message: format!("Expected String series: {e}"),
                    })?;
                    let lengths: Vec<usize> = ca.iter().flatten().map(|s| s.len()).collect();
                    if !lengths.is_empty() {
                        let avg_len = lengths.iter().sum::<usize>() as f64 / lengths.len() as f64;
                        let min_len = lengths.iter().min().unwrap_or(&0);
                        let max_len = lengths.iter().max().unwrap_or(&0);
                        col_json["string_stats"] = serde_json::json!({
                            "min_len": min_len,
                            "max_len": max_len,
                            "avg_len": (avg_len * 10.0).round() / 10.0,
                        });
                    }

                    // Top 3 frequent values
                    let freq: std::collections::HashMap<&str, usize> =
                        ca.iter()
                            .flatten()
                            .fold(std::collections::HashMap::new(), |mut acc, s| {
                                *acc.entry(s).or_insert(0) += 1;
                                acc
                            });
                    let mut freq_vec: Vec<(&&str, &usize)> = freq.iter().collect();
                    freq_vec.sort_by(|a, b| b.1.cmp(a.1));
                    let top_values: Vec<serde_json::Value> = freq_vec.iter().take(3).map(|(val, count)| {
                        serde_json::json!({
                            "value": val,
                            "count": count,
                            "pct": ((**count as f64 / non_null_count as f64) * 10000.0).round() / 100.0,
                        })
                    }).collect();
                    col_json["top_values"] = serde_json::json!(top_values);
                }

                columns_json.push(col_json);
            }

            let result = serde_json::json!({
                "file": file_path,
                "total_rows": shape.0,
                "total_cols": shape.1,
                "columns": columns_json,
            });

            Ok(ToolResult::success_json(result))
        })
    }
}

// ── Data transform tool ──────────────────────────────────────────────

pub struct DataTransformTool;

impl Tool for DataTransformTool {
    fn name(&self) -> &str {
        "transform_data"
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Read, ToolPermission::Write]
    }

    fn description(&self) -> &str {
        "Transform data: sort, select columns, rename columns, drop columns, etc."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the data file"
                },
                "operation": {
                    "type": "string",
                    "description": "Operation type: 'sort', 'select' (select columns), 'drop' (remove columns), 'rename' (rename columns)"
                },
                "params": {
                    "type": "string",
                    "description": "Operation params. sort: 'col:asc/desc'; select: 'col1,col2'; drop: 'col1,col2'; rename: 'old:new' (one pair) or 'old1:new1,old2:new2' (multiple pairs)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Result row count limit (optional)"
                }
            },
            "required": ["file_path", "operation", "params"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let file_path = parameters
                .get("file_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("file_path".to_string()))?;

            let operation = parameters
                .get("operation")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("operation".to_string()))?;

            let params = parameters
                .get("params")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("params".to_string()))?;

            let limit = parameters
                .get("limit")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize);

            let security = SecurityConfig::global();
            let path = security.validate_file(file_path)?;
            let format = parameters.get("format").and_then(|v| v.as_str());

            let lf = load_lazyframe(&path, format)?;

            let result_lf = match operation {
                "sort" => {
                    let parts: Vec<&str> = params.split(':').collect();
                    let col_name = parts[0].trim();
                    let descending = parts
                        .get(1)
                        .map(|s| s.trim().to_lowercase() == "desc")
                        .unwrap_or(false);

                    lf.sort(
                        [col_name],
                        SortMultipleOptions {
                            descending: vec![descending],
                            nulls_last: vec![true],
                            multithreaded: true,
                            maintain_order: false,
                            limit: None,
                        },
                    )
                }
                "select" => {
                    let cols: Vec<Expr> = params.split(',').map(|s| col(s.trim())).collect();
                    lf.select(cols)
                }
                "drop" => {
                    let drop_cols: Vec<&str> = params.split(',').map(|s| s.trim()).collect();
                    lf.drop(cols(drop_cols))
                }
                "rename" => {
                    let mut renamed = lf;
                    for pair in params.split(',') {
                        let parts: Vec<&str> = pair.trim().split(':').collect();
                        if parts.len() == 2 {
                            renamed = renamed.rename(
                                [parts[0].trim().to_string()],
                                [parts[1].trim().to_string()],
                                false,
                            );
                        }
                    }
                    renamed
                }
                _ => {
                    return Err(ToolError::InvalidParameter {
                        name: "operation".to_string(),
                        message: format!(
                            "Unsupported operation: '{}', please use sort/select/drop/rename",
                            operation
                        ),
                    }
                    .into());
                }
            };

            let df = result_lf
                .collect()
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("Transform execution failed: {}", e),
                })?;

            let max_rows = security.limits.max_preview_rows;
            let effective_limit = limit.map(|n| n.min(max_rows)).unwrap_or(max_rows);
            let result_df = df.head(Some(effective_limit));

            let result = data_tool_response_with_meta(
                "transform_data",
                &result_df,
                effective_limit,
                vec![
                    ("operation", Value::String(operation.to_string())),
                    ("params", Value::String(params.to_string())),
                ],
            );
            Ok(ToolResult::success_json(result))
        })
    }
}

// ── Data export tool ─────────────────────────────────────────────────

pub struct DataExportTool;

impl Tool for DataExportTool {
    fn name(&self) -> &str {
        "export_data"
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Write]
    }

    fn description(&self) -> &str {
        "Export processed data to CSV, JSON, or Parquet file."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "input_file": {
                    "type": "string",
                    "description": "Input data file path"
                },
                "output_file": {
                    "type": "string",
                    "description": "Output file path"
                },
                "format": {
                    "type": "string",
                    "description": "Output format: 'csv', 'json', or 'parquet'"
                },
                "filter": {
                    "type": "string",
                    "description": "Optional filter condition"
                },
                "columns": {
                    "type": "string",
                    "description": "Optional column selection"
                }
            },
            "required": ["input_file", "output_file", "format"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let input_file = parameters
                .get("input_file")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("input_file".to_string()))?;

            let output_file = parameters
                .get("output_file")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("output_file".to_string()))?;

            let format = parameters
                .get("format")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("format".to_string()))?;

            let filter = parameters.get("filter").and_then(|v| v.as_str());
            let columns = parameters.get("columns").and_then(|v| v.as_str());

            let security = SecurityConfig::global();
            let path = security.validate_file(input_file)?;

            let mut lf = load_lazyframe(&path, None)?;

            if let Some(filter_expr) = filter {
                let expr = parse_filter_expression(filter_expr)?;
                lf = lf.filter(expr);
            }

            if let Some(cols) = columns {
                let col_exprs: Vec<Expr> = cols.split(',').map(|s| col(s.trim())).collect();
                lf = lf.select(col_exprs);
            }

            let mut df = lf.collect().map_err(|e| ToolError::ExecutionFailed {
                tool: TOOL_NAME.to_string(),
                message: format!("Data processing failed: {}", e),
            })?;

            let max_export_rows = security.limits.max_preview_rows;
            if df.shape().0 > max_export_rows {
                df = df.head(Some(max_export_rows));
            }

            let output_path = security.validate_output_file(output_file)?;
            if let Some(parent) = output_path.parent()
                && !parent.as_os_str().is_empty()
            {
                std::fs::create_dir_all(parent).map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("Failed to create output directory: {}", e),
                })?;
            }

            match format {
                "csv" => {
                    let mut file = std::fs::File::create(&output_path).map_err(|e| {
                        ToolError::ExecutionFailed {
                            tool: TOOL_NAME.to_string(),
                            message: format!("Failed to create output file: {}", e),
                        }
                    })?;
                    CsvWriter::new(&mut file).finish(&mut df).map_err(|e| {
                        ToolError::ExecutionFailed {
                            tool: TOOL_NAME.to_string(),
                            message: format!("Failed to write CSV: {}", e),
                        }
                    })?;
                }
                "json" => {
                    let json_value = df_to_json(&df)?;
                    std::fs::write(&output_path, serde_json::to_string_pretty(&json_value)?)
                        .map_err(|e| ToolError::ExecutionFailed {
                            tool: TOOL_NAME.to_string(),
                            message: format!("Failed to write JSON: {}", e),
                        })?;
                }
                "parquet" => {
                    let file = std::fs::File::create(&output_path).map_err(|e| {
                        ToolError::ExecutionFailed {
                            tool: TOOL_NAME.to_string(),
                            message: format!("Failed to create output file: {}", e),
                        }
                    })?;
                    ParquetWriter::new(file).finish(&mut df).map_err(|e| {
                        ToolError::ExecutionFailed {
                            tool: TOOL_NAME.to_string(),
                            message: format!("Failed to write Parquet: {}", e),
                        }
                    })?;
                }
                _ => {
                    return Err(ToolError::InvalidParameter {
                        name: "format".to_string(),
                        message: format!("Unsupported export format: '{}'", format),
                    }
                    .into());
                }
            }

            Ok(ToolResult::success_json(serde_json::json!({
                "input_file": input_file,
                "output_file": output_file,
                "format": format,
                "exported_rows": df.shape().0,
                "truncated": df.shape().0 >= max_export_rows,
                "max_export_rows": max_export_rows,
            })))
        })
    }
}

// ── Data profiling tool (dimension/metric identification) ────────────

pub struct DataProfileTool;

impl Tool for DataProfileTool {
    fn name(&self) -> &str {
        "profile_data"
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Read]
    }

    fn description(&self) -> &str {
        "[Quick data understanding - preferred tool] Automatically identifies each column as dimension or metric: computes missing rate, distinct rate, [min,max,mean,sum] for numeric columns, length range for string columns, and top 5 sample values. Output also includes column classification summary (dimension/metric/time column counts) and suggests follow-up analysis tools (topn_data, contribution_data, bin_data, etc.). Does not return detailed data, only a profile scan. Example: profile_data(file_path='sales.csv')"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the data file"
                }
            },
            "required": ["file_path"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let file_path = parameters
                .get("file_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("file_path".to_string()))?;

            let security = SecurityConfig::global();
            let path = security.validate_file(file_path)?;
            let format = parameters.get("format").and_then(|v| v.as_str());

            let df = load_dataframe(&path, format)?;
            let shape = df.shape();
            let row_count = shape.0;
            let col_count = shape.1;

            let columns: Vec<String> = df
                .get_column_names()
                .iter()
                .map(|s| s.to_string())
                .collect();

            let mut dim_count = 0;
            let mut metric_count = 0;
            let mut temporal_count = 0;

            let mut columns_json = Vec::new();

            for col_name in &columns {
                let c = match df.column(col_name.as_str()) {
                    Ok(c) => c,
                    Err(_) => continue,
                };

                let dtype = c.dtype();
                let null_count = c.null_count();
                let null_pct = if row_count > 0 {
                    ((null_count as f64 / row_count as f64) * 10000.0).round() / 100.0
                } else {
                    0.0
                };

                let distinct_count = c.n_unique().unwrap_or(0);
                let distinct_pct = if row_count > 0 {
                    ((distinct_count as f64 / row_count as f64) * 10000.0).round() / 100.0
                } else {
                    0.0
                };

                let category = classify_column(dtype, distinct_count, row_count);
                let cat_label = match category {
                    ColumnCategory::Dimension => "dimension",
                    ColumnCategory::Metric => "metric",
                    ColumnCategory::Temporal => "temporal",
                    ColumnCategory::Unknown => "other",
                };

                match category {
                    ColumnCategory::Dimension => dim_count += 1,
                    ColumnCategory::Metric => metric_count += 1,
                    ColumnCategory::Temporal => temporal_count += 1,
                    _ => {}
                }

                let mut col_json = serde_json::json!({
                    "name": col_name,
                    "dtype": dtype.to_string(),
                    "category": cat_label,
                    "null_count": null_count,
                    "null_pct": null_pct,
                    "distinct_count": distinct_count,
                    "distinct_pct": distinct_pct,
                });

                // Numeric columns: range/stats
                if is_numeric(dtype) && (row_count - null_count) > 0 {
                    let series = c.as_materialized_series();
                    if let Ok(f64_series) = series.cast(&DataType::Float64)
                        && let Ok(ca) = f64_series.f64()
                    {
                        let vals: Vec<f64> = ca.iter().flatten().collect();
                        if !vals.is_empty() {
                            let min_v = vals.iter().fold(f64::INFINITY, |a, &b| a.min(b));
                            let max_v = vals.iter().fold(f64::NEG_INFINITY, |a, &b| a.max(b));
                            let sum: f64 = vals.iter().sum();
                            let mean = sum / vals.len() as f64;
                            col_json["numeric_range"] = serde_json::json!({
                                "min": min_v,
                                "max": max_v,
                                "mean": mean,
                                "sum": sum,
                            });
                        }
                    }
                }

                // String columns: length info
                if matches!(dtype, DataType::String) && (row_count - null_count) > 0 {
                    let series = c.as_materialized_series();
                    if let Ok(ca) = series.str() {
                        let lengths: Vec<usize> = ca.iter().flatten().map(|s| s.len()).collect();
                        if !lengths.is_empty() {
                            let min_len = lengths.iter().min().unwrap_or(&0);
                            let max_len = lengths.iter().max().unwrap_or(&0);
                            let avg_len =
                                lengths.iter().sum::<usize>() as f64 / lengths.len() as f64;
                            col_json["string_length"] = serde_json::json!({
                                "min": min_len,
                                "max": max_len,
                                "avg": (avg_len * 10.0).round() / 10.0,
                            });
                        }
                    }
                }

                // Sample values (top 5)
                let sample_count = 5.min(row_count - null_count);
                if sample_count > 0 {
                    let mut sample_values: Vec<String> = Vec::new();
                    let mut seen = std::collections::HashSet::new();
                    for i in 0..row_count.min(1000) {
                        let val_str = c
                            .get(i)
                            .map(|v| format_value(&v))
                            .unwrap_or_else(|_| "-".to_string());
                        if val_str != "-" && seen.insert(val_str.clone()) {
                            sample_values.push(val_str);
                            if sample_values.len() >= 5 {
                                break;
                            }
                        }
                    }
                    if !sample_values.is_empty() {
                        col_json["sample_values"] = serde_json::json!(sample_values);
                    }
                }

                columns_json.push(col_json);
            }

            // Suggestions
            let mut suggestions: Vec<String> = Vec::new();
            if metric_count > 0 && dim_count > 0 {
                suggestions.push(format!(
                    "Use topn_data to analyze dimension rankings on metrics ({} dims x {} metrics)",
                    dim_count, metric_count
                ));
                suggestions.push(
                    "Use contribution_data to analyze contribution ratios by dimension".to_string(),
                );
            }
            if metric_count >= 2 {
                suggestions.push(
                    "Metric columns may have correlations worth further exploration".to_string(),
                );
            }
            if metric_count > 0 {
                suggestions
                    .push("Use bin_data to analyze the distribution of metric columns".to_string());
            }

            let result = serde_json::json!({
                "file": file_path,
                "rows": row_count,
                "cols": col_count,
                "columns": columns_json,
                "summary": {
                    "dimensions": dim_count,
                    "metrics": metric_count,
                    "temporal": temporal_count,
                    "other": col_count - dim_count - metric_count - temporal_count,
                },
                "suggestions": suggestions,
            });

            Ok(ToolResult::success_json(result))
        })
    }
}

// ── TopN analysis tool ───────────────────────────────────────────────

pub struct DataTopNTool;

impl Tool for DataTopNTool {
    fn name(&self) -> &str {
        "topn_data"
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Read]
    }

    fn description(&self) -> &str {
        "Sort by a metric column and take Top N. Without dimension columns, returns global top N; with dimension_columns specified, returns top N within each group. Suitable for questions like 'top 10 products by sales', 'top 3 categories by revenue in each region'. Example: topn_data(file_path='sales.csv', metric_column='revenue', dimension_columns='region', top_n=3)"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the data file"
                },
                "metric_column": {
                    "type": "string",
                    "description": "Metric column name for sorting"
                },
                "dimension_columns": {
                    "type": "string",
                    "description": "Grouping dimension columns (optional, comma-separated). Global sort if not specified"
                },
                "top_n": {
                    "type": "integer",
                    "description": "Return top N rows (default 10)"
                },
                "ascending": {
                    "type": "boolean",
                    "description": "Whether to sort ascending (default false, i.e., descending)"
                }
            },
            "required": ["file_path", "metric_column"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let file_path = parameters
                .get("file_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("file_path".to_string()))?;

            let metric_col = parameters
                .get("metric_column")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("metric_column".to_string()))?;

            let dim_cols_str = parameters.get("dimension_columns").and_then(|v| v.as_str());

            let top_n = parameters
                .get("top_n")
                .and_then(|v| v.as_u64())
                .unwrap_or(10)
                .clamp(1, 100) as usize;

            let ascending = parameters
                .get("ascending")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let security = SecurityConfig::global();
            let path = security.validate_file(file_path)?;
            let format = parameters.get("format").and_then(|v| v.as_str());

            let df = load_dataframe(&path, format)?;

            let result_df = if let Some(dim_str) = dim_cols_str {
                // Grouped TopN: take top_n records within each group
                let dim_cols: Vec<&str> = dim_str.split(',').map(|s| s.trim()).collect();
                let group_cols: Vec<Expr> = dim_cols.iter().map(|&d| col(d)).collect();

                // Collect all column names (for head operation in agg phase)
                let all_col_names: Vec<String> = df
                    .get_column_names()
                    .iter()
                    .map(|s| s.to_string())
                    .collect();

                // After sorting by metric column, group_by + agg uses head(N) to get top N per group
                // This is the correct way to implement within-group TopN with Polars
                let sort_desc = !ascending;
                let agg_exprs: Vec<Expr> = all_col_names
                    .iter()
                    .map(|c| {
                        if dim_cols.contains(&c.as_str()) {
                            col(c).first()
                        } else {
                            col(c).head(Some(top_n))
                        }
                    })
                    .collect();

                let sorted = df.lazy().sort(
                    [metric_col],
                    SortMultipleOptions {
                        descending: vec![sort_desc],
                        nulls_last: vec![true],
                        multithreaded: true,
                        maintain_order: false,
                        limit: None,
                    },
                );

                // For each group, take TopN rows in sorted order
                // group_by + agg(all().sort_by(metric).head(n)) ensures top n within each group
                sorted
                    .group_by(group_cols)
                    .agg(agg_exprs)
                    .limit((top_n * dim_cols.len().max(1)).try_into().map_err(|_| {
                        ToolError::ExecutionFailed {
                            tool: TOOL_NAME.to_string(),
                            message: "top_n value too large".to_string(),
                        }
                    })?)
                    .collect()
                    .map_err(|e| ToolError::ExecutionFailed {
                        tool: TOOL_NAME.to_string(),
                        message: format!("Grouped TopN execution failed: {}", e),
                    })?
            } else {
                // Global TopN
                df.lazy()
                    .sort(
                        [metric_col],
                        SortMultipleOptions {
                            descending: vec![!ascending],
                            nulls_last: vec![true],
                            multithreaded: true,
                            maintain_order: false,
                            limit: Some(top_n.try_into().map_err(|_| {
                                ToolError::ExecutionFailed {
                                    tool: TOOL_NAME.to_string(),
                                    message: "top_n value too large".to_string(),
                                }
                            })?),
                        },
                    )
                    .collect()
                    .map_err(|e| ToolError::ExecutionFailed {
                        tool: TOOL_NAME.to_string(),
                        message: format!("TopN sort failed: {}", e),
                    })?
            };

            let max_rows = security.limits.max_preview_rows;
            let mut extra = vec![
                ("top_n", serde_json::json!(top_n)),
                ("metric_column", Value::String(metric_col.to_string())),
                ("ascending", serde_json::json!(ascending)),
            ];
            if let Some(dim) = dim_cols_str {
                extra.push(("dimension_columns", Value::String(dim.to_string())));
            }

            let result = data_tool_response_with_meta("topn_data", &result_df, max_rows, extra);

            Ok(ToolResult::success_json(result))
        })
    }
}

// ── Contribution ratio analysis tool ─────────────────────────────────

pub struct DataContributionTool;

impl Tool for DataContributionTool {
    fn name(&self) -> &str {
        "contribution_data"
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Read]
    }

    fn description(&self) -> &str {
        "Calculate contribution ratio (percentage) and cumulative ratio (Pareto analysis / 80-20 rule) of each dimension value to the metric column. Outputs dimension value, metric value, ratio (%), cumulative (%). Dimension values beyond top_n are merged into 'Other'. Suitable for questions like 'sales ratio by region', 'which categories contribute 80% of revenue? (Pareto analysis)'. Example: contribution_data(file_path='sales.csv', dimension_column='category', metric_column='revenue', top_n=15)"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the data file"
                },
                "dimension_column": {
                    "type": "string",
                    "description": "Dimension column name (column used for grouping)"
                },
                "metric_column": {
                    "type": "string",
                    "description": "Metric column name (column used for sum calculation)"
                },
                "top_n": {
                    "type": "integer",
                    "description": "Show top N dimension values (default 20, rest grouped as \"Other\")"
                }
            },
            "required": ["file_path", "dimension_column", "metric_column"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let file_path = parameters
                .get("file_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("file_path".to_string()))?;

            let dim_col = parameters
                .get("dimension_column")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("dimension_column".to_string()))?;

            let metric_col = parameters
                .get("metric_column")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("metric_column".to_string()))?;

            let top_n = parameters
                .get("top_n")
                .and_then(|v| v.as_u64())
                .unwrap_or(20)
                .clamp(1, 200) as usize;

            let security = SecurityConfig::global();
            let path = security.validate_file(file_path)?;
            let format = parameters.get("format").and_then(|v| v.as_str());

            let df = load_dataframe(&path, format)?;

            // Group by dimension, sum metric
            let agg_df = df
                .lazy()
                .group_by([col(dim_col)])
                .agg([col(metric_col).sum().alias(metric_col)])
                .sort(
                    [metric_col],
                    SortMultipleOptions {
                        descending: vec![true],
                        nulls_last: vec![true],
                        multithreaded: true,
                        maintain_order: false,
                        limit: None,
                    },
                )
                .collect()
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("Group aggregation failed: {}", e),
                })?;

            let total: f64 = agg_df
                .column(metric_col)
                .ok()
                .and_then(|c| {
                    let s = c.as_materialized_series();
                    s.sum::<f64>().ok()
                })
                .unwrap_or(0.0);

            if total == 0.0 {
                return Ok(ToolResult::success_json(serde_json::json!({
                    "dimension_column": dim_col,
                    "metric_column": metric_col,
                    "total": 0.0,
                    "error": "Metric total is 0, cannot calculate ratio",
                })));
            }

            let height = agg_df.height();

            let mut items = Vec::new();
            let mut cumulative = 0.0;
            let display_rows = top_n.min(height);
            let mut other_sum = 0.0;
            let mut other_count = 0u64;

            for i in 0..height {
                let dim_val = agg_df
                    .column(dim_col)
                    .and_then(|c| c.get(i).map(|v| format_value(&v)))
                    .unwrap_or_else(|_| "-".to_string());

                let metric_val: f64 = agg_df
                    .column(metric_col)
                    .map(|c| {
                        let s = c.as_materialized_series();
                        s.get(i)
                            .map(|v| match v {
                                polars::prelude::AnyValue::Float64(f) => f,
                                polars::prelude::AnyValue::Float32(f) => f as f64,
                                polars::prelude::AnyValue::Int64(i) => i as f64,
                                polars::prelude::AnyValue::Int32(i) => i as f64,
                                polars::prelude::AnyValue::UInt64(i) => i as f64,
                                polars::prelude::AnyValue::UInt32(i) => i as f64,
                                _ => 0.0,
                            })
                            .unwrap_or(0.0)
                    })
                    .unwrap_or(0.0);

                if i < display_rows {
                    let pct = ((metric_val / total) * 10000.0).round() / 100.0;
                    cumulative += pct;
                    items.push(serde_json::json!({
                        "dim_value": dim_val,
                        "metric_value": metric_val,
                        "pct": pct,
                        "cumulative_pct": (cumulative * 100.0).round() / 100.0,
                    }));
                } else {
                    other_sum += metric_val;
                    other_count += 1;
                }
            }

            let mut result = serde_json::json!({
                "dimension_column": dim_col,
                "metric_column": metric_col,
                "total": total,
                "items": items,
            });

            if other_count > 0 {
                let other_pct = ((other_sum / total) * 10000.0).round() / 100.0;
                cumulative += other_pct;
                result["other"] = serde_json::json!({
                    "count": other_count,
                    "sum": other_sum,
                    "pct": other_pct,
                    "cumulative_pct": (cumulative * 100.0).round() / 100.0,
                });
            }

            Ok(ToolResult::success_json(result))
        })
    }
}

// ── Numeric binning tool ─────────────────────────────────────────────

pub struct DataBinTool;

impl Tool for DataBinTool {
    fn name(&self) -> &str {
        "bin_data"
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Read]
    }

    fn description(&self) -> &str {
        "Bin numeric columns (equal-width / equal-frequency), counting records per bin and summarizing metrics. Suitable for analyzing data distribution and generating histogram data."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the data file"
                },
                "column": {
                    "type": "string",
                    "description": "Numeric column name to bin"
                },
                "num_bins": {
                    "type": "integer",
                    "description": "Number of bins (default 10)"
                },
                "method": {
                    "type": "string",
                    "description": "Binning method: 'equal_width' (default) or 'equal_frequency'"
                }
            },
            "required": ["file_path", "column"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let file_path = parameters
                .get("file_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("file_path".to_string()))?;

            let col_name = parameters
                .get("column")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("column".to_string()))?;

            let num_bins = parameters
                .get("num_bins")
                .and_then(|v| v.as_u64())
                .unwrap_or(10)
                .clamp(2, 50) as usize;

            let method = parameters
                .get("method")
                .and_then(|v| v.as_str())
                .unwrap_or("equal_width");

            let security = SecurityConfig::global();
            let path = security.validate_file(file_path)?;
            let format = parameters.get("format").and_then(|v| v.as_str());

            let df = load_dataframe(&path, format)?;
            let c = df
                .column(col_name)
                .map_err(|_| ToolError::InvalidParameter {
                    name: "column".to_string(),
                    message: format!("Column '{}' not found", col_name),
                })?;

            let series = c.as_materialized_series();
            let values: Vec<f64> = series
                .cast(&DataType::Float64)
                .unwrap_or_default()
                .f64()
                .unwrap_or(&polars::prelude::Float64Chunked::full(
                    PlSmallStr::from_static("tmp"),
                    0.0,
                    0,
                ))
                .iter()
                .flatten()
                .collect();

            if values.is_empty() {
                return Ok(ToolResult::success(
                    "This column has no valid numeric data".to_string(),
                ));
            }

            let mut sorted = values.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

            let min_val = sorted[0];
            let max_val = sorted[sorted.len() - 1];

            let bins = match method {
                "equal_frequency" => {
                    let n = sorted.len();
                    let mut bins = Vec::new();
                    let per_bin = (n as f64 / num_bins as f64).ceil() as usize;
                    for i in 0..num_bins {
                        let start_idx = i * per_bin;
                        if start_idx >= n {
                            break;
                        }
                        let end_idx = ((i + 1) * per_bin).min(n);
                        let bin_start = sorted[start_idx];
                        let bin_end = if end_idx >= n {
                            sorted[n - 1]
                        } else {
                            sorted[end_idx - 1]
                        };
                        let count = end_idx - start_idx;
                        let bin_vals: Vec<f64> = sorted[start_idx..end_idx].to_vec();
                        let bin_sum: f64 = bin_vals.iter().sum();
                        let bin_mean = bin_sum / count as f64;
                        bins.push((bin_start, bin_end, count, bin_sum, bin_mean));
                    }
                    bins
                }
                _ => {
                    // equal_width (default)
                    let mut bins = Vec::new();
                    let width = (max_val - min_val) / num_bins as f64;
                    if width == 0.0 {
                        bins.push((
                            min_val,
                            max_val,
                            values.len(),
                            values.iter().sum(),
                            values.iter().sum::<f64>() / values.len() as f64,
                        ));
                    } else {
                        for i in 0..num_bins {
                            let bin_start = min_val + i as f64 * width;
                            let bin_end = if i == num_bins - 1 {
                                max_val + 0.0001 // include max
                            } else {
                                bin_start + width
                            };
                            let bin_vals: Vec<f64> = values
                                .iter()
                                .filter(|&&v| {
                                    if i == num_bins - 1 {
                                        v >= bin_start && v <= max_val
                                    } else {
                                        v >= bin_start && v < bin_end
                                    }
                                })
                                .copied()
                                .collect();
                            let count = bin_vals.len();
                            let bin_sum: f64 = bin_vals.iter().sum();
                            let bin_mean = if count > 0 {
                                bin_sum / count as f64
                            } else {
                                0.0
                            };
                            bins.push((bin_start, bin_end, count, bin_sum, bin_mean));
                        }
                    }
                    bins
                }
            };

            let total_count = values.len();
            let bins_json: Vec<Value> = bins
                .iter()
                .map(|(start, end, count, sum_val, mean_val)| {
                    let pct = (*count as f64 / total_count as f64) * 100.0;
                    serde_json::json!({
                        "range": [format!("{:.2}", start), format!("{:.2}", end)],
                        "count": count,
                        "pct": format!("{:.1}", pct),
                        "sum": format!("{:.2}", sum_val),
                        "mean": format!("{:.2}", mean_val),
                    })
                })
                .collect();

            let result = serde_json::json!({
                "column": col_name,
                "method": if method == "equal_frequency" { "equal_frequency" } else { "equal_width" },
                "num_bins": bins.len(),
                "range": [min_val, max_val],
                "total_count": total_count,
                "bins": bins_json,
            });
            Ok(ToolResult::success_json(result))
        })
    }
}

// ── Ratio / expression computation tool ──────────────────────────────

pub struct DataRatioTool;

impl Tool for DataRatioTool {
    fn name(&self) -> &str {
        "ratio_data"
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Read]
    }

    fn description(&self) -> &str {
        "Compute arithmetic expressions and ratios between columns. Supports +, -, *, / and parentheses, with optional grouping dimensions for within-group ratios. Suitable for computing profit margin, conversion rate, YoY/MoM, proportions, etc. Example: ratio_data(file_path='sales.csv', expressions='profit_margin:(revenue-cost)/revenue*100, ratio:cost/revenue')"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the data file"
                },
                "expressions": {
                    "type": "string",
                    "description": "Expression definitions, comma-separated. Format: 'alias:expression'. Expressions support +, -, *, / and parentheses, referencing column names and numeric constants. Example: 'margin:(revenue-cost)/revenue*100, ratio:a/b'"
                },
                "dimension_columns": {
                    "type": "string",
                    "description": "Grouping dimension column names (optional, comma-separated). When specified, expressions are computed within each group"
                },
                "limit": {
                    "type": "integer",
                    "description": "Row count limit (default 50)"
                }
            },
            "required": ["file_path", "expressions"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let file_path = parameters
                .get("file_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("file_path".to_string()))?;

            let exprs_str = parameters
                .get("expressions")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("expressions".to_string()))?;

            let dim_cols_str = parameters.get("dimension_columns").and_then(|v| v.as_str());

            let limit = parameters
                .get("limit")
                .and_then(|v| v.as_u64())
                .unwrap_or(50)
                .clamp(1, 500) as usize;

            let security = SecurityConfig::global();
            let path = security.validate_file(file_path)?;
            let format = parameters.get("format").and_then(|v| v.as_str());

            let df = load_dataframe(&path, format)?;

            // Collect valid column names
            let valid_columns: Vec<String> = df
                .get_column_names()
                .iter()
                .map(|s| s.to_string())
                .collect();

            // Parse expressions
            let parsed_exprs = parse_ratio_expressions(exprs_str, &valid_columns)?;

            // Build Polars expressions
            let mut polars_exprs: Vec<Expr> = Vec::new();
            for (alias, _) in &parsed_exprs {
                let polars_expr = build_ratio_expr(exprs_str, &valid_columns, alias)?;
                polars_exprs.push(polars_expr);
            }

            // If grouping columns exist, group first then compute; otherwise select directly
            let result_df = if let Some(dim_str) = dim_cols_str {
                let dim_cols: Vec<&str> = dim_str.split(',').map(|s| s.trim()).collect();

                // Validate all grouping columns exist
                for dc in &dim_cols {
                    if !valid_columns.iter().any(|c| c == dc) {
                        return Err(ToolError::InvalidParameter {
                            name: "dimension_columns".to_string(),
                            message: format!(
                                "Group column '{}' not found. Available columns: {}",
                                dc,
                                valid_columns.join(", ")
                            ),
                        }
                        .into());
                    }
                }

                let group_cols: Vec<Expr> = dim_cols.iter().map(|&d| col(d)).collect();

                df.lazy()
                    .group_by(group_cols)
                    .agg(polars_exprs.clone())
                    .sort(
                        [dim_cols[0]],
                        SortMultipleOptions {
                            descending: vec![false],
                            nulls_last: vec![true],
                            multithreaded: true,
                            maintain_order: false,
                            limit: Some(limit.try_into().map_err(|_| {
                                ToolError::ExecutionFailed {
                                    tool: TOOL_NAME.to_string(),
                                    message: "limit value too large".to_string(),
                                }
                            })?),
                        },
                    )
                    .collect()
                    .map_err(|e| ToolError::ExecutionFailed {
                        tool: TOOL_NAME.to_string(),
                        message: format!("Grouped ratio calculation failed: {}", e),
                    })?
            } else {
                // No grouping: directly select expression columns + first rows of all original columns as context
                let mut all_exprs: Vec<Expr> = valid_columns.iter().map(col).collect();
                all_exprs.extend(polars_exprs);

                df.lazy()
                    .select(all_exprs)
                    .limit(limit.try_into().map_err(|_| ToolError::ExecutionFailed {
                        tool: TOOL_NAME.to_string(),
                        message: "limit value too large".to_string(),
                    })?)
                    .collect()
                    .map_err(|e| ToolError::ExecutionFailed {
                        tool: TOOL_NAME.to_string(),
                        message: format!("Expression evaluation failed: {}", e),
                    })?
            };

            let data_json = df_to_json(&result_df)?;
            let mut result = serde_json::json!({
                "expressions": exprs_str,
                "data": data_json,
            });
            if let Some(dim) = dim_cols_str {
                result["dimension_columns"] = serde_json::json!(dim);
            }
            Ok(ToolResult::success_json(result))
        })
    }
}

/// Parse ratio expression string, returns (alias, expression) list
/// Format: "alias1:expr1, alias2:expr2"
fn parse_ratio_expressions(
    expr_str: &str,
    valid_columns: &[String],
) -> Result<Vec<(String, String)>> {
    let mut result = Vec::new();
    let mut depth = 0;
    let mut current = String::new();

    for ch in expr_str.chars() {
        match ch {
            '(' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                depth -= 1;
                current.push(ch);
            }
            ',' if depth == 0 => {
                let trimmed = current.trim().to_string();
                if !trimmed.is_empty() {
                    let (alias, expr) = parse_single_expression(&trimmed, valid_columns)?;
                    result.push((alias, expr));
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }

    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        let (alias, expr) = parse_single_expression(&trimmed, valid_columns)?;
        result.push((alias, expr));
    }

    if result.is_empty() {
        return Err(ToolError::InvalidParameter {
            name: "expressions".to_string(),
            message: format!(
                "Expression format error: '{}'. Correct format: 'alias:expression', e.g. 'profit_margin:(revenue-cost)/revenue*100'",
                expr_str
            ),
        }
        .into());
    }

    Ok(result)
}

/// Parse a single "alias:expression" pair
fn parse_single_expression(spec: &str, _valid_columns: &[String]) -> Result<(String, String)> {
    let colon_pos = spec.find(':').ok_or_else(|| ToolError::InvalidParameter {
        name: "expressions".to_string(),
        message: format!(
            "Expression '{}' is missing colon separator. Format: 'alias:expression'",
            spec
        ),
    })?;

    let alias = spec[..colon_pos].trim().to_string();
    let expr = spec[colon_pos + 1..].trim().to_string();

    if alias.is_empty() || expr.is_empty() {
        return Err(ToolError::InvalidParameter {
            name: "expressions".to_string(),
            message: format!("Expression '{}' has empty alias or expression", spec),
        }
        .into());
    }

    // Alias cannot be a number
    if alias.parse::<f64>().is_ok() {
        return Err(ToolError::InvalidParameter {
            name: "expressions".to_string(),
            message: format!("Alias '{}' cannot be a numeric value", alias),
        }
        .into());
    }

    Ok((alias, expr))
}

/// Build a Polars Expr from expression and alias (for a single parsed expression)
fn build_ratio_expr(exprs_str: &str, valid_columns: &[String], target_alias: &str) -> Result<Expr> {
    let parsed = parse_ratio_expressions(exprs_str, valid_columns)?;
    for (alias, expr_text) in &parsed {
        if alias == target_alias {
            return build_single_polars_expr(expr_text, valid_columns, alias);
        }
    }
    Err(ToolError::InvalidParameter {
        name: "expressions".to_string(),
        message: format!("Cannot find expression for alias '{}'", target_alias),
    }
    .into())
}

/// Compile a single arithmetic expression into a Polars Expr
fn build_single_polars_expr(
    expr_text: &str,
    valid_columns: &[String],
    alias: &str,
) -> Result<Expr> {
    let tokens = tokenize_expr(expr_text, valid_columns)?;
    let (_, expr) = parse_expr_tokens(&tokens, 0, valid_columns)?;

    // Try casting to ensure Float64 type
    Ok(expr.alias(alias))
}

/// Token type
#[derive(Debug, Clone, PartialEq)]
enum ExprToken {
    ColRef(String),
    Number(f64),
    Plus,
    Minus,
    Star,
    Slash,
    LParen,
    RParen,
}

/// Tokenize an expression string
fn tokenize_expr(expr_text: &str, valid_columns: &[String]) -> Result<Vec<ExprToken>> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = expr_text.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        let ch = chars[i];

        // Skip whitespace
        if ch.is_whitespace() {
            i += 1;
            continue;
        }

        match ch {
            '+' => {
                tokens.push(ExprToken::Plus);
                i += 1;
            }
            '-' => {
                tokens.push(ExprToken::Minus);
                i += 1;
            }
            '*' => {
                tokens.push(ExprToken::Star);
                i += 1;
            }
            '/' => {
                tokens.push(ExprToken::Slash);
                i += 1;
            }
            '(' => {
                tokens.push(ExprToken::LParen);
                i += 1;
            }
            ')' => {
                tokens.push(ExprToken::RParen);
                i += 1;
            }
            _ if ch.is_ascii_digit() || ch == '.' => {
                // Number literal
                let start = i;
                while i < len && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    i += 1;
                }
                let num_str: String = chars[start..i].iter().collect();
                let num: f64 = num_str.parse().map_err(|_| ToolError::InvalidParameter {
                    name: "expressions".to_string(),
                    message: format!("Cannot parse number: '{}'", num_str),
                })?;
                tokens.push(ExprToken::Number(num));
            }
            _ if ch.is_alphabetic() || ch == '_' => {
                // Identifier (column name)
                let start = i;
                while i < len && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let ident: String = chars[start..i].iter().collect();

                // Check if it's a valid column name
                if !valid_columns.iter().any(|c| c == &ident) {
                    return Err(ToolError::InvalidParameter {
                        name: "expressions".to_string(),
                        message: format!(
                            "Column '{}' in expression does not exist. Available columns: {}",
                            ident,
                            valid_columns.join(", ")
                        ),
                    }
                    .into());
                }

                tokens.push(ExprToken::ColRef(ident));
            }
            _ => {
                return Err(ToolError::InvalidParameter {
                    name: "expressions".to_string(),
                    message: format!("Invalid character in expression: '{}'", ch),
                }
                .into());
            }
        }
    }

    Ok(tokens)
}

/// Recursive descent parser for expression token stream
/// Grammar: expr = term (('+' | '-') term)*
///       term = factor (('*' | '/') factor)*
///       factor = NUMBER | ColRef | '(' expr ')'
fn parse_expr_tokens(
    tokens: &[ExprToken],
    pos: usize,
    valid_columns: &[String],
) -> Result<(usize, Expr)> {
    let (pos, mut left) = parse_term(tokens, pos, valid_columns)?;

    let mut p = pos;
    while p < tokens.len() {
        match tokens[p] {
            ExprToken::Plus => {
                let (next_pos, right) = parse_term(tokens, p + 1, valid_columns)?;
                left = left + right;
                p = next_pos;
            }
            ExprToken::Minus => {
                let (next_pos, right) = parse_term(tokens, p + 1, valid_columns)?;
                left = left - right;
                p = next_pos;
            }
            _ => break,
        }
    }

    Ok((p, left))
}

/// Parse term: factor (('*' | '/') factor)*
fn parse_term(tokens: &[ExprToken], pos: usize, valid_columns: &[String]) -> Result<(usize, Expr)> {
    let (pos, mut left) = parse_factor(tokens, pos, valid_columns)?;

    let mut p = pos;
    while p < tokens.len() {
        match tokens[p] {
            ExprToken::Star => {
                let (next_pos, right) = parse_factor(tokens, p + 1, valid_columns)?;
                left = left * right;
                p = next_pos;
            }
            ExprToken::Slash => {
                let (next_pos, right) = parse_factor(tokens, p + 1, valid_columns)?;
                left = left / right;
                p = next_pos;
            }
            _ => break,
        }
    }

    Ok((p, left))
}

/// Parse factor: NUMBER | ColRef | '(' expr ')'
fn parse_factor(
    tokens: &[ExprToken],
    pos: usize,
    _valid_columns: &[String],
) -> Result<(usize, Expr)> {
    if pos >= tokens.len() {
        return Err(ToolError::InvalidParameter {
            name: "expressions".to_string(),
            message: "Incomplete expression: missing operand".to_string(),
        }
        .into());
    }

    match &tokens[pos] {
        ExprToken::Number(n) => Ok((pos + 1, lit(*n))),
        ExprToken::ColRef(name) => Ok((pos + 1, col(name.as_str()))),
        ExprToken::LParen => {
            let (next_pos, inner) = parse_expr_tokens(tokens, pos + 1, _valid_columns)?;
            if next_pos < tokens.len() && tokens[next_pos] == ExprToken::RParen {
                Ok((next_pos + 1, inner))
            } else {
                Err(ToolError::InvalidParameter {
                    name: "expressions".to_string(),
                    message: "Expression is missing closing parenthesis ')'".to_string(),
                }
                .into())
            }
        }
        _ => Err(ToolError::InvalidParameter {
            name: "expressions".to_string(),
            message: "Unexpected token in expression: expected number, column name, or '(' but got operator".to_string(),
        }
        .into()),
    }
}

// ── Helper functions ──────────────────────────────────────────────────

/// Format a Polars value
fn format_value(value: &AnyValue) -> String {
    match value {
        AnyValue::Null => "-".to_string(),
        AnyValue::Boolean(b) => b.to_string(),
        AnyValue::Int8(i) => i.to_string(),
        AnyValue::Int16(i) => i.to_string(),
        AnyValue::Int32(i) => i.to_string(),
        AnyValue::Int64(i) => i.to_string(),
        AnyValue::UInt8(i) => i.to_string(),
        AnyValue::UInt16(i) => i.to_string(),
        AnyValue::UInt32(i) => i.to_string(),
        AnyValue::UInt64(i) => i.to_string(),
        AnyValue::Float32(f) => format_smart_float(*f as f64),
        AnyValue::Float64(f) => format_smart_float(*f),
        AnyValue::String(s) => s.to_string(),
        AnyValue::StringOwned(s) => s.to_string(),
        _ => value.to_string(),
    }
}

/// Smart float formatting: preserve precision (up to 6 decimals), strip trailing zeros.
fn format_smart_float(f: f64) -> String {
    if f.is_nan() {
        return "NaN".to_string();
    }
    if f.is_infinite() {
        return if f > 0.0 {
            "Infinity".to_string()
        } else {
            "-Infinity".to_string()
        };
    }
    let formatted = format!("{:.6}", f);
    if formatted.contains('.') {
        formatted
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    } else {
        formatted
    }
}

// ── Multi-file read tool ─────────────────────────────────────────────

const MAX_MULTI_FILES: usize = 50;

/// Load multiple DataFrames and vertically concatenate them (using Polars concat for efficiency)
fn load_multi_dataframes(paths: &[&Path], format: Option<&str>) -> Result<DataFrame> {
    if paths.is_empty() {
        return Err(ToolError::MissingParameter("file_paths".to_string()).into());
    }
    if paths.len() > MAX_MULTI_FILES {
        return Err(ToolError::InvalidParameter {
            name: "file_paths".to_string(),
            message: format!(
                "Too many files ({}). Maximum is {}.",
                paths.len(),
                MAX_MULTI_FILES
            ),
        }
        .into());
    }

    let first_df = load_dataframe(paths[0], format)?;
    let schema = first_df.schema().clone();

    // Load all DataFrames and validate schema compatibility
    let mut all_dfs = vec![first_df];
    for path in &paths[1..] {
        let df = load_dataframe(path, format)?;
        if *df.schema() != schema {
            return Err(ToolError::InvalidParameter {
                name: "file_paths".to_string(),
                message: format!(
                    "Schema mismatch: {} has different columns than {}",
                    path.display(),
                    paths[0].display()
                ),
            }
            .into());
        }
        all_dfs.push(df);
    }

    // Use single-pass concat via vstack_mut for efficiency (avoids O(n²) copies)
    let mut result = all_dfs.remove(0);
    for df in &all_dfs {
        result = result.vstack(df).map_err(|e| ToolError::ExecutionFailed {
            tool: TOOL_NAME.to_string(),
            message: format!("Failed to concatenate DataFrames: {}", e),
        })?;
    }

    Ok(result)
}

pub struct DataMultiReadTool;

impl Tool for DataMultiReadTool {
    fn name(&self) -> &str {
        "multi_read_data"
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Read]
    }

    fn description(&self) -> &str {
        "Read and concatenate multiple same-schema data files into a single DataFrame. Supports CSV, JSON, and Parquet. Maximum 50 files. Useful for merging monthly reports, log files, or sharded data. Example: multi_read_data(file_paths=['data/jan.csv','data/feb.csv','data/mar.csv'])"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_paths": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "List of absolute file paths to read (max 50). All files must have the same column schema."
                },
                "format": {
                    "type": "string",
                    "description": "File format: 'csv', 'json', or 'parquet' (optional, auto-detected per file)"
                },
                "preview_rows": {
                    "type": "integer",
                    "description": "Number of preview rows from the merged result (default 10)"
                }
            },
            "required": ["file_paths"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let paths_val = parameters
                .get("file_paths")
                .ok_or_else(|| ToolError::MissingParameter("file_paths".to_string()))?;

            let path_strs: Vec<&str> = paths_val
                .as_array()
                .ok_or_else(|| ToolError::InvalidParameter {
                    name: "file_paths".to_string(),
                    message: "Must be an array of strings".to_string(),
                })?
                .iter()
                .map(|v| {
                    v.as_str().ok_or_else(|| {
                        ToolError::InvalidParameter {
                            name: "file_paths".to_string(),
                            message: "All entries must be strings".to_string(),
                        }
                        .into()
                    })
                })
                .collect::<Result<Vec<_>>>()?;

            if path_strs.is_empty() {
                return Err(ToolError::MissingParameter("file_paths".to_string()).into());
            }

            let format = parameters.get("format").and_then(|v| v.as_str());
            let preview_rows = parameters
                .get("preview_rows")
                .and_then(|v| v.as_u64())
                .unwrap_or(10) as usize;

            let security = SecurityConfig::global();

            // Validate all paths
            let paths: Vec<std::path::PathBuf> = path_strs
                .iter()
                .map(|p| security.validate_file(p))
                .collect::<Result<Vec<_>>>()?;
            let path_refs: Vec<&Path> = paths.iter().map(|p| p.as_path()).collect();

            // Load and concatenate
            let df = load_multi_dataframes(&path_refs, format)?;

            let effective_preview = preview_rows.min(security.limits.max_preview_rows);
            let shape = df.shape();
            let columns: Vec<String> = df
                .get_column_names()
                .iter()
                .map(|s| s.to_string())
                .collect();

            let preview = df.head(Some(effective_preview));
            let preview_json = df_to_json(&preview)?;

            // Per-file row counts (reload individually for stats)
            let file_details: Vec<Value> = path_strs
                .iter()
                .map(|p| {
                    let path = Path::new(p);
                    let fmt = detect_format(path, format);
                    match load_dataframe(path, Some(fmt)) {
                        Ok(f) => serde_json::json!({
                            "file": p,
                            "rows": f.height(),
                            "format": fmt,
                        }),
                        Err(e) => serde_json::json!({
                            "file": p,
                            "error": e.to_string(),
                        }),
                    }
                })
                .collect();

            let result = serde_json::json!({
                "files_count": path_strs.len(),
                "total_rows": shape.0,
                "columns": shape.1,
                "column_info": columns.iter().map(|col| {
                    if let Ok(c) = df.column(col.as_str()) {
                        serde_json::json!({"name": col, "dtype": c.dtype().to_string()})
                    } else {
                        serde_json::json!({"name": col, "dtype": "unknown"})
                    }
                }).collect::<Vec<_>>(),
                "file_details": file_details,
                "preview_rows": effective_preview,
                "preview": preview_json,
            });

            Ok(ToolResult::success_json(result))
        })
    }
}

// ── Join tool ────────────────────────────────────────────────────────

pub struct DataJoinTool;

impl Tool for DataJoinTool {
    fn name(&self) -> &str {
        "join_data"
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Read]
    }

    fn description(&self) -> &str {
        "Join two data files on common key columns. Supports inner, left, and outer joins. Useful for combining orders with customers, facts with dimensions, etc. Example: join_data(left_file='orders.csv', right_file='customers.csv', join_keys=['customer_id'], join_type='left')"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "left_file": {
                    "type": "string",
                    "description": "Absolute path to the left (primary) data file"
                },
                "right_file": {
                    "type": "string",
                    "description": "Absolute path to the right (lookup) data file"
                },
                "join_keys": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Column names to join on (must exist in both files). At least 1 key required."
                },
                "join_type": {
                    "type": "string",
                    "description": "Join type: 'inner' (default), 'left', 'outer', or 'cross'"
                },
                "left_suffix": {
                    "type": "string",
                    "description": "Suffix for duplicate column names from left file (default: '_left')"
                },
                "right_suffix": {
                    "type": "string",
                    "description": "Suffix for duplicate column names from right file (default: '_right')"
                },
                "format": {
                    "type": "string",
                    "description": "File format override for both files: 'csv', 'json', or 'parquet'"
                },
                "preview_rows": {
                    "type": "integer",
                    "description": "Number of preview rows (default 10)"
                }
            },
            "required": ["left_file", "right_file", "join_keys"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let left_path = parameters
                .get("left_file")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("left_file".to_string()))?;

            let right_path = parameters
                .get("right_file")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("right_file".to_string()))?;

            let keys_val = parameters
                .get("join_keys")
                .ok_or_else(|| ToolError::MissingParameter("join_keys".to_string()))?;

            let key_strs: Vec<String> = keys_val
                .as_array()
                .ok_or_else(|| ToolError::InvalidParameter {
                    name: "join_keys".to_string(),
                    message: "Must be an array of strings".to_string(),
                })?
                .iter()
                .map(|v| {
                    v.as_str().map(|s| s.to_string()).ok_or_else(|| {
                        ToolError::InvalidParameter {
                            name: "join_keys".to_string(),
                            message: "All entries must be strings".to_string(),
                        }
                        .into()
                    })
                })
                .collect::<Result<Vec<_>>>()?;

            if key_strs.is_empty() {
                return Err(ToolError::InvalidParameter {
                    name: "join_keys".to_string(),
                    message: "At least one join key is required".to_string(),
                }
                .into());
            }

            let join_type_str = parameters
                .get("join_type")
                .and_then(|v| v.as_str())
                .unwrap_or("inner");

            let format = parameters.get("format").and_then(|v| v.as_str());
            let preview_rows = parameters
                .get("preview_rows")
                .and_then(|v| v.as_u64())
                .unwrap_or(10) as usize;

            let _left_suffix = parameters
                .get("left_suffix")
                .and_then(|v| v.as_str())
                .unwrap_or("_left");
            let right_suffix = parameters
                .get("right_suffix")
                .and_then(|v| v.as_str())
                .unwrap_or("_right");

            let security = SecurityConfig::global();
            let l_path = security.validate_file(left_path)?;
            let r_path = security.validate_file(right_path)?;

            let left_df = load_dataframe(&l_path, format)?;
            let right_df = load_dataframe(&r_path, format)?;

            // Validate join keys exist in both DataFrames
            let left_names: std::collections::HashSet<String> = left_df
                .get_column_names()
                .iter()
                .map(|s| s.to_string())
                .collect();
            let right_names: std::collections::HashSet<String> = right_df
                .get_column_names()
                .iter()
                .map(|s| s.to_string())
                .collect();

            for key in &key_strs {
                if !left_names.contains(key.as_str()) {
                    return Err(ToolError::InvalidParameter {
                        name: "join_keys".to_string(),
                        message: format!("Column '{}' not found in left file", key),
                    }
                    .into());
                }
                if !right_names.contains(key.as_str()) {
                    return Err(ToolError::InvalidParameter {
                        name: "join_keys".to_string(),
                        message: format!("Column '{}' not found in right file", key),
                    }
                    .into());
                }
            }

            // Resolve duplicate non-key column names by adding suffixes
            let key_set: std::collections::HashSet<&str> =
                key_strs.iter().map(|s| s.as_str()).collect();
            let mut right_rename_map = std::collections::HashMap::new();
            for col_name in &right_names {
                if !key_set.contains(col_name.as_str()) && left_names.contains(col_name.as_str()) {
                    right_rename_map
                        .insert(col_name.clone(), format!("{}{}", col_name, right_suffix));
                }
            }

            let mut right_df = right_df;
            for (old, new) in &right_rename_map {
                let _ = right_df.rename(old, PlSmallStr::from_str(new));
            }

            // Determine join type
            let how = match join_type_str {
                "inner" => JoinType::Inner,
                "left" => JoinType::Left,
                "outer" => JoinType::Full,
                "cross" => JoinType::Cross,
                _ => {
                    return Err(ToolError::InvalidParameter {
                        name: "join_type".to_string(),
                        message: format!(
                            "Unsupported join type: '{}'. Use: inner, left, outer, cross",
                            join_type_str
                        ),
                    }
                    .into());
                }
            };

            let key_refs: Vec<&str> = key_strs.iter().map(|s| s.as_str()).collect();

            let joined = left_df
                .join(
                    &right_df,
                    key_refs.clone(),
                    key_refs,
                    JoinArgs::new(how),
                    None,
                )
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("Join failed: {}", e),
                })?;

            let effective_preview = preview_rows.min(security.limits.max_preview_rows);
            let shape = joined.shape();
            let columns: Vec<String> = joined
                .get_column_names()
                .iter()
                .map(|s| s.to_string())
                .collect();

            let preview = joined.head(Some(effective_preview));
            let preview_json = df_to_json(&preview)?;

            let result = serde_json::json!({
                "left_file": left_path,
                "right_file": right_path,
                "join_keys": key_strs,
                "join_type": join_type_str,
                "total_rows": shape.0,
                "columns": shape.1,
                "column_info": columns.iter().map(|col| {
                    if let Ok(c) = joined.column(col.as_str()) {
                        serde_json::json!({"name": col, "dtype": c.dtype().to_string()})
                    } else {
                        serde_json::json!({"name": col, "dtype": "unknown"})
                    }
                }).collect::<Vec<_>>(),
                "renamed_columns": right_rename_map,
                "preview_rows": effective_preview,
                "preview": preview_json,
            });

            Ok(ToolResult::success_json(result))
        })
    }
}

/// Convert DataFrame to a JSON array (column-major for performance)
fn df_to_json(df: &DataFrame) -> Result<Value> {
    let height = df.height();
    if height == 0 {
        return Ok(Value::Array(vec![]));
    }

    let columns: Vec<String> = df
        .get_column_names()
        .iter()
        .map(|s| s.to_string())
        .collect();

    // Pre-extract all column data as Vec<Value> to avoid repeated column lookups
    let col_data: Vec<Vec<Value>> = columns
        .iter()
        .map(|col_name| {
            df.column(col_name.as_str())
                .map(|c| {
                    (0..height)
                        .map(|i| {
                            c.get(i)
                                .map(|v| any_value_to_json(&v))
                                .unwrap_or(Value::Null)
                        })
                        .collect()
                })
                .unwrap_or_else(|_| vec![Value::Null; height])
        })
        .collect();

    // Build records from pre-extracted column data
    let mut records = Vec::with_capacity(height);
    for i in 0..height {
        let mut record = serde_json::Map::with_capacity(columns.len());
        for (j, col_name) in columns.iter().enumerate() {
            record.insert(col_name.clone(), col_data[j][i].clone());
        }
        records.push(Value::Object(record));
    }

    Ok(Value::Array(records))
}

/// Convert AnyValue to JSON Value
fn any_value_to_json(value: &AnyValue) -> Value {
    match value {
        AnyValue::Null => Value::Null,
        AnyValue::Boolean(b) => Value::Bool(*b),
        AnyValue::Int8(i) => Value::Number((*i).into()),
        AnyValue::Int16(i) => Value::Number((*i).into()),
        AnyValue::Int32(i) => Value::Number((*i).into()),
        AnyValue::Int64(i) => Value::Number((*i).into()),
        AnyValue::UInt8(i) => Value::Number((*i).into()),
        AnyValue::UInt16(i) => Value::Number((*i).into()),
        AnyValue::UInt32(i) => Value::Number((*i).into()),
        AnyValue::UInt64(i) => Value::Number((*i).into()),
        AnyValue::Float32(f) => serde_json::Number::from_f64(*f as f64)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        AnyValue::Float64(f) => serde_json::Number::from_f64(*f)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        AnyValue::String(s) => Value::String(s.to_string()),
        AnyValue::StringOwned(s) => Value::String(s.to_string()),
        _ => Value::String(value.to_string()),
    }
}

/// Parse filter expression (extended: supports AND/OR, contains, starts_with, ends_with, in)
fn parse_filter_expression(expr_str: &str) -> Result<Expr> {
    // Try splitting by AND / OR first (respecting quoted strings)
    for separator in [" AND ", " and ", " OR ", " or "] {
        if let Some(pos) = find_separator_outside_quotes(expr_str, separator) {
            let left = &expr_str[..pos];
            let right = &expr_str[pos + separator.len()..];
            let left_expr = parse_filter_expression(left)?;
            let right_expr = parse_filter_expression(right)?;
            return if separator.trim().to_lowercase() == "and" {
                Ok(left_expr.and(right_expr))
            } else {
                Ok(left_expr.or(right_expr))
            };
        }
    }

    let s = expr_str.trim();

    // Column name pattern: either quoted "Column Name" or unquoted word_chars
    // Capture group 1 = quoted col, capture group 2 = unquoted col
    let col_pat = r#"(?:"([^"]+)"|(\w[\w\s]*\w|\w+))"#;

    // Helper to extract column name from capture groups
    let extract_col = |cap: &regex::Captures| -> String {
        cap.get(1)
            .or_else(|| cap.get(2))
            .map(|m| m.as_str().trim().to_string())
            .unwrap_or_default()
    };

    // Helper to parse numeric value with proper error handling
    let parse_num = |s: &str, expr: &str| -> Result<f64> {
        s.parse::<f64>().map_err(|_| {
            ToolError::InvalidParameter {
                name: "filter".to_string(),
                message: format!("Invalid number '{}' in filter expression: '{}'", s, expr),
            }
            .into()
        })
    };

    // Numeric comparisons (supports negative numbers: -?[\d.]+)
    // >=
    let re = regex::Regex::new(&format!(r#"^{}\s*>=\s*(-?[\d.]+)$"#, col_pat)).unwrap();
    if let Some(cap) = re.captures(s) {
        let col_name = extract_col(&cap);
        let val = parse_num(cap.get(3).unwrap().as_str(), expr_str)?;
        return Ok(col(col_name.as_str()).gt_eq(lit(val)));
    }
    // <=
    let re = regex::Regex::new(&format!(r#"^{}\s*<=\s*(-?[\d.]+)$"#, col_pat)).unwrap();
    if let Some(cap) = re.captures(s) {
        let col_name = extract_col(&cap);
        let val = parse_num(cap.get(3).unwrap().as_str(), expr_str)?;
        return Ok(col(col_name.as_str()).lt_eq(lit(val)));
    }
    // !=
    let re = regex::Regex::new(&format!(r#"^{}\s*!=\s*(-?[\d.]+)$"#, col_pat)).unwrap();
    if let Some(cap) = re.captures(s) {
        let col_name = extract_col(&cap);
        let val = parse_num(cap.get(3).unwrap().as_str(), expr_str)?;
        return Ok(col(col_name.as_str()).neq(lit(val)));
    }
    // == (numeric)
    let re = regex::Regex::new(&format!(r#"^{}\s*==\s*(-?[\d.]+)$"#, col_pat)).unwrap();
    if let Some(cap) = re.captures(s) {
        let col_name = extract_col(&cap);
        let val = parse_num(cap.get(3).unwrap().as_str(), expr_str)?;
        return Ok(col(col_name.as_str()).eq(lit(val)));
    }
    // >
    let re = regex::Regex::new(&format!(r#"^{}\s*>\s*(-?[\d.]+)$"#, col_pat)).unwrap();
    if let Some(cap) = re.captures(s) {
        let col_name = extract_col(&cap);
        let val = parse_num(cap.get(3).unwrap().as_str(), expr_str)?;
        return Ok(col(col_name.as_str()).gt(lit(val)));
    }
    // <
    let re = regex::Regex::new(&format!(r#"^{}\s*<\s*(-?[\d.]+)$"#, col_pat)).unwrap();
    if let Some(cap) = re.captures(s) {
        let col_name = extract_col(&cap);
        let val = parse_num(cap.get(3).unwrap().as_str(), expr_str)?;
        return Ok(col(col_name.as_str()).lt(lit(val)));
    }

    // String comparison: col == "value" or col != "value"
    let re = regex::Regex::new(&format!(r#"^{}\s*==\s*"([^"]+)"$"#, col_pat)).unwrap();
    if let Some(cap) = re.captures(s) {
        let col_name = extract_col(&cap);
        let val = cap.get(3).unwrap().as_str();
        return Ok(col(col_name.as_str()).eq(lit(val)));
    }
    let re = regex::Regex::new(&format!(r#"^{}\s*!=\s*"([^"]+)"$"#, col_pat)).unwrap();
    if let Some(cap) = re.captures(s) {
        let col_name = extract_col(&cap);
        let val = cap.get(3).unwrap().as_str();
        return Ok(col(col_name.as_str()).neq(lit(val)));
    }

    // String contains/starts_with/ends_with
    let re = regex::Regex::new(&format!(r#"^{}\s+contains\s+"([^"]+)"$"#, col_pat)).unwrap();
    if let Some(cap) = re.captures(s) {
        let col_name = extract_col(&cap);
        let val = cap.get(3).unwrap().as_str();
        return Ok(col(col_name.as_str()).str().contains(lit(val), false));
    }
    let re = regex::Regex::new(&format!(r#"^{}\s+starts_with\s+"([^"]+)"$"#, col_pat)).unwrap();
    if let Some(cap) = re.captures(s) {
        let col_name = extract_col(&cap);
        let val = cap.get(3).unwrap().as_str();
        return Ok(col(col_name.as_str()).str().starts_with(lit(val)));
    }
    let re = regex::Regex::new(&format!(r#"^{}\s+ends_with\s+"([^"]+)"$"#, col_pat)).unwrap();
    if let Some(cap) = re.captures(s) {
        let col_name = extract_col(&cap);
        let val = cap.get(3).unwrap().as_str();
        return Ok(col(col_name.as_str()).str().ends_with(lit(val)));
    }

    // IN expression: col in ("a", "b", "c")
    let re = regex::Regex::new(&format!(r#"(?s)^{}\s+in\s*\((.+)\)$"#, col_pat)).unwrap();
    if let Some(cap) = re.captures(s) {
        let col_name = extract_col(&cap);
        let vals_str = cap.get(3).unwrap().as_str();
        let vals: Vec<String> = vals_str
            .split(',')
            .map(|v| v.trim().trim_matches('"').to_string())
            .collect();
        if !vals.is_empty() {
            let series = Series::new(PlSmallStr::EMPTY, vals);
            return Ok(col(col_name.as_str()).is_in(lit(series), false));
        }
    }

    Err(ToolError::InvalidParameter {
        name: "filter".to_string(),
        message: format!(
            "Cannot parse filter expression: '{}'. Supported formats:\n\
             - Numeric: col > 10, col <= 100, col == 42 (supports negative numbers)\n\
             - Quoted columns: \"Total Revenue\" > 1000\n\
             - String: col == \"val\", col != \"val\"\n\
             - Pattern: col contains \"sub\", col starts_with \"pre\", col ends_with \"suf\"\n\
             - Set: col in (\"a\", \"b\", \"c\")\n\
             - Combined: A > 10 AND B < 5, A > 0 OR B > 0",
            expr_str
        ),
    }
    .into())
}

/// Find separator position outside of quoted strings
fn find_separator_outside_quotes(expr: &str, separator: &str) -> Option<usize> {
    let mut in_quotes = false;
    let bytes = expr.as_bytes();
    let sep_bytes = separator.as_bytes();

    for i in 0..expr.len() {
        if bytes[i] == b'"' {
            in_quotes = !in_quotes;
        }
        if !in_quotes && i + separator.len() <= expr.len() {
            if &bytes[i..i + separator.len()] == sep_bytes {
                return Some(i);
            }
        }
    }
    None
}

/// Parse aggregation expression (extended: supports more operations)
fn parse_aggregations(agg_str: &str) -> Result<Vec<Expr>> {
    let mut exprs = Vec::new();

    for part in agg_str.split(',') {
        let parts: Vec<&str> = part.trim().split(':').collect();
        if parts.len() != 2 {
            return Err(ToolError::InvalidParameter {
                name: "aggregations".to_string(),
                message: format!(
                    "Aggregation expression format error: '{}', expected 'column:operation'",
                    part
                ),
            }
            .into());
        }

        let col_name = parts[0].trim();
        let op = parts[1].trim();

        let expr = match op {
            "sum" => col(col_name).sum().alias(format!("{}_sum", col_name)),
            "mean" | "avg" => col(col_name).mean().alias(format!("{}_mean", col_name)),
            "min" => col(col_name).min().alias(format!("{}_min", col_name)),
            "max" => col(col_name).max().alias(format!("{}_max", col_name)),
            "count" => col(col_name).count().alias(format!("{}_count", col_name)),
            "count_distinct" | "n_unique" => col(col_name)
                .n_unique()
                .alias(format!("{}_distinct", col_name)),
            "variance" | "var" => col(col_name).var(1).alias(format!("{}_var", col_name)),
            "stddev" | "std" => col(col_name).std(1).alias(format!("{}_std", col_name)),
            "median" => col(col_name).median().alias(format!("{}_median", col_name)),
            "p90" => col(col_name)
                .quantile(0.9.into(), QuantileMethod::default())
                .alias(format!("{}_p90", col_name)),
            "p95" => col(col_name)
                .quantile(0.95.into(), QuantileMethod::default())
                .alias(format!("{}_p95", col_name)),
            "p25" => col(col_name)
                .quantile(0.25.into(), QuantileMethod::default())
                .alias(format!("{}_p25", col_name)),
            "p75" => col(col_name)
                .quantile(0.75.into(), QuantileMethod::default())
                .alias(format!("{}_p75", col_name)),
            "first" => col(col_name).first().alias(format!("{}_first", col_name)),
            "last" => col(col_name).last().alias(format!("{}_last", col_name)),
            _ => {
                // Support percentile:N for custom percentile
                if op.starts_with("percentile:") || op.starts_with("pct:") {
                    let pct_str = op
                        .strip_prefix("percentile:")
                        .or_else(|| op.strip_prefix("pct:"))
                        .unwrap_or("50");
                    let pct: f64 = pct_str.parse().map_err(|_| ToolError::InvalidParameter {
                        name: "aggregations".to_string(),
                        message: format!("Invalid percentile format: '{}'", pct_str),
                    })?;
                    if !(0.0..=100.0).contains(&pct) {
                        return Err(ToolError::InvalidParameter {
                            name: "aggregations".to_string(),
                            message: format!("Percentile must be between 0 and 100: {}", pct),
                        }
                        .into());
                    }
                    let q = pct / 100.0;
                    col(col_name)
                        .quantile(q.into(), QuantileMethod::default())
                        .alias(format!("{}_p{:.0}", col_name, pct))
                } else {
                    return Err(ToolError::InvalidParameter {
                        name: "aggregations".to_string(),
                        message: format!(
                            "Unsupported aggregation operation: '{}'. Supported: sum, mean/avg, min, max, count, count_distinct, variance, stddev, median, p25, p75, p90, p95, percentile:N, first, last",
                            op
                        ),
                    }
                    .into());
                }
            }
        };

        exprs.push(expr);
    }

    Ok(exprs)
}

// ── Helper functions for data tools ──────────────────────────────────

/// Compute covariance between two Series
fn compute_covariance(s1: &Series, s2: &Series) -> f64 {
    if s1.len() != s2.len() || s1.is_empty() {
        return f64::NAN;
    }

    let mean1 = s1.mean().unwrap_or(f64::NAN);
    let mean2 = s2.mean().unwrap_or(f64::NAN);

    let mut sum = 0.0;
    let mut count = 0;

    for i in 0..s1.len() {
        if let (Ok(v1), Ok(v2)) = (s1.get(i), s2.get(i)) {
            if let (Some(val1), Some(val2)) =
                (v1.try_extract::<f64>().ok(), v2.try_extract::<f64>().ok())
            {
                sum += (val1 - mean1) * (val2 - mean2);
                count += 1;
            }
        }
    }

    if count > 1 {
        sum / (count as f64 - 1.0) // Sample covariance
    } else {
        f64::NAN
    }
}

/// Compute Pearson correlation between two f64 Series
fn compute_pearson(s1: &Series, s2: &Series) -> f64 {
    let cov = compute_covariance(s1, s2);
    let std1 = s1.std(1).unwrap_or(f64::NAN);
    let std2 = s2.std(1).unwrap_or(f64::NAN);
    if std1 == 0.0 || std2 == 0.0 || std1.is_nan() || std2.is_nan() {
        f64::NAN
    } else {
        cov / (std1 * std2)
    }
}

/// Convert a Series to ranks (average rank for ties) for Spearman correlation.
fn compute_ranks(s: &Series) -> Vec<f64> {
    let n = s.len();
    let mut indexed: Vec<(usize, f64)> = Vec::with_capacity(n);

    for i in 0..n {
        let val = match s.get(i) {
            Ok(v) => v.try_extract::<f64>().unwrap_or(f64::NAN),
            Err(_) => f64::NAN,
        };
        indexed.push((i, val));
    }

    // Sort by value (NaN goes to end)
    indexed.sort_by(|a, b| {
        if a.1.is_nan() && b.1.is_nan() {
            std::cmp::Ordering::Equal
        } else if a.1.is_nan() {
            std::cmp::Ordering::Greater
        } else if b.1.is_nan() {
            std::cmp::Ordering::Less
        } else {
            a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)
        }
    });

    let mut ranks = vec![f64::NAN; n];
    let mut i = 0;
    while i < n {
        if indexed[i].1.is_nan() {
            i += 1;
            continue;
        }
        // Find all ties (same value)
        let mut j = i;
        while j < n && (indexed[j].1 - indexed[i].1).abs() < f64::EPSILON {
            j += 1;
        }
        // Average rank for ties (1-based)
        let avg_rank = (i as f64 + j as f64 + 1.0) / 2.0;
        for k in i..j {
            ranks[indexed[k].0] = avg_rank;
        }
        i = j;
    }
    ranks
}

// ── CorrelateTool ────────────────────────────────────────────────────────────

/// Compute correlation matrix between numeric columns
pub struct CorrelateTool;

impl Tool for CorrelateTool {
    fn name(&self) -> &str {
        "correlate_data"
    }

    fn description(&self) -> &str {
        "Compute correlation matrix between numeric columns. Returns Pearson correlation coefficients between -1 and 1."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the data file"
                },
                "columns": {
                    "type": "string",
                    "description": "Comma-separated list of numeric columns to correlate (default: all numeric columns)"
                },
                "method": {
                    "type": "string",
                    "enum": ["pearson", "spearman"],
                    "description": "Correlation method (default: pearson)"
                }
            },
            "required": ["file_path"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let file_path = parameters
                .get("file_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("file_path".to_string()))?;

            let security = SecurityConfig::global();
            let path = security.validate_file(file_path)?;

            let columns = parameters.get("columns").and_then(|v| v.as_str());
            let method = parameters
                .get("method")
                .and_then(|v| v.as_str())
                .unwrap_or("pearson");

            let df = load_dataframe(&path, None)?;

            // Select numeric columns
            let numeric_cols: Vec<String> = if let Some(cols_str) = columns {
                cols_str.split(',').map(|s| s.trim().to_string()).collect()
            } else {
                df.get_column_names()
                    .into_iter()
                    .filter(|col| {
                        df.column(*col)
                            .map(|s| s.dtype().is_numeric())
                            .unwrap_or(false)
                    })
                    .map(|s| s.to_string())
                    .collect()
            };

            if numeric_cols.len() < 2 {
                return Ok(ToolResult::success_json(serde_json::json!({
                    "tool": "correlate_data",
                    "error": "Need at least 2 numeric columns for correlation analysis",
                })));
            }

            // Precompute ranks if Spearman
            let all_ranks: Option<Vec<Vec<f64>>> = if method == "spearman" {
                let ranks: std::result::Result<Vec<Vec<f64>>, ToolError> = numeric_cols
                    .iter()
                    .map(|c| {
                        let s = df
                            .column(c)
                            .map_err(|e| ToolError::ExecutionFailed {
                                tool: "correlate_data".to_string(),
                                message: format!("column '{}' missing: {}", c, e),
                            })?
                            .as_materialized_series();
                        let s_f64 =
                            s.cast(&DataType::Float64)
                                .map_err(|e| ToolError::ExecutionFailed {
                                    tool: "correlate_data".to_string(),
                                    message: format!("failed to cast column '{}' to f64: {}", c, e),
                                })?;
                        Ok(compute_ranks(&s_f64))
                    })
                    .collect();
                Some(ranks?)
            } else {
                None
            };

            // Compute correlation matrix
            let mut matrix = Vec::new();
            for (ci, col1) in numeric_cols.iter().enumerate() {
                let mut row = Vec::new();
                for (cj, col2) in numeric_cols.iter().enumerate() {
                    let corr = if col1 == col2 {
                        1.0
                    } else if let Some(ref ranks) = all_ranks {
                        // Spearman: Pearson correlation on ranks
                        let r1 = Series::new(PlSmallStr::EMPTY, &ranks[ci]);
                        let r2 = Series::new(PlSmallStr::EMPTY, &ranks[cj]);
                        compute_pearson(&r1, &r2)
                    } else {
                        // Pearson
                        let s1 = df
                            .column(col1)
                            .map_err(|e| ToolError::ExecutionFailed {
                                tool: "correlate_data".to_string(),
                                message: format!("column '{}' missing: {}", col1, e),
                            })?
                            .as_materialized_series();
                        let s2 = df
                            .column(col2)
                            .map_err(|e| ToolError::ExecutionFailed {
                                tool: "correlate_data".to_string(),
                                message: format!("column '{}' missing: {}", col2, e),
                            })?
                            .as_materialized_series();
                        let s1_f64 = s1.cast(&DataType::Float64).map_err(|e| {
                            ToolError::ExecutionFailed {
                                tool: "correlate_data".to_string(),
                                message: format!("failed to cast '{}' to f64: {}", col1, e),
                            }
                        })?;
                        let s2_f64 = s2.cast(&DataType::Float64).map_err(|e| {
                            ToolError::ExecutionFailed {
                                tool: "correlate_data".to_string(),
                                message: format!("failed to cast '{}' to f64: {}", col2, e),
                            }
                        })?;
                        compute_pearson(&s1_f64, &s2_f64)
                    };
                    row.push(corr);
                }
                matrix.push(row);
            }

            // Format output as structured JSON
            let matrix_json: Vec<Vec<Value>> = matrix
                .iter()
                .map(|row| {
                    row.iter()
                        .map(|v| {
                            if v.is_nan() {
                                Value::Null
                            } else {
                                serde_json::Number::from_f64((*v * 1000.0).round() / 1000.0)
                                    .map(Value::Number)
                                    .unwrap_or(Value::Null)
                            }
                        })
                        .collect()
                })
                .collect();

            let result = serde_json::json!({
                "tool": "correlate_data",
                "method": method,
                "columns": numeric_cols,
                "matrix": matrix_json,
            });

            Ok(ToolResult::success_json(result))
        })
    }
}

// ── PivotTool ────────────────────────────────────────────────────────────────

/// Create pivot tables from data
pub struct PivotTool;

impl Tool for PivotTool {
    fn name(&self) -> &str {
        "pivot_data"
    }

    fn description(&self) -> &str {
        "Create pivot tables. Reshape data by grouping rows by one or more columns and pivoting another column's values into new columns."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the data file"
                },
                "index": {
                    "type": "string",
                    "description": "Column(s) to use as row index (comma-separated)"
                },
                "columns": {
                    "type": "string",
                    "description": "Column whose unique values will become new columns"
                },
                "values": {
                    "type": "string",
                    "description": "Column(s) to aggregate (comma-separated)"
                },
                "agg_function": {
                    "type": "string",
                    "enum": ["sum", "mean", "count", "min", "max", "first", "last"],
                    "description": "Aggregation function (default: sum)"
                }
            },
            "required": ["file_path", "index", "columns", "values"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let file_path = parameters
                .get("file_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("file_path".to_string()))?;

            let security = SecurityConfig::global();
            let path = security.validate_file(file_path)?;

            let index = parameters
                .get("index")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("index".to_string()))?;

            let columns = parameters
                .get("columns")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("columns".to_string()))?;

            let values = parameters
                .get("values")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("values".to_string()))?;

            let agg_function = parameters
                .get("agg_function")
                .and_then(|v| v.as_str())
                .unwrap_or("sum");

            let df = load_dataframe(&path, None)?;

            // Parse column lists
            let index_cols: Vec<String> = index.split(',').map(|s| s.trim().to_string()).collect();
            let value_cols: Vec<String> = values.split(',').map(|s| s.trim().to_string()).collect();

            // Validate columns exist
            for col in index_cols.iter().chain(value_cols.iter()) {
                let col_str = col.as_str();
                if !df.get_column_names().iter().any(|c| c.as_str() == col_str) {
                    return Err(ToolError::InvalidParameter {
                        name: "columns".to_string(),
                        message: format!("Column '{}' not found in data", col),
                    }
                    .into());
                }
            }

            let columns_str = columns;
            if !df
                .get_column_names()
                .iter()
                .any(|c| c.as_str() == columns_str)
            {
                return Err(ToolError::InvalidParameter {
                    name: "columns".to_string(),
                    message: format!("Column '{}' not found in data", columns_str),
                }
                .into());
            }

            // ── Real pivot: spread `columns` values into new columns ──
            //
            // Example: index=["Region"], columns="Product", values="Revenue", agg="sum"
            //   Input:  Region | Product | Revenue
            //           North  | A       | 100
            //           North  | B       | 200
            //           South  | A       | 300
            //   Output: Region | A   | B
            //           North  | 100 | 200
            //           South  | 300 | null

            // Step 1: group by (index_cols + columns_col) and aggregate values
            let mut group_cols: Vec<Expr> = index_cols.iter().map(|c| col(c)).collect();
            group_cols.push(col(columns_str));

            let mut agg_exprs = Vec::new();
            for val_col in &value_cols {
                let expr = match agg_function {
                    "mean" => col(val_col).mean(),
                    "count" => col(val_col).count(),
                    "min" => col(val_col).min(),
                    "max" => col(val_col).max(),
                    "first" => col(val_col).first(),
                    "last" => col(val_col).last(),
                    _ => col(val_col).sum(),
                };
                agg_exprs.push(expr.alias(val_col));
            }

            let grouped = df
                .clone()
                .lazy()
                .group_by(group_cols)
                .agg(agg_exprs)
                .collect()
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: "pivot_data".to_string(),
                    message: format!("Group-by step failed: {}", e),
                })?;

            // Step 2: Get unique values of the pivot column
            let pivot_col_series = grouped
                .column(columns_str)
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: "pivot_data".to_string(),
                    message: format!("Pivot column '{}' not found: {}", columns_str, e),
                })?
                .as_materialized_series()
                .clone();

            let unique_vals =
                pivot_col_series
                    .unique()
                    .map_err(|e| ToolError::ExecutionFailed {
                        tool: "pivot_data".to_string(),
                        message: format!("Failed to get unique pivot values: {}", e),
                    })?;

            let pivot_values: Vec<String> = (0..unique_vals.len())
                .map(|i| match unique_vals.get(i) {
                    Ok(v) => format!("{}", v),
                    Err(_) => "null".to_string(),
                })
                .collect();

            // Step 3: For each pivot value, filter + rename value columns, then join
            let index_exprs: Vec<Expr> = index_cols.iter().map(|c| col(c)).collect();

            // Start with the unique index rows
            let index_names: Arc<[PlSmallStr]> = index_cols
                .iter()
                .map(|s| PlSmallStr::from(s.as_str()))
                .collect::<Vec<_>>()
                .into();
            let index_selector = Selector::ByName {
                names: index_names,
                strict: true,
            };
            let mut result = grouped
                .clone()
                .lazy()
                .select(index_exprs.clone())
                .unique(Some(index_selector), UniqueKeepStrategy::First)
                .collect()
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: "pivot_data".to_string(),
                    message: format!("Failed to get unique index rows: {}", e),
                })?;

            for pval in &pivot_values {
                for val_col in &value_cols {
                    // Filter grouped data for this pivot value
                    let filter_expr = col(columns_str).eq(lit(pval.as_str()));
                    let new_col_name = if value_cols.len() == 1 {
                        pval.clone()
                    } else {
                        format!("{}_{}", pval, val_col)
                    };

                    let filtered = grouped
                        .clone()
                        .lazy()
                        .filter(filter_expr)
                        .select(
                            index_cols
                                .iter()
                                .map(|c| col(c))
                                .chain(std::iter::once(col(val_col).alias(new_col_name.as_str())))
                                .collect::<Vec<_>>(),
                        )
                        .collect()
                        .map_err(|e| ToolError::ExecutionFailed {
                            tool: "pivot_data".to_string(),
                            message: format!("Failed to filter for pivot value '{}': {}", pval, e),
                        })?;

                    // Left join with result
                    let left_on: Vec<Expr> = index_cols.iter().map(|c| col(c)).collect();
                    let right_on: Vec<Expr> = index_cols.iter().map(|c| col(c)).collect();
                    result = result
                        .lazy()
                        .join(
                            filtered.lazy(),
                            left_on,
                            right_on,
                            JoinArgs::new(JoinType::Left),
                        )
                        .collect()
                        .map_err(|e| ToolError::ExecutionFailed {
                            tool: "pivot_data".to_string(),
                            message: format!(
                                "Failed to join pivot column '{}': {}",
                                new_col_name, e
                            ),
                        })?;
                }
            }

            // Convert to JSON for output
            let json_value = df_to_json(&result)?;

            Ok(ToolResult::success_json(serde_json::json!({
                "pivot_result": {
                    "index": index_cols,
                    "columns": columns_str,
                    "pivot_values": pivot_values,
                    "values": value_cols,
                    "agg_function": agg_function,
                    "shape": [result.height(), result.width()],
                    "data": json_value
                }
            })))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_df_to_json_empty_dataframe() {
        let df = DataFrame::empty();
        let result = df_to_json(&df).unwrap();
        assert!(result.as_array().unwrap().is_empty());
    }

    #[test]
    fn test_df_to_json_with_data() {
        let s1 = Series::new(PlSmallStr::from("name"), &["Alice", "Bob"]);
        let s2 = Series::new(PlSmallStr::from("age"), &[30i32, 25]);
        let df = DataFrame::new(2, vec![s1.into_column(), s2.into_column()]).unwrap();

        let result = df_to_json(&df).unwrap();
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["name"], "Alice");
        assert_eq!(arr[0]["age"], 30);
        assert_eq!(arr[1]["name"], "Bob");
        assert_eq!(arr[1]["age"], 25);
    }

    #[test]
    fn test_data_tool_response_structure() {
        let s1 = Series::new(PlSmallStr::from("x"), &[1i32, 2, 3]);
        let df = DataFrame::new(3, vec![s1.into_column()]).unwrap();

        let result = data_tool_response("test_tool", &df, 100);
        let obj = result.as_object().unwrap();

        // Verify standardized envelope
        assert_eq!(obj["tool"], "test_tool");
        assert_eq!(obj["rows"], 3);
        assert_eq!(obj["columns"], 1);
        assert!(!obj["truncated"].as_bool().unwrap());
        assert!(obj["data"].is_array());
        assert_eq!(obj["data"].as_array().unwrap().len(), 3);
        assert!(
            obj["column_names"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("x"))
        );
    }

    #[test]
    fn test_data_tool_response_truncation() {
        let s1 = Series::new(PlSmallStr::from("v"), &(0..10).collect::<Vec<i32>>());
        let df = DataFrame::new(10, vec![s1.into_column()]).unwrap();

        let result = data_tool_response("test_tool", &df, 3);
        let obj = result.as_object().unwrap();

        assert_eq!(obj["rows"], 10);
        assert!(obj["truncated"].as_bool().unwrap());
        assert_eq!(obj["data"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn test_data_tool_response_with_meta() {
        let s1 = Series::new(PlSmallStr::from("a"), &[1i32]);
        let df = DataFrame::new(1, vec![s1.into_column()]).unwrap();

        let result = data_tool_response_with_meta(
            "filter_data",
            &df,
            100,
            vec![
                ("filter", Value::String("a > 0".to_string())),
                ("matched_rows", serde_json::json!(1)),
            ],
        );
        let obj = result.as_object().unwrap();

        // Standard fields
        assert_eq!(obj["tool"], "filter_data");
        assert_eq!(obj["rows"], 1);
        // Extra fields
        assert_eq!(obj["filter"], "a > 0");
        assert_eq!(obj["matched_rows"], 1);
    }

    #[test]
    fn test_any_value_to_json_types() {
        assert_eq!(any_value_to_json(&AnyValue::Null), Value::Null);
        assert_eq!(
            any_value_to_json(&AnyValue::Boolean(true)),
            Value::Bool(true)
        );
        assert_eq!(
            any_value_to_json(&AnyValue::Int32(42)),
            serde_json::json!(42)
        );
        assert_eq!(
            any_value_to_json(&AnyValue::String("hello")),
            serde_json::json!("hello")
        );
    }

    #[test]
    fn test_detect_format() {
        use std::path::PathBuf;
        assert_eq!(detect_format(&PathBuf::from("data.csv"), None), "csv");
        assert_eq!(detect_format(&PathBuf::from("data.json"), None), "json");
        assert_eq!(
            detect_format(&PathBuf::from("data.parquet"), None),
            "parquet"
        );
        assert_eq!(detect_format(&PathBuf::from("data.tsv"), None), "csv");
        assert_eq!(
            detect_format(&PathBuf::from("data.txt"), Some("json")),
            "json"
        );
    }
}
