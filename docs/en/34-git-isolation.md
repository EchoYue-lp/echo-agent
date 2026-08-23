# Git Isolation

## Overview

echo-agent uses git primitives to provide two layers of safety for file operations:

1. **Git Worktree Isolation** — parallel subagents work on isolated copies of the repository, avoiding file conflicts
2. **Git Checkpoint** — lightweight tags are automatically created before file mutations, enabling instant rollback

Both features are built on standard `git` commands, requiring no additional dependencies. They share a common goal: agents should be able to mutate files aggressively without risking irreversible damage.

---

## Git Worktree Isolation

**Feature gate**: `git`

```toml
[dependencies]
echo_agent = { version = "0.2", features = ["git"] }
```

### Purpose

When multiple subagents need to work on the same repository simultaneously, they compete for the same files. Worktrees solve this by giving each subagent its own working directory — while sharing the same `.git` object store. This makes them lightweight compared to full clones.

```
Main repo (branch: main)
├── .worktrees/
│   ├── feature-auth/     ← subagent A's isolated workspace
│   └── fix-typo/         ← subagent B's isolated workspace
├── src/
├── Cargo.toml
└── .git/                 ← shared object store
```

### Rust API

The low-level API lives in `echo_agent::tools::git_worktree`:

```rust
use echo_agent::tools::git_worktree::{
    create_worktree, remove_worktree, list_worktrees,
    merge_worktree, WorktreeConfig, ManagedWorktree,
};
use std::path::Path;

// Create a worktree on a new branch from HEAD
let config = WorktreeConfig {
    branch: "feature-auth".to_string(),
    base: None,                // defaults to HEAD
    path_suffix: None,         // auto-derived from branch name
};
let worktree = create_worktree(Path::new("."), &config)?;
// worktree.path → /repo/.worktrees/feature-auth
// worktree.branch → "feature-auth"
// worktree.managed → true

// Create from a specific base branch
let config = WorktreeConfig {
    branch: "hotfix".to_string(),
    base: Some("main".to_string()),
    path_suffix: Some("hotfix-dir".to_string()),  // custom directory name
};

// List all worktrees
let all = list_worktrees(Path::new("."))?;
for wt in &all {
    println!("{} (branch: {})", wt.path.display(), wt.branch);
}

// Merge worktree changes back to a target branch
merge_worktree(Path::new("."), &worktree, "main")?;

// Remove worktree and delete its branch
remove_worktree(Path::new("."), &worktree)?;
```

#### `WorktreeConfig`

| Field | Type | Description |
|-------|------|-------------|
| `branch` | `String` | Branch name for the worktree (created if it does not exist) |
| `base` | `Option<String>` | Base branch or commit to create from (defaults to HEAD) |
| `path_suffix` | `Option<String>` | Custom directory name under `.worktrees/` (defaults to sanitized branch name) |

#### `ManagedWorktree`

| Field | Type | Description |
|-------|------|-------------|
| `path` | `PathBuf` | Path to the worktree directory |
| `branch` | `String` | Branch name |
| `managed` | `bool` | Whether the worktree was created by the framework (used for cleanup tracking) |

### Agent Tools

Three tools expose worktree management to agents. They are registered when the `git` feature is enabled.

#### `enter_worktree`

Creates a new git worktree for isolated parallel work.

| Parameter | Required | Description |
|-----------|----------|-------------|
| `branch` | Yes | Branch name for the new worktree (created if it does not exist) |
| `base` | No | Base branch or commit to create from (defaults to HEAD) |
| `path_suffix` | No | Custom directory name under `.worktrees/` |
| `repo_path` | No | Repository path (defaults to current working directory) |

Returns the worktree path and branch name. The caller should use this directory as the working root for the subagent.

```
Agent: I'll create an isolated workspace for the auth refactor.
→ enter_worktree(branch="feature-auth", base="main")
→ Returns: "Created worktree at '.worktrees/feature-auth' on branch 'feature-auth'."
```

#### `exit_worktree`

Removes a managed worktree, optionally merging its changes before cleanup.

| Parameter | Required | Description |
|-----------|----------|-------------|
| `worktree_path` | Yes | Path to the worktree directory to remove |
| `merge_to` | No | If set, merge the worktree branch into this target branch before removal |
| `repo_path` | No | Repository path (defaults to current working directory) |

Risk level: **Dangerous** — may delete branches and modify the main working tree via merge.

```
Agent: Done with the auth work. Merging into main and cleaning up.
→ exit_worktree(worktree_path=".worktrees/feature-auth", merge_to="main")
→ Returns: "Merged feature-auth into main. Worktree at '.worktrees/feature-auth' removed."
```

#### `list_worktrees`

Lists all git worktrees in the repository.

| Parameter | Required | Description |
|-----------|----------|-------------|
| `repo_path` | No | Repository path (defaults to current working directory) |

Risk level: **ReadOnly** — no side effects.

```
Agent: Let me check what workspaces are active.
→ list_worktrees()
→ Returns: "Worktrees (3):\n  /repo (branch: main)\n  /repo/.worktrees/feature-auth (branch: feature-auth)\n  /repo/.worktrees/fix-typo (branch: fix-typo)"
```

### Worktree Directory Layout

Worktrees are created under `<repo>/.worktrees/`:

```
<repo>/
├── .worktrees/
│   ├── feature-auth/        ← branch: feature-auth
│   │   ├── src/
│   │   └── Cargo.toml
│   └── fix_typo/            ← branch: fix-typo (non-alphanumeric chars sanitized to _)
│       ├── src/
│       └── Cargo.toml
```

Branch names are sanitized for directory safety: any character that is not alphanumeric, `-`, or `_` is replaced with `_`. A custom `path_suffix` bypasses this sanitization.

---

## Git Checkpoint

**Always available** — `git_checkpoint` is not behind a feature gate.

### Automatic Safety Net

Git Checkpoint creates lightweight git tags before file mutations, capturing the exact state of the repository at that moment. If a tool call produces unwanted changes, the agent (or user) can roll back to the checkpoint.

The integration is transparent: file-mutating tools (`write_file`, `delete_file`) call `create_checkpoint()` internally before modifying any file.

### Core API

```rust
use echo_agent::tools::git_checkpoint::{
    create_checkpoint,
    rollback_to_checkpoint,
    cleanup_old_checkpoints,
};
use std::path::Path;

// Create a checkpoint before a mutation
let tag: Option<String> = create_checkpoint(Path::new("src/main.rs"));
// tag → Some("echo-checkpoint/1748864400")

// ... file mutation happens ...

// If something went wrong, roll back
if need_rollback {
    let ok = rollback_to_checkpoint(
        Path::new("src/main.rs"),
        "echo-checkpoint/1748864400",
    );
    // ok → true
}

// Periodically clean up old checkpoints (keep the most recent 10)
cleanup_old_checkpoints(Path::new("src/main.rs"), 10);
```

#### `create_checkpoint(file_path: &Path) -> Option<String>`

Creates a lightweight git tag (`echo-checkpoint/<unix_timestamp>`) on the current HEAD.

- Returns `Some(tag_name)` on success
- Returns `None` if the path is not inside a git repository (graceful degradation — non-git environments are not an error)

#### `rollback_to_checkpoint(file_path: &Path, tag_name: &str) -> bool`

Restores the working tree to the state captured by the checkpoint tag.

- Uses `git checkout <tag> -- .` to restore all files to the checkpoint state
- Returns `true` on success, `false` on failure or if not in a git repo

#### `cleanup_old_checkpoints(file_path: &Path, keep: usize)`

Deletes old checkpoint tags, keeping only the most recent `keep` entries.

- Lists tags matching `echo-checkpoint/*` sorted by creation date (newest first)
- Deletes all tags beyond the `keep` threshold

### Integration with File Tools

Checkpoint creation is woven into the file tools automatically:

#### `write_file`

```rust
// Inside WriteFileTool::execute() — simplified
let checkpoint_tag = if path.exists() {
    // Only checkpoint if overwriting an existing file
    let tag = crate::git_checkpoint::create_checkpoint(&path);
    if tag.is_some() {
        crate::git_checkpoint::cleanup_old_checkpoints(&path, 10);
    }
    tag
} else {
    None  // New file — nothing to checkpoint
};

// ... perform the write ...

// The checkpoint tag is returned in the tool result metadata
if let Some(tag) = checkpoint_tag {
    result = result.with_meta("git_checkpoint", tag);
}
```

Key behavior:
- Checkpoints are created **only when overwriting** an existing file
- New file creation does not trigger a checkpoint (nothing to roll back to)
- Old checkpoints are cleaned up to keep at most 10 tags
- The tag name is returned in the tool result as `git_checkpoint` metadata

#### `delete_file`

```rust
// Inside DeleteFileTool::execute() — simplified
let checkpoint_tag = crate::git_checkpoint::create_checkpoint(&path);
if checkpoint_tag.is_some() {
    crate::git_checkpoint::cleanup_old_checkpoints(&path, 10);
}

// ... perform the deletion ...

if let Some(tag) = checkpoint_tag {
    result = result.with_meta("git_checkpoint", tag);
}
```

Key behavior:
- A checkpoint is always created before deletion (the file's current state is worth preserving)
- The tag name is returned in the tool result metadata

### Tag Naming Convention

Checkpoint tags follow the pattern `echo-checkpoint/<unix_timestamp>`:

```
echo-checkpoint/1748864400
echo-checkpoint/1748864450
echo-checkpoint/1748864500
```

This ensures:
- Tags are chronologically sortable
- They never collide with user-created tags
- They are easy to filter with `git tag -l "echo-checkpoint/*"`
- Cleanup can use `--sort=-creatordate` for reliable ordering

### Graceful Degradation

All checkpoint functions handle non-git environments silently:

- `create_checkpoint()` returns `None` if not in a git repository
- `rollback_to_checkpoint()` returns `false` if not in a git repository
- `cleanup_old_checkpoints()` is a no-op if not in a git repository
- File tools do not fail if checkpoint creation fails — the write proceeds normally

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                      Agent ReAct Loop                            │
│                                                                  │
│   ┌─────────────────┐        ┌─────────────────────────────┐    │
│   │  Sub-agent A    │        │  Sub-agent B                 │    │
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
│   │            Shared .git object store           │               │
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

## Combining Worktrees and Checkpoints

The two systems compose naturally. A typical multi-agent workflow:

1. **Enter worktree** — subagent creates an isolated workspace via `enter_worktree`
2. **Work with checkpoint safety** — inside the worktree, every `write_file` / `delete_file` call automatically creates checkpoints
3. **Rollback if needed** — if a file edit goes wrong, use `rollback_to_checkpoint()` to restore state within the worktree
4. **Exit and merge** — when done, `exit_worktree(merge_to="main")` merges the branch and cleans up

```rust
// Orchestration example
use echo_agent::tools::git_worktree::{create_worktree, remove_worktree, merge_worktree, WorktreeConfig};
use echo_agent::tools::git_checkpoint::{create_checkpoint, rollback_to_checkpoint, cleanup_old_checkpoints};

// 1. Create isolated workspace
let worktree = create_worktree(&repo_path, &WorktreeConfig {
    branch: "feature-auth".to_string(),
    base: Some("main".to_string()),
    path_suffix: None,
})?;

// 2. Sub-agent works in worktree.path
//    (write_file/delete_file auto-create checkpoints)

// 3. If something went wrong, roll back
rollback_to_checkpoint(&worktree.path, "echo-checkpoint/1748864400");

// 4. Merge and clean up
merge_worktree(&repo_path, &worktree, "main")?;
remove_worktree(&repo_path, &worktree)?;

// 5. Clean up old checkpoint tags
cleanup_old_checkpoints(&repo_path, 10);
```
