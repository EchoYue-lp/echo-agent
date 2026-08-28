//! Database SQL tools
//!
//! Provides cross-database read-only query capabilities via sqlx:
//! - sql_query: execute read-only SQL queries
//! - list_tables: list all tables in the database
//! - describe_table: view table structure

use base64::Engine;
use futures::{StreamExt, future::BoxFuture};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::any::{AnyPoolOptions, AnyTypeInfoKind};
use sqlx::{Column, Row};

use echo_core::error::{Result, ToolError};
use echo_core::tools::artifact::{
    ToolOutputArtifactIdentity, ToolOutputArtifactRef, persist_tool_output,
};
use echo_core::tools::pagination::{PageInfo, PageRequest};
use echo_core::tools::permission::ToolPermission;
use echo_core::tools::{Tool, ToolParameters, ToolResult, ToolRiskLevel};

const DEFAULT_SQL_PAGE_ROWS: usize = 50;
const MAX_SQL_PAGE_ROWS: usize = 100;
const MAX_INLINE_COLUMN_CHARS: usize = 80;
const MAX_INLINE_CELL_CHARS: usize = 512;

// sqlx 0.8 `any` mode requires drivers to be installed at runtime before the
// first `AnyPool` connection. Without this, connecting panics with
// "No drivers installed". We install all compiled-in drivers once, lazily on
// first use, guarded by a OnceLock so repeated calls are cheap and safe.
//
// (sqlx 0.8 removed the automatic install that earlier versions did via
// `AnyConnection::install_default_drivers` being called implicitly; the
// explicit `install_default_drivers()` is the documented replacement.)
fn ensure_drivers_installed() {
    use std::sync::OnceLock;
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        // install_default_drivers installs every driver feature compiled into
        // sqlx (sqlite/postgres/mysql as enabled in Cargo.toml). Idempotent.
        sqlx::any::install_default_drivers();
    });
}

// ── SQL Query (read-only) ─────────────────────────────────────────────────────────

pub struct SqlQueryTool;

impl Tool for SqlQueryTool {
    fn name(&self) -> &str {
        "sql_query"
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Network]
    }

    fn risk_level(&self) -> ToolRiskLevel {
        ToolRiskLevel::ReadOnly
    }

    fn description(&self) -> &str {
        "Execute read-only SQL queries (SELECT only). Supports SQLite, MySQL, PostgreSQL. \
         Connection URL format: sqlite://path.db, mysql://user:pass@host/db, postgresql://user:pass@host/db"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "connection_url": {
                    "type": "string",
                    "description": "Database connection URL (sqlite:///path.db | mysql://user:pass@host/db | postgresql://user:pass@host/db)"
                },
                "query": {
                    "type": "string",
                    "description": "SQL query to execute (only SELECT / SHOW / DESCRIBE / EXPLAIN allowed)"
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_SQL_PAGE_ROWS,
                    "description": "Maximum rows per page (default 50)"
                },
                "cursor": {
                    "type": "string",
                    "description": "Opaque cursor from page.next_cursor; reuse only with identical parameters"
                }
            },
            "required": ["connection_url", "query"]
        })
    }

    fn execute_with_context<'a>(
        &'a self,
        parameters: ToolParameters,
        ctx: &'a echo_core::tools::ToolContext,
    ) -> BoxFuture<'a, Result<ToolResult>> {
        Box::pin(async move {
            let conn_url = parameters
                .get("connection_url")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("connection_url".to_string()))?;

            // Validate connection URL scheme
            validate_db_url(conn_url)?;

            let query = parameters
                .get("query")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("query".to_string()))?;
            let page_request = match PageRequest::from_parameters(
                &parameters,
                DEFAULT_SQL_PAGE_ROWS,
                MAX_SQL_PAGE_ROWS,
            ) {
                Ok(request) => request,
                Err(error) => return Ok(ToolResult::invalid_arguments(error.to_string())),
            };

            if let Err(error) = validate_readonly_query(query) {
                return Ok(ToolResult::error(error));
            }

            match execute_readonly_page(conn_url, query, &page_request).await {
                Ok(data) => {
                    let full_data = serde_json::json!({
                        "columns": data.columns,
                        "rows": data.rows,
                        "page": &data.page_info,
                    });
                    sql_page_result(ctx, full_data, data.page_info)
                }
                Err(e) => Ok(ToolResult::error(format!("Query failed: {}", e))),
            }
        })
    }
}

fn validate_readonly_query(query: &str) -> std::result::Result<(), String> {
    let trimmed = query.trim().to_uppercase();
    let allowed = trimmed.starts_with("SELECT")
        || trimmed.starts_with("SHOW")
        || trimmed.starts_with("DESCRIBE")
        || trimmed.starts_with("DESC ")
        || trimmed.starts_with("EXPLAIN")
        || trimmed.starts_with("WITH");
    if !allowed {
        return Err(format!(
            "Only read-only queries allowed (SELECT/SHOW/DESCRIBE/EXPLAIN), received: {query}"
        ));
    }
    const MUTATING_SQL: &[&str] = &[
        "INSERT",
        "UPDATE",
        "DELETE",
        "DROP",
        "ALTER",
        "CREATE",
        "TRUNCATE",
        "GRANT",
        "REVOKE",
        "REPLACE",
        "EXECUTE",
        "EXEC",
        "INTO OUTFILE",
        "INTO DUMPFILE",
        "LOAD_FILE",
        "DBLINK",
        "LO_IMPORT",
        "PG_TERMINATE_BACKEND",
        "PG_CANCEL_BACKEND",
        "PG_RELOAD_CONF",
        "COPY",
        "PRAGMA",
    ];
    if let Some(keyword) = MUTATING_SQL
        .iter()
        .find(|keyword| trimmed.contains(**keyword))
    {
        return Err(format!(
            "Query contains forbidden keyword: {keyword}. Only read-only queries allowed."
        ));
    }
    Ok(())
}

// ── List tables ───────────────────────────────────────────────────────────────────

pub struct ListTablesTool;

impl Tool for ListTablesTool {
    fn name(&self) -> &str {
        "list_tables"
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Network]
    }

    fn description(&self) -> &str {
        "List all tables in the database. Supports SQLite, MySQL, PostgreSQL."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "connection_url": {
                    "type": "string",
                    "description": "Database connection URL"
                }
            },
            "required": ["connection_url"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let conn_url = parameters
                .get("connection_url")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("connection_url".to_string()))?;

            // Validate connection URL scheme
            validate_db_url(conn_url)?;

            // Choose appropriate query based on database type
            let query = if conn_url.starts_with("sqlite") {
                "SELECT name AS table_name FROM sqlite_master WHERE type='table' ORDER BY name"
            } else if conn_url.starts_with("mysql") {
                "SELECT TABLE_NAME AS table_name FROM information_schema.TABLES WHERE TABLE_SCHEMA = DATABASE() ORDER BY TABLE_NAME"
            } else {
                // PostgreSQL and others
                "SELECT table_name FROM information_schema.tables WHERE table_schema = 'public' ORDER BY table_name"
            };

            match execute_readonly_query(conn_url, query).await {
                Ok(data) => Ok(ToolResult::success_json(data.into_json())),
                Err(e) => Ok(ToolResult::error(format!("List tables failed: {}", e))),
            }
        })
    }
}

// ── Describe table structure ───────────────────────────────────────────────────────────────

pub struct DescribeTableTool;

impl Tool for DescribeTableTool {
    fn name(&self) -> &str {
        "describe_table"
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Network]
    }

    fn description(&self) -> &str {
        "View the structure of a specified table (column names, types, nullable). Supports SQLite, MySQL, PostgreSQL."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "connection_url": {
                    "type": "string",
                    "description": "Database connection URL"
                },
                "table_name": {
                    "type": "string",
                    "description": "Table name to view"
                }
            },
            "required": ["connection_url", "table_name"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let conn_url = parameters
                .get("connection_url")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("connection_url".to_string()))?;

            // Validate connection URL scheme
            validate_db_url(conn_url)?;

            let table_name = parameters
                .get("table_name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("table_name".to_string()))?;

            // Validate table name: only allow alphanumeric, underscore, dot (for schema.table)
            if !table_name
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '.')
            {
                return Ok(ToolResult::error(format!(
                    "Invalid table name '{}': only alphanumeric, underscore, and dot characters allowed",
                    table_name
                )));
            }

            // Choose appropriate query based on database type
            let query = if conn_url.starts_with("sqlite") {
                format!("PRAGMA table_info('{}')", table_name.replace('\'', "''"))
            } else if conn_url.starts_with("mysql") {
                format!(
                    "SELECT COLUMN_NAME, DATA_TYPE, IS_NULLABLE, COLUMN_DEFAULT \
                     FROM information_schema.COLUMNS \
                     WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = '{}' \
                     ORDER BY ORDINAL_POSITION",
                    table_name.replace('\'', "''")
                )
            } else {
                // PostgreSQL
                format!(
                    "SELECT column_name, data_type, is_nullable, column_default \
                     FROM information_schema.columns \
                     WHERE table_name = '{}' \
                     ORDER BY ordinal_position",
                    table_name.replace('\'', "''")
                )
            };

            match execute_readonly_query(conn_url, &query).await {
                Ok(data) => Ok(ToolResult::success_json(data.into_json())),
                Err(e) => Ok(ToolResult::error(format!("Describe table failed: {}", e))),
            }
        })
    }
}

// ── Helper ───────────────────────────────────────────────────────────────────

/// Validate database connection URL scheme using proper parsing.
///
/// Only allows `sqlite://`, `mysql://`, `postgresql://`, and `postgres://` schemes.
/// For SQLite, also validates that the path doesn't contain suspicious patterns.
fn validate_db_url(url_str: &str) -> Result<()> {
    let (scheme, path) = url_str
        .split_once("://")
        .ok_or_else(|| ToolError::InvalidParameter {
            name: "connection_url".to_string(),
            message: "Invalid URL format: missing '://'".to_string(),
        })?;

    match scheme {
        "sqlite" | "mysql" | "postgresql" | "postgres" => {}
        _ => {
            return Err(ToolError::InvalidParameter {
                name: "connection_url".to_string(),
                message: format!(
                    "Unsupported database scheme '{}'. Use sqlite://, mysql://, or postgresql://",
                    scheme
                ),
            }
            .into());
        }
    }

    // For SQLite, validate the path doesn't contain suspicious patterns
    if scheme == "sqlite" {
        // Strip leading slash for relative paths (sqlite:///path vs sqlite://:memory:)
        let path = path.strip_prefix('/').unwrap_or(path);
        if path.contains('\0') || path.contains('\n') || path.contains('\r') {
            return Err(ToolError::InvalidParameter {
                name: "connection_url".to_string(),
                message: "SQLite path contains invalid characters".to_string(),
            }
            .into());
        }
    }

    Ok(())
}

/// Execute a read-only query and return structured JSON.
///
/// Defense-in-depth: wraps non-SQLite queries in an explicit READ ONLY transaction,
/// so the database itself rejects any mutation attempt (not just our keyword filter).
struct DbRows {
    columns: Vec<String>,
    rows: Vec<Vec<serde_json::Value>>,
}

struct DbPage {
    columns: Vec<String>,
    rows: Vec<Vec<serde_json::Value>>,
    page_info: PageInfo,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SqlCursor {
    version: u8,
    fingerprint: String,
    offset: usize,
    limit: usize,
}

fn sql_query_fingerprint(conn_url: &str, query: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(conn_url.as_bytes());
    hasher.update([0]);
    hasher.update(query.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn decode_sql_cursor(request: &PageRequest, conn_url: &str, query: &str) -> Result<usize> {
    let Some(cursor) = request.cursor.as_deref() else {
        return Ok(0);
    };
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|error| ToolError::InvalidParameter {
            name: "cursor".to_string(),
            message: format!("invalid SQL cursor encoding: {error}"),
        })?;
    let decoded: SqlCursor =
        serde_json::from_slice(&bytes).map_err(|error| ToolError::InvalidParameter {
            name: "cursor".to_string(),
            message: format!("invalid SQL cursor payload: {error}"),
        })?;
    if decoded.version != 1
        || decoded.limit != request.limit
        || decoded.fingerprint != sql_query_fingerprint(conn_url, query)
    {
        return Err(ToolError::InvalidParameter {
            name: "cursor".to_string(),
            message: "SQL cursor does not match this query and limit".to_string(),
        }
        .into());
    }
    Ok(decoded.offset)
}

fn encode_sql_cursor(cursor: &SqlCursor) -> Result<String> {
    let bytes = serde_json::to_vec(cursor).map_err(|error| ToolError::ExecutionFailed {
        tool: "sql_query".to_string(),
        message: format!("failed to encode SQL cursor: {error}"),
    })?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

async fn execute_readonly_page(
    conn_url: &str,
    query: &str,
    request: &PageRequest,
) -> Result<DbPage> {
    ensure_drivers_installed();
    let offset = decode_sql_cursor(request, conn_url, query)?;
    let pool = AnyPoolOptions::new()
        .max_connections(1)
        .connect(conn_url)
        .await
        .map_err(|error| ToolError::ExecutionFailed {
            tool: "database".to_string(),
            message: format!("Database connection failed: {error}"),
        })?;
    let take = request.limit.saturating_add(1);
    let rows = if conn_url.starts_with("sqlite") {
        collect_bounded_rows(sqlx::query(query).fetch(&pool), offset, take).await?
    } else {
        let mut transaction = pool
            .begin()
            .await
            .map_err(|error| ToolError::ExecutionFailed {
                tool: "database".to_string(),
                message: format!("Failed to begin transaction: {error}"),
            })?;
        sqlx::query("SET TRANSACTION READ ONLY")
            .execute(&mut *transaction)
            .await
            .map_err(|error| ToolError::ExecutionFailed {
                tool: "database".to_string(),
                message: format!("Failed to set read-only mode: {error}"),
            })?;
        let rows =
            collect_bounded_rows(sqlx::query(query).fetch(&mut *transaction), offset, take).await?;
        transaction
            .rollback()
            .await
            .map_err(|error| ToolError::ExecutionFailed {
                tool: "database".to_string(),
                message: format!("Failed to close read-only transaction: {error}"),
            })?;
        rows
    };
    build_db_page(rows, request, offset, conn_url, query)
}

async fn collect_bounded_rows<S>(
    mut stream: S,
    offset: usize,
    take: usize,
) -> Result<Vec<sqlx::any::AnyRow>>
where
    S: futures::Stream<Item = std::result::Result<sqlx::any::AnyRow, sqlx::Error>> + Unpin,
{
    let mut rows = Vec::with_capacity(take);
    let mut seen = 0usize;
    while let Some(row) = stream.next().await {
        let row = row.map_err(|error| ToolError::ExecutionFailed {
            tool: "database".to_string(),
            message: format!("Query execution failed: {error}"),
        })?;
        if seen < offset {
            seen = seen.saturating_add(1);
            continue;
        }
        rows.push(row);
        if rows.len() >= take {
            break;
        }
    }
    Ok(rows)
}

fn build_db_page(
    mut rows: Vec<sqlx::any::AnyRow>,
    request: &PageRequest,
    offset: usize,
    conn_url: &str,
    query: &str,
) -> Result<DbPage> {
    let has_more = rows.len() > request.limit;
    if has_more {
        rows.truncate(request.limit);
    }
    let formatted = format_db_rows(&rows)?;
    let next_offset = offset.saturating_add(formatted.rows.len());
    let next_cursor = if has_more {
        Some(encode_sql_cursor(&SqlCursor {
            version: 1,
            fingerprint: sql_query_fingerprint(conn_url, query),
            offset: next_offset,
            limit: request.limit,
        })?)
    } else {
        None
    };
    Ok(DbPage {
        columns: formatted.columns,
        rows: formatted.rows,
        page_info: PageInfo {
            next_cursor,
            truncated: has_more,
            total_known: false,
            total: None,
            returned: next_offset.saturating_sub(offset),
        },
    })
}

impl DbRows {
    fn into_json(self) -> serde_json::Value {
        let total_rows = self.rows.len();
        serde_json::json!({
            "columns": self.columns,
            "rows": self.rows,
            "total_rows": total_rows,
        })
    }
}

async fn execute_readonly_query(conn_url: &str, query: &str) -> Result<DbRows> {
    ensure_drivers_installed();
    let pool = AnyPoolOptions::new()
        .max_connections(1)
        .connect(conn_url)
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            tool: "database".to_string(),
            message: format!("Database connection failed: {}", e),
        })?;

    if conn_url.starts_with("sqlite") {
        // SQLite does not support SET TRANSACTION READ ONLY — skip.
        let rows =
            sqlx::query(query)
                .fetch_all(&pool)
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: "database".to_string(),
                    message: format!("Query execution failed: {}", e),
                })?;
        format_db_rows(&rows)
    } else {
        // PostgreSQL/MySQL: wrap in an explicit READ ONLY transaction.
        // Using pool.begin() starts an explicit transaction; SET TRANSACTION READ ONLY
        // then applies to this transaction (not just an autocommit-internal one).
        let mut tx = pool.begin().await.map_err(|e| ToolError::ExecutionFailed {
            tool: "database".to_string(),
            message: format!("Failed to begin transaction: {}", e),
        })?;
        sqlx::query("SET TRANSACTION READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                tool: "database".to_string(),
                message: format!("Failed to set read-only mode: {}", e),
            })?;
        let rows = sqlx::query(query).fetch_all(&mut *tx).await.map_err(|e| {
            ToolError::ExecutionFailed {
                tool: "database".to_string(),
                message: format!("Query execution failed: {}", e),
            }
        })?;
        // Read-only transaction — explicitly rollback (nothing to write). The
        // comment previously said "rollback on drop otherwise" but the code
        // committed; align the two (P1-1). For a read-only tx commit/rollback
        // are equivalent, but rollback is unambiguous about intent.
        tx.rollback().await.ok();
        format_db_rows(&rows)
    }
}

/// Format sqlx query result rows into structured JSON.
fn format_db_rows(rows: &[sqlx::any::AnyRow]) -> Result<DbRows> {
    let columns: Vec<String> = rows
        .first()
        .map(|row| row.columns().iter().map(|c| c.name().to_string()).collect())
        .unwrap_or_default();

    let col_count = columns.len();
    let mut row_values: Vec<Vec<serde_json::Value>> = Vec::with_capacity(rows.len());

    for row in rows {
        let mut values: Vec<serde_json::Value> = Vec::with_capacity(col_count);
        for (index, column) in row.columns().iter().enumerate() {
            values.push(decode_db_cell(
                row,
                index,
                column.name(),
                column.type_info().kind(),
            )?);
        }
        row_values.push(values);
    }

    Ok(DbRows {
        columns,
        rows: row_values,
    })
}

fn decode_db_cell(
    row: &sqlx::any::AnyRow,
    index: usize,
    column_name: &str,
    kind: AnyTypeInfoKind,
) -> Result<serde_json::Value> {
    let decode_error = |error: sqlx::Error| ToolError::ExecutionFailed {
        tool: "database".to_string(),
        message: format!(
            "Failed to decode column '{}' as {:?}: {error}",
            column_name, kind
        ),
    };
    let value = match kind {
        AnyTypeInfoKind::Null => serde_json::Value::Null,
        AnyTypeInfoKind::Bool => match row
            .try_get::<Option<bool>, _>(index)
            .map_err(decode_error)?
        {
            Some(value) => serde_json::Value::Bool(value),
            None => serde_json::Value::Null,
        },
        AnyTypeInfoKind::SmallInt => {
            serde_json::json!(row.try_get::<Option<i16>, _>(index).map_err(decode_error)?)
        }
        AnyTypeInfoKind::Integer => {
            serde_json::json!(row.try_get::<Option<i32>, _>(index).map_err(decode_error)?)
        }
        AnyTypeInfoKind::BigInt => {
            serde_json::json!(row.try_get::<Option<i64>, _>(index).map_err(decode_error)?)
        }
        AnyTypeInfoKind::Real => {
            serde_json::json!(row.try_get::<Option<f32>, _>(index).map_err(decode_error)?)
        }
        AnyTypeInfoKind::Double => {
            serde_json::json!(row.try_get::<Option<f64>, _>(index).map_err(decode_error)?)
        }
        AnyTypeInfoKind::Text => match row
            .try_get::<Option<String>, _>(index)
            .map_err(decode_error)?
        {
            Some(value) => serde_json::Value::String(value),
            None => serde_json::Value::Null,
        },
        AnyTypeInfoKind::Blob => match row
            .try_get::<Option<Vec<u8>>, _>(index)
            .map_err(decode_error)?
        {
            Some(bytes) => serde_json::json!({
                "type": "binary",
                "encoding": "base64",
                "data": base64::engine::general_purpose::STANDARD.encode(bytes),
            }),
            None => serde_json::Value::Null,
        },
    };
    Ok(value)
}

fn sql_page_result(
    ctx: &echo_core::tools::ToolContext,
    full_data: serde_json::Value,
    page_info: echo_core::tools::pagination::PageInfo,
) -> Result<ToolResult> {
    let full_output =
        serde_json::to_string(&full_data).map_err(|error| ToolError::ExecutionFailed {
            tool: "sql_query".to_string(),
            message: format!("Failed to serialize query page: {error}"),
        })?;
    let mut artifact_error = None;
    let artifact = match ctx.output_artifacts.clone() {
        Some(config) => match persist_tool_output(
            config,
            ToolOutputArtifactIdentity::from_context(ctx, "sql_query"),
            &full_output,
        ) {
            Ok(artifact) => artifact,
            Err(error) => {
                tracing::warn!(%error, "sql_query output artifact write failed");
                artifact_error = Some(error.to_string());
                None
            }
        },
        None => None,
    };

    let visible_data = if artifact.is_some() {
        project_sql_data(&full_data)
    } else {
        full_data
    };
    let mut result = ToolResult::success_json(visible_data);
    if let Some(artifact) = artifact {
        apply_sql_artifact(artifact, full_output.len(), &mut result);
    } else if let Some(error) = artifact_error {
        result
            .metadata
            .insert("artifact_status".to_string(), "write_failed".to_string());
        result.metadata.insert("artifact_error".to_string(), error);
    }
    page_info.apply_to(&mut result);
    Ok(result)
}

fn project_sql_data(data: &serde_json::Value) -> serde_json::Value {
    let columns = data
        .get("columns")
        .and_then(Value::as_array)
        .map(|columns| {
            columns
                .iter()
                .map(|column| match column.as_str() {
                    Some(column) => Value::String(char_preview(column, MAX_INLINE_COLUMN_CHARS)),
                    None => column.clone(),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let rows = data
        .get("rows")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    Value::Array(
                        row.as_array()
                            .map(|cells| {
                                cells
                                    .iter()
                                    .map(|cell| match cell.as_str() {
                                        Some(cell) => {
                                            Value::String(char_preview(cell, MAX_INLINE_CELL_CHARS))
                                        }
                                        None => cell.clone(),
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    serde_json::json!({
        "columns": columns,
        "rows": rows,
        "page": data.get("page").cloned().unwrap_or(Value::Null),
        "complete_page": "stored in artifact metadata",
    })
}

fn char_preview(value: &str, max_chars: usize) -> String {
    let mut preview: String = value.chars().take(max_chars).collect();
    if value.chars().count() > max_chars {
        preview.push_str("...");
    }
    preview
}

fn apply_sql_artifact(
    artifact: ToolOutputArtifactRef,
    original_bytes: usize,
    result: &mut ToolResult,
) {
    result.artifact = Some(artifact);
    result
        .metadata
        .insert("output_handling".to_string(), "spilled".to_string());
    result
        .metadata
        .insert("original_bytes".to_string(), original_bytes.to_string());
    result.metadata.insert(
        "returned_bytes".to_string(),
        result.output.len().to_string(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::error::ReactError;

    #[tokio::test]
    async fn sqlite_page_is_bounded_and_preserves_typed_cells() -> Result<()> {
        let directory = tempfile::tempdir().map_err(ReactError::from)?;
        let database = directory.path().join("typed.sqlite");
        let url = format!("sqlite://{}?mode=rwc", database.display());
        ensure_drivers_installed();
        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .map_err(|error| ToolError::ExecutionFailed {
                tool: "database-test".to_string(),
                message: error.to_string(),
            })?;
        sqlx::query(
            "CREATE TABLE samples (id INTEGER, flag INTEGER, score REAL, label TEXT, payload BLOB, optional INTEGER)",
        )
        .execute(&pool)
        .await
        .map_err(|error| ToolError::ExecutionFailed {
            tool: "database-test".to_string(),
            message: error.to_string(),
        })?;
        for id in 1_i64..=5_i64 {
            sqlx::query(
                "INSERT INTO samples (id, flag, score, label, payload, optional) VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(id)
            .bind(id % 2 == 0)
            .bind(id as f64 + 0.5)
            .bind(format!("row-{id}"))
            .bind(vec![u8::try_from(id).unwrap_or_default(), 255])
            .bind((id != 1).then_some(id))
            .execute(&pool)
            .await
            .map_err(|error| ToolError::ExecutionFailed {
                tool: "database-test".to_string(),
                message: error.to_string(),
            })?;
        }
        pool.close().await;

        let first_request = PageRequest {
            limit: 2,
            cursor: None,
        };
        let first = execute_readonly_page(
            &url,
            "SELECT id, flag, score, label, payload, optional FROM samples ORDER BY id",
            &first_request,
        )
        .await?;
        assert_eq!(first.rows.len(), 2);
        assert_eq!(
            first.rows.first().and_then(|row| row.first()),
            Some(&serde_json::json!(1))
        );
        assert_eq!(
            first.rows.first().and_then(|row| row.get(5)),
            Some(&serde_json::Value::Null)
        );
        assert_eq!(
            first
                .rows
                .first()
                .and_then(|row| row.get(4))
                .and_then(|value| value.get("encoding")),
            Some(&serde_json::json!("base64"))
        );
        let cursor = first.page_info.next_cursor.ok_or_else(|| {
            ReactError::Other("first SQL page did not return a cursor".to_string())
        })?;

        let second = execute_readonly_page(
            &url,
            "SELECT id, flag, score, label, payload, optional FROM samples ORDER BY id",
            &PageRequest {
                limit: 2,
                cursor: Some(cursor),
            },
        )
        .await?;
        assert_eq!(second.rows.len(), 2);
        assert_eq!(
            second.rows.first().and_then(|row| row.first()),
            Some(&serde_json::json!(3))
        );
        assert!(second.page_info.next_cursor.is_some());
        assert!(!second.page_info.total_known);
        Ok(())
    }

    #[test]
    fn writable_sql_never_qualifies_as_read_only() {
        use echo_core::tools::Tool;

        assert_eq!(SqlQueryTool.risk_level(), ToolRiskLevel::ReadOnly);
        assert!(validate_readonly_query("SELECT * FROM samples").is_ok());
        assert!(validate_readonly_query("UPDATE samples SET flag = true").is_err());
        assert!(
            validate_readonly_query(
                "WITH changed AS (DELETE FROM samples RETURNING id) SELECT * FROM changed"
            )
            .is_err()
        );
    }
}
