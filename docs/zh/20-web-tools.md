# Web 工具（Web Tools）

## 是什么

Web 工具为 Agent 提供互联网访问能力，包括 Web 搜索和网页内容获取。Agent 可以主动搜索信息、抓取网页内容并转换为可读文本，不再局限于离线知识。

---

## 解决什么问题

LLM 的知识有截止日期，且无法访问实时信息。Web 工具让 Agent 能够：

- 搜索最新新闻、技术动态、实时数据
- 获取网页详细内容用于深度分析
- 验证事实、查找引用来源
- 基于实时信息做出决策

---

## 架构

```
WebSearchTool                      ← 搜索工具（Tool trait 实现）
    │
SearchProvider trait               ← 搜索引擎抽象接口
    ├─ DuckDuckGoProvider          ← 免费，无需 API Key（默认）
    ├─ BraveSearchProvider         ← 付费 API，高质量
    └─ TavilyProvider              ← AI 优化搜索，专为 Agent 设计

WebFetchTool                       ← 网页获取工具（Tool trait 实现）
    └─ HTML → 可读文本（html2text）
```

---

## 快速开始

### 基本用法

```rust
use echo_agent::prelude::*;
use echo_agent::tools::web::{WebSearchTool, WebFetchTool};

let mut agent = ReactAgentBuilder::new()
    .model("qwen3-max")
    .system_prompt("你是一个信息搜索助手")
    .enable_tools()
    .build()?;

// 注册 Web 工具
agent.add_tool(Box::new(WebSearchTool::with_duckduckgo()));
agent.add_tool(Box::new(WebFetchTool::new()));

let answer = agent.execute("搜索 Rust 2024 edition 新特性").await?;
```

### 自动选择 Provider

```rust
// 自动按优先级选择：Tavily > Brave > DuckDuckGo
// 从环境变量读取 API Key：TAVILY_API_KEY / BRAVE_SEARCH_API_KEY
agent.add_tool(Box::new(WebSearchTool::auto()));
```

---

## WebSearchTool — Web 搜索

### 参数

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `query` | string | 是 | 搜索关键词 |
| `max_results` | integer | 否 | 最大返回结果数（默认 5，最大 10） |

### 返回格式

```
搜索结果（共 3 条）:

1. Rust Programming Language
   https://www.rust-lang.org/
   A language empowering everyone to build reliable and efficient software.

2. Rust Documentation
   https://doc.rust-lang.org/
   The Rust programming language documentation.
```

### 搜索引擎 Provider

#### DuckDuckGo（免费兜底）

无需 API Key，通过抓取 `html.duckduckgo.com` 实现搜索。

```rust
let tool = WebSearchTool::with_duckduckgo();
```

> **注意**：DuckDuckGo 依赖 HTML 页面结构，如果页面结构变更可能导致解析失败。

#### Brave Search（付费高质量）

需要 API Key，每月 2000 次免费额度。

```rust
// 方式 1：直接传入 API Key
let tool = WebSearchTool::with_brave("your-api-key");

// 方式 2：从环境变量 BRAVE_SEARCH_API_KEY 读取
let tool = WebSearchTool::auto();
```

注册获取 API Key：https://brave.com/search/api/

#### Tavily（AI 优化搜索）

专为 AI Agent 设计的搜索引擎，摘要质量最高。

```rust
// 方式 1：直接传入 API Key
let tool = WebSearchTool::with_tavily("your-api-key");

// 方式 2：从环境变量 TAVILY_API_KEY 读取
let tool = WebSearchTool::auto();
```

注册获取 API Key：https://tavily.com/

#### Provider 对比

| Provider | 费用 | 质量 | 延迟 | 说明 |
|----------|------|------|------|------|
| DuckDuckGo | 免费 | 中 | 较高 | HTML 抓取，无需 Key |
| Brave | 免费 2000 次/月 | 高 | 低 | 官方 API，稳定可靠 |
| Tavily | 付费（有免费额度） | 最高 | 低 | AI 优化，专为 Agent 设计 |

---

## WebFetchTool — 网页获取

获取指定 URL 的内容，自动将 HTML 转换为可读文本。

### 参数

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `url` | string | 是 | 网页 URL（必须以 http:// 或 https:// 开头） |
| `max_length` | integer | 否 | 最大内容长度（字符数，默认 50000） |

### 示例

```rust
use echo_agent::tools::web::WebFetchTool;

let tool = WebFetchTool::new()
    .with_max_content_length(10000);  // 限制最大 10000 字符

agent.add_tool(Box::new(tool));
```

### 特性

- 自动 HTML → 纯文本转换
- 跟随重定向（最多 5 次）
- 自动检测内容类型（HTML / 纯文本 / JSON）
- 超长内容自动截断

---

## 自定义 SearchProvider

实现 `SearchProvider` trait 即可扩展新的搜索引擎：

```rust
use echo_agent::tools::web::providers::{SearchProvider, SearchResult};
use async_trait::async_trait;

struct GoogleProvider {
    api_key: String,
}

#[async_trait]
impl SearchProvider for GoogleProvider {
    fn name(&self) -> &str {
        "google"
    }

    async fn search(&self, query: &str, max_results: usize)
        -> echo_agent::error::Result<Vec<SearchResult>>
    {
        // 调用 Google Custom Search API ...
        Ok(vec![
            SearchResult {
                title: "Example".into(),
                url: "https://example.com".into(),
                snippet: "An example result".into(),
            },
        ])
    }
}

// 使用自定义 Provider
let tool = WebSearchTool::new(Box::new(GoogleProvider { api_key: "xxx".into() }));
```

---

## Feature Flag

Web 工具需要启用 `web` feature（已包含在 `full` feature 中）：

```toml
# Cargo.toml
[dependencies]
echo_agent = { version = "1.3", features = ["web"] }
```

---

## 环境变量

| 环境变量 | 说明 |
|----------|------|
| `BRAVE_SEARCH_API_KEY` | Brave Search API Key |
| `TAVILY_API_KEY` | Tavily API Key |

---

对应示例：`echo-agent-learning/examples/demo41_web_tools.rs`
