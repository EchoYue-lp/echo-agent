//! RAG (检索增强生成) 工具
//!
//! 提供文档分块、向量索引和语义检索能力：
//! - rag_index: 文档分块并建立向量索引
//! - rag_search: 语义搜索已索引的文档
//! - rag_chunk_document: 文档分块预览（不索引）

use futures::future::BoxFuture;
use serde_json::Value;
use std::sync::{Arc, OnceLock};
use tokio::sync::RwLock;

use crate::error::{Result, ToolError};
use crate::tools::{Tool, ToolParameters, ToolResult};

// ── 共享向量存储 ────────────────────────────────────────────────────────────

/// 文档分块
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct DocumentChunk {
    id: String,
    content: String,
    embedding: Vec<f32>,
    /// 文档来源标识（可选）
    source: Option<String>,
    /// 块索引（在文档中的第几个块）
    chunk_index: usize,
    /// 总块数
    total_chunks: usize,
}

/// 内存向量存储
struct VectorStore {
    chunks: Vec<DocumentChunk>,
}

impl VectorStore {
    fn new() -> Self {
        Self { chunks: Vec::new() }
    }

    fn add_chunks(&mut self, mut chunks: Vec<DocumentChunk>) {
        self.chunks.append(&mut chunks);
    }

    fn search(&self, query_embedding: &[f32], top_k: usize) -> Vec<(f32, DocumentChunk)> {
        let mut scored: Vec<(f32, DocumentChunk)> = self
            .chunks
            .iter()
            .map(|chunk| {
                let sim = cosine_similarity(query_embedding, &chunk.embedding);
                (sim, chunk.clone())
            })
            .collect();

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);
        scored
    }
}

fn global_vector_store() -> Arc<RwLock<VectorStore>> {
    static STORE: OnceLock<Arc<RwLock<VectorStore>>> = OnceLock::new();
    STORE
        .get_or_init(|| Arc::new(RwLock::new(VectorStore::new())))
        .clone()
}

/// 余弦相似度
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a * norm_b)
    }
}

/// 段落感知的文本分块
fn chunk_text(text: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
    let paragraphs: Vec<&str> = text.split("\n\n").collect();
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();

    for para in paragraphs {
        let para = para.trim();
        if para.is_empty() {
            continue;
        }

        if current.len() + para.len() + 2 > chunk_size && !current.is_empty() {
            chunks.push(current.trim().to_string());
            current = String::new();
        }

        if para.len() > chunk_size {
            // 大段落需要进一步拆分（按句子）
            if !current.is_empty() {
                chunks.push(current.trim().to_string());
                current = String::new();
            }
            let sub_chunks = chunk_by_sentences(para, chunk_size, overlap);
            chunks.extend(sub_chunks);
        } else {
            if !current.is_empty() {
                current.push_str("\n\n");
            }
            current.push_str(para);
        }
    }

    if !current.trim().is_empty() {
        chunks.push(current.trim().to_string());
    }

    chunks
}

/// 按句子分块（处理超大段落）
fn chunk_by_sentences(text: &str, chunk_size: usize, _overlap: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        current.push(ch);

        if current.len() >= chunk_size
            && (ch == '.' || ch == '!' || ch == '?' || ch == '。' || ch == '！' || ch == '？')
        {
            chunks.push(current.trim().to_string());
            current = String::new();
        }
    }

    if !current.trim().is_empty() {
        if !chunks.is_empty() && current.len() < chunk_size / 3 {
            // 剩余小片段合并到前一个 chunk
            let last = chunks.pop().unwrap();
            chunks.push(format!("{} {}", last, current.trim()));
        } else {
            chunks.push(current.trim().to_string());
        }
    }

    chunks
}

// ── 嵌入辅助 ────────────────────────────────────────────────────────────────

/// 对一批文本生成嵌入向量
async fn generate_embeddings(texts: &[String]) -> Result<Vec<Vec<f32>>> {
    use echo_state::memory::{Embedder, HttpEmbedder};

    let embedder = HttpEmbedder::from_env();
    let mut embeddings = Vec::with_capacity(texts.len());

    for text in texts {
        let vec = embedder
            .embed(text)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                tool: "rag".to_string(),
                message: format!("嵌入生成失败: {}", e),
            })?;
        embeddings.push(vec);
    }

    Ok(embeddings)
}

// ── rag_index 工具 ──────────────────────────────────────────────────────────

pub struct RagIndexTool;

impl Tool for RagIndexTool {
    fn name(&self) -> &str {
        "rag_index"
    }

    fn description(&self) -> &str {
        "将文档分块并建立向量索引，支持后续语义检索。\
         需要配置 EMBEDDING_API_KEY 环境变量（或兼容的 OPENAI_API_KEY / EMBEDDING_APIKEY）。\
         分块大小默认 1000 字符，重叠默认 100 字符。"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "要索引的文档文本内容"
                },
                "source": {
                    "type": "string",
                    "description": "文档来源标识（如文件名、URL），用于结果溯源"
                },
                "chunk_size": {
                    "type": "integer",
                    "description": "分块大小（字符数，默认 1000）"
                },
                "overlap": {
                    "type": "integer",
                    "description": "块间重叠字符数（默认 100）"
                }
            },
            "required": ["content"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let content = parameters
                .get("content")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("content".to_string()))?;

            let source = parameters
                .get("source")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let chunk_size = parameters
                .get("chunk_size")
                .and_then(|v| v.as_u64())
                .unwrap_or(1000) as usize;

            let overlap = parameters
                .get("overlap")
                .and_then(|v| v.as_u64())
                .unwrap_or(100) as usize;

            // 分块
            let texts = chunk_text(content, chunk_size, overlap);
            let total_chunks = texts.len();

            if texts.is_empty() {
                return Ok(ToolResult::success("文档内容为空，已跳过索引".to_string()));
            }

            // 生成嵌入
            let embeddings = match generate_embeddings(&texts).await {
                Ok(e) => e,
                Err(e) => return Ok(ToolResult::error(format!("嵌入生成失败: {}", e))),
            };

            // 构建分块并存储
            let chunks: Vec<DocumentChunk> = texts
                .into_iter()
                .zip(embeddings)
                .enumerate()
                .map(|(i, (text, embedding))| DocumentChunk {
                    id: uuid::Uuid::new_v4().to_string(),
                    content: text,
                    embedding,
                    source: source.clone(),
                    chunk_index: i,
                    total_chunks,
                })
                .collect();

            let store = global_vector_store();
            store.write().await.add_chunks(chunks);

            Ok(ToolResult::success(format!(
                "成功索引 {} 个文档块{}",
                total_chunks,
                source
                    .map(|s| format!("（来源: {}）", s))
                    .unwrap_or_default()
            )))
        })
    }
}

// ── rag_search 工具 ─────────────────────────────────────────────────────────

pub struct RagSearchTool;

impl Tool for RagSearchTool {
    fn name(&self) -> &str {
        "rag_search"
    }

    fn description(&self) -> &str {
        "对已索引的文档进行语义搜索，返回最相关的 top_k 个片段及其相似度分数。\
         需要先使用 rag_index 建好索引。"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "搜索查询"
                },
                "top_k": {
                    "type": "integer",
                    "description": "返回结果数量（默认 5）"
                }
            },
            "required": ["query"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let query = parameters
                .get("query")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("query".to_string()))?;

            let top_k = parameters
                .get("top_k")
                .and_then(|v| v.as_u64())
                .unwrap_or(5) as usize;

            // 生成查询嵌入
            let query_embedding = match generate_embeddings(&[query.to_string()]).await {
                Ok(mut e) if !e.is_empty() => e.remove(0),
                Ok(_) => return Ok(ToolResult::error("嵌入生成返回空结果".to_string())),
                Err(e) => return Ok(ToolResult::error(format!("嵌入生成失败: {}", e))),
            };

            // 搜索
            let store = global_vector_store();
            let results = store.read().await.search(&query_embedding, top_k);

            if results.is_empty() {
                return Ok(ToolResult::success(
                    "未找到相关文档。请先使用 rag_index 索引文档。".to_string(),
                ));
            }

            let items: Vec<Value> = results
                .iter()
                .enumerate()
                .map(|(rank, (score, chunk))| {
                    serde_json::json!({
                        "rank": rank + 1,
                        "similarity_pct": format!("{:.1}", score * 100.0),
                        "chunk_index": chunk.chunk_index,
                        "total_chunks": chunk.total_chunks,
                        "source": chunk.source,
                        "content": chunk.content,
                    })
                })
                .collect();

            let result = serde_json::json!({
                "query": query,
                "total_results": results.len(),
                "results": items,
            });
            Ok(ToolResult::success_json(result))
        })
    }
}

// ── rag_chunk_document 工具 ─────────────────────────────────────────────────

pub struct RagChunkDocumentTool;

impl Tool for RagChunkDocumentTool {
    fn name(&self) -> &str {
        "rag_chunk_document"
    }

    fn description(&self) -> &str {
        "预览文档分块结果（不会建立索引）。用于检查分块策略和调整参数。"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "要分块的文本内容"
                },
                "chunk_size": {
                    "type": "integer",
                    "description": "分块大小（字符数，默认 1000）"
                },
                "overlap": {
                    "type": "integer",
                    "description": "块间重叠字符数（默认 100）"
                }
            },
            "required": ["content"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let content = parameters
                .get("content")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("content".to_string()))?;

            let chunk_size = parameters
                .get("chunk_size")
                .and_then(|v| v.as_u64())
                .unwrap_or(1000) as usize;

            let overlap = parameters
                .get("overlap")
                .and_then(|v| v.as_u64())
                .unwrap_or(100) as usize;

            let chunks = chunk_text(content, chunk_size, overlap);

            if chunks.is_empty() {
                return Ok(ToolResult::success("文档内容为空".to_string()));
            }

            let items: Vec<Value> = chunks
                .iter()
                .enumerate()
                .map(|(i, chunk)| {
                    let preview: String = chunk.chars().take(200).collect();
                    serde_json::json!({
                        "index": i + 1,
                        "char_count": chunk.len(),
                        "preview": preview,
                        "truncated": chunk.len() > 200,
                    })
                })
                .collect();

            let result = serde_json::json!({
                "chunk_size": chunk_size,
                "overlap": overlap,
                "total_chunks": chunks.len(),
                "chunks": items,
            });
            Ok(ToolResult::success_json(result))
        })
    }
}
