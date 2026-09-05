use crate::mcp::translate_mcp_servers;
use agent_client_protocol::BoxFuture;
use echo_agent::acp::{AcpSessionContext, AcpSessionFactory};
use echo_agent::agent::{Agent, AgentConfig, ReactAgent};
use echo_agent::config::FrameworkConfig;
use echo_agent::error::{ReactError, Result};
use echo_agent::llm::LlmClient;
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct DefaultHostSessionFactory {
    framework: Arc<FrameworkConfig>,
    llm_client: Arc<dyn LlmClient>,
}

impl DefaultHostSessionFactory {
    pub fn new(framework: FrameworkConfig, llm_client: Arc<dyn LlmClient>) -> Self {
        Self {
            framework: Arc::new(framework),
            llm_client,
        }
    }

    async fn create_agent(&self, context: AcpSessionContext) -> Result<ReactAgent> {
        let mcp_servers = translate_mcp_servers(&context)?;
        let session_id = context.session_id.to_string();
        let agent_config = AgentConfig::from(self.framework.as_ref().clone())
            .session_id(&session_id)
            .conversation_id(&session_id)
            .working_dir(Some(context.cwd));
        let mut agent = ReactAgent::new(agent_config).with_llm_client(self.llm_client.clone());
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

impl AcpSessionFactory for DefaultHostSessionFactory {
    fn create_session(
        &self,
        context: AcpSessionContext,
    ) -> BoxFuture<'static, Result<Box<dyn Agent>>> {
        let factory = self.clone();
        Box::pin(async move {
            factory
                .create_agent(context)
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
            .create_agent(context("session-a", first_cwd.clone(), Vec::new()))
            .await?;
        let second = factory
            .create_agent(context("session-b", second_cwd.clone(), Vec::new()))
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
            .create_agent(context(
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
