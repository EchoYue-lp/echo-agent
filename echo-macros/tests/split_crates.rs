use echo_core::__macro_support::serde_json::Value;
use echo_core::tools::{Tool, ToolResult};
use echo_orchestration::human_loop::{ApprovalDecision, HumanLoopHandler};

#[echo_macros::tool(name = "split_add", description = "Add two numbers")]
async fn split_add(a: f64, b: f64) -> echo_core::error::Result<ToolResult> {
    Ok(ToolResult::success((a + b).to_string()))
}

struct SplitHandler;

#[echo_macros::handler]
impl SplitHandler {
    async fn on_approval(
        &self,
        _tool_name: &str,
        _args: &Value,
        _prompt: &str,
    ) -> ApprovalDecision {
        ApprovalDecision::Approved
    }

    async fn on_input(&self, prompt: &str) -> String {
        prompt.to_string()
    }
}

fn accepts_tool<T: Tool>() {}
fn accepts_handler<T: HumanLoopHandler>() {}

#[test]
fn split_crate_consumers_do_not_require_the_facade() {
    accepts_tool::<SplitAddTool>();
    accepts_handler::<SplitHandler>();
}
