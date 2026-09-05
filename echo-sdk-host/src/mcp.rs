use agent_client_protocol::schema::v1::McpServer;
use echo_agent::acp::AcpSessionContext;
use echo_agent::error::{ReactError, Result};
use echo_agent::mcp::{McpServerConfig, TransportConfig};
use std::collections::BTreeSet;

pub(crate) fn translate_mcp_servers(context: &AcpSessionContext) -> Result<Vec<McpServerConfig>> {
    if !context.additional_directories.is_empty() {
        return Err(ReactError::Other(
            "additionalDirectories are not supported by the standard Host profile".to_string(),
        ));
    }
    let mut names = BTreeSet::new();
    let mut translated = Vec::with_capacity(context.mcp_servers.len());
    for server in &context.mcp_servers {
        let McpServer::Stdio(server) = server else {
            return Err(ReactError::Other(
                "only ACP stdio MCP servers are supported by the standard Host profile".to_string(),
            ));
        };
        if server.name.trim().is_empty() {
            return Err(ReactError::Other(
                "ACP MCP server name must not be empty".to_string(),
            ));
        }
        if !names.insert(server.name.clone()) {
            return Err(ReactError::Other(format!(
                "duplicate ACP MCP server name: {}",
                server.name
            )));
        }
        if !server.command.is_absolute() {
            return Err(ReactError::Other(format!(
                "ACP stdio MCP command for {} must be absolute",
                server.name
            )));
        }
        let command = server.command.to_str().ok_or_else(|| {
            ReactError::Other(format!(
                "ACP stdio MCP command for {} must be valid UTF-8",
                server.name
            ))
        })?;
        let env = server
            .env
            .iter()
            .map(|entry| (entry.name.clone(), entry.value.clone()))
            .collect::<Vec<_>>();
        translated.push(McpServerConfig {
            name: server.name.clone(),
            transport: TransportConfig::Stdio {
                command: command.to_string(),
                args: server.args.clone(),
                env,
                cwd: Some(context.cwd.clone()),
            },
        });
    }
    Ok(translated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{
        ClientCapabilities, EnvVariable, McpServerHttp, McpServerStdio, SessionId,
    };
    use std::path::PathBuf;

    fn context(servers: Vec<McpServer>) -> AcpSessionContext {
        AcpSessionContext {
            session_id: SessionId::new("session"),
            cwd: std::env::temp_dir(),
            additional_directories: Vec::new(),
            mcp_servers: servers,
            client_capabilities: ClientCapabilities::default(),
            meta: None,
        }
    }

    #[test]
    fn stdio_mcp_fields_are_preserved() -> Result<()> {
        let command = std::env::temp_dir().join("fixture-mcp");
        let server = McpServer::Stdio(
            McpServerStdio::new("fixture", command.clone())
                .args(vec!["--stdio".to_string()])
                .env(vec![EnvVariable::new("TOKEN", "value")]),
        );
        let context = context(vec![server]);
        let expected_cwd = context.cwd.clone();
        let translated = translate_mcp_servers(&context)?;
        assert_eq!(translated.len(), 1);
        assert_eq!(
            translated.first().map(|server| server.name.as_str()),
            Some("fixture")
        );
        let transport = translated
            .first()
            .map(|server| &server.transport)
            .ok_or_else(|| ReactError::Other("missing translated MCP server".to_string()))?;
        match transport {
            TransportConfig::Stdio {
                command: actual_command,
                args,
                env,
                cwd,
            } => {
                assert_eq!(Some(actual_command.as_str()), command.to_str());
                assert_eq!(args, &["--stdio".to_string()]);
                assert_eq!(env, &[("TOKEN".to_string(), "value".to_string())]);
                assert_eq!(cwd.as_ref(), Some(&expected_cwd));
            }
            TransportConfig::Http { .. } | TransportConfig::Sse { .. } => {
                return Err(ReactError::Other(
                    "stdio MCP converted to a remote transport".to_string(),
                ));
            }
        }
        Ok(())
    }

    #[test]
    fn invalid_or_unadvertised_mcp_inputs_are_rejected() {
        let relative = McpServer::Stdio(McpServerStdio::new(
            "relative",
            PathBuf::from("relative-command"),
        ));
        assert!(translate_mcp_servers(&context(vec![relative])).is_err());

        let duplicate_command = std::env::temp_dir().join("fixture-mcp");
        let duplicate = vec![
            McpServer::Stdio(McpServerStdio::new("same", duplicate_command.clone())),
            McpServer::Stdio(McpServerStdio::new("same", duplicate_command)),
        ];
        assert!(translate_mcp_servers(&context(duplicate)).is_err());

        let remote = McpServer::Http(McpServerHttp::new("remote", "https://example.com/mcp"));
        assert!(translate_mcp_servers(&context(vec![remote])).is_err());

        let mut extra_root = context(Vec::new());
        extra_root.additional_directories.push(std::env::temp_dir());
        assert!(translate_mcp_servers(&extra_root).is_err());
    }
}
