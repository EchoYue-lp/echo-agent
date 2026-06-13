//! Statistical analysis tools
//!
//! Provides hypothesis testing, linear regression, and advanced descriptive
//! statistics using Polars for data loading and computation.
//!
//! Tools:
//! - `HypothesisTestTool`: t-test, chi-square, correlation significance
//! - `RegressionTool`: linear regression with coefficients, R², p-values
//! - `DescriptiveAdvancedTool`: skewness, kurtosis, confidence intervals

use polars::prelude::*;

use crate::data::{is_numeric, load_dataframe};
use crate::security::SecurityConfig;
use echo_core::error::{Result, ToolError};
use echo_core::tools::{ToolResult, ToolRunner};

const TOOL_NAME: &str = "statistics_tools";

// ── Statistical helper functions ─────────────────────────────────────

/// √(2π) — defined locally for toolchains that lack `std::f64::consts::SQRT_2PI`.
const SQRT_2PI: f64 = 2.506_628_274_631_000_2;

/// Normal CDF approximation (Abramowitz & Stegun, formula 26.2.19).
/// Accurate to ~1.5e-7 for |x| <= 5.
fn normal_cdf(x: f64) -> f64 {
    if x < 0.0 {
        return 1.0 - normal_cdf(-x);
    }
    let p = 0.332906;
    let a1 = 0.93629;
    let a2 = -0.00976;
    let a3 = 0.00746;
    let t = 1.0 / (1.0 + p * x);
    let z = (1.0 / SQRT_2PI) * (-0.5 * x * x).exp();
    1.0 - z * t * (a1 + t * (a2 + t * a3))
}

/// Approximate p-value for chi-square statistic using Wilson-Hilferty approximation.
fn chi_square_p_value(chi2: f64, df: usize) -> f64 {
    if df == 0 || chi2 <= 0.0 {
        return 1.0;
    }
    let df_f = df as f64;
    let ratio = chi2 / df_f;
    let cbrt = ratio.powf(1.0 / 3.0);
    let mean = 1.0 - 2.0 / (9.0 * df_f);
    let sd = (2.0 / (9.0 * df_f)).sqrt();
    if sd == 0.0 {
        return if chi2 > 0.0 { 0.0 } else { 1.0 };
    }
    let z = (cbrt - mean) / sd;
    1.0 - normal_cdf(z)
}

/// Compute mean and standard deviation of a numeric column.
fn column_mean_std(df: &DataFrame, col_name: &str) -> Result<(f64, f64, usize)> {
    let col = df
        .column(col_name)
        .map_err(|e| ToolError::ExecutionFailed {
            tool: TOOL_NAME.to_string(),
            message: format!("Column '{}' not found: {}", col_name, e),
        })?;
    if !is_numeric(col.dtype()) {
        return Err(ToolError::InvalidParameter {
            name: col_name.to_string(),
            message: format!(
                "Column '{}' is not numeric (type: {})",
                col_name,
                col.dtype()
            ),
        }
        .into());
    }
    let series = col.as_materialized_series();
    let n_valid = series.len() - series.null_count();
    Ok((
        series.mean().unwrap_or(0.0),
        series.std(0).unwrap_or(0.0),
        n_valid,
    ))
}

/// Approximate inverse normal CDF (Beasley-Springer-Moro algorithm).
fn inverse_normal_cdf(p: f64) -> f64 {
    if p <= 0.0 {
        return f64::NEG_INFINITY;
    }
    if p >= 1.0 {
        return f64::INFINITY;
    }
    if p == 0.5 {
        return 0.0;
    }
    let a: [f64; 4] = [
        2.50662823884,
        -18.61500062529,
        41.39119773534,
        -25.44106049637,
    ];
    let b: [f64; 4] = [
        -8.47351093090,
        23.08336743743,
        -21.06224101826,
        3.13082909312,
    ];
    let r = if p < 0.5 { p } else { 1.0 - p };
    let t = -2.0 * r.ln();
    let numerator = a[0] + t * (a[1] + t * (a[2] + t * a[3]));
    let denominator = 1.0 + t * (b[0] + t * (b[1] + t * (b[2] + t * b[3])));
    let z = numerator / denominator;
    if p < 0.5 { z } else { -z }
}

// ── HypothesisTestTool ───────────────────────────────────────────────

#[derive(Default, echo_macros::Tool)]
#[tool(
    name = "hypothesis_test",
    description = "Perform statistical hypothesis tests: t-test (compare means of two numeric columns), chi-square test of independence (two categorical columns), or correlation significance test (two numeric columns). Returns test statistic, p-value, degrees of freedom, and conclusion."
)]
#[allow(dead_code)]
pub struct HypothesisTestTool {
    #[tool_param(description = "Absolute path to the data file (CSV, JSON, Parquet)")]
    data_path: String,
    #[tool_param(description = "Test type: 't_test', 'chi_square', or 'correlation_significance'")]
    test_type: String,
    #[tool_param(description = "First column name")]
    column1: String,
    #[tool_param(
        description = "Second column name (required for chi_square and correlation_significance)"
    )]
    column2: Option<String>,
    #[tool_param(description = "Significance level alpha (default 0.05)")]
    alpha: Option<f64>,
}

impl ToolRunner<HypothesisTestToolParams> for HypothesisTestTool {
    async fn run(&self, params: HypothesisTestToolParams) -> Result<ToolResult> {
        let alpha = params.alpha.unwrap_or(0.05);
        let security = SecurityConfig::global();
        let path = security.validate_file(&params.data_path)?;
        let df = load_dataframe(&path, None)?;

        match params.test_type.as_str() {
            "t_test" => t_test(&df, &params.column1, params.column2.as_deref(), alpha),
            "chi_square" => chi_square_test(&df, &params.column1, params.column2.as_deref(), alpha),
            "correlation_significance" => {
                correlation_significance(&df, &params.column1, params.column2.as_deref(), alpha)
            }
            other => Err(ToolError::InvalidParameter {
                name: "test_type".to_string(),
                message: format!(
                    "Unsupported test type '{}'. Use: t_test, chi_square, correlation_significance",
                    other
                ),
            }
            .into()),
        }
    }
}

fn t_test(df: &DataFrame, col1: &str, col2: Option<&str>, alpha: f64) -> Result<ToolResult> {
    let col2_name = col2.unwrap_or(col1);
    let (mean1, std1, n1) = column_mean_std(df, col1)?;
    let (mean2, std2, n2) = column_mean_std(df, col2_name)?;
    if n1 < 2 || n2 < 2 {
        return Ok(ToolResult::success(
            "Each column needs at least 2 non-null values for a t-test",
        ));
    }
    let se1 = (std1 * std1) / (n1 as f64);
    let se2 = (std2 * std2) / (n2 as f64);
    let t_stat = (mean1 - mean2) / (se1 + se2).sqrt();
    let numerator = (se1 + se2).powi(2);
    let denominator = (se1.powi(2) / (n1 as f64 - 1.0)) + (se2.powi(2) / (n2 as f64 - 1.0));
    let df_val = if denominator > 0.0 {
        numerator / denominator
    } else {
        n1 as f64 + n2 as f64 - 2.0
    };
    let p_value = 2.0 * (1.0 - normal_cdf(t_stat.abs()));
    let significant = p_value < alpha;
    let conclusion = if significant {
        format!(
            "Reject null hypothesis: means are significantly different (p={:.4} < α={:.2})",
            p_value, alpha
        )
    } else {
        format!(
            "Fail to reject null: no significant difference in means (p={:.4} ≥ α={:.2})",
            p_value, alpha
        )
    };
    Ok(ToolResult::success_json(serde_json::json!({
        "test": "Welch's t-test",
        "column1": { "name": col1, "mean": mean1, "std": std1, "n": n1 },
        "column2": { "name": col2_name, "mean": mean2, "std": std2, "n": n2 },
        "t_statistic": t_stat, "degrees_of_freedom": df_val,
        "p_value": p_value, "alpha": alpha,
        "significant": significant, "conclusion": conclusion,
    })))
}

fn chi_square_test(
    df: &DataFrame,
    col1: &str,
    col2: Option<&str>,
    alpha: f64,
) -> Result<ToolResult> {
    let col2_name = col2.ok_or_else(|| ToolError::MissingParameter("column2".to_string()))?;
    let c1 = df.column(col1).map_err(|e| ToolError::ExecutionFailed {
        tool: TOOL_NAME.to_string(),
        message: format!("Column '{}' not found: {}", col1, e),
    })?;
    let c2 = df
        .column(col2_name)
        .map_err(|e| ToolError::ExecutionFailed {
            tool: TOOL_NAME.to_string(),
            message: format!("Column '{}' not found: {}", col2_name, e),
        })?;
    let c1_str = c1
        .cast(&DataType::String)
        .map_err(|e| ToolError::ExecutionFailed {
            tool: TOOL_NAME.to_string(),
            message: format!("Cast '{}' to string failed: {}", col1, e),
        })?;
    let c2_str = c2
        .cast(&DataType::String)
        .map_err(|e| ToolError::ExecutionFailed {
            tool: TOOL_NAME.to_string(),
            message: format!("Cast '{}' to string failed: {}", col2_name, e),
        })?;
    let unique1: Vec<String> = c1_str
        .unique()
        .map_err(|e| ToolError::ExecutionFailed {
            tool: TOOL_NAME.to_string(),
            message: format!("Unique '{}' failed: {}", col1, e),
        })?
        .str()
        .map_err(|e| ToolError::ExecutionFailed {
            tool: TOOL_NAME.to_string(),
            message: format!("Cast '{}' to str failed: {}", col1, e),
        })?
        .into_iter()
        .filter_map(|opt| opt.map(|s| s.to_string()))
        .collect();
    let unique2: Vec<String> = c2_str
        .unique()
        .map_err(|e| ToolError::ExecutionFailed {
            tool: TOOL_NAME.to_string(),
            message: format!("Unique '{}' failed: {}", col2_name, e),
        })?
        .str()
        .map_err(|e| ToolError::ExecutionFailed {
            tool: TOOL_NAME.to_string(),
            message: format!("Cast '{}' to str failed: {}", col2_name, e),
        })?
        .into_iter()
        .filter_map(|opt| opt.map(|s| s.to_string()))
        .collect();
    let r = unique1.len();
    let c = unique2.len();
    if r < 2 || c < 2 {
        return Ok(ToolResult::success(
            "Each column needs at least 2 unique values for chi-square test",
        ));
    }
    let s1: Vec<&str> = c1_str
        .str()
        .map_err(|e| ToolError::ExecutionFailed {
            tool: TOOL_NAME.to_string(),
            message: format!("Get str '{}' failed: {}", col1, e),
        })?
        .into_iter()
        .map(|opt| opt.unwrap_or(""))
        .collect();
    let s2: Vec<&str> = c2_str
        .str()
        .map_err(|e| ToolError::ExecutionFailed {
            tool: TOOL_NAME.to_string(),
            message: format!("Get str '{}' failed: {}", col2_name, e),
        })?
        .into_iter()
        .map(|opt| opt.unwrap_or(""))
        .collect();
    let mut observed: Vec<Vec<usize>> = vec![vec![0; c]; r];
    for i in 0..df.height() {
        if let (Some(idx1), Some(idx2)) = (
            unique1.iter().position(|x| x == s1[i]),
            unique2.iter().position(|x| x == s2[i]),
        ) {
            observed[idx1][idx2] += 1;
        }
    }
    let row_totals: Vec<usize> = observed.iter().map(|row| row.iter().sum()).collect();
    let col_totals: Vec<usize> = (0..c)
        .map(|j| observed.iter().map(|row| row[j]).sum())
        .collect();
    let grand_total: usize = row_totals.iter().sum();
    let mut chi2_stat = 0.0;
    let mut expected: Vec<Vec<f64>> = vec![vec![0.0; c]; r];
    for i in 0..r {
        for j in 0..c {
            let exp = (row_totals[i] as f64 * col_totals[j] as f64) / (grand_total as f64);
            expected[i][j] = exp;
            if exp > 0.0 {
                chi2_stat += ((observed[i][j] as f64 - exp).powi(2)) / exp;
            }
        }
    }
    let dof = (r - 1) * (c - 1);
    let p_value = chi_square_p_value(chi2_stat, dof);
    let significant = p_value < alpha;
    let conclusion = if significant {
        format!(
            "Reject null: columns are dependent (p={:.4} < α={:.2})",
            p_value, alpha
        )
    } else {
        format!(
            "Fail to reject null: columns appear independent (p={:.4} ≥ α={:.2})",
            p_value, alpha
        )
    };
    Ok(ToolResult::success_json(serde_json::json!({
        "test": "Chi-square test of independence",
        "column1": col1, "column2": col2_name,
        "observed": observed, "expected": expected,
        "chi2_statistic": chi2_stat, "degrees_of_freedom": dof,
        "p_value": p_value, "alpha": alpha,
        "significant": significant, "conclusion": conclusion,
    })))
}

fn correlation_significance(
    df: &DataFrame,
    col1: &str,
    col2: Option<&str>,
    alpha: f64,
) -> Result<ToolResult> {
    let col2_name = col2.ok_or_else(|| ToolError::MissingParameter("column2".to_string()))?;
    let s1 = df
        .column(col1)
        .map_err(|e| ToolError::ExecutionFailed {
            tool: TOOL_NAME.to_string(),
            message: format!("Column '{}' not found: {}", col1, e),
        })?
        .cast(&DataType::Float64)
        .map_err(|e| ToolError::ExecutionFailed {
            tool: TOOL_NAME.to_string(),
            message: format!("Cast '{}' failed: {}", col1, e),
        })?;
    let s2 = df
        .column(col2_name)
        .map_err(|e| ToolError::ExecutionFailed {
            tool: TOOL_NAME.to_string(),
            message: format!("Column '{}' not found: {}", col2_name, e),
        })?
        .cast(&DataType::Float64)
        .map_err(|e| ToolError::ExecutionFailed {
            tool: TOOL_NAME.to_string(),
            message: format!("Cast '{}' failed: {}", col2_name, e),
        })?;
    let v1: Vec<Option<f64>> = s1
        .f64()
        .map_err(|e| ToolError::ExecutionFailed {
            tool: TOOL_NAME.to_string(),
            message: format!("Convert '{}' to f64 failed: {}", col1, e),
        })?
        .into_iter()
        .collect();
    let v2: Vec<Option<f64>> = s2
        .f64()
        .map_err(|e| ToolError::ExecutionFailed {
            tool: TOOL_NAME.to_string(),
            message: format!("Convert '{}' to f64 failed: {}", col2_name, e),
        })?
        .into_iter()
        .collect();
    let n_valid: usize = v1
        .iter()
        .zip(v2.iter())
        .filter(|(a, b)| a.is_some() && b.is_some())
        .count();
    if n_valid < 3 {
        return Ok(ToolResult::success(
            "Need at least 3 valid pairs for correlation significance test",
        ));
    }
    let mean1 = v1.iter().filter_map(|x| *x).sum::<f64>() / n_valid as f64;
    let mean2 = v2.iter().filter_map(|x| *x).sum::<f64>() / n_valid as f64;
    let mut cov = 0.0;
    let mut var1 = 0.0;
    let mut var2 = 0.0;
    for i in 0..v1.len() {
        if let (Some(a), Some(b)) = (v1[i], v2[i]) {
            let d1 = a - mean1;
            let d2 = b - mean2;
            cov += d1 * d2;
            var1 += d1 * d1;
            var2 += d2 * d2;
        }
    }
    let r = if var1 > 0.0 && var2 > 0.0 {
        cov / (var1.sqrt() * var2.sqrt())
    } else {
        0.0
    };
    let denom = (1.0 - r * r).sqrt();
    let t_stat = if denom > 0.0 {
        r * ((n_valid as f64 - 2.0).sqrt()) / denom
    } else {
        if r.abs() >= 1.0 { f64::INFINITY } else { 0.0 }
    };
    let p_value = 2.0 * (1.0 - normal_cdf(t_stat.abs()));
    let significant = p_value < alpha;
    let conclusion = if significant {
        format!(
            "Reject null: correlation is significant (r={:.4}, p={:.4} < α={:.2})",
            r, p_value, alpha
        )
    } else {
        format!(
            "Fail to reject null: correlation not significant (r={:.4}, p={:.4} ≥ α={:.2})",
            r, p_value, alpha
        )
    };
    Ok(ToolResult::success_json(serde_json::json!({
        "test": "Correlation significance test",
        "column1": col1, "column2": col2_name,
        "correlation": r, "n_valid_pairs": n_valid,
        "t_statistic": t_stat, "p_value": p_value, "alpha": alpha,
        "significant": significant, "conclusion": conclusion,
    })))
}

// ── RegressionTool ───────────────────────────────────────────────────

#[derive(Default, echo_macros::Tool)]
#[tool(
    name = "regression",
    description = "Perform linear regression analysis. Computes regression coefficients, R², standard errors, and p-values for the relationship between target and feature columns. Supports multiple feature columns (comma-separated)."
)]
#[allow(dead_code)]
pub struct RegressionTool {
    #[tool_param(description = "Absolute path to the data file")]
    data_path: String,
    #[tool_param(description = "Target (dependent) column name")]
    target_column: String,
    #[tool_param(description = "Feature (independent) column names, comma-separated")]
    feature_columns: String,
    #[tool_param(description = "Optional path to save regression results as JSON")]
    output_path: Option<String>,
}

impl ToolRunner<RegressionToolParams> for RegressionTool {
    async fn run(&self, params: RegressionToolParams) -> Result<ToolResult> {
        let security = SecurityConfig::global();
        let path = security.validate_file(&params.data_path)?;
        let df = load_dataframe(&path, None)?;
        let features: Vec<&str> = params
            .feature_columns
            .split(',')
            .map(|s| s.trim())
            .collect();
        if features.is_empty() {
            return Err(ToolError::InvalidParameter {
                name: "feature_columns".into(),
                message: "At least one feature column is required".into(),
            }
            .into());
        }
        let target_col =
            df.column(&params.target_column)
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("Target column '{}' not found: {}", params.target_column, e),
                })?;
        if !is_numeric(target_col.dtype()) {
            return Err(ToolError::InvalidParameter {
                name: "target_column".into(),
                message: format!("Target column must be numeric, got {}", target_col.dtype()),
            }
            .into());
        }
        let target_f64 =
            target_col
                .cast(&DataType::Float64)
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("Cast target column failed: {}", e),
                })?;
        let y: Vec<Option<f64>> = target_f64
            .f64()
            .map_err(|e| ToolError::ExecutionFailed {
                tool: TOOL_NAME.to_string(),
                message: format!("Convert to f64 failed: {}", e),
            })?
            .into_iter()
            .collect();
        let y_mean: f64 =
            y.iter().filter_map(|x| *x).sum::<f64>() / y.iter().filter_map(|x| *x).count() as f64;
        let mut total_ss = 0.0;
        for y_val in y.iter() {
            if let Some(v) = y_val {
                total_ss += (v - y_mean).powi(2);
            }
        }
        let mut coefficients = Vec::new();
        let mut predicted: Vec<f64> = vec![y_mean; y.len()];

        for feat_name in &features {
            let feat_col = df
                .column(*feat_name)
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("Feature column '{}' not found: {}", feat_name, e),
                })?;
            if !is_numeric(feat_col.dtype()) {
                return Err(ToolError::InvalidParameter {
                    name: feat_name.to_string(),
                    message: format!("Feature column must be numeric, got {}", feat_col.dtype()),
                }
                .into());
            }
            let (mean_x, std_x, n) = column_mean_std(&df, *feat_name)?;
            if n < 3 || std_x == 0.0 {
                coefficients.push(serde_json::json!({ "feature": feat_name, "slope": 0.0, "intercept": y_mean, "r_squared": 0.0, "p_value": 1.0, "note": "Insufficient variance or sample size" }));
                continue;
            }
            let feat_f64 =
                feat_col
                    .cast(&DataType::Float64)
                    .map_err(|e| ToolError::ExecutionFailed {
                        tool: TOOL_NAME.to_string(),
                        message: format!("Cast feature column '{}' failed: {}", feat_name, e),
                    })?;
            let x: Vec<Option<f64>> = feat_f64
                .f64()
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("Convert feature '{}' to f64 failed: {}", feat_name, e),
                })?
                .into_iter()
                .collect();
            let mut cov_xy = 0.0;
            let mut var_x = 0.0;
            for i in 0..x.len() {
                if let (Some(xi), Some(yi)) = (x[i], y[i]) {
                    cov_xy += (xi - mean_x) * (yi - y_mean);
                    var_x += (xi - mean_x).powi(2);
                }
            }
            let slope = if var_x > 0.0 { cov_xy / var_x } else { 0.0 };
            let intercept = y_mean - slope * mean_x;
            let r = if var_x > 0.0 && total_ss > 0.0 {
                cov_xy / (var_x.sqrt() * total_ss.sqrt())
            } else {
                0.0
            };
            let r_sq = r * r;
            let mut residual_var = 0.0;
            let mut pair_count = 0usize;
            for i in 0..x.len() {
                if let (Some(xi), Some(yi)) = (x[i], y[i]) {
                    residual_var += (yi - (intercept + slope * xi)).powi(2);
                    pair_count += 1;
                }
            }
            let se_slope = if pair_count > 2 && var_x > 0.0 {
                (residual_var / (pair_count as f64 - 2.0) / var_x).sqrt()
            } else {
                0.0
            };
            let t_stat = if se_slope > 0.0 {
                slope / se_slope
            } else {
                0.0
            };
            let p_value = 2.0 * (1.0 - normal_cdf(t_stat.abs()));
            coefficients.push(serde_json::json!({ "feature": feat_name, "slope": slope, "intercept": intercept, "r_squared": r_sq, "standard_error": se_slope, "t_statistic": t_stat, "p_value": p_value }));
            for i in 0..x.len() {
                if let Some(xi) = x[i] {
                    predicted[i] += slope * (xi - mean_x);
                }
            }
        }
        let mut residual_ss = 0.0;
        let mut valid_count = 0usize;
        for i in 0..y.len() {
            if let Some(yi) = y[i] {
                residual_ss += (yi - predicted[i]).powi(2);
                valid_count += 1;
            }
        }
        let overall_r_squared = if total_ss > 0.0 {
            1.0 - residual_ss / total_ss
        } else {
            0.0
        };
        let result = serde_json::json!({ "target": params.target_column, "features": features, "n_valid": valid_count, "coefficients": coefficients, "overall_r_squared": overall_r_squared, "residual_sum_of_squares": residual_ss, "total_sum_of_squares": total_ss });
        if let Some(out_path) = &params.output_path {
            let out = security.validate_file(out_path)?;
            std::fs::write(
                &out,
                serde_json::to_string_pretty(&result).unwrap_or_default(),
            )
            .map_err(|e| ToolError::ExecutionFailed {
                tool: TOOL_NAME.to_string(),
                message: format!("Failed to write output: {}", e),
            })?;
        }
        Ok(ToolResult::success_json(result))
    }
}

// ── DescriptiveAdvancedTool ──────────────────────────────────────────

#[derive(Default, echo_macros::Tool)]
#[tool(
    name = "descriptive_advanced",
    description = "Compute advanced descriptive statistics: skewness, kurtosis, and confidence intervals for the mean. Provides distribution shape analysis beyond basic mean/std."
)]
#[allow(dead_code)]
pub struct DescriptiveAdvancedTool {
    #[tool_param(description = "Absolute path to the data file")]
    data_path: String,
    #[tool_param(
        description = "Column names to analyze, comma-separated (default: all numeric columns)"
    )]
    columns: Option<String>,
    #[tool_param(description = "Confidence level for CI (default 0.95)")]
    confidence_level: Option<f64>,
}

impl ToolRunner<DescriptiveAdvancedToolParams> for DescriptiveAdvancedTool {
    async fn run(&self, params: DescriptiveAdvancedToolParams) -> Result<ToolResult> {
        let confidence_level = params.confidence_level.unwrap_or(0.95);
        let security = SecurityConfig::global();
        let path = security.validate_file(&params.data_path)?;
        let df = load_dataframe(&path, None)?;
        let target_cols: Vec<String> = if let Some(cols_str) = &params.columns {
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
        if target_cols.is_empty() {
            return Ok(ToolResult::success(
                "No numeric columns found for advanced descriptive statistics",
            ));
        }
        let alpha = 1.0 - confidence_level;
        let z_value = inverse_normal_cdf(1.0 - alpha / 2.0);
        let mut columns_json = Vec::new();
        for col_name in &target_cols {
            let col = df
                .column(col_name.as_str())
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("Column '{}' not found: {}", col_name, e),
                })?;
            if !is_numeric(col.dtype()) {
                columns_json
                    .push(serde_json::json!({ "name": col_name, "error": "Non-numeric column" }));
                continue;
            }
            let casted = col
                .cast(&DataType::Float64)
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("Cast column '{}' failed: {}", col_name, e),
                })?;
            let values: Vec<f64> = casted
                .f64()
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("Convert '{}' to f64 failed: {}", col_name, e),
                })?
                .into_iter()
                .filter_map(|opt| opt)
                .collect();
            let n = values.len();
            if n < 3 {
                columns_json.push(serde_json::json!({ "name": col_name, "n_valid": n, "error": "Need at least 3 values for skewness/kurtosis" }));
                continue;
            }
            let mean = values.iter().sum::<f64>() / n as f64;
            let std_dev =
                (values.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n as f64 - 1.0)).sqrt();
            let skewness = if std_dev > 0.0 {
                values
                    .iter()
                    .map(|x| ((x - mean) / std_dev).powi(3))
                    .sum::<f64>()
                    / n as f64
            } else {
                0.0
            };
            let kurtosis = if std_dev > 0.0 {
                values
                    .iter()
                    .map(|x| ((x - mean) / std_dev).powi(4))
                    .sum::<f64>()
                    / n as f64
                    - 3.0
            } else {
                0.0
            };
            let se_mean = std_dev / (n as f64).sqrt();
            columns_json.push(serde_json::json!({
                "name": col_name, "n_valid": n, "mean": mean, "std_dev": std_dev,
                "skewness": skewness, "kurtosis": kurtosis,
                "confidence_interval": { "level": confidence_level, "lower": mean - z_value * se_mean, "upper": mean + z_value * se_mean, "standard_error": se_mean },
            }));
        }
        Ok(ToolResult::success_json(serde_json::json!({
            "file": params.data_path, "confidence_level": confidence_level, "columns": columns_json,
        })))
    }
}
