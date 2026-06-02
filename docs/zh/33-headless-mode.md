# 无头模式（Headless Mode）

> **状态：已实现。**
> `headless` 模块提供单次提示词、非交互式执行模式，专为 CI/CD 流水线、脚本和自动化工作流设计。

---

## 是什么

无头模式让 Agent 执行单条提示词，收集输出后退出 —— 没有交互式 REPL，没有 TUI，没有人工介入。当你希望 Agent 像传统 CLI 工具一样工作时，这是正确的入口：读入输入、产出输出、返回退出码。

典型使用场景：
- **CI/CD 流水线** — 自动代码审查、测试生成、文档更新
- **脚本编排** — 与 shell 管道、`cron` 定时任务、Makefile 目标串联
- **批量处理** — 对多个仓库或数据集执行相同提示词
- **无人值守自动化** — 任何没有人在场回答问题的场景

---

## HeadlessConfig

`HeadlessConfig` 控制无头执行的所有方面：

```rust
pub struct HeadlessConfig {
    /// 要执行的提示词
    pub prompt: String,

    /// Agent 报告失败时以错误码退出
    pub exit_on_error: bool,

    /// 输出格式："text"（默认）或 "json"
    pub output_format: String,

    /// 强制停止前的最大迭代次数（安全限制）
    pub max_iterations: Option<usize>,
}
```

| 字段 | 类型 | 默认值 | 用途 |
|------|------|--------|------|
| `prompt` | `String` | `""`（空） | 发送给 Agent 的提示词。空提示词会立即返回错误，不会构建 Agent。 |
| `exit_on_error` | `bool` | `true` | 为 `true` 时，Agent 失败则进程以退出码 1 终止。 |
| `output_format` | `String` | `"text"` | 控制 `format_output()`：`"text"` 返回原始输出；`"json"` 将结果包装为结构化 JSON。 |
| `max_iterations` | `Option<usize>` | `None` | ReAct 循环的迭代次数上限。在无人值守环境中防止失控执行。 |

---

## run_headless() API

```rust
pub async fn run_headless<F>(config: HeadlessConfig, configure: F) -> HeadlessResult
where
    F: FnOnce(ReactAgentBuilder) -> ReactAgentBuilder,
```

函数接受两个参数：

1. **`config`** — 上述 `HeadlessConfig`。
2. **`configure`** — 接收 `ReactAgentBuilder` 并返回它的闭包。在这里设置模型、系统提示词、工具以及其他 Agent 级别的配置。

Builder 模式的闭包让你完全控制 Agent 构造，而无需暴露内部生命周期管理：

```rust
let result = run_headless(config, |builder| {
    builder
        .model("qwen3-max")
        .system_prompt("你是一个代码审查助手")
        .tool(Box::new(FileReadTool))
        .tool(Box::new(ShellTool))
})
.await;
```

### 执行流程

```
run_headless(config, configure)
    │
    ├─ 空提示词？ → 立即返回错误 HeadlessResult
    │
    ├─ ReactAgentBuilder::new()
    │       │
    │       └─ configure(builder)   ← 调用方在此自定义
    │               │
    │               └─ 如果设置了 max_iterations，则应用
    │                       │
    │                       └─ builder.build()
    │                               │
    │                               ├─ 构建失败？ → 返回错误 HeadlessResult
    │                               │
    │                               └─ agent.execute(&prompt)
    │                                       │
    │                                       ├─ Ok → HeadlessResult { success: true, ... }
    │                                       └─ Err → HeadlessResult { success: false, ... }
    │
    └─ 返回 HeadlessResult
```

---

## HeadlessResult

```rust
pub struct HeadlessResult {
    /// Agent 的最终输出文本
    pub output: String,

    /// 执行是否成功
    pub success: bool,

    /// 使用的模型名称
    pub model: String,

    /// 请求的输出格式
    pub format: String,
}
```

### 关键方法

#### exit_code()

```rust
pub fn exit_code(&self) -> i32
```

成功返回 `0`，失败返回 `1` —— 可直接配合 `std::process::exit()` 使用。

#### format_output()

```rust
pub fn format_output(&self) -> String
```

根据请求的 `output_format` 格式化 stdout 输出：

- **`"text"`** — 原样返回 `output`。
- **`"json"`** — 返回格式化后的 JSON 信封：

```json
{
  "success": true,
  "model": "qwen3-max",
  "output": "..."
}
```

---

## 示例

### 示例 1：基本无头执行

执行单条提示词，打印输出，以对应退出码终止：

```rust
use echo_agent::headless::{HeadlessConfig, run_headless};

#[tokio::main]
async fn main() {
    let config = HeadlessConfig {
        prompt: "列出项目中所有 Rust 文件".into(),
        exit_on_error: true,
        output_format: "text".into(),
        max_iterations: Some(10),
    };

    let result = run_headless(config, |builder| builder).await;
    println!("{}", result.format_output());
    std::process::exit(result.exit_code());
}
```

### 示例 2：自定义 Agent

通过 builder 闭包提供模型、系统提示词和工具：

```rust
use echo_agent::headless::{HeadlessConfig, run_headless};

#[tokio::main]
async fn main() {
    let config = HeadlessConfig {
        prompt: "审查 src/main.rs 的安全问题".into(),
        exit_on_error: true,
        output_format: "text".into(),
        max_iterations: Some(20),
    };

    let result = run_headless(config, |builder| {
        builder
            .model("qwen3-max")
            .system_prompt("你是一个专注于安全的代码审查员。")
            .tool(Box::new(FileReadTool))
            .tool(Box::new(ShellTool))
    })
    .await;

    println!("{}", result.format_output());
    std::process::exit(result.exit_code());
}
```

### 示例 3：JSON 输出供程序消费

当下游进程需要解析结果时，使用 `output_format: "json"`：

```rust
use echo_agent::headless::{HeadlessConfig, run_headless};

#[tokio::main]
async fn main() {
    let config = HeadlessConfig {
        prompt: "总结最近一次 commit 的变更".into(),
        exit_on_error: true,
        output_format: "json".into(),   // ← JSON 信封
        max_iterations: Some(10),
    };

    let result = run_headless(config, |builder| builder).await;

    // 输出：
    // {
    //   "success": true,
    //   "model": "qwen3-max",
    //   "output": "..."
    // }
    println!("{}", result.format_output());
    std::process::exit(result.exit_code());
}
```

在 shell 脚本中消费输出：

```bash
OUTPUT=$(my-agent 2>/dev/null)
SUCCESS=$(echo "$OUTPUT" | jq -r '.success')
if [ "$SUCCESS" = "true" ]; then
    echo "$OUTPUT" | jq -r '.output'
else
    echo "Agent 执行失败" >&2
    exit 1
fi
```

### 示例 4：CI/CD 集成（GitHub Actions）

在每个 Pull Request 上运行 Agent 的典型 GitHub Actions 工作流：

```yaml
name: AI 代码审查
on:
  pull_request:
    branches: [main]

jobs:
  review:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: 安装 Rust
        uses: dtolnay/rust-toolchain@stable

      - name: 构建 Agent
        run: cargo build --release

      - name: 无头模式执行审查
        env:
          LLM_API_KEY: ${{ secrets.LLM_API_KEY }}
        run: |
          ./target/release/my-agent \
            --prompt "审查本 PR 的 diff，检查 bug 和代码风格问题" \
            --output-format json \
            --max-iterations 15 \
          | tee review.json

      - name: 发布审查评论
        if: always()
        run: |
          COMMENT=$(jq -r '.output' review.json)
          gh pr comment ${{ github.event.pull_request.number }} --body "$COMMENT"
```

CI/CD 环境中的关键要点：
- **`max_iterations`** — 始终设置上限，防止构建失控
- **`exit_code()`** — `std::process::exit(result.exit_code())` 在 Agent 出错时让 CI 步骤失败
- **`output_format: "json"`** — 方便下游步骤解析并处理结果

---

## 错误处理

`run_headless` 不会 panic。所有失败都被捕获在 `HeadlessResult` 中：

| 场景 | `success` | `output` |
|------|-----------|----------|
| 空提示词 | `false` | `"Error: empty prompt"` |
| Agent 构建失败 | `false` | `"Error building agent: <详情>"` |
| Agent 执行出错 | `false` | `"Error: <详情>"` |
| 执行成功 | `true` | Agent 的最终回答 |

这意味着调用方始终可以信赖 `exit_code()` 和 `format_output()`，无需额外 `match` 一个 `Result`。

---

## 与其他模式的对比

| | 无头模式 | Chat / REPL | 流式输出 |
|---|---------|-------------|----------|
| 交互 | 无 | 交互式 | 交互式 |
| 提示词 | 单次 | 多轮 | 多轮 |
| 输出 | 收集后返回 | 实时打印 | 逐 Token |
| 退出行为 | 进程退出 | 保持运行 | 保持运行 |
| 最佳场景 | CI/CD、脚本 | 开发调试 | 用户体验敏感应用 |

---

## 相关文档

- [React Agent](01-react-agent.md) — 底层 Agent 循环
- [流式输出](10-streaming.md) — 交互场景下的实时 Token 推送
- [长程任务](29-long-running-tasks.md) — 会话内的后台任务执行
