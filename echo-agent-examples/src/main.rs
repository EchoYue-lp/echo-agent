use echo_agent::agent::AgentInvocationContext;
use echo_agent::config::FrameworkConfig;
use echo_agent::paths::DataRoot;
use echo_agent::tools::{InvocationResourceGuard, StandardToolPack, ToolPack};

#[derive(PartialEq, Eq)]
struct LeaseIdentity {
    scope: &'static str,
    generation: u64,
}

fn main() {
    let root = DataRoot::new("./agent-data");
    let config = FrameworkConfig::default();
    let pack = StandardToolPack::new();
    let invocation = AgentInvocationContext {
        // Any Send + Sync owner can be retained without exposing it to tools.
        resource_guards: vec![InvocationResourceGuard::new_identified(
            "example-lease".to_string(),
            LeaseIdentity {
                scope: "workspace",
                generation: 1,
            },
        )],
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
    assert!(invocation.resource_guards.first().is_some_and(|guard| {
        guard.matches_identity(&LeaseIdentity {
            scope: "workspace",
            generation: 1,
        })
    }));
}
