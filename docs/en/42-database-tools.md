# Database Tools

**Feature**: `database` | **Supported Databases**: SQLite, MySQL, PostgreSQL

All database tools enforce read-only mode to prevent data modification.

## SqlQueryTool — SQL Query

**Risk Level**: DatabaseRead (Level 2) | **Permission**: Read

### Security Mechanisms

- **SQL keyword filtering**: Rejects `INSERT`, `UPDATE`, `DELETE`, `DROP`, `ALTER`, `CREATE`, `TRUNCATE` and other write operation keywords
- **PostgreSQL**: Automatically executes `SET TRANSACTION READ ONLY`
- **MySQL**: Automatically executes `SET TRANSACTION READ ONLY`
- **Table name validation**: Regex-based validation to prevent SQL injection

### Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `connection` | string | ✅ | Database connection string |
| `query` | string | ✅ | SQL query (SELECT only) |
| `limit` | integer | ❌ | Max rows to return (default 100) |

### Connection String Formats

```
sqlite:///path/to/database.db
mysql://user:password@host:port/database
postgresql://user:password@host:port/database
```

### Output Format

```json
{
  "columns": ["id", "name", "email"],
  "rows": [
    {"id": 1, "name": "Alice", "email": "alice@example.com"},
    {"id": 2, "name": "Bob", "email": "bob@example.com"}
  ],
  "total_rows": 2
}
```

---

## ListTablesTool — List Database Tables

**Permission**: Read

### Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `connection` | string | ✅ | Database connection string |

### Output Format

```json
{
  "columns": ["table_name"],
  "rows": [
    {"table_name": "users"},
    {"table_name": "orders"},
    {"table_name": "products"}
  ],
  "total_rows": 3
}
```

---

## DescribeTableTool — View Table Structure

**Permission**: Read

### Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `connection` | string | ✅ | Database connection string |
| `table` | string | ✅ | Table name |

### Output Format

```json
{
  "columns": ["column_name", "data_type", "nullable"],
  "rows": [
    {"column_name": "id", "data_type": "INTEGER", "nullable": false},
    {"column_name": "name", "data_type": "VARCHAR(255)", "nullable": false},
    {"column_name": "email", "data_type": "VARCHAR(255)", "nullable": true}
  ],
  "total_rows": 3
}
```

---

## Usage Example

### Typical Data Analysis Flow

```
1. list_tables → discover available tables
2. describe_table("users") → understand table schema
3. sql_query("SELECT * FROM users WHERE age > 30 LIMIT 10") → explore data
4. sql_query("SELECT region, COUNT(*) as cnt, AVG(revenue) as avg_rev FROM orders GROUP BY region") → aggregate analysis
```
