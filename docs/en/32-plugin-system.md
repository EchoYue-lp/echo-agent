# Plugin System

## What It Is

The plugin system extends Agent capabilities through declarative `manifest.yaml` files without modifying core code. A plugin is a self-contained directory that can provide the following components:

| Component | Current Status | Description |
|-----------|----------------|-------------|
| Skills | Wired to `SkillRegistry` | SKILL.md files with live load/unload |
| Hooks | Wired to `HookRegistry` | Lifecycle and tool hooks with live load/unload |
| MCP Servers | Wired to `McpManager` | MCP servers and tools with live load/unload |
| Agents | Live in EKO | Parsed and registered with an executable subagent factory |
| LSP Servers | Live in EKO | Started and stopped by the application-owned `LspManager` |
| Monitors | Live in EKO | Reconciled with the application scheduler |
| Themes | Live in EKO GUI | Selectable, applied as CSS variables, and persisted |
| Output Styles | Live in EKO | Projected into agent context and persisted |

```
Core framework:  React Agent loop, tool execution, context management
Plugin:          a directory-packaged capability unit, declares components via manifest, loaded on demand
```

---

## Problem It Solves

Without a plugin system, extending the Agent requires:
- **Modifying core code**: every new capability means changing framework source
- **Tight coupling**: custom tools, hooks, and MCP configs scattered across the codebase
- **Hard to distribute**: no way to package a set of capabilities for team or community sharing

The plugin system unifies extension points into "one directory + one manifest" for plug-and-play.

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                     Plugin System                                │
│                                                                  │
│   PluginRegistry                                                 │
│   ┌──────────────────────────────────────────────────────────┐  │
│   │  scan_all() → discover installed plugins                  │  │
│   │  install()  → install from local / Git                    │  │
│   │  enable() / disable() → toggle plugins                    │  │
│   │  resolve_dependencies() → topological sort                │  │
│   └──────────────────────────────────────────────────────────┘  │
│       │                                                          │
│       ▼                                                          │
│   PluginIntegrator                                               │
│   ┌──────────────────────────────────────────────────────────┐  │
│   │  wire_all() → wire components in dependency order         │  │
│   │  ┌──────────┐  ┌──────────┐  ┌──────────┐               │  │
│   │  │ Skills   │  │ Hooks    │  │ MCP      │               │  │
│   │  │ → Agent  │  │ → Agent  │  │ → Agent  │               │  │
│   │  └──────────┘  └──────────┘  └──────────┘               │  │
│   └──────────────────────────────────────────────────────────┘  │
│       │                                                          │
│       ▼                                                          │
│   PluginVariables                                                │
│   ┌──────────────────────────────────────────────────────────┐  │
│   │  ${ECHO_PLUGIN_ROOT} → plugin install directory           │  │
│   │  ${ECHO_PLUGIN_DATA} → persistent data directory          │  │
│   │  ${ECHO_PROJECT_DIR} → project root                       │  │
│   │  ${user_config.*}    → user configuration values          │  │
│   └──────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
```

---

## Plugin Manifest Format

Every plugin must have `.echo-plugin/manifest.yaml` at its root:

```yaml
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
  agents:
    - "./agents/reviewer.md"
    - "./agents/analyst.md"
  hooks: "./hooks/hooks.yaml"
  mcp_servers: "./.mcp.json"

config:
  api_endpoint:
    type: string
    title: "API Endpoint"
    description: "Data service address"
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
```

### Manifest Fields

| Field | Required | Description |
|-------|----------|-------------|
| `name` | ✓ | Unique identifier, kebab-case (lowercase letters, digits, hyphens) |
| `display_name` | | Human-readable name, falls back to `name` |
| `version` | | Semantic version string, e.g. `"1.2.0"`, defaults to `"0.0.0"` |
| `description` | | Brief description of the plugin's purpose |
| `author` | | Author info (`name`, `email`, `url`) |
| `license` | | License identifier (e.g. `"MIT"`, `"Apache-2.0"`) |
| `keywords` | | Discovery tags for search and filtering |
| `components` | | Component declarations — paths relative to plugin root |
| `config` | | User-configurable options, prompted at install time |
| `dependencies` | | Other plugins this one depends on |
| `default_enabled` | | Whether the plugin starts enabled (default `true`) |

### Component Path Rules

All component paths must:
- Start with `./` (e.g. `"./skills/"`)
- Not contain `..` (no path traversal — prevents escaping the plugin root)

Path values accept a single string or an array of strings:

```yaml
components:
  skills: "./skills/"              # single path
  agents:                           # multiple paths
    - "./agents/reviewer.md"
    - "./agents/analyst.md"
```

When omitted, some components use default paths:
- `skills` → `./skills/`
- `mcp_servers` → `./.mcp.json`

### User Configuration Types

| Type | Description |
|------|-------------|
| `string` | Free-form text, set `multiple: true` to allow arrays |
| `number` | Numeric value, optionally with `min` / `max` bounds |
| `boolean` | Boolean toggle |
| `directory` | Directory path (validated for existence) |
| `file` | File path (validated for existence) |

Common config entry properties:
- `sensitive: true` — masks the UI input and keeps the resolved value redacted
  from Hook command diagnostics
- `required: true` — field must be provided
- `default` — default value when the user provides nothing

The registry resolves defaults, validates types/required values/ranges and
file paths, then persists the validated `user_config` in the application plugin
registry file. EKO protects that local file with owner-only permissions on Unix.
Values are exposed to component loaders as `${user_config.KEY}`; they are not
advertised as an operating-system keychain.

---

## PluginScope: Installation Scopes

Plugins can be installed to three different scopes:

| Scope | Path | Use Case |
|-------|------|----------|
| `User` | `~/.echo-agent/plugins/` | Personal plugins, available in all projects |
| `Project` | `.echo-agent/plugins/` | Team-shared plugins committed via VCS |
| `Local` | `.echo-agent/plugins.local/` | Project-private plugins, gitignored |

```rust
use echo_agent::plugin::{PluginScope, InstallSource};

// Parse scope from string
let scope = PluginScope::from_arg("user").unwrap();    // "user" | "project" | "local"

// Resolve filesystem path
let dir = scope.resolve_dir(Some(Path::new("/home/user/my-project")));
// User    → /home/user/.echo-agent/plugins/
// Project → /home/user/my-project/.echo-agent/plugins/
// Local   → /home/user/my-project/.echo-agent/plugins.local/
```

---

## PluginRegistry API

`PluginRegistry` is the central hub for plugin management — discovery, installation, uninstallation, enable/disable, and dependency resolution.

### Creating a Registry

```rust
use echo_agent::plugin::PluginRegistry;

// Default paths (~/.echo-agent/plugins/)
let mut registry = PluginRegistry::new(None);

// With project root (resolves Project/Local scopes)
let mut registry = PluginRegistry::new(Some(PathBuf::from("/home/user/my-project")));

// Custom paths (for testing)
let mut registry = PluginRegistry::with_paths(
    PathBuf::from("/tmp/registry.json"),
    PathBuf::from("/tmp/data"),
    Some(PathBuf::from("/tmp/project")),
);
```

### Scanning for Plugins

```rust
// Scan all scopes, load installed plugins
let count = registry.scan_all().unwrap();
println!("Discovered {} plugins", count);
```

Scanning logic: iterates each scope directory looking for subdirectories containing `.echo-plugin/manifest.yaml`.

### Installation

```rust
use echo_agent::plugin::{InstallSource, PluginScope};

// Install from a local directory
let source = InstallSource::Local(PathBuf::from("/path/to/my-plugin"));
let plugin_id = registry.install(&source, PluginScope::User)?;

// Install from a Git repository (HTTPS or SSH)
let source = InstallSource::parse("https://github.com/echo/data-plugin.git");
let plugin_id = registry.install(&source, PluginScope::Project)?;

// Auto-detect install source
let source = InstallSource::parse("./my-plugin");       // → Local
let source = InstallSource::parse("https://...git");    // → Git
```

### Uninstallation

```rust
// Uninstall and remove data directory
registry.uninstall("data-analysis-pack", false)?;

// Uninstall but keep data directory
registry.uninstall("data-analysis-pack", true)?;
```

### Enable / Disable

```rust
// Disable a plugin (files and data preserved)
registry.disable("data-analysis-pack")?;

// Re-enable
registry.enable("data-analysis-pack")?;
```

Enable/disable state is persisted to `registry.json` and restored on restart.
Validated user configuration is persisted in the same registry:

```rust
registry.configure(
    "data-analysis-pack",
    HashMap::from([(
        "api_endpoint".to_string(),
        serde_json::json!("http://localhost:8080"),
    )]),
)?;
```

A plugin with missing required configuration starts disabled and cannot be
enabled until `configure` succeeds.

### Querying

```rust
// List all installed plugins
for entry in registry.list() {
    println!("{} v{} [{}]",
        entry.manifest.name,
        entry.manifest.version,
        if entry.enabled { "enabled" } else { "disabled" }
    );
}

// List enabled plugins only
for entry in registry.list_enabled() {
    println!("{}", entry.manifest.display_name());
}

// Search by keyword (matches name, description, keywords)
let results = registry.search("polars");

// Get a single plugin's details
if let Some(entry) = registry.get("data-analysis-pack") {
    println!("Install path: {}", entry.root.display());
    println!("Scope: {}", entry.scope);
}

// Total count
println!("{} plugins installed", registry.count());
```

### Dependency Resolution

```rust
// Topological sort: dependencies come first
let ordered = registry.resolve_dependencies()?;
// e.g. A depends on B, B depends on C → returns [C, B, A]

// Error cases:
// - Missing dependency: "Plugin 'a' depends on 'b' which is not installed"
// - Circular dependency: "Circular dependency detected among plugins"
```

---

## Plugin Lifecycle

A plugin goes through these stages from installation to removal:

```
Install → Scan & Discover → Resolve Components → Wire into Agent → Enable/Disable → Uninstall
  │                                              │
  │  install()                                   │  enable() / disable()
  │  scan_all()                                  │
  │  resolve_components()                        │
  │  PluginIntegrator::wire_all()                │
  ▼                                              ▼
```

### Lifecycle Callbacks (PluginLifecycle trait)

For plugins that need code-level lifecycle management, implement the `PluginLifecycle` trait:

```rust
use echo_agent::plugin::PluginLifecycle;

struct MyPluginLifecycle;

impl PluginLifecycle for MyPluginLifecycle {
    /// Called once after the plugin is loaded and components are registered
    fn init(&self) -> Result<(), String> {
        // Start background processes, open connections, initialize caches
        Ok(())
    }

    /// Called when the plugin is enabled (or at startup if default_enabled: true)
    fn activate(&self) -> Result<(), String> {
        // Start monitors, activation-specific logic
        Ok(())
    }

    /// Called when the plugin is disabled
    fn deactivate(&self) -> Result<(), String> {
        // Stop background processes, release resources
        Ok(())
    }

    /// Called at agent shutdown
    fn shutdown(&self) -> Result<(), String> {
        // Flush buffers, close connections, save state to ${ECHO_PLUGIN_DATA}
        Ok(())
    }
}
```

Lifecycle flow:

```text
load → init → activate ⇄ deactivate → shutdown
                   ↑          ↓
                   └──────────┘  (can cycle on reload)
```

`PluginLifecycleManager` owns registered callbacks. Native/plugin-host code
registers its callback through `PluginRuntimeService::register_lifecycle`; a
declarative manifest does not name or dynamically instantiate Rust callback
types. EKO's shared `PluginRuntimeService` drives the manager for GUI, TUI, and
CLI alike.

Every candidate replacement is bracketed atomically: active callbacks are
deactivated before old components are unwired, candidate components are wired,
then callbacks for the candidate enabled set are activated. A deactivation,
wiring, or activation failure aborts publication, restores the previous
component/LSP/monitor set, and reactivates its callbacks. Uninstall unregisters
the callback after `deactivate`/`shutdown`, so reinstall can register it again.
`init` runs once per registration, while `activate`/`deactivate` may cycle on
successful reloads.

---

## Component Wiring

`PluginIntegrator` wires plugin components into the Agent's subsystems:

```rust
use echo_agent::plugin::PluginIntegrator;

let integrator = PluginIntegrator::new();
let result = integrator.wire_all(&mut agent, &mut registry).await;

println!("Loaded {} skills", result.skills_loaded.len());
println!("Registered {} hook sources", result.hooks_registered.len());
println!("Connected {} MCP servers", result.mcp_connected.len());

if !result.is_ok() {
    for err in &result.errors {
        eprintln!("Wiring error: {}", err);
    }
}

println!("{} components wired total", result.total_wired());
```

Wiring order:

1. `resolve_dependencies()` determines plugin load order
2. `resolve_components()` resolves paths for each enabled plugin
3. Components are wired by type:

| Component | Wiring Method |
|-----------|--------------|
| Skills | `agent.load_skills_from_dir()` |
| Hooks | `hook_registry.register("plugin:{name}", ...)` |
| MCP Servers | `agent.load_mcp_from_file()` |
| Agents / LSP / Monitors / Themes / Output Styles | Returned at the framework/application adapter boundary; EKO parses and activates them in `PluginRuntimeService` |

`PluginIntegrator::total_wired()` intentionally counts only framework-owned
Skills/Hooks/MCP components. The EKO application reports its own live Agent,
LSP, monitor, theme, and output-style counts in the same reload summary.

Theme selection activates and persists a runtime preference.
GUI and TUI both apply the selected plugin theme immediately and return to their
built-in theme when it is cleared, disabled, uninstalled, or disappears during
reload. Selecting a built-in GUI theme first deactivates the plugin preference,
so DOM variables, frontend state, TUI rendering, and persisted backend state do
not diverge. Output-style activation follows the same persisted preference
model and updates the replaceable Agent context projection.

`wire_all` is the only component wiring authority. It returns source-owned
receipts and compensates the entire candidate generation if any framework
component fails. Use `PluginIntegrator::unwire` with those receipts during
disable, replacement, or shutdown.

---

## Variable Substitution

Plugin configs support variable placeholders, replaced at runtime with actual paths or values.

### Built-in Variables

| Variable | Value |
|----------|-------|
| `${ECHO_PLUGIN_ROOT}` | Absolute path to the plugin's install directory |
| `${ECHO_PLUGIN_DATA}` | Persistent data directory (survives updates) |
| `${ECHO_PROJECT_DIR}` | Project root directory |

### User Configuration Variables

Reference user config values declared in the manifest's `config` section via `${user_config.KEY}`:

```yaml
# manifest.yaml
config:
  api_endpoint:
    type: string
    title: "API Endpoint"
    default: "http://localhost:8080"
```

Use in component configs:

```json
{
  "server": {
    "url": "${user_config.api_endpoint}/api/v1"
  }
}
```

Substitution runs before parsing every text-based plugin component: Hooks, MCP,
Agent definitions, LSP, monitors, themes, output styles, and the complete
contents of plugin-owned `SKILL.md` files. This ordering means variables in a
Skill's YAML frontmatter Hook actions resolve exactly like variables in its
Markdown instructions.

### Environment Variables

`${ENV_VAR}` patterns are substituted from OS environment variables. Unknown variables are left as-is.

### Programmatic Usage

```rust
use echo_agent::plugin::PluginVariables;
use std::collections::HashMap;

let vars = PluginVariables::new(
    "my-plugin",
    PathBuf::from("/home/user/.echo-agent/plugins/my-plugin"),
    PathBuf::from("/home/user/my-project"),
);

// Add user config
let mut config = HashMap::new();
config.insert("api_endpoint".into(), "http://localhost:9090".into());
let vars = vars.with_user_config(config);

// Substitute variables
let result = vars.substitute("run ${ECHO_PLUGIN_ROOT}/scripts/start.sh");
// → "run /home/user/.echo-agent/plugins/my-plugin/scripts/start.sh"

let result = vars.substitute("connect to ${user_config.api_endpoint}");
// → "connect to http://localhost:9090"

// Resolve relative paths
let abs = vars.resolve_path("./skills/my-skill");
// → /home/user/.echo-agent/plugins/my-plugin/skills/my-skill

// Ensure data directory exists
vars.ensure_data_dir()?;
```

### Exporting as Environment Variables

Export plugin variables as process environment variables (for hook scripts and subprocesses):

```rust
use echo_agent::plugin::variables::export_to_env;

// ⚠️ Must be called during single-threaded initialization (set_var is not thread-safe)
export_to_env(&vars);
// Sets: ECHO_PLUGIN_ROOT, ECHO_PLUGIN_DATA, ECHO_PROJECT_DIR
// User config: ECHO_PLUGIN_OPTION_{KEY} (uppercased)
```

---

## Security

### Git Clone Restrictions

EKO accepts encrypted Git transports chosen by the local user: HTTPS, `ssh://`,
and SCP-style `git@host:path` URLs. It rejects cleartext HTTP/Git and malformed
inputs:

```rust
// Allowed
InstallSource::parse("https://github.com/echo/plugin.git")
InstallSource::parse("git@github.com:echo/private-plugin.git")

// Rejected
InstallSource::parse("file:///path/to/plugin")     // use Local instead
InstallSource::parse("git://host/repo")            // cleartext transport
InstallSource::parse("http://host/repo")           // cleartext transport
```

This is a local trusted-extension boundary, not a public multi-tenant service:
private and loopback Git hosts are valid when the user configures them.

### Path Traversal Protection

The manifest validator rejects all component paths containing `..`:

```yaml
# ❌ Validation fails
components:
  skills: "../shared-skills/"   # path traversal!

# ✅ Correct
components:
  skills: "./skills/"
```

### Variable Name Validation

When exporting environment variables, user config keys are restricted to `[A-Z0-9_]` characters, preventing environment variable injection:

```rust
// Valid:   "API_ENDPOINT" → ECHO_PLUGIN_OPTION_API_ENDPOINT
// Invalid: "api;rm -rf /" → skipped with warning
```

### Manifest Validation

`PluginManifest::validate()` performs comprehensive checks:

```rust
let manifest = PluginManifest::from_file(&path)?;
let errors = manifest.validate();

// Checks:
// - name is non-empty and kebab-case
// - version is valid semver
// - all component paths start with ./ and don't contain ..
// - config keys are valid identifiers
// - 'multiple' is only used with string type
// - dependency names are kebab-case

if !errors.is_empty() {
    for e in &errors {
        eprintln!("{}: {}", e.field, e.message);
    }
}
```

---

## Example: Creating a Plugin Manifest

Creating a simple code review plugin:

### Directory Structure

```
code-review-plugin/
├── .echo-plugin/
│   └── manifest.yaml
├── skills/
│   └── code-review/
│       ├── SKILL.md
│       └── references/
│           └── checklist.md
├── hooks/
│   └── hooks.yaml
└── .mcp.json
```

### .echo-plugin/manifest.yaml

```yaml
name: code-review-plugin
display_name: "Code Review Plugin"
version: "1.0.0"
description: "Automated code review with custom checklists and security scanning"
author:
  name: "My Team"
license: MIT
keywords: [code-review, security, quality]

components:
  skills: "./skills/"
  hooks: "./hooks/hooks.yaml"
  mcp_servers: "./.mcp.json"

config:
  strict_mode:
    type: boolean
    title: "Strict Mode"
    description: "When enabled, any warning blocks the commit"
    default: false
  exclude_patterns:
    type: string
    title: "Exclude Patterns"
    description: "File glob patterns to skip during review"
    multiple: true
    default: ["*.generated.*", "vendor/**"]

default_enabled: true
```

### hooks/hooks.yaml

```yaml
hooks:
  PreToolUse:
    - matcher: "Bash"
      hooks:
        - type: prompt
          prompt: "Before executing, verify the command won't affect repository state"
  PostToolUse:
    - matcher: "*"
      hooks:
        - type: command
          command: "${ECHO_PLUGIN_ROOT}/scripts/log_tool_usage.sh"
          timeout: 3
```

---

## Example: Programmatically Installing and Using a Plugin

```rust
use echo_agent::prelude::*;
use echo_agent::plugin::{
    PluginRegistry, PluginScope, InstallSource,
    PluginIntegrator, PluginVariables,
};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Create the Agent
    let config = AgentConfig::new("qwen3-max", "assistant", "You are a helpful assistant")
        .enable_tool(true);
    let mut agent = ReactAgent::new(config);

    // 2. Create the plugin registry
    let project_root = PathBuf::from("/home/user/my-project");
    let mut registry = PluginRegistry::new(Some(project_root.clone()));

    // 3. Scan for installed plugins
    let count = registry.scan_all()?;
    println!("Discovered {} installed plugins", count);

    // 4. Install a plugin from a local directory
    let source = InstallSource::Local(PathBuf::from("/path/to/code-review-plugin"));
    let plugin_id = registry.install(&source, PluginScope::User)?;
    println!("Installed: {}", plugin_id);

    // 5. Install from Git
    let source = InstallSource::parse("https://github.com/echo/data-plugin.git");
    let plugin_id = registry.install(&source, PluginScope::Project)?;
    println!("Installed: {}", plugin_id);

    // 6. List enabled plugins
    for entry in registry.list_enabled() {
        println!("- {} v{}: {}",
            entry.manifest.name,
            entry.manifest.version,
            entry.manifest.description
        );
    }

    // 7. Wire all plugins into the Agent
    let integrator = PluginIntegrator::new();
    let result = integrator.wire_all(&mut agent, &mut registry).await;
    println!("Wiring complete: {} components", result.total_wired());

    if !result.is_ok() {
        for err in &result.errors {
            eprintln!("Warning: {}", err);
        }
    }

    // 8. Use variable substitution
    if let Some(entry) = registry.get("code-review-plugin") {
        let vars = PluginVariables::new(
            "code-review-plugin",
            entry.root.clone(),
            project_root,
        );
        let cmd = vars.substitute("${ECHO_PLUGIN_ROOT}/scripts/run.sh");
        println!("Running: {}", cmd);
    }

    // 9. Disable an unneeded plugin
    registry.disable("data-plugin")?;

    // 10. Uninstall a plugin
    registry.uninstall("data-plugin", false)?;

    // 11. Agent now has all plugin capabilities — use normally
    let response = agent.execute("Please review this code").await?;
    println!("{}", response);

    Ok(())
}
```

---

## Plugin Data Directory

Each plugin has its own persistent data directory that survives updates:

```
~/.echo-agent/plugins/data/
├── code-review-plugin/     ← code-review-plugin's data
├── data-analysis-pack/     ← data-analysis-pack's data
└── my-plugin/              ← my-plugin's data
```

Data directory names are auto-sanitized from the plugin name (non-`[a-zA-Z0-9_-]` characters replaced with `-`).

When uninstalling, you can choose to preserve the data directory:

```rust
registry.uninstall("my-plugin", true)?;  // keep_data = true
```
