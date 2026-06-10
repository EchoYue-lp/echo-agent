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
                },
                "skip_rows": {
                    "type": "integer",
                    "description": "Number of rows to skip from the top (default 0). Use this when the file has title/metadata rows before the actual header row."
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

            let skip_rows = parameters
                .get("skip_rows")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;

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
                    skip_rows,
                    &security.limits,
                )?,
                "xls" => read_excel_xls(
                    file_path,
                    sheet_name,
                    effective_preview_rows,
                    skip_rows,
                    &security.limits,
                )?,
                "xlsb" => read_excel_xlsb(
                    file_path,
                    sheet_name,
                    effective_preview_rows,
                    skip_rows,
                    &security.limits,
                )?,
                "ods" => read_excel_ods(
                    file_path,
                    sheet_name,
                    effective_preview_rows,
                    skip_rows,
                    &security.limits,
                )?,
                _ => {
                    // Try opening as xlsx
                    read_excel_xlsx(
                        file_path,
                        sheet_name,
                        effective_preview_rows,
                        skip_rows,
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

            let mut workbook = open_excel_auto(file_path)?;

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
                        info.push(format!("    Row 1: {}", non_empty.join(" | ")));
                    }

                    // Show row 2 if it looks different from row 1 (hint: might be the real header)
                    if height > 1 {
                        let row2: Vec<String> = (0..width)
                            .map(|col| {
                                range
                                    .get_value((1, col as u32))
                                    .map(format_cell_value)
                                    .unwrap_or_default()
                            })
                            .collect();
                        let row2_non_empty: Vec<&str> = row2.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect();
                        if !row2_non_empty.is_empty() && row2 != headers {
                            info.push(format!("    Row 2: {}", row2_non_empty.join(" | ")));
                        }
                    }
                }

                // Detect formula cells using worksheet_formula
                if let Ok(formula_range) = workbook.worksheet_formula(sheet_name) {
                    let mut formula_count = 0usize;
                    let mut sample_formulas: Vec<String> = Vec::new();
                    let (fh, fw) = formula_range.get_size();
                    for row in 0..fh {
                        for col in 0..fw {
                            if let Some(formula_str) = formula_range.get_value((row as u32, col as u32)) {
                                if !formula_str.is_empty() {
                                    formula_count += 1;
                                    if sample_formulas.len() < 5 {
                                        // Get the cell reference (e.g. "A10")
                                        let col_letter = col_to_letter(col);
                                        sample_formulas.push(format!("{}{}={}", col_letter, row + 1, formula_str));
                                    }
                                }
                            }
                        }
                    }
                    if formula_count > 0 {
                        info.push(format!("    Formulas: {} cells", formula_count));
                        if !sample_formulas.is_empty() {
                            info.push(format!("    Sample: {}", sample_formulas.join(", ")));
                        }
                        info.push("    Note: Formula cells return cached results. The formulas themselves are preserved in the original file.".to_string());
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
                    "description": "Worksheet name or index (e.g. 'Sheet1' or '0', defaults to first sheet)"
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

            let mut workbook = open_excel_auto(input_file)?;

            // Get sheet name
            let sheets = workbook.sheet_names();
            let sheet = resolve_sheet_name(&sheets, sheet_name);

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

            // Prepend UTF-8 BOM so Excel correctly detects encoding for non-ASCII content
            let bom = "\xEF\xBB\xBF";
            let csv_output = format!("{}{}", bom, csv_content.join("\n"));
            tokio::fs::write(output_path, csv_output)
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
    skip_rows: usize,
    limits: &ResourceLimits,
) -> Result<String> {
    use calamine::{Xlsx, open_workbook};

    let mut workbook: Xlsx<_> =
        open_workbook(file_path).map_err(|e| ToolError::ExecutionFailed {
            tool: TOOL_NAME.to_string(),
            message: format!("Failed to open Excel file: {}", e),
        })?;

    read_excel_data(&mut workbook, file_path, sheet_name, preview_rows, skip_rows, limits)
}

/// Read xls file
fn read_excel_xls(
    file_path: &str,
    sheet_name: Option<&str>,
    preview_rows: usize,
    skip_rows: usize,
    limits: &ResourceLimits,
) -> Result<String> {
    use calamine::{Xls, open_workbook};

    let mut workbook: Xls<_> =
        open_workbook(file_path).map_err(|e| ToolError::ExecutionFailed {
            tool: TOOL_NAME.to_string(),
            message: format!("Failed to open Excel file: {}", e),
        })?;

    read_excel_data(&mut workbook, file_path, sheet_name, preview_rows, skip_rows, limits)
}

/// Read xlsb file
fn read_excel_xlsb(
    file_path: &str,
    sheet_name: Option<&str>,
    preview_rows: usize,
    skip_rows: usize,
    limits: &ResourceLimits,
) -> Result<String> {
    use calamine::{Xlsb, open_workbook};

    let mut workbook: Xlsb<_> =
        open_workbook(file_path).map_err(|e| ToolError::ExecutionFailed {
            tool: TOOL_NAME.to_string(),
            message: format!("Failed to open Excel file: {}", e),
        })?;

    read_excel_data(&mut workbook, file_path, sheet_name, preview_rows, skip_rows, limits)
}

/// Read ods file
fn read_excel_ods(
    file_path: &str,
    sheet_name: Option<&str>,
    preview_rows: usize,
    skip_rows: usize,
    limits: &ResourceLimits,
) -> Result<String> {
    use calamine::{Ods, open_workbook};

    let mut workbook: Ods<_> =
        open_workbook(file_path).map_err(|e| ToolError::ExecutionFailed {
            tool: TOOL_NAME.to_string(),
            message: format!("Failed to open Excel file: {}", e),
        })?;

    read_excel_data(&mut workbook, file_path, sheet_name, preview_rows, skip_rows, limits)
}

/// Generic Excel data reader
fn read_excel_data<R: calamine::Reader<std::io::BufReader<std::fs::File>>>(
    workbook: &mut R,
    file_path: &str,
    sheet_name: Option<&str>,
    preview_rows: usize,
    skip_rows: usize,
    limits: &ResourceLimits,
) -> Result<String> {
    // Get sheet name
    let sheets = workbook.sheet_names();
    let target_sheet = resolve_sheet_name(&sheets, sheet_name);

    // Read sheet data
    let range =
        workbook
            .worksheet_range(&target_sheet)
            .map_err(|e| ToolError::ExecutionFailed {
                tool: TOOL_NAME.to_string(),
                message: format!("Failed to read sheet '{}': {:?}", target_sheet, e),
            })?;

    let (height, width) = range.get_size();

    // Apply skip_rows offset
    let skip = skip_rows.min(height.saturating_sub(1));
    let available_rows = height - skip;

    // Limit preview rows to max allowed
    let display_rows = preview_rows.min(available_rows).min(limits.max_preview_rows);

    // Format output
    let mut result = Vec::new();
    result.push(format!("File: {}", file_path));
    result.push(format!("Sheet: {}", target_sheet));
    result.push(format!("Total rows: {}", height));
    result.push(format!("Total cols: {}", width));
    if skip > 0 {
        result.push(format!("Skipped rows: {} (starting from row {})", skip, skip + 1));
    }
    result.push(String::new());
    result.push(format!("Data preview ({} rows from row {}):", display_rows, skip + 1));
    result.push(String::new());

    // Headers and data (with skip offset)
    for row in 0..display_rows {
        let mut row_data = Vec::new();
        for col in 0..width {
            let cell_value = range
                .get_value(((row + skip) as u32, col as u32))
                .map(format_cell_value)
                .unwrap_or_default();
            row_data.push(cell_value);
        }
        result.push(row_data.join("\t"));
    }

    if available_rows > display_rows {
        result.push(format!("... ({} total rows after skip)", available_rows));
    }

    Ok(result.join("\n"))
}

/// Format cell value with proper type handling
fn format_cell_value(value: &calamine::Data) -> String {
    use calamine::Data;
    match value {
        Data::Empty => String::new(),
        Data::String(s) => s.clone(),
        Data::Float(f) => format_smart_float(*f),
        Data::Int(i) => i.to_string(),
        Data::Bool(b) => b.to_string(),
        Data::DateTime(dt) => {
            // Convert Excel serial date to human-readable format
            match dt.as_datetime() {
                Some(ndt) => {
                    // If time component is midnight, show date only
                    if ndt.time() == chrono::NaiveTime::MIN {
                        ndt.format("%Y-%m-%d").to_string()
                    } else {
                        ndt.format("%Y-%m-%d %H:%M:%S").to_string()
                    }
                }
                None => format!("{}", dt),
            }
        }
        Data::Error(e) => format!("Error: {:?}", e),
        Data::DateTimeIso(dt) => dt.clone(),
        Data::DurationIso(d) => d.clone(),
    }
}

/// Smart float formatting: preserve precision (up to 6 decimals), strip trailing zeros.
fn format_smart_float(f: f64) -> String {
    if f.is_nan() {
        return "NaN".to_string();
    }
    if f.is_infinite() {
        return if f > 0.0 { "Infinity".to_string() } else { "-Infinity".to_string() };
    }
    // Use up to 6 decimal places, then strip trailing zeros
    let formatted = format!("{:.6}", f);
    if formatted.contains('.') {
        let trimmed = formatted.trim_end_matches('0').trim_end_matches('.');
        trimmed.to_string()
    } else {
        formatted
    }
}

/// Open any Excel format (.xlsx/.xls/.xlsb/.ods) using auto-detection.
fn open_excel_auto(file_path: &str) -> Result<calamine::Sheets<std::io::BufReader<std::fs::File>>> {
    calamine::open_workbook_auto(file_path).map_err(|e| ToolError::ExecutionFailed {
        tool: TOOL_NAME.to_string(),
        message: format!("Failed to open Excel file '{}': {}", file_path, e),
    })
}

/// Resolve a sheet name or numeric index to an actual sheet name.
/// If `sheet_name` is a numeric string like "0", "1", etc., return the corresponding sheet name.
fn resolve_sheet_name(
    sheets: &[String],
    sheet_name: Option<&str>,
) -> String {
    match sheet_name {
        Some(name) => {
            // Try parsing as index
            if let Ok(idx) = name.parse::<usize>() {
                if idx < sheets.len() {
                    return sheets[idx].clone();
                }
            }
            // Otherwise treat as literal name
            name.to_string()
        }
        None => sheets
            .first()
            .cloned()
            .unwrap_or_else(|| "Sheet1".to_string()),
    }
}

/// Convert 0-based column index to Excel column letter (0→A, 1→B, ..., 25→Z, 26→AA, ...)
fn col_to_letter(col: usize) -> String {
    let mut result = String::new();
    let mut c = col;
    loop {
        let r = c % 26;
        result.insert(0, (b'A' + r as u8) as char);
        if c < 26 {
            break;
        }
        c = c / 26 - 1;
    }
    result
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
                    "description": "Worksheet name or index (e.g. 'Sheet1' or '0', defaults to first sheet)"
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

            let mut workbook = open_excel_auto(file_path)?;

            let sheets = workbook.sheet_names();
            let target_sheet = resolve_sheet_name(&sheets, sheet_name);

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
        "Load Excel sheet(s) into Polars DataFrame(s) and save as Parquet/CSV. \
         This bridges Excel data into the data processing pipeline — \
         after loading, use filter_data, aggregate_data, data_stats, transform_data, etc.\n\n\
         Supports:\n\
         - Single sheet: sheet='Sheet1' or sheet='0'\n\
         - Multiple sheets: sheets='Sheet1,Sheet2' or sheets='0,1,2'\n\
         - All sheets: sheets='all'\n\
         - Custom header row: header_row=2 means row index 2 (0-based) is the header (default 0)\n\
         - Skip rows: skip_rows=2 to ignore the first 2 rows entirely"
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
                    "description": "Single worksheet name or index (e.g. 'Sheet1' or '0'). Use 'sheets' for multiple."
                },
                "sheets": {
                    "type": "string",
                    "description": "Multiple worksheets: comma-separated names/indices (e.g. 'Sheet1,Sheet2') or 'all' for all sheets. Overrides 'sheet'."
                },
                "output_file": {
                    "type": "string",
                    "description": "Output file path (default: same name with .parquet extension). For multiple sheets, a suffix is added."
                },
                "format": {
                    "type": "string",
                    "description": "Output format: 'parquet' (default) or 'csv'"
                },
                "header_row": {
                    "type": "integer",
                    "description": "0-based row index of the header row (default 0). Use 1 if row 2 is the actual header, 2 if row 3, etc. Rows before the header are skipped."
                },
                "max_rows": {
                    "type": "integer",
                    "description": "Maximum rows to load per sheet (default: all)"
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
            let sheets_param = parameters.get("sheets").and_then(|v| v.as_str());
            let output_file = parameters.get("output_file").and_then(|v| v.as_str());
            let format = parameters
                .get("format")
                .and_then(|v| v.as_str())
                .unwrap_or("parquet");
            // header_row: integer (0-based row index of header). Default 0.
            let header_row_idx = parameters
                .get("header_row")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;
            let max_rows = parameters
                .get("max_rows")
                .and_then(|v| v.as_u64())
                .map(|r| r as usize);

            let security = SecurityConfig::global();
            let _path = security.validate_file(file_path)?;

            // Read Excel
            let mut workbook = open_excel_auto(file_path)?;
            let all_sheets = workbook.sheet_names();

            // Determine which sheets to load
            let sheets_to_load: Vec<String> = if let Some(sheets_str) = sheets_param {
                if sheets_str.trim().to_lowercase() == "all" {
                    all_sheets.clone()
                } else {
                    sheets_str
                        .split(',')
                        .map(|s| s.trim())
                        .map(|s| resolve_sheet_name(&all_sheets, Some(s)))
                        .collect()
                }
            } else {
                vec![resolve_sheet_name(&all_sheets, sheet_name)]
            };

            let input = std::path::Path::new(file_path);
            let stem = input.file_stem().unwrap_or_default();
            let parent = input.parent().unwrap_or(std::path::Path::new("."));

            let mut all_results = Vec::new();
            let is_multi = sheets_to_load.len() > 1;

            for sheet_name_str in &sheets_to_load {
                let range = workbook.worksheet_range(sheet_name_str).map_err(|e| {
                    ToolError::ExecutionFailed {
                        tool: TOOL_NAME.to_string(),
                        message: format!("Failed to read sheet '{}': {:?}", sheet_name_str, e),
                    }
                })?;

                let (height, width) = range.get_size();
                if height == 0 || width == 0 {
                    all_results.push(format!("Sheet '{}' is empty, skipped.", sheet_name_str));
                    continue;
                }

                // header_row_idx is the 0-based row index of the header.
                // All rows before it are skipped (title rows, metadata, etc.)
                let data_start_row = header_row_idx + 1;
                if data_start_row >= height {
                    all_results.push(format!("Sheet '{}': header_row={} but only {} rows, skipped.", sheet_name_str, header_row_idx, height));
                    continue;
                }
                let data_rows = height - data_start_row;
                let load_rows = max_rows.unwrap_or(data_rows).min(data_rows);

                // Extract headers from the header row
                let headers: Vec<String> = (0..width)
                    .map(|col| {
                        range
                            .get_value((header_row_idx as u32, col as u32))
                            .map(format_cell_value)
                            .unwrap_or_else(|| format!("Column_{}", col + 1))
                    })
                    .map(|h| {
                        let trimmed = h.trim().to_string();
                        if trimmed.is_empty() {
                            "unnamed".to_string()
                        } else {
                            trimmed
                        }
                    })
                    .collect();

                // Detect column types by scanning all data rows
                let mut col_is_numeric: Vec<bool> = vec![true; width];
                let mut col_has_float: Vec<bool> = vec![false; width];
                let mut col_is_bool: Vec<bool> = vec![true; width];
                let mut col_is_datetime: Vec<bool> = vec![true; width];
                let mut col_has_data: Vec<bool> = vec![false; width];

                for row_idx in data_start_row..(data_start_row + load_rows) {
                    for col in 0..width {
                        match range.get_value((row_idx as u32, col as u32)) {
                            Some(calamine::Data::Float(_)) => {
                                col_has_float[col] = true;
                                col_is_bool[col] = false;
                                col_is_datetime[col] = false;
                                col_has_data[col] = true;
                            }
                            Some(calamine::Data::Int(_)) => {
                                col_is_bool[col] = false;
                                col_is_datetime[col] = false;
                                col_has_data[col] = true;
                            }
                            Some(calamine::Data::Bool(_)) => {
                                col_is_numeric[col] = false;
                                col_is_datetime[col] = false;
                                col_has_data[col] = true;
                            }
                            Some(calamine::Data::DateTime(_)) | Some(calamine::Data::DateTimeIso(_)) => {
                                col_is_numeric[col] = false;
                                col_is_bool[col] = false;
                                col_has_data[col] = true;
                            }
                            Some(calamine::Data::DurationIso(_)) => {
                                col_is_numeric[col] = false;
                                col_is_bool[col] = false;
                                col_is_datetime[col] = false;
                                col_has_data[col] = true;
                            }
                            Some(calamine::Data::String(_)) => {
                                col_is_numeric[col] = false;
                                col_is_bool[col] = false;
                                col_is_datetime[col] = false;
                                col_has_data[col] = true;
                            }
                            Some(calamine::Data::Error(_)) => {
                                col_is_numeric[col] = false;
                                col_is_bool[col] = false;
                                col_is_datetime[col] = false;
                                col_has_data[col] = true;
                            }
                            Some(calamine::Data::Empty) | None => {}
                        }
                    }
                }

                // Build Polars Series for each column
                use polars::prelude::*;
                let mut series_list: Vec<Series> = Vec::with_capacity(width);

                for col in 0..width {
                    let col_name = PlSmallStr::from_str(&headers[col]);

                    if !col_has_data[col] {
                        let vals: Vec<Option<String>> = vec![None; load_rows];
                        series_list.push(Series::new(col_name, vals));
                    } else if col_is_bool[col] {
                        let vals: Vec<Option<bool>> = (data_start_row..(data_start_row + load_rows))
                            .map(|row_idx| match range.get_value((row_idx as u32, col as u32)) {
                                Some(calamine::Data::Bool(b)) => Some(*b),
                                _ => None,
                            })
                            .collect();
                        series_list.push(Series::new(col_name, vals));
                    } else if col_is_datetime[col] {
                        let vals: Vec<Option<String>> = (data_start_row..(data_start_row + load_rows))
                            .map(|row_idx| match range.get_value((row_idx as u32, col as u32)) {
                                Some(calamine::Data::Empty) | None => None,
                                Some(v) => {
                                    let s = format_cell_value(v);
                                    if s.is_empty() { None } else { Some(s) }
                                }
                            })
                            .collect();
                        series_list.push(Series::new(col_name, vals));
                    } else if col_is_numeric[col] {
                        if col_has_float[col] {
                            let vals: Vec<Option<f64>> = (data_start_row..(data_start_row + load_rows))
                                .map(|row_idx| match range.get_value((row_idx as u32, col as u32)) {
                                    Some(calamine::Data::Float(f)) => Some(*f),
                                    Some(calamine::Data::Int(i)) => Some(*i as f64),
                                    _ => None,
                                })
                                .collect();
                            series_list.push(Series::new(col_name, vals));
                        } else {
                            let vals: Vec<Option<i64>> = (data_start_row..(data_start_row + load_rows))
                                .map(|row_idx| match range.get_value((row_idx as u32, col as u32)) {
                                    Some(calamine::Data::Int(i)) => Some(*i),
                                    Some(calamine::Data::Float(f)) => Some(*f as i64),
                                    _ => None,
                                })
                                .collect();
                            series_list.push(Series::new(col_name, vals));
                        }
                    } else {
                        let vals: Vec<Option<String>> = (data_start_row..(data_start_row + load_rows))
                            .map(|row_idx| match range.get_value((row_idx as u32, col as u32)) {
                                Some(calamine::Data::Empty) | None => None,
                                Some(v) => {
                                    let s = format_cell_value(v);
                                    if s.is_empty() { None } else { Some(s) }
                                }
                            })
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
                        message: format!("Failed to create DataFrame for sheet '{}': {}", sheet_name_str, e),
                    })?;

                // Determine output path (with sheet suffix for multi-sheet)
                let out_path = if let Some(out) = output_file {
                    if is_multi {
                        let ext = std::path::Path::new(out)
                            .extension()
                            .and_then(|e| e.to_str())
                            .unwrap_or(format.as_str());
                        let stem_out = std::path::Path::new(out).file_stem().unwrap_or_default();
                        let parent_out = std::path::Path::new(out).parent().unwrap_or(std::path::Path::new("."));
                        parent_out.join(format!("{}_{}.{}", stem_out.to_string_lossy(), sheet_name_str, ext))
                    } else {
                        security.validate_output_file(out)?
                    }
                } else {
                    let ext = match format {
                        "csv" => ".csv",
                        _ => ".parquet",
                    };
                    let suffix = if is_multi {
                        format!("_{}", sheet_name_str)
                    } else {
                        String::new()
                    };
                    let out_name = format!("{}{}{}", stem.to_string_lossy(), suffix, ext);
                    parent.join(out_name)
                };

                // Create parent directory if needed
                if let Some(p) = out_path.parent() {
                    std::fs::create_dir_all(p).map_err(|e| ToolError::ExecutionFailed {
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

                // Build result summary for this sheet
                let shape = df.shape();
                let col_names: Vec<String> = df.get_column_names().iter().map(|s| s.to_string()).collect();
                let dtypes: Vec<String> = col_names.iter().map(|name| {
                    let dtype = df.column(name).map(|c| c.dtype().clone()).unwrap_or_default();
                    format!("{}: {:?}", name, dtype)
                }).collect();

                let mut sheet_result = Vec::new();
                if is_multi {
                    sheet_result.push(format!("[Sheet: {}]", sheet_name_str));
                }
                sheet_result.push(format!("  Loaded → {} ({})", out_path.display(), format));
                sheet_result.push(format!("  Shape: {} rows x {} columns", shape.0, shape.1));
                if header_row_idx > 0 {
                    sheet_result.push(format!("  Header row: {} (skipped {} rows above)", header_row_idx, header_row_idx));
                }
                sheet_result.push("  Columns:".to_string());
                for d in &dtypes {
                    sheet_result.push(format!("    {}", d));
                }
                all_results.push(sheet_result.join("\n"));
            }

            // Final summary
            let mut result = Vec::new();
            result.push(format!(
                "Loaded {} sheet(s) from '{}' → {}",
                sheets_to_load.len(),
                file_path,
                format
            ));
            result.push(String::new());
            result.push(all_results.join("\n\n"));
            result.push(String::new());
            result.push("Use these files with data tools: filter_data, aggregate_data, data_stats, transform_data, etc.".to_string());

            Ok(ToolResult::success(result.join("\n")))
        })
    }
}
