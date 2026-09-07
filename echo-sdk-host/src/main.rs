use echo_sdk_host::{HELP, HostCommand, VERSION, parse_args, run_stdio, validate_config};
use std::process::ExitCode;
use tracing_subscriber::EnvFilter;

const MAX_ERROR_CHARS: usize = 1024;

#[tokio::main]
async fn main() -> ExitCode {
    let command = match parse_args(std::env::args().skip(1)) {
        Ok(command) => command,
        Err(error) => return fail(&error),
    };
    match command {
        HostCommand::Help => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        HostCommand::Version => {
            println!("echo-agent-sdk-host {VERSION}");
            ExitCode::SUCCESS
        }
        HostCommand::CheckConfig { config } => match validate_config(config) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => fail(&error),
        },
        HostCommand::Run { config } => {
            if let Err(error) = init_tracing() {
                return fail_text(&format!("failed to initialize stderr tracing: {error}"));
            }
            match run_stdio(config).await {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => fail(&error),
            }
        }
    }
}

fn init_tracing() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            // The official runtime's "Handler errored" warn formats the whole handler
            // chain as Debug; that nested-generic string grows exponentially with every
            // registered handler (minutes of formatting past ~30 handlers) and a plain
            // client's method-not-found fires it on every forced extension call. The
            // typed error still reaches the client through the responder; the log line
            // is pure diagnostics, so the Host pins that target to .
            EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                EnvFilter::new("warn,agent_client_protocol::jsonrpc::incoming_actor=error")
            }),
        )
        .with_writer(std::io::stderr)
        .try_init()
}

fn fail(error: &impl std::fmt::Display) -> ExitCode {
    fail_text(&error.to_string())
}

fn fail_text(message: &str) -> ExitCode {
    let bounded: String = message.chars().take(MAX_ERROR_CHARS).collect();
    eprintln!("echo-agent-sdk-host: {bounded}");
    ExitCode::from(2)
}
