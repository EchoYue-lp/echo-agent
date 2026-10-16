#![cfg(feature = "web")]

use echo_core::tools::{ToolPack, ToolParameters};
use echo_tools::StandardToolPack;
use echo_tools::web::WebSearchTool;
use serde_json::json;

/// Exercise environment selection in child processes without unsafe global
/// environment mutation or interference with other concurrently running tests.
#[test]
fn web_search_selects_configured_provider() -> Result<(), Box<dyn std::error::Error>> {
    if let Ok(expected) = std::env::var("ECHO_EXA_PROVIDER_TEST") {
        assert_eq!(WebSearchTool::auto().provider_name(), expected);
        if expected == "exa" {
            assert_eq!(WebSearchTool::with_exa("test-key").provider_name(), "exa");
            let tool = StandardToolPack::new()
                .tools()
                .into_iter()
                .find(|entry| entry.tool.name() == "web_search")
                .ok_or("standard pack has no web_search")?;
            let mut parameters = ToolParameters::new();
            parameters.insert("query".to_string(), json!("rust"));
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            // The fixture key contains an invalid header byte, so reqwest fails
            // before network I/O. Its error identifies the installed provider.
            let result = runtime.block_on(tool.tool.execute(parameters))?;
            assert!(!result.success);
            assert!(
                result
                    .error
                    .as_deref()
                    .is_some_and(|error| error.contains("'exa'"))
            );
        }
        return Ok(());
    }

    for (exa, tavily, brave, expected) in [
        (
            Some("invalid\nheader"),
            Some("test-tavily"),
            Some("test-brave"),
            "exa",
        ),
        (Some("invalid\nheader"), None, None, "exa"),
        (None, Some("test-tavily"), Some("test-brave"), "tavily"),
        (None, None, Some("test-brave"), "brave"),
        (None, None, None, "duckduckgo"),
        (Some("  "), Some("test-tavily"), None, "tavily"),
    ] {
        let mut command = std::process::Command::new(std::env::current_exe()?);
        command.args(["--exact", "web_search_selects_configured_provider"]);
        command.env("ECHO_EXA_PROVIDER_TEST", expected);
        for (name, value) in [
            ("EXA_API_KEY", exa),
            ("TAVILY_API_KEY", tavily),
            ("BRAVE_SEARCH_API_KEY", brave),
        ] {
            command.env_remove(name);
            if let Some(value) = value {
                command.env(name, value);
            }
        }
        let output = command.output()?;
        assert!(
            output.status.success(),
            "{expected}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}
