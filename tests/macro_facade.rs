use echo_agent::error::Result;
use echo_agent::tools::{Tool, ToolParameters, ToolResult, ToolRunner};

#[derive(Default, echo_agent::Tool)]
#[tool(name = "facade_echo", description = "Echo one facade parameter")]
#[allow(dead_code)]
struct FacadeEchoTool {
    #[tool_param(description = "Text to echo")]
    text: String,
}

impl ToolRunner<FacadeEchoToolParams> for FacadeEchoTool {
    async fn run(&self, params: FacadeEchoToolParams) -> Result<ToolResult> {
        Ok(ToolResult::success(params.text))
    }
}

#[derive(Default, echo_agent::Tool)]
#[tool(name = "facade_ping", description = "Exercise a unit tool")]
struct FacadePingTool;

impl ToolRunner<FacadePingToolParams> for FacadePingTool {
    async fn run(&self, _params: FacadePingToolParams) -> Result<ToolResult> {
        Ok(ToolResult::success("pong"))
    }
}

#[tokio::test]
async fn facade_derive_supports_named_and_unit_tools() -> Result<()> {
    let named = FacadeEchoTool::default();
    let schema = named.parameters();
    assert_eq!(
        schema
            .get("properties")
            .and_then(|properties| properties.get("text"))
            .and_then(|text| text.get("description"))
            .and_then(|description| description.as_str()),
        Some("Text to echo")
    );

    let mut parameters = ToolParameters::new();
    parameters.insert("text".to_string(), "hello".into());
    named.validate_parameters(&parameters).await?;
    let result = named.execute(parameters).await?;
    assert_eq!(result.output, "hello");

    let unit = FacadePingTool;
    unit.validate_parameters(&ToolParameters::new()).await?;
    let result = unit.execute(ToolParameters::new()).await?;
    assert_eq!(result.output, "pong");
    Ok(())
}
