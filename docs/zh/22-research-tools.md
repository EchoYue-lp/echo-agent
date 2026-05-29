# 论文检索工具

## 概述

论文检索工具支持学术论文的发现、获取和引用管理，覆盖完整的文献调研流程：搜索 → 下载 → 阅读 → 引用。

可用工具：
- `arxiv_search` — 搜索 ArXiv 预印本
- `semantic_scholar_search` — 搜索 Semantic Scholar 已发表论文
- `pdf_fetch` — 下载并解析 PDF 文档
- `bibtex_generate` — 从论文元数据生成 BibTeX 引用

> **Feature flag：** `research` — 在 Cargo.toml 中通过 `features = ["research"]` 启用。

---

## arxiv_search

搜索 ArXiv API 获取学术预印本。返回结构化元数据，包括标题、作者、摘要、分类和 PDF 链接。

### 参数

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `query` | string | 是 | — | 搜索关键词、标题或作者名 |
| `max_results` | integer | 否 | 20 | 最大结果数（上限 100） |
| `category` | string | 否 | — | ArXiv 分类过滤（如 `cs.AI`、`cs.LG`、`cs.CL`、`math.CO`） |
| `sort_by` | string | 否 | `relevance` | 排序方式：`relevance`、`lastUpdatedDate`、`submittedDate` |
| `sort_order` | string | 否 | `descending` | 排序方向：`descending` 或 `ascending` |

### 示例

```json
{
  "query": "transformer attention mechanism",
  "max_results": 10,
  "category": "cs.CL",
  "sort_by": "submittedDate"
}
```

### 响应

```json
{
  "query": "transformer attention mechanism",
  "total_results": 10,
  "papers": [
    {
      "arxiv_id": "1706.03762",
      "title": "Attention Is All You Need",
      "authors": ["Ashish Vaswani", "..."],
      "abstract": "The dominant sequence transduction models...",
      "published": "2017-06-12T17:47:36Z",
      "pdf_url": "http://arxiv.org/pdf/1706.03762v2",
      "categories": ["cs.CL", "cs.AI", "cs.LG"]
    }
  ]
}
```

---

## semantic_scholar_search

搜索 Semantic Scholar 获取已发表的学术论文。返回丰富的元数据，包括引用数、参考文献数、研究领域和发表期刊。无需 API key（速率限制：每 5 分钟 100 次请求）。

### 参数

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `query` | string | 是 | — | 搜索关键词、标题或作者 |
| `limit` | integer | 否 | 20 | 最大结果数（上限 100） |
| `fields` | string | 否 | （全部） | 逗号分隔的返回字段 |
| `year_range` | string | 否 | — | 年份过滤：`2020-2025`、`2020-`、`-2023` |
| `fields_of_study` | string | 否 | — | 按研究领域过滤（如 `Computer Science`） |
| `sort` | string | 否 | `relevance` | 排序：`relevance`、`citationCount:desc`、`year:desc` |

### 示例

```json
{
  "query": "large language models",
  "limit": 5,
  "year_range": "2023-",
  "fields_of_study": "Computer Science",
  "sort": "citationCount:desc"
}
```

### 响应

```json
{
  "query": "large language models",
  "total_available": 15432,
  "returned": 5,
  "papers": [
    {
      "paper_id": "df2b0e26d0599ce3e70df8a0da7b5...",
      "title": "GPT-4 Technical Report",
      "authors": ["OpenAI"],
      "abstract": "We report the development of GPT-4...",
      "year": 2023,
      "citation_count": 5432,
      "reference_count": 89,
      "venue": "arXiv",
      "url": "https://www.semanticscholar.org/paper/...",
      "arxiv_id": "2303.08774",
      "doi": "",
      "fields_of_study": ["Computer Science"],
      "publication_date": "2023-03-15"
    }
  ]
}
```

---

## pdf_fetch

从 URL 下载 PDF 并提取文本内容。支持 ArXiv PDF 链接、直接 URL 和 HTTP 重定向。适用于在 Agent 工作流中阅读学术论文。

### 参数

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `url` | string | 是 | — | PDF 下载地址 |
| `pages` | string | 否 | `1-20` | 页码范围：`1-5`、`1,3,7` 或 `all` |
| `max_chars` | integer | 否 | 50000 | 最大提取字符数 |

### 示例

```json
{
  "url": "https://arxiv.org/pdf/1706.03762",
  "pages": "1-10",
  "max_chars": 30000
}
```

### 响应

```json
{
  "url": "https://arxiv.org/pdf/1706.03762",
  "pages_requested": "1-10",
  "text_length": 28456,
  "text": "\n--- Page 1 ---\nAttention Is All You Need\n...",
  "metadata": {
    "pages": 15,
    "title": "Attention Is All You Need",
    "author": "Ashish Vaswani et al.",
    "creation_date": "D:20170612174736"
  }
}
```

### 安全特性

- SSRF 防护：阻止私有 IP（127.0.0.1、10.0.0.0/8、172.16.0.0/12、192.168.0.0/16）、localhost、`.local` 域名
- 解析前验证 `%PDF` 魔数字节
- 60 秒超时，安全重定向策略

---

## bibtex_generate

从论文元数据生成 BibTeX 条目。支持来自 ArXiv、Semantic Scholar 或手动提供的论文元数据，输出标准 `.bib` 格式文本。

### 参数

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `papers` | array | 是 | — | 论文元数据数组 |
| `papers[].title` | string | 是 | — | 论文标题 |
| `papers[].authors` | array | 是 | — | 作者列表 |
| `papers[].year` | integer | 否 | — | 发表年份 |
| `papers[].url` | string | 否 | — | 论文链接 |
| `papers[].arxiv_id` | string | 否 | — | ArXiv ID（如 `1706.03762`） |
| `papers[].doi` | string | 否 | — | DOI 标识符 |
| `papers[].venue` | string | 否 | — | 会议或期刊名称 |

### 示例

```json
{
  "papers": [
    {
      "title": "Attention Is All You Need",
      "authors": ["Ashish Vaswani", "Noam Shazeer", "Niki Parmar"],
      "year": 2017,
      "arxiv_id": "1706.03762"
    }
  ]
}
```

### 响应

```bibtex
@article{vaswani2017attention,
  title     = {Attention Is All You Need},
  author    = {Ashish Vaswani and Noam Shazeer and Niki Parmar},
  year      = {2017},
  eprint    = {1706.03762},
  archivePrefix = {arXiv},
  primaryClass  = {cs.CL},
  url       = {https://arxiv.org/abs/1706.03762}
}
```

### 特性

- 自动识别条目类型（article、inproceedings、misc）
- ArXiv ID → `archivePrefix` + `primaryClass` 自动提取
- 引用键去重：重复的 author-year 组合自动消歧（`smith2023`、`smith2023a`、`smith2023b`）

---

## 典型工作流

论文检索工具的典型文献调研工作流：

```
1. arxiv_search("attention mechanism", category="cs.CL")
   → 获取相关论文列表

2. semantic_scholar_search("attention mechanism", year_range="2020-")
   → 获取带引用数的论文

3. pdf_fetch("https://arxiv.org/pdf/1706.03762", pages="1-5")
   → 阅读最相关的论文

4. bibtex_generate(papers=[...])
   → 为论文生成参考文献
```

Agent 可以根据用户的研究问题自动串联这些工具。
