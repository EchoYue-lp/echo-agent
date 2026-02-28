//! Echo Agent CLI
//!
//! 通过命令行与 AI 智能体进行交互。
//!
//! # 交互模式（默认，stdin 为 TTY 时）
//! ```bash
//! cargo run
//! cargo run -- --tools math,files,shell
//! cargo run -- --compressor summary --token-limit 4000
//! cargo run -- --skills-dir ./skills
//! ```
//!
//! # MCP 服务端接入
//! ```bash
//! # 从 YAML 配置文件加载
//! cargo run -- --mcp mcp.yaml
//!
//! # 内联 stdio 模式（格式: "名称 命令 参数..."）
//! cargo run -- --mcp-stdio "fs npx -y @modelcontextprotocol/server-filesystem /tmp"
//!
//! # 内联 HTTP 模式（格式: "名称 URL [Header=Value...]"）
//! cargo run -- --mcp-http "api http://localhost:3000 Authorization=Bearer token"
//!
//! # 混合使用
//! cargo run -- --mcp mcp.yaml --mcp-stdio "extra npx -y @mcp/extra" --tools shell
//! ```
//!
//! # mcp.yaml 配置文件格式
//! ```yaml
//! servers:
//!   - name: filesystem
//!     stdio:
//!       command: npx
//!       args: ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
//!       env:              # 可选，额外环境变量
//!         NODE_ENV: production
//!
//!   - name: my-api
//!     http:
//!       url: "http://localhost:3000"
//!       headers:          # 可选，自定义请求头
//!         Authorization: "Bearer mytoken"
//! ```
//!
//! # 单次查询 / 管道模式（stdin 非 TTY 时自动切换）
//! ```bash
//! cargo run -- -q "帮我计算 1+1" --tools math
//! echo "列出当前目录" | cargo run -- --tools files
//! ```

use clap::Parser;
use echo_agent::agent::react_agent::ReactAgent;
use echo_agent::agent::{Agent, AgentConfig, AgentEvent};
use echo_agent::compression::ContextCompressor;
use echo_agent::compression::compressor::{
    DefaultSummaryPrompt, HybridCompressor, SlidingWindowCompressor, SummaryCompressor,
};
use echo_agent::error::ReactError;
use echo_agent::llm::DefaultLlmClient;
use echo_agent::mcp::{McpManager, McpServerConfig, TransportConfig};
use echo_agent::skills::builtin::{CalculatorSkill, FileSystemSkill, ShellSkill, WeatherSkill};
use echo_agent::tools::Tool;
use futures::StreamExt;
use reqwest::Client;
use rustyline::DefaultEditor;
use serde::Deserialize;
use std::collections::HashMap;
use std::io::{self, BufRead, IsTerminal, Write};
use std::sync::Arc;

// ── CLI 参数定义 ──────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(
    name = "echo-agent",
    about = "Echo Agent CLI —— 基于 ReAct 的 AI 智能体交互终端",
    long_about = "Echo Agent 是一个支持工具调用、任务规划、流式输出的 AI 智能体框架。\n\
                  通过此 CLI 可以在终端中与智能体进行交互对话。\n\n\
                  需要在 .env 中配置 LLM 接口，格式：\n\
                    AGENT_MODEL_<ID>_MODEL=<model-name>\n\
                    AGENT_MODEL_<ID>_BASEURL=<api-url>\n\
                    AGENT_MODEL_<ID>_APIKEY=<api-key>",
    version
)]
struct Cli {
    /// 使用的模型名称（需在 .env 中配置对应的 AGENT_MODEL_*_* 环境变量）
    #[arg(short, long, default_value = "qwen3-max", env = "ECHO_MODEL")]
    model: String,

    /// 系统提示词（不填则使用默认通用助手提示词）
    #[arg(short, long)]
    system: Option<String>,

    /// 启用的内置工具集，逗号分隔（可选: math, weather, files, shell）
    #[arg(short, long)]
    tools: Option<String>,

    /// 单次查询模式：发送一条消息后打印结果并退出
    #[arg(short, long)]
    query: Option<String>,

    /// 从指定目录加载外部技能（包含 SKILL.md 的子目录）
    #[arg(long)]
    skills_dir: Option<String>,

    // ── MCP 配置 ────────────────────────────────────────────────────────────
    /// MCP 服务端配置文件路径（YAML 格式）
    ///
    /// 配置文件格式:
    ///   servers:
    ///     - name: filesystem
    ///       stdio:
    ///         command: npx
    ///         args: ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
    ///     - name: my-api
    ///       http:
    ///         url: "http://localhost:3000"
    ///         headers:
    ///           Authorization: "Bearer token"
    #[arg(long, value_name = "FILE")]
    mcp: Option<String>,

    /// 添加 stdio 模式的 MCP 服务端（可多次指定）
    ///
    /// 格式: "名称 命令 [参数...]"
    /// 示例: --mcp-stdio "fs npx -y @modelcontextprotocol/server-filesystem /tmp"
    #[arg(long, value_name = "SPEC")]
    mcp_stdio: Vec<String>,

    /// 添加 HTTP 模式的 MCP 服务端（可多次指定）
    ///
    /// 格式: "名称 URL [Header=Value ...]"
    /// 示例: --mcp-http "api http://localhost:3000 Authorization=Bearer_token"
    #[arg(long, value_name = "SPEC")]
    mcp_http: Vec<String>,

    // ── 运行时配置 ───────────────────────────────────────────────────────────
    /// 日志级别（trace, debug, info, warn, error）
    #[arg(long, default_value = "warn")]
    log_level: String,

    /// 最大推理迭代次数
    #[arg(long, default_value = "20")]
    max_iter: usize,

    /// 禁用流式输出（等待完整响应后一次性打印）
    #[arg(long)]
    no_stream: bool,

    /// 启用 human-in-loop（agent 在关键操作前征询用户确认）
    #[arg(long)]
    human_loop: bool,

    /// 上下文压缩策略（summary[:N] / sliding[:N] / hybrid[:N] / none）
    ///
    /// 作为 /compress 命令的默认策略，并在设置 --token-limit 后自动触发。
    /// 示例: --compressor summary:4  --compressor sliding:20
    #[arg(long, default_value = "summary")]
    compressor: String,

    /// 上下文 token 上限，超过后在每次 LLM 调用前自动触发压缩
    #[arg(long)]
    token_limit: Option<usize>,

    /// 每轮对话结束后显示上下文统计信息（消息数 / 估算 token 数）
    #[arg(long)]
    ctx_stats: bool,
}

// ── MCP 配置文件结构体 ────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct McpConfigFile {
    servers: Vec<McpServerYaml>,
}

#[derive(Deserialize)]
struct McpServerYaml {
    name: String,
    #[serde(flatten)]
    transport: McpTransportYaml,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum McpTransportYaml {
    Stdio(McpStdioYaml),
    Http(McpHttpYaml),
}

#[derive(Deserialize)]
struct McpStdioYaml {
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: HashMap<String, String>,
}

#[derive(Deserialize)]
struct McpHttpYaml {
    url: String,
    #[serde(default)]
    headers: HashMap<String, String>,
}

// ── 入口 ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    let log_filter = std::env::var("RUST_LOG").unwrap_or_else(|_| cli.log_level.clone());
    tracing_subscriber::fmt()
        .with_env_filter(log_filter)
        .without_time()
        .with_target(false)
        .init();

    dotenv::dotenv().ok();

    let enabled_tools: Vec<&str> = cli
        .tools
        .as_deref()
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();

    // 是否存在 MCP 配置（影响 enable_tool 默认值）
    let has_mcp_config = cli.mcp.is_some() || !cli.mcp_stdio.is_empty() || !cli.mcp_http.is_empty();

    let http = Arc::new(Client::new());
    let mut agent = build_agent(&cli, &enabled_tools, &http, has_mcp_config);

    // 加载外部技能
    if let Some(ref dir) = cli.skills_dir {
        match agent.load_skills_from_dir(dir).await {
            Ok(names) if !names.is_empty() => {
                println!("已加载外部技能: {}\n", names.join(", "))
            }
            Ok(_) => eprintln!("警告: 技能目录 '{dir}' 中未找到有效技能"),
            Err(e) => eprintln!("警告: 加载技能失败: {e}"),
        }
    }

    // 连接 MCP 服务端
    let (mcp_manager, mcp_tools) = connect_mcp(&cli).await;
    if !mcp_tools.is_empty() {
        agent.add_tools(mcp_tools);
    }

    if let Some(query) = &cli.query {
        run_single_query(&mut agent, query, cli.no_stream).await?;
    } else if !io::stdin().is_terminal() {
        run_pipe_mode(&mut agent, cli.no_stream).await?;
    } else {
        print_banner(&cli, &enabled_tools, &mcp_manager);
        run_interactive(&mut agent, &cli, &http, &mcp_manager).await?;
    }

    // 程序退出前关闭所有 MCP 连接
    mcp_manager.close_all().await;

    Ok(())
}

// ── Agent 构建 ────────────────────────────────────────────────────────────────

fn build_agent(cli: &Cli, tools: &[&str], http: &Arc<Client>, has_mcp: bool) -> ReactAgent {
    let has_tools = !tools.is_empty() || has_mcp;

    let default_system = if has_tools {
        "你是一个能力全面的 AI 助手，可以使用工具来完成任务。\
         完成后用 final_answer 给出完整的结论。"
    } else {
        "你是一个知识渊博的 AI 助手，用中文简洁、准确地回答用户的问题。"
    };

    let system_prompt = cli.system.as_deref().unwrap_or(default_system);

    let mut config = AgentConfig::new(&cli.model, "echo-agent", system_prompt)
        .enable_tool(has_tools)
        .enable_task(false)
        .enable_human_in_loop(cli.human_loop)
        .enable_subagent(false)
        .max_iterations(cli.max_iter);

    if let Some(limit) = cli.token_limit {
        config = config.token_limit(limit);
    }

    let mut agent = ReactAgent::new(config);

    for tool_name in tools {
        match *tool_name {
            "math" => agent.add_skill(Box::new(CalculatorSkill)),
            "weather" => agent.add_skill(Box::new(WeatherSkill)),
            "files" => agent.add_skill(Box::new(FileSystemSkill::new())),
            "shell" => agent.add_skill(Box::new(ShellSkill::new())),
            other => {
                eprintln!("警告: 未知工具 '{other}'，已跳过（可选: math, weather, files, shell）")
            }
        }
    }

    if cli.compressor != "none" {
        let (kind, n) = parse_compressor_spec(&cli.compressor);
        if let Some(c) = build_compressor(kind, n, &cli.model, http) {
            agent.set_compressor(c);
        }
    }

    agent
}

// ── MCP 连接 ──────────────────────────────────────────────────────────────────

/// 根据 CLI 参数连接所有 MCP 服务端，返回管理器和工具列表。
async fn connect_mcp(cli: &Cli) -> (McpManager, Vec<Box<dyn Tool>>) {
    let mut manager = McpManager::new();
    let mut all_tools: Vec<Box<dyn Tool>> = Vec::new();

    let configs = collect_mcp_configs(cli);
    if configs.is_empty() {
        return (manager, all_tools);
    }

    println!("正在连接 MCP 服务端...");

    for config in configs {
        let name = config.name.clone();
        let transport_desc = describe_transport(&config.transport);

        match manager.connect(config).await {
            Ok(tools) => {
                println!(
                    "  ✓ {} ({})  —  {} 个工具",
                    name,
                    transport_desc,
                    tools.len()
                );
                all_tools.extend(tools);
            }
            Err(e) => {
                eprintln!("  ✗ {} 连接失败: {}", name, e);
            }
        }
    }

    if !all_tools.is_empty() {
        println!("MCP 就绪，共注册 {} 个工具\n", all_tools.len());
    }

    (manager, all_tools)
}

/// 收集所有来源（配置文件 + 命令行内联）的 MCP 服务端配置。
fn collect_mcp_configs(cli: &Cli) -> Vec<McpServerConfig> {
    let mut configs = Vec::new();

    // 从 YAML 文件加载
    if let Some(ref path) = cli.mcp {
        match load_mcp_config_file(path) {
            Ok(file_configs) => configs.extend(file_configs),
            Err(e) => eprintln!("警告: 读取 MCP 配置文件 '{path}' 失败: {e}"),
        }
    }

    // 内联 stdio 服务端
    for spec in &cli.mcp_stdio {
        match parse_mcp_stdio_spec(spec) {
            Some(config) => configs.push(config),
            None => eprintln!("警告: --mcp-stdio 格式错误 '{spec}'（格式: 名称 命令 [参数...]）"),
        }
    }

    // 内联 HTTP 服务端
    for spec in &cli.mcp_http {
        match parse_mcp_http_spec(spec) {
            Some(config) => configs.push(config),
            None => {
                eprintln!("警告: --mcp-http 格式错误 '{spec}'（格式: 名称 URL [Header=Value ...]）")
            }
        }
    }

    configs
}

/// 解析 YAML 配置文件，返回 McpServerConfig 列表。
fn load_mcp_config_file(path: &str) -> Result<Vec<McpServerConfig>, Box<dyn std::error::Error>> {
    let content = std::fs::read_to_string(path)?;
    let file: McpConfigFile = serde_yaml::from_str(&content)?;

    let configs = file
        .servers
        .into_iter()
        .map(|def| match def.transport {
            McpTransportYaml::Stdio(s) => McpServerConfig {
                name: def.name,
                transport: TransportConfig::Stdio {
                    command: s.command,
                    args: s.args,
                    env: s.env.into_iter().collect(),
                },
            },
            McpTransportYaml::Http(h) => McpServerConfig {
                name: def.name,
                transport: TransportConfig::Http {
                    base_url: h.url,
                    headers: h.headers,
                },
            },
        })
        .collect();

    Ok(configs)
}

/// 解析 stdio 内联格式："名称 命令 [参数...]"
fn parse_mcp_stdio_spec(spec: &str) -> Option<McpServerConfig> {
    let mut parts = spec.splitn(3, ' ').map(str::trim);
    let name = parts.next().filter(|s| !s.is_empty())?;
    let command = parts.next().filter(|s| !s.is_empty())?;
    // 剩余部分按空格分割为参数
    let args: Vec<String> = parts
        .next()
        .unwrap_or("")
        .split_whitespace()
        .map(String::from)
        .collect();

    Some(McpServerConfig {
        name: name.to_string(),
        transport: TransportConfig::Stdio {
            command: command.to_string(),
            args,
            env: vec![],
        },
    })
}

/// 解析 HTTP 内联格式："名称 URL [Key=Value ...]"
fn parse_mcp_http_spec(spec: &str) -> Option<McpServerConfig> {
    let mut parts = spec.splitn(3, ' ').map(str::trim);
    let name = parts.next().filter(|s| !s.is_empty())?;
    let url = parts.next().filter(|s| !s.is_empty())?;

    // 额外的 Key=Value 解析为请求头
    let headers: HashMap<String, String> = parts
        .next()
        .unwrap_or("")
        .split_whitespace()
        .filter_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            // 支持 Bearer_token 格式（下划线替换为空格）
            Some((k.to_string(), v.replace('_', " ")))
        })
        .collect();

    Some(McpServerConfig {
        name: name.to_string(),
        transport: TransportConfig::Http {
            base_url: url.to_string(),
            headers,
        },
    })
}

fn describe_transport(transport: &TransportConfig) -> String {
    match transport {
        TransportConfig::Stdio { command, .. } => format!("stdio: {command}"),
        TransportConfig::Http { base_url, .. } => {
            // 只取 host:port 部分，避免太长
            let short = base_url
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .split('/')
                .next()
                .unwrap_or(base_url);
            format!("http: {short}")
        }
    }
}

// ── 压缩器工厂 ────────────────────────────────────────────────────────────────

fn build_compressor(
    kind: &str,
    n: Option<usize>,
    model: &str,
    http: &Arc<Client>,
) -> Option<Box<dyn ContextCompressor>> {
    match kind {
        "summary" | "sum" => {
            let keep = n.unwrap_or(6);
            let llm = Arc::new(DefaultLlmClient::new(http.clone(), model));
            Some(Box::new(SummaryCompressor::new(
                llm,
                DefaultSummaryPrompt,
                keep,
            )))
        }
        "sliding" | "slide" | "window" => {
            let window = n.unwrap_or(20);
            Some(Box::new(SlidingWindowCompressor::new(window)))
        }
        "hybrid" => {
            let window = n.unwrap_or(10);
            let keep = (window / 2).max(2);
            let llm = Arc::new(DefaultLlmClient::new(http.clone(), model));
            Some(Box::new(
                HybridCompressor::builder()
                    .stage(SlidingWindowCompressor::new(window))
                    .stage(SummaryCompressor::new(llm, DefaultSummaryPrompt, keep))
                    .build(),
            ))
        }
        "none" | "" => None,
        other => {
            eprintln!(
                "警告: 未知压缩策略 '{other}'，已忽略（可选: summary, sliding, hybrid, none）"
            );
            None
        }
    }
}

fn parse_compressor_spec(spec: &str) -> (&str, Option<usize>) {
    match spec.split_once(':') {
        Some((kind, n_str)) => (kind, n_str.parse().ok()),
        None => (spec, None),
    }
}

// ── 单次查询模式 ──────────────────────────────────────────────────────────────

async fn run_single_query(
    agent: &mut ReactAgent,
    query: &str,
    no_stream: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if no_stream {
        println!("{}", agent.execute(query).await?);
    } else {
        stream_execute(agent, query, false).await?;
        println!();
    }
    Ok(())
}

// ── 管道模式 ──────────────────────────────────────────────────────────────────

async fn run_pipe_mode(
    agent: &mut ReactAgent,
    no_stream: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let query = line?;
        let query = query.trim().to_string();
        if query.is_empty() || query.starts_with('#') {
            continue;
        }
        if no_stream {
            match agent.execute(&query).await {
                Ok(answer) => println!("{}", answer),
                Err(e) => eprintln!("错误: {e:?}"),
            }
        } else {
            if let Err(e) = stream_execute(agent, &query, false).await {
                eprintln!("错误: {e:?}");
            }
            println!();
        }
    }
    Ok(())
}

// ── 交互模式 ──────────────────────────────────────────────────────────────────

async fn run_interactive(
    agent: &mut ReactAgent,
    cli: &Cli,
    http: &Arc<Client>,
    mcp: &McpManager,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut rl = DefaultEditor::new()?;
    let history_path = home_dir().map(|h| h.join(".echo_agent_history"));
    if let Some(ref path) = history_path {
        let _ = rl.load_history(path);
    }

    println!("输入 /help 查看所有命令，/quit 或 Ctrl+D 退出。\n");

    loop {
        let readline = rl.readline("you > ");
        match readline {
            Ok(line) => {
                let input = line.trim().to_string();
                if input.is_empty() {
                    continue;
                }

                // 解析命令名和参数
                let (cmd, arg) = if input.starts_with('/') {
                    match input.splitn(2, ' ').collect::<Vec<_>>().as_slice() {
                        [c, a] => (*c, a.trim()),
                        [c] => (*c, ""),
                        _ => (input.as_str(), ""),
                    }
                } else {
                    let _ = rl.add_history_entry(&input);
                    ("", "")
                };

                match cmd {
                    "/quit" | "/exit" | "/q" => {
                        println!("\n再见！");
                        break;
                    }
                    "/help" | "/h" => {
                        print_help();
                        continue;
                    }
                    "/clear" | "/cls" => {
                        print!("\x1B[2J\x1B[1;1H");
                        io::stdout().flush().ok();
                        continue;
                    }
                    "/model" => {
                        if arg.is_empty() {
                            println!("当前模型: {}\n", agent.model_name());
                        } else {
                            agent.set_model(arg);
                            println!("模型已切换至: {arg}\n");
                        }
                        continue;
                    }
                    "/system" => {
                        println!("系统提示词:\n{}\n", agent.system_prompt());
                        continue;
                    }
                    "/ctx" | "/context" => {
                        let (count, tokens) = agent.context_stats();
                        println!("上下文: {} 条消息  /  ~{} tokens\n", count, tokens);
                        continue;
                    }
                    "/tools" => {
                        let tools = agent.list_tools();
                        if tools.is_empty() {
                            println!("（未注册任何工具）\n");
                        } else {
                            println!("已注册工具 ({}):", tools.len());
                            for t in &tools {
                                println!("  • {t}");
                            }
                            println!();
                        }
                        continue;
                    }
                    "/skills" => {
                        let skills = agent.list_skills();
                        if skills.is_empty() {
                            println!("（未安装任何技能）\n");
                        } else {
                            println!("已安装技能 ({}):", skills.len());
                            for s in &skills {
                                println!("  • {}  —  {}", s.name, s.description);
                                if !s.tool_names.is_empty() {
                                    println!("    工具: {}", s.tool_names.join(", "));
                                }
                            }
                            println!();
                        }
                        continue;
                    }
                    "/mcp" => {
                        print_mcp_status(mcp);
                        continue;
                    }
                    "/reset" => {
                        agent.reset();
                        println!("上下文已重置，仅保留系统提示词。\n");
                        continue;
                    }
                    "/compress" => {
                        let (strategy, keep_n) = parse_compress_args(arg);
                        run_compress(agent, &strategy, keep_n, &cli.compressor, &cli.model, http)
                            .await;
                        println!();
                        continue;
                    }
                    c if c.starts_with('/') => {
                        println!("未知命令: {c}（输入 /help 查看帮助）\n");
                        continue;
                    }
                    _ => {}
                }

                // 执行对话
                print!("\nagent > ");
                io::stdout().flush().ok();

                let res: Result<(), ReactError> = if cli.no_stream {
                    match agent.execute(&input).await {
                        Ok(answer) => {
                            println!("{}", answer);
                            Ok(())
                        }
                        Err(e) => Err(e),
                    }
                } else {
                    stream_execute(agent, &input, cli.ctx_stats).await
                };

                println!();
                if let Err(e) = res {
                    eprintln!("错误: {e:?}\n");
                } else if cli.ctx_stats && cli.no_stream {
                    let (count, tokens) = agent.context_stats();
                    println!("  (上下文: {} 条 / ~{} tokens)\n", count, tokens);
                }
            }
            Err(rustyline::error::ReadlineError::Interrupted) => {
                println!("\n（使用 /quit 退出，或按 Ctrl+D）");
            }
            Err(rustyline::error::ReadlineError::Eof) => {
                println!("\n再见！");
                break;
            }
            Err(e) => {
                eprintln!("输入错误: {e}");
                break;
            }
        }
    }

    if let Some(ref path) = history_path {
        let _ = rl.save_history(path);
    }
    Ok(())
}

// ── /mcp 命令 ─────────────────────────────────────────────────────────────────

fn print_mcp_status(mcp: &McpManager) {
    let names = mcp.server_names();
    if names.is_empty() {
        println!("未连接任何 MCP 服务端。");
        println!("启动时使用 --mcp 或 --mcp-stdio / --mcp-http 接入服务端。\n");
        return;
    }

    println!("已连接 MCP 服务端 ({}):", names.len());
    for name in &names {
        if let Some(client) = mcp.get_client(name) {
            let tools = client.tools();
            let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
            println!("  • {}  —  {} 个工具", name, tools.len());
            if !tool_names.is_empty() {
                // 最多展示 8 个工具名，超出显示省略
                let preview: Vec<&str> = tool_names.iter().copied().take(8).collect();
                let suffix = if tool_names.len() > 8 {
                    format!("… (+{})", tool_names.len() - 8)
                } else {
                    String::new()
                };
                println!("    工具: {}{}", preview.join(", "), suffix);
            }
        }
    }
    println!();
}

// ── /compress 命令处理 ────────────────────────────────────────────────────────

fn parse_compress_args(arg: &str) -> (String, Option<usize>) {
    let parts: Vec<&str> = arg.splitn(2, ' ').collect();
    let strategy = parts.first().copied().unwrap_or("").to_string();
    let keep_n = parts.get(1).and_then(|s| s.trim().parse().ok());
    (strategy, keep_n)
}

async fn run_compress(
    agent: &mut ReactAgent,
    strategy: &str,
    keep_n: Option<usize>,
    default_compressor: &str,
    model: &str,
    http: &Arc<Client>,
) {
    let (before_count, before_tokens) = agent.context_stats();

    if before_count <= 1 {
        println!("上下文只有系统消息，无需压缩。");
        return;
    }

    let (kind, n) = if strategy.is_empty() {
        parse_compressor_spec(default_compressor)
    } else {
        parse_compressor_spec(strategy)
    };
    let effective_n = keep_n.or(n);

    println!(
        "正在压缩… (策略: {}{}，压缩前: {} 条 / ~{} tokens)",
        kind,
        effective_n.map(|n| format!(":{n}")).unwrap_or_default(),
        before_count,
        before_tokens
    );
    io::stdout().flush().ok();

    let compressor = build_compressor(kind, effective_n, model, http);
    let Some(c) = compressor else {
        eprintln!("未知策略: '{kind}'（可选: summary, sliding, hybrid）");
        return;
    };

    match agent.force_compress_with(c.as_ref()).await {
        Ok(stats) if stats.evicted == 0 => {
            println!("消息数未超过保留阈值，未裁剪任何内容。");
        }
        Ok(stats) => {
            let saved = stats.before_tokens.saturating_sub(stats.after_tokens);
            println!(
                "压缩完成: {} → {} 条  |  ~{} → ~{} tokens  |  节省 ~{} tokens（裁剪 {} 条）",
                stats.before_count,
                stats.after_count,
                stats.before_tokens,
                stats.after_tokens,
                saved,
                stats.evicted,
            );
        }
        Err(e) => eprintln!("压缩失败: {e:?}"),
    }
}

// ── 流式执行 ──────────────────────────────────────────────────────────────────

async fn stream_execute(
    agent: &mut ReactAgent,
    task: &str,
    show_ctx_stats: bool,
) -> Result<(), ReactError> {
    let result = stream_run(agent, task).await;
    if show_ctx_stats {
        let (count, tokens) = agent.context_stats();
        println!("\n  (上下文: {} 条 / ~{} tokens)", count, tokens);
    }
    result
}

async fn stream_run(agent: &mut ReactAgent, task: &str) -> Result<(), ReactError> {
    let mut event_stream = agent.execute_stream(task).await?;
    let mut in_token = false;
    let mut iter = 0usize;

    while let Some(event_result) = event_stream.next().await {
        match event_result? {
            AgentEvent::Token(token) => {
                if !in_token {
                    iter += 1;
                    in_token = true;
                }
                print!("{}", token);
                io::stdout().flush().ok();
            }
            AgentEvent::ToolCall { name, args } => {
                if in_token {
                    println!();
                    in_token = false;
                }
                println!("\n  [工具调用] {}({})", name, fmt_args(&args));
            }
            AgentEvent::ToolResult { name, output } => {
                println!("  [工具结果] {} → {}", name, truncate_chars(&output, 120));
                print!("\nagent > ");
                io::stdout().flush().ok();
            }
            AgentEvent::FinalAnswer(answer) => {
                if in_token {
                    println!();
                    in_token = false;
                } else {
                    println!("{}", answer);
                }
                if iter > 1 {
                    println!("\n  (共 {} 轮推理)", iter);
                }
            }
        }
    }
    Ok(())
}

// ── 辅助函数 ──────────────────────────────────────────────────────────────────

fn print_banner(cli: &Cli, tools: &[&str], mcp: &McpManager) {
    let tools_str = if tools.is_empty() {
        "无（纯对话模式）".to_string()
    } else {
        tools.join(", ")
    };
    let compress_str = if cli.compressor == "none" {
        "未启用".to_string()
    } else {
        match cli.token_limit {
            Some(limit) => format!("{}（超过 {} tokens 自动触发）", cli.compressor, limit),
            None => format!("{}（仅手动 /compress 触发）", cli.compressor),
        }
    };

    // MCP 摘要
    let mcp_names = mcp.server_names();
    let mcp_str: Option<String> = if mcp_names.is_empty() {
        None
    } else {
        let parts: Vec<String> = mcp_names
            .iter()
            .map(|name| {
                let count = mcp.get_client(name).map(|c| c.tools().len()).unwrap_or(0);
                format!("{name}({count})")
            })
            .collect();
        Some(parts.join(", "))
    };

    println!("╔══════════════════════════════════════════════════════════╗");
    println!("║              Echo Agent  ——  交互式 AI 终端              ║");
    println!("╚══════════════════════════════════════════════════════════╝");
    println!();
    println!("  模型    : {}", cli.model);
    println!("  工具    : {}", tools_str);
    if let Some(ref mcp_info) = mcp_str {
        println!("  MCP     : {}", mcp_info);
    }
    println!("  压缩    : {}", compress_str);
    if cli.skills_dir.is_some() {
        println!("  技能目录: {}", cli.skills_dir.as_deref().unwrap_or(""));
    }
    println!();
}

fn print_help() {
    println!("──────────────────────────────────────────────────────────");
    println!("  对话命令:");
    println!("    /help  /h              显示此帮助");
    println!("    /system                查看当前系统提示词");
    println!("    /model [名称]          查看或切换模型（如 /model qwen3-max）");
    println!("    /tools                 列出已注册的工具");
    println!("    /skills                列出已安装的技能");
    println!("    /mcp                   查看已连接的 MCP 服务端和工具");
    println!("    /ctx                   显示上下文消息数与 token 估算");
    println!("    /reset                 重置对话（清空历史，保留系统提示词）");
    println!("    /clear  /cls           清屏");
    println!("    /quit  /exit           退出程序");
    println!();
    println!("  压缩命令:");
    println!("    /compress              使用默认策略压缩上下文");
    println!("    /compress summary [N]  摘要压缩，保留最近 N 条（默认 6）");
    println!("    /compress sliding [N]  滑动窗口，保留最近 N 条（默认 20）");
    println!("    /compress hybrid  [N]  混合压缩（滑动+摘要），窗口 N（默认 10）");
    println!();
    println!("  快捷键:");
    println!("    ↑ / ↓                  浏览历史输入");
    println!("    Ctrl+C                 取消当前输入行");
    println!("    Ctrl+D                 退出程序");
    println!("──────────────────────────────────────────────────────────");
    println!();
}

fn truncate_chars(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let result: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{}…", result)
    } else {
        result
    }
}

fn fmt_args(args: &serde_json::Value) -> String {
    match args {
        serde_json::Value::Object(map) => map
            .iter()
            .map(|(k, v)| {
                let val = match v {
                    serde_json::Value::String(s) => truncate_chars(s, 40),
                    other => truncate_chars(&other.to_string(), 40),
                };
                format!("{k}={val}")
            })
            .collect::<Vec<_>>()
            .join(", "),
        other => truncate_chars(&other.to_string(), 80),
    }
}

fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var("HOME").ok().map(std::path::PathBuf::from)
}
