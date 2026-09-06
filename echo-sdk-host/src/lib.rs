//! Product-neutral, source-built standard ACP v1 Host for `echo-agent`.
//!
//! The crate owns configuration and Session Agent construction only. The root
//! framework adapter remains the authority for ACP methods, Session state,
//! cancellation, event projection, and bounded shutdown.
//!
//! With the `sdk-core-profile` feature and an explicit `sdk_profile` config
//! section, the Host additionally negotiates the `_echo_agent/*` core
//! profile over the same connection: generation-fenced Agent/Session/Run/
//! Stream handles, typed RPC, ACK-bounded live events, durable replay and
//! restart recovery under the configured state root. Without that section
//! the Host is standard-only and never advertises the extension.

// The typed `EchoSdkError` wire envelope is returned by value throughout the
// core profile handlers. It is a small, fixed-shape in-process DTO (never a
// heavyweight boxed tree), and boxing it at every handler boundary would
// churn the host code without changing behavior: these results never cross a
// hot loop and the process is local and single-tenant.
#![allow(clippy::result_large_err)]

mod config;
#[cfg(feature = "runtime")]
mod factory;
#[cfg(feature = "runtime")]
mod mcp;

#[cfg(feature = "sdk-core-profile")]
mod bounded_stdio;
#[cfg(feature = "sdk-core-profile")]
mod core_profile;

pub use config::{
    HOST_CONFIG_SCHEMA_VERSION, HostCommand, MAX_HOST_CONFIG_BYTES, SdkHostConfig,
    SdkProfileConfig, SdkProfileLimits, parse_args,
};

#[cfg(feature = "sdk-core-profile")]
pub use core_profile::SdkCoreProfile;

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
/// When the config carries an `sdk_profile` section (and the Host was built
/// with `sdk-core-profile`), the negotiated core profile rides the same
/// official connection and bounded stdio transport.
#[cfg(feature = "runtime")]
pub async fn run_stdio(path: impl AsRef<std::path::Path>) -> Result<(), HostError> {
    let prepared = SdkHostConfig::from_path(path)?.prepare()?;
    match prepared.sdk_profile {
        #[cfg(feature = "sdk-core-profile")]
        Some(profile_config) => {
            let profile = core_profile::SdkCoreProfile::install(
                &profile_config.state_root,
                profile_config.limits,
                prepared.framework,
                prepared.llm_client,
            )?;
            // The standard Session factory shares the profile's default
            // definition, so standard `session/new` and
            // `_echo_agent/session/create` construct Sessions from the same
            // definition authority.
            let factory = profile.session_factory();
            let transport = bounded_stdio::bounded_stdio(profile_config.limits.max_frame_bytes);
            AcpAgentAdapter::new(factory)
                .with_profile(profile)
                .connect_to(transport)
                .await
                .map_err(|error| HostError::Runtime(error.to_string()))
        }
        #[cfg(not(feature = "sdk-core-profile"))]
        Some(profile_config) => Err(HostError::Config(format!(
            "sdk_profile requires a Host built with the sdk-core-profile feature \
             (state root {} was configured)",
            profile_config.state_root.display()
        ))),
        None => {
            let factory =
                factory::DefaultHostSessionFactory::new(prepared.framework, prepared.llm_client);
            AcpAgentAdapter::new(factory)
                .connect_to(Stdio::new())
                .await
                .map_err(|error| HostError::Runtime(error.to_string()))
        }
    }
}

/// Help text printed only when the executable is not serving ACP.
pub const HELP: &str = "echo-agent-sdk-host\n\nUSAGE:\n  echo-agent-sdk-host --config <path> [--check-config]\n  echo-agent-sdk-host --help\n  echo-agent-sdk-host --version\n";

/// Version of the source-built Host crate.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
