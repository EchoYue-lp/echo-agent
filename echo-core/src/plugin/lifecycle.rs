//! Plugin lifecycle trait — hooks into plugin state transitions.
//!
//! Plugins implement this trait to receive callbacks at key moments
//! in their lifecycle: loading, activation, deactivation, and shutdown.

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
