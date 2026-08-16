use echo_core::tools::{Tool, ToolParameters, ToolResult};

#[echo_macros::tool(name = "core_echo", description = "Echo text through echo_core")]
async fn core_echo(text: String) -> echo_core::error::Result<ToolResult> {
    Ok(ToolResult::success(text))
}

#[tokio::test]
async fn attribute_macro_resolves_echo_core_without_facade_dependency()
-> echo_core::error::Result<()> {
    let mut parameters = ToolParameters::new();
    parameters.insert("text".to_string(), serde_json::json!("direct-core"));

    let result = CoreEchoTool.execute(parameters).await?;

    assert!(result.success);
    assert_eq!(result.output, "direct-core");
    Ok(())
}
