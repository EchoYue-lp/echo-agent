//! Data quality analysis tools
//!
//! Provides missing value analysis, outlier detection, and consistency checking
//! to help assess and improve data quality before analysis.
//!
//! Tools:
//! - `MissingValueAnalysisTool`: Analyze missing values per column with patterns and imputation suggestions
//! - `OutlierDetectionTool`: Detect outliers using IQR or Z-score methods
//! - `ConsistencyCheckTool`: Check data consistency (type mismatches, range validation, cross-column rules)

use polars::prelude::*;
use serde_json::Value;

use crate::data::{is_numeric, load_dataframe};
use crate::security::SecurityConfig;
use echo_core::error::{Result, ToolError};
use echo_core::tools::permission::ToolPermission;
use echo_core::tools::{Tool, ToolParameters, ToolResult};

// ── MissingValueAnalysisTool ─────────────────────────────────────────

pub struct MissingValueAnalysisTool;

impl Tool for MissingValueAnalysisTool {
    fn name(&self) -> &str {
        "missing_value_analysis"
    }
    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Read]
    }
    fn description(&self) -> &str {
        "Analyze missing values in a dataset: counts per column, percentage, pattern classification (all_missing, random_missing, monotonic), and imputation suggestions based on column type and missing rate."
    }
    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": { "data_path": { "type": "string", "description": "Absolute path to the data file" } },
            "required": ["data_path"]
        })
    }
    fn execute(
        &self,
        parameters: ToolParameters,
    ) -> futures::future::BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let data_path = parameters
                .get("data_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("data_path".to_string()))?;
            let security = SecurityConfig::global();
            let path = security.validate_file(data_path)?;
            let df = load_dataframe(&path, None)?;
            let row_count = df.height();
            let mut columns_json = Vec::new();
            let mut total_missing = 0usize;
            for col_name in df.get_column_names() {
                let col = df
                    .column(col_name)
                    .map_err(|e| ToolError::ExecutionFailed {
                        tool: "missing_value_analysis".to_string(),
                        message: format!("Column '{}' error: {}", col_name, e),
                    })?;
                let null_count = col.null_count();
                let total = col.len();
                let missing_pct = if total > 0 {
                    (null_count as f64 / total as f64) * 100.0
                } else {
                    0.0
                };
                total_missing += null_count;
                let pattern = classify_missing_pattern(col, row_count);
                let suggestion = suggest_imputation(col.dtype(), missing_pct, pattern);
                columns_json.push(serde_json::json!({
                    "name": col_name, "dtype": col.dtype().to_string(),
                    "total": total, "non_null": total - null_count,
                    "missing_count": null_count, "missing_pct": (missing_pct * 100.0).round() / 100.0,
                    "pattern": pattern, "suggestion": suggestion,
                }));
            }
            let overall_pct = if row_count * df.width() > 0 {
                (total_missing as f64 / (row_count * df.width()) as f64) * 100.0
            } else {
                0.0
            };
            Ok(ToolResult::success_json(serde_json::json!({
                "file": data_path, "rows": row_count, "columns": df.width(),
                "total_missing_cells": total_missing, "overall_missing_pct": (overall_pct * 100.0).round() / 100.0,
                "per_column": columns_json,
            })))
        })
    }
}

fn classify_missing_pattern(col: &Column, row_count: usize) -> &'static str {
    let null_count = col.null_count();
    if null_count == 0 {
        return "no_missing";
    }
    if null_count == row_count {
        return "all_missing";
    }
    let series = col.as_materialized_series();
    let is_null_seq: Vec<bool> = series.iter().map(|v| v.is_null()).collect();
    let transitions = is_null_seq.windows(2).filter(|w| w[0] != w[1]).count();
    if transitions <= 2 {
        "monotonic_missing"
    } else if transitions as f64 / (null_count as f64).max(1.0) > 0.5 {
        "random_missing"
    } else {
        "scattered_missing"
    }
}

fn suggest_imputation(dtype: &DataType, missing_pct: f64, pattern: &str) -> String {
    if pattern == "no_missing" {
        return "No imputation needed".to_string();
    }
    if pattern == "all_missing" || missing_pct > 80.0 {
        return "Consider dropping column (>80% missing)".to_string();
    }
    if is_numeric(dtype) {
        if missing_pct < 10.0 {
            "Mean/median imputation (low missing rate)".to_string()
        } else if missing_pct < 30.0 {
            "Median or interpolation imputation".to_string()
        } else {
            "Consider model-based imputation (high missing rate)".to_string()
        }
    } else if matches!(dtype, DataType::String | DataType::Categorical(_, _)) {
        if missing_pct < 10.0 {
            "Mode imputation or 'Unknown' category".to_string()
        } else {
            "Add 'Missing' category indicator".to_string()
        }
    } else if matches!(dtype, DataType::Datetime(_, _) | DataType::Date) {
        "Forward/backward fill or interpolation".to_string()
    } else {
        "Domain-specific imputation recommended".to_string()
    }
}

// ── OutlierDetectionTool ─────────────────────────────────────────────

pub struct OutlierDetectionTool;

impl Tool for OutlierDetectionTool {
    fn name(&self) -> &str {
        "outlier_detection"
    }
    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Read]
    }
    fn description(&self) -> &str {
        "Detect outliers in numeric columns using IQR method (values beyond Q1 - k*IQR or Q3 + k*IQR) or Z-score method (values with |z| > threshold). Default threshold: 1.5 for IQR, 3.0 for Z-score."
    }
    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "data_path": { "type": "string", "description": "Absolute path to the data file" },
                "columns": { "type": "string", "description": "Column names to check, comma-separated (default: all numeric columns)" },
                "method": { "type": "string", "enum": ["iqr", "zscore"], "description": "Detection method: 'iqr' (default) or 'zscore'" },
                "threshold": { "type": "number", "description": "Threshold: 1.5 for IQR (default), 3.0 for Z-score (default)" }
            },
            "required": ["data_path"]
        })
    }
    fn execute(
        &self,
        parameters: ToolParameters,
    ) -> futures::future::BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let data_path = parameters
                .get("data_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("data_path".to_string()))?;
            let method = parameters
                .get("method")
                .and_then(|v| v.as_str())
                .unwrap_or("iqr");
            let default_threshold = if method == "iqr" { 1.5 } else { 3.0 };
            let threshold = parameters
                .get("threshold")
                .and_then(|v| v.as_f64())
                .unwrap_or(default_threshold);
            let security = SecurityConfig::global();
            let path = security.validate_file(data_path)?;
            let df = load_dataframe(&path, None)?;
            let target_cols: Vec<String> =
                if let Some(cols_str) = parameters.get("columns").and_then(|v| v.as_str()) {
                    cols_str.split(',').map(|s| s.trim().to_string()).collect()
                } else {
                    df.get_column_names()
                        .into_iter()
                        .filter(|name| {
                            df.column(*name)
                                .map(|c| is_numeric(c.dtype()))
                                .unwrap_or(false)
                        })
                        .map(|s| s.to_string())
                        .collect()
                };
            let mut columns_json = Vec::new();
            for col_name in &target_cols {
                let col = df
                    .column(col_name.as_str())
                    .map_err(|e| ToolError::ExecutionFailed {
                        tool: "outlier_detection".to_string(),
                        message: format!("Column '{}' error: {}", col_name, e),
                    })?;
                if !is_numeric(col.dtype()) {
                    columns_json.push(
                        serde_json::json!({ "name": col_name, "error": "Non-numeric column" }),
                    );
                    continue;
                }
                let casted =
                    col.cast(&DataType::Float64)
                        .map_err(|e| ToolError::ExecutionFailed {
                            tool: "outlier_detection".to_string(),
                            message: format!("Cast '{}' failed: {}", col_name, e),
                        })?;
                let values: Vec<f64> = casted
                    .f64()
                    .map_err(|e| ToolError::ExecutionFailed {
                        tool: "outlier_detection".to_string(),
                        message: format!("Convert '{}' to f64 failed: {}", col_name, e),
                    })?
                    .into_iter()
                    .filter_map(|opt| opt)
                    .collect();
                if values.len() < 4 {
                    columns_json.push(serde_json::json!({ "name": col_name, "n_valid": values.len(), "error": "Need at least 4 values" }));
                    continue;
                }
                columns_json.push(if method == "iqr" {
                    detect_iqr_outliers(&values, threshold, col_name)
                } else {
                    detect_zscore_outliers(&values, threshold, col_name)
                });
            }
            Ok(ToolResult::success_json(
                serde_json::json!({ "file": data_path, "method": method, "threshold": threshold, "columns": columns_json }),
            ))
        })
    }
}

fn detect_iqr_outliers(values: &[f64], k: f64, col_name: &str) -> serde_json::Value {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = sorted.len();
    let q1 = sorted[n / 4.min(n - 1)];
    let q3 = sorted[3 * n / 4.min(n - 1)];
    let iqr = q3 - q1;
    let lower = q1 - k * iqr;
    let upper = q3 + k * iqr;
    let outliers: Vec<f64> = values
        .iter()
        .filter(|&&v| v < lower || v > upper)
        .cloned()
        .collect();
    let pct = if n > 0 {
        (outliers.len() as f64 / n as f64) * 100.0
    } else {
        0.0
    };
    serde_json::json!({
        "name": col_name, "n_valid": n, "q1": q1, "q3": q3, "iqr": iqr,
        "lower_bound": lower, "upper_bound": upper,
        "outlier_count": outliers.len(), "outlier_pct": (pct * 100.0).round() / 100.0,
        "outlier_samples": outliers.iter().take(10).cloned().collect::<Vec<f64>>(),
    })
}

fn detect_zscore_outliers(values: &[f64], threshold: f64, col_name: &str) -> serde_json::Value {
    let n = values.len();
    let mean = values.iter().sum::<f64>() / n as f64;
    let std_dev =
        (values.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n as f64 - 1.0)).sqrt();
    if std_dev == 0.0 {
        return serde_json::json!({ "name": col_name, "n_valid": n, "mean": mean, "std_dev": 0.0, "outlier_count": 0, "note": "Zero variance — no outliers possible" });
    }
    let z_scores: Vec<f64> = values.iter().map(|x| (x - mean) / std_dev).collect();
    let outliers: Vec<f64> = values
        .iter()
        .zip(z_scores.iter())
        .filter(|(_, z)| z.abs() > threshold)
        .map(|(v, _)| *v)
        .collect();
    let pct = if n > 0 {
        (outliers.len() as f64 / n as f64) * 100.0
    } else {
        0.0
    };
    serde_json::json!({
        "name": col_name, "n_valid": n, "mean": mean, "std_dev": std_dev, "threshold": threshold,
        "outlier_count": outliers.len(), "outlier_pct": (pct * 100.0).round() / 100.0,
        "outlier_samples": outliers.iter().take(10).cloned().collect::<Vec<f64>>(),
        "max_z_score": z_scores.iter().map(|z| z.abs()).fold(0.0, f64::max),
    })
}

// ── ConsistencyCheckTool ─────────────────────────────────────────────

pub struct ConsistencyCheckTool;

impl Tool for ConsistencyCheckTool {
    fn name(&self) -> &str {
        "consistency_check"
    }
    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Read]
    }
    fn description(&self) -> &str {
        "Check data consistency: type mismatches in string columns, out-of-range values in numeric columns, and cross-column validation. Custom rules can be provided as JSON."
    }
    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "data_path": { "type": "string", "description": "Absolute path to the data file" },
                "rules": { "type": "string", "description": "Custom validation rules as JSON: [{\"column\":\"age\",\"type\":\"range\",\"min\":0,\"max\":120}]" }
            },
            "required": ["data_path"]
        })
    }
    fn execute(
        &self,
        parameters: ToolParameters,
    ) -> futures::future::BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let data_path = parameters
                .get("data_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("data_path".to_string()))?;
            let security = SecurityConfig::global();
            let path = security.validate_file(data_path)?;
            let df = load_dataframe(&path, None)?;
            let mut issues = Vec::new();
            // ── Automatic type mismatch detection ──
            for col_name in df.get_column_names() {
                let col = df
                    .column(col_name)
                    .map_err(|e| ToolError::ExecutionFailed {
                        tool: "consistency_check".to_string(),
                        message: format!("Column '{}' error: {}", col_name, e),
                    })?;
                let dtype = col.dtype();
                if matches!(dtype, DataType::String) {
                    let ca = col.str().map_err(|e| ToolError::ExecutionFailed {
                        tool: "consistency_check".to_string(),
                        message: format!("Cast '{}' to string failed: {}", col_name, e),
                    })?;
                    let mut numeric_count = 0usize;
                    let mut empty_count = 0usize;
                    let mut total_valid = 0usize;
                    for opt in ca.into_iter() {
                        if let Some(s) = opt {
                            total_valid += 1;
                            if s.trim().parse::<f64>().is_ok() {
                                numeric_count += 1;
                            }
                            if s.trim().is_empty() {
                                empty_count += 1;
                            }
                        }
                    }
                    if total_valid > 0 && numeric_count as f64 / total_valid as f64 > 0.8 {
                        issues.push(serde_json::json!({ "column": col_name, "type": "type_mismatch", "detail": format!("String column contains {:.0}% numeric values", numeric_count as f64 / total_valid as f64 * 100.0), "severity": "medium" }));
                    }
                    if empty_count > 0 {
                        issues.push(serde_json::json!({ "column": col_name, "type": "empty_strings", "detail": format!("Found {} empty strings that should be null", empty_count), "severity": "low" }));
                    }
                }
                if is_numeric(dtype) {
                    let casted =
                        col.cast(&DataType::Float64)
                            .map_err(|e| ToolError::ExecutionFailed {
                                tool: "consistency_check".to_string(),
                                message: format!("Cast '{}' to Float64 failed: {}", col_name, e),
                            })?;
                    let values: Vec<f64> = casted
                        .f64()
                        .map_err(|e| ToolError::ExecutionFailed {
                            tool: "consistency_check".to_string(),
                            message: format!("Convert '{}' to f64 failed: {}", col_name, e),
                        })?
                        .into_iter()
                        .filter_map(|opt| opt)
                        .collect();
                    let negatives = values.iter().filter(|&&v| v < 0.0).count();
                    if negatives > 0 && negatives < values.len() / 10 {
                        issues.push(serde_json::json!({ "column": col_name, "type": "negative_values", "detail": format!("Found {} negative values", negatives), "severity": "medium" }));
                    }
                    if values.len() >= 10 {
                        let mean = values.iter().sum::<f64>() / values.len() as f64;
                        let std_dev = (values.iter().map(|x| (x - mean).powi(2)).sum::<f64>()
                            / (values.len() as f64 - 1.0))
                            .sqrt();
                        if std_dev > 0.0 {
                            let extreme = values
                                .iter()
                                .filter(|&&v| ((v - mean) / std_dev).abs() > 5.0)
                                .count();
                            if extreme > 0 {
                                issues.push(serde_json::json!({ "column": col_name, "type": "extreme_values", "detail": format!("Found {} values beyond 5σ", extreme), "severity": "high" }));
                            }
                        }
                    }
                }
            }
            // ── Custom rule validation ──
            if let Some(rules_str) = parameters.get("rules").and_then(|v| v.as_str()) {
                let rules: Vec<serde_json::Value> =
                    serde_json::from_str(rules_str).map_err(|e| ToolError::InvalidParameter {
                        name: "rules".into(),
                        message: format!("Invalid rules JSON: {}", e),
                    })?;
                for rule in &rules {
                    let col_name =
                        rule.get("column").and_then(|v| v.as_str()).ok_or_else(|| {
                            ToolError::InvalidParameter {
                                name: "rules".into(),
                                message: "Each rule must have a 'column' field".into(),
                            }
                        })?;
                    let rule_type = rule.get("type").and_then(|v| v.as_str()).unwrap_or("range");
                    let col = df
                        .column(col_name)
                        .map_err(|e| ToolError::ExecutionFailed {
                            tool: "data_quality_tools".into(),
                            message: format!("Column '{}' not found: {}", col_name, e),
                        })?;
                    match rule_type {
                        "range" => {
                            let casted = col.cast(&DataType::Float64).map_err(|e| {
                                ToolError::ExecutionFailed {
                                    tool: "consistency_check".to_string(),
                                    message: format!("Cast '{}' failed: {}", col_name, e),
                                }
                            })?;
                            let values: Vec<Option<f64>> = casted
                                .f64()
                                .map_err(|e| ToolError::ExecutionFailed {
                                    tool: "consistency_check".to_string(),
                                    message: format!("Convert '{}' failed: {}", col_name, e),
                                })?
                                .into_iter()
                                .collect();
                            let min = rule.get("min").and_then(|v| v.as_f64());
                            let max = rule.get("max").and_then(|v| v.as_f64());
                            let violations = values
                                .iter()
                                .filter(|opt| {
                                    if let Some(v) = opt {
                                        (min.is_some_and(|m| *v < m))
                                            || (max.is_some_and(|mx| *v > mx))
                                    } else {
                                        false
                                    }
                                })
                                .count();
                            if violations > 0 {
                                issues.push(serde_json::json!({ "column": col_name, "type": "range_violation", "detail": format!("{} values outside range", violations), "severity": "high", "rule": rule }));
                            }
                        }
                        "regex" => {
                            let pattern =
                                rule.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
                            let col_str = col.cast(&DataType::String).map_err(|e| {
                                ToolError::ExecutionFailed {
                                    tool: "consistency_check".to_string(),
                                    message: format!("Cast '{}' to string failed: {}", col_name, e),
                                }
                            })?;
                            let ca = col_str.str().map_err(|e| ToolError::ExecutionFailed {
                                tool: "consistency_check".to_string(),
                                message: format!("Get string column '{}' failed: {}", col_name, e),
                            })?;
                            let violations = ca
                                .into_iter()
                                .filter(|opt| {
                                    if let Some(s) = opt {
                                        !s.contains(pattern)
                                    } else {
                                        false
                                    }
                                })
                                .count();
                            if violations > 0 {
                                issues.push(serde_json::json!({ "column": col_name, "type": "regex_violation", "detail": format!("{} values don't match pattern '{}'", violations, pattern), "severity": "medium", "rule": rule }));
                            }
                        }
                        other => {
                            issues.push(serde_json::json!({ "column": col_name, "type": "unknown_rule", "detail": format!("Unknown rule type '{}' (supported: range, regex)", other), "severity": "low" }));
                        }
                    }
                }
            }
            let severity_counts = {
                let mut counts = serde_json::Map::new();
                for level in &["high", "medium", "low"] {
                    counts.insert(
                        level.to_string(),
                        serde_json::json!(
                            issues
                                .iter()
                                .filter(
                                    |i| i.get("severity").and_then(|v| v.as_str()) == Some(level)
                                )
                                .count()
                        ),
                    );
                }
                serde_json::Value::Object(counts)
            };
            Ok(ToolResult::success_json(
                serde_json::json!({ "file": data_path, "total_issues": issues.len(), "severity_counts": severity_counts, "issues": issues }),
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::tools::Tool;
    use std::io::Write;

    fn create_temp_csv(content: &str) -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join("echo_tools_dq_tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("test_{}_{}.csv", std::process::id(), id));
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        path.to_string_lossy().to_string()
    }

    fn cleanup(path: &str) {
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn test_missing_value_analysis() {
        let path = create_temp_csv("a,b,c\n1,x,foo\n,y,bar\n3,,\n");

        let tool = super::MissingValueAnalysisTool;
        let mut params = std::collections::HashMap::new();
        params.insert("data_path".to_string(), serde_json::json!(path));

        let result = tool.execute(params).await.unwrap();
        assert!(result.success);
        let output: Value = serde_json::from_str(&result.output).unwrap();
        assert!(output["total_missing_cells"].as_u64().unwrap() > 0);
        assert!(output["per_column"].is_array());

        cleanup(&path);
    }

    #[tokio::test]
    async fn test_outlier_detection_iqr() {
        // Create data with a clear outlier
        let path = create_temp_csv("value\n1\n2\n3\n2\n1\n100\n3\n2\n1\n");

        let tool = super::OutlierDetectionTool;
        let mut params = std::collections::HashMap::new();
        params.insert("data_path".to_string(), serde_json::json!(path));
        params.insert("method".to_string(), serde_json::json!("iqr"));

        let result = tool.execute(params).await.unwrap();
        assert!(result.success);
        let output: Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(output["method"], "iqr");
        assert!(output["columns"].is_array());

        cleanup(&path);
    }

    #[tokio::test]
    async fn test_consistency_check_range() {
        let path = create_temp_csv("age\n25\n30\n-5\n40\n200\n");

        let tool = super::ConsistencyCheckTool;
        let mut params = std::collections::HashMap::new();
        params.insert("data_path".to_string(), serde_json::json!(path));
        params.insert(
            "rules".to_string(),
            serde_json::json!(r#"[{"column": "age", "type": "range", "min": 0, "max": 150}]"#),
        );

        let result = tool.execute(params).await.unwrap();
        assert!(result.success);
        let output: Value = serde_json::from_str(&result.output).unwrap();
        assert!(output["total_issues"].as_u64().unwrap() > 0);

        cleanup(&path);
    }
}
