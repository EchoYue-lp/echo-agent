//! Plugin system — re-exports core types and provides integration layer.
//!
//! This module bridges the plugin types defined in `echo-core` with
//! the concrete subsystems (`SkillRegistry`, `HookRegistry`, `McpManager`).
//!
//! ## Quick start
//!
//! ```rust,no_run
//! use echo_agent::plugin::{PluginRegistry, PluginScope, InstallSource, PluginIntegrator};
//!
//! let mut registry = PluginRegistry::new(None);
//! registry.scan_all().unwrap();
//!
//! for entry in registry.list_enabled() {
//!     println!("{}: {}", entry.manifest.name, entry.manifest.description);
//! }
//! ```

// ── Re-exports from echo-core ──────────────────────────────────────────

pub use echo_core::plugin::{
    InstallSource, PluginAuthor, PluginCapability, PluginComponents, PluginDependency, PluginEntry,
    PluginId, PluginLifecycle, PluginManifest, PluginRegistry, PluginScope, PluginUserConfigEntry,
    PluginUserConfigType, PluginVariables, ResolvedComponents, plugin_data_base_dir,
    set_plugin_data_base_dir, set_plugin_data_base_dir_name,
};

use std::path::PathBuf;

/// Result of wiring plugin components into an agent.
#[derive(Debug, Default)]
pub struct PluginWiringResult {
    /// Names of skills loaded.
    pub skills_loaded: Vec<String>,
    /// Names of plugins whose hooks were registered.
    pub hooks_registered: Vec<String>,
    /// Names of MCP servers connected.
    pub mcp_connected: Vec<String>,
    /// Names of subagent definitions registered (late-binding: the application
    /// layer supplies the executable instance afterwards).
    pub agents_registered: Vec<String>,
    /// LSP config files discovered but not yet wired (TODO: framework
    /// `ReactAgent` does not hold an `LspManager`; the application layer
    /// constructs one and feeds it `LspConfig::from_file`).
    pub lsp_discovered: Vec<String>,
    /// Monitor config files discovered (TODO: no framework runtime consumer).
    pub monitors_discovered: Vec<String>,
    /// Theme files discovered (TODO: UI-layer consumer, not framework).
    pub themes_discovered: Vec<String>,
    /// Output-style files discovered (TODO: output-format consumer, not framework).
    pub output_styles_discovered: Vec<String>,
    /// Errors encountered during wiring.
    pub errors: Vec<String>,
}

impl PluginWiringResult {
    /// Whether wiring completed without errors.
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }

    /// Total number of components wired into the agent.
    ///
    /// Counts the four assembled categories (skills, hooks, MCP, subagent
    /// definitions). Discovery-only categories (lsp/monitors/themes/output
    /// styles) are excluded — they have no framework consumer yet and are
    /// reported via their own `*_discovered` fields.
    pub fn total_wired(&self) -> usize {
        self.skills_loaded.len()
            + self.hooks_registered.len()
            + self.mcp_connected.len()
            + self.agents_registered.len()
    }
}

/// Wires plugin components into the agent's subsystems.
///
/// The integrator reads resolved component paths from a `PluginEntry`
/// and registers them with the appropriate subsystem:
///
/// | Component | Target | Status |
/// |-----------|--------|--------|
/// | Skills | `SkillRegistry` via `ReactAgent::load_skills_from_dir` | assembled |
/// | Hooks | `HookRegistry` via `HookRegistry::register` | assembled |
/// | MCP servers | `McpManager` via `ReactAgent::load_mcp_from_file` | assembled |
/// | Agents | `SubagentRegistry` via `ReactAgent::register_subagent_definition` (definition only; app supplies instance) | assembled (late-binding) |
/// | LSP servers | discovered path; `ReactAgent` holds no `LspManager` | TODO |
/// | Monitors / Themes / Output styles | discovered paths; no framework runtime consumer | TODO |
///
/// This struct lives in the facade crate because it needs access to
/// types from multiple workspace crates.
pub struct PluginIntegrator;

impl PluginIntegrator {
    pub fn new() -> Self {
        Self
    }

    /// Wire all enabled plugin components into a `ReactAgent`.
    ///
    /// This is the primary entry point. It:
    /// 1. Scans all plugin scopes and resolves dependencies.
    /// 2. For each enabled plugin, resolves component paths.
    /// 3. Wires skills, hooks, MCP servers, and subagent **definitions**
    ///    into the agent; reports discovered-but-unconsumed LSP/monitors/
    ///    themes/output styles.
    ///
    /// # Subagent definitions (agents)
    ///
    /// Each `.md` agent file is parsed by the framework-minimal
    /// [`parse_subagent_md`] and registered via
    /// [`ReactAgent::register_subagent_definition`](crate::agent::react::ReactAgent::register_subagent_definition)
    /// — a **definition-only**, late-binding path (no executable instance).
    /// The application layer, which owns the prompt-compiler / tool-filter /
    /// sandbox wiring, is expected to subsequently supply the real instance
    /// (or a factory) under the same name. This keeps the framework free of
    /// product-specific subagent construction while still making plugin
    /// agents discoverable end-to-end.
    ///
    /// # Discovery-only components
    ///
    /// LSP servers, monitors, themes, and output styles are resolved and
    /// reported (`*_discovered`) but not assembled: `ReactAgent` holds no
    /// `LspManager`, and monitors/themes/output styles have no framework
    /// runtime consumer. See the [`PluginIntegrator`] table for the TODOs.
    pub async fn wire_all(
        &self,
        agent: &mut crate::agent::react::ReactAgent,
        registry: &mut PluginRegistry,
    ) -> PluginWiringResult {
        let mut result = PluginWiringResult::default();

        // Resolve dependency order
        let ordered_ids = match registry.resolve_dependencies() {
            Ok(ids) => ids,
            Err(e) => {
                result
                    .errors
                    .push(format!("Dependency resolution failed: {e}"));
                return result;
            }
        };

        // Collect components from all enabled plugins
        let mut skill_dirs: Vec<PathBuf> = Vec::new();
        let mut hooks_defs: Vec<(
            String,
            String,
            echo_execution::skills::hooks::HooksDefinition,
        )> = Vec::new();
        let mut mcp_files: Vec<PathBuf> = Vec::new();
        // (plugin_id, source_tag, file_path) — source_tag marks provenance
        // (e.g. "plugin:my-plugin") for the framework SubagentKind::Plugin.
        let mut agent_files: Vec<(String, String, PathBuf)> = Vec::new();

        for plugin_id in &ordered_ids {
            // Extract entry info before mutable borrow
            let entry_info = registry
                .get(plugin_id)
                .map(|e| (e.enabled, e.root.display().to_string()));

            let Some((enabled, root_display)) = entry_info else {
                continue;
            };
            if !enabled {
                continue;
            }

            let resolved = match registry.resolve_components(plugin_id) {
                Ok(r) => r,
                Err(e) => {
                    result
                        .errors
                        .push(format!("Plugin '{plugin_id}' component resolution: {e}"));
                    continue;
                }
            };

            // Collect skill dirs
            skill_dirs.extend(resolved.skill_dirs.iter().cloned());

            // Collect hooks
            if let Some(ref hooks_file) = resolved.hooks_file
                && let Ok(content) = std::fs::read_to_string(hooks_file)
            {
                match serde_yaml_ng::from_str::<echo_execution::skills::hooks::HooksDefinition>(
                    &content,
                ) {
                    Ok(def) => {
                        hooks_defs.push((plugin_id.clone(), root_display, def));
                    }
                    Err(e) => {
                        result
                            .errors
                            .push(format!("Plugin '{plugin_id}' hooks YAML parse: {e}"));
                    }
                }
            }

            // Collect MCP files
            if let Some(ref mcp_file) = resolved.mcp_config_file {
                mcp_files.push(mcp_file.clone());
            }

            // Collect agent definition files (late-binding; wired below)
            for file in &resolved.agent_files {
                agent_files.push((
                    plugin_id.clone(),
                    format!("plugin:{plugin_id}"),
                    file.clone(),
                ));
            }

            // Discovery-only: report but do not assemble (no framework consumer).
            if let Some(ref lsp_file) = resolved.lsp_config_file {
                result.lsp_discovered.push(lsp_file.display().to_string());
            }
            if let Some(ref monitors_file) = resolved.monitors_file {
                result
                    .monitors_discovered
                    .push(monitors_file.display().to_string());
            }
            for theme_file in &resolved.theme_files {
                result
                    .themes_discovered
                    .push(theme_file.display().to_string());
            }
            for style_file in &resolved.output_style_files {
                result
                    .output_styles_discovered
                    .push(style_file.display().to_string());
            }
        }

        // Wire skills
        for dir in &skill_dirs {
            match agent.load_skills_from_dir(dir).await {
                Ok(names) => {
                    result.skills_loaded.extend(names);
                }
                Err(e) => {
                    result
                        .errors
                        .push(format!("Skills from {}: {e}", dir.display()));
                }
            }
        }

        // Wire hooks — use the plugin-source registration path so hooks carry
        // `HookSource::Plugin(name)`, distinct from skill/user-config sources
        // (audit P0-2). Previously this filed plugin hooks under
        // `HookSource::Skill("plugin:…")`, collapsing source identity.
        {
            let mut hook_reg = agent.hook_registry().write().await;
            for (plugin_name, source_dir, def) in &hooks_defs {
                hook_reg.register_plugin_hooks(plugin_name, source_dir, def.clone());
                result.hooks_registered.push(plugin_name.clone());
            }
        }

        // Wire MCP servers
        #[cfg(feature = "mcp")]
        {
            for mcp_file in &mcp_files {
                match agent.load_mcp_from_file(mcp_file).await {
                    Ok(clients) => {
                        for _c in &clients {
                            result.mcp_connected.push(mcp_file.display().to_string());
                        }
                    }
                    Err(e) => {
                        result
                            .errors
                            .push(format!("MCP from {}: {e}", mcp_file.display()));
                    }
                }
            }
        }

        // Wire subagent definitions — parse each `.md` and register a
        // definition-only entry (late-binding). The application layer supplies
        // the executable instance afterwards (see `wire_all` doc comment).
        #[cfg(feature = "subagent")]
        {
            for (plugin_id, source_tag, file) in &agent_files {
                let content = match std::fs::read_to_string(file) {
                    Ok(c) => c,
                    Err(e) => {
                        result.errors.push(format!("Agent {}: {e}", file.display()));
                        continue;
                    }
                };
                match parse_subagent_md(&content, Some(source_tag)) {
                    Ok(def) => {
                        let name = def.name.clone();
                        agent.register_subagent_definition(def);
                        result.agents_registered.push(name);
                    }
                    Err(e) => {
                        result.errors.push(format!(
                            "Plugin '{plugin_id}' agent {}: {e}",
                            file.display()
                        ));
                    }
                }
            }
        }
        // Without the `subagent` feature, agent files are still discoverable
        // (their paths were resolved) but cannot be wired — surface a single
        // notice per result so callers aren't left guessing.
        #[cfg(not(feature = "subagent"))]
        if !agent_files.is_empty() {
            result.errors.push(format!(
                "{} plugin agent definition(s) discovered but the `subagent` feature is \
                 disabled; skipping registration",
                agent_files.len()
            ));
        }

        result
    }

    /// Wire only skills from resolved components into the agent.
    pub async fn wire_skills(
        &self,
        agent: &mut crate::agent::react::ReactAgent,
        skill_dirs: &[PathBuf],
    ) -> Vec<String> {
        let mut loaded = Vec::new();
        for dir in skill_dirs {
            if let Ok(names) = agent.load_skills_from_dir(dir).await {
                loaded.extend(names);
            }
        }
        loaded
    }

    /// Wire only hooks from resolved components into the agent.
    pub async fn wire_hooks(
        &self,
        agent: &crate::agent::react::ReactAgent,
        hooks: &[(
            String,
            String,
            echo_execution::skills::hooks::HooksDefinition,
        )],
    ) {
        let mut registry = agent.hook_registry().write().await;
        for (plugin_name, source_dir, def) in hooks {
            registry.register_plugin_hooks(plugin_name, source_dir, def.clone());
        }
    }

    /// Wire only MCP servers from resolved components into the agent.
    #[cfg(feature = "mcp")]
    pub async fn wire_mcp(
        &self,
        agent: &mut crate::agent::react::ReactAgent,
        mcp_files: &[PathBuf],
    ) -> Vec<String> {
        let mut connected = Vec::new();
        for file in mcp_files {
            if let Ok(clients) = agent.load_mcp_from_file(file).await {
                for _ in clients {
                    connected.push(file.display().to_string());
                }
            }
        }
        connected
    }
}

impl Default for PluginIntegrator {
    fn default() -> Self {
        Self::new()
    }
}

// ── Framework-minimal subagent `.md` parser ──────────────────────────────
//
// This parser is intentionally minimal: it extracts the framework-relevant
// subset of the agent-definition frontmatter (name / description / model /
// tools / tags) and treats the markdown body as the system prompt. Richer,
// EKO-specific frontmatter (readonly / worktree / workspace / team) stays in
// the application-layer loader (`echo-agent-app-core::subagent_loader`), which
// owns the full prompt-compiler / tool-filter / sandbox wiring needed to
// build a real subagent instance. The two layers do not duplicate semantics:
// this parser is the framework *discovery* surface; the app loader is the
// *construction* surface.
//
// The frontmatter convention (`---\n<yaml>\n---\n<body>`) is the same one
// used across the codebase (skills loader, evolution MEMORY.md, app
// subagent loader), so a single `.md` file is valid for both layers.

/// Parsed frontmatter for a plugin agent definition (framework subset).
#[cfg(feature = "subagent")]
#[derive(Debug, Default, serde::Deserialize)]
struct AgentFrontmatter {
    name: Option<String>,
    description: Option<String>,
    /// Optional model override (None/empty/"inherit" → inherit parent).
    #[serde(default)]
    model: Option<String>,
    /// Optional tool whitelist (None → inherit all parent tools).
    #[serde(default)]
    tools: Option<Vec<String>>,
    /// Discovery / filter tags.
    #[serde(default)]
    tags: Option<Vec<String>>,
}

/// Parse a plugin agent-definition `.md` into a framework
/// [`SubagentDefinition`].
///
/// `source_tag` (e.g. `"plugin:my-plugin"`) marks provenance on the
/// resulting [`SubagentKind::Plugin`] and is also used as the fallback name
/// when the frontmatter omits `name`.
///
/// Only `name` and `description` are required; the body is the system prompt.
/// Returns an error string suitable for inclusion in
/// [`PluginWiringResult::errors`].
///
/// Only available with the `subagent` feature (the framework
/// `SubagentDefinition` type lives under that feature).
#[cfg(feature = "subagent")]
pub fn parse_subagent_md(
    content: &str,
    source_tag: Option<&str>,
) -> Result<crate::agent::subagent::SubagentDefinition, String> {
    use crate::agent::subagent::types::ExecutionMode;
    use crate::agent::subagent::{SubagentBuilder, SubagentKind};

    let (fm_str, body) = split_frontmatter(content)?;
    let fm: AgentFrontmatter = if fm_str.trim().is_empty() {
        AgentFrontmatter::default()
    } else {
        serde_yaml_ng::from_str(fm_str)
            .map_err(|e| format!("agent frontmatter parse error: {e}"))?
    };

    // Resolve name: frontmatter wins; otherwise an explicit source_tag;
    // otherwise error (a nameless agent is undiscoverable).
    let name = fm
        .name
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .or_else(|| source_tag.map(|s| s.to_string()))
        .ok_or_else(|| "agent frontmatter missing required `name` field".to_string())?;

    let description = fm
        .description
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty())
        .ok_or_else(|| format!("agent `{name}` missing `description`"))?;

    let system_prompt = body.trim().to_string();

    let model = fm
        .model
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty() && m != "inherit");

    let mut builder = SubagentBuilder::new(&name)
        .description(&description)
        // Plugin agents default to Fork (independent, inherit-trailing context)
        // — matches the app loader's default for `.md` subagents.
        .fork_mode();
    builder = builder.kind(SubagentKind::Plugin {
        source: source_tag.unwrap_or("plugin").to_string(),
    });
    if !system_prompt.is_empty() {
        builder = builder.system_prompt(&system_prompt);
    }
    if let Some(model) = model {
        builder = builder.model(&model);
    }
    if let Some(tools) = fm.tools.filter(|v| !v.is_empty()) {
        builder = builder.tools(tools);
    }
    if let Some(tags) = fm.tags.filter(|v| !v.is_empty()) {
        builder = builder.tags(tags);
    }
    // Fork-mode default inherit_history(2) is kept; clear only if the body
    // explicitly requested otherwise (not exposed here — app loader handles
    // the richer field). ExecutionMode stays Fork.
    let _ = ExecutionMode::Fork;
    Ok(builder.build())
}

/// Split a `.md` document into `(frontmatter_yaml, markdown_body)`.
///
/// Mirrors the skills loader / app subagent loader convention:
/// - Requires a leading `---` on the first line (a BOM is tolerated).
/// - The closing `---` (on its own line) ends the frontmatter.
/// - Returns `(frontmatter_str, body_str)`; an empty document or one with no
///   closing delimiter yields an error.
///
/// Kept compiled even without the `subagent` feature so the unit tests below
/// (which exercise frontmatter splitting independently of subagent
/// construction) run under the default test matrix; the `allow(dead_code)`
/// silences the unused warning in non-subagent lib builds where
/// [`parse_subagent_md`] is configured out.
#[cfg_attr(not(feature = "subagent"), allow(dead_code))]
fn split_frontmatter(content: &str) -> Result<(&str, &str), String> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let after_open = content
        .strip_prefix("---")
        .ok_or_else(|| "missing leading `---` frontmatter delimiter".to_string())?;
    let after_open = after_open
        .strip_prefix(['\r', '\n'])
        .ok_or_else(|| "opening `---` must be on its own line".to_string())?;

    // Find the closing `\n---` delimiter on its own line.
    let mut search_from = 0;
    loop {
        let Some(relative) = after_open[search_from..].find("---") else {
            return Err("missing closing `---` frontmatter delimiter".to_string());
        };
        // The `---` must start at the beginning of a line (preceded by `\n`).
        let absolute = search_from + relative;
        let at_line_start =
            absolute == 0 || after_open.as_bytes().get(absolute - 1) == Some(&b'\n');
        if at_line_start {
            let fm = after_open.get(..absolute).unwrap_or("");
            // Skip the closing `---` and its trailing newline.
            let after_close = absolute + 3;
            let body = after_open.get(after_close..).unwrap_or("");
            let body = body
                .strip_prefix(['\r', '\n'])
                .or_else(|| body.strip_prefix('\n'))
                .unwrap_or(body);
            return Ok((fm, body));
        }
        search_from = absolute + 3;
    }
}

// ── Backward-compatible legacy trait ───────────────────────────────────

/// Legacy native plugin trait — for code-level extensions.
///
/// Prefer file-based plugins (with `manifest.yaml`) for most use cases.
/// This trait is retained for backward compatibility with code-level plugins
/// that need to inject custom Rust logic.
pub trait NativePlugin: Send + Sync {
    /// Unique plugin identifier.
    fn id(&self) -> &str;
    /// Human-readable name.
    fn name(&self) -> &str;
    /// What this plugin provides.
    fn capabilities(&self) -> Vec<PluginCapability>;
    /// Plugin version.
    fn version(&self) -> &str;

    /// Initialize the plugin. Called once at startup.
    fn init(&mut self) -> Result<(), String> {
        Ok(())
    }
    /// Shutdown the plugin. Called at agent shutdown.
    fn shutdown(&mut self) -> Result<(), String> {
        Ok(())
    }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_frontmatter_extracts_yaml_and_body() {
        let md = "---\nname: reviewer\ndescription: rev\n---\nYou are a reviewer.\n";
        let (fm, body) = split_frontmatter(md).unwrap();
        assert!(fm.contains("name: reviewer"));
        assert_eq!(body, "You are a reviewer.\n");
    }

    #[test]
    fn split_frontmatter_strips_leading_bom() {
        let md = "\u{feff}---\nname: x\n---\nbody";
        let (fm, body) = split_frontmatter(md).unwrap();
        assert!(fm.contains("name: x"));
        assert_eq!(body, "body");
    }

    #[test]
    fn split_frontmatter_handles_body_with_hr_separator() {
        // A `---` inside the body (markdown horizontal rule) must not be
        // mistaken for the closing delimiter: only a `---` at line start
        // immediately after a newline closes the frontmatter. Here the first
        // line-start `---` after the opening one is the real closer.
        let md = "---\nname: x\n---\nintro\n\n---\n\nmore";
        let (fm, body) = split_frontmatter(md).unwrap();
        assert!(fm.contains("name: x"));
        assert_eq!(body, "intro\n\n---\n\nmore");
    }

    #[test]
    fn split_frontmatter_errors_without_closing_delimiter() {
        let md = "---\nname: x\nbody without close";
        assert!(split_frontmatter(md).is_err());
    }

    #[test]
    fn split_frontmatter_errors_without_opening_delimiter() {
        assert!(split_frontmatter("no frontmatter here").is_err());
    }

    #[test]
    fn split_frontmatter_allows_empty_body() {
        let md = "---\nname: x\ndescription: y\n---\n";
        let (fm, body) = split_frontmatter(md).unwrap();
        assert!(fm.contains("name: x"));
        assert_eq!(body, "");
    }

    #[cfg(feature = "subagent")]
    #[test]
    fn parse_subagent_md_full_subset() {
        use crate::agent::subagent::SubagentKind;
        let md = "---\n\
                  name: data-explorer\n\
                  description: Explores datasets\n\
                  model: qwen3\n\
                  tools: [read_file, search]\n\
                  tags: [data, readonly]\n\
                  ---\n\
                  You are a data exploration specialist.";
        let def = parse_subagent_md(md, Some("plugin:data-pack")).unwrap();
        assert_eq!(def.name, "data-explorer");
        assert_eq!(def.description, "Explores datasets");
        assert_eq!(def.model.as_deref(), Some("qwen3"));
        assert_eq!(
            def.tool_filter.as_deref(),
            Some(["read_file".to_string(), "search".to_string()].as_slice())
        );
        assert_eq!(def.tags, vec!["data".to_string(), "readonly".to_string()]);
        assert_eq!(
            def.system_prompt.as_deref(),
            Some("You are a data exploration specialist.")
        );
        assert!(matches!(def.kind, SubagentKind::Plugin { .. }));
        if let SubagentKind::Plugin { source } = &def.kind {
            assert_eq!(source, "plugin:data-pack");
        }
    }

    #[cfg(feature = "subagent")]
    #[test]
    fn parse_subagent_md_inherit_model_drops_override() {
        let md = "---\nname: a\ndescription: d\nmodel: inherit\n---\nbody";
        let def = parse_subagent_md(md, None).unwrap();
        assert!(def.model.is_none(), "inherit must map to None");
    }

    #[cfg(feature = "subagent")]
    #[test]
    fn parse_subagent_md_requires_name_and_description() {
        let no_name = "---\ndescription: d\n---\nbody";
        assert!(parse_subagent_md(no_name, None).is_err());

        let no_desc = "---\nname: a\n---\nbody";
        assert!(parse_subagent_md(no_desc, None).is_err());

        // Missing frontmatter `name` is filled from source_tag fallback.
        let with_fallback = "---\ndescription: d\n---\nbody";
        let def = parse_subagent_md(with_fallback, Some("plugin:p")).unwrap();
        assert_eq!(def.name, "plugin:p");
    }

    #[cfg(feature = "subagent")]
    #[test]
    fn parse_subagent_md_empty_body_keeps_auto_description() {
        // Empty system prompt is allowed; SubagentBuilder auto-fills description.
        let md = "---\nname: a\ndescription: d\n---\n";
        let def = parse_subagent_md(md, None).unwrap();
        assert_eq!(def.name, "a");
        assert!(def.system_prompt.is_none());
    }
}
