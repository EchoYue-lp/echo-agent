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

### 确定性后台 Cell Watch

`ShellTool` 以 `background=true` 启动后，消费者可以 retained 并观察 cell，不需要模型驱动的
轮询 Agent：

```rust,no_run
use echo_agent::tasks::{
    CommandCellWatchConfig, CommandCellWatcher,
};
use echo_agent::agent::CancellationToken;

# async fn observe(
#   registry: std::sync::Arc<dyn echo_agent::tools::cell::CommandCellRegistry>
# ) -> echo_agent::error::Result<()> {
let watcher = CommandCellWatcher::acquire(
    registry,
    "cell-id",
    CommandCellWatchConfig::default(),
)?;
let terminal = watcher
    .wait_terminal(&CancellationToken::new())
    .await?;
assert!(terminal.snapshot.phase.is_terminal());
# Ok(())
# }
```

watcher 持有 retention lease、复用返回的 byte cursor，并且只在 typed terminal 与当前可见 output
全部 drain 后退出。取消行为通过 `CommandCellWatchCancellation` 显式选择，绝不隐式停止底层命令。

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

### 后台启动的接纳边界

`CommandCellRequest.admission` 控制接纳前取消与预算；`cancel` 独立控制已接纳 cell 的生命周期。产品可通过 `resource_admission` 向原 Queued runner 提供异步资源政策，启动回执不等待产品槽。期限从 accepted_at 起算，包含排队；显式停止和 shutdown 沿原 supervisor 结算。保有 terminal observation lease 的消费者须在持久事实结算后释放 lease。详见 [ADR 0107](../adr/0107-command-cell-admission-and-save-work.md)。

### 可选原生资源准入

嵌入方可在最终 `ToolManager` 安装 `NativeExecutionResourceAdmission`，将自己的机器容量政策用于 Agent 托管原生命令；未配置时保持原行为。框架不硬编码容量值或产品角色。ToolContext 只传递 provider 引用，返回的资源 guard 只由本次命令的物理 owner 保有，不跨后续命令累积。

前台 shell、run_code 与技能脚本的解释器探测／实际执行在原校验成功后取得许可。前台准入耗时计入原 attempt 与该次命令预算；取消、期限或迟到准入不得启动进程。后台 shell 保持原 receipt／Queued runner，在 Queued 获取资源，不在 launch 外层重复取得。各入口零值、隔离下限、source executor 与 cwd 规则分别保持。

本地 direct 的取消、超时或调用者掉线由原 child owner 完成 kill／wait／drain。物理停止未证实时保有原 owner 和资源，直到观察到原 child／进程组实际结束；工具返回不是资源释放凭证。

新 provider 使用明确的后端能力合同。Local 的原 Unix owner 和 ProjectSSH 的原 remote-operation owner 兑现资源保有；路由器检查实际叶子及 fallback。未支持后端在 workload 创建前明确 Unavailable，原控制探测仍可运行。Docker／K8s 当前不声明支持新合同，原无新要求的行为保留。

Unix direct 的保有控制、未证明平台的 Some/Unsupported 与最终未启用 provider 的旧路径须分别验收。`register_descriptor_with_source`／定义时 Skill 来源不会被替换；显式 local(None) 或 custom executor 可能因有效新政策收到 Unsupported，迁移需要真实 owner 保有和对应 trait 声明。不能把输入 ToolContext.provider=None 当作应用最终 Some 政策的豁免。用户直接 PTY／MCP／控制 I/O 不因此新增 gate。

本地 `--version` 探测以零字节保留持续排空 stdout/stderr，避免先缓存全部输出再丢弃；实际 direct script 保留原输出语义和关闭 stdin 的行为。
