use echo_agent::agent::AgentInvocationContext;
use echo_agent::config::FrameworkConfig;
use echo_agent::paths::DataRoot;
use echo_agent::tools::{InvocationResourceGuard, StandardToolPack, ToolPack};

fn main() {
    let root = DataRoot::new("./agent-data");
    let config = FrameworkConfig::default();
    let pack = StandardToolPack::new();
    let invocation = AgentInvocationContext {
        // Any Send + Sync owner can be retained without exposing it to tools.
        resource_guards: vec![InvocationResourceGuard::new("example-lease".to_string())],
        ..AgentInvocationContext::default()
    };

    println!(
        "facade example: root={}, model={}, tool_pack={}, guards={}",
        root.as_path().display(),
        config.model.name,
        pack.name(),
        invocation.resource_guards.len()
    );
    assert!(
        invocation
            .resource_guards
            .first()
            .is_some_and(InvocationResourceGuard::retains::<String>)
    );
}
