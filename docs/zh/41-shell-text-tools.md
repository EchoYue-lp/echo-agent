# Shell 与文本工具

## ShellTool — 安全的 Shell 命令执行

**Feature**: `shell` | **风险等级**: ShellExec (Level 3) | **权限**: Execute

### 安全架构（三层模型）

| 层级 | 行为 | 命令示例 |
|------|------|----------|
| ✅ 白名单 | 直接执行 | `ls`, `cat`, `head`, `tail`, `git`, `cargo`, `grep`, `echo`, `find`, `wc` |
| ⚠️ 审批队列 | 需人工确认 | `rm`, `curl`, `npm`, `pip`, `bash`, `python`, `sed`, `awk` |
| 🚫 黑名单 | 直接拒绝 | `dd`, `sudo`, `chmod`, `chown`, `reboot`, `shutdown`, `nmap` |

### 参数

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `command` | string | ✅ | 要执行的 shell 命令 |
| `timeout` | integer | ❌ | 超时秒数（默认 60s，上限 300s） |

### 超时机制

- **默认超时**: 60 秒
- **构造器配置**: `ShellTool::new().with_timeout(120)` — 设置默认超时
- **每次调用**: 通过 `timeout` 参数覆盖，硬上限 300 秒
- **实现**: `tokio::time::timeout` 异步超时，不阻塞线程
- **超时返回**: `⏱️ Command timeout after N seconds`

### Shell 注入防护

- 检测元字符: `| ; & $ \` > < ( ) \n`
- 使用 `shlex::split` 严格解析 argv
- 默认严格模式：不在白名单的命令一律拒绝
- 沙箱模式下可通过 `sh -c` 执行含元字符的命令

### 输出格式

```
成功: stdout 文本
失败: 错误信息（含 exit code、stdout、stderr）
超时: ⏱️ Command timeout after N seconds
```

---

## TextSearchTool — 文本文件搜索

**Feature**: `media` | **风险等级**: ReadOnly | **权限**: Read

### 参数

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_path` | string | ✅ | 文本文件绝对路径 |
| `pattern` | string | ✅ | 搜索模式（支持正则表达式） |
| `context` | integer | ❌ | 匹配行前后上下文行数（默认 0） |
| `ignore_case` | boolean | ❌ | 忽略大小写（默认 false） |

### 输出格式

```json
{
  "file": "/path/to/file.txt",
  "pattern": "error:\\d+",
  "match_count": 3,
  "truncated": false,
  "max_matches": 200,
  "matches": ["  123 | error: 404 not found"]
}
```

---

## TextStatsTool — 文本统计

**Feature**: `media` | **风险等级**: ReadOnly（隐式）| **权限**: Read

### 参数

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_path` | string | ✅ | 文本文件绝对路径 |

### 输出格式

```json
{
  "file": "/path/to/file.txt",
  "lines": 150,
  "chars": 3200,
  "words": 450,
  "chinese_chars": 120,
  "english_words": 330,
  "file_size_kb": 3.2,
  "avg_line_len": 21.3,
  "max_line_len": 120
}
```

---

## TextProcessTool — 文本处理

**Feature**: `media` | **权限**: Read

### 参数

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_path` | string | ✅ | 文本文件绝对路径 |
| `operation` | string | ✅ | 操作类型（见下表） |
| `count` | integer | ❌ | head/tail 操作的行数（默认 10） |

### 支持的操作

| 操作 | 说明 |
|------|------|
| `unique` | 去重（保留首次出现顺序） |
| `sort` | 字典序排序 |
| `reverse` | 行序反转 |
| `trim` | 删除空行 |
| `head` | 取前 N 行 |
| `tail` | 取后 N 行 |

### 输出格式

```json
{
  "file": "/path/to/file.txt",
  "operation": "unique",
  "original_lines": 100,
  "result_lines": 75,
  "preview": ["line1", "line2", "..."],
  "truncated": false
}
```

---

## TextExportTool — 文本导出

**Feature**: `media` | **权限**: Write

### 参数

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `input_file` | string | ✅ | 输入文本文件路径 |
| `output_file` | string | ✅ | 输出文件路径 |
| `operation` | string | ❌ | 可选处理: `unique`, `sort`, `trim` |

### 输出

```
Text exported: /path/input.txt -> /path/output.txt
```
