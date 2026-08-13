//! RAG (Retrieval-Augmented Generation) tools
//!
//! Provides document chunking, vector indexing, and semantic retrieval capabilities:
//! - rag_index: chunk documents and build vector index
//! - rag_search: semantic search over indexed documents
//! - rag_chunk_document: document chunking preview (without indexing)

use futures::future::BoxFuture;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::RwLock;

use echo_core::error::{Result, ToolError};
use echo_core::tools::permission::ToolPermission;
use echo_core::tools::{Tool, ToolParameters, ToolResult};

// ── Shared vector store ───────────────────────────────────────────────────

/// Document chunk
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct DocumentChunk {
    id: String,
    content: String,
    embedding: Vec<f32>,
    /// Document source identifier (optional)
    source: Option<String>,
    /// Chunk index (position within the document)
    chunk_index: usize,
    /// Total number of chunks
    total_chunks: usize,
}

/// Maximum number of chunks to retain in memory (prevent unbounded growth)
const MAX_CHUNKS: usize = 10_000;

/// In-memory vector store
struct VectorStore {
    chunks: Vec<DocumentChunk>,
}

impl VectorStore {
    fn new() -> Self {
        Self { chunks: Vec::new() }
    }

    fn add_chunks(&mut self, mut chunks: Vec<DocumentChunk>) -> usize {
        self.chunks.append(&mut chunks);
        // Evict oldest chunks when exceeding the limit
        if self.chunks.len() > MAX_CHUNKS {
            let excess = self.chunks.len() - MAX_CHUNKS;
            self.chunks.drain(0..excess);
            excess
        } else {
            0
        }
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

/// Instance-scoped vector store shared by a configured index/search pair.
#[derive(Clone)]
pub struct RagStore {
    inner: Arc<RwLock<VectorStore>>,
}

impl RagStore {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(VectorStore::new())),
        }
    }
}

impl Default for RagStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Cosine similarity
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

/// Paragraph-aware text chunking
fn chunk_text(text: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
    if chunk_size == 0 || overlap >= chunk_size {
        return Vec::new();
    }
    let characters: Vec<char> = text.chars().collect();
    let step = chunk_size.saturating_sub(overlap);
    let mut chunks = Vec::new();
    let mut start = 0usize;
    while start < characters.len() {
        let end = start.saturating_add(chunk_size).min(characters.len());
        let chunk: String = characters
            .get(start..end)
            .unwrap_or_default()
            .iter()
            .collect();
        if !chunk.trim().is_empty() {
            chunks.push(chunk);
        }
        if end == characters.len() {
            break;
        }
        start = start.saturating_add(step);
    }
    chunks
}

// ── Embedding helper ────────────────────────────────────────────────────────────────

/// Generate embeddings for a batch of texts
async fn generate_embeddings(
    embedder: &dyn echo_core::memory::Embedder,
    texts: &[String],
) -> Result<Vec<Vec<f32>>> {
    let mut embeddings = Vec::with_capacity(texts.len());

    for text in texts {
        let vec = embedder
            .embed(text)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                tool: "rag".to_string(),
                message: format!("Embedding generation failed: {}", e),
            })?;
        embeddings.push(vec);
    }

    Ok(embeddings)
}

// ── rag_index tool ──────────────────────────────────────────────────────────

pub struct RagIndexTool {
    embedder: Arc<dyn echo_core::memory::Embedder>,
    store: RagStore,
}

impl RagIndexTool {
    pub fn new(embedder: Arc<dyn echo_core::memory::Embedder>, store: RagStore) -> Self {
        Self { embedder, store }
    }
}

impl Tool for RagIndexTool {
    fn name(&self) -> &str {
        "rag_index"
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Read, ToolPermission::Write]
    }

    fn description(&self) -> &str {
        "Chunk documents and build a vector index for semantic retrieval. \
         Requires EMBEDDING_API_KEY env var (or compatible OPENAI_API_KEY / EMBEDDING_APIKEY). \
         Default chunk size 1000 chars, overlap 100 chars."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "Document text content to index"
                },
                "source": {
                    "type": "string",
                    "description": "Document source identifier (e.g. filename, URL) for result traceability"
                },
                "chunk_size": {
                    "type": "integer",
                    "description": "Chunk size in characters (default 1000)"
                },
                "overlap": {
                    "type": "integer",
                    "description": "Overlap characters between chunks (default 100)"
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

            if chunk_size == 0 || overlap >= chunk_size {
                return Err(ToolError::InvalidParameter {
                    name: "overlap".to_string(),
                    message:
                        "chunk_size must be positive and overlap must be smaller than chunk_size"
                            .to_string(),
                }
                .into());
            }
            let texts = chunk_text(content, chunk_size, overlap);
            let total_chunks = texts.len();

            if texts.is_empty() {
                return Ok(ToolResult::success(
                    "Document content is empty, index skipped".to_string(),
                ));
            }

            // Generate embeddings
            let embeddings = match generate_embeddings(self.embedder.as_ref(), &texts).await {
                Ok(e) => e,
                Err(e) => {
                    return Ok(ToolResult::error(format!(
                        "Embedding generation failed: {}",
                        e
                    )));
                }
            };

            // Build chunks and store
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

            let evicted = self.store.inner.write().await.add_chunks(chunks);

            let mut result = ToolResult::success(format!(
                "Successfully indexed {} document chunks{}",
                total_chunks,
                source
                    .map(|s| format!(" (source: {})", s))
                    .unwrap_or_default()
            ));
            result
                .metadata
                .insert("evicted_chunks".to_string(), evicted.to_string());
            Ok(result)
        })
    }
}

// ── rag_search tool ─────────────────────────────────────────────────────────

pub struct RagSearchTool {
    embedder: Arc<dyn echo_core::memory::Embedder>,
    store: RagStore,
}

impl RagSearchTool {
    pub fn new(embedder: Arc<dyn echo_core::memory::Embedder>, store: RagStore) -> Self {
        Self { embedder, store }
    }
}

impl Tool for RagSearchTool {
    fn name(&self) -> &str {
        "rag_search"
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Read]
    }

    fn description(&self) -> &str {
        "Semantic search over indexed documents, returning top_k most relevant chunks with similarity scores. \
         Requires prior rag_index to build the index."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query"
                },
                "top_k": {
                    "type": "integer",
                    "description": "Number of results to return (default 5)"
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

            // Generate query embedding
            let query_embedding =
                match generate_embeddings(self.embedder.as_ref(), &[query.to_string()]).await {
                    Ok(e) => match e.into_iter().next() {
                        Some(embedding) => embedding,
                        None => {
                            return Ok(ToolResult::error(
                                "Embedding generation returned empty result".to_string(),
                            ));
                        }
                    },
                    Err(e) => {
                        return Ok(ToolResult::error(format!(
                            "Embedding generation failed: {}",
                            e
                        )));
                    }
                };

            // Search
            let results = self
                .store
                .inner
                .read()
                .await
                .search(&query_embedding, top_k);

            if results.is_empty() {
                return Ok(ToolResult::success(
                    "No relevant documents found. Please use rag_index to index documents first."
                        .to_string(),
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

// ── rag_chunk_document tool ─────────────────────────────────────────────────

pub struct RagChunkDocumentTool;

impl Tool for RagChunkDocumentTool {
    fn name(&self) -> &str {
        "rag_chunk_document"
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Read]
    }

    fn description(&self) -> &str {
        "Preview document chunking results (does not build an index). Use to inspect chunking strategy and tune parameters."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "Text content to chunk"
                },
                "chunk_size": {
                    "type": "integer",
                    "description": "Chunk size in characters (default 1000)"
                },
                "overlap": {
                    "type": "integer",
                    "description": "Overlap characters between chunks (default 100)"
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
                return Ok(ToolResult::success("Document content is empty".to_string()));
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
