# Plugin System

EchoAgent uses the Agent Plugins 1.0 root manifest, Skills, and MCP conventions in a flat package tailored to a local personal assistant. Every supported component has one fixed location; there are no client-extension namespaces or component path declarations.

## Package layout

```text
my-plugin/
├── plugin.json
├── skills/
│   └── code-review/
│       └── SKILL.md
├── mcp.json
├── agents/
│   └── reviewer.md
├── hooks/
│   └── hooks.yaml
├── lsp.yaml
├── monitors.yaml
├── themes/
├── output-styles/
├── scripts/
└── README.md
```

`plugin.json` belongs at the package root. There is no `.echo-plugin/manifest.yaml` compatibility path.

## Manifest

```json
{
  "$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
  "name": "review.tools",
  "version": "1.0.0",
  "description": "Review workflows",
  "author": { "name": "Example Team" },
  "license": "MIT",
  "keywords": ["review"],
  "displayName": "Review Tools",
  "defaultEnabled": true,
  "config": {
    "endpoint": {
      "type": "string",
      "title": "Endpoint",
      "default": "https://example.com"
    }
  },
  "dependencies": [
    { "name": "base.tools", "version": ">=1.0.0" }
  ]
}
```

The portable identity fields follow Agent Plugins 1.0. EchoAgent also reads the root `displayName`, `defaultEnabled`, `config`, and `dependencies` fields. Unknown top-level fields are reported and ignored.

Plugin names contain 1-64 lowercase ASCII letters, digits, hyphens, or periods. They begin and end with an alphanumeric character and contain neither `--` nor `..`.

## Standard Skills

Skills use the fixed `skills/` root. Each immediate child is one skill:

```text
skills/<skill-name>/SKILL.md
```

EchoAgent does not recursively discover nested category directories for plugin Skills. An invalid Skill is skipped without disabling sibling Skills or other plugin components.

## Standard MCP

MCP configuration uses fixed root `mcp.json` and the Agent Plugins schema:

```json
{
  "$schema": "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
  "mcpServers": {
    "local-review": {
      "type": "stdio",
      "command": "node",
      "args": ["${PLUGIN_ROOT}/server.js"],
      "env": { "CACHE": "${PLUGIN_DATA}/cache" },
      "cwd": "${PLUGIN_ROOT}"
    },
    "remote-review": {
      "type": "streamable-http",
      "url": "https://example.com/mcp",
      "headers": { "X-Tenant": "public" }
    }
  }
}
```

Supported transports are `stdio`, `streamable-http`, and legacy `sse`. A stdio `command` is one bare executable name or a plugin-relative path beginning with `./`; it is never interpreted by a shell.

EchoAgent provides `PLUGIN_ROOT` and `PLUGIN_DATA` to stdio subprocesses. `${PLUGIN_ROOT}` and `${PLUGIN_DATA}` are expanded once in `args`, environment values, and `cwd`. They are not expanded in environment keys, commands, remote URLs, or HTTP headers. A plugin cannot override the two reserved environment variables.

An invalid top-level `mcp.json` disables only MCP for that plugin. An invalid, unavailable, or colliding server disables only that entry.

## Fixed local components

The remaining components are discovered from fixed root locations:

| Location | Consumer |
|---|---|
| `agents/*.md` | Subagent adapter |
| `hooks/hooks.yaml` | Hook registry |
| `lsp.yaml` | Embedding application's LSP manager |
| `monitors.yaml` | EKO scheduler |
| `themes/*.json` | EKO GUI/TUI theme catalogs |
| `output-styles/*.md` | EKO system-context projection |

`scripts/` and `README.md` are package resources rather than automatically executed components. Skills and Hooks may reference scripts explicitly.

## Framework and application boundary

The reusable framework owns manifest parsing, Skills, MCP, plugin scopes/lifecycle, Hooks, Subagent definitions, and LSP adapter output. EKO discovers and converts only its product-specific `monitors.yaml`, `themes/`, and `output-styles/` files. The application adapter does not duplicate dependency ordering, component ownership, or reload semantics.

## Discovery and lifecycle

The registry scans these scopes:

| Scope | Default location |
|---|---|
| User | `~/.echo-agent/plugins/<name>/plugin.json` |
| Project | `<project>/.echo-agent/plugins/<name>/plugin.json` |
| Local | `<project>/.echo-agent/plugins.local/<name>/plugin.json` |

Applications can override the plugin data base directory. EKO sets it to `~/.eko`.

Loading proceeds in dependency order. Fatal manifest errors skip the package. Component errors remain isolated at the smallest practical boundary. Runtime replacement records ownership so disable, uninstall, and reload remove exactly the components contributed by each plugin.

## API

```rust,no_run
use echo_agent::plugin::{InstallSource, PluginRegistry, PluginScope};

let mut registry = PluginRegistry::new(Some(std::env::current_dir()?));
registry.scan_all()?;

let id = registry.install(
    &InstallSource::Local("./review-tools".into()),
    PluginScope::Project,
)?;
registry.disable(&id)?;
registry.enable(&id)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

Use `PluginRegistry::validate_plugin_dir` before installation when a validation report is required.

## Design references

This design reuses the official Agent Plugins 1.0 [manifest](https://agent-plugins.org/plugin-authors/manifest), [Skills](https://agent-plugins.org/plugin-authors/skills), [MCP](https://agent-plugins.org/plugin-authors/mcp-servers), and [loading](https://agent-plugins.org/client-implementers/loading-and-discovery) contracts. EKO intentionally uses fixed root locations for its additional local-assistant components instead of introducing client-extension namespaces.
