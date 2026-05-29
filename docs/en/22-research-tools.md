# Research Tools

## Overview

Research tools enable academic paper discovery, retrieval, and bibliography management. They cover the full literature review workflow: search → fetch → read → cite.

Available tools:
- `arxiv_search` — Search ArXiv for preprints
- `semantic_scholar_search` — Search Semantic Scholar for published papers
- `pdf_fetch` — Download and parse PDF documents
- `bibtex_generate` — Generate BibTeX citations from paper metadata

> **Feature flag:** `research` — enable via `features = ["research"]` in Cargo.toml.

---

## arxiv_search

Search the ArXiv API for academic preprints. Returns structured metadata including title, authors, abstract, categories, and PDF link.

### Parameters

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `query` | string | yes | — | Search keywords, title, or author name |
| `max_results` | integer | no | 20 | Max results (capped at 100) |
| `category` | string | no | — | ArXiv category filter (e.g. `cs.AI`, `cs.LG`, `cs.CL`, `math.CO`) |
| `sort_by` | string | no | `relevance` | Sort: `relevance`, `lastUpdatedDate`, `submittedDate` |
| `sort_order` | string | no | `descending` | Direction: `descending` or `ascending` |

### Example

```json
{
  "query": "transformer attention mechanism",
  "max_results": 10,
  "category": "cs.CL",
  "sort_by": "submittedDate"
}
```

### Response

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

Search Semantic Scholar for published academic papers. Returns rich metadata including citation count, reference count, fields of study, and venue. No API key required (rate-limited to 100 requests per 5 minutes).

### Parameters

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `query` | string | yes | — | Search keywords, title, or author |
| `limit` | integer | no | 20 | Max results (capped at 100) |
| `fields` | string | no | (all) | Comma-separated fields to return |
| `year_range` | string | no | — | Year filter: `2020-2025`, `2020-`, `-2023` |
| `fields_of_study` | string | no | — | Filter by field (e.g. `Computer Science`) |
| `sort` | string | no | `relevance` | Sort: `relevance`, `citationCount:desc`, `year:desc` |

### Example

```json
{
  "query": "large language models",
  "limit": 5,
  "year_range": "2023-",
  "fields_of_study": "Computer Science",
  "sort": "citationCount:desc"
}
```

### Response

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

Download a PDF from a URL and extract its text content. Supports arXiv PDF links, direct URLs, and HTTP redirects. Useful for reading academic papers within the agent workflow.

### Parameters

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `url` | string | yes | — | URL of the PDF to download |
| `pages` | string | no | `1-20` | Page range: `1-5`, `1,3,7`, or `all` |
| `max_chars` | integer | no | 50000 | Maximum characters to extract |

### Example

```json
{
  "url": "https://arxiv.org/pdf/1706.03762",
  "pages": "1-10",
  "max_chars": 30000
}
```

### Response

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

### Security

- SSRF protection: blocks private IPs (127.0.0.1, 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16), localhost, `.local` domains
- Validates `%PDF` magic bytes before parsing
- 60-second timeout with safe redirect policy

---

## bibtex_generate

Generate BibTeX entries from paper metadata. Supports papers from arXiv, Semantic Scholar, or manually provided metadata. Produces standard `.bib` format text.

### Parameters

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `papers` | array | yes | — | Array of paper metadata objects |
| `papers[].title` | string | yes | — | Paper title |
| `papers[].authors` | array | yes | — | List of author names |
| `papers[].year` | integer | no | — | Publication year |
| `papers[].url` | string | no | — | Paper URL |
| `papers[].arxiv_id` | string | no | — | ArXiv ID (e.g. `1706.03762`) |
| `papers[].doi` | string | no | — | DOI identifier |
| `papers[].venue` | string | no | — | Conference or journal name |

### Example

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

### Response

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

### Features

- Automatic entry type detection (article, inproceedings, misc)
- ArXiv ID → `archivePrefix` + `primaryClass` extraction
- Cite key disambiguation for duplicate author-year combinations (`smith2023`, `smith2023a`, `smith2023b`)

---

## Typical Workflow

A typical literature review workflow with research tools:

```
1. arxiv_search("attention mechanism", category="cs.CL")
   → Get list of relevant papers

2. semantic_scholar_search("attention mechanism", year_range="2020-")
   → Get papers with citation counts

3. pdf_fetch("https://arxiv.org/pdf/1706.03762", pages="1-5")
   → Read the most relevant paper

4. bibtex_generate(papers=[...])
   → Generate bibliography for the paper
```

The agent can chain these tools automatically based on the user's research query.
