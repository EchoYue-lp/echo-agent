//! demo56 — Plugin System
//!
//! Demonstrates the file-based plugin architecture that extends echo-agent
//! with custom skills, hooks, MCP servers, and more:
//!
//! 1. `PluginManifest` — parse `manifest.yaml` with validation
//! 2. `PluginRegistry` — discover, install, uninstall, enable/disable
//! 3. `PluginScope` — User / Project / Local access control
//! 4. `InstallSource` — local directory or git URL
//! 5. Dependency resolution via topological sort
//! 6. Component resolution (skills, hooks, MCP, etc.)
//!
//! All operations are local — no LLM calls needed.
//!
//! ```sh
//! cargo run --example demo56_plugin_system
//! ```

use echo_agent::plugin::{
    InstallSource, PluginCapability, PluginManifest, PluginRegistry, PluginScope,
};
use std::path::{Path, PathBuf};

macro_rules! section {
    ($n:expr, $title:expr) => {
        println!("\n══════════════════════════════════════════════════");
        println!("  Scenario {} : {}", $n, $title);
        println!("══════════════════════════════════════════════════");
    };
}

/// Create a test plugin directory with a manifest (source for installation).
fn create_test_plugin(base: &Path, name: &str, yaml: &str) -> PathBuf {
    let plugin_dir = base.join(name);
    let manifest_dir = plugin_dir.join(".echo-plugin");
    std::fs::create_dir_all(&manifest_dir).unwrap();
    std::fs::write(manifest_dir.join("manifest.yaml"), yaml).unwrap();
    plugin_dir
}

#[tokio::main]
async fn main() {
    println!("╔══════════════════════════════════════════════════╗");
    println!("║       echo-agent  Plugin System Demo             ║");
    println!("║  (all local — no LLM calls)                      ║");
    println!("╚══════════════════════════════════════════════════╝");

    demo_manifest_parsing();
    demo_manifest_validation();
    demo_capabilities();
    demo_scopes();
    demo_registry_lifecycle();
    demo_dependency_resolution();
    demo_install_source();
    demo_search();

    println!("\n╔══════════════════════════════════════════════════╗");
    println!("║  All 8 scenarios passed ✅                       ║");
    println!("╚══════════════════════════════════════════════════╝");
}

/// Scenario 1: Parse plugin manifest YAML
fn demo_manifest_parsing() {
    section!(1, "PluginManifest — YAML Parsing");

    let yaml = r#"
name: data-analysis-pack
display_name: "Data Analysis Pack"
version: "1.2.0"
description: "Enhanced data analysis with polars extensions"
author:
  name: "Echo Team"
  email: "team@echo.dev"
license: MIT
keywords: [data, analysis, polars]
components:
  skills: "./skills/"
  hooks: "./hooks/hooks.yaml"
  mcp_servers: "./.mcp.json"
config:
  api_endpoint:
    type: string
    title: "API Endpoint"
    description: "Service address"
    default: "http://localhost:8080"
  api_token:
    type: string
    title: "API Token"
    sensitive: true
    required: true
dependencies:
  - name: base-tools
    version: ">=1.0.0"
  - simple-dep
"#;
    let manifest = PluginManifest::from_yaml(yaml).unwrap();
    println!("  name:         {}", manifest.name);
    println!("  display_name: {:?}", manifest.display_name);
    println!("  version:      {}", manifest.version);
    println!("  description:  {}", manifest.description);
    println!("  license:      {:?}", manifest.license);
    println!("  keywords:     {:?}", manifest.keywords);
    println!(
        "  config keys:  {:?}",
        manifest.config.keys().collect::<Vec<_>>()
    );
    println!("  dependencies: {}", manifest.dependencies.len());
    println!("  default_enabled: {}", manifest.default_enabled);

    assert_eq!(manifest.name, "data-analysis-pack");
    assert_eq!(manifest.version, "1.2.0");
    assert!(manifest.is_valid());
    println!("  ✅ Full manifest parsed and validated");

    // Minimal manifest
    let minimal_yaml = r#"
name: my-minimal-plugin
description: "A minimal plugin"
"#;
    let minimal = PluginManifest::from_yaml(minimal_yaml).unwrap();
    println!("\n  Minimal manifest:");
    println!("    name: {}", minimal.name);
    println!("    version: {} (default)", minimal.version);
    println!("    default_enabled: {}", minimal.default_enabled);
    assert!(minimal.is_valid());
    println!("  ✅ Minimal manifest also valid");
}

/// Scenario 2: Manifest validation errors
fn demo_manifest_validation() {
    section!(2, "Manifest Validation — Error Detection");

    // Invalid name (not kebab-case)
    let yaml = "name: My Bad Plugin\ndescription: test";
    let manifest = PluginManifest::from_yaml(yaml).unwrap();
    let errors = manifest.validate();
    println!("  Bad name: \"My Bad Plugin\"");
    for e in &errors {
        println!("    ❌ {}: {}", e.field, e.message);
    }
    assert!(errors.iter().any(|e| e.field == "name"));

    // Path traversal attack
    let yaml = r#"
name: evil-plugin
description: "tries to escape"
components:
  skills: "../shared-skills/"
"#;
    let manifest = PluginManifest::from_yaml(yaml).unwrap();
    let errors = manifest.validate();
    println!("\n  Path traversal: \"../shared-skills/\"");
    for e in &errors {
        println!("    ❌ {}: {}", e.field, e.message);
    }
    assert!(errors.iter().any(|e| e.field == "components.skills"));

    // Path without ./ prefix
    let yaml = r#"
name: bad-paths
description: test
components:
  hooks: "hooks/hooks.yaml"
"#;
    let manifest = PluginManifest::from_yaml(yaml).unwrap();
    let errors = manifest.validate();
    println!("\n  Missing ./ prefix: \"hooks/hooks.yaml\"");
    for e in &errors {
        println!("    ❌ {}: {}", e.field, e.message);
    }
    assert!(errors.iter().any(|e| e.field == "components.hooks"));

    println!("\n  ✅ Validation catches common mistakes and security issues");
}

/// Scenario 3: Inferred capabilities from components
fn demo_capabilities() {
    section!(3, "PluginCapability — Inferred from Components");

    let yaml = r#"
name: full-plugin
version: "1.0.0"
description: "A plugin with all component types"
components:
  skills: "./skills/"
  hooks: "./hooks.yaml"
  mcp_servers: "./.mcp.json"
  lsp_servers: "./.lsp.yaml"
  agents: "./agents/"
  monitors: "./monitors.json"
  themes: "./themes/"
"#;
    let manifest = PluginManifest::from_yaml(yaml).unwrap();
    let caps = manifest.inferred_capabilities();

    println!("  Inferred capabilities ({} total):", caps.len());
    for cap in &caps {
        println!("    - {} ({:?})", cap.display_name(), cap);
    }

    assert_eq!(caps.len(), 7);
    assert!(caps.contains(&PluginCapability::Skill));
    assert!(caps.contains(&PluginCapability::Hook));
    assert!(caps.contains(&PluginCapability::McpServer));
    assert!(caps.contains(&PluginCapability::LspServer));
    assert!(caps.contains(&PluginCapability::Agent));
    assert!(caps.contains(&PluginCapability::Monitor));
    assert!(caps.contains(&PluginCapability::Theme));
    println!("  ✅ All 7 capability types detected");

    // Loose parsing of capability strings
    println!("\n  Loose string parsing:");
    let test_cases = [
        "skill",
        "skills",
        "hooks",
        "mcp",
        "lsp_server",
        "agent",
        "themes",
        "invalid",
    ];
    for s in &test_cases {
        let parsed = PluginCapability::from_str_loose(s);
        println!("    {:?} → {:?}", s, parsed);
    }
}

/// Scenario 4: PluginScope access control
fn demo_scopes() {
    section!(4, "PluginScope — Access Control");

    let project_root = Path::new("/home/user/my-project");

    println!("  Scope directory resolution:");
    for scope in PluginScope::all() {
        let dir = scope.resolve_dir(Some(project_root));
        println!("    {} → {}", scope, dir.display());
    }

    // Verify paths
    let user_dir = PluginScope::User.resolve_dir(None);
    assert!(user_dir.to_string_lossy().contains(".echo-agent/plugins"));

    let project_dir = PluginScope::Project.resolve_dir(Some(project_root));
    assert_eq!(
        project_dir,
        PathBuf::from("/home/user/my-project/.echo-agent/plugins")
    );

    let local_dir = PluginScope::Local.resolve_dir(Some(project_root));
    assert_eq!(
        local_dir,
        PathBuf::from("/home/user/my-project/.echo-agent/plugins.local")
    );

    // CLI argument parsing
    println!("\n  CLI argument parsing:");
    let args = ["user", "u", "project", "p", "local", "l", "invalid"];
    for arg in &args {
        let parsed = PluginScope::from_arg(arg);
        println!("    {:?} → {:?}", arg, parsed);
    }
    println!("  ✅ Three scopes: User (global), Project (shared), Local (private)");
}

/// Scenario 5: Registry install/uninstall lifecycle
fn demo_registry_lifecycle() {
    section!(5, "PluginRegistry — Install / Enable / Disable / Uninstall");

    let tmp = std::env::temp_dir().join("echo_plugin_demo56_lifecycle");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    // Create source plugin directories (what you'd check into a repo or download)
    let src_dir = tmp.join("sources");
    std::fs::create_dir_all(&src_dir).unwrap();
    create_test_plugin(
        &src_dir,
        "alpha-tool",
        "name: alpha-tool\nversion: \"1.0.0\"\ndescription: \"Alpha tools\"",
    );
    create_test_plugin(
        &src_dir,
        "beta-utils",
        "name: beta-utils\nversion: \"2.0.0\"\ndescription: \"Beta utilities\"",
    );

    // Create registry with custom paths and a project root
    let mut registry = PluginRegistry::with_paths(
        tmp.join("registry.json"),
        tmp.join("data"),
        Some(tmp.clone()),
    );

    // Install plugins into Local scope
    let alpha_id = registry
        .install(
            &InstallSource::Local(src_dir.join("alpha-tool")),
            PluginScope::Local,
        )
        .unwrap();
    let beta_id = registry
        .install(
            &InstallSource::Local(src_dir.join("beta-utils")),
            PluginScope::Local,
        )
        .unwrap();
    println!("  Installed: {} and {}", alpha_id, beta_id);
    assert_eq!(registry.count(), 2);

    // List all plugins
    println!("\n  Installed plugins:");
    for entry in registry.list() {
        println!(
            "    {} v{} [{}] enabled={}",
            entry.manifest.name, entry.manifest.version, entry.scope, entry.enabled
        );
    }

    // Disable a plugin
    registry.disable("alpha-tool").unwrap();
    assert!(!registry.get("alpha-tool").unwrap().enabled);
    println!("\n  Disabled alpha-tool");

    // list_enabled filters disabled plugins
    let enabled = registry.list_enabled();
    assert_eq!(enabled.len(), 1);
    assert_eq!(enabled[0].manifest.name, "beta-utils");
    println!(
        "  Enabled plugins: {:?}",
        enabled.iter().map(|e| &e.manifest.name).collect::<Vec<_>>()
    );

    // Re-enable
    registry.enable("alpha-tool").unwrap();
    assert!(registry.get("alpha-tool").unwrap().enabled);
    println!("  Re-enabled alpha-tool");

    // Uninstall
    registry.uninstall("beta-utils", false).unwrap();
    assert_eq!(registry.count(), 1);
    println!("  Uninstalled beta-utils");
    println!("  Remaining plugins: {}", registry.count());

    let _ = std::fs::remove_dir_all(&tmp);
    println!("  ✅ Full install → disable → enable → uninstall lifecycle");
}

/// Scenario 6: Dependency resolution
fn demo_dependency_resolution() {
    section!(6, "Dependency Resolution (Topological Sort)");

    let tmp = std::env::temp_dir().join("echo_plugin_demo56_deps");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    // Build a dependency graph: A → B → C
    // (A depends on B, B depends on C)
    let yaml_c = "name: plugin-c\nversion: \"1.0.0\"\ndescription: \"Foundation\"";
    let yaml_b = "name: plugin-b\nversion: \"1.0.0\"\ndescription: \"Middle layer\"\ndependencies:\n  - plugin-c";
    let yaml_a = "name: plugin-a\nversion: \"1.0.0\"\ndescription: \"Top layer\"\ndependencies:\n  - name: plugin-b\n    version: \">=1.0.0\"";

    // Create source plugin directories
    let src_dir = tmp.join("sources");
    std::fs::create_dir_all(&src_dir).unwrap();
    create_test_plugin(&src_dir, "plugin-c", yaml_c);
    create_test_plugin(&src_dir, "plugin-b", yaml_b);
    create_test_plugin(&src_dir, "plugin-a", yaml_a);

    let mut registry = PluginRegistry::with_paths(
        tmp.join("registry.json"),
        tmp.join("data"),
        Some(tmp.clone()),
    );

    // Install all three plugins
    registry
        .install(
            &InstallSource::Local(src_dir.join("plugin-c")),
            PluginScope::Local,
        )
        .unwrap();
    registry
        .install(
            &InstallSource::Local(src_dir.join("plugin-b")),
            PluginScope::Local,
        )
        .unwrap();
    registry
        .install(
            &InstallSource::Local(src_dir.join("plugin-a")),
            PluginScope::Local,
        )
        .unwrap();

    println!("  Dependency graph:");
    println!("    plugin-a depends on plugin-b");
    println!("    plugin-b depends on plugin-c");
    println!("    plugin-c has no dependencies");

    let sorted = registry.resolve_dependencies().unwrap();
    println!("\n  Resolved load order: {:?}", sorted);

    // C must come before B, B before A
    let pos_a = sorted.iter().position(|x| x == "plugin-a").unwrap();
    let pos_b = sorted.iter().position(|x| x == "plugin-b").unwrap();
    let pos_c = sorted.iter().position(|x| x == "plugin-c").unwrap();
    assert!(pos_c < pos_b, "C should load before B");
    assert!(pos_b < pos_a, "B should load before A");
    println!("  ✅ Dependencies resolved: C → B → A (topological order)");

    let _ = std::fs::remove_dir_all(&tmp);
}

/// Scenario 7: InstallSource parsing
fn demo_install_source() {
    section!(7, "InstallSource — Local vs Git");

    let test_cases = [
        "/home/user/my-plugin",
        "./relative-path",
        "https://github.com/echo/plugin.git",
        "https://github.com/echo/monorepo.git",
    ];

    println!("  Parsing install sources:");
    for input in &test_cases {
        let source = InstallSource::parse(input);
        println!(
            "    {:?} → is_git={}, display={}",
            input,
            source.is_git(),
            source
        );
    }

    assert!(!InstallSource::parse("/home/user/my-plugin").is_git());
    assert!(InstallSource::parse("https://github.com/echo/plugin.git").is_git());
    println!("  ✅ Local paths and git URLs detected correctly");
}

/// Scenario 8: Search plugins by keyword
fn demo_search() {
    section!(8, "Search Plugins by Keyword");

    let tmp = std::env::temp_dir().join("echo_plugin_demo56_search");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let src_dir = tmp.join("sources");
    std::fs::create_dir_all(&src_dir).unwrap();

    let yaml1 = r#"
name: data-analysis
version: "1.0.0"
description: "Data analysis with polars"
keywords: [data, polars, visualization]
"#;
    let yaml2 = r#"
name: web-scraper
version: "1.0.0"
description: "Web scraping toolkit"
keywords: [web, scraping]
"#;
    let yaml3 = r#"
name: data-pipeline
version: "1.0.0"
description: "ETL data pipeline"
keywords: [data, etl]
"#;

    create_test_plugin(&src_dir, "data-analysis", yaml1);
    create_test_plugin(&src_dir, "web-scraper", yaml2);
    create_test_plugin(&src_dir, "data-pipeline", yaml3);

    let mut registry = PluginRegistry::with_paths(
        tmp.join("registry.json"),
        tmp.join("data"),
        Some(tmp.clone()),
    );

    // Install all three plugins
    registry
        .install(
            &InstallSource::Local(src_dir.join("data-analysis")),
            PluginScope::Local,
        )
        .unwrap();
    registry
        .install(
            &InstallSource::Local(src_dir.join("web-scraper")),
            PluginScope::Local,
        )
        .unwrap();
    registry
        .install(
            &InstallSource::Local(src_dir.join("data-pipeline")),
            PluginScope::Local,
        )
        .unwrap();

    // Search by keyword
    let results = registry.search("data");
    println!("  Search \"data\" → {} results:", results.len());
    for entry in &results {
        println!(
            "    - {} ({})",
            entry.manifest.name, entry.manifest.description
        );
    }
    assert_eq!(results.len(), 2);

    let results = registry.search("polars");
    println!("\n  Search \"polars\" → {} results:", results.len());
    for entry in &results {
        println!(
            "    - {} (keywords: {:?})",
            entry.manifest.name, entry.manifest.keywords
        );
    }
    assert_eq!(results.len(), 1);

    let results = registry.search("nothing");
    println!("\n  Search \"nothing\" → {} results", results.len());
    assert_eq!(results.len(), 0);

    let _ = std::fs::remove_dir_all(&tmp);
    println!("  ✅ Keyword search across name, description, and keywords");
}
