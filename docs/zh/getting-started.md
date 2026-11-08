# 快速上手

本教程将带你用 echo-agent 从零构建一个带工具的 AI Agent，全程不超过 10 分钟。

## 前置条件

- **Rust 1.95+**（2024 edition）
- **cargo**（随 Rust 一起安装）
- 一个 LLM API Key（支持 OpenAI、DeepSeek、Qwen、Anthropic、Ollama 等）

```bash
# 确认 Rust 版本
rustc --version   # 应输出 rustc 1.95.x 或更高

# 若需安装或升级
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

设置你的 API Key（任选其一）：

```bash
export OPENAI_API_KEY=sk-xxx         # OpenAI
export DEEPSEEK_API_KEY=sk-xxx       # DeepSeek
export DASHSCOPE_API_KEY=sk-xxx      # 阿里通义千问
export ANTHROPIC_API_KEY=sk-ant-xxx  # Anthropic
```

---

## 第一步：创建项目

```bash
cargo new my-agent
cd my-agent
```

编辑 `Cargo.toml`，添加依赖：

```toml
[dependencies]
echo_agent = "0.2.0"
tokio = { version = "1", features = ["full"] }
```

---

## 第二步：最小 Agent（无工具）

编辑 `src/main.rs`：

```rust,no_run
use echo_agent::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let agent = ReactAgentBuilder::simple("deepseek-v4-flash", "你是一个有帮助的助手")?;

    let answer = agent.execute("Rust 的所有权机制是什么？").await?;
    println!("{answer}");
    Ok(())
}
```

运行：

```bash
cargo run
```

这就是一个最基础的对话 Agent——没有工具，纯靠 LLM 回答。

---

## 第三步：添加自定义工具

Agent 的真正威力来自工具调用。用 `#[tool]` 宏定义工具，一行注解自动生成 JSON Schema：

```rust,no_run
use echo_agent::prelude::*;
use echo_agent::{agent, tool};

#[tool(name = "add", description = "Add two numbers")]
async fn add(a: f64, b: f64) -> Result<ToolResult> {
    Ok(ToolResult::success(format!("{}", a + b)))
}

#[tool(name = "multiply", description = "Multiply two numbers")]
async fn multiply(a: f64, b: f64) -> Result<ToolResult> {
    Ok(ToolResult::success(format!("{}", a * b)))
}

#[tokio::main]
async fn main() -> Result<()> {
    let api_key = std::env::var("OPENAI_API_KEY").map_err(|_| {
        echo_agent::error::ConfigError::MissingConfig(
            "getting-started".to_string(),
            "OPENAI_API_KEY".to_string(),
        )
    })?;
    let llm_config = LlmConfig::for_provider(
        "openai",
        "https://api.openai.com/v1",
        api_key,
        "gpt-5.5",
        LlmApiProtocol::Responses,
    )?;
    let agent = agent! {
        llm_config: llm_config,
        system_prompt: "你是一个数学助手，使用工具进行计算。",
        tools: [AddTool, MultiplyTool],
    }?;

    let answer = agent.execute("计算 (3 + 4) * 5").await?;
    println!("答案: {answer}");
    Ok(())
}
```

运行后，Agent 会自动推理：先调用 `add(3, 4)` 得到 7，再调用 `multiply(7, 5)` 得到 35。

```bash
OPENAI_API_KEY=sk-... cargo run
```

---

## 第四步：理解执行流程

执行循环不要求模型调用 think/final 控制工具，也不强制公开私有推理。真实流程是：

```text
用户 Message
  -> provider 原生正文 / reasoning / 工具提议
  -> 有工具则执行并回填真实 ToolResult，再继续同一执行
  -> 最终候选经过配置的 Guard/Critic、Stop 与持久化
  -> 当前 runtime FinalAnswer 通知与真实 ExecutionStatus
  -> 原 source 物理 EOF 后才向调用方返回成功
```

工具提议同响应中的 Final 文字不能跳过工具。reasoning-only、截断或异常断流不构成成功。

---

## 第五步：添加记忆

让 Agent 跨会话记住信息，只需一行代码：

```rust,no_run
use echo_agent::prelude::*;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<()> {
    let store = Arc::new(InMemoryStore::new());

    let agent = ReactAgentBuilder::new()
        .model("deepseek-v4-flash")
        .system_prompt("你是一个助手，可以记住用户告诉你的信息。")
        .with_memory_tools(store)  // 自动注册已批准记忆的 recall / search_memory 工具
        .build()?;

    // 安装 MemoryLayerManager 后才启用经过 journal 的 remember / forget 写入。
    let answer = agent.execute("有哪些已批准的项目事实？").await?;
    println!("{answer}");
    Ok(())
}
```

需要持久化到磁盘？把 `InMemoryStore` 换成 `FileStore` 或 `SqliteStore`（需要 `sqlite` feature）。

---

## 第六步：原生流与返回形式

String/event 返回形式与模型 HTTP streaming 是两个独立选择。下面显式设置
`model_streaming(false)`，展示完整模型响应仍进入同一原生事件、Guard 和 EOF 路径；
省略这一行时默认 true，执行与结束规则不变。

```rust,no_run
use echo_agent::agent::{Agent, AgentEvent, AgentOutputProjection, AssistantPhase, ExecutionStatusKind};
use echo_agent::error::{AgentError, AgentTerminalKind, ReactError};
use echo_agent::prelude::*;
use futures::StreamExt;
use std::io::Write;

#[tokio::main]
async fn main() -> Result<()> {
    let agent = ReactAgentBuilder::new()
        .model("deepseek-v4-flash")
        .system_prompt("你是一个有帮助的助手，请用中文回答。")
        .model_streaming(false) // 显式使用完整模型响应，仍返回同一原生事件流。
        .build()?;
    let mut stream = agent.execute_stream("解释 Rust 的异步编程").await?;
    let owner_run = agent.current_run_id();
    let mut output = AgentOutputProjection::default();
    let mut first_error = None;
    let mut unresolved = None::<ReactError>;
    let same_source = |error: &ReactError, projection: &AgentOutputProjection| {
        matches!(error, ReactError::ExecutionNotSettled { run_id, execution_id }
            if owner_run.as_deref() == Some(run_id.as_str())
                && execution_id.as_deref().is_some()
                && execution_id.as_deref() == projection.execution_id())
    };

    // 持续读取同一原 source 到真实 EOF，再判断是否成功。
    while let Some(event) = stream.next().await {
        let observed = match event {
            Err(error @ ReactError::ExecutionNotSettled { .. }) => {
                // 未结算观察不是终态；foreign 观察不会被这条 source 的完成覆盖。
                if unresolved
                    .as_ref()
                    .is_none_or(|prior| same_source(prior, &output))
                {
                    unresolved = Some(error);
                }
                Ok(())
            }
            Ok(AgentEvent::ExecutionStatus(status)) => {
                output.observe_status(&status).map(|changed| {
                    if changed
                        && matches!(
                            status.status,
                            ExecutionStatusKind::Settled
                                | ExecutionStatusKind::Failed
                                | ExecutionStatusKind::Cancelled
                        )
                        && unresolved
                            .as_ref()
                            .is_some_and(|prior| same_source(prior, &output))
                    {
                        unresolved = None;
                    }
                    if let Some(error) = output.terminal_failure() {
                        first_error.get_or_insert(error);
                    }
                })
            }
            Ok(AgentEvent::AssistantMessage { block }) => {
                output.observe_block(&block).map(|delta| {
                    if let Some(delta) = delta {
                        // 只显示投影后的可见文本；签名、密文和重放载体不直接输出。
                        match delta.block.phase {
                            AssistantPhase::Reasoning if !delta.text.is_empty() => {
                                println!("\n[推理] {}", delta.text)
                            }
                            AssistantPhase::Commentary if !delta.text.is_empty() => {
                                println!("\n[进度] {}", delta.text)
                            }
                            AssistantPhase::Reasoning | AssistantPhase::Commentary => {}
                            _ => {
                                print!("{}", delta.text);
                                std::io::stdout().flush().ok();
                            }
                        }
                    }
                })
            }
            Ok(AgentEvent::LlmUsage {
                model,
                prompt_tokens,
                completion_tokens,
                total_tokens,
                usage_reported,
                ..
            }) => {
                if usage_reported {
                    println!(
                        "\n[LLM 用量] model={model}, prompt={prompt_tokens}, completion={completion_tokens}, total={total_tokens}"
                    );
                } else {
                    println!("\n[LLM 用量] model={model}，上游未报告用量");
                }
                Ok(())
            }
            Ok(AgentEvent::FinalAnswer(answer)) => output.confirm_answer(&answer),
            Ok(AgentEvent::Error { failure, .. }) => Err(match failure.terminal_kind {
                AgentTerminalKind::Cancelled => AgentError::Cancelled(failure.message).into(),
                AgentTerminalKind::TimedOut => AgentError::Timeout(failure.message).into(),
                AgentTerminalKind::PermissionDenied => {
                    AgentError::PermissionDenied(failure.message).into()
                }
                _ => ReactError::Other(failure.message),
            }),
            Ok(AgentEvent::Cancelled) => {
                Err(AgentError::Cancelled("原生 Agent source 已取消".into()).into())
            }
            Err(error) => Err(error),
            _ => Ok(()),
        };
        if let Err(error) = observed {
            first_error.get_or_insert(error);
        }
    }
    if let Some(error) = unresolved {
        return Err(error);
    }
    if let Some(error) = first_error {
        return Err(error);
    }
    let final_answer = output.finish()?;
    // 原 source EOF 后只补尚未展示的安全最终正文，确认通知本身不重印。
    if let Some(missing) = output.take_final_presentation()? {
        print!("{missing}");
        std::io::stdout().flush().ok();
    }
    if final_answer.trim().is_empty() {
        return Err(ReactError::Other(
            "流式示例返回空答案".into(),
        ));
    }
    Ok(())
}
```

增加依赖 `futures = "0.3"`。示例只使用 public facade，没有从旧正文补 Started/
settlement，也没有在最终通知上提前 break。Reasoning 的可见文本和普通正文分开，
签名/密文不直接打印。完整多轮示例见
[demo17_chat.rs](../../echo-agent-learning/examples/demo17_chat.rs)，事件与投递边界见
[10-streaming.md](./10-streaming.md)。本轮只做文本与源码核对，没有执行片段编译或模型验收。

---

## 下一步

恭喜你完成了第一个带工具的 AI Agent！以下是深入学习的方向：

| 主题 | 文档 |
|------|------|
| ReAct 引擎详解 | [01-react-agent.md](./01-react-agent.md) |
| 工具系统 | [02-tools.md](./02-tools.md) |
| 记忆系统 | [03-memory.md](./03-memory.md) |
| 流式输出 | [10-streaming.md](./10-streaming.md) |
| 上下文压缩 | [04-compression.md](./04-compression.md) |
| MCP 协议 | [08-mcp.md](./08-mcp.md) |
| 多 Agent 编排 | [26-multi-agent.md](./26-multi-agent.md) |

也可以直接运行仓库中的示例：

```bash
cargo run -p echo-agent-learning --example demo01_tools          # 自定义工具
cargo run -p echo-agent-learning --example demo10_streaming      # 流式输出
cargo run -p echo-agent-learning --example demo04_subagent --features subagent # 多 Agent 编排
cargo run -p echo-agent-learning --example demo06_mcp --features mcp # MCP 协议集成
```

完整示例列表见 [echo-agent-learning/examples/](../../echo-agent-learning/examples/) 目录。


本页当前源码示例已接入workspace-only `echo-agent-learning` 的rustdoc编译入口，
本地 `scripts/verify.sh` 显式执行 `cargo test -p echo-agent-learning --doc --all-features --locked`，
CI learning 组按既有默认 feature 执行 `cargo test -p echo-agent-learning --doc --locked`；片段的实际通过资格
按准确源码与运行回执记录。网络／模型示例均为 `no_run`；完整流式源码
还纳入现有 `demo17_chat` example编译链，文档、编译与运行资格分别记录。
