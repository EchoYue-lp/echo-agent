//! Chart generation tool
//!
//! Generate data visualization charts via vega-lite v5 JSON specification.
//! Supports line, bar, pie, scatter, area, heatmap, boxplot, and more.

use futures::future::BoxFuture;
use serde_json::Value;

use echo_core::error::{Result, ToolError};
use echo_core::tools::permission::ToolPermission;
use echo_core::tools::{Tool, ToolParameters, ToolResult};

// ── generate_chart tool ──────────────────────────────────────────────────────

pub struct GenerateChartTool;

impl Tool for GenerateChartTool {
    fn name(&self) -> &str {
        "generate_chart"
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Read, ToolPermission::Write]
    }

    fn description(&self) -> &str {
        "Generate data visualization charts (vega-lite v5 JSON spec). \
         Supports line, bar, pie, scatter, area, heatmap, boxplot."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "chart_type": {
                    "type": "string",
                    "description": "Chart type",
                    "enum": ["line", "bar", "pie", "scatter", "area", "heatmap", "boxplot"]
                },
                "title": {
                    "type": "string",
                    "description": "Chart title"
                },
                "x_field": {
                    "type": "string",
                    "description": "X axis field name (used as color grouping in pie charts)"
                },
                "y_field": {
                    "type": "string",
                    "description": "Y axis field name (used as value in pie charts)"
                },
                "color_field": {
                    "type": "string",
                    "description": "Color grouping field name (optional)"
                },
                "data": {
                    "type": "array",
                    "description": "Chart data (array of JSON objects, each object's key maps to a field name)",
                    "items": {"type": "object"}
                },
                "width": {
                    "type": "integer",
                    "description": "Chart width in pixels (default 600)"
                },
                "height": {
                    "type": "integer",
                    "description": "Chart height in pixels (default 400)"
                },
                "output_format": {
                    "type": "string",
                    "description": "Output format: 'json' (default, Vega-Lite spec) or 'html' (standalone HTML page with embedded chart)",
                    "enum": ["json", "html"]
                }
            },
            "required": ["chart_type", "data", "x_field", "y_field"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let chart_type = parameters
                .get("chart_type")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("chart_type".to_string()))?;

            let title = parameters
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let x_field = parameters
                .get("x_field")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("x_field".to_string()))?;

            let y_field = parameters
                .get("y_field")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("y_field".to_string()))?;

            let color_field = parameters.get("color_field").and_then(|v| v.as_str());

            let data = parameters
                .get("data")
                .ok_or_else(|| ToolError::MissingParameter("data".to_string()))?;

            let width = parameters
                .get("width")
                .and_then(|v| v.as_u64())
                .unwrap_or(600) as u32;

            let height = parameters
                .get("height")
                .and_then(|v| v.as_u64())
                .unwrap_or(400) as u32;

            let output_format = parameters
                .get("output_format")
                .and_then(|v| v.as_str())
                .unwrap_or("json");

            let spec = build_vega_lite_spec(
                chart_type,
                title,
                x_field,
                y_field,
                color_field,
                data,
                width,
                height,
            );

            let output = if output_format == "html" {
                generate_html_page(&spec, title)
            } else {
                serde_json::to_string_pretty(&spec).unwrap_or_else(|_| "{}".to_string())
            };

            Ok(ToolResult::success(output))
        })
    }
}

// ── Vega-Lite Spec Builder ───────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn build_vega_lite_spec(
    chart_type: &str,
    title: &str,
    x_field: &str,
    y_field: &str,
    color_field: Option<&str>,
    data: &Value,
    width: u32,
    height: u32,
) -> Value {
    let mut spec = serde_json::json!({
        "$schema": "https://vega.github.io/schema/vega-lite/v5.json",
        "width": width,
        "height": height,
        "data": {"values": data},
        "mark": mark_type(chart_type),
    });

    if !title.is_empty() {
        spec["title"] = Value::String(title.to_string());
    }

    match chart_type {
        "pie" => {
            spec["encoding"] = serde_json::json!({
                "theta": {"field": y_field, "type": "quantitative"},
                "color": {"field": x_field, "type": "nominal"}
            });
            if let Some(color) = color_field {
                spec["encoding"]["color"] = serde_json::json!({
                    "field": color, "type": "nominal"
                });
            }
        }
        "heatmap" => {
            spec["encoding"] = serde_json::json!({
                "x": {"field": x_field, "type": "nominal"},
                "y": {"field": y_field, "type": "nominal"},
                "color": {"field": color_field.unwrap_or(y_field), "type": "quantitative"}
            });
        }
        "boxplot" => {
            spec["encoding"] = serde_json::json!({
                "x": {"field": x_field, "type": "nominal"},
                "y": {"field": y_field, "type": "quantitative", "scale": {"zero": false}}
            });
        }
        _ => {
            // line, bar, scatter, area
            spec["encoding"] = serde_json::json!({
                "x": {"field": x_field, "type": "nominal"},
                "y": {"field": y_field, "type": "quantitative"}
            });
            if let Some(color) = color_field {
                spec["encoding"]["color"] = serde_json::json!({
                    "field": color, "type": "nominal"
                });
            }
        }
    }

    spec
}

fn mark_type(chart_type: &str) -> &str {
    match chart_type {
        "bar" => "bar",
        "pie" => "arc",
        "scatter" => "point",
        "area" => "area",
        "heatmap" => "rect",
        "boxplot" => "boxplot",
        _ => "line", // default to line
    }
}

// ── HTML Page Generator ─────────────────────────────────────────────────────

fn generate_html_page(spec: &Value, title: &str) -> String {
    let spec_json = serde_json::to_string_pretty(spec).unwrap_or_else(|_| "{}".to_string());
    let display_title = if title.is_empty() {
        "Chart Visualization"
    } else {
        title
    };

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>{}</title>
    <script src="https://cdn.jsdelivr.net/npm/vega@5"></script>
    <script src="https://cdn.jsdelivr.net/npm/vega-lite@5"></script>
    <script src="https://cdn.jsdelivr.net/npm/vega-embed@6"></script>
    <style>
        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif;
            margin: 0;
            padding: 20px;
            background-color: #f5f5f5;
        }}
        .container {{
            max-width: 1200px;
            margin: 0 auto;
            background-color: white;
            padding: 30px;
            border-radius: 8px;
            box-shadow: 0 2px 8px rgba(0,0,0,0.1);
        }}
        h1 {{
            color: #333;
            margin-top: 0;
        }}
        #vis {{
            margin: 20px 0;
        }}
        .info {{
            margin-top: 20px;
            padding: 15px;
            background-color: #f9f9f9;
            border-left: 4px solid #4CAF50;
            font-size: 14px;
            color: #666;
        }}
    </style>
</head>
<body>
    <div class="container">
        <h1>{}</h1>
        <div id="vis"></div>
        <div class="info">
            <strong>Chart Type:</strong> {} |
            <strong>Powered by:</strong> Vega-Lite v5
        </div>
    </div>

    <script type="text/javascript">
        var spec = {};
        vegaEmbed('#vis', spec, {{
            actions: {{
                export: true,
                source: false,
                compiled: false,
                editor: false
            }}
        }}).catch(console.error);
    </script>
</body>
</html>"#,
        display_title, display_title, "Interactive Chart", spec_json
    )
}
