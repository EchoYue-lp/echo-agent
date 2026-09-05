//! Product-neutral, source-built standard ACP v1 Host for `echo-agent`.
//!
//! The crate owns configuration and Session Agent construction only. The root
//! framework adapter remains the authority for ACP methods, Session state,
//! cancellation, event projection, and bounded shutdown.

mod config;
#[cfg(feature = "runtime")]
mod factory;
#[cfg(feature = "runtime")]
mod mcp;

pub use config::{
    HOST_CONFIG_SCHEMA_VERSION, HostCommand, MAX_HOST_CONFIG_BYTES, SdkHostConfig, parse_args,
};

#[cfg(feature = "runtime")]
use agent_client_protocol::{ConnectTo as _, Stdio};
#[cfg(feature = "runtime")]
use echo_agent::acp::AcpAgentAdapter;
use std::fmt;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
/// Bounded startup/runtime error reported by the Host executable.
pub enum HostError {
    /// Invalid command-line input.
    Argument(String),
    /// Invalid Host configuration.
    Config(String),
    /// Configuration file access failed.
    ConfigFile { path: PathBuf, message: String },
    /// The official ACP connection failed after startup.
    Runtime(String),
}

impl fmt::Display for HostError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Argument(message) | Self::Config(message) | Self::Runtime(message) => {
                formatter.write_str(message)
            }
            Self::ConfigFile { path, message } => {
                write!(
                    formatter,
                    "failed to read Host config {}: {message}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for HostError {}

/// Validate a Host JSON file without opening ACP stdio.
pub fn validate_config(path: impl AsRef<std::path::Path>) -> Result<(), HostError> {
    SdkHostConfig::from_path(path)?.validate()
}

/// Run the configured standard ACP v1 Agent on the official stdio transport.
#[cfg(feature = "runtime")]
pub async fn run_stdio(path: impl AsRef<std::path::Path>) -> Result<(), HostError> {
    let prepared = SdkHostConfig::from_path(path)?.prepare()?;
    let factory = factory::DefaultHostSessionFactory::new(prepared.framework, prepared.llm_client);
    AcpAgentAdapter::new(factory)
        .connect_to(Stdio::new())
        .await
        .map_err(|error| HostError::Runtime(error.to_string()))
}

/// Help text printed only when the executable is not serving ACP.
pub const HELP: &str = "echo-agent-sdk-host\n\nUSAGE:\n  echo-agent-sdk-host --config <path> [--check-config]\n  echo-agent-sdk-host --help\n  echo-agent-sdk-host --version\n";

/// Version of the source-built Host crate.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
