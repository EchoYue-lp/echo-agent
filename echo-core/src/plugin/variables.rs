//! Plugin environment variables — path substitution in component configs.
//!
//! Three variables are available for substitution in skill content, agent content,
//! hook commands, monitor commands, and MCP/LSP server configurations:
//!
//! | Variable | Description |
//! |----------|-------------|
//! | `${ECHO_PLUGIN_ROOT}` | Absolute path to the plugin's install directory |
//! | `${ECHO_PLUGIN_DATA}` | Persistent data directory, survives updates |
//! | `${ECHO_PROJECT_DIR}` | Project root directory |
//!
//! Environment variables from the OS (`${ENV_VAR}`) are also substituted.
//!
//! Agent Plugins 1.0 MCP uses a narrower portable contract: `PLUGIN_ROOT` and
//! `PLUGIN_DATA` are provided by the strict `mcp.json` adapter and expanded
//! only in the standard-defined stdio fields.

use std::collections::HashMap;
use std::path::PathBuf;

/// Resolved plugin variables for path substitution.
#[derive(Debug, Clone)]
pub struct PluginVariables {
    /// Absolute path to the plugin's install directory.
    pub plugin_root: PathBuf,
    /// Persistent data directory for the plugin.
    pub plugin_data: PathBuf,
    /// Project root directory.
    pub project_dir: PathBuf,
    /// Additional user-defined variables from plugin config.
    pub user_config: HashMap<String, String>,
}

impl PluginVariables {
    /// Create a new set of variables for a plugin.
    pub fn new(plugin_name: &str, plugin_root: PathBuf, project_dir: PathBuf) -> Self {
        let plugin_data = Self::data_dir_for(plugin_name);
        Self {
            plugin_root,
            plugin_data,
            project_dir,
            user_config: HashMap::new(),
        }
    }

    /// Set user configuration values.
    pub fn with_user_config(mut self, config: HashMap<String, String>) -> Self {
        self.user_config = config;
        self
    }

    /// Set user configuration from persisted JSON values.
    pub fn with_json_user_config(mut self, config: &HashMap<String, serde_json::Value>) -> Self {
        self.user_config = config
            .iter()
            .map(|(key, value)| {
                let text = match value {
                    serde_json::Value::String(text) => text.clone(),
                    other => other.to_string(),
                };
                (key.clone(), text)
            })
            .collect();
        self
    }

    /// Override the persistent data directory selected by the default plugin
    /// base path. Embedded runtimes use this to keep variables aligned with
    /// their configured registry data directory.
    pub fn with_plugin_data(mut self, plugin_data: PathBuf) -> Self {
        self.plugin_data = plugin_data;
        self
    }

    /// Get the persistent data directory path for a named plugin.
    ///
    /// Located at `<plugin_base>/plugins/data/{plugin-name}/` (default
    /// `~/.echo-agent/plugins/data/...`; app-overridable via
    /// [`super::set_plugin_data_base_dir`]).
    pub fn data_dir_for(plugin_name: &str) -> PathBuf {
        let sanitized = plugin_name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect::<String>();

        super::plugins_child("data").join(sanitized)
    }

    /// Substitute all variables in a string.
    ///
    /// Replaces `${ECHO_PLUGIN_ROOT}`, `${ECHO_PLUGIN_DATA}`,
    /// `${ECHO_PROJECT_DIR}`, `${user_config.KEY}`, and `${ENV_VAR}`.
    pub fn substitute(&self, input: &str) -> String {
        let mut result = input.to_string();

        // Built-in variables
        result = result.replace("${ECHO_PLUGIN_ROOT}", &self.plugin_root.to_string_lossy());
        result = result.replace("${ECHO_PLUGIN_DATA}", &self.plugin_data.to_string_lossy());
        result = result.replace("${ECHO_PROJECT_DIR}", &self.project_dir.to_string_lossy());

        // User config variables: ${user_config.KEY}
        for (key, value) in &self.user_config {
            let placeholder = format!("${{user_config.{key}}}");
            result = result.replace(&placeholder, value);
        }

        // Environment variables: ${ENV_VAR}
        result = substitute_env_vars(&result);

        result
    }

    /// Resolve a relative plugin path to an absolute path.
    ///
    /// The path should start with `./` and is resolved relative to `plugin_root`.
    pub fn resolve_path(&self, relative: &str) -> PathBuf {
        let stripped = relative.strip_prefix("./").unwrap_or(relative);
        self.plugin_root.join(stripped)
    }

    /// Ensure the data directory exists.
    pub fn ensure_data_dir(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.plugin_data)?;
        Ok(())
    }
}

/// Substitute `${ENV_VAR}` patterns with environment variable values.
///
/// Unknown variables are left as-is (not removed).
fn substitute_env_vars(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '$' && chars.peek() == Some(&'{') {
            chars.next(); // consume '{'
            let mut var_name = String::new();
            let mut closed = false;
            for ch in chars.by_ref() {
                if ch == '}' {
                    closed = true;
                    break;
                }
                var_name.push(ch);
            }
            if closed && !var_name.starts_with("ECHO_") && !var_name.starts_with("user_config.") {
                // Try as environment variable
                match std::env::var(&var_name) {
                    Ok(val) => result.push_str(&val),
                    Err(_) => {
                        // Leave as-is if not found
                        result.push_str(&format!("${{{var_name}}}"));
                    }
                }
            } else if !closed {
                // Unterminated ${, push back what we consumed
                result.push('$');
                result.push('{');
                result.push_str(&var_name);
            } else {
                // Already handled variable (ECHO_ or user_config.)
                result.push_str(&format!("${{{var_name}}}"));
            }
        } else {
            result.push(c);
        }
    }

    result
}

/// Export plugin variables as environment variables for subprocess hooks.
///
/// # Safety
/// This function modifies the process environment using `std::env::set_var()`, which
/// is inherently unsafe in multi-threaded programs (data race on libc `environ`).
///
/// **Caller must ensure** this function is called during single-threaded plugin
/// initialization only (typically at startup before background threads are spawned).
///
/// All user-controlled variable names are validated to contain only `[A-Z0-9_]`
/// characters to prevent environment variable injection attacks.
pub fn export_to_env(vars: &PluginVariables) {
    // SAFETY: Caller must ensure this is called during single-threaded initialization.
    // std::env::set_var is not thread-safe — concurrent calls from multiple threads
    // can cause data races on the process environment.
    unsafe {
        std::env::set_var("ECHO_PLUGIN_ROOT", &vars.plugin_root);
        std::env::set_var("ECHO_PLUGIN_DATA", &vars.plugin_data);
        std::env::set_var("ECHO_PROJECT_DIR", &vars.project_dir);

        for (key, value) in &vars.user_config {
            let upper_key = key.to_uppercase();
            // Validate env var name: only allow [A-Z0-9_] to prevent injection
            if !upper_key
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            {
                tracing::warn!(
                    "Skipping plugin env var with invalid characters in key: '{}'",
                    key
                );
                continue;
            }
            let env_key = format!("ECHO_PLUGIN_OPTION_{}", upper_key);
            std::env::set_var(&env_key, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_substitute_builtin_vars() {
        let vars = PluginVariables {
            plugin_root: PathBuf::from("/home/user/.echo-agent/plugins/my-plugin"),
            plugin_data: PathBuf::from("/home/user/.echo-agent/plugins/data/my-plugin"),
            project_dir: PathBuf::from("/home/user/my-project"),
            user_config: HashMap::new(),
        };

        let input = "run ${ECHO_PLUGIN_ROOT}/scripts/start.sh";
        let result = vars.substitute(input);
        assert!(result.contains("/home/user/.echo-agent/plugins/my-plugin/scripts/start.sh"));
        assert!(!result.contains("${ECHO_PLUGIN_ROOT}"));
    }

    #[test]
    fn test_substitute_user_config() {
        let mut config = HashMap::new();
        config.insert(
            "api_endpoint".to_string(),
            "http://localhost:9090".to_string(),
        );

        let vars = PluginVariables {
            plugin_root: PathBuf::from("/tmp/plugin"),
            plugin_data: PathBuf::from("/tmp/data"),
            project_dir: PathBuf::from("/tmp/project"),
            user_config: config,
        };

        let input = "connect to ${user_config.api_endpoint}";
        let result = vars.substitute(input);
        assert_eq!(result, "connect to http://localhost:9090");
    }

    #[test]
    fn test_resolve_path() {
        let vars = PluginVariables::new(
            "test",
            PathBuf::from("/home/user/.echo-agent/plugins/test"),
            PathBuf::from("/home/user/project"),
        );

        assert_eq!(
            vars.resolve_path("./skills/my-skill"),
            PathBuf::from("/home/user/.echo-agent/plugins/test/skills/my-skill")
        );
    }

    #[test]
    fn test_data_dir_sanitization() {
        let dir = PluginVariables::data_dir_for("my-plugin@marketplace");
        assert!(dir.to_string_lossy().contains("my-plugin-marketplace"));
        assert!(!dir.to_string_lossy().contains('@'));
    }

    #[test]
    fn test_substitute_env_vars() {
        unsafe { std::env::set_var("TEST_ECHO_VAR", "hello") };
        let result = substitute_env_vars("value is ${TEST_ECHO_VAR}");
        assert_eq!(result, "value is hello");
        unsafe { std::env::remove_var("TEST_ECHO_VAR") };
    }

    #[test]
    fn test_unknown_env_var_preserved() {
        let result = substitute_env_vars("${UNKNOWN_VAR_XYZ_12345}");
        assert_eq!(result, "${UNKNOWN_VAR_XYZ_12345}");
    }
}
