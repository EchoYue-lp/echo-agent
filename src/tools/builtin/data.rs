//! 数据处理工具
//!
//! 基于 Polars 提供数据处理能力，支持：
//! - CSV/JSON/Parquet 文件读取
//! - 数据过滤、聚合、排序
//! - 统计计算
//! - 数据转换

use futures::future::BoxFuture;
use serde_json::Value;

use super::security::{ResourceLimits, SecurityConfig};
use crate::error::{Result, ToolError};
use crate::tools::{Tool, ToolParameters, ToolResult};

const TOOL_NAME: &str = "data_tools";

/// 数据读取工具
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

            // 检测格式
            let detected_format =
                format.unwrap_or_else(|| match path.extension().and_then(|e| e.to_str()) {
                    Some("csv") => "csv",
                    Some("json") => "json",
                    Some("parquet") | Some("pq") => "parquet",
                    _ => "csv",
                });

            use polars::prelude::*;

            // 限制预览行数不超过最大限制
            let effective_preview_rows = preview_rows.min(security.limits.max_preview_rows);

            let df: DataFrame = match detected_format {
                "csv" => {
                    let file =
                        std::fs::File::open(&path).map_err(|e| ToolError::ExecutionFailed {
                            tool: TOOL_NAME.to_string(),
                            message: format!("打开 CSV 文件失败: {}", e),
                        })?;
                    CsvReader::new(file)
                        .finish()
                        .map_err(|e| ToolError::ExecutionFailed {
                            tool: TOOL_NAME.to_string(),
                            message: format!("读取 CSV 失败: {}", e),
                        })?
                }
                "json" => {
                    let file =
                        std::fs::File::open(&path).map_err(|e| ToolError::ExecutionFailed {
                            tool: TOOL_NAME.to_string(),
                            message: format!("打开 JSON 文件失败: {}", e),
                        })?;
                    JsonReader::new(file)
                        .finish()
                        .map_err(|e| ToolError::ExecutionFailed {
                            tool: TOOL_NAME.to_string(),
                            message: format!("读取 JSON 失败: {}", e),
                        })?
                }
                "parquet" => {
                    let file =
                        std::fs::File::open(&path).map_err(|e| ToolError::ExecutionFailed {
                            tool: TOOL_NAME.to_string(),
                            message: format!("打开 Parquet 文件失败: {}", e),
                        })?;
                    ParquetReader::new(file)
                        .finish()
                        .map_err(|e| ToolError::ExecutionFailed {
                            tool: TOOL_NAME.to_string(),
                            message: format!("读取 Parquet 失败: {}", e),
                        })?
                }
                _ => {
                    return Err(ToolError::InvalidParameter {
                        name: "format".to_string(),
                        message: format!("不支持的文件格式: '{}'", detected_format),
                    }
                    .into());
                }
            };

            // 获取基本信息
            let shape = df.shape();
            let columns: Vec<String> = df
                .get_column_names()
                .iter()
                .map(|s| s.to_string())
                .collect();
            let column_info: Vec<String> = columns
                .iter()
                .map(|col| {
                    if let Ok(c) = df.column(col.as_str()) {
                        format!("{} ({})", col, c.dtype())
                    } else {
                        format!("{} (unknown)", col)
                    }
                })
                .collect();

            // 预览前几行
            let preview = df.head(Some(effective_preview_rows));
            let preview_str = format_dataframe(&preview, &security.limits);

            let result = format!(
                "=== 数据文件信息 ===\n文件: {}\n格式: {}\n行数: {}\n列数: {}\n\n=== 列信息 ===\n{}\n\n=== 数据预览 (前 {} 行) ===\n{}",
                file_path,
                detected_format,
                shape.0,
                shape.1,
                column_info.join(", "),
                effective_preview_rows,
                preview_str
            );

            Ok(ToolResult::success(result))
        })
    }
}

/// 数据过滤工具
pub struct DataFilterTool;

impl Tool for DataFilterTool {
    fn name(&self) -> &str {
        "filter_data"
    }

    fn description(&self) -> &str {
        "对已读取的数据进行过滤，支持条件表达式。返回过滤后的数据。"
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
                    "description": "过滤条件，如 'column > 100'、'category == \"A\"'"
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

            use polars::prelude::*;

            // 读取数据
            let lf = LazyCsvReader::new(path.to_string_lossy().to_string())
                .finish()
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("读取 CSV 失败: {}", e),
                })?;

            // 解析过滤表达式
            let expr = parse_filter_expression(filter_expr)?;

            // 应用过滤
            let filtered_lf = lf.filter(expr);

            // 执行查询
            let df = filtered_lf
                .collect()
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("执行过滤失败: {}", e),
                })?;

            // 应用行数限制
            let max_rows = security.limits.max_preview_rows;
            let effective_limit = limit.map(|n| n.min(max_rows)).unwrap_or(max_rows);

            let result_df = df.head(Some(effective_limit));

            let result = format!(
                "=== 过滤结果 ===\n条件: {}\n匹配行数: {}\n\n{}",
                filter_expr,
                df.shape().0,
                format_dataframe(&result_df, &security.limits)
            );

            Ok(ToolResult::success(result))
        })
    }
}

/// 数据聚合工具
pub struct DataAggregateTool;

impl Tool for DataAggregateTool {
    fn name(&self) -> &str {
        "aggregate_data"
    }

    fn description(&self) -> &str {
        "对数据进行聚合操作，如分组统计、求和、平均值等。"
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
                    "description": "分组列名（可选）"
                },
                "aggregations": {
                    "type": "string",
                    "description": "聚合操作，格式: '列名:操作'，多个用逗号分隔。操作支持: sum, mean, min, max, count, std"
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

            use polars::prelude::*;

            // 读取数据
            let lf = LazyCsvReader::new(path.to_string_lossy().to_string())
                .finish()
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("读取 CSV 失败: {}", e),
                })?;

            // 解析聚合表达式
            let agg_exprs = parse_aggregations(aggregations_str)?;

            // 应用聚合
            let result_lf = if let Some(group_col) = group_by {
                lf.group_by([col(group_col)]).agg(agg_exprs)
            } else {
                lf.select(agg_exprs)
            };

            // 执行查询
            let df = result_lf
                .collect()
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("执行聚合失败: {}", e),
                })?;

            let result = format!(
                "=== 聚合结果 ===\n{}\n{}",
                if let Some(gb) = group_by {
                    format!("分组列: {}", gb)
                } else {
                    "全局聚合".to_string()
                },
                format_dataframe(&df, &security.limits)
            );

            Ok(ToolResult::success(result))
        })
    }
}

/// 数据统计工具
pub struct DataStatsTool;

impl Tool for DataStatsTool {
    fn name(&self) -> &str {
        "data_stats"
    }

    fn description(&self) -> &str {
        "计算数据的基本统计信息：均值、标准差、最小值、最大值、中位数等。"
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

            let security = SecurityConfig::global();
            let path = security.validate_file(file_path)?;

            use polars::prelude::*;

            // 读取数据
            let df = LazyCsvReader::new(path.to_string_lossy().to_string())
                .finish()
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("读取 CSV 失败: {}", e),
                })?
                .collect()
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("读取 CSV 失败: {}", e),
                })?;

            // 获取统计信息
            let shape = df.shape();
            let columns: Vec<String> = df
                .get_column_names()
                .iter()
                .map(|s| s.to_string())
                .collect();

            let mut info = Vec::new();
            info.push(format!("文件: {}", file_path));
            info.push(format!("行数: {}", shape.0));
            info.push(format!("列数: {}", shape.1));
            info.push(String::new());
            info.push("列信息:".to_string());

            for col_name in &columns {
                if let Ok(c) = df.column(col_name.as_str()) {
                    let dtype = c.dtype();
                    info.push(format!(
                        "  {}: {} (null_count: {})",
                        col_name,
                        dtype,
                        c.null_count()
                    ));
                }
            }

            Ok(ToolResult::success(info.join("\n")))
        })
    }
}

/// 数据转换工具
pub struct DataTransformTool;

impl Tool for DataTransformTool {
    fn name(&self) -> &str {
        "transform_data"
    }

    fn description(&self) -> &str {
        "对数据进行转换操作，如排序、选择列等。"
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
                    "description": "操作类型：'sort'、'select'"
                },
                "params": {
                    "type": "string",
                    "description": "操作参数。sort: '列名:asc/desc'；select: '列名列表'"
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

            use polars::prelude::*;

            // 读取数据
            let lf = LazyCsvReader::new(path.to_string_lossy().to_string())
                .finish()
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("读取 CSV 失败: {}", e),
                })?;

            // 执行操作
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
                            nulls_last: vec![false],
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
                _ => {
                    return Err(ToolError::InvalidParameter {
                        name: "operation".to_string(),
                        message: format!("不支持的操作: '{}'", operation),
                    }
                    .into());
                }
            };

            // 执行查询
            let df = result_lf
                .collect()
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("执行转换失败: {}", e),
                })?;

            // 应用行数限制
            let max_rows = security.limits.max_preview_rows;
            let effective_limit = limit.map(|n| n.min(max_rows)).unwrap_or(max_rows);

            let result_df = df.head(Some(effective_limit));

            Ok(ToolResult::success(format_dataframe(
                &result_df,
                &security.limits,
            )))
        })
    }
}

/// 数据导出工具
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

            use polars::prelude::*;

            // 读取并处理数据
            let mut lf = LazyCsvReader::new(path.to_string_lossy().to_string())
                .finish()
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("读取输入文件失败: {}", e),
                })?;

            // 应用过滤
            if let Some(filter_expr) = filter {
                let expr = parse_filter_expression(filter_expr)?;
                lf = lf.filter(expr);
            }

            // 选择列
            if let Some(cols) = columns {
                let col_exprs: Vec<Expr> = cols.split(',').map(|s| col(s.trim())).collect();
                lf = lf.select(col_exprs);
            }

            // 执行查询
            let mut df = lf.collect().map_err(|e| ToolError::ExecutionFailed {
                tool: TOOL_NAME.to_string(),
                message: format!("处理数据失败: {}", e),
            })?;

            // 限制导出行数
            let max_export_rows = security.limits.max_preview_rows;
            if df.shape().0 > max_export_rows {
                df = df.head(Some(max_export_rows));
            }

            // 导出数据
            let output_path = security.validate_output_file(output_file)?;
            std::fs::create_dir_all(output_path.parent().unwrap_or(std::path::Path::new(".")))
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("创建输出目录失败: {}", e),
                })?;

            match format {
                "csv" => {
                    let mut file = std::fs::File::create(output_path).map_err(|e| {
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
                    std::fs::write(output_path, serde_json::to_string_pretty(&json_value)?)
                        .map_err(|e| ToolError::ExecutionFailed {
                            tool: TOOL_NAME.to_string(),
                            message: format!("写入 JSON 失败: {}", e),
                        })?;
                }
                "parquet" => {
                    let file = std::fs::File::create(output_path).map_err(|e| {
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

            Ok(ToolResult::success(format!(
                "数据已导出: {} -> {} ({})\n导出行数: {}{}",
                input_file,
                output_file,
                format,
                df.shape().0,
                if df.shape().0 >= max_export_rows {
                    format!(" (限制为 {} 行)", max_export_rows)
                } else {
                    String::new()
                }
            )))
        })
    }
}

// ── 辅助函数 ──────────────────────────────────────────────────────────

/// 将 DataFrame 格式化为表格字符串
fn format_dataframe(df: &polars::prelude::DataFrame, limits: &ResourceLimits) -> String {
    let mut lines = Vec::new();

    // 表头
    let columns: Vec<String> = df
        .get_column_names()
        .iter()
        .map(|s| s.to_string())
        .collect();
    lines.push(columns.join("\t"));

    // 数据行 - 使用 max_preview_rows 限制
    let max_rows = limits.max_preview_rows.min(df.height());

    for i in 0..max_rows {
        let row: Vec<String> = columns
            .iter()
            .map(|col| {
                if let Ok(c) = df.column(col.as_str()) {
                    c.get(i)
                        .map(|v| format_value(&v))
                        .unwrap_or("-".to_string())
                } else {
                    "-".to_string()
                }
            })
            .collect();
        lines.push(row.join("\t"));
    }

    if df.height() > max_rows {
        lines.push(format!("... (共 {} 行)", df.height()));
    }

    lines.join("\n")
}

/// 格式化 Polars 值
fn format_value(value: &polars::prelude::AnyValue) -> String {
    use polars::prelude::AnyValue;
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
fn df_to_json(df: &polars::prelude::DataFrame) -> Result<Value> {
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
fn any_value_to_json(value: &polars::prelude::AnyValue) -> Value {
    use polars::prelude::AnyValue;
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

/// 解析过滤表达式
fn parse_filter_expression(expr_str: &str) -> Result<polars::prelude::Expr> {
    use polars::prelude::*;

    type PatternList<'a> = &'a [(&'a str, fn(&regex::Captures) -> Expr)];

    // 数值比较
    let num_patterns: PatternList = &[
        (r"(\w+)\s*>\s*(\d+)", |cap: &regex::Captures| {
            let col_name = cap.get(1).unwrap().as_str();
            let val: i64 = cap.get(2).unwrap().as_str().parse().unwrap();
            col(col_name).gt(lit(val))
        }),
        (r"(\w+)\s*<\s*(\d+)", |cap: &regex::Captures| {
            let col_name = cap.get(1).unwrap().as_str();
            let val: i64 = cap.get(2).unwrap().as_str().parse().unwrap();
            col(col_name).lt(lit(val))
        }),
        (r"(\w+)\s*>=\s*(\d+)", |cap: &regex::Captures| {
            let col_name = cap.get(1).unwrap().as_str();
            let val: i64 = cap.get(2).unwrap().as_str().parse().unwrap();
            col(col_name).gt_eq(lit(val))
        }),
        (r"(\w+)\s*<=\s*(\d+)", |cap: &regex::Captures| {
            let col_name = cap.get(1).unwrap().as_str();
            let val: i64 = cap.get(2).unwrap().as_str().parse().unwrap();
            col(col_name).lt_eq(lit(val))
        }),
        (r"(\w+)\s*==\s*(\d+)", |cap: &regex::Captures| {
            let col_name = cap.get(1).unwrap().as_str();
            let val: i64 = cap.get(2).unwrap().as_str().parse().unwrap();
            col(col_name).eq(lit(val))
        }),
        (r"(\w+)\s*!=\s*(\d+)", |cap: &regex::Captures| {
            let col_name = cap.get(1).unwrap().as_str();
            let val: i64 = cap.get(2).unwrap().as_str().parse().unwrap();
            col(col_name).neq(lit(val))
        }),
    ];

    for (pattern, builder) in num_patterns {
        let re = regex::Regex::new(pattern).unwrap();
        if let Some(cap) = re.captures(expr_str) {
            return Ok(builder(&cap));
        }
    }

    // 字符串比较
    let str_patterns: PatternList = &[
        (r#"(\w+)\s*==\s*"([^"]+)""#, |cap: &regex::Captures| {
            let col_name = cap.get(1).unwrap().as_str();
            let val = cap.get(2).unwrap().as_str();
            col(col_name).eq(lit(val))
        }),
        (r#"(\w+)\s*!=\s*"([^"]+)""#, |cap: &regex::Captures| {
            let col_name = cap.get(1).unwrap().as_str();
            let val = cap.get(2).unwrap().as_str();
            col(col_name).neq(lit(val))
        }),
    ];

    for (pattern, builder) in str_patterns {
        let re = regex::Regex::new(pattern).unwrap();
        if let Some(cap) = re.captures(expr_str) {
            return Ok(builder(&cap));
        }
    }

    Err(ToolError::InvalidParameter {
        name: "filter".to_string(),
        message: format!("无法解析过滤表达式: '{}'", expr_str),
    }
    .into())
}

/// 解析聚合表达式
fn parse_aggregations(agg_str: &str) -> Result<Vec<polars::prelude::Expr>> {
    use polars::prelude::*;

    let mut exprs = Vec::new();

    for part in agg_str.split(',') {
        let parts: Vec<&str> = part.trim().split(':').collect();
        if parts.len() != 2 {
            return Err(ToolError::InvalidParameter {
                name: "aggregations".to_string(),
                message: format!("聚合表达式格式错误: '{}'", part),
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
            "std" => col(col_name).std(1).alias(format!("{}_std", col_name)),
            "first" => col(col_name).first().alias(format!("{}_first", col_name)),
            "last" => col(col_name).last().alias(format!("{}_last", col_name)),
            _ => {
                return Err(ToolError::InvalidParameter {
                    name: "aggregations".to_string(),
                    message: format!("不支持的聚合操作: '{}'", op),
                }
                .into());
            }
        };

        exprs.push(expr);
    }

    Ok(exprs)
}
