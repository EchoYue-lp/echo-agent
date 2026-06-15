# 44 — 自进化系统：现有组件兼容性分析

## 概述

现有 `echo-agent/src/improve/` 模块包含 8 个组件，其中 4 个已集成到 CLI，4 个为实验性（未集成）。新的 `echo-agent/src/evolution/` 模块需要与这些组件正确兼容：能复用就复用，不能复用就删除旧版。

---

## 逐组件分析

### 1. TrajectorySaver (`trajectory.rs`) — ✅ 保留不动

**功能**：将 Run 轨迹转换为 ShareGPT JSONL 格式，用于模型微调。

**与新系统的关系**：完全独立。TrajectorySaver 做数据格式转换和持久化，不涉及记忆类型化、分层、技能生命周期等。两者共存无冲突。

**决策**：**保留原样**，不做任何修改。

---

### 2. BackgroundReviewer (`background_review.rs`) — 🔄 扩展

**功能**：用 LLM 审查对话，提取记忆和技能建议。当前写入 `["background_reviews"]` 命名空间，存储原始 JSON（无类型化元数据）。

**与新系统的重叠**：
- 审查 prompt（MEMORY_REVIEW_PROMPT, SKILL_REVIEW_PROMPT, COMBINED_REVIEW_PROMPT）有直接价值，可复用
- 但写入的是原始 JSON，没有 `MemoryType`、`confidence`、`stability` 等元数据
- 新系统的 `TriggerDetector` 需要从 BackgroundReviewer 的结果中提取 `UserCorrection` 和 `SkillCandidate`

**决策**：**保留 LLM 审查机制，扩展写入路径**。
- Phase 0 后：`run_review()` 中 `store.put()` 调用改为 `TypedMemoryStore::put_typed()`，带上 `MemoryType`、`MemorySource::AutoExtracted`、`confidence` 等元数据
- 保留 `ReviewOutcome` 返回类型
- 保留 `build_transcript()` 方法
- 保留三种审查 prompt
- 新增：审查完成后调用 `TriggerDetector` 进行触发分类

**具体改动**：
```rust
// 旧: store.put(&["background_reviews"], &key, value)
// 新: typed_store.put_typed(
//     &["agent", "typed_memories"],
//     &key,
//     content,
//     MemoryMeta {
//         memory_type: MemoryType::UserPreference, // 或根据内容分类
//         source: MemorySource::AutoExtracted,
//         confidence: 0.6, // AutoExtracted 默认值
//         ..Default::default()
//     },
// )
```

---

### 3. Curator (`curator.rs`) — 🔄 扩展

**功能**：管理技能生命周期 Active → Stale → Archived。仅操作 Agent 创建的技能。

**与新系统的重叠**：
- 新系统需要 6 状态生命周期：`Candidate → Draft → Active → Stale → Deprecated → Archived`
- 现有只有 3 状态：`Active, Stale, Archived`
- 需要在前面加 `Candidate, Draft`，在 Stale 和 Archived 之间加 `Deprecated`

**决策**：**扩展 SkillLifecycle 枚举**。
- 在现有枚举中新增 3 个变体
- 扩展 `apply_transitions()` 方法，增加新的转换路径
- `CuratorStatus` 新增对应计数器
- 保留 `SkillMeta`、`CuratorConfig`、`CuratorState` 的结构
- 保留 `pin_skill`、`unpin_skill`、`touch_skill` 方法
- 新增 `promote_candidate()`、`activate_draft()`、`deprecate_skill()` 方法

**具体改动**：
```rust
// 旧:
pub enum SkillLifecycle { Active, Stale, Archived }

// 新:
pub enum SkillLifecycle {
    Candidate,  // 新增：从记忆中发现的模式
    Draft,      // 新增：已生成 SKILL.md 但未激活
    Active,     // 保留
    Stale,      // 保留
    Deprecated, // 新增：被替代或已知过时
    Archived,   // 保留
}
```

---

### 4. Analyzer (`analyzer.rs`) — ✅ 保留 + 集成

**功能**：检测运行轨迹中的失败模式（WriteWithoutRead, ExcessiveRetries, ExcessiveToolCalls），生成 `ImprovementSuggestion`。

**与新系统的关系**：
- 失败模式检测直接服务于 Phase 2 的 `MemoryReviewer`（将失败写入 `ErrorResolution` 类型记忆）
- `CritiqueIssue` 枚举可被 Phase 4 的 `SkillPatcher` 消费
- `ImprovementSuggestion::PromptChange` 与 `SkillPatch::RefineInstruction` 有重叠但服务不同目标

**决策**：**保留原样，集成到新系统**。
- Analyzer 继续作为独立的运行分析器
- Phase 2 `MemoryReviewer` 在分析完成后，将 `CritiqueIssue` 转化为 `MemoryType::ErrorResolution` 记忆
- Phase 4 `SkillPatcher` 消费 `CritiqueIssue` 和 `ImprovementSuggestion` 生成 `SkillPatch`
- 不需要修改 Analyzer 本身

---

### 5. ImprovementLoop (`loop.rs`) — ✅ 保留不动

**功能**：迭代式评估→批判→建议→重评估循环。使用 EvalRunner + Analyzer 进行 prompt 优化。

**与新系统的关系**：功能不同。
- ImprovementLoop 是 **eval-driven prompt optimization**（基于评估用例的 prompt 优化）
- 新 evolution 系统是 **memory/skill lifecycle management**（记忆/技能生命周期管理）
- 两者互补但不重叠

**决策**：**保留原样**。如果未来需要，新系统可以消费 ImprovementLoop 的 `LoopResult` 作为 skill health 判断的输入，但当前无需改动。

---

### 6. SelfEvolution (`evolution.rs`) — 🔄 重命名

**功能**：ImprovementLoop 的薄封装 + HTML 报告生成。一个 `.enable()` 开关。

**与新系统的冲突**：
- 名称 `SelfEvolution` 与新的 `evolution/` 模块名冲突
- 新的 `evolution/` 模块覆盖范围远大于此（记忆、技能、规则、审计）
- 功能上不冲突：这个是 eval-driven 改进，新模块是 memory/skill-driven 自进化

**决策**：**重命名为 `EvalDrivenImprovement`**，避免命名冲突。
- 文件从 `evolution.rs` 移至 `eval_improvement.rs`
- 类型从 `SelfEvolution` 重命名为 `EvalDrivenImprovement`
- 功能不变
- `mod.rs` 的 re-export 更新

---

### 7. PromptGenerator (`generator.rs`) — ✅ 保留 + 复用

**功能**：用 LLM 从失败分析中生成改进的 system prompt。

**与新系统的关系**：
- Phase 3 `SkillDraftGenerator` 可以复用 `PromptGenerator` 来生成 SKILL.md 草稿
- Phase 4 `SkillPatcher` 可以参考其 prompt 构造模式

**决策**：**保留原样，被新系统复用**。
- `SkillDraftGenerator` 内部调用 `PromptGenerator::generate_improved_prompt()` 或类似方法
- 不需要修改 PromptGenerator 本身

---

### 8. CritiqueStore / DualLayerCritiqueStore (`store.rs`) — ❌ 删除

**功能**：内存 + 文件持久化的批判存储，支持项目级和全局级双层。

**与新系统的重叠**：
- 新的 `ChangeLog` (`audit.rs`) 提供更通用的变更跟踪（不仅限于 Critique，还覆盖 Memory/Skill/Rule）
- 新的 `TypedMemoryStore` + 命名空间约定可以替代双层概念
- `CritiqueStore` 使用了 `.unwrap()` 违反项目规则（Known Pitfall #5）
- 未集成到 CLI，是实验性组件

**决策**：**删除 `store.rs`**。
- 用 `ChangeLog` 替代变更追踪功能
- 双层概念通过 `TypedMemoryStore` 命名空间实现：
  - 项目级：`["project", "evolution", "critiques"]`
  - 全局级：`["agent", "evolution", "critiques"]`
- `CritiqueStore` 的 `top_patterns()` 功能可由 `TypedMemoryStore::search_typed()` + 聚合实现
- 删除后更新 `mod.rs` 的 re-export

---

### 9. 共享类型（在 `mod.rs` 中定义）

| 类型 | 决策 | 原因 |
|------|------|------|
| `CritiqueIssue` | ✅ 保留 | Analyzer 产出，被 SkillPatcher 消费 |
| `ImprovementSuggestion` | ✅ 保留 | Analyzer 和 ImprovementLoop 使用 |
| `RunCritique` | ✅ 保留 | 核心分析数据类型 |

---

## 决策汇总

| 组件 | 动作 | 原因 |
|------|------|------|
| `TrajectorySaver` | 保留不动 | 完全独立，无冲突 |
| `BackgroundReviewer` | 扩展 | 写入路径改为 TypedMemoryStore |
| `Curator` | 扩展 | SkillLifecycle 新增 3 个状态 |
| `Analyzer` | 保留 + 集成 | 失败检测服务于新系统 |
| `ImprovementLoop` | 保留不动 | 功能不同（eval-driven），互补 |
| `SelfEvolution` | 重命名为 `EvalDrivenImprovement` | 命名冲突，功能不冲突 |
| `PromptGenerator` | 保留 + 复用 | 被 SkillDraftGenerator 复用 |
| `CritiqueStore` | ❌ 删除 | 被 ChangeLog + TypedMemoryStore 替代 |
| `DualLayerCritiqueStore` | ❌ 删除 | 同上 |
| `CritiqueIssue` | 保留 | Analyzer 产出类型 |
| `ImprovementSuggestion` | 保留 | Analyzer/ImprovementLoop 使用 |
| `RunCritique` | 保留 | 核心数据类型 |

## 实施顺序

在 Phase 0 实现时，需同步完成以下兼容性操作：

1. **删除 `store.rs`**，更新 `mod.rs` 移除 re-export
2. **重命名 `evolution.rs` → `eval_improvement.rs`**，更新 `mod.rs` re-export
3. **扩展 `curator.rs`** 的 `SkillLifecycle` 枚举
4. **扩展 `background_review.rs`** 的写入路径

这些改动应作为 Phase 0 的一部分完成，确保新模块和旧模块在同一阶段对齐。
