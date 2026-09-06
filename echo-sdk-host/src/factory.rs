use crate::mcp::translate_mcp_servers;
use agent_client_protocol::BoxFuture;
use echo_agent::acp::{AcpSessionContext, AcpSessionFactory};
use echo_agent::agent::{Agent, AgentConfig, ReactAgent};
use echo_agent::config::FrameworkConfig;
use echo_agent::error::{ReactError, Result};
use echo_agent::llm::LlmClient;
use echo_agent::state::RuntimeStateStore;
use std::sync::Arc;

/// One immutable Agent construction definition.
///
/// Extension handles reference a definition, never a shared conversation
/// Agent: every Session created from a definition still constructs its own
/// independent [`ReactAgent`] through [`PreparedAgentDefinition::create_agent`],
/// so histories, working directories and MCP connections never leak across
/// Sessions (design §8).
#[derive(Clone)]
#[allow(dead_code)]
pub(crate) struct PreparedAgentDefinition {
    /// Stable factory identity: the Session factory resolves this id from
    /// the Session context meta so every Session of an Agent definition is
    /// built from exactly that definition.
    id: Arc<str>,
    framework: Arc<FrameworkConfig>,
    llm_client: Arc<dyn LlmClient>,
    state_store: Option<Arc<dyn RuntimeStateStore>>,
    host_default: bool,
}

impl PreparedAgentDefinition {
    pub fn new(
        framework: FrameworkConfig,
        llm_client: Arc<dyn LlmClient>,
        state_store: Option<Arc<dyn RuntimeStateStore>>,
        host_default: bool,
    ) -> Self {
        Self {
            id: Arc::from(uuid::Uuid::new_v4().to_string().as_str()),
            framework: Arc::new(framework),
            llm_client,
            state_store,
            host_default,
        }
    }

    #[cfg(feature = "sdk-core-profile")]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Typed capability snapshot served by `_echo_agent/agent/describe`.
    /// Tool and MCP lists are Session-scoped in this framework: a definition
    /// alone does not own them, so the snapshot reports construction facts
    /// and the per-Session surfaces stay empty at definition level.
    #[cfg(feature = "sdk-core-profile")]
    pub fn snapshot(&self) -> echo_sdk_protocol::methods::AgentSnapshotWire {
        echo_sdk_protocol::methods::AgentSnapshotWire {
            name: self.framework.agent.name.clone(),
            model_name: self.framework.model.name.clone(),
            system_prompt: self.framework.agent.system_prompt.clone(),
            tool_names: Vec::new(),
            skill_names: Vec::new(),
            mcp_server_names: Vec::new(),
            working_dir: None,
            host_default: self.host_default,
        }
    }

    /// Construct the independent Agent that owns one Session's history,
    /// installing the state store before first use so the framework can
    /// checkpoint after every tool batch and resume from it later.
    pub async fn create_agent(&self, context: &AcpSessionContext) -> Result<ReactAgent> {
        let mcp_servers = translate_mcp_servers(context)?;
        let session_id = context.session_id.to_string();
        let agent_config = AgentConfig::from(self.framework.as_ref().clone())
            .session_id(&session_id)
            .conversation_id(&session_id)
            .working_dir(Some(context.cwd.clone()));
        let mut agent = ReactAgent::new(agent_config).with_llm_client(self.llm_client.clone());
        if let Some(store) = &self.state_store {
            agent.set_state_store(store.clone());
        }
        self.framework.apply_compressor(&agent).await;
        for server in mcp_servers {
            if let Err(error) = agent.connect_mcp_from_config(server).await {
                let cleanup = Agent::close(&agent).await;
                let cleanup_note = cleanup
                    .err()
                    .map(|cleanup_error| format!("; cleanup failed: {cleanup_error}"))
                    .unwrap_or_default();
                return Err(ReactError::Other(format!(
                    "failed to prepare ACP Session MCP servers: {error}{cleanup_note}"
                )));
            }
        }
        Ok(agent)
    }
}

/// Session factory of the core profile: resolves the Session's Agent
/// definition from the context meta (`echo_agent_definition_id`) and
/// constructs an independent Agent from exactly that definition. Sessions
/// without a marker bind the Host default definition, keeping standard
/// `session/new` behavior unchanged.
#[cfg(feature = "sdk-core-profile")]
#[derive(Clone)]
pub(crate) struct CoreProfileSessionFactory {
    default: Arc<PreparedAgentDefinition>,
    definitions:
        Arc<std::sync::Mutex<std::collections::HashMap<String, Arc<PreparedAgentDefinition>>>>,
    max_definitions: usize,
}

#[cfg(feature = "sdk-core-profile")]
pub(crate) const DEFINITION_META_KEY: &str = "echo_agent_definition_id";

#[cfg(feature = "sdk-core-profile")]
impl CoreProfileSessionFactory {
    pub fn new(default: Arc<PreparedAgentDefinition>, max_definitions: usize) -> Self {
        let mut definitions = std::collections::HashMap::new();
        definitions.insert(default.id().to_string(), default.clone());
        Self {
            default,
            definitions: Arc::new(std::sync::Mutex::new(definitions)),
            max_definitions,
        }
    }

    /// Shared registration sink; the profile state registers every created
    /// definition here so later Sessions can resolve it.
    pub fn register_definition(
        &self,
        definition: Arc<PreparedAgentDefinition>,
    ) -> std::result::Result<(), String> {
        let mut definitions = self
            .definitions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if definitions.contains_key(definition.id()) {
            return Ok(());
        }
        if definitions.len() >= self.max_definitions {
            return Err(format!(
                "Agent definition limit {} reached",
                self.max_definitions
            ));
        }
        definitions.insert(definition.id().to_string(), definition);
        Ok(())
    }

    pub fn remove_definition(&self, id: &str) {
        if id != self.default.id() {
            self.definitions
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .remove(id);
        }
    }
}

#[cfg(feature = "sdk-core-profile")]
impl AcpSessionFactory for CoreProfileSessionFactory {
    fn create_session(
        &self,
        context: AcpSessionContext,
    ) -> BoxFuture<'static, Result<Box<dyn Agent>>> {
        let definition_id = context
            .meta
            .as_ref()
            .and_then(|meta| meta.get(DEFINITION_META_KEY))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let definitions = self.definitions.clone();
        let default = self.default.clone();
        Box::pin(async move {
            let definition = match definition_id {
                Some(id) => definitions
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .get(&id)
                    .cloned()
                    .ok_or_else(|| {
                        ReactError::Other(format!("unknown Agent definition id {id}"))
                    })?,
                None => default,
            };
            definition
                .create_agent(&context)
                .await
                .map(|agent| Box::new(agent) as Box<dyn Agent>)
        })
    }
}

/// Standard-profile Session factory over the Host default definition.
#[derive(Clone)]
pub(crate) struct DefaultHostSessionFactory {
    definition: std::sync::Arc<PreparedAgentDefinition>,
}

impl DefaultHostSessionFactory {
    pub fn new(framework: FrameworkConfig, llm_client: Arc<dyn LlmClient>) -> Self {
        Self {
            definition: std::sync::Arc::new(PreparedAgentDefinition::new(
                framework, llm_client, None, true,
            )),
        }
    }

    /// Factory over an explicit definition (used by the core profile so the
    /// standard `session/new` shares the profile's default definition).
    #[cfg(test)]
    pub(crate) fn definition(&self) -> &PreparedAgentDefinition {
        &self.definition
    }
}

impl AcpSessionFactory for DefaultHostSessionFactory {
    fn create_session(
        &self,
        context: AcpSessionContext,
    ) -> BoxFuture<'static, Result<Box<dyn Agent>>> {
        let definition = self.definition.clone();
        Box::pin(async move {
            definition
                .create_agent(&context)
                .await
                .map(|agent| Box::new(agent) as Box<dyn Agent>)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{
        ClientCapabilities, McpServer, McpServerStdio, SessionId,
    };
    use echo_agent::config::{AgentSettings, ModelConfig};
    use echo_agent::llm::{LlmApiProtocol, LlmConfig};
    use std::path::PathBuf;
    use std::process::Stdio;
    use std::time::Duration;

    fn factory() -> Result<DefaultHostSessionFactory> {
        let framework = FrameworkConfig {
            model: ModelConfig {
                provider: "local".to_string(),
                name: "fixture-model".to_string(),
                base_url: Some("http://127.0.0.1:9/v1/chat/completions".to_string()),
                api_protocol: Some(LlmApiProtocol::ChatCompletions),
                ..ModelConfig::default()
            },
            agent: AgentSettings {
                enable_tools: true,
                ..AgentSettings::default()
            },
        };
        let client = LlmConfig::for_provider(
            "local",
            "http://127.0.0.1:9/v1/chat/completions",
            "",
            "fixture-model",
            LlmApiProtocol::ChatCompletions,
        )?
        .build_client()?;
        Ok(DefaultHostSessionFactory::new(framework, Arc::from(client)))
    }

    fn context(id: &str, cwd: PathBuf, mcp_servers: Vec<McpServer>) -> AcpSessionContext {
        AcpSessionContext {
            session_id: SessionId::new(id),
            cwd,
            additional_directories: Vec::new(),
            mcp_servers,
            client_capabilities: ClientCapabilities::default(),
            meta: None,
        }
    }

    #[tokio::test]
    async fn sessions_have_distinct_agents_contexts_and_working_directories() -> Result<()> {
        let directory = tempfile::tempdir().map_err(ReactError::Io)?;
        let first_cwd = directory.path().join("first");
        let second_cwd = directory.path().join("second");
        std::fs::create_dir_all(&first_cwd).map_err(ReactError::Io)?;
        std::fs::create_dir_all(&second_cwd).map_err(ReactError::Io)?;
        let factory = factory()?;
        let first = factory
            .definition()
            .create_agent(&context("session-a", first_cwd.clone(), Vec::new()))
            .await?;
        let second = factory
            .definition()
            .create_agent(&context("session-b", second_cwd.clone(), Vec::new()))
            .await?;

        assert_eq!(first.config().get_session_id(), Some("session-a"));
        assert_eq!(first.conversation_id(), Some("session-a"));
        assert_eq!(first.working_dir().as_ref(), Some(&first_cwd));
        assert_eq!(second.config().get_session_id(), Some("session-b"));
        assert_eq!(second.conversation_id(), Some("session-b"));
        assert_eq!(second.working_dir().as_ref(), Some(&second_cwd));
        assert!(!Arc::ptr_eq(first.context(), second.context()));
        let first_llm = first
            .llm_client()
            .ok_or_else(|| ReactError::Other("first Session has no LLM client".to_string()))?;
        let second_llm = second
            .llm_client()
            .ok_or_else(|| ReactError::Other("second Session has no LLM client".to_string()))?;
        assert!(Arc::ptr_eq(first_llm, second_llm));
        Agent::close(&first).await?;
        Agent::close(&second).await?;
        Ok(())
    }

    #[tokio::test]
    async fn definition_snapshot_reports_construction_facts() -> Result<()> {
        let snapshot = factory()?.definition().snapshot();
        // AgentSettings::default() names the agent "assistant"; the model
        // name comes from the fixture config.
        assert_eq!(snapshot.name, "assistant");
        assert_eq!(snapshot.model_name, "fixture-model");
        assert!(snapshot.host_default);
        Ok(())
    }

    #[tokio::test]
    #[cfg(feature = "sdk-core-profile")]
    async fn core_profile_factory_binds_each_session_to_its_definition() -> Result<()> {
        let standard = factory()?;
        let default = standard.definition.clone();
        let explicit_framework = FrameworkConfig {
            model: ModelConfig {
                provider: "local".to_string(),
                name: "explicit-model".to_string(),
                base_url: Some("http://127.0.0.1:9/v1/chat/completions".to_string()),
                api_protocol: Some(LlmApiProtocol::ChatCompletions),
                ..ModelConfig::default()
            },
            agent: AgentSettings {
                name: "explicit-agent".to_string(),
                ..AgentSettings::default()
            },
        };
        let explicit_client = LlmConfig::for_provider(
            "local",
            "http://127.0.0.1:9/v1/chat/completions",
            "",
            "explicit-model",
            LlmApiProtocol::ChatCompletions,
        )?
        .build_client()?;
        let explicit = Arc::new(PreparedAgentDefinition::new(
            explicit_framework,
            Arc::from(explicit_client),
            None,
            false,
        ));
        let profile = CoreProfileSessionFactory::new(default, 8);
        profile
            .register_definition(explicit.clone())
            .map_err(ReactError::Other)?;
        let mut meta = agent_client_protocol::schema::v1::Meta::new();
        meta.insert(
            DEFINITION_META_KEY.to_string(),
            serde_json::Value::String(explicit.id().to_string()),
        );
        let agent = profile
            .create_session(context(
                "explicit-session",
                PathBuf::from("/tmp"),
                Vec::new(),
            ))
            .await?;
        assert_eq!(agent.model_name(), "fixture-model");
        let explicit_context = AcpSessionContext {
            meta: Some(meta),
            ..context("explicit-session-2", PathBuf::from("/tmp"), Vec::new())
        };
        assert!(
            explicit_context
                .meta
                .as_ref()
                .and_then(|meta| meta.get(DEFINITION_META_KEY))
                .is_some()
        );
        let explicit_agent = profile.create_session(explicit_context).await?;
        assert_eq!(explicit_agent.model_name(), "explicit-model");
        Agent::close(agent.as_ref()).await?;
        Agent::close(explicit_agent.as_ref()).await?;
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn later_mcp_failure_closes_previously_connected_process() -> Result<()> {
        let directory = tempfile::tempdir().map_err(ReactError::Io)?;
        let session_cwd = directory.path().join("session");
        std::fs::create_dir_all(&session_cwd).map_err(ReactError::Io)?;
        let script = directory.path().join("mcp-fixture.sh");
        let pid_file = directory.path().join("mcp.pid");
        let cwd_file = directory.path().join("mcp.cwd");
        std::fs::write(&script, MCP_FIXTURE_SCRIPT).map_err(ReactError::Io)?;
        let first = McpServer::Stdio(McpServerStdio::new("first", PathBuf::from("/bin/sh")).args(
            vec![
                script.to_string_lossy().into_owned(),
                pid_file.to_string_lossy().into_owned(),
                cwd_file.to_string_lossy().into_owned(),
            ],
        ));
        let missing_command = directory.path().join("missing-mcp-command");
        let second = McpServer::Stdio(McpServerStdio::new("second", missing_command));

        let result = factory()?
            .definition()
            .create_agent(&context(
                "mcp-cleanup",
                session_cwd.clone(),
                vec![first, second],
            ))
            .await;
        assert!(result.is_err());
        let pid = std::fs::read_to_string(&pid_file)
            .map_err(ReactError::Io)?
            .trim()
            .parse::<u32>()
            .map_err(|error| ReactError::Other(format!("invalid MCP fixture pid: {error}")))?;
        let recorded_cwd = PathBuf::from(
            std::fs::read_to_string(&cwd_file)
                .map_err(ReactError::Io)?
                .trim(),
        );
        assert_eq!(
            std::fs::canonicalize(recorded_cwd).map_err(ReactError::Io)?,
            std::fs::canonicalize(session_cwd).map_err(ReactError::Io)?
        );
        wait_for_process_exit(pid).await?;
        Ok(())
    }

    #[cfg(unix)]
    async fn wait_for_process_exit(pid: u32) -> Result<()> {
        for _ in 0..100 {
            if !process_is_alive(pid)? {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Err(ReactError::Other(format!(
            "MCP fixture process {pid} remained alive after Agent cleanup"
        )))
    }

    #[cfg(unix)]
    fn process_is_alive(pid: u32) -> Result<bool> {
        std::process::Command::new("/bin/kill")
            .arg("-0")
            .arg(pid.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .map_err(ReactError::Io)
    }

    #[cfg(unix)]
    const MCP_FIXTURE_SCRIPT: &str = r#"#!/bin/sh
pid_file="$1"
cwd_file="$2"
printf '%s' "$$" > "$pid_file"
pwd > "$cwd_file"
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      tail=${line#*\"id\":}
      id=${tail%%,*}
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-03-26","capabilities":{}}}\n' "$id"
      ;;
  esac
done
"#;
}
