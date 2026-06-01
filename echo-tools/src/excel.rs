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
use echo_core::tools::permission::ToolPermission;
use echo_core::tools::{Tool, ToolParameters, ToolResult};

const TOOL_NAME: &str = "excel_tools";

/// Excel reading tool
pub struct ExcelReadTool;

impl Tool for ExcelReadTool {
    fn name(&self) -> &str {
        "read_excel"
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Read]
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

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Read]
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

                // Show header row (first row)
                if height > 0 {
                    let headers: Vec<String> = (0..width)
                        .map(|col| {
                            range
                                .get_value((0, col as u32))
                                .map(format_cell_value)
                                .unwrap_or_default()
                        })
                        .collect();
                    let non_empty: Vec<&str> = headers
                        .iter()
                        .map(|s| s.as_str())
                        .filter(|s| !s.is_empty())
                        .collect();
                    if !non_empty.is_empty() {
                        info.push(format!("    Headers: {}", non_empty.join(" | ")));
                    }
                }
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

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Read, ToolPermission::Write]
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

            tokio::fs::write(output_path, csv_content.join("\n"))
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("Failed to write CSV file: {}", e),
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

// ── ExcelProfileTool ───────────────────────────────────────────────

/// Excel profiling tool — automatic column type detection and statistical summary.
pub struct ExcelProfileTool;

impl Tool for ExcelProfileTool {
    fn name(&self) -> &str {
        "excel_profile"
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Read]
    }

    fn description(&self) -> &str {
        "Profile an Excel file: detect column types, count nulls, compute basic statistics (min/max/mean for numbers). \
         Useful for quick data quality assessment."
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
                    "description": "Worksheet name (defaults to first sheet)"
                },
                "max_rows": {
                    "type": "integer",
                    "description": "Maximum rows to profile (default: all rows)"
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
            let max_rows = parameters
                .get("max_rows")
                .and_then(|v| v.as_u64())
                .map(|r| r as usize);

            let security = SecurityConfig::global();
            let _path = security.validate_file(file_path)?;

            use calamine::{Reader, Xlsx, open_workbook};

            let mut workbook: Xlsx<_> =
                open_workbook(file_path).map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("Failed to open Excel file: {}", e),
                })?;

            let sheets = workbook.sheet_names();
            let target_sheet = if let Some(name) = sheet_name {
                name.to_string()
            } else {
                sheets
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "Sheet1".to_string())
            };

            let range = workbook.worksheet_range(&target_sheet).map_err(|e| {
                ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("Failed to read sheet '{}': {:?}", target_sheet, e),
                }
            })?;

            let (height, width) = range.get_size();
            let profile_rows = max_rows.unwrap_or(height).min(height);

            // Detect column types and compute stats
            let mut col_types: Vec<String> = Vec::new();
            let mut col_nulls: Vec<usize> = Vec::new();
            let mut col_numeric_values: Vec<Vec<f64>> = Vec::new();

            for col in 0..width {
                let mut num_count = 0usize;
                let mut str_count = 0usize;
                let mut null_count = 0usize;
                let mut numeric_vals = Vec::new();

                for row in 0..profile_rows {
                    match range.get_value((row as u32, col as u32)) {
                        Some(calamine::Data::Empty) | None => null_count += 1,
                        Some(calamine::Data::Float(f)) => {
                            num_count += 1;
                            numeric_vals.push(*f);
                        }
                        Some(calamine::Data::Int(i)) => {
                            num_count += 1;
                            numeric_vals.push(*i as f64);
                        }
                        Some(_) => str_count += 1,
                    }
                }

                let col_type = if num_count > str_count && num_count > null_count {
                    "numeric"
                } else if str_count > 0 {
                    "string"
                } else {
                    "empty"
                };
                col_types.push(col_type.to_string());
                col_nulls.push(null_count);
                col_numeric_values.push(numeric_vals);
            }

            // Build profile report
            let mut report = Vec::new();
            report.push(format!("=== Profile: {} / {} ===", file_path, target_sheet));
            report.push(format!("Rows: {} (profiled: {})", height, profile_rows));
            report.push(format!("Columns: {}", width));
            report.push(String::new());

            // Get header row if available
            let headers: Vec<String> = (0..width)
                .map(|col| {
                    range
                        .get_value((0, col as u32))
                        .map(format_cell_value)
                        .unwrap_or_else(|| format!("Col_{}", col + 1))
                })
                .collect();

            report.push("Column Details:".to_string());
            for col in 0..width {
                let null_pct = if profile_rows > 0 {
                    col_nulls[col] as f64 / profile_rows as f64 * 100.0
                } else {
                    0.0
                };

                let mut detail = format!(
                    "  {} [{}]: {} nulls ({:.1}%)",
                    headers[col], col_types[col], col_nulls[col], null_pct
                );

                // Add stats for numeric columns
                if col_types[col] == "numeric" && !col_numeric_values[col].is_empty() {
                    let vals = &col_numeric_values[col];
                    let min = vals.iter().cloned().fold(f64::INFINITY, f64::min);
                    let max = vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
                    let mean = vals.iter().sum::<f64>() / vals.len() as f64;
                    detail.push_str(&format!(
                        " | min={:.2} max={:.2} mean={:.2}",
                        min, max, mean
                    ));
                }

                report.push(detail);
            }

            Ok(ToolResult::success(report.join("\n")))
        })
    }
}

// ── ExcelWriteTool ─────────────────────────────────────────────────

/// Excel write tool — create or modify Excel files.
pub struct ExcelWriteTool;

impl Tool for ExcelWriteTool {
    fn name(&self) -> &str {
        "write_excel"
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Write]
    }

    fn description(&self) -> &str {
        "Create or write data to an Excel file (.xlsx). Supports specifying sheet name, headers, and data rows."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Output Excel file path"
                },
                "sheet": {
                    "type": "string",
                    "description": "Sheet name (default: 'Sheet1')"
                },
                "headers": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Column headers"
                },
                "data": {
                    "type": "array",
                    "items": {
                        "type": "array",
                        "items": {}
                    },
                    "description": "Data rows (array of arrays)"
                }
            },
            "required": ["file_path", "data"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let file_path = parameters
                .get("file_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("file_path".to_string()))?;

            let sheet_name = parameters
                .get("sheet")
                .and_then(|v| v.as_str())
                .unwrap_or("Sheet1");

            let headers: Option<Vec<String>> = parameters
                .get("headers")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                });

            let data = parameters
                .get("data")
                .and_then(|v| v.as_array())
                .ok_or_else(|| ToolError::MissingParameter("data".to_string()))?;

            let security = SecurityConfig::global();
            let output_path = security.validate_output_file(file_path)?;

            // Create parent directory if needed
            if let Some(parent) = output_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("Failed to create output directory: {}", e),
                })?;
            }

            let mut workbook = rust_xlsxwriter::Workbook::new();
            let worksheet = workbook.add_worksheet();

            // Set sheet name
            worksheet
                .set_name(sheet_name)
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("Failed to set sheet name: {}", e),
                })?;

            let mut row = 0u32;

            // Write headers
            if let Some(ref headers) = headers {
                for (col, header) in headers.iter().enumerate() {
                    worksheet
                        .write_string(row, col as u16, header)
                        .map_err(|e| ToolError::ExecutionFailed {
                            tool: TOOL_NAME.to_string(),
                            message: format!("Failed to write header: {}", e),
                        })?;
                }
                row += 1;
            }

            // Write data rows
            let mut rows_written = 0usize;
            for row_data in data {
                if let Some(cols) = row_data.as_array() {
                    for (col, value) in cols.iter().enumerate() {
                        let write_err = |e| ToolError::ExecutionFailed {
                            tool: TOOL_NAME.to_string(),
                            message: format!("Failed to write cell ({}, {}): {}", row, col, e),
                        };
                        match value {
                            serde_json::Value::Number(n) => {
                                if let Some(f) = n.as_f64() {
                                    worksheet
                                        .write_number(row, col as u16, f)
                                        .map_err(write_err)?;
                                } else if let Some(i) = n.as_i64() {
                                    worksheet
                                        .write_number(row, col as u16, i as f64)
                                        .map_err(write_err)?;
                                }
                            }
                            serde_json::Value::String(s) => {
                                worksheet
                                    .write_string(row, col as u16, s)
                                    .map_err(write_err)?;
                            }
                            serde_json::Value::Bool(b) => {
                                worksheet
                                    .write_boolean(row, col as u16, *b)
                                    .map_err(write_err)?;
                            }
                            serde_json::Value::Null => {
                                // Skip null cells
                            }
                            _ => {
                                worksheet
                                    .write_string(row, col as u16, &value.to_string())
                                    .map_err(write_err)?;
                            }
                        }
                    }
                    rows_written += 1;
                }
                row += 1;
            }

            workbook
                .save(&output_path)
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("Failed to save Excel file: {}", e),
                })?;

            Ok(ToolResult::success(format!(
                "Excel file written: {} (sheet: {}, {} rows, {} cols)",
                output_path.display(),
                sheet_name,
                rows_written,
                headers.as_ref().map(|h| h.len()).unwrap_or(0)
            )))
        })
    }
}

// ── ExcelLoadTool ─────────────────────────────────────────────────

/// Excel → Polars DataFrame bridge tool.
///
/// Reads an Excel sheet, converts to a Polars DataFrame with type inference,
/// and saves as Parquet (default) or CSV. This unlocks all Polars data tools
/// (filter, aggregate, stats, transform, etc.) for Excel data.
#[cfg(feature = "data")]
pub struct ExcelLoadTool;

#[cfg(feature = "data")]
impl Tool for ExcelLoadTool {
    fn name(&self) -> &str {
        "excel_load"
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Read]
    }

    fn description(&self) -> &str {
        "Load Excel sheet into a Polars DataFrame and save as Parquet/CSV. \
         This bridges Excel data into the data processing pipeline — \
         after loading, use data_filter, data_aggregate, data_stats, data_transform, etc."
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
                    "description": "Worksheet name (defaults to first sheet)"
                },
                "output_file": {
                    "type": "string",
                    "description": "Output file path (default: same name with .parquet extension)"
                },
                "format": {
                    "type": "string",
                    "description": "Output format: 'parquet' (default) or 'csv'"
                },
                "header_row": {
                    "type": "boolean",
                    "description": "Whether first row is headers (default: true)"
                },
                "max_rows": {
                    "type": "integer",
                    "description": "Maximum rows to load (default: all)"
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
            let output_file = parameters.get("output_file").and_then(|v| v.as_str());
            let format = parameters
                .get("format")
                .and_then(|v| v.as_str())
                .unwrap_or("parquet");
            let has_header = parameters
                .get("header_row")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let max_rows = parameters
                .get("max_rows")
                .and_then(|v| v.as_u64())
                .map(|r| r as usize);

            let security = SecurityConfig::global();
            let _path = security.validate_file(file_path)?;

            // Determine output path
            let out_path = if let Some(out) = output_file {
                security.validate_output_file(out)?
            } else {
                let ext = match format {
                    "csv" => ".csv",
                    _ => ".parquet",
                };
                let input = std::path::Path::new(file_path);
                let stem = input.file_stem().unwrap_or_default();
                let parent = input.parent().unwrap_or(std::path::Path::new("."));
                let out_name = format!("{}{}", stem.to_string_lossy(), ext);
                parent.join(out_name)
            };

            // Read Excel
            use calamine::{Reader, Xlsx, open_workbook};
            let mut workbook: Xlsx<_> =
                open_workbook(file_path).map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("Failed to open Excel file: {}", e),
                })?;

            let sheets = workbook.sheet_names();
            let target_sheet = if let Some(name) = sheet_name {
                name.to_string()
            } else {
                sheets
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "Sheet1".to_string())
            };

            let range = workbook.worksheet_range(&target_sheet).map_err(|e| {
                ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("Failed to read sheet '{}': {:?}", target_sheet, e),
                }
            })?;

            let (height, width) = range.get_size();
            if height == 0 || width == 0 {
                return Ok(ToolResult::success(
                    "Sheet is empty, nothing to load.".to_string(),
                ));
            }

            let data_start_row = if has_header { 1 } else { 0 };
            let data_rows = if has_header { height - 1 } else { height };
            let load_rows = max_rows.unwrap_or(data_rows).min(data_rows);

            // Extract headers
            let headers: Vec<String> = if has_header {
                (0..width)
                    .map(|col| {
                        range
                            .get_value((0, col as u32))
                            .map(format_cell_value)
                            .unwrap_or_else(|| format!("Column_{}", col + 1))
                    })
                    .collect()
            } else {
                (0..width)
                    .map(|col| format!("Column_{}", col + 1))
                    .collect()
            };

            // Detect column types by scanning all data rows
            let mut col_is_numeric: Vec<bool> = vec![true; width];
            let mut col_has_float: Vec<bool> = vec![false; width];
            let mut col_is_bool: Vec<bool> = vec![true; width];

            for row_idx in data_start_row..(data_start_row + load_rows) {
                for col in 0..width {
                    match range.get_value((row_idx as u32, col as u32)) {
                        Some(calamine::Data::Float(_)) => {
                            col_has_float[col] = true;
                            col_is_bool[col] = false;
                        }
                        Some(calamine::Data::Int(_)) => {
                            col_is_bool[col] = false;
                        }
                        Some(calamine::Data::Bool(_)) => {
                            // keep is_bool true
                        }
                        Some(calamine::Data::Empty) | None => {}
                        _ => {
                            col_is_numeric[col] = false;
                            col_is_bool[col] = false;
                        }
                    }
                }
            }

            // Build Polars Series for each column
            use polars::prelude::*;
            let mut series_list: Vec<Series> = Vec::with_capacity(width);

            for col in 0..width {
                let col_name = PlSmallStr::from_str(&headers[col]);

                if col_is_bool[col] {
                    // Boolean column
                    let vals: Vec<Option<bool>> = (data_start_row..(data_start_row + load_rows))
                        .map(
                            |row_idx| match range.get_value((row_idx as u32, col as u32)) {
                                Some(calamine::Data::Bool(b)) => Some(*b),
                                _ => None,
                            },
                        )
                        .collect();
                    series_list.push(Series::new(col_name, vals));
                } else if col_is_numeric[col] {
                    if col_has_float[col] {
                        // Float column
                        let vals: Vec<Option<f64>> = (data_start_row..(data_start_row + load_rows))
                            .map(
                                |row_idx| match range.get_value((row_idx as u32, col as u32)) {
                                    Some(calamine::Data::Float(f)) => Some(*f),
                                    Some(calamine::Data::Int(i)) => Some(*i as f64),
                                    _ => None,
                                },
                            )
                            .collect();
                        series_list.push(Series::new(col_name, vals));
                    } else {
                        // Integer column
                        let vals: Vec<Option<i64>> = (data_start_row..(data_start_row + load_rows))
                            .map(
                                |row_idx| match range.get_value((row_idx as u32, col as u32)) {
                                    Some(calamine::Data::Int(i)) => Some(*i),
                                    Some(calamine::Data::Float(f)) => Some(*f as i64),
                                    _ => None,
                                },
                            )
                            .collect();
                        series_list.push(Series::new(col_name, vals));
                    }
                } else {
                    // String column
                    let vals: Vec<Option<String>> = (data_start_row..(data_start_row + load_rows))
                        .map(
                            |row_idx| match range.get_value((row_idx as u32, col as u32)) {
                                Some(calamine::Data::Empty) | None => None,
                                Some(v) => {
                                    let s = format_cell_value(v);
                                    if s.is_empty() { None } else { Some(s) }
                                }
                            },
                        )
                        .collect();
                    series_list.push(Series::new(col_name, vals));
                }
            }

            // Build DataFrame
            let columns: Vec<polars::prelude::Column> =
                series_list.into_iter().map(|s| s.into_column()).collect();
            let col_count = columns.len();
            let mut df =
                DataFrame::new(col_count, columns).map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("Failed to create DataFrame: {}", e),
                })?;

            // Create parent directory if needed
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("Failed to create output directory: {}", e),
                })?;
            }

            // Write output
            match format {
                "csv" => {
                    let mut file = std::fs::File::create(&out_path).map_err(|e| {
                        ToolError::ExecutionFailed {
                            tool: TOOL_NAME.to_string(),
                            message: format!("Failed to create CSV file: {}", e),
                        }
                    })?;
                    polars::prelude::CsvWriter::new(&mut file)
                        .finish(&mut df)
                        .map_err(|e| ToolError::ExecutionFailed {
                            tool: TOOL_NAME.to_string(),
                            message: format!("Failed to write CSV: {}", e),
                        })?;
                }
                _ => {
                    let file = std::fs::File::create(&out_path).map_err(|e| {
                        ToolError::ExecutionFailed {
                            tool: TOOL_NAME.to_string(),
                            message: format!("Failed to create Parquet file: {}", e),
                        }
                    })?;
                    polars::prelude::ParquetWriter::new(file)
                        .finish(&mut df)
                        .map_err(|e| ToolError::ExecutionFailed {
                            tool: TOOL_NAME.to_string(),
                            message: format!("Failed to write Parquet: {}", e),
                        })?;
                }
            }

            // Build result summary
            let shape = df.shape();
            let col_names: Vec<String> = df
                .get_column_names()
                .iter()
                .map(|s| s.to_string())
                .collect();
            let dtypes: Vec<String> = col_names
                .iter()
                .map(|name| {
                    let dtype = df
                        .column(name)
                        .map(|c| c.dtype().clone())
                        .unwrap_or_default();
                    format!("{}: {:?}", name, dtype)
                })
                .collect();

            // Preview: first 10 rows
            let preview_height = 10.min(shape.0);
            let preview = df.head(Some(preview_height));

            let mut result = Vec::new();
            result.push(format!(
                "Loaded Excel → {} ({})",
                out_path.display(),
                format
            ));
            result.push(format!("Shape: {} rows x {} columns", shape.0, shape.1));
            result.push(String::new());
            result.push("Columns:".to_string());
            for d in &dtypes {
                result.push(format!("  {}", d));
            }
            result.push(String::new());
            result.push(format!("Preview (first {} rows):", preview_height));
            result.push(format!("{:?}", preview));

            result.push(String::new());
            result.push(format!(
                "Use this file with data tools: data_filter, data_aggregate, data_stats, data_transform, etc."
            ));

            Ok(ToolResult::success(result.join("\n")))
        })
    }
}
