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
| `connection_url` | string | ✅ | 数据库连接字符串 |
| `table_name` | string | ✅ | 表名 |

### 输出格式

输出保留各数据库 schema 查询的原始列名，每一行是按列顺序排列的数组。例如 SQLite
对 `records(id INTEGER PRIMARY KEY, label TEXT NOT NULL, quantity INTEGER DEFAULT 7, note TEXT)`
执行 `PRAGMA table_info` 后返回：

```json
{
  "columns": ["cid", "name", "type", "notnull", "dflt_value", "pk"],
  "rows": [
    [0, "id", "INTEGER", 0, null, 1],
    [1, "label", "TEXT", 1, null, 0],
    [2, "quantity", "INTEGER", 0, "7", 0],
    [3, "note", "TEXT", 0, null, 0]
  ],
  "total_rows": 4
}
```

数据库工具按单元格实际的 SQLx 类型和空值状态解码，包括列声明类型未知的 SQLite
表达式和 PRAGMA 结果。SQL NULL 转为 JSON `null`，整数、有限浮点数和 UTF-8 文本保留
原值，BLOB 使用 `{"type":"binary","encoding":"base64","data":"..."}`。
解码失败或非有限浮点数返回明确错误，不填充 `null`。表结构查询与 SQL 查询共用框架
已有转换器，SDK 客户端调用该转换器，不另设解码权威。

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
