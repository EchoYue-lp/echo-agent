//! demo56 - Agent Plugins 1.0 package discovery and lifecycle.
//!
//! All operations are local; no LLM calls are made.

use echo_agent::plugin::{
    AGENT_PLUGIN_SCHEMA_V1, InstallSource, PluginIntegrator, PluginRegistry, PluginScope,
};
use std::path::{Path, PathBuf};

fn create_plugin(base: &Path, name: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let root = base.join(name);
    std::fs::create_dir_all(root.join("skills/example"))?;
    std::fs::create_dir_all(root.join("hooks"))?;
    std::fs::write(
        root.join("skills/example/SKILL.md"),
        "---\nname: example\ndescription: Example plugin skill\n---\nUse the example workflow.\n",
    )?;
    std::fs::write(root.join("hooks/hooks.yaml"), "{}\n")?;
    let manifest = serde_json::json!({
        "$schema": AGENT_PLUGIN_SCHEMA_V1,
        "name": name,
        "version": "1.0.0",
        "description": "Agent Plugins 1.0 example",
        "displayName": "Demo Plugin",
        "config": {
            "endpoint": {
                "type": "string",
                "title": "Endpoint",
                "default": "https://example.com"
            }
        }
    });
    std::fs::write(
        root.join("plugin.json"),
        serde_json::to_string_pretty(&manifest)?,
    )?;
    Ok(root)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let sources = temporary.path().join("sources");
    let installed = temporary.path().join("installed");
    std::fs::create_dir_all(&sources)?;
    std::fs::create_dir_all(&installed)?;
    let source = create_plugin(&sources, "demo.tools")?;

    let (manifest, resolved) =
        PluginRegistry::validate_plugin_dir(&source).map_err(|errors| errors.join("; "))?;
    println!("Plugin: {}", manifest.display_name());
    println!("Version: {}", manifest.version_label());
    println!("Skills root: {}", resolved.skill_dirs.len());
    println!("Hooks: {}", resolved.hooks_file.is_some());

    let mut registry = PluginRegistry::with_paths(
        temporary.path().join("registry.json"),
        temporary.path().join("data"),
        Some(temporary.path().to_path_buf()),
    );
    let plugin_id = registry.install(&InstallSource::Local(source), PluginScope::Local)?;
    let integrator = PluginIntegrator::new();
    let prepared = integrator.prepare(&mut registry).await;
    println!(
        "Prepared generation {} ({})",
        prepared.generation(),
        prepared.identity()
    );
    let entry = registry
        .get(&plugin_id)
        .ok_or_else(|| format!("installed plugin '{plugin_id}' was not registered"))?;
    println!(
        "Installed {} with capabilities: {}",
        entry.manifest.name,
        entry
            .inferred_capabilities()
            .into_iter()
            .map(|capability| capability.display_name())
            .collect::<Vec<_>>()
            .join(", ")
    );

    registry.disable(&plugin_id)?;
    registry.enable(&plugin_id)?;
    registry.uninstall(&plugin_id, false)?;
    println!("Lifecycle complete");
    Ok(())
}
