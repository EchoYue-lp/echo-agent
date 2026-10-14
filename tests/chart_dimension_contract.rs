#![cfg(feature = "chart")]

use echo_agent::error::{ReactError, Result, ToolError};
use echo_agent::tools::{ToolManager, ToolParameters};
use echo_tools::chart::GenerateChartTool;
use serde_json::{Value, json};

fn parameters() -> ToolParameters {
    ToolParameters::from([
        ("chart_type".to_string(), json!("line")),
        ("data".to_string(), json!([{ "x": "中文😀", "y": 1 }])),
        ("x_field".to_string(), json!("x")),
        ("y_field".to_string(), json!("y")),
    ])
}

#[tokio::test]
async fn chart_dimension_schema_and_manager_reject_out_of_range_integer() -> Result<()> {
    let manager = ToolManager::new();
    manager.register(Box::new(GenerateChartTool));
    let mut failures = Vec::new();
    for field in ["width", "height"] {
        for value in [json!(-1), json!(4294967296_u64), json!(u64::MAX)] {
            let mut params = parameters();
            params.insert(field.to_string(), value.clone());
            if echo_agent::tools::execution::validate_tool_input(&GenerateChartTool, &params)
                .await
                .is_ok()
            {
                failures.push(format!("schema admitted {field}={value}"));
            }
            match manager.execute_tool("generate_chart", params).await {
                Err(ReactError::Tool(error))
                    if matches!(error.as_ref(), ToolError::InvalidParameter { .. }) => {}
                other => failures.push(format!(
                    "manager admitted/lost invalid {field}={value}: {other:?}"
                )),
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(ReactError::Other(failures.join("; ")))
    }
}

#[tokio::test]
async fn chart_dimension_validator_and_manager_preserve_defaults_and_boundaries() -> Result<()> {
    let manager = ToolManager::new();
    manager.register(Box::new(GenerateChartTool));
    for dimensions in [
        None,
        Some((321_u64, 123_u64)),
        Some((0, 0)),
        Some((1, 1)),
        Some((u64::from(u32::MAX), u64::from(u32::MAX))),
    ] {
        let mut params = parameters();
        if let Some((width, height)) = dimensions {
            params.insert("width".to_string(), json!(width));
            params.insert("height".to_string(), json!(height));
        }
        echo_agent::tools::execution::validate_tool_input(&GenerateChartTool, &params).await?;
        let result = manager.execute_tool("generate_chart", params).await?;
        let spec: Value = serde_json::from_str(&result.output)?;
        let (width, height) = dimensions.unwrap_or((600, 400));
        if spec.get("width").and_then(Value::as_u64) != Some(width)
            || spec.get("height").and_then(Value::as_u64) != Some(height)
        {
            return Err(ReactError::Other(
                "canonical manager lost chart dimensions".to_string(),
            ));
        }
    }
    Ok(())
}

#[tokio::test]
async fn chart_dimension_schema_manager_reject_supplied_noninteger_values() -> Result<()> {
    let manager = ToolManager::new();
    manager.register(Box::new(GenerateChartTool));
    for field in ["width", "height"] {
        for value in [json!(1.5), json!("600"), json!(true), Value::Null] {
            let mut params = parameters();
            params.insert(field.to_string(), value.clone());
            if echo_agent::tools::execution::validate_tool_input(&GenerateChartTool, &params)
                .await
                .is_ok()
                || manager.execute_tool("generate_chart", params).await.is_ok()
            {
                return Err(ReactError::Other(format!(
                    "invalid supplied {field}={value} was admitted"
                )));
            }
        }
    }
    Ok(())
}
