//! Excel file processing tools
//!
//! Provides Excel file reading capabilities, supporting:
//! - .xlsx / .xls / .xlsb / .ods formats
//! - Read worksheet list
//! - Extract cell data

use futures::future::BoxFuture;
use serde_json::Value;

use crate::security::{ResourceLimits, SecurityConfig};
use echo_core::error::{Result, ToolError};
use echo_core::tools::{Tool, ToolParameters, ToolResult};

const TOOL_NAME: &str = "excel_tools";

/// Excel reading tool
pub struct ExcelReadTool;

impl Tool for ExcelReadTool {
    fn name(&self) -> &str {
        "read_excel"
    }

    fn description(&self) -> &str {
        "Read Excel file (.xlsx/.xls/.xlsb/.ods), return sheet list and data preview."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the Excel file"
                },
                "sheet": {
                    "type": "string",
                    "description": "Worksheet name or index (e.g. 'Sheet1' or '0', defaults to first sheet)"
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

            let sheet_name = parameters.get("sheet").and_then(|v| v.as_str());

            let preview_rows = parameters
                .get("preview_rows")
                .and_then(|v| v.as_u64())
                .unwrap_or(10) as usize;

            let security = SecurityConfig::global();
            let path = security.validate_file(file_path)?;

            // Open file based on extension
            let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");

            // Limit preview rows to max allowed
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
                    // Try opening as xlsx
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

/// Excel info tool
pub struct ExcelInfoTool;

impl Tool for ExcelInfoTool {
    fn name(&self) -> &str {
        "excel_info"
    }

    fn description(&self) -> &str {
        "Get basic info about an Excel file: sheet list, row/column counts, etc."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the Excel file"
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
                    message: format!("Failed to open Excel file: {}", e),
                })?;

            let mut info = Vec::new();
            info.push(format!("File: {}", file_path));

            // Get sheet list
            let sheets = workbook.sheet_names();
            info.push(format!("Number of sheets: {}", sheets.len()));
            info.push(String::new());
            info.push("Sheet list:".to_string());

            for (idx, sheet_name) in sheets.iter().enumerate() {
                // Get sheet range
                let range = workbook.worksheet_range(sheet_name).map_err(|e| {
                    ToolError::ExecutionFailed {
                        tool: TOOL_NAME.to_string(),
                        message: format!("Failed to read sheet '{}': {:?}", sheet_name, e),
                    }
                })?;

                let (height, width) = range.get_size();
                info.push(format!(
                    "  {}. {} ({} rows x {} cols)",
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

/// Excel export tool (export to CSV)
pub struct ExcelToCsvTool;

impl Tool for ExcelToCsvTool {
    fn name(&self) -> &str {
        "excel_to_csv"
    }

    fn description(&self) -> &str {
        "Export an Excel worksheet to a CSV file."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "input_file": {
                    "type": "string",
                    "description": "Input Excel file path"
                },
                "output_file": {
                    "type": "string",
                    "description": "Output CSV file path"
                },
                "sheet": {
                    "type": "string",
                    "description": "Worksheet name (defaults to first sheet)"
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
                    message: format!("Failed to open Excel file: {}", e),
                })?;

            // Get sheet name
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
                        message: format!("Failed to read sheet '{}': {:?}", sheet, e),
                    })?;

            // Create output directory
            let output_path = security.validate_output_file(output_file)?;
            if let Some(parent) = output_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("Failed to create output directory: {}", e),
                })?;
            }

            // Write CSV
            let mut csv_content = Vec::new();
            let (height, width) = range.get_size();

            // Limit export rows
            let max_export_rows = security.limits.max_preview_rows;
            let export_height = height.min(max_export_rows);

            for row in 0..export_height {
                let mut row_data = Vec::new();
                for col in 0..width {
                    let cell_value = range
                        .get_value((row as u32, col as u32))
                        .map(format_cell_value)
                        .unwrap_or_default();
                    // Escape quotes and commas in CSV
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
                    message: format!("Failed to write CSV file: {}", e),
                }
            })?;

            Ok(ToolResult::success(format!(
                "Excel sheet '{}' exported to CSV: {} -> {}\nTotal {} rows{}",
                sheet,
                input_file,
                output_file,
                export_height,
                if height > max_export_rows {
                    format!(" (limited to {} rows)", max_export_rows)
                } else {
                    String::new()
                }
            )))
        })
    }
}

// ── Helper Functions ──────────────────────────────────────────────────

/// Read xlsx file
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
            message: format!("Failed to open Excel file: {}", e),
        })?;

    read_excel_data(&mut workbook, file_path, sheet_name, preview_rows, limits)
}

/// Read xls file
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
            message: format!("Failed to open Excel file: {}", e),
        })?;

    read_excel_data(&mut workbook, file_path, sheet_name, preview_rows, limits)
}

/// Read xlsb file
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
            message: format!("Failed to open Excel file: {}", e),
        })?;

    read_excel_data(&mut workbook, file_path, sheet_name, preview_rows, limits)
}

/// Read ods file
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
            message: format!("Failed to open Excel file: {}", e),
        })?;

    read_excel_data(&mut workbook, file_path, sheet_name, preview_rows, limits)
}

/// Generic Excel data reader
fn read_excel_data<R: calamine::Reader<std::io::BufReader<std::fs::File>>>(
    workbook: &mut R,
    file_path: &str,
    sheet_name: Option<&str>,
    preview_rows: usize,
    limits: &ResourceLimits,
) -> Result<String> {
    // Get sheet name
    let sheets = workbook.sheet_names();
    let target_sheet = if let Some(name) = sheet_name {
        name.to_string()
    } else {
        sheets
            .first()
            .cloned()
            .unwrap_or_else(|| "Sheet1".to_string())
    };

    // Read sheet data
    let range =
        workbook
            .worksheet_range(&target_sheet)
            .map_err(|e| ToolError::ExecutionFailed {
                tool: TOOL_NAME.to_string(),
                message: format!("Failed to read sheet '{}': {:?}", target_sheet, e),
            })?;

    let (height, width) = range.get_size();

    // Limit preview rows to max allowed
    let display_rows = preview_rows.min(height).min(limits.max_preview_rows);

    // Format output
    let mut result = Vec::new();
    result.push(format!("File: {}", file_path));
    result.push(format!("Sheet: {}", target_sheet));
    result.push(format!("Total rows: {}", height));
    result.push(format!("Total cols: {}", width));
    result.push(String::new());
    result.push(format!("Data preview (first {} rows):", display_rows));
    result.push(String::new());

    // Headers and data
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
        result.push(format!("... ({} total rows)", height));
    }

    Ok(result.join("\n"))
}

/// Format cell value
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
