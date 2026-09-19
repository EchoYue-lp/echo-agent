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

### MCP ownership and projections

The `name` key remains the portable local server name. At runtime the framework
qualifies it with its owner: direct configuration uses `McpServerOwner::Direct`,
while plugin wiring uses the stable `PreparedPlugin.id`. Thus two plugins may
both declare `filesystem` without sharing a connection or cleanup debt. Use the
typed `McpServerId`/`server_ids()` APIs when the owner matters; legacy string
APIs continue to address direct servers.

Direct tools retain `mcp__<server>__<tool>`. Plugin tools use
`mcp__plugin_<plugin>_<server>__<tool>` with a stable digest suffix when the
canonical Unicode or punctuation name is lossy. Resource selectors are opaque
`plugin:<base64url-plugin>:<base64url-server>` values. The resource directory
keeps the typed identity and does not infer an owner from a tool name or URI.
Direct names keep their legacy selector except reserved `plugin:`/`direct:`
prefixes, which are reversibly escaped as `direct:<base64url-name>`. A plugin
Hook `mcp_tool.server` remains a local name in the package and is qualified by
the prepared plugin owner before registration.

## Fixed local components

The remaining components are discovered from fixed root locations:

| Location | Consumer |
|---|---|
| `agents/*.md` | Subagent adapter |
| `hooks/hooks.yaml` | Hook registry |
| `lsp.yaml` | Embedding application's LSP manager |
| `monitors.yaml` | embedding application scheduler |
| `themes/*.json` | embedding application GUI/TUI theme catalogs |
| `output-styles/*.md` | embedding application system-context projection |

`scripts/` and `README.md` are package resources rather than automatically executed components. Skills and Hooks may reference scripts explicitly.

## Framework and application boundary

The reusable framework owns manifest parsing, Skills, MCP, plugin scopes/lifecycle, Hooks, Subagent definitions, and LSP adapter output. An embedding application discovers and converts only its product-specific `monitors.yaml`, `themes/`, and `output-styles/` files. The application integration does not duplicate dependency ordering, component ownership, or reload semantics.

## Discovery and lifecycle

The registry scans these scopes:

| Scope | Default location |
|---|---|
| User | `~/.echo-agent/plugins/<name>/plugin.json` |
| Project | `<project>/.echo-agent/plugins/<name>/plugin.json` |
| Local | `<project>/.echo-agent/plugins.local/<name>/plugin.json` |

Applications can override the plugin data base directory. embedding application sets it to `<application-data>`.

Loading proceeds in dependency order. Fatal manifest errors skip the package. Framework-parsed Skill, Hook, and MCP errors remain isolated at the component boundary and are recorded as error diagnostics; unreadable frozen Subagent or LSP documents are omitted the same way. Their product-specific syntax is validated by the embedding application's second preparation stage. Healthy siblings remain in the prepared plugin. Runtime replacement records ownership so disable, uninstall, and reload remove exactly the components contributed by each plugin.

`PluginIntegrator::prepare` captures one immutable `PreparedPluginSet` with a monotonic generation,
deterministic content identity, structured diagnostics, parsed Skills/Hooks/MCP, and owner-qualified
Subagent/LSP documents. `wire_prepared` and rollback perform no component file reads. Disk changes
become visible only after registry mutation or explicit invalidation. See [ADR 0012](../adr/0012-immutable-plugin-preparation.md).
Generation ordinals are allocated across all integrators in the process, so independent integrators
can publish successive snapshots to the same Agent without resetting their order.

Each `ReactAgent` owns one publication target. Obtain its cloneable handle through
`integrator.publication_target(&agent)` and call `target.wire_prepared(&mut agent, &prepared)`;
the returned receipt records the generation and identity. Withdraw with
`target.rollback(&mut agent, &receipt)` before publishing a newer generation. A stale prepared set,
foreign or altered receipt, or repeated publication is rejected before mutating registries.
Successful withdrawal can be repeated; after a newer generation publishes, the old receipt becomes
stale. Failed apply does not advance the active generation, and failed cleanup keeps its receipt
for retry. A cancelled apply's pending receipt can be obtained with
`target.pending_cleanup_receipt()` and settled before another publication. Cloned integrators
preparing for independent Agents do not share active publication state.
Each successful MCP connection enters the pending receipt before the next server begins. A name
that was absent before connection is reserved for cleanup across cancellation during its await;
failed new connections settle that name before the generation may publish. This does not change
the separate owner-qualified MCP identity work tracked by #75.

The set remains applicable when a component diagnostic is present. It is rejected only when a
generation-wide invariant, such as dependency ordering or generation allocation, prevents building
the complete immutable snapshot.

`PluginLifecycleManager` owns callback cleanup separately from component wiring. A failed
`deactivate`, `init`, or `activate` retains cleanup debt and blocks further callback activation
through `reconcile`, `activate_enabled`, and direct `activate`. The failed registration remains
available for a cleanup retry; successful deactivation settles its deactivation debt, while
failed initialization requires shutdown but not deactivation, and failed activation requires
successful `unregister` cleanup. A failed callback is
not treated as withdrawn merely because the desired enabled set changed. Failed `shutdown` remains
unsettled even if a later `deactivate` succeeds, and must be retried through `unregister`. See
[ADR 0060](../adr/0060-plugin-lifecycle-reconcile-settlement.md). Registry and component wiring
still require host-level coordination for a complete reload transaction.

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
`PluginScope` implements `FromStr`, including the documented short forms (`u`,
`p`, and `l`), so callers can parse configuration and command input with the
standard `scope.parse()` API.

## Design references

This design reuses the official Agent Plugins 1.0 [manifest](https://agent-plugins.org/plugin-authors/manifest), [Skills](https://agent-plugins.org/plugin-authors/skills), [MCP](https://agent-plugins.org/plugin-authors/mcp-servers), and [loading](https://agent-plugins.org/client-implementers/loading-and-discovery) contracts. embedding application intentionally uses fixed root locations for its additional local-assistant components instead of introducing client-extension namespaces.
