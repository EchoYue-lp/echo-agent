//! Plugin lifecycle trait — hooks into plugin state transitions.
//!
//! Plugins implement this trait to receive callbacks at key moments
//! in their lifecycle: loading, activation, deactivation, and shutdown.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Lifecycle callbacks for a plugin.
///
/// The agent framework calls these methods at the appropriate times:
///
/// ```text
/// load → init → activate ⇄ deactivate → shutdown
///                    ↑          ↓
///                    └──────────┘  (can cycle on reload)
/// ```
pub trait PluginLifecycle: Send + Sync {
    /// Called once after the plugin is loaded and its components are registered.
    ///
    /// Use this to perform one-time setup: start background processes,
    /// open connections, initialize caches.
    fn init(&self) -> Result<(), String> {
        Ok(())
    }

    /// Called when the plugin is enabled (or at startup if `default_enabled: true`).
    ///
    /// Components are already wired at this point. Use this for
    /// activation-specific logic like starting monitors.
    fn activate(&self) -> Result<(), String> {
        Ok(())
    }

    /// Called when the plugin is disabled.
    ///
    /// Use this to stop background processes and release resources.
    /// Components will be unwired after this returns.
    fn deactivate(&self) -> Result<(), String> {
        Ok(())
    }

    /// Called at agent shutdown.
    ///
    /// Use this for final cleanup: flush buffers, close connections,
    /// save state to `${ECHO_PLUGIN_DATA}`.
    fn shutdown(&self) -> Result<(), String> {
        Ok(())
    }
}

/// A no-op lifecycle implementation for plugins that don't need callbacks.
pub struct NoopLifecycle;

impl PluginLifecycle for NoopLifecycle {}

struct ManagedLifecycle {
    callbacks: Arc<dyn PluginLifecycle>,
    initialized: bool,
    active: bool,
    cleanup_required: bool,
}

/// Owns lifecycle callbacks and drives them from enabled plugin state.
#[derive(Default)]
pub struct PluginLifecycleManager {
    plugins: HashMap<String, ManagedLifecycle>,
}

impl PluginLifecycleManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register callbacks for a plugin before the next reconciliation.
    pub fn register(
        &mut self,
        plugin_id: impl Into<String>,
        callbacks: Arc<dyn PluginLifecycle>,
    ) -> Result<(), String> {
        let plugin_id = plugin_id.into();
        if self.plugins.contains_key(&plugin_id) {
            return Err(format!(
                "Lifecycle callbacks already registered for plugin '{plugin_id}'"
            ));
        }
        self.plugins.insert(
            plugin_id,
            ManagedLifecycle {
                callbacks,
                initialized: false,
                active: false,
                cleanup_required: false,
            },
        );
        Ok(())
    }

    /// Initialize and activate a registered plugin exactly once per transition.
    pub fn activate(&mut self, plugin_id: &str) -> Result<(), String> {
        let lifecycle = self
            .plugins
            .get_mut(plugin_id)
            .ok_or_else(|| format!("No lifecycle callbacks registered for '{plugin_id}'"))?;
        if !lifecycle.initialized {
            if lifecycle.cleanup_required {
                return Err(format!(
                    "Plugin '{plugin_id}' has unresolved lifecycle cleanup debt"
                ));
            }
            if let Err(error) = lifecycle.callbacks.init() {
                lifecycle.cleanup_required = true;
                return Err(format!("Plugin '{plugin_id}' init failed: {error}"));
            }
            lifecycle.initialized = true;
        }
        if !lifecycle.active {
            if let Err(error) = lifecycle.callbacks.activate() {
                lifecycle.cleanup_required = true;
                return Err(format!("Plugin '{plugin_id}' activation failed: {error}"));
            }
            lifecycle.active = true;
        }
        Ok(())
    }

    /// Deactivate a registered plugin when it is currently active.
    pub fn deactivate(&mut self, plugin_id: &str) -> Result<(), String> {
        let lifecycle = self
            .plugins
            .get_mut(plugin_id)
            .ok_or_else(|| format!("No lifecycle callbacks registered for '{plugin_id}'"))?;
        if lifecycle.active {
            if let Err(error) = lifecycle.callbacks.deactivate() {
                lifecycle.cleanup_required = true;
                return Err(format!("Plugin '{plugin_id}' deactivation failed: {error}"));
            }
            lifecycle.active = false;
        }
        Ok(())
    }

    /// Deactivate every currently active callback before a runtime-wide rewire.
    pub fn deactivate_all(&mut self) -> Vec<String> {
        let mut ids = self.plugins.keys().cloned().collect::<Vec<_>>();
        ids.sort();
        let mut errors = Vec::new();
        for plugin_id in ids {
            if let Err(error) = self.deactivate(&plugin_id) {
                errors.push(error);
            }
        }
        errors
    }

    /// Remove callbacks for an uninstalled plugin after releasing their resources.
    ///
    /// Failed cleanup retains ownership so callers can retry instead of
    /// replacing callbacks that may still own live resources.
    pub fn unregister(&mut self, plugin_id: &str) -> Result<bool, String> {
        let Some(lifecycle) = self.plugins.get_mut(plugin_id) else {
            return Ok(false);
        };
        let mut errors = Vec::new();
        if lifecycle.active || lifecycle.cleanup_required {
            if let Err(error) = lifecycle.callbacks.deactivate() {
                errors.push(format!(
                    "Plugin '{plugin_id}' deactivation failed during unregister: {error}"
                ));
            } else {
                lifecycle.active = false;
            }
        }
        if lifecycle.initialized || lifecycle.cleanup_required {
            if let Err(error) = lifecycle.callbacks.shutdown() {
                errors.push(format!(
                    "Plugin '{plugin_id}' shutdown failed during unregister: {error}"
                ));
            } else {
                lifecycle.initialized = false;
            }
        }
        if errors.is_empty() {
            lifecycle.cleanup_required = false;
            self.plugins.remove(plugin_id);
            Ok(true)
        } else {
            lifecycle.cleanup_required = true;
            Err(errors.join("; "))
        }
    }

    /// Reconcile registered callbacks with the registry's enabled plugin set.
    pub fn reconcile<'a>(
        &mut self,
        enabled_plugins: impl IntoIterator<Item = &'a str>,
    ) -> Vec<String> {
        let enabled = enabled_plugins.into_iter().collect::<HashSet<_>>();
        let mut errors = self.deactivate_not_in_set(&enabled);
        errors.extend(self.activate_in_set(&enabled));
        errors
    }

    /// Deactivate active callbacks whose plugins are absent from the next set.
    /// Embedding runtimes call this before unwiring plugin components.
    pub fn deactivate_not_in<'a>(
        &mut self,
        enabled_plugins: impl IntoIterator<Item = &'a str>,
    ) -> Vec<String> {
        let enabled = enabled_plugins.into_iter().collect::<HashSet<_>>();
        self.deactivate_not_in_set(&enabled)
    }

    /// Initialize and activate callbacks present in the current enabled set.
    /// Embedding runtimes call this after plugin components are wired.
    pub fn activate_enabled<'a>(
        &mut self,
        enabled_plugins: impl IntoIterator<Item = &'a str>,
    ) -> Vec<String> {
        let enabled = enabled_plugins.into_iter().collect::<HashSet<_>>();
        self.activate_in_set(&enabled)
    }

    fn deactivate_not_in_set(&mut self, enabled: &HashSet<&str>) -> Vec<String> {
        let mut ids = self.plugins.keys().cloned().collect::<Vec<_>>();
        ids.sort();
        let mut errors = Vec::new();
        for plugin_id in ids {
            if !enabled.contains(plugin_id.as_str())
                && let Err(error) = self.deactivate(&plugin_id)
            {
                errors.push(error);
            }
        }
        errors
    }

    fn activate_in_set(&mut self, enabled: &HashSet<&str>) -> Vec<String> {
        let mut ids = self.plugins.keys().cloned().collect::<Vec<_>>();
        ids.sort();
        let mut errors = Vec::new();
        for plugin_id in ids {
            if enabled.contains(plugin_id.as_str())
                && let Err(error) = self.activate(&plugin_id)
            {
                errors.push(error);
            }
        }
        errors
    }

    /// Deactivate active plugins and invoke final shutdown callbacks.
    pub fn shutdown(&mut self) -> Vec<String> {
        let mut ids = self.plugins.keys().cloned().collect::<Vec<_>>();
        ids.sort();
        let mut errors = Vec::new();
        for plugin_id in ids {
            if let Err(error) = self.unregister(&plugin_id) {
                errors.push(error);
            }
        }
        errors
    }
}

impl Drop for PluginLifecycleManager {
    fn drop(&mut self) {
        for error in self.shutdown() {
            tracing::warn!(%error, "Plugin lifecycle shutdown callback failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct Counts {
        init: AtomicUsize,
        activate: AtomicUsize,
        deactivate: AtomicUsize,
        shutdown: AtomicUsize,
    }

    struct CountingLifecycle(Arc<Counts>);

    struct FailingCleanupLifecycle;

    impl PluginLifecycle for CountingLifecycle {
        fn init(&self) -> Result<(), String> {
            self.0.init.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn activate(&self) -> Result<(), String> {
            self.0.activate.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn deactivate(&self) -> Result<(), String> {
            self.0.deactivate.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn shutdown(&self) -> Result<(), String> {
            self.0.shutdown.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    impl PluginLifecycle for FailingCleanupLifecycle {
        fn deactivate(&self) -> Result<(), String> {
            Err("injected deactivate failure".to_string())
        }

        fn shutdown(&self) -> Result<(), String> {
            Err("injected shutdown failure".to_string())
        }
    }

    #[test]
    fn lifecycle_manager_drives_enabled_state_and_shutdown_once() -> Result<(), String> {
        let counts = Arc::new(Counts::default());
        let mut manager = PluginLifecycleManager::new();
        manager.register("example", Arc::new(CountingLifecycle(Arc::clone(&counts))))?;

        assert!(manager.reconcile(["example"]).is_empty());
        assert!(manager.reconcile(["example"]).is_empty());
        assert!(manager.reconcile(std::iter::empty()).is_empty());
        assert!(manager.shutdown().is_empty());

        assert_eq!(counts.init.load(Ordering::SeqCst), 1);
        assert_eq!(counts.activate.load(Ordering::SeqCst), 1);
        assert_eq!(counts.deactivate.load(Ordering::SeqCst), 1);
        assert_eq!(counts.shutdown.load(Ordering::SeqCst), 1);
        Ok(())
    }

    #[test]
    fn unregister_releases_callbacks_and_allows_reregistration() -> Result<(), String> {
        let first = Arc::new(Counts::default());
        let second = Arc::new(Counts::default());
        let mut manager = PluginLifecycleManager::new();
        manager.register("example", Arc::new(CountingLifecycle(Arc::clone(&first))))?;
        manager.activate("example")?;

        assert!(manager.unregister("example")?);
        assert_eq!(first.deactivate.load(Ordering::SeqCst), 1);
        assert_eq!(first.shutdown.load(Ordering::SeqCst), 1);

        manager.register("example", Arc::new(CountingLifecycle(Arc::clone(&second))))?;
        manager.activate("example")?;
        assert_eq!(second.init.load(Ordering::SeqCst), 1);
        assert_eq!(second.activate.load(Ordering::SeqCst), 1);
        Ok(())
    }

    #[test]
    fn deactivate_all_brackets_reactivation_without_reinitializing() -> Result<(), String> {
        let counts = Arc::new(Counts::default());
        let mut manager = PluginLifecycleManager::new();
        manager.register("example", Arc::new(CountingLifecycle(Arc::clone(&counts))))?;
        manager.activate("example")?;

        assert!(manager.deactivate_all().is_empty());
        assert!(manager.activate_enabled(["example"]).is_empty());

        assert_eq!(counts.init.load(Ordering::SeqCst), 1);
        assert_eq!(counts.activate.load(Ordering::SeqCst), 2);
        assert_eq!(counts.deactivate.load(Ordering::SeqCst), 1);
        Ok(())
    }

    #[test]
    fn failed_unregister_retains_cleanup_ownership() -> Result<(), String> {
        let mut manager = PluginLifecycleManager::new();
        manager.register("example", Arc::new(FailingCleanupLifecycle))?;
        manager.activate("example")?;

        let error = manager
            .unregister("example")
            .err()
            .ok_or_else(|| "failing cleanup unexpectedly succeeded".to_string())?;
        assert!(error.contains("injected deactivate failure"));
        assert!(error.contains("injected shutdown failure"));
        assert!(
            manager
                .register("example", Arc::new(NoopLifecycle))
                .is_err()
        );
        Ok(())
    }
}
