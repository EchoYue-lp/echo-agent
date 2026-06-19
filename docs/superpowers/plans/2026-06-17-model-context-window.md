# 模型上下文窗口（context_window）实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 新增 `context_window: Option<u32>` 字段，全链路支持用户自定义模型上下文窗口，影响压缩阈值、TokenBudget 和压缩调优。

**Architecture:** 自底向上三层推进——先改核心框架 `echo-agent`（数据结构 + 推断逻辑 + 消费点），再改应用层 `echo-agent-cli`（透传 + API），最后改前端 UI。每层改完立即验证。

**Tech Stack:** Rust + TypeScript/React

**Worktree 路径:**
- echo-agent: `/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent/.worktrees/feature/context-window-config`
- echo-agent-cli: `/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent-cli/.worktrees/feature/context-window-config`

> ⚠️ echo-agent-cli 的 3 个 Cargo.toml 已临时改为绝对路径引用 echo-agent worktree，**不要提交这些路径变更**。

---

## 文件变更映射

| 文件 | 操作 |
|------|------|
| `echo-agent/echo-core/src/llm/capabilities.rs` | 删除字段 + 提取函数 |
| `echo-agent/src/config.rs` | 新增字段 + 修改转换 + 修改消费 |
| `echo-agent/echo-state/src/compression/levels.rs` | 参数重命名 |
| `echo-agent-cli/echo-agent-app-core/src/model_config.rs` | 新增字段 + 透传 |
| `echo-agent-cli/src/tauri/commands/providers.rs` | 新增字段 |
| `echo-agent-cli/web-frontend/src/types/api.ts` | 新增类型字段 |
| `echo-agent-cli/web-frontend/src/api/endpoints.ts` | 新增 API 参数 |
| `echo-agent-cli/web-frontend/src/components/providers/ProviderPanel.tsx` | UI 改造 |

---

### Task 1: 提取 `infer_context_window()` + 删除 `max_context_tokens`

**Files:**
- Modify: `echo-agent/echo-core/src/llm/capabilities.rs` (完整文件)

- [ ] **Step 1: 删除 `ProviderCapabilities::max_context_tokens` 字段**

在 `echo-agent/echo-core/src/llm/capabilities.rs` 中：

删除第 59-61 行：
```rust
    /// Known maximum context window in tokens for this provider's default model.
    /// `None` means unknown.
    pub max_context_tokens: Option<u32>,
```

删除第 85 行 `max_context_tokens: None, // varies by model; see ModelProfile`（在 `openai_compatible()` 中）

删除第 103 行 `max_context_tokens: Some(200_000), // Claude 3.x+`（在 `anthropic()` 中）

删除第 121 行 `max_context_tokens: None, // varies by model`（在 `ollama()` 中）

- [ ] **Step 2: 删除 `ModelProfile::max_context_tokens` 字段**

删除第 170-171 行：
```rust
    /// Known maximum context window in tokens (None if unknown).
    pub max_context_tokens: Option<u32>,
```

删除第 248 行 `max_context_tokens,`（在 `Self { ... }` 构造中）

- [ ] **Step 3: 提取 `infer_context_window()` 函数**

在 `impl ModelProfile` 之前（第 136 行附近），新增独立函数：

```rust
/// 根据厂商和模型名称推断上下文窗口大小。
/// 未匹配到已知模式时返回 None。
pub fn infer_context_window(_provider: &str, model_name: &str) -> Option<u32> {
    let lower = model_name.to_ascii_lowercase();
    if lower.contains("qwen3-235b") {
        Some(131_072)
    } else if lower.starts_with("gpt-5.5") || lower.starts_with("gpt-4.5") {
        Some(128_000)
    } else if lower.starts_with("gpt-4") && !lower.starts_with("gpt-5.5") {
        Some(8_192)
    } else if lower.starts_with("claude-3-opus") {
        Some(200_000)
    } else if lower.starts_with("claude-3.5") || lower.starts_with("claude-4") {
        Some(200_000)
    } else if lower.starts_with("claude-") {
        Some(200_000)
    } else if lower.starts_with("deepseek-") {
        Some(128_000)
    } else if lower.starts_with("qwen-") {
        Some(131_072)
    } else {
        None
    }
}
```

- [ ] **Step 4: 修改 `ModelProfile::new()` 使用新函数**

将第 197-216 行的 `max_context_tokens` 局部变量逻辑替换为调用 `infer_context_window()`：

```rust
        // 已知最大上下文窗口（token 数）
        let max_context_tokens = infer_context_window(provider, model_name)
            .or(capabilities.max_context_tokens);
```

等等——`capabilities.max_context_tokens` 已删除。直接改为：

```rust
        // 已知最大上下文窗口（token 数）
        let max_context_tokens = infer_context_window(provider, model_name);
```

但还需要保留它用于构造 `Self { ... max_context_tokens, ... }`——等等，我们也要删掉 Self 中的 max_context_tokens 字段。所以第 238-250 行的 Self 构造中也要移除 `max_context_tokens,`。

等等，`ModelProfile` 的 `max_context_tokens` 字段已经删除，所以 Self 构造中不需要它了。

完整修改 `ModelProfile::new()`：删除 max_context_tokens 局部变量计算（第 197-216 行），Self 构造中移除 `max_context_tokens,`（第 248 行）。

- [ ] **Step 5: 编译验证**

```bash
cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent/.worktrees/feature/context-window-config
cargo check 2>&1
```

Expected: 编译通过。如果有 `max_context_tokens` 的引用报错，逐一修复。

> 注意：`echo-state/src/compression/levels.rs` 中的 doc comment 引用 `max_context_tokens`（第 90 行），需要更新注释；`src/config.rs` 中的 `caps.max_context_tokens` 引用也需要处理——这些在后续 Task 中处理，此处先让 check 通过即可（可能因 dead_code 等产生警告，后续 Task 一并清理）。

- [ ] **Step 6: 提交**

```bash
cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent/.worktrees/feature/context-window-config
git add echo-core/src/llm/capabilities.rs
git -c commit.gpgsign=false commit -m "refactor(capabilities): extract infer_context_window(), remove max_context_tokens fields"
```

---

### Task 2: 添加 `context_window` 到 `ConfiguredModel` 和 `ModelConfig`

**Files:**
- Modify: `echo-agent/src/config.rs`

- [ ] **Step 1: `ConfiguredModel` 新增字段**

在 `echo-agent/src/config.rs` 的 `ConfiguredModel` 结构体中，`temperature` 字段之后添加（约第 275 行之后）：

```rust
    /// Optional model context window size in tokens.
    /// When set, overrides the auto-detected value for compression threshold,
    /// TokenBudget allocation, and adaptive compression tuning.
    /// When None, falls back to name-based inference.
    pub context_window: Option<u32>,
```

同时更新 `Default` impl（约第 285 行）：
```rust
            context_window: None,
```

- [ ] **Step 2: `ModelConfig` 新增字段**

在 `ModelConfig` 结构体中，`temperature` 字段之后添加（约第 197 行之后）：

```rust
    /// Optional model context window size in tokens.
    /// When None, falls back to name-based inference.
    pub context_window: Option<u32>,
```

更新 `Default` impl：
```rust
            context_window: None,
```

- [ ] **Step 3: 编译验证**

```bash
cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent/.worktrees/feature/context-window-config
cargo check 2>&1
```

Expected: 编译通过。

- [ ] **Step 4: 提交**

```bash
git add src/config.rs
git -c commit.gpgsign=false commit -m "feat: add context_window field to ConfiguredModel and ModelConfig"
```

---

### Task 3: `to_agent_config()` 中使用 `context_window` 覆盖 `token_limit` 和 `total_window`

**Files:**
- Modify: `echo-agent/src/config.rs` (to_agent_config 方法 + 新增 resolve 辅助函数)
- Modify: `echo-agent/src/agent/config.rs` (如有需要暴露 setter)

- [ ] **Step 1: 新增 `resolve_context_window` 解析函数**

在 `echo-agent/src/config.rs` 中，`impl AppConfig` 之前（或作为私有函数），添加：

```rust
/// 解析最终的上下文窗口值。
/// 优先级：用户显式设置 > 名称模式推断 > 默认 128K
fn resolve_context_window(
    explicit: Option<u32>,
    provider: &str,
    model_name: &str,
) -> usize {
    explicit
        .or_else(|| echo_core::llm::capabilities::infer_context_window(provider, model_name))
        .unwrap_or(128_000)
        .clamp(1, 10_000_000) as usize
}
```

如 `echo_core::llm::capabilities` 模块路径不同，按实际情况调整。检查 `echo-core/src/llm/mod.rs` 是否 re-export `infer_context_window`；如果没有，需要添加 re-export。

- [ ] **Step 2: 修改 `to_agent_config()`**

将 `to_agent_config()` 中的 `token_limit` 解析逻辑（第 81-85 行）替换为使用 `context_window` 解析：

```rust
    pub fn to_agent_config(&self) -> AgentConfig {
        let context_window = resolve_context_window(
            self.model.context_window,
            &self.model.provider,
            &self.model.name,
        );
        // YAML 中显式设置 token_limit 时，以 YAML 为准（覆盖 context_window）
        // 否则使用解析后的 context_window
        let token_limit = if self.agent.token_limit > 0 {
            self.agent.token_limit
        } else {
            context_window
        };
        let mut token_budget_config = TokenBudgetConfig::default();
        token_budget_config.total_window = context_window;
        
        AgentConfig::standard(
            &self.model.name,
            &self.agent.name,
            &self.agent.system_prompt,
        )
        .enable_tool(self.agent.enable_tools)
        .enable_memory(self.agent.enable_memory)
        .enable_human_in_loop(self.agent.enable_human_in_loop)
        .max_iterations(self.agent.max_iterations)
        .memory_path(&self.agent.memory_path)
        .temperature(self.model.temperature)
        .max_tokens(self.model.max_tokens)
        .token_limit(token_limit)
        .token_budget_config(token_budget_config)
        .tool_execution(crate::tools::ToolExecutionConfig {
            timeout_ms: self.agent.tool_timeout_ms,
            ..Default::default()
        })
    }
```

需要引入 `TokenBudgetConfig`：在 `config.rs` 顶部已有 `use echo_core::budget::TokenBudgetConfig;` — 检查确认；如果没有，添加：
```rust
use echo_core::budget::TokenBudgetConfig;
```

检查 `AgentConfig` builder 是否有 `.token_budget_config()` 方法。如果没有，需要在 `echo-agent/src/agent/config.rs` 中添加：

```rust
    pub fn token_budget_config(mut self, config: TokenBudgetConfig) -> Self {
        self.token_budget_config = config;
        self
    }
```

- [ ] **Step 3: 编译验证**

```bash
cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent/.worktrees/feature/context-window-config
cargo check 2>&1
```

Expected: 编译通过。如有缺失的 import 或方法，按错误提示修复。

- [ ] **Step 4: 运行测试**

```bash
cargo test 2>&1
```

检查 `test_to_agent_config` 是否通过。由于 `token_limit` 处理逻辑变化（`self.agent.token_limit` 为 0 时现在使用 `context_window` 解析值而非 `usize::MAX`），需更新该测试的期望值。

- [ ] **Step 5: 提交**

```bash
git add src/config.rs src/agent/config.rs
git -c commit.gpgsign=false commit -m "feat: resolve context_window in to_agent_config, override token_limit and TokenBudget"
```

---

### Task 4: 更新 `apply_compressor()` 消费逻辑

**Files:**
- Modify: `echo-agent/src/config.rs` (apply_compressor 方法)
- Modify: `echo-agent/echo-state/src/compression/levels.rs` (tune_for_model 参数重命名)
- Modify: `echo-agent/src/agent/react/mod.rs` (新增 token_limit 访问器，如果需要)

- [ ] **Step 1: 修改 `tune_for_model` 参数重命名**

在 `echo-state/src/compression/levels.rs` 第 107-108 行：

```rust
pub fn tune_for_model(config: &mut AdaptiveCompressionConfig, context_window: usize) {
    let w = context_window;
```

同时更新第 90 行的 doc comment：
```rust
/// Thresholds are set as percentages of the context window:
```

- [ ] **Step 2: 更新 `apply_compressor` — 删除 `caps.max_context_tokens` 引用**

在 `echo-agent/src/config.rs` 的 `apply_compressor()` 方法中，将 `caps.max_context_tokens` 的引用全部替换为使用 `self.agent.token_limit`（它已经由 `to_agent_config` 中的 `context_window` 解析而来）。

**修改 "sliding" 分支的警告逻辑（第 123-136 行）**：

删除整个 `caps.max_context_tokens` 警告块，因为 `token_limit` 现在本身就是 `context_window` 解析值，不存在"超过模型窗口"的问题：

```rust
            "sliding" | "" => {
                agent
                    .set_compressor(SlidingWindowCompressor::new(window))
                    .await;
            }
```

**修改 "adaptive" 分支的调优逻辑（第 141-157 行）**：

将 `caps.max_context_tokens` 替换为 `self.agent.token_limit`（它已在 `to_agent_config` 中被 `context_window` 解析）：

```rust
            "adaptive" => {
                use crate::compression::levels::{
                    AdaptiveCompressionConfig, AdaptiveCompressor, tune_for_model,
                };
                let mut config = AdaptiveCompressionConfig::default();
                // Auto-tune thresholds from token_limit (resolved from context_window)
                if self.agent.token_limit > 0 && self.agent.token_limit < usize::MAX {
                    tune_for_model(&mut config, self.agent.token_limit);
                    tracing::info!(
                        token_limit = self.agent.token_limit,
                        "Tuned adaptive compression from context window"
                    );
                }
                agent.set_compressor(AdaptiveCompressor::new(config)).await;
            }
```

- [ ] **Step 3: 编译验证**

```bash
cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent/.worktrees/feature/context-window-config
cargo check 2>&1
```

Expected: 编译通过，无 `max_context_tokens` 引用残留。

- [ ] **Step 4: 运行测试**

```bash
cargo test 2>&1
```

- [ ] **Step 5: 提交**

```bash
git add src/config.rs echo-state/src/compression/levels.rs
git -c commit.gpgsign=false commit -m "refactor: use resolved context_window in apply_compressor, drop max_context_tokens refs"
```

---

### Task 5: 框架层全量验证 + `cargo fmt`

- [ ] **Step 1: 检查是否还有其他 `max_context_tokens` 引用**

```bash
cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent/.worktrees/feature/context-window-config
grep -rn "max_context_tokens" --include="*.rs" .
```

Expected: 无输出（或仅剩 `levels.rs` 的 doc comment 中还有——在 Task 4 已更新）。

- [ ] **Step 2: 全量编译 + 测试 + 格式化**

```bash
cargo check --workspace && cargo test --workspace && cargo fmt --all && cargo clippy --all-targets -- -D warnings 2>&1
```

Expected: 全部通过。

- [ ] **Step 3: 提交（如有格式化变更）**

```bash
git add -u && git -c commit.gpgsign=false commit -m "chore: cargo fmt after context_window changes"
```

---

### Task 6: 应用层 — `ModelRuntimeConfig` / `ConfiguredModelView` 新增字段

**Files:**
- Modify: `echo-agent-cli/echo-agent-app-core/src/model_config.rs`

- [ ] **Step 1: `ModelRuntimeConfig` 新增字段**

在 `ModelRuntimeConfig` 结构体的 `max_tokens` 字段之后添加（约第 40 行）：

```rust
    pub context_window: Option<u32>,
```

- [ ] **Step 2: `ConfiguredModelView` 新增字段**

在 `ConfiguredModelView` 结构体的 `max_tokens` 字段之后添加（约第 27 行）：

```rust
    pub context_window: Option<u32>,
```

- [ ] **Step 3: `configured_model_views()` 透传**

在 `configured_model_views()` 函数中，`ConfiguredModelView` 构造中添加（约第 108 行之后）：

```rust
                context_window: model.context_window,
```

- [ ] **Step 4: `resolve_runtime_model()` 透传**

在 `resolve_runtime_model()` 函数中：

1. 解构 selected 时添加 `context_window`（约第 206-216 行）：
```rust
    let (id, display_name, provider, model, temperature, max_tokens, context_window) =
        if let Some(selected) = selected {
            (
                selected.id.clone(),
                selected.display_name.clone(),
                selected.provider.clone(),
                selected.model.clone(),
                selected.temperature,
                selected.max_tokens,
                selected.context_window,
            )
        } else {
            (
                fallback_id,
                display_name_from_model(&config.model.name),
                config.model.provider.clone(),
                config.model.name.clone(),
                config.model.temperature,
                config.model.max_tokens,
                config.model.context_window,  // 从 ModelConfig 获取
            )
        };
```

2. `ModelRuntimeConfig` 构造中添加（约第 253-263 行）：
```rust
            context_window,
```

- [ ] **Step 5: `set_default_model()` 透传**

在 `set_default_model()` 中，同步 `context_window` 到 `config.model`（约第 154 行之后）：
```rust
    config.model.context_window = model.context_window;
```

- [ ] **Step 6: 编译验证**

```bash
cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent-cli/.worktrees/feature/context-window-config
cargo check 2>&1
```

Expected: 编译通过。

- [ ] **Step 7: 提交**

```bash
cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent-cli/.worktrees/feature/context-window-config
git add echo-agent-app-core/src/model_config.rs
git -c commit.gpgsign=false commit -m "feat: add context_window to ModelRuntimeConfig and ConfiguredModelView"
```

---

### Task 7: 应用层 — `UpsertConfiguredModelRequest` 新增字段 + Tauri 命令透传

**Files:**
- Modify: `echo-agent-cli/src/tauri/commands/providers.rs`

- [ ] **Step 1: `UpsertConfiguredModelRequest` 新增字段**

在结构体的 `max_tokens` 字段之后添加（约第 20 行）：

```rust
    pub context_window: Option<u32>,
```

- [ ] **Step 2: `upsert_configured_model()` 构造 `ConfiguredModel` 时填入**

在 `upsert_configured_model` 函数中，`ConfiguredModel` 构造处（约第 159-167 行）添加：

```rust
        let configured = ConfiguredModel {
            id: req.id.unwrap_or_default(),
            display_name: req.display_name.unwrap_or_default(),
            provider: req.provider,
            model: req.model,
            enabled: req.enabled.unwrap_or(true),
            max_tokens: req.max_tokens,
            temperature: req.temperature,
            context_window: req.context_window,
        };
```

- [ ] **Step 3: 编译验证**

```bash
cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent-cli/.worktrees/feature/context-window-config
cargo check 2>&1
```

Expected: 编译通过。

- [ ] **Step 4: 提交**

```bash
git add src/tauri/commands/providers.rs
git -c commit.gpgsign=false commit -m "feat: add context_window to UpsertConfiguredModelRequest"
```

---

### Task 8: 应用层全量测试

- [ ] **Step 1: 运行测试**

```bash
cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent-cli/.worktrees/feature/context-window-config
cargo test 2>&1
```

Expected: 全部通过。

---

### Task 9: 前端 — TypeScript 类型更新

**Files:**
- Modify: `echo-agent-cli/web-frontend/src/types/api.ts`
- Modify: `echo-agent-cli/web-frontend/src/api/endpoints.ts`

- [ ] **Step 1: `ConfiguredModel` 接口新增字段**

在 `api.ts` 的 `ConfiguredModel` 接口中 `max_tokens` 之后添加：

```typescript
  context_window: number | null;
```

- [ ] **Step 2: `upsertConfigured` 请求类型更新**

在 `endpoints.ts` 的 `upsertConfigured` 请求类型中 `max_tokens` 之后添加：

```typescript
  context_window?: number | null;
```

- [ ] **Step 3: 前端类型检查**

```bash
cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent-cli/.worktrees/feature/context-window-config/web-frontend
npx tsc -b 2>&1
```

Expected: 零错误。

- [ ] **Step 4: 提交**

```bash
cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent-cli/.worktrees/feature/context-window-config
git add web-frontend/src/types/api.ts web-frontend/src/api/endpoints.ts
git -c commit.gpgsign=false commit -m "feat: add context_window to frontend types and API"
```

---

### Task 10: 前端 — ProviderPanel UI 改造

**Files:**
- Modify: `echo-agent-cli/web-frontend/src/components/providers/ProviderPanel.tsx`

- [ ] **Step 1: 新增 `contextWindow` state 和快捷选项常量**

在组件顶部、其他 state 声明之后添加（约第 21 行之后）：

```typescript
  const [contextWindow, setContextWindow] = useState('');
  const [contextWindowPreset, setContextWindowPreset] = useState('auto');

  const CONTEXT_PRESETS = [
    { label: '自动', value: 'auto' },
    { label: '4K', value: '4096' },
    { label: '8K', value: '8192' },
    { label: '16K', value: '16384' },
    { label: '32K', value: '32768' },
    { label: '64K', value: '65536' },
    { label: '128K', value: '131072' },
    { label: '200K', value: '200000' },
    { label: '500K', value: '500000' },
    { label: '1M', value: '1000000' },
    { label: '2M', value: '2000000' },
    { label: '自定义', value: 'custom' },
  ];
```

- [ ] **Step 2: 修改 `handleSelectProvider` 清零新 state**

在 `handleSelectProvider` 函数中，`setMaxTokens('')` 之后添加：

```typescript
    setContextWindow('');
    setContextWindowPreset('auto');
```

- [ ] **Step 3: 重命名"最大上下文" → "最大输出 Token"**

在第 403-404 行，将标签从：
```typescript
                最大上下文
```
改为：
```typescript
                最大输出 Token
```

placeholder 从 `"默认"` 改为 `"默认（由模型决定）"`（第 412 行）。

- [ ] **Step 4: 新增"模型上下文窗口"输入区域**

在"温度 / 最大输出 Token"行（第 385 行的 `<div className="grid grid-cols-2 gap-3">`）之后，新增独立行：

```tsx
          {/* Context Window */}
          <div>
            <label className="mb-1 block text-xs text-[var(--text-secondary)]">
              模型上下文窗口
              <span className="ml-1 text-[var(--text-tertiary)]">(可选，留空自动推断)</span>
            </label>
            <div className="flex gap-2">
              <select
                value={contextWindowPreset}
                onChange={(e) => {
                  const val = e.target.value;
                  setContextWindowPreset(val);
                  if (val === 'auto') {
                    setContextWindow('');
                  } else if (val === 'custom') {
                    // 保持当前输入值，由用户手动输入
                  } else {
                    setContextWindow(val);
                  }
                }}
                className="rounded-lg border border-[var(--border-primary)] bg-[var(--bg-input)] px-2 py-2 text-sm text-[var(--text-primary)] outline-none focus:border-[var(--border-focus)]"
              >
                {CONTEXT_PRESETS.map((p) => (
                  <option key={p.value} value={p.value}>
                    {p.label}
                  </option>
                ))}
              </select>
              {contextWindowPreset === 'custom' && (
                <input
                  type="number"
                  min="1"
                  step="1000"
                  value={contextWindow}
                  onChange={(e) => setContextWindow(e.target.value)}
                  placeholder="输入 token 数"
                  className="flex-1 rounded-lg border border-[var(--border-primary)] bg-[var(--bg-input)] px-3 py-2 text-sm text-[var(--text-primary)] outline-none placeholder:text-[var(--text-tertiary)] focus:border-[var(--border-focus)]"
                />
              )}
            </div>
            <p className="mt-1 text-[10px] text-[var(--text-tertiary)]">
              用于压缩触发、TokenBudget 分配和自适应压缩调优。自动模式下根据模型名称推断（DeepSeek→128K、Claude→200K 等）。
            </p>
          </div>
```

- [ ] **Step 5: `handleSwitch` 中提交 `context_window`**

在 `providerApi.upsertConfigured()` 调用中（约第 125-133 行），添加：

```typescript
        context_window: contextWindow ? Number(contextWindow) : null,
```

完整修改处：
```typescript
      const res = await providerApi.upsertConfigured({
        model,
        provider: selected.id,
        api_key: hasCustomApiKey ? trimmedApiKey : undefined,
        base_url: hasCustomBaseUrl || isCustom ? trimmedBaseUrl : undefined,
        temperature: temperature ? Number(temperature) : undefined,
        max_tokens: maxTokens ? Number(maxTokens) : undefined,
        context_window: contextWindow ? Number(contextWindow) : null,
        set_default: true,
      });
```

- [ ] **Step 6: 前端类型检查 + 构建**

```bash
cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent-cli/.worktrees/feature/context-window-config/web-frontend
npx tsc -b && npm run build 2>&1
```

Expected: 类型检查通过、构建成功。

- [ ] **Step 7: 提交**

```bash
cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent-cli/.worktrees/feature/context-window-config
git add web-frontend/src/components/providers/ProviderPanel.tsx
git -c commit.gpgsign=false commit -m "feat: add context window UI with quick-select presets; rename max context → max output tokens"
```

---

### Task 11: 最终全量验证 + 提交

- [ ] **Step 1: echo-agent 全 feature 矩阵验证**

```bash
cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent/.worktrees/feature/context-window-config

# 默认 feature
cargo check --workspace && cargo test --workspace

# 逐个 feature
cargo check -p echo_agent --no-default-features --features sqlite
cargo check -p echo_agent --no-default-features --features subagent
cargo check -p echo_agent --no-default-features --features human-loop
cargo check -p echo_agent --no-default-features --features mcp
cargo check -p echo_agent --no-default-features --features lsp
cargo check -p echo_agent --no-default-features --features tasks
cargo check -p echo_agent --no-default-features --features eval
cargo check -p echo_agent --no-default-features --features improve
cargo check -p echo_agent --no-default-features --features channels

# fmt + clippy
cargo fmt --all
cargo clippy --all-targets -- -D warnings
```

Expected: 全部通过。

- [ ] **Step 2: echo-agent-cli 全 feature 验证**

```bash
cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent-cli/.worktrees/feature/context-window-config

# 默认 + GUI feature
cargo check --no-default-features --features gui --bin echo-agent-tauri
cargo test --no-default-features --features gui

# 默认 feature
cargo check --workspace && cargo test --workspace

# fmt + clippy
cargo fmt --all
cargo clippy --all-targets -- -D warnings
```

Expected: 全部通过。

- [ ] **Step 3: 前端验证**

```bash
cd web-frontend
npx tsc -b && npm run build
```

Expected: 类型检查零错误，构建成功。

- [ ] **Step 4: YAML 兼容性测试**

创建测试 YAML 文件验证旧配置向后兼容：

```bash
cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent/.worktrees/feature/context-window-config
# 创建一个无 context_window 字段的最小配置
echo 'model:
  provider: deepseek
  name: deepseek-v4-flash
agent:
  name: test
  system_prompt: "test"' > /tmp/test_no_context.yaml

# 用该配置跑测试
cargo test -- --test-threads=1 2>&1
```

验证旧配置不报错。

- [ ] **Step 5: 最终提交（如有遗漏的格式变更）**

```bash
cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent/.worktrees/feature/context-window-config
git add -u && git -c commit.gpgsign=false commit -m "chore: final cleanup after context_window integration" || echo "nothing to commit"

cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent-cli/.worktrees/feature/context-window-config
git add -u && git -c commit.gpgsign=false commit -m "chore: final cleanup after context_window integration" || echo "nothing to commit"
```

---

## ⚠️ 合并前检查清单

合并回 main 前，echo-agent-cli 的以下 3 个 Cargo.toml 需将 `echo_agent` path 从绝对路径还原：

- `Cargo.toml` line 51 → `path = "../echo-agent"`
- `echo-agent-app-core/Cargo.toml` line 13, 61 → `path = "../../echo-agent"`
- `echo-agent-eval/Cargo.toml` line 10 → `path = "../../echo-agent"`

**不要提交 worktree 中的路径变更。**
