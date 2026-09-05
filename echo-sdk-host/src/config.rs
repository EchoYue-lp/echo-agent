use echo_agent::config::FrameworkConfig;
use echo_agent::error::ReactError;
use echo_agent::llm::{LlmClient, LlmConfig};
use serde::Deserialize;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::HostError;

/// Host configuration schema version accepted by this source revision.
pub const HOST_CONFIG_SCHEMA_VERSION: u32 = 1;
/// Maximum number of bytes read from one Host configuration file.
pub const MAX_HOST_CONFIG_BYTES: u64 = 1024 * 1024;

/// Versioned, product-neutral configuration for `echo-agent-sdk-host`.
///
/// This type intentionally does not implement `Debug`: `FrameworkConfig`
/// contains an optional resolved credential.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SdkHostConfig {
    /// Version of this Host configuration document.
    pub schema_version: u32,
    /// Product-neutral framework configuration used to construct each Session Agent.
    pub default_agent: FrameworkConfig,
    /// Optional environment variable containing the model credential.
    #[serde(default)]
    pub api_key_env: Option<String>,
}

pub(crate) struct PreparedHostConfig {
    pub framework: FrameworkConfig,
    pub llm_client: Arc<dyn LlmClient>,
}

impl SdkHostConfig {
    /// Read and deserialize a bounded JSON configuration file.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, HostError> {
        let path = path.as_ref();
        let file = std::fs::File::open(path).map_err(|error| HostError::ConfigFile {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        let metadata = file.metadata().map_err(|error| HostError::ConfigFile {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        if !metadata.is_file() {
            return Err(HostError::Config(
                "Host config path must be a regular file".to_string(),
            ));
        }
        if metadata.len() > MAX_HOST_CONFIG_BYTES {
            return Err(HostError::Config(format!(
                "Host config exceeds the {MAX_HOST_CONFIG_BYTES} byte limit"
            )));
        }
        let bytes = read_bounded(file, path)?;
        serde_json::from_slice(&bytes)
            .map_err(|error| HostError::Config(format!("invalid Host config JSON: {error}")))
    }

    /// Validate the configuration and construct its model client without starting ACP.
    pub fn validate(self) -> Result<(), HostError> {
        let PreparedHostConfig {
            framework,
            llm_client,
        } = self.prepare()?;
        drop((framework, llm_client));
        Ok(())
    }

    pub(crate) fn prepare(self) -> Result<PreparedHostConfig, HostError> {
        self.validate_with_env(|name| std::env::var(name).map_err(|error| error.to_string()))
    }

    pub(crate) fn validate_with_env(
        mut self,
        read_env: impl FnOnce(&str) -> Result<String, String>,
    ) -> Result<PreparedHostConfig, HostError> {
        if self.schema_version != HOST_CONFIG_SCHEMA_VERSION {
            return Err(HostError::Config(format!(
                "unsupported Host config schema_version {}; expected {HOST_CONFIG_SCHEMA_VERSION}",
                self.schema_version
            )));
        }
        validate_agent_settings(&self.default_agent)?;
        let api_protocol = self.default_agent.model.api_protocol.ok_or_else(|| {
            HostError::Config("default_agent.model.api_protocol is required".to_string())
        })?;
        let base_url = self.default_agent.model.get_base_url().ok_or_else(|| {
            HostError::Config("default_agent.model.base_url is required".to_string())
        })?;
        let model = self.default_agent.model.get_model_name();
        if model.trim().is_empty() {
            return Err(HostError::Config(
                "default_agent.model.name must not be empty".to_string(),
            ));
        }
        let provider = self.default_agent.model.provider.trim().to_string();
        if provider.is_empty() {
            return Err(HostError::Config(
                "default_agent.model.provider must not be empty".to_string(),
            ));
        }
        let inline_token = self.default_agent.model.get_auth_token();
        let env_name = self.api_key_env.take().map(|name| name.trim().to_string());
        if env_name.as_deref() == Some("") {
            return Err(HostError::Config(
                "api_key_env must not be empty when provided".to_string(),
            ));
        }
        if inline_token.is_some() && env_name.is_some() {
            return Err(HostError::Config(
                "default_agent.model.auth_token and api_key_env are mutually exclusive".to_string(),
            ));
        }
        let api_key = if let Some(token) = inline_token {
            token
        } else if let Some(name) = env_name {
            read_env(&name).map_err(|_| {
                HostError::Config(format!(
                    "credential environment variable {name} is unavailable"
                ))
            })?
        } else {
            String::new()
        };
        self.default_agent.model.auth_token = None;
        let llm_config = LlmConfig::for_provider(provider, base_url, api_key, model, api_protocol)
            .map_err(framework_error)?;
        let llm_client: Arc<dyn LlmClient> =
            Arc::from(llm_config.build_client().map_err(framework_error)?);
        Ok(PreparedHostConfig {
            framework: self.default_agent,
            llm_client,
        })
    }
}

fn read_bounded(reader: impl std::io::Read, path: &Path) -> Result<Vec<u8>, HostError> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_HOST_CONFIG_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| HostError::ConfigFile {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_HOST_CONFIG_BYTES {
        return Err(HostError::Config(format!(
            "Host config exceeds the {MAX_HOST_CONFIG_BYTES} byte limit"
        )));
    }
    Ok(bytes)
}

fn validate_agent_settings(config: &FrameworkConfig) -> Result<(), HostError> {
    if config.agent.name.trim().is_empty() {
        return Err(HostError::Config(
            "default_agent.agent.name must not be empty".to_string(),
        ));
    }
    if config.agent.system_prompt.trim().is_empty() {
        return Err(HostError::Config(
            "default_agent.agent.system_prompt must not be empty".to_string(),
        ));
    }
    if config.agent.max_iterations == 0 {
        return Err(HostError::Config(
            "default_agent.agent.max_iterations must be positive".to_string(),
        ));
    }
    if !config.agent.enable_tools {
        return Err(HostError::Config(
            "default_agent.agent.enable_tools must be true for ACP stdio MCP support".to_string(),
        ));
    }
    if config.agent.enable_memory {
        return Err(HostError::Config(
            "default_agent.agent.enable_memory is not supported by the standard Host profile"
                .to_string(),
        ));
    }
    if config.agent.enable_human_in_loop {
        return Err(HostError::Config(
            "default_agent.agent.enable_human_in_loop requires a later ACP callback profile"
                .to_string(),
        ));
    }
    Ok(())
}

fn framework_error(error: ReactError) -> HostError {
    HostError::Config(error.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Command selected from the Host's deliberately small command-line surface.
pub enum HostCommand {
    /// Validate configuration, then serve ACP over stdio.
    Run { config: PathBuf },
    /// Validate configuration and exit without opening ACP stdio.
    CheckConfig { config: PathBuf },
    /// Print command help outside ACP mode.
    Help,
    /// Print the source-built Host crate version outside ACP mode.
    Version,
}

/// Parse Host command-line arguments after the executable name.
pub fn parse_args<I, S>(args: I) -> Result<HostCommand, HostError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut args = args.into_iter().map(Into::into);
    let mut config = None;
    let mut check_config = false;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--config" => {
                if config.is_some() {
                    return Err(HostError::Argument(
                        "--config may appear only once".to_string(),
                    ));
                }
                let value = args
                    .next()
                    .ok_or_else(|| HostError::Argument("--config requires a path".to_string()))?;
                config = Some(PathBuf::from(value));
            }
            "--check-config" => check_config = true,
            "--help" | "-h" => return Ok(HostCommand::Help),
            "--version" | "-V" => return Ok(HostCommand::Version),
            other => {
                return Err(HostError::Argument(format!(
                    "unknown Host argument: {other}"
                )));
            }
        }
    }
    let config = config.ok_or_else(|| {
        HostError::Argument("--config <path> is required for Host startup".to_string())
    })?;
    if check_config {
        Ok(HostCommand::CheckConfig { config })
    } else {
        Ok(HostCommand::Run { config })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_agent::config::{AgentSettings, ModelConfig};
    use echo_agent::llm::LlmApiProtocol;

    static_assertions::assert_not_impl_any!(SdkHostConfig: serde::Serialize);

    fn config() -> SdkHostConfig {
        SdkHostConfig {
            schema_version: HOST_CONFIG_SCHEMA_VERSION,
            default_agent: FrameworkConfig {
                model: ModelConfig {
                    provider: "local".to_string(),
                    name: "test-model".to_string(),
                    base_url: Some("http://127.0.0.1:11434/v1/chat/completions".to_string()),
                    api_protocol: Some(LlmApiProtocol::ChatCompletions),
                    ..ModelConfig::default()
                },
                agent: AgentSettings {
                    enable_tools: true,
                    ..AgentSettings::default()
                },
            },
            api_key_env: None,
        }
    }

    #[test]
    fn valid_local_config_prepares_without_a_secret() {
        assert!(
            config()
                .validate_with_env(|_| Err("unused".to_string()))
                .is_ok()
        );
    }

    #[test]
    fn credential_sources_are_exclusive_and_secret_is_not_in_error() {
        let mut config = config();
        config.default_agent.model.auth_token = Some("sentinel-secret".to_string());
        config.api_key_env = Some("TEST_TOKEN".to_string());
        let error = config
            .validate_with_env(|_| Ok("environment-secret".to_string()))
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default();
        assert!(error.contains("mutually exclusive"));
        assert!(!error.contains("sentinel-secret"));
        assert!(!error.contains("environment-secret"));
    }

    #[test]
    fn blank_environment_credential_name_is_rejected() {
        let mut config = config();
        config.api_key_env = Some("  ".to_string());
        assert!(
            config
                .validate_with_env(|_| Ok("unused".to_string()))
                .is_err()
        );
    }

    #[test]
    fn environment_credential_is_resolved_without_entering_framework_config()
    -> Result<(), HostError> {
        let mut config = config();
        config.api_key_env = Some(" TEST_TOKEN ".to_string());
        let prepared = config.validate_with_env(|name| {
            if name == "TEST_TOKEN" {
                Ok("environment-secret".to_string())
            } else {
                Err("unexpected environment variable".to_string())
            }
        })?;
        assert!(prepared.framework.model.auth_token.is_none());
        Ok(())
    }

    #[test]
    fn schema_and_required_model_fields_fail_fast() {
        let mut wrong_schema = config();
        wrong_schema.schema_version = 2;
        assert!(
            wrong_schema
                .validate_with_env(|_| Err("unused".to_string()))
                .is_err()
        );

        let mut missing_provider = config();
        missing_provider.default_agent.model.provider.clear();
        assert!(
            missing_provider
                .validate_with_env(|_| Err("unused".to_string()))
                .is_err()
        );

        let mut missing_model = config();
        missing_model.default_agent.model.name.clear();
        assert!(
            missing_model
                .validate_with_env(|_| Err("unused".to_string()))
                .is_err()
        );

        let mut missing_endpoint = config();
        missing_endpoint.default_agent.model.base_url = None;
        assert!(
            missing_endpoint
                .validate_with_env(|_| Err("unused".to_string()))
                .is_err()
        );

        let mut missing_protocol = config();
        missing_protocol.default_agent.model.api_protocol = None;
        assert!(
            missing_protocol
                .validate_with_env(|_| Err("unused".to_string()))
                .is_err()
        );

        let mut invalid_endpoint = config();
        invalid_endpoint.default_agent.model.base_url = Some("not a URL".to_string());
        assert!(
            invalid_endpoint
                .validate_with_env(|_| Err("unused".to_string()))
                .is_err()
        );
    }

    #[test]
    fn unsupported_profile_settings_fail_before_stdio() {
        for mutate in [
            |agent: &mut AgentSettings| agent.enable_tools = false,
            |agent: &mut AgentSettings| agent.enable_memory = true,
            |agent: &mut AgentSettings| agent.enable_human_in_loop = true,
        ] {
            let mut config = config();
            mutate(&mut config.default_agent.agent);
            assert!(
                config
                    .validate_with_env(|_| Err("unused".to_string()))
                    .is_err()
            );
        }
    }

    #[test]
    fn cli_requires_one_explicit_config_path() {
        assert!(parse_args(Vec::<String>::new()).is_err());
        assert!(parse_args(["--config", "host.json", "--config", "other.json"]).is_err());
        assert_eq!(
            parse_args(["--config", "host.json", "--check-config"]),
            Ok(HostCommand::CheckConfig {
                config: PathBuf::from("host.json")
            })
        );
    }

    #[test]
    fn checked_in_example_is_current_and_valid() -> Result<(), Box<dyn std::error::Error>> {
        let parsed: SdkHostConfig = serde_json::from_str(include_str!("../config.example.json"))?;
        parsed.validate_with_env(|_| Err("unused".to_string()))?;
        Ok(())
    }

    #[test]
    fn unknown_top_level_config_field_is_rejected() {
        let encoded = r#"{
            "schema_version": 1,
            "default_agent": {},
            "unexpected": true
        }"#;
        assert!(serde_json::from_str::<SdkHostConfig>(encoded).is_err());
    }

    #[test]
    fn bounded_reader_stops_after_limit_plus_one() {
        let error = read_bounded(std::io::repeat(b'x'), Path::new("streaming-config"));
        assert!(matches!(error, Err(HostError::Config(message)) if message.contains("byte limit")));
    }

    #[test]
    fn config_path_must_be_a_regular_file() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        assert!(SdkHostConfig::from_path(directory.path()).is_err());
        Ok(())
    }
}
