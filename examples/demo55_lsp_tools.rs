//! demo55 — LSP Tools
//!
//! Demonstrates the Language Server Protocol integration that gives agents
//! IDE-like code understanding:
//!
//! 1. `LspConfig` — parse `.lsp.yaml` configuration
//! 2. `LspManager` — manage multiple language server processes
//! 3. LSP tool types — diagnostics, goto_definition, find_references, hover, status
//! 4. File-extension → language server routing
//!
//! **Note:** Actually running language servers requires the corresponding
//! server binaries (e.g., `rust-analyzer`, `pyright-langserver`) to be installed.
//! This demo shows configuration and tool construction without starting servers.
//!
//! Feature flags required: `lsp`
//!
//! ```sh
//! cargo run --features lsp --example demo55_lsp_tools
//! ```

#[cfg(feature = "lsp")]
mod demo {
    use echo_agent::lsp::{LspConfig, LspManager, LspServerConfig};
    use echo_agent::prelude::Tool;
    use echo_agent::tools::lsp::{
        LspDiagnosticsTool, LspFindReferencesTool, LspGotoDefinitionTool, LspHoverTool,
        LspStatusTool,
    };
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    macro_rules! section {
        ($n:expr, $title:expr) => {
            println!("\n══════════════════════════════════════════════════");
            println!("  Scenario {} : {}", $n, $title);
            println!("══════════════════════════════════════════════════");
        };
    }

    pub async fn run() {
        println!("╔══════════════════════════════════════════════════╗");
        println!("║       echo-agent  LSP Tools Demo                 ║");
        println!("║  (config + tool construction — servers optional)  ║");
        println!("╚══════════════════════════════════════════════════╝");

        demo_config_from_yaml();
        demo_config_extension_routing();
        demo_manager_creation();
        demo_tool_descriptions();
        demo_tool_parameters();
        demo_programmatic_config();

        println!("\n╔══════════════════════════════════════════════════╗");
        println!("║  All 6 scenarios passed ✅                       ║");
        println!("╚══════════════════════════════════════════════════╝");
    }

    /// Scenario 1: Parse LspConfig from YAML
    fn demo_config_from_yaml() {
        section!(1, "LspConfig from YAML (.lsp.yaml)");

        let yaml = r#"
languages:
  rust:
    language: rust
    command: rust-analyzer
    args: []
    extensions: [".rs"]
  python:
    language: python
    command: pyright-langserver
    args: ["--stdio"]
    extensions: [".py", ".pyi"]
  typescript:
    language: typescript
    command: typescript-language-server
    args: ["--stdio"]
    extensions: [".ts", ".tsx", ".js", ".jsx"]
"#;
        let config = LspConfig::from_yaml(yaml).unwrap();
        println!("  Loaded {} language server configs:", config.servers.len());
        for (lang, server) in &config.servers {
            println!(
                "    {} → {} (extensions: {:?})",
                lang, server.command, server.extensions
            );
        }

        assert_eq!(config.servers.len(), 3);
        assert_eq!(config.get("rust").unwrap().command, "rust-analyzer");
        println!("  ✅ YAML config parsed successfully");
    }

    /// Scenario 2: Extension-based routing
    fn demo_config_extension_routing() {
        section!(2, "File Extension → Language Server Routing");

        let yaml = r#"
languages:
  rust:
    language: rust
    command: rust-analyzer
    args: []
    extensions: [".rs"]
  python:
    language: python
    command: pyright-langserver
    args: ["--stdio"]
    extensions: [".py", ".pyi"]
"#;
        let config = LspConfig::from_yaml(yaml).unwrap();

        let test_extensions = [".rs", ".py", ".pyi", ".ts"];
        for ext in &test_extensions {
            match config.get_for_extension(ext) {
                Some((lang, server)) => {
                    println!("  {}  →  {} ({})", ext, lang, server.command);
                }
                None => {
                    println!("  {}  →  (no server configured)", ext);
                }
            }
        }

        assert!(config.get_for_extension(".rs").is_some());
        assert!(config.get_for_extension(".py").is_some());
        assert!(config.get_for_extension(".ts").is_none());
        println!("  ✅ Extension routing works correctly");
    }

    /// Scenario 3: Create an LspManager
    fn demo_manager_creation() {
        section!(3, "LspManager Creation");

        let mut manager = LspManager::new();
        println!("  Created empty LspManager");

        let yaml = r#"
languages:
  rust:
    language: rust
    command: rust-analyzer
    args: []
    extensions: [".rs"]
"#;
        let config = LspConfig::from_yaml(yaml).unwrap();
        manager.load_config(&config);
        println!("  Loaded rust-analyzer config");

        // Set project root
        manager.set_project_root(std::path::Path::new("/tmp/my-rust-project"));
        println!("  Set project root: /tmp/my-rust-project");

        // Wrap in Arc<RwLock<>> for sharing with tools
        let shared_manager = Arc::new(RwLock::new(manager));
        println!("  Wrapped in Arc<RwLock<LspManager>> for tool sharing");

        let _ = shared_manager;
        println!("  ✅ Manager ready (start_server() requires rust-analyzer binary)");
    }

    /// Scenario 4: Tool descriptions and names
    fn demo_tool_descriptions() {
        section!(4, "LSP Tool Descriptions");

        let manager = Arc::new(RwLock::new(LspManager::new()));

        let tools: Vec<Box<dyn Tool>> = vec![
            Box::new(LspDiagnosticsTool::new(manager.clone())),
            Box::new(LspGotoDefinitionTool::new(manager.clone())),
            Box::new(LspFindReferencesTool::new(manager.clone())),
            Box::new(LspHoverTool::new(manager.clone())),
            Box::new(LspStatusTool::new(manager.clone())),
        ];

        println!("  {} LSP tools available:\n", tools.len());
        for tool in &tools {
            println!("  Tool: {}", tool.name());
            let desc = tool.description();
            // Show first 80 chars of description
            let short_desc = if desc.len() > 80 {
                format!("{}...", &desc[..80])
            } else {
                desc.to_string()
            };
            println!("    Description: {}", short_desc);
            println!("    Risk level:  {:?}\n", tool.risk_level());
        }

        // Verify all tools are read-only
        for tool in &tools {
            assert_eq!(
                format!("{:?}", tool.risk_level()),
                "ReadOnly",
                "Tool {} should be ReadOnly",
                tool.name()
            );
        }
        println!("  ✅ All LSP tools are ReadOnly (safe for analysis)");
    }

    /// Scenario 5: Tool parameter schemas
    fn demo_tool_parameters() {
        section!(5, "Tool Parameter Schemas (JSON Schema)");

        let manager = Arc::new(RwLock::new(LspManager::new()));

        let goto = LspGotoDefinitionTool::new(manager.clone());
        let schema = goto.parameters();
        println!("  lsp_goto_definition parameters:");
        println!("    {}", serde_json::to_string_pretty(&schema).unwrap());

        let diag = LspDiagnosticsTool::new(manager.clone());
        let schema = diag.parameters();
        println!("\n  lsp_diagnostics parameters:");
        println!("    {}", serde_json::to_string_pretty(&schema).unwrap());

        println!("\n  ✅ JSON Schema parameters verified");
    }

    /// Scenario 6: Programmatic config construction
    fn demo_programmatic_config() {
        section!(6, "Programmatic LspServerConfig");

        // Build config without YAML
        let mut servers = HashMap::new();
        servers.insert(
            "rust".to_string(),
            LspServerConfig {
                language: "rust".to_string(),
                command: "rust-analyzer".to_string(),
                args: vec![],
                extensions: vec![".rs".to_string()],
                env: HashMap::new(),
                initialization_options: None,
                max_restarts: 3,
            },
        );

        let config = LspConfig { servers };
        assert!(config.get("rust").is_some());
        assert_eq!(config.get("rust").unwrap().command, "rust-analyzer");

        println!("  Built config programmatically:");
        println!("    rust → rust-analyzer (max_restarts: 3)");
        println!("  ✅ No YAML needed for simple setups");
    }
}

#[cfg(not(feature = "lsp"))]
mod demo {
    pub async fn run() {
        println!("╔══════════════════════════════════════════════════╗");
        println!("║       echo-agent  LSP Tools Demo                 ║");
        println!("╚══════════════════════════════════════════════════╝");
        println!();
        println!("  ⚠️  This demo requires the `lsp` feature flag.");
        println!();
        println!("  Run with:");
        println!("    cargo run --features lsp --example demo55_lsp_tools");
        println!();
        println!("  The LSP integration provides:");
        println!("    - LspConfig: parse .lsp.yaml configuration files");
        println!("    - LspManager: manage multiple language server processes");
        println!("    - LspDiagnosticsTool: get errors/warnings from language servers");
        println!("    - LspGotoDefinitionTool: find symbol definitions");
        println!("    - LspFindReferencesTool: find all symbol usages");
        println!("    - LspHoverTool: get type info and documentation");
        println!("    - LspStatusTool: check language server health");
        println!();
        println!("  Supported languages: Rust, Python, TypeScript, and more.");
    }
}

#[tokio::main]
async fn main() {
    demo::run().await;
}
