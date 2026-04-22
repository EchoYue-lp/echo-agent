//! Excel 文件处理工具
//!
//! 提供 Excel 文件读取能力，支持：
//! - .xlsx / .xls / .xlsb / .ods 格式
//! - 读取工作表列表
//! - 提取单元格数据

use futures::future::BoxFuture;
use serde_json::Value;

use super::security::{ResourceLimits, SecurityConfig};
use crate::error::{Result, ToolError};
use crate::tools::{Tool, ToolParameters, ToolResult};

const TOOL_NAME: &str = "excel_tools";

/// Excel 读取工具
pub struct ExcelReadTool;

impl Tool for ExcelReadTool {
    fn name(&self) -> &str {
        "read_excel"
    }

    fn description(&self) -> &str {
        "读取 Excel 文件（.xlsx/.xls/.xlsb/.ods），返回工作表列表和数据预览。"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Excel 文件的绝对路径"
                },
                "sheet": {
                    "type": "string",
                    "description": "工作表名称或索引（如 'Sheet1' 或 '0'），默认读取第一个工作表"
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

            let sheet_name = parameters.get("sheet").and_then(|v| v.as_str());

            let preview_rows = parameters
                .get("preview_rows")
                .and_then(|v| v.as_u64())
                .unwrap_or(10) as usize;

            let security = SecurityConfig::global();
            let path = security.validate_file(file_path)?;

            // 根据扩展名打开文件
            let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");

            // 限制预览行数不超过最大限制
            let effective_preview_rows = preview_rows.min(security.limits.max_preview_rows);

            let result = match extension {
                "xlsx" => read_excel_xlsx(
                    file_path,
                    sheet_name,
                    effective_preview_rows,
                    &security.limits,
                )?,
                "xls" => read_excel_xls(
                    file_path,
                    sheet_name,
                    effective_preview_rows,
                    &security.limits,
                )?,
                "xlsb" => read_excel_xlsb(
                    file_path,
                    sheet_name,
                    effective_preview_rows,
                    &security.limits,
                )?,
                "ods" => read_excel_ods(
                    file_path,
                    sheet_name,
                    effective_preview_rows,
                    &security.limits,
                )?,
                _ => {
                    // 尝试作为 xlsx 打开
                    read_excel_xlsx(
                        file_path,
                        sheet_name,
                        effective_preview_rows,
                        &security.limits,
                    )?
                }
            };

            Ok(ToolResult::success(result))
        })
    }
}

/// Excel 信息工具
pub struct ExcelInfoTool;

impl Tool for ExcelInfoTool {
    fn name(&self) -> &str {
        "excel_info"
    }

    fn description(&self) -> &str {
        "获取 Excel 文件的基本信息：工作表列表、行列数等。"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Excel 文件的绝对路径"
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
            let _path = security.validate_file(file_path)?;

            use calamine::{Reader, Xlsx, open_workbook};

            let mut workbook: Xlsx<_> =
                open_workbook(file_path).map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("打开 Excel 文件失败: {}", e),
                })?;

            let mut info = Vec::new();
            info.push(format!("文件: {}", file_path));

            // 获取工作表列表
            let sheets = workbook.sheet_names();
            info.push(format!("工作表数量: {}", sheets.len()));
            info.push(String::new());
            info.push("工作表列表:".to_string());

            for (idx, sheet_name) in sheets.iter().enumerate() {
                // 获取工作表范围
                let range = workbook.worksheet_range(sheet_name).map_err(|e| {
                    ToolError::ExecutionFailed {
                        tool: TOOL_NAME.to_string(),
                        message: format!("读取工作表 '{}' 失败: {:?}", sheet_name, e),
                    }
                })?;

                let (height, width) = range.get_size();
                info.push(format!(
                    "  {}. {} ({} 行 x {} 列)",
                    idx + 1,
                    sheet_name,
                    height,
                    width
                ));
            }

            Ok(ToolResult::success(info.join("\n")))
        })
    }
}

/// Excel 导出工具（导出为 CSV）
pub struct ExcelToCsvTool;

impl Tool for ExcelToCsvTool {
    fn name(&self) -> &str {
        "excel_to_csv"
    }

    fn description(&self) -> &str {
        "将 Excel 工作表导出为 CSV 文件。"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "input_file": {
                    "type": "string",
                    "description": "输入 Excel 文件路径"
                },
                "output_file": {
                    "type": "string",
                    "description": "输出 CSV 文件路径"
                },
                "sheet": {
                    "type": "string",
                    "description": "工作表名称（默认第一个工作表）"
                }
            },
            "required": ["input_file", "output_file"]
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

            let sheet_name = parameters.get("sheet").and_then(|v| v.as_str());

            let security = SecurityConfig::global();
            let _path = security.validate_file(input_file)?;

            use calamine::{Reader, Xlsx, open_workbook};

            let mut workbook: Xlsx<_> =
                open_workbook(input_file).map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("打开 Excel 文件失败: {}", e),
                })?;

            // 获取工作表名称
            let sheet = if let Some(name) = sheet_name {
                name.to_string()
            } else {
                workbook
                    .sheet_names()
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "Sheet1".to_string())
            };

            let range =
                workbook
                    .worksheet_range(&sheet)
                    .map_err(|e| ToolError::ExecutionFailed {
                        tool: TOOL_NAME.to_string(),
                        message: format!("读取工作表 '{}' 失败: {:?}", sheet, e),
                    })?;

            // 创建输出目录
            let output_path = security.validate_output_file(output_file)?;
            if let Some(parent) = output_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("创建输出目录失败: {}", e),
                })?;
            }

            // 写入 CSV
            let mut csv_content = Vec::new();
            let (height, width) = range.get_size();

            // 限制导出行数
            let max_export_rows = security.limits.max_preview_rows;
            let export_height = height.min(max_export_rows);

            for row in 0..export_height {
                let mut row_data = Vec::new();
                for col in 0..width {
                    let cell_value = range
                        .get_value((row as u32, col as u32))
                        .map(format_cell_value)
                        .unwrap_or_default();
                    // 转义 CSV 中的引号和逗号
                    let escaped = if cell_value.contains(',')
                        || cell_value.contains('"')
                        || cell_value.contains('\n')
                    {
                        format!("\"{}\"", cell_value.replace('"', "\"\""))
                    } else {
                        cell_value
                    };
                    row_data.push(escaped);
                }
                csv_content.push(row_data.join(","));
            }

            std::fs::write(output_path, csv_content.join("\n")).map_err(|e| {
                ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("写入 CSV 文件失败: {}", e),
                }
            })?;

            Ok(ToolResult::success(format!(
                "Excel 工作表 '{}' 已导出为 CSV: {} -> {}\n共 {} 行数据{}",
                sheet,
                input_file,
                output_file,
                export_height,
                if height > max_export_rows {
                    format!(" (限制为 {} 行)", max_export_rows)
                } else {
                    String::new()
                }
            )))
        })
    }
}

// ── 辅助函数 ──────────────────────────────────────────────────────────

/// 读取 xlsx 文件
fn read_excel_xlsx(
    file_path: &str,
    sheet_name: Option<&str>,
    preview_rows: usize,
    limits: &ResourceLimits,
) -> Result<String> {
    use calamine::{Xlsx, open_workbook};

    let mut workbook: Xlsx<_> =
        open_workbook(file_path).map_err(|e| ToolError::ExecutionFailed {
            tool: TOOL_NAME.to_string(),
            message: format!("打开 Excel 文件失败: {}", e),
        })?;

    read_excel_data(&mut workbook, file_path, sheet_name, preview_rows, limits)
}

/// 读取 xls 文件
fn read_excel_xls(
    file_path: &str,
    sheet_name: Option<&str>,
    preview_rows: usize,
    limits: &ResourceLimits,
) -> Result<String> {
    use calamine::{Xls, open_workbook};

    let mut workbook: Xls<_> =
        open_workbook(file_path).map_err(|e| ToolError::ExecutionFailed {
            tool: TOOL_NAME.to_string(),
            message: format!("打开 Excel 文件失败: {}", e),
        })?;

    read_excel_data(&mut workbook, file_path, sheet_name, preview_rows, limits)
}

/// 读取 xlsb 文件
fn read_excel_xlsb(
    file_path: &str,
    sheet_name: Option<&str>,
    preview_rows: usize,
    limits: &ResourceLimits,
) -> Result<String> {
    use calamine::{Xlsb, open_workbook};

    let mut workbook: Xlsb<_> =
        open_workbook(file_path).map_err(|e| ToolError::ExecutionFailed {
            tool: TOOL_NAME.to_string(),
            message: format!("打开 Excel 文件失败: {}", e),
        })?;

    read_excel_data(&mut workbook, file_path, sheet_name, preview_rows, limits)
}

/// 读取 ods 文件
fn read_excel_ods(
    file_path: &str,
    sheet_name: Option<&str>,
    preview_rows: usize,
    limits: &ResourceLimits,
) -> Result<String> {
    use calamine::{Ods, open_workbook};

    let mut workbook: Ods<_> =
        open_workbook(file_path).map_err(|e| ToolError::ExecutionFailed {
            tool: TOOL_NAME.to_string(),
            message: format!("打开 Excel 文件失败: {}", e),
        })?;

    read_excel_data(&mut workbook, file_path, sheet_name, preview_rows, limits)
}

/// 通用的 Excel 数据读取
fn read_excel_data<R: calamine::Reader<std::io::BufReader<std::fs::File>>>(
    workbook: &mut R,
    file_path: &str,
    sheet_name: Option<&str>,
    preview_rows: usize,
    limits: &ResourceLimits,
) -> Result<String> {
    // 获取工作表名称
    let sheets = workbook.sheet_names();
    let target_sheet = if let Some(name) = sheet_name {
        name.to_string()
    } else {
        sheets
            .first()
            .cloned()
            .unwrap_or_else(|| "Sheet1".to_string())
    };

    // 读取工作表数据
    let range =
        workbook
            .worksheet_range(&target_sheet)
            .map_err(|e| ToolError::ExecutionFailed {
                tool: TOOL_NAME.to_string(),
                message: format!("读取工作表 '{}' 失败: {:?}", target_sheet, e),
            })?;

    let (height, width) = range.get_size();

    // 限制预览行数不超过最大限制
    let display_rows = preview_rows.min(height).min(limits.max_preview_rows);

    // 格式化输出
    let mut result = Vec::new();
    result.push(format!("文件: {}", file_path));
    result.push(format!("工作表: {}", target_sheet));
    result.push(format!("总行数: {}", height));
    result.push(format!("总列数: {}", width));
    result.push(String::new());
    result.push(format!("数据预览 (前 {} 行):", display_rows));
    result.push(String::new());

    // 表头和数据
    for row in 0..display_rows {
        let mut row_data = Vec::new();
        for col in 0..width {
            let cell_value = range
                .get_value((row as u32, col as u32))
                .map(format_cell_value)
                .unwrap_or_default();
            row_data.push(cell_value);
        }
        result.push(row_data.join("\t"));
    }

    if height > display_rows {
        result.push(format!("... (共 {} 行)", height));
    }

    Ok(result.join("\n"))
}

/// 格式化单元格值
fn format_cell_value(value: &calamine::Data) -> String {
    use calamine::Data;
    match value {
        Data::Empty => String::new(),
        Data::String(s) => s.clone(),
        Data::Float(f) => format!("{:.2}", f),
        Data::Int(i) => i.to_string(),
        Data::Bool(b) => b.to_string(),
        Data::DateTime(dt) => format!("{:?}", dt),
        Data::Error(e) => format!("Error: {:?}", e),
        Data::DateTimeIso(dt) => dt.clone(),
        Data::DurationIso(d) => d.clone(),
    }
}
