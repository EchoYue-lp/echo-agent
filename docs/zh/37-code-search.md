# Code Search

## 概述

`CodeSearchTool` 是 echo-agent 中面向"跨项目搜索代码"的工具，以 ripgrep (`rg`) 为主后端，通过解析 `rg --json` 输出提供结构化结果。当 `rg` 不在 PATH 上时，自动回退到内置的符号搜索引擎（基于语言感知的正则匹配）。

工具名称：`code_search`
风险等级：`ReadOnly`
所需权限：`Read`

---

## 解决的问题

Agent 在大型代码库中工作时，需要快速定位函数定义、类型声明、用法引用等信息。单纯依赖 LLM 逐文件阅读效率极低。`CodeSearchTool` 提供：

- **毫秒级搜索** — ripgrep 是 Rust 实现的工业级搜索工具，处理数万文件仅需毫秒
- **结构化输出** — 解析 JSON 输出，提取文件路径、行号、匹配文本和上下文
- **零外部依赖回退** — 即使没有安装 ripgrep，内置引擎也能搜索函数、类、结构体等符号定义

---

## 架构

```
CodeSearchTool                        ← 实现 Tool trait
    │
    ├─ try_ripgrep_search()           ← 主后端：调用 `rg --json` 并解析输出
    │   ├─ --glob / --type            ← 文件过滤
    │   ├─ -i / -F / -w              ← 匹配模式
    │   ├─ -C / -A / -B              ← 上下文行数
    │   ├─ -m                         ← 每文件最大匹配数
    │   └─ parse_rg_json()            ← JSON 解析器（match/context 事件）
    │
    └─ search_symbols()               ← 回退后端：内置符号搜索
        ├─ Rust (fn, struct, enum, trait, type)
        ├─ Python (def, class)
        ├─ JS/TS (function, class, interface, type, const)
        ├─ Go (func, struct, interface)
        ├─ Java (method, class, interface)
        └─ C/C++ (function, struct/class)

输出保护：
    └─ MAX_OUTPUT_BYTES = 50_000      ← 50KB 截断，防止 token 溢出
```

---

## Feature Gate

`CodeSearchTool` 归属于 `files` feature：

```toml
[dependencies]
echo_tools = { version = "0.2", features = ["files"] }
```

注册方式（由 `echo_tools::registry` 自动完成）：

```rust
#[cfg(feature = "files")]
tool_manager.register(Box::new(CodeSearchTool::new()));
```

---

## 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `query` | string | 是 | 搜索模式（默认正则，`fixed_strings=true` 时为字面量） |
| `path` | string | 否 | 搜索目录（默认 `.`） |
| `glob` | string | 否 | 文件名 glob 过滤（如 `*.rs`、`*.py`） |
| `file_type` | string | 否 | ripgrep 识别的文件类型（如 `rust`、`python`、`js`） |
| `case_insensitive` | boolean | 否 | 忽略大小写（`-i`），默认 `false` |
| `fixed_strings` | boolean | 否 | 字面匹配，不做正则解析（`-F`），默认 `false` |
| `word_regexp` | boolean | 否 | 仅匹配完整单词（`-w`），默认 `false` |
| `context` | integer | 否 | 匹配行前后的上下文行数（`-C N`） |
| `max_count` | integer | 否 | 每个文件最大匹配数（`-m N`） |
| `max_results` | integer | 否 | 返回结果总数上限（默认 50） |
| `symbol` | string | 否 | （旧版兼容）符号名称，作为 `query` 的备选 |
| `symbol_type` | string | 否 | （旧版兼容）符号类型过滤，取值 `function`/`class`/`struct`/`enum`/`trait`/`interface`/`type`/`method`/`any`（默认 `any`） |

---

## 后端行为

### 主后端：ripgrep

当 `rg` 存在于 PATH 时，工具自动调用 `rg --json <flags> <query> <path>`，逐行解析 JSON 事件：

- `match` 事件 — 提取 `path`、`line_number`、`lines.text`
- `context` 事件 — 提取上下文行，附加到前一个匹配结果
- `begin`/`end`/`summary` 事件 — 忽略

`rg` 退出码处理：
- 退出码 `0` — 有匹配结果
- 退出码 `1` — 无匹配（不是错误，正常返回"No matches found"）
- 退出码 `2` — 真正的错误（如无效正则），返回 `ToolError`

### 回退后端：内置符号搜索

当 `rg` 不在 PATH（`io::ErrorKind::NotFound`）时，自动切换到内置引擎：

- 递归遍历目录，跳过隐藏目录和常见构建产物（`.git`、`target`、`node_modules`、`__pycache__`）
- 根据文件扩展名识别语言，使用语言特定的正则匹配符号定义
- 支持通配符模式（`*` 转换为正则 `.*`）
- 结果包含文件名、行号、符号类型和上下文行

### 50KB 输出保护

无论使用哪个后端，最终输出超过 `MAX_OUTPUT_BYTES`（50,000 字节）时自动截断，并附加提示：

```
... [output truncated to prevent overflow]
```

这避免了大型搜索结果耗尽 LLM 的上下文窗口。

---

## 快速开始

### 基本用法

```rust
use echo_agent::prelude::*;
use echo_tools::files::code_search::CodeSearchTool;

let mut agent = ReactAgentBuilder::new()
    .model("qwen3-max")
    .system_prompt("You are a code assistant.")
    .enable_tools()
    .build()?;

agent.add_tool(Box::new(CodeSearchTool::new()));

let answer = agent
    .execute("Find all implementations of the Tool trait in the project")
    .await?;
```

### 限定搜索目录

```rust
use echo_tools::files::code_search::CodeSearchTool;

// 将搜索限制在指定目录下
agent.add_tool(Box::new(CodeSearchTool::with_base_dir("/workspace/my-project")));
```

`with_base_dir` 设置后，所有相对路径都在该目录下解析，并启用路径沙箱保护（防止 `../` 逃逸和符号链接绕过）。

---

## Agent 调用示例

Agent 在推理过程中可能这样调用 `code_search`：

```json
{
  "query": "fn execute",
  "path": "src",
  "glob": "*.rs",
  "context": 2,
  "max_results": 20
}
```

返回结果示例：

```
Found 5 match(es) for 'fn execute' in src:

src/tools/code_search.rs:134:     fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
  132-     }
  133-
  134:     fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
  135-         Box::pin(async move {
  136-             let query = parameters
src/tools/grep.rs:91:     fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
  89-     }
  90-
  91:     fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
  92-         Box::pin(async move {
  93-             let pattern_str = parameters
```

---

## 与 GrepTool 的对比

项目中同时提供了 `GrepTool`（工具名 `grep`），两者定位不同：

| 维度 | `code_search`（CodeSearchTool） | `grep`（GrepTool） |
|------|--------------------------------|-------------------|
| **后端** | ripgrep（`rg --json`），无 rg 时回退内置引擎 | 纯 Rust `regex` crate，逐文件读取匹配 |
| **性能** | 极快（ripgrep 并行 + 智能忽略） | 较慢（串行遍历 + 逐文件读取） |
| **搜索范围** | 全项目代码搜索，支持 `--type`、`--glob` | 单目录/文件内容搜索，仅支持 `glob` |
| **上下文** | `-C` / `-A` / `-B`（ripgrep 原生支持） | `-C`（自行实现前后行读取） |
| **输出保护** | 50KB 截断 | 按 `max_results` 行数截断（默认 100） |
| **符号搜索** | 回退模式下支持语言感知的符号定义搜索 | 不支持 |
| **适用场景** | 大型项目全局搜索、代码导航 | 小范围精确匹配、已知目录内的正则搜索 |

**选择建议**：
- 跨项目搜索代码模式 → `code_search`
- 在已知文件中查找特定文本 → `grep`
- 搜索函数/类/结构体定义 → `code_search`（利用回退模式的符号搜索能力）

---

## 回退模式支持的语言

当 `rg` 不可用时，内置符号搜索支持以下语言的符号定义识别：

| 语言 | 扩展名 | 可识别符号类型 |
|------|--------|---------------|
| Rust | `.rs` | `function`、`struct`、`enum`、`trait`、`type` |
| Python | `.py` | `function`、`class` |
| JavaScript/TypeScript | `.js`、`.jsx`、`.ts`、`.tsx` | `function`、`class`、`interface`、`type` |
| Go | `.go` | `function`、`struct`、`interface` |
| Java | `.java` | `method`、`class`、`interface` |
| C/C++ | `.c`、`.h`、`.cpp`、`.hpp` | `function`、`class`（struct/class） |

---

## 安全考虑

- **只读操作** — 风险等级为 `ReadOnly`，不修改任何文件
- **路径沙箱** — 当设置 `base_dir` 时，通过 `resolve_path` 阻止目录逃逸（包括符号链接检测）
- **输出截断** — 50KB 上限防止搜索结果耗尽 LLM 上下文窗口
- **权限声明** — 仅请求 `Read` 权限

---

## 完整示例

```rust
use echo_agent::prelude::*;
use echo_tools::files::code_search::CodeSearchTool;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .system_prompt("You are a senior Rust developer assisting with code exploration.")
        .enable_tools()
        .build()?;

    // 注册 CodeSearchTool，限定搜索范围为项目根目录
    agent.add_tool(Box::new(CodeSearchTool::with_base_dir("./my-project")));

    // Agent 会自动调用 code_search 来定位相关代码
    let answer = agent
        .execute("Find all async functions that return Result in the src/ directory, \
                   and explain the error handling patterns used.")
        .await?;

    println!("{}", answer);
    Ok(())
}
```
