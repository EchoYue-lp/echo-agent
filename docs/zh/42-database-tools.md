# 数据库工具

**Feature**: `database` | **支持的数据库**: SQLite, MySQL, PostgreSQL

所有数据库工具均强制只读模式，防止数据修改。

## SqlQueryTool — SQL 查询

**风险等级**: DatabaseRead (Level 2) | **权限**: Read

### 安全机制

- **SQL 关键字过滤**: 拒绝 `INSERT`, `UPDATE`, `DELETE`, `DROP`, `ALTER`, `CREATE`, `TRUNCATE` 等写操作关键字
- **PostgreSQL**: 自动执行 `SET TRANSACTION READ ONLY`
- **MySQL**: 自动执行 `SET TRANSACTION READ ONLY`
- **表名验证**: 使用正则表达式验证，防止 SQL 注入

### 参数

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `connection` | string | ✅ | 数据库连接字符串 |
| `query` | string | ✅ | SQL 查询语句（仅 SELECT） |
| `limit` | integer | ❌ | 返回行数上限（默认 100） |

### 连接字符串格式

```
sqlite:///path/to/database.db
mysql://user:password@host:port/database
postgresql://user:password@host:port/database
```

### 输出格式

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

## ListTablesTool — 列出数据库表

**权限**: Read

### 参数

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `connection` | string | ✅ | 数据库连接字符串 |

### 输出格式

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

## DescribeTableTool — 查看表结构

**权限**: Read

### 参数

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `connection` | string | ✅ | 数据库连接字符串 |
| `table` | string | ✅ | 表名 |

### 输出格式

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

## 使用示例

### 典型数据分析流程

```
1. list_tables → 发现可用表
2. describe_table("users") → 了解表结构
3. sql_query("SELECT * FROM users WHERE age > 30 LIMIT 10") → 探索数据
4. sql_query("SELECT region, COUNT(*) as cnt, AVG(revenue) as avg_rev FROM orders GROUP BY region") → 聚合分析
```

### 安全注意事项

- 所有查询在只读事务中执行
- 连接字符串中的凭据不会被日志记录
- 使用 `sqlx::AnyPool` 支持跨数据库兼容
