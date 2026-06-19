# 模型上下文窗口（`context_window`）集成设计

**日期**：2026-06-17  
**状态**：草稿  
**范围**：`echo-agent`（核心框架） + `echo-agent-cli`（应用层/前端）

---

## 1. 背景与动机

### 1.1 现状问题

项目中已有多个与"模型长度"相关的参数，但分散且不一致：

| 参数 | 位置 | 含义 | 问题 |
|------|------|------|------|
| `max_tokens` | `ModelConfig` / `ConfiguredModel` / 前端 | 输出 token 上限（发往 API） | 前端标签写的是"最大上下文"，**标签误导** |
| `token_limit` | `AgentYamlConfig` / `AgentConfig` | 上下文触发压缩的阈值（默认 128K） | 仅在 YAML 层暴露，前端不可配 |
| `max_context_tokens` | `ModelProfile`（硬编码） | 模型最大上下文窗口容量 | 仅靠模型名模式匹配推断，**自定义模型/新模型无法识别** |
| `max_output_tokens` | `ModelProfile`（硬编码） | 模型输出能力上限 | 同上 |
| `TokenBudget.total_window` | `echo-core/src/budget.rs` | 上下文窗口分配预算 | 依赖 `max_context_tokens`，用户不可控 |

**核心痛点**：用户在前端配置模型时，无法显式告知系统"这个模型的最大上下文窗口是多大"——这对自定义模型、新模型、或想限制上下文窗口以节省成本的场景很重要。

### 1.2 目标

新增 `context_window: Option<u32>` 字段，从 `ConfiguredModel` 一路透传到 `AgentConfig` / `TokenBudget` / 压缩调优：

- 用户显式设置优先
- 未设置时 fallback 到名称模式自动推断
- 同时影响压缩阈值 + TokenBudget 窗口 + `ModelProfile` 能力声明
- 清理冗余字段 `max_context_tokens`

---

## 2. 设计决策

### 2.1 优先级策略

```
用户显式设置 context_window
  → 覆盖所有下游（token_limit / TokenBudget / 压缩调优）
  ↓ 未设置时
自动推断 infer_context_window(provider, model_name)
  → 基于模型名称模式匹配（deepseek→128K, claude→200K, qwen→131K...）
  ↓ 也未匹配到
None → 使用 AgentConfig 默认值 token_limit=128K
```

### 2.2 数据流

```
前端 UI（新输入框"模型上下文窗口"）
  ↓ upsertConfigured({ context_window: 128000 })
UpsertConfiguredModelRequest
  ↓
ConfiguredModel { context_window: Some(128000) }
  ↓ set_default_model() 同步
ModelConfig { context_window: Some(128000) }
  ↓ to_agent_config() 解析
AgentConfig {
  token_limit: 128000,           // ← context_window
  token_budget_config: {
    total_window: 128000,        // ← context_window
    ...
  }
}
  ↓ apply_compressor()
tune_for_model(&mut config, token_limit)  // ← context_window
```

### 2.3 删除项

| 删除 | 位置 | 原因 |
|------|------|------|
| `ModelProfile::max_context_tokens` | `echo-core/.../capabilities.rs` | 降级为纯信息字段，无独立消费必要 |
| `ProviderCapabilities::max_context_tokens` | 同上 | 同上 |

**保留并提取**：`ModelProfile::new()` 中的名称推断逻辑 → 独立函数 `pub fn infer_context_window(provider: &str, model_name: &str) -> Option<u32>`

### 2.4 下游影响映射

| 消费点 | 当前来源 | 改为 |
|--------|----------|------|
| 压缩触发阈值 | `AgentConfig.token_limit`（默认 128K） | ← 被 `context_window` 覆盖 |
| TokenBudget 窗口分配 | `TokenBudgetConfig.total_window` | ← 同上 |
| 自适应压缩调优 `tune_for_model()` | `caps.max_context_tokens` | ← `AgentConfig.token_limit` |
| 安全警告（token_limit > 窗口） | `caps.max_context_tokens` | ← `AgentConfig.token_limit` |

---

## 3. 数据结构变更

### 3.1 新增字段

| 结构体 | 文件 | 变更 |
|--------|------|------|
| `ConfiguredModel` | `echo-agent/src/config.rs` | + `context_window: Option<u32>`，`#[serde(default)]` |
| `ModelConfig` | 同上 | + `context_window: Option<u32>`，`#[serde(default)]` |
| `ModelRuntimeConfig` | `echo-agent-cli/echo-agent-app-core/src/model_config.rs` | + `context_window: Option<u32>` |
| `ConfiguredModelView` | 同上 | + `context_window: Option<u32>` |
| `UpsertConfiguredModelRequest` | `echo-agent-cli/src/tauri/commands/providers.rs` | + `context_window: Option<u32>` |
| 前端 `ConfiguredModel` | `web-frontend/src/types/api.ts` | + `context_window: number \| null` |

### 3.2 AgentConfig 不改结构体

`AgentConfig` 已有 `token_limit: usize` 和 `token_budget_config: TokenBudgetConfig`。在 `to_agent_config()` 转换时，由 `context_window` 覆盖这两个字段的值。

---

## 4. 核心逻辑变更

### 4.1 提取推断函数（`echo-core/src/llm/capabilities.rs`）

```rust
/// 根据厂商和模型名称推断上下文窗口大小。
/// 未匹配到已知模式时返回 None。
pub fn infer_context_window(provider: &str, model_name: &str) -> Option<u32> {
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

### 4.2 解析 context_window（`echo-agent/src/config.rs` 或 `agent/config.rs`）

```rust
/// 解析最终的上下文窗口值：用户设置 > 自动推断 > 默认值
pub fn resolve_context_window(
    explicit: Option<u32>,
    provider: &str,
    model_name: &str,
) -> usize {
    explicit
        .or_else(|| infer_context_window(provider, model_name))
        .unwrap_or(128_000)
        .clamp(1, 10_000_000) as usize  // 合理的上下界
}
```

### 4.3 to_agent_config 转换

在 `AppConfig::to_agent_config()` 中：

```rust
let context_window = resolve_context_window(
    model_config.context_window,
    &model_config.provider,
    &model_config.name,
);
config.token_limit = context_window;
config.token_budget_config.total_window = context_window;
```

### 4.4 apply_compressor 改造（`echo-agent/src/config.rs`）

```rust
// 安全警告 —— 改为读 agent 的 token_limit（已是解析后的 context_window）
let token_limit = agent.token_limit();  // 新增 accessor
// ...

// 自适应压缩调优
tune_for_model(&mut config, agent.token_limit());
```

`ReactAgent` 需新增 `pub fn token_limit(&self) -> usize` 访问器。

### 4.5 tune_for_model 重命名参数（`echo-state/.../levels.rs`）

```rust
// 仅重命名参数，逻辑不变
pub fn tune_for_model(config: &mut AdaptiveCompressionConfig, context_window: usize) {
    let w = context_window;
    // ... 原有计算逻辑不变
}
```

---

## 5. 前端 UI 变更（`ProviderPanel.tsx`）

### 5.1 重命名现有字段

- 现有"最大上下文"输入框的标签改为 **"最大输出 Token"**
- 绑定保持 `maxTokens` / `setMaxTokens` 不变
- placeholder 从 "默认" 改为 "默认（由模型决定）"

### 5.2 新增"模型上下文窗口"输入

位置：在"温度 / 最大输出 Token"行下方，新增一行。

交互设计：
- 下拉快捷选：`自动`（默认）/ `4K` / `8K` / `16K` / `32K` / `64K` / `128K` / `200K` / `500K` / `1M` / `2M` / `自定义`
- 选"自动"→ `context_window` 为 `null`/`undefined`（走 fallback 推断）
- 选手动值 → 自动填入对应数字
- 选"自定义"→ 显示数字输入框，用户手动输入 token 数
- 快捷值旁的辅助文字：显示对应模型参考（如"128K — DeepSeek/GPT"）

### 5.3 修改 `handleSwitch` 提交

`providerApi.upsertConfigured()` 调用中新增 `context_window` 参数。

---

## 6. YAML 兼容性

- `context_window: null` 或字段缺失 → 自动推断（向后兼容，现有配置文件无需改动）
- `context_window: 128000` → 显式覆盖
- 使用 `#[serde(default)]` + `Option<u32>` 保证旧配置文件反序列化不报错

---

## 7. 涉及文件清单

### 核心框架 `echo-agent/`

| 文件 | 变更类型 |
|------|----------|
| `src/config.rs` | 新增字段 `ConfiguredModel.context_window`、`ModelConfig.context_window`；修改 `to_agent_config()` 解析逻辑；修改 `apply_compressor()` 消费逻辑 |
| `src/agent/config.rs` | `to_agent_config()` 中新增 `context_window` → `token_limit`/`total_window` 覆盖逻辑；确认 `ReactAgent` 有 `token_limit()` 访问器 |
| `echo-core/src/llm/capabilities.rs` | 删除 `ModelProfile::max_context_tokens`、`ProviderCapabilities::max_context_tokens`；提取 `infer_context_window()` 函数；清理 `ModelProfile::new()` |
| `echo-state/src/compression/levels.rs` | `tune_for_model()` 参数重命名 |

### 应用层 `echo-agent-cli/`

| 文件 | 变更类型 |
|------|----------|
| `echo-agent-app-core/src/model_config.rs` | `ModelRuntimeConfig` + `ConfiguredModelView` 新增 `context_window` 字段；`resolve_runtime_model()` / `configured_model_views()` / `set_default_model()` 透传该字段 |
| `src/tauri/commands/providers.rs` | `UpsertConfiguredModelRequest` 新增 `context_window` 字段；`upsert_configured_model` 构造 `ConfiguredModel` 时填入 |
| `web-frontend/src/types/api.ts` | `ConfiguredModel` 接口新增 `context_window: number \| null` |
| `web-frontend/src/api/endpoints.ts` | `upsertConfigured` 请求类型新增 `context_window?: number \| null` |
| `web-frontend/src/components/providers/ProviderPanel.tsx` | UI 改造：重命名 + 新增上下文窗口输入 |

---

## 8. 验证清单

- [ ] `cargo check --workspace`（全 feature 矩阵）
- [ ] `cargo test --workspace`（全 feature 矩阵）
- [ ] `cargo fmt --all`
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] 前端 `npx tsc -b` + `npm run build`
- [ ] YAML 向后兼容：旧配置文件（无 `context_window` 字段）反序列化不报错
- [ ] 手动测试：预置厂商模型 → 不填上下文窗口 → 自动推断生效
- [ ] 手动测试：自定义模型 → 填写 64K → 压缩/预算使用 64K
- [ ] 手动测试：前端下拉快捷选择 → 正确填充数值
