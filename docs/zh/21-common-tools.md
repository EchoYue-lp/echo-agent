# 常用工具速查

## 这篇文档讲什么

这是一页面向日常使用的工具速查，覆盖 `echo-agent` 中最常见的几类工具：

- `WebSearchTool`：实时搜索
- `WebFetchTool`：网页内容抓取
- 通过 Playwright MCP 暴露出来的浏览器自动化工具
- 内置数据处理工具：CSV / JSON / Parquet / Excel / Word / 文本文件

可以把它当作“遇到任务时先选哪类工具”的入口页。

---

## 快速选择

| 目标 | 推荐工具 | 最适合的场景 |
|------|----------|-------------|
| 查最新信息 | `WebSearchTool` | 新闻、官方文档、实时动态、外部参考 |
| 读取指定网页 | `WebFetchTool` | 博客正文、文档页面、文章内容 |
| 操作网站 | Playwright MCP 工具 | 点击、输入、导航、截图 |
| 分析本地数据文件 | Data tools | CSV / JSON / Parquet 统计与转换 |
| 读取办公文档 | `read_excel`、`excel_info`、`read_word`、`word_info` | 表格 / Word 预览与提取 |

---

## 1. Web 搜索

当 Agent 还不知道应该看哪个网址时，优先用 `WebSearchTool`。

典型场景：

- “搜索 Rust 2024 发布说明”
- “查找某个产品最新的定价页”
- “查询最近的 AI 基准测试更新”

快速开始：

```rust
use echo_agent::prelude::*;
use echo_agent::tools::web::WebSearchTool;

let mut agent = ReactAgentBuilder::new()
    .model("qwen3-max")
    .system_prompt("你是一个研究助手。")
    .enable_tools()
    .build()?;

agent.add_tool(Box::new(WebSearchTool::auto()));
```

`auto()` 的 provider 选择顺序：

- `TAVILY_API_KEY` -> Tavily
- `BRAVE_SEARCH_API_KEY` -> Brave
- 否则 DuckDuckGo

参考示例：`examples/demo41_web_tools.rs`

---

## 2. 网页抓取

当你已经有 URL，需要读取页面正文时，用 `WebFetchTool`。

典型场景：

- 抓取产品文档页面
- 抓取 changelog 条目
- 抓取官方博客文章做总结

快速开始：

```rust
use echo_agent::tools::web::WebFetchTool;

agent.add_tool(Box::new(
    WebFetchTool::new().with_max_content_length(10_000)
));
```

推荐组合方式：

1. 先用 `WebSearchTool` 搜索
2. 选出相关结果
3. 用 `WebFetchTool` 拉取正文
4. 再让 Agent 总结、比较或提取要点

补充文档：`docs/zh/20-web-tools.md`

---

## 3. 浏览器自动化

浏览器工具通常不是内置在框架里的，而是通过 MCP 接入，最常见的是 Playwright MCP。

常见浏览器操作：

- `playwright_navigate`
- `playwright_snapshot`
- `playwright_click`
- `playwright_type`
- `playwright_screenshot`
- `playwright_close`

适合浏览器工具的情况：

- 页面需要交互
- 内容是动态渲染的
- 需要截图或校验页面状态
- 单纯抓取 HTML 不够

适合 `WebFetchTool` 的情况：

- 页面是静态的
- 只需要文本
- 不需要点击或输入

参考示例：`examples/demo42_playwright_mcp.rs`

---

## 4. 数据工具

内置数据工具用于本地结构化文件的分析和转换。

常用工具：

- `read_data` — 读取 CSV / JSON / Parquet，并给出预览
- `data_stats` — 计算基础统计信息
- `filter_data` — 按条件过滤
- `aggregate_data` — 做聚合统计
- `transform_data` — 排序 / 选列等转换
- `export_data` — 导出处理结果

典型场景：

- 先预览 CSV 再决定分析方法
- 统计数值列的基本分布
- 筛选满足业务条件的记录
- 按分类聚合收入或数量

参考示例：`tests/example_contracts/demo43_data_tools.rs`

---

## 5. Excel / Word / 文本工具

处理办公文档时，优先用专门的文件工具：

- `excel_info` — 工作簿 / 工作表概览
- `read_excel` — 表格预览
- `read_word` — `.docx` 内容提取
- `word_info` — Word 文档元信息
- `text_stats` — 文本统计
- `search_text` — 文本关键词搜索

这些工具通常比让 LLM 直接面对原始文件更稳定、更可控。

---

## 推荐工作流

多数真实任务都适合按这个顺序做：

1. 用 Web 搜索找到候选信息源。
2. 用网页抓取或浏览器工具拿到细节。
3. 用数据工具分析结构化文件。
4. 最后由 Agent 汇总结果并输出 `final_answer`。

如果你希望结果更可复现，优先依赖工具原始输出，不要只看 LLM 的自由总结。

---

## 相关文档

- `docs/zh/02-tools.md`
- `docs/zh/08-mcp.md`
- `docs/zh/20-web-tools.md`
- `docs/zh/14-semantic-search.md`
