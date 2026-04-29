//! 数据处理工具
//!
//! 基于 Polars 提供数据处理能力，支持：
//! - CSV/JSON/Parquet 文件读取
//! - 数据过滤、聚合、排序
//! - 统计计算
//! - 数据转换
//! - 数据概况分析（维度/指标识别）
//! - TopN / 贡献占比 / 数值分箱

use std::path::Path;

use futures::future::BoxFuture;
use polars::prelude::*;
use serde_json::Value;

use super::security::SecurityConfig;
use crate::error::{Result, ToolError};
use crate::tools::{Tool, ToolParameters, ToolResult};

const TOOL_NAME: &str = "data_tools";

// ── 共享数据加载辅助函数 ────────────────────────────────────────────

/// 根据文件扩展名检测格式
fn detect_format<'a>(path: &'a Path, hint: Option<&'a str>) -> &'a str {
    hint.unwrap_or_else(|| match path.extension().and_then(|e| e.to_str()) {
        Some("csv") | Some("txt") | Some("tsv") => "csv",
        Some("json") | Some("jsonl") => "json",
        Some("parquet") | Some("pq") => "parquet",
        _ => "csv",
    })
}

/// 加载 DataFrame（eager），支持 CSV/JSON/Parquet
fn load_dataframe(path: &Path, format: Option<&str>) -> Result<DataFrame> {
    let fmt = detect_format(path, format);

    let file = std::fs::File::open(path).map_err(|e| ToolError::ExecutionFailed {
        tool: TOOL_NAME.to_string(),
        message: format!("打开文件失败: {}", e),
    })?;

    match fmt {
        "csv" => Ok(CsvReader::new(file)
            .finish()
            .map_err(|e| ToolError::ExecutionFailed {
                tool: TOOL_NAME.to_string(),
                message: format!("读取 CSV 失败: {}", e),
            })?),
        "json" => {
            let file2 = std::fs::File::open(path).map_err(|e| ToolError::ExecutionFailed {
                tool: TOOL_NAME.to_string(),
                message: format!("打开 JSON 文件失败: {}", e),
            })?;
            Ok(JsonReader::new(file2)
                .finish()
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("读取 JSON 失败: {}", e),
                })?)
        }
        "parquet" => {
            let file2 = std::fs::File::open(path).map_err(|e| ToolError::ExecutionFailed {
                tool: TOOL_NAME.to_string(),
                message: format!("打开 Parquet 文件失败: {}", e),
            })?;
            Ok(ParquetReader::new(file2)
                .finish()
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("读取 Parquet 失败: {}", e),
                })?)
        }
        _ => Err(ToolError::InvalidParameter {
            name: "format".to_string(),
            message: format!("不支持的文件格式: '{}'", fmt),
        }
        .into()),
    }
}

/// 加载 LazyFrame，支持 CSV/JSON/Parquet
fn load_lazyframe(path: &Path, format: Option<&str>) -> Result<LazyFrame> {
    let fmt = detect_format(path, format);
    let path_str = path.to_string_lossy().to_string();

    match fmt {
        "csv" => {
            Ok(LazyCsvReader::new(path_str)
                .finish()
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("读取 CSV 失败: {}", e),
                })?)
        }
        "json" => {
            // For JSON, we use the eager reader and convert to LazyFrame
            let df = load_dataframe(path, Some("json"))?;
            Ok(df.lazy())
        }
        "parquet" => {
            let file = std::fs::File::open(path).map_err(|e| ToolError::ExecutionFailed {
                tool: TOOL_NAME.to_string(),
                message: format!("打开 Parquet 文件失败: {}", e),
            })?;
            let df = ParquetReader::new(file)
                .finish()
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("读取 Parquet 失败: {}", e),
                })?;
            Ok(df.lazy())
        }
        _ => Err(ToolError::InvalidParameter {
            name: "format".to_string(),
            message: format!("不支持的文件格式: '{}'", fmt),
        }
        .into()),
    }
}

/// 判断列的数值类型
fn is_numeric(dtype: &DataType) -> bool {
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

/// 判断列是否是日期时间类型
fn is_temporal(dtype: &DataType) -> bool {
    matches!(
        dtype,
        DataType::Date | DataType::Datetime(_, _) | DataType::Time | DataType::Duration(_)
    )
}

/// 分类列的类型
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

    // 低基数（< 10% 或 < 50 个不同值）→ 维度
    // 字符串类型 → 维度
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

// ── 数据读取工具 ────────────────────────────────────────────────────

pub struct DataReadTool;

impl Tool for DataReadTool {
    fn name(&self) -> &str {
        "read_data"
    }

    fn description(&self) -> &str {
        "读取数据文件（CSV、JSON、Parquet），返回基本信息和前几行数据预览。"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "数据文件的绝对路径"
                },
                "format": {
                    "type": "string",
                    "description": "文件格式：'csv'、'json' 或 'parquet'（可选，自动检测）"
                },
                "preview_rows": {
                    "type": "integer",
                    "description": "预览行数（默认 10）"
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

            // 基本信息
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

// ── 数据过滤工具 ────────────────────────────────────────────────────

pub struct DataFilterTool;

impl Tool for DataFilterTool {
    fn name(&self) -> &str {
        "filter_data"
    }

    fn description(&self) -> &str {
        "对数据文件进行过滤，支持条件表达式（比较、AND/OR组合、包含匹配等）。返回过滤后的数据预览。"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "数据文件的绝对路径"
                },
                "filter": {
                    "type": "string",
                    "description": "过滤条件。支持: 'col > 100', 'col == \"value\"', 'col contains \"text\"', 'A > 10 AND B < 5', 'col starts_with \"prefix\"'"
                },
                "limit": {
                    "type": "integer",
                    "description": "返回结果行数限制（可选）"
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
                    message: format!("执行过滤失败: {}", e),
                })?;

            let max_rows = security.limits.max_preview_rows;
            let effective_limit = limit.map(|n| n.min(max_rows)).unwrap_or(max_rows);
            let result_df = df.head(Some(effective_limit));
            let data_json = df_to_json(&result_df)?;

            let result = serde_json::json!({
                "filter": filter_expr,
                "matched_rows": df.shape().0,
                "data": data_json,
            });

            Ok(ToolResult::success_json(result))
        })
    }
}

// ── 数据聚合工具 ────────────────────────────────────────────────────

pub struct DataAggregateTool;

impl Tool for DataAggregateTool {
    fn name(&self) -> &str {
        "aggregate_data"
    }

    fn description(&self) -> &str {
        "对数据进行分组聚合操作：分组统计、求和、均值、计数、去重计数、方差、标准差、中位数、p25/p75/p90/p95/任意百分位数等。支持的操作：sum, mean/avg, min, max, count, count_distinct/n_unique, variance/var, stddev/std, median, p25/p75/p90/p95, percentile:N/pct:N, first, last。示例：aggregate_data(file_path='sales.csv', group_by='region', aggregations='sales:sum,profit:mean,users:count_distinct,revenue:p95')"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "数据文件的绝对路径"
                },
                "group_by": {
                    "type": "string",
                    "description": "分组列名（可选，多个用逗号分隔）"
                },
                "aggregations": {
                    "type": "string",
                    "description": "聚合操作，格式: '列名:操作'，多个用逗号分隔。操作: sum, mean/avg, min, max, count, count_distinct, variance, stddev, median, p90, p95, percentile:N 等"
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
                    message: format!("执行聚合失败: {}", e),
                })?;

            let data_json = df_to_json(&df)?;

            let result = serde_json::json!({
                "group_by": group_by,
                "data": data_json,
            });

            Ok(ToolResult::success_json(result))
        })
    }
}

// ── 数据统计工具（修复版：真正计算统计量）─────────────────────────

pub struct DataStatsTool;

impl Tool for DataStatsTool {
    fn name(&self) -> &str {
        "data_stats"
    }

    fn description(&self) -> &str {
        "按列计算详细统计信息（不做分组）：计数、空值及空值率、去重数及去重率、均值、标准差、方差、最小值、最大值、中位数、p25/p75/p90/p95等百分位数；对字符串列还展示最短/最长/平均长度和最高频值。与 aggregate_data 的区别：data_stats 是按列整体统计（不分组），aggregate_data 是分组聚合。示例：data_stats(file_path='data.csv', columns='age,income,region')"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "数据文件的绝对路径"
                },
                "columns": {
                    "type": "string",
                    "description": "要计算统计的列名，多个用逗号分隔（可选，默认所有数值列）"
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

            // 筛选要统计的列
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
                            "error": "列不存在",
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

                // 去重计数
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

                // 数值统计
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

                        let median = if n.is_multiple_of(2) {
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

                // 字符串列统计
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

                    // Top 3 频次值
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

// ── 数据转换工具 ────────────────────────────────────────────────────

pub struct DataTransformTool;

impl Tool for DataTransformTool {
    fn name(&self) -> &str {
        "transform_data"
    }

    fn description(&self) -> &str {
        "对数据进行转换操作：排序、选择列、重命名列、删除列等。"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "数据文件的绝对路径"
                },
                "operation": {
                    "type": "string",
                    "description": "操作类型：'sort'（排序）、'select'（选择列）、'drop'（删除列）、'rename'（重命名列）"
                },
                "params": {
                    "type": "string",
                    "description": "操作参数。sort: '列名:asc/desc'；select: 'col1,col2'；drop: 'col1,col2'；rename: '旧名:新名'（一对）或 'old1:new1,old2:new2'（多对）"
                },
                "limit": {
                    "type": "integer",
                    "description": "返回结果行数限制（可选）"
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
                    lf.drop(drop_cols)
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
                            "不支持的操作: '{}'，请使用 sort/select/drop/rename",
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
                    message: format!("执行转换失败: {}", e),
                })?;

            let max_rows = security.limits.max_preview_rows;
            let effective_limit = limit.map(|n| n.min(max_rows)).unwrap_or(max_rows);
            let result_df = df.head(Some(effective_limit));

            let data_json = df_to_json(&result_df)?;
            Ok(ToolResult::success_json(serde_json::json!({
                "operation": operation,
                "params": params,
                "data": data_json,
            })))
        })
    }
}

// ── 数据导出工具 ────────────────────────────────────────────────────

pub struct DataExportTool;

impl Tool for DataExportTool {
    fn name(&self) -> &str {
        "export_data"
    }

    fn description(&self) -> &str {
        "将处理后的数据导出为 CSV、JSON 或 Parquet 文件。"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "input_file": {
                    "type": "string",
                    "description": "输入数据文件路径"
                },
                "output_file": {
                    "type": "string",
                    "description": "输出文件路径"
                },
                "format": {
                    "type": "string",
                    "description": "输出格式：'csv'、'json' 或 'parquet'"
                },
                "filter": {
                    "type": "string",
                    "description": "可选的过滤条件"
                },
                "columns": {
                    "type": "string",
                    "description": "可选的列选择"
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
                message: format!("处理数据失败: {}", e),
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
                    message: format!("创建输出目录失败: {}", e),
                })?;
            }

            match format {
                "csv" => {
                    let mut file = std::fs::File::create(&output_path).map_err(|e| {
                        ToolError::ExecutionFailed {
                            tool: TOOL_NAME.to_string(),
                            message: format!("创建输出文件失败: {}", e),
                        }
                    })?;
                    CsvWriter::new(&mut file).finish(&mut df).map_err(|e| {
                        ToolError::ExecutionFailed {
                            tool: TOOL_NAME.to_string(),
                            message: format!("写入 CSV 失败: {}", e),
                        }
                    })?;
                }
                "json" => {
                    let json_value = df_to_json(&df)?;
                    std::fs::write(&output_path, serde_json::to_string_pretty(&json_value)?)
                        .map_err(|e| ToolError::ExecutionFailed {
                            tool: TOOL_NAME.to_string(),
                            message: format!("写入 JSON 失败: {}", e),
                        })?;
                }
                "parquet" => {
                    let file = std::fs::File::create(&output_path).map_err(|e| {
                        ToolError::ExecutionFailed {
                            tool: TOOL_NAME.to_string(),
                            message: format!("创建输出文件失败: {}", e),
                        }
                    })?;
                    ParquetWriter::new(file).finish(&mut df).map_err(|e| {
                        ToolError::ExecutionFailed {
                            tool: TOOL_NAME.to_string(),
                            message: format!("写入 Parquet 失败: {}", e),
                        }
                    })?;
                }
                _ => {
                    return Err(ToolError::InvalidParameter {
                        name: "format".to_string(),
                        message: format!("不支持的导出格式: '{}'", format),
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

// ── 新增：数据概况分析工具（维度/指标识别） ──────────────────────

pub struct DataProfileTool;

impl Tool for DataProfileTool {
    fn name(&self) -> &str {
        "profile_data"
    }

    fn description(&self) -> &str {
        "【快速理解数据结构 - 首选工具】自动识别每一列是维度还是指标：计算缺失率、去重率、数值列的[min,max,mean,sum]、字符串列的长度范围、以及前5个样本值。输出还会给出列分类总结（维度/指标/时间列分别有多少），并建议后续可用的分析工具（topn_data、contribution_data、bin_data 等）。不返回明细数据，仅做概况扫描。示例：profile_data(file_path='sales.csv')"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "数据文件的绝对路径"
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

                // 数值列：范围/统计
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

                // 字符串列：长度信息
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

                // 样本值 (top 5)
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

            // 建议
            let mut suggestions: Vec<String> = Vec::new();
            if metric_count > 0 && dim_count > 0 {
                suggestions.push(format!(
                    "使用 topn_data 分析维度对指标的排名（{} 个维度 x {} 个指标）",
                    dim_count, metric_count
                ));
                suggestions.push("使用 contribution_data 分析各维度的贡献占比".to_string());
            }
            if metric_count >= 2 {
                suggestions.push("指标列之间可能存在相关关系，可进一步探索".to_string());
            }
            if metric_count > 0 {
                suggestions.push("使用 bin_data 对指标列进行分布分析".to_string());
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

// ── 新增：TopN 分析工具 ─────────────────────────────────────────────

pub struct DataTopNTool;

impl Tool for DataTopNTool {
    fn name(&self) -> &str {
        "topn_data"
    }

    fn description(&self) -> &str {
        "对指标列排序取 Top N。不指定分组维度时全局排序取前N名；指定分组维度（dimension_columns）后，每个维度组内取前N名。适合分析'销售额最高的10个产品'、'各地区销售额前三的品类'等问题。示例：topn_data(file_path='sales.csv', metric_column='revenue', dimension_columns='region', top_n=3)"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "数据文件的绝对路径"
                },
                "metric_column": {
                    "type": "string",
                    "description": "排序指标列名"
                },
                "dimension_columns": {
                    "type": "string",
                    "description": "分组维度列（可选，逗号分隔）。不指定则全局排序"
                },
                "top_n": {
                    "type": "integer",
                    "description": "返回前N条（默认10）"
                },
                "ascending": {
                    "type": "boolean",
                    "description": "是否升序排列（默认false，即降序）"
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
                // 分组 TopN：每个分组内取 top_n 条记录
                let dim_cols: Vec<&str> = dim_str.split(',').map(|s| s.trim()).collect();
                let group_cols: Vec<Expr> = dim_cols.iter().map(|&d| col(d)).collect();

                // 收集所有列名（用于 agg 阶段的 head 操作）
                let all_col_names: Vec<String> = df
                    .get_column_names()
                    .iter()
                    .map(|s| s.to_string())
                    .collect();

                // 按指标列排序后，group_by + agg 使用 head(N) 获取每组前N条
                // 这是 Polars 实现组内 TopN 的正确方式
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

                // 对每个组，按已排序的顺序取 TopN 行
                // group_by + agg(all().sort_by(metric).head(n)) 确保组内取前n
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
                        message: format!("分组TopN执行失败: {}", e),
                    })?
            } else {
                // 全局 TopN
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
                        message: format!("TopN排序失败: {}", e),
                    })?
            };

            let data_json = df_to_json(&result_df)?;

            let mut result = serde_json::json!({
                "top_n": top_n,
                "metric_column": metric_col,
                "ascending": ascending,
                "data": data_json,
            });
            if let Some(dim) = dim_cols_str {
                result["dimension_columns"] = serde_json::json!(dim);
            }

            Ok(ToolResult::success_json(result))
        })
    }
}

// ── 新增：贡献占比分析工具 ─────────────────────────────────────────

pub struct DataContributionTool;

impl Tool for DataContributionTool {
    fn name(&self) -> &str {
        "contribution_data"
    }

    fn description(&self) -> &str {
        "计算维度列各值对指标列的贡献占比（百分比）和累计占比（帕累托分析/80-20法则）。输出维度值、指标值、占比(%)、累计(%)。超过 top_n 的维度值合并为'其他'。适合回答'各地区销售额占比'、'哪些品类贡献了80%的收入？（帕累托分析）'等问题。示例：contribution_data(file_path='sales.csv', dimension_column='category', metric_column='revenue', top_n=15)"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "数据文件的绝对路径"
                },
                "dimension_column": {
                    "type": "string",
                    "description": "维度列名（用于分组的列）"
                },
                "metric_column": {
                    "type": "string",
                    "description": "指标列名（用于求和计算的列）"
                },
                "top_n": {
                    "type": "integer",
                    "description": "展示前N个维度值（默认20，其余归为\"其他\"）"
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
                    message: format!("分组聚合失败: {}", e),
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
                    "error": "指标总计为 0，无法计算占比",
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

// ── 新增：数值分箱工具 ─────────────────────────────────────────────

pub struct DataBinTool;

impl Tool for DataBinTool {
    fn name(&self) -> &str {
        "bin_data"
    }

    fn description(&self) -> &str {
        "对数值列进行分箱（等宽/等频），统计每箱的记录数和指标汇总。适合分析数据分布、生成直方图数据。"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "数据文件的绝对路径"
                },
                "column": {
                    "type": "string",
                    "description": "要分箱的数值列名"
                },
                "num_bins": {
                    "type": "integer",
                    "description": "分箱数量（默认10）"
                },
                "method": {
                    "type": "string",
                    "description": "分箱方法：'equal_width'（等宽，默认）或 'equal_frequency'（等频）"
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
                    message: format!("列 '{}' 不存在", col_name),
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
                return Ok(ToolResult::success("该列没有有效数值数据".to_string()));
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

// ── 新增：比率/表达式计算工具 ──────────────────────────────────────

pub struct DataRatioTool;

impl Tool for DataRatioTool {
    fn name(&self) -> &str {
        "ratio_data"
    }

    fn description(&self) -> &str {
        "计算列之间的算术表达式和比率。支持 +、-、*、/ 和括号组合，可指定分组维度计算组内比率。适合计算利润率、转化率、同比/环比、占比等指标。示例：ratio_data(file_path='sales.csv', expressions='profit_margin:(revenue-cost)/revenue*100, ratio:cost/revenue')"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "数据文件的绝对路径"
                },
                "expressions": {
                    "type": "string",
                    "description": "表达式定义，逗号分隔。格式：'别名:表达式'。表达式支持 +、-、*、/ 和括号，可引用列名和数字常量。示例：'margin:(revenue-cost)/revenue*100, ratio:a/b'"
                },
                "dimension_columns": {
                    "type": "string",
                    "description": "分组维度列名（可选，逗号分隔）。指定后会在每个分组内计算表达式"
                },
                "limit": {
                    "type": "integer",
                    "description": "返回行数限制（默认50）"
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

            // 收集有效的列名
            let valid_columns: Vec<String> = df
                .get_column_names()
                .iter()
                .map(|s| s.to_string())
                .collect();

            // 解析表达式
            let parsed_exprs = parse_ratio_expressions(exprs_str, &valid_columns)?;

            // 构建 Polars 表达式
            let mut polars_exprs: Vec<Expr> = Vec::new();
            for (alias, _) in &parsed_exprs {
                let polars_expr = build_ratio_expr(exprs_str, &valid_columns, alias)?;
                polars_exprs.push(polars_expr);
            }

            // 如果有分组列，先分组再计算；否则直接 select
            let result_df = if let Some(dim_str) = dim_cols_str {
                let dim_cols: Vec<&str> = dim_str.split(',').map(|s| s.trim()).collect();

                // 验证所有分组列存在
                for dc in &dim_cols {
                    if !valid_columns.iter().any(|c| c == dc) {
                        return Err(ToolError::InvalidParameter {
                            name: "dimension_columns".to_string(),
                            message: format!(
                                "分组列 '{}' 不存在。可用列: {}",
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
                        message: format!("分组比率计算失败: {}", e),
                    })?
            } else {
                // 无分组：直接 select 表达式列 + 所有原始列的前几行作为上下文
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
                        message: format!("表达式计算失败: {}", e),
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

/// 解析 ratio 表达式字符串，返回 (alias, expression) 列表
/// 格式: "alias1:expr1, alias2:expr2"
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
                "表达式格式错误: '{}'。正确格式: 'alias:expression'，例如 'profit_margin:(revenue-cost)/revenue*100'",
                expr_str
            ),
        }
        .into());
    }

    Ok(result)
}

/// 解析单个 "alias:expression" 对
fn parse_single_expression(spec: &str, _valid_columns: &[String]) -> Result<(String, String)> {
    let colon_pos = spec.find(':').ok_or_else(|| ToolError::InvalidParameter {
        name: "expressions".to_string(),
        message: format!("表达式 '{}' 缺少冒号分隔符。格式: '别名:表达式'", spec),
    })?;

    let alias = spec[..colon_pos].trim().to_string();
    let expr = spec[colon_pos + 1..].trim().to_string();

    if alias.is_empty() || expr.is_empty() {
        return Err(ToolError::InvalidParameter {
            name: "expressions".to_string(),
            message: format!("表达式 '{}' 的别名或表达式为空", spec),
        }
        .into());
    }

    // 别名不能是数字
    if alias.parse::<f64>().is_ok() {
        return Err(ToolError::InvalidParameter {
            name: "expressions".to_string(),
            message: format!("别名 '{}' 不能是纯数字", alias),
        }
        .into());
    }

    Ok((alias, expr))
}

/// 根据表达式和别名构建 Polars Expr（针对单个已解析的表达式）
fn build_ratio_expr(exprs_str: &str, valid_columns: &[String], target_alias: &str) -> Result<Expr> {
    let parsed = parse_ratio_expressions(exprs_str, valid_columns)?;
    for (alias, expr_text) in &parsed {
        if alias == target_alias {
            return build_single_polars_expr(expr_text, valid_columns, alias);
        }
    }
    Err(ToolError::InvalidParameter {
        name: "expressions".to_string(),
        message: format!("找不到别名 '{}' 对应的表达式", target_alias),
    }
    .into())
}

/// 将单个算术表达式编译为 Polars Expr
fn build_single_polars_expr(
    expr_text: &str,
    valid_columns: &[String],
    alias: &str,
) -> Result<Expr> {
    let tokens = tokenize_expr(expr_text, valid_columns)?;
    let (_, expr) = parse_expr_tokens(&tokens, 0, valid_columns)?;

    // 尝试用 cast 确保是 Float64 类型
    Ok(expr.alias(alias))
}

/// Token 类型
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

/// 将表达式字符串拆分为 token
fn tokenize_expr(expr_text: &str, valid_columns: &[String]) -> Result<Vec<ExprToken>> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = expr_text.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        let ch = chars[i];

        // 跳过空白
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
                // 数字字面量
                let start = i;
                while i < len && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    i += 1;
                }
                let num_str: String = chars[start..i].iter().collect();
                let num: f64 = num_str.parse().map_err(|_| ToolError::InvalidParameter {
                    name: "expressions".to_string(),
                    message: format!("无法解析数字: '{}'", num_str),
                })?;
                tokens.push(ExprToken::Number(num));
            }
            _ if ch.is_alphabetic() || ch == '_' => {
                // 标识符（列名）
                let start = i;
                while i < len && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let ident: String = chars[start..i].iter().collect();

                // 检查是否是有效列名
                if !valid_columns.iter().any(|c| c == &ident) {
                    return Err(ToolError::InvalidParameter {
                        name: "expressions".to_string(),
                        message: format!(
                            "表达式中的列 '{}' 不存在。可用列: {}",
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
                    message: format!("表达式中的无效字符: '{}'", ch),
                }
                .into());
            }
        }
    }

    Ok(tokens)
}

/// 递归下降解析表达式 token 流
/// 文法: expr = term (('+' | '-') term)*
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

/// 解析 term: factor (('*' | '/') factor)*
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

/// 解析 factor: NUMBER | ColRef | '(' expr ')'
fn parse_factor(
    tokens: &[ExprToken],
    pos: usize,
    _valid_columns: &[String],
) -> Result<(usize, Expr)> {
    if pos >= tokens.len() {
        return Err(ToolError::InvalidParameter {
            name: "expressions".to_string(),
            message: "表达式不完整：缺少操作数".to_string(),
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
                    message: "表达式缺少右括号 ')'".to_string(),
                }
                .into())
            }
        }
        _ => Err(ToolError::InvalidParameter {
            name: "expressions".to_string(),
            message: "表达式意外 token: 期望数字、列名或 '(' 但得到了操作符".to_string(),
        }
        .into()),
    }
}

// ── 辅助函数 ──────────────────────────────────────────────────────────

/// 格式化 Polars 值
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
        AnyValue::Float32(f) => format!("{:.2}", f),
        AnyValue::Float64(f) => format!("{:.2}", f),
        AnyValue::String(s) => s.to_string(),
        AnyValue::StringOwned(s) => s.to_string(),
        _ => value.to_string(),
    }
}

/// 将 DataFrame 转换为 JSON 数组
fn df_to_json(df: &DataFrame) -> Result<Value> {
    let columns: Vec<String> = df
        .get_column_names()
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut records = Vec::new();

    for i in 0..df.height() {
        let mut record = serde_json::Map::new();
        for col in &columns {
            if let Ok(c) = df.column(col.as_str()) {
                let value = c
                    .get(i)
                    .map(|v| any_value_to_json(&v))
                    .unwrap_or(Value::Null);
                record.insert(col.clone(), value);
            }
        }
        records.push(Value::Object(record));
    }

    Ok(Value::Array(records))
}

/// 将 AnyValue 转换为 JSON Value
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

/// 解析过滤表达式（扩展版：支持 AND/OR、contains、starts_with、ends_with、in）
fn parse_filter_expression(expr_str: &str) -> Result<Expr> {
    // 先尝试拆分 AND / OR
    for separator in [" AND ", " and ", " OR ", " or "] {
        let parts: Vec<&str> = if let Some(pos) = expr_str.find(separator) {
            let left = &expr_str[..pos];
            let right = &expr_str[pos + separator.len()..];
            vec![left, right]
        } else {
            vec![]
        };

        if parts.len() == 2 {
            let left_expr = parse_filter_expression(parts[0])?;
            let right_expr = parse_filter_expression(parts[1])?;
            return if separator.trim().to_lowercase() == "and" {
                Ok(left_expr.and(right_expr))
            } else {
                Ok(left_expr.or(right_expr))
            };
        }
    }

    let s = expr_str.trim();

    // 数值比较
    if let Ok(re) = regex::Regex::new(r"^(\w+)\s*>=\s*([\d.]+)$")
        && let Some(cap) = re.captures(s)
    {
        let col_name = cap.get(1).unwrap().as_str();
        let val: f64 = cap.get(2).unwrap().as_str().parse().unwrap_or(0.0);
        return Ok(col(col_name).gt_eq(lit(val)));
    }
    if let Ok(re) = regex::Regex::new(r"^(\w+)\s*<=\s*([\d.]+)$")
        && let Some(cap) = re.captures(s)
    {
        let col_name = cap.get(1).unwrap().as_str();
        let val: f64 = cap.get(2).unwrap().as_str().parse().unwrap_or(0.0);
        return Ok(col(col_name).lt_eq(lit(val)));
    }
    if let Ok(re) = regex::Regex::new(r"^(\w+)\s*!=\s*([\d.]+)$")
        && let Some(cap) = re.captures(s)
    {
        let col_name = cap.get(1).unwrap().as_str();
        let val: f64 = cap.get(2).unwrap().as_str().parse().unwrap_or(0.0);
        return Ok(col(col_name).neq(lit(val)));
    }
    if let Ok(re) = regex::Regex::new(r"^(\w+)\s*==\s*([\d.]+)$")
        && let Some(cap) = re.captures(s)
    {
        let col_name = cap.get(1).unwrap().as_str();
        let val: f64 = cap.get(2).unwrap().as_str().parse().unwrap_or(0.0);
        return Ok(col(col_name).eq(lit(val)));
    }
    if let Ok(re) = regex::Regex::new(r"^(\w+)\s*>\s*([\d.]+)$")
        && let Some(cap) = re.captures(s)
    {
        let col_name = cap.get(1).unwrap().as_str();
        let val: f64 = cap.get(2).unwrap().as_str().parse().unwrap_or(0.0);
        return Ok(col(col_name).gt(lit(val)));
    }
    if let Ok(re) = regex::Regex::new(r"^(\w+)\s*<\s*([\d.]+)$")
        && let Some(cap) = re.captures(s)
    {
        let col_name = cap.get(1).unwrap().as_str();
        let val: f64 = cap.get(2).unwrap().as_str().parse().unwrap_or(0.0);
        return Ok(col(col_name).lt(lit(val)));
    }

    // 字符串比较
    if let Ok(re) = regex::Regex::new(r#"^(\w+)\s*==\s*"([^"]+)"$"#)
        && let Some(cap) = re.captures(s)
    {
        let col_name = cap.get(1).unwrap().as_str();
        let val = cap.get(2).unwrap().as_str();
        return Ok(col(col_name).eq(lit(val)));
    }
    if let Ok(re) = regex::Regex::new(r#"^(\w+)\s*!=\s*"([^"]+)"$"#)
        && let Some(cap) = re.captures(s)
    {
        let col_name = cap.get(1).unwrap().as_str();
        let val = cap.get(2).unwrap().as_str();
        return Ok(col(col_name).neq(lit(val)));
    }

    // 字符串包含/开头/结尾匹配
    if let Ok(re) = regex::Regex::new(r#"^(\w+)\s+contains\s+"([^"]+)"$"#)
        && let Some(cap) = re.captures(s)
    {
        let col_name = cap.get(1).unwrap().as_str();
        let val = cap.get(2).unwrap().as_str();
        return Ok(col(col_name).str().contains(lit(val), false));
    }
    if let Ok(re) = regex::Regex::new(r#"^(\w+)\s+starts_with\s+"([^"]+)"$"#)
        && let Some(cap) = re.captures(s)
    {
        let col_name = cap.get(1).unwrap().as_str();
        let val = cap.get(2).unwrap().as_str();
        return Ok(col(col_name).str().starts_with(lit(val)));
    }
    if let Ok(re) = regex::Regex::new(r#"^(\w+)\s+ends_with\s+"([^"]+)"$"#)
        && let Some(cap) = re.captures(s)
    {
        let col_name = cap.get(1).unwrap().as_str();
        let val = cap.get(2).unwrap().as_str();
        return Ok(col(col_name).str().ends_with(lit(val)));
    }

    // IN 表达式
    if let Ok(re) = regex::Regex::new(r#"(?s)^(\w+)\s+in\s*\((.+)\)$"#)
        && let Some(cap) = re.captures(s)
    {
        let col_name = cap.get(1).unwrap().as_str();
        let vals_str = cap.get(2).unwrap().as_str();
        let vals: Vec<String> = vals_str
            .split(',')
            .map(|v| v.trim().trim_matches('"').to_string())
            .collect();
        if !vals.is_empty() {
            let series = Series::new(PlSmallStr::EMPTY, vals);
            return Ok(col(col_name).is_in(lit(series)));
        }
    }

    Err(ToolError::InvalidParameter {
        name: "filter".to_string(),
        message: format!(
            "无法解析过滤表达式: '{}'。支持格式: col > 10, col == \"val\", col contains \"sub\", col starts_with \"pre\", col in (\"a\",\"b\"), A > 10 AND B < 5",
            expr_str
        ),
    }
    .into())
}

/// 解析聚合表达式（扩展版：支持更多操作）
fn parse_aggregations(agg_str: &str) -> Result<Vec<Expr>> {
    let mut exprs = Vec::new();

    for part in agg_str.split(',') {
        let parts: Vec<&str> = part.trim().split(':').collect();
        if parts.len() != 2 {
            return Err(ToolError::InvalidParameter {
                name: "aggregations".to_string(),
                message: format!("聚合表达式格式错误: '{}'，应为 '列名:操作'", part),
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
                // 支持 percentile:N 自定义百分位
                if op.starts_with("percentile:") || op.starts_with("pct:") {
                    let pct_str = op
                        .strip_prefix("percentile:")
                        .or_else(|| op.strip_prefix("pct:"))
                        .unwrap_or("50");
                    let pct: f64 = pct_str.parse().map_err(|_| ToolError::InvalidParameter {
                        name: "aggregations".to_string(),
                        message: format!("百分位数值格式错误: '{}'", pct_str),
                    })?;
                    if !(0.0..=100.0).contains(&pct) {
                        return Err(ToolError::InvalidParameter {
                            name: "aggregations".to_string(),
                            message: format!("百分位数必须在 0-100 之间: {}", pct),
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
                            "不支持的聚合操作: '{}'。支持: sum, mean/avg, min, max, count, count_distinct, variance, stddev, median, p25, p75, p90, p95, percentile:N, first, last",
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
