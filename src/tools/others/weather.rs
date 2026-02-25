use crate::error::{Result, ToolError};
use crate::tools::{Tool, ToolParameters, ToolResult};
use serde_json::{Value, json};

pub struct WeatherTool;

#[async_trait::async_trait]
impl Tool for WeatherTool {
    fn name(&self) -> &str {
        "query_weather"
    }

    fn description(&self) -> &str {
        "我可以帮你查询天气奥。"
    }

    fn parameters(&self) -> Value {
        json!( {
            "type": "object",
            "properties": {
                "city": {
                    "type": "string",
                    "description": "城市名称"
                },
                "date": {
                    "type": "string",
                    "description": "日期"
                }
            },
            "required": ["city", "date"]
        })
    }

    async fn execute(&self, parameters: ToolParameters) -> Result<ToolResult> {
        let city = parameters
            .get("city")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::MissingParameter("city".to_string()))?;

        let date = parameters
            .get("date")
            .and_then(|city| city.as_str())
            .ok_or_else(|| ToolError::MissingParameter("date".to_string()))?;

        let result = format!("{} 的 {} 天气是暴雨，温度 30摄氏度。", city, date);

        Ok(ToolResult::success(result))
    }
}
