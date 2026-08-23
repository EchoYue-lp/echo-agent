# Git 隔离机制

## 概述

echo-agent 利用 git 原语为文件操作提供两层安全保障：

1. **Git Worktree 隔离** — 并行子代理在仓库的隔离副本上工作，避免文件冲突
2. **Git Checkpoint 检查点** — 在文件变更前自动创建轻量标签，支持即时回滚

两个功能均基于标准 `git` 命令构建，无需额外依赖。共同目标是：代理可以大胆修改文件，而不会造成不可逆的损坏。

---

## Git Worktree 隔离

**Feature gate**：`git`

```toml
[dependencies]
echo_agent = { version = "0.2", features = ["git"] }
```

### 用途

当多个子代理需要同时操作同一个仓库时，它们会竞争相同的文件。Worktree 通过为每个子代理分配独立的工作目录来解决这个问题——同时共享同一个 `.git` 对象库。与完整克隆相比，这种方式非常轻量。

```
主仓库 (branch: main)
├── .worktrees/
│   ├── feature-auth/     ← 子代理 A 的隔离工作空间
│   └── fix-typo/         ← 子代理 B 的隔离工作空间
├── src/
├── Cargo.toml
└── .git/                 ← 共享对象库
```

### Rust API

底层 API 位于 `echo_agent::tools::git_worktree`：

```rust
use echo_agent::tools::git_worktree::{
    create_worktree, remove_worktree, list_worktrees,
    merge_worktree, WorktreeConfig, ManagedWorktree,
};
use std::path::Path;

// 从 HEAD 创建一个新分支的 worktree
let config = WorktreeConfig {
    branch: "feature-auth".to_string(),
    base: None,                // 默认为 HEAD
    path_suffix: None,         // 从分支名自动推导
};
let worktree = create_worktree(Path::new("."), &config)?;
// worktree.path → /repo/.worktrees/feature-auth
// worktree.branch → "feature-auth"
// worktree.managed → true

// 从指定基础分支创建
let config = WorktreeConfig {
    branch: "hotfix".to_string(),
    base: Some("main".to_string()),
    path_suffix: Some("hotfix-dir".to_string()),  // 自定义目录名
};

// 列出所有 worktree
let all = list_worktrees(Path::new("."))?;
for wt in &all {
    println!("{} (branch: {})", wt.path.display(), wt.branch);
}

// 将 worktree 的变更合并回目标分支
merge_worktree(Path::new("."), &worktree, "main")?;

// 移除 worktree 并删除其分支
remove_worktree(Path::new("."), &worktree)?;
```

#### `WorktreeConfig`

| 字段 | 类型 | 描述 |
|------|------|------|
| `branch` | `String` | worktree 的分支名（不存在则创建） |
| `base` | `Option<String>` | 创建的基础分支或 commit（默认为 HEAD） |
| `path_suffix` | `Option<String>` | `.worktrees/` 下的自定义目录名（默认从分支名推导） |

#### `ManagedWorktree`

| 字段 | 类型 | 描述 |
|------|------|------|
| `path` | `PathBuf` | worktree 目录路径 |
| `branch` | `String` | 分支名 |
| `managed` | `bool` | 是否由框架创建（用于清理追踪） |

### 代理工具

三个工具将 worktree 管理暴露给代理。启用 `git` feature 后自动注册。

#### `enter_worktree`

创建一个新的 git worktree，用于隔离的并行工作。

| 参数 | 必填 | 描述 |
|------|------|------|
| `branch` | 是 | 新 worktree 的分支名（不存在则创建） |
| `base` | 否 | 创建的基础分支或 commit（默认 HEAD） |
| `path_suffix` | 否 | `.worktrees/` 下的自定义目录名 |
| `repo_path` | 否 | 仓库路径（默认当前工作目录） |

返回 worktree 路径和分支名。调用者应将该目录作为子代理的工作根目录。

```
Agent: 我来为 auth 重构创建隔离工作空间。
→ enter_worktree(branch="feature-auth", base="main")
→ Returns: "Created worktree at '.worktrees/feature-auth' on branch 'feature-auth'."
```

#### `exit_worktree`

移除托管的 worktree，可选择在清理前合并其变更。

| 参数 | 必填 | 描述 |
|------|------|------|
| `worktree_path` | 是 | 要移除的 worktree 目录路径 |
| `merge_to` | 否 | 如设置，在移除前将 worktree 分支合并到此目标分支 |
| `repo_path` | 否 | 仓库路径（默认当前工作目录） |

风险等级：**Dangerous** — 可能删除分支并通过 merge 修改主工作树。

```
Agent: auth 工作完成，合并到 main 并清理。
→ exit_worktree(worktree_path=".worktrees/feature-auth", merge_to="main")
→ Returns: "Merged feature-auth into main. Worktree at '.worktrees/feature-auth' removed."
```

#### `list_worktrees`

列出仓库中的所有 git worktree。

| 参数 | 必填 | 描述 |
|------|------|------|
| `repo_path` | 否 | 仓库路径（默认当前工作目录） |

风险等级：**ReadOnly** — 无副作用。

```
Agent: 让我看看有哪些活跃的工作空间。
→ list_worktrees()
→ Returns: "Worktrees (3):\n  /repo (branch: main)\n  /repo/.worktrees/feature-auth (branch: feature-auth)\n  /repo/.worktrees/fix-typo (branch: fix-typo)"
```

### Worktree 目录布局

Worktree 创建在 `<repo>/.worktrees/` 下：

```
<repo>/
├── .worktrees/
│   ├── feature-auth/        ← branch: feature-auth
│   │   ├── src/
│   │   └── Cargo.toml
│   └── fix_typo/            ← branch: fix-typo（非安全字符被替换为 _）
│       ├── src/
│       └── Cargo.toml
```

分支名会进行目录安全处理：任何非字母数字、非 `-`、非 `_` 的字符都会被替换为 `_`。指定 `path_suffix` 可跳过此处理。

---

## Git Checkpoint 检查点

**始终可用** — `git_checkpoint` 不在 feature gate 之后。

### 自动安全网

Git Checkpoint 在文件变更前创建轻量级 git 标签，捕获该时刻仓库的精确状态。如果工具调用产生了不期望的变更，代理（或用户）可以回滚到检查点。

集成是透明的：文件修改工具（`write_file`、`delete_file`）在修改文件之前内部调用 `create_checkpoint()`。

### 核心 API

```rust
use echo_agent::tools::git_checkpoint::{
    create_checkpoint,
    rollback_to_checkpoint,
    cleanup_old_checkpoints,
};
use std::path::Path;

// 在变更前创建检查点
let tag: Option<String> = create_checkpoint(Path::new("src/main.rs"));
// tag → Some("echo-checkpoint/1748864400")

// ... 文件变更发生 ...

// 如果出了问题，回滚
if need_rollback {
    let ok = rollback_to_checkpoint(
        Path::new("src/main.rs"),
        "echo-checkpoint/1748864400",
    );
    // ok → true
}

// 定期清理旧检查点（保留最近 10 个）
cleanup_old_checkpoints(Path::new("src/main.rs"), 10);
```

#### `create_checkpoint(file_path: &Path) -> Option<String>`

在当前 HEAD 上创建轻量级 git 标签（`echo-checkpoint/<unix_timestamp>`）。

- 成功时返回 `Some(tag_name)`
- 如果路径不在 git 仓库内，返回 `None`（优雅降级——非 git 环境不算错误）

#### `rollback_to_checkpoint(file_path: &Path, tag_name: &str) -> bool`

将工作树恢复到检查点标签捕获的状态。

- 使用 `git checkout <tag> -- .` 将所有文件恢复到检查点状态
- 成功返回 `true`，失败或不在 git 仓库中返回 `false`

#### `cleanup_old_checkpoints(file_path: &Path, keep: usize)`

删除旧的检查点标签，仅保留最近的 `keep` 个。

- 列出匹配 `echo-checkpoint/*` 的标签，按创建日期降序排列
- 删除超出 `keep` 阈值的所有标签

### 与文件工具的集成

检查点创建自动编织进文件工具中：

#### `write_file`

```rust
// WriteFileTool::execute() 内部 — 简化版
let checkpoint_tag = if path.exists() {
    // 仅在覆盖已有文件时创建检查点
    let tag = crate::git_checkpoint::create_checkpoint(&path);
    if tag.is_some() {
        crate::git_checkpoint::cleanup_old_checkpoints(&path, 10);
    }
    tag
} else {
    None  // 新文件 — 无需检查点
};

// ... 执行写入 ...

// 检查点标签作为工具结果的元数据返回
if let Some(tag) = checkpoint_tag {
    result = result.with_meta("git_checkpoint", tag);
}
```

关键行为：
- 仅在**覆盖已有文件**时创建检查点
- 创建新文件不触发检查点（没有可回滚的内容）
- 自动清理旧检查点，最多保留 10 个标签
- 标签名作为 `git_checkpoint` 元数据返回在工具结果中

#### `delete_file`

```rust
// DeleteFileTool::execute() 内部 — 简化版
let checkpoint_tag = crate::git_checkpoint::create_checkpoint(&path);
if checkpoint_tag.is_some() {
    crate::git_checkpoint::cleanup_old_checkpoints(&path, 10);
}

// ... 执行删除 ...

if let Some(tag) = checkpoint_tag {
    result = result.with_meta("git_checkpoint", tag);
}
```

关键行为：
- 删除前始终创建检查点（文件的当前状态值得保留）
- 标签名作为工具结果的元数据返回

### 标签命名约定

检查点标签遵循 `echo-checkpoint/<unix_timestamp>` 格式：

```
echo-checkpoint/1748864400
echo-checkpoint/1748864450
echo-checkpoint/1748864500
```

这确保：
- 标签按时间排序
- 不会与用户创建的标签冲突
- 可通过 `git tag -l "echo-checkpoint/*"` 轻松过滤
- 清理时可使用 `--sort=-creatordate` 获得可靠排序

### 优雅降级

所有检查点函数在非 git 环境中静默处理：

- `create_checkpoint()` — 不在 git 仓库中时返回 `None`
- `rollback_to_checkpoint()` — 不在 git 仓库中时返回 `false`
- `cleanup_old_checkpoints()` — 不在 git 仓库中时为空操作
- 文件工具在检查点创建失败时不会失败——写入照常进行

---

## 架构图

```
┌─────────────────────────────────────────────────────────────────┐
│                      Agent ReAct 循环                            │
│                                                                  │
│   ┌─────────────────┐        ┌─────────────────────────────┐    │
│   │  子代理 A        │        │  子代理 B                    │    │
│   │  enter_worktree │        │  enter_worktree              │    │
│   │  branch: feat-x │        │  branch: fix-y               │    │
│   └────────┬────────┘        └────────┬─────────────────────┘    │
│            │                          │                          │
│            ▼                          ▼                          │
│   ┌─────────────────┐      ┌──────────────────┐                 │
│   │ .worktrees/     │      │ .worktrees/       │                 │
│   │ feat-x/         │      │ fix-y/            │                 │
│   │                 │      │                   │                 │
│   │ write_file() ───┤──────┤── write_file()    │                 │
│   │     │           │      │       │           │                 │
│   │     ▼           │      │       ▼           │                 │
│   │ checkpoint ─────┤──────┤── checkpoint      │                 │
│   │ (git tag)       │      │  (git tag)        │                 │
│   └────────┬────────┘      └────────┬──────────┘                 │
│            │                         │                           │
│            ▼                         ▼                           │
│   ┌─────────────────────────────────────────────┐                │
│   │            共享 .git 对象库                   │                │
│   │  ┌───────────────────────────────────────┐   │               │
│   │  │ echo-checkpoint/1748864400            │   │               │
│   │  │ echo-checkpoint/1748864450            │   │               │
│   │  │ refs/heads/main                       │   │               │
│   │  │ refs/heads/feat-x                     │   │               │
│   │  │ refs/heads/fix-y                      │   │               │
│   │  └───────────────────────────────────────┘   │               │
│   └─────────────────────────────────────────────┘                │
└─────────────────────────────────────────────────────────────────┘
```

---

## 组合使用 Worktree 与 Checkpoint

两个系统可以自然组合。典型的多代理工作流：

1. **进入 worktree** — 子代理通过 `enter_worktree` 创建隔离工作空间
2. **带检查点保护工作** — 在 worktree 内，每次 `write_file` / `delete_file` 调用自动创建检查点
3. **需要时回滚** — 如果文件编辑出错，使用 `rollback_to_checkpoint()` 恢复 worktree 内的状态
4. **退出并合并** — 完成后，`exit_worktree(merge_to="main")` 合并分支并清理

```rust
// 编排示例
use echo_agent::tools::git_worktree::{create_worktree, remove_worktree, merge_worktree, WorktreeConfig};
use echo_agent::tools::git_checkpoint::{create_checkpoint, rollback_to_checkpoint, cleanup_old_checkpoints};

// 1. 创建隔离工作空间
let worktree = create_worktree(&repo_path, &WorktreeConfig {
    branch: "feature-auth".to_string(),
    base: Some("main".to_string()),
    path_suffix: None,
})?;

// 2. 子代理在 worktree.path 中工作
//    （write_file/delete_file 自动创建检查点）

// 3. 如果出了问题，回滚
rollback_to_checkpoint(&worktree.path, "echo-checkpoint/1748864400");

// 4. 合并并清理
merge_worktree(&repo_path, &worktree, "main")?;
remove_worktree(&repo_path, &worktree)?;

// 5. 清理旧的检查点标签
cleanup_old_checkpoints(&repo_path, 10);
```
