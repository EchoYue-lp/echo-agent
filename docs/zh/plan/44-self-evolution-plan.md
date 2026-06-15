# 44 — 自进化与自改善实现计划

## 背景

echo-agent 系统已有记忆（Store trait, MemoryPromoter, UnifiedMemory, AutoMemory）和技能（SkillRegistry, SkillTelemetry, Curator, 文件型 SKILL.md）的坚实基础，但缺少**闭环自进化**机制：记忆没有结构化类型和置信度评分，没有分层热/暖/冷系统，没有自动写入触发器，没有记忆审查/GC，没有从观察模式创建技能，没有技能合并/健康/补丁，也没有规则晋升。

本计划增量添加自进化：每个阶段独立有价值、可测试，且遵循框架/产品边界。

## 关键设计决策

1. **MemoryMeta 存储**：嵌入 `StoreItem.value` JSON — 向后兼容，无需修改 Store trait 或 schema
2. **MEMORY.md / AGENTS.md**：人类可读文件，Agent 和人类都可编辑；Agent 必须尊重人类编辑（检测并保留手动修改）
3. **实现范围**：全部 5 个阶段，顺序实现
4. **Evolution 模块**：新建 `echo-agent/src/evolution/` 模块，与现有 `echo-agent/src/improve/` 并行

## 与现有 improve 模块的兼容性（重要）

现有 `improve/` 模块的 8 个组件需要逐一处理。详见 [兼容性分析文档](./44-self-evolution-compatibility.md)。

| 组件 | 动作 | 原因 |
|------|------|------|
| `TrajectorySaver` | 保留不动 | 完全独立，无冲突 |
| `BackgroundReviewer` | 扩展 | 写入路径改为 TypedMemoryStore |
| `Curator` | 扩展 | SkillLifecycle 新增 Candidate/Draft/Deprecated |
| `Analyzer` | 保留 + 集成 | 失败检测服务于新系统 |
| `ImprovementLoop` | 保留不动 | 功能不同（eval-driven），互补 |
| `SelfEvolution` | 重命名为 `EvalDrivenImprovement` | 命名冲突，功能不冲突 |
| `PromptGenerator` | 保留 + 复用 | 被 SkillDraftGenerator 复用 |
| `CritiqueStore` / `DualLayerCritiqueStore` | ❌ 删除 | 被 ChangeLog + TypedMemoryStore 替代 |

---

## 阶段 0：类型化记忆基础 + 变更审计

**为何优先**：一切依赖于记忆条目具有结构化元数据。没有类型、置信度、状态和审计追踪，分层、审查或晋升都无法运作。

### 框架层（echo-agent）

**1. MemoryType + MemoryMeta** — `echo-core/src/memory/types.rs`（新建）
```
MemoryType: UserPreference | ProjectFact | ArchitectureDecision | DebuggingLesson | ErrorResolution | CommandPattern | ToolUsage | WorkflowPattern | SkillCandidate | DeprecatedNote
MemoryMeta { memory_type, confidence: f32, stability: f32, risk: MemoryRisk(Low/Medium/High), status: MemoryStatus(Draft/Active/Superseded/Archived), source: MemorySource(UserCorrection|ErrorResolution|RepeatedWorkflow|ExplicitSave|AutoExtracted), topic: String, revision_count: u32, superseded_by: Option<String> }
```
- 序列化：`MemoryMeta` 嵌入 `StoreItem.value` JSON 的 `content` 字段旁。无需修改 `StoreItem` schema。

**2. TypedMemoryStore** — `echo-state/src/memory/typed_store.rs`（新建）
- 包装 `Arc<dyn Store>`，提供类型化读写：`put_typed()`, `get_typed()`, `search_typed(filter: MemoryFilter)`
- `MemoryFilter`：按类型、状态、最低置信度、主题、来源过滤
- 向后兼容：未类型化条目读取时自动获得 `MemoryMeta::default()`

**3. ChangeLog + ChangeAuditor** — `echo-agent/src/evolution/audit.rs`（新建）
- `ChangeEntry { timestamp, entity_type(Memory/Skill/Rule), entity_key, change_type(Create/Update/Delete/Promote/Demote/Merge), before, after, reason, trigger }`
- `ChangeLog` trait：`record()`, `query()`, `rollback()`
- `JsonlChangeLog`：写入 `.echo-agent/evolution/change-log.jsonl`
- `ChangeAuditor`：包装任意 Store，拦截写入并记录变更

**4. 密钥扫描** — `echo-agent/src/evolution/security.rs`（新建）
- API 密钥正则模式（AWS `AKIA...`, GitHub `ghp_...`, 通用 base64 token, `BEGIN PRIVATE KEY`, `Bearer ...`）
- 持久化前扫描；匹配项替换为 `[REDACTED: <type>]`，记录警告
- `UntrustedInputMarker`：工具输出内容设置 `MemoryMeta.risk = High`

### 集成点
- `StoreMemoryPromoter` → 使用 `TypedMemoryStore::put_typed()` 替代原始 `store.put()`
- `AutoMemory::extract_observations()` → 通过 `TypedMemoryStore` 写入，`MemorySource::AutoExtracted`
- `BackgroundReviewer` → 通过 `TypedMemoryStore` 写入，`MemorySource::UserCorrection`

### 新模块结构
```
echo-agent/src/evolution/
  mod.rs          (新建, re-exports)
  audit.rs        (新建)
  security.rs     (新建)
```

### 阶段 0 兼容性操作（与上述同步进行）

1. **删除 `improve/store.rs`**（CritiqueStore / DualLayerCritiqueStore）
   - 被 `ChangeLog` + `TypedMemoryStore` 命名空间替代
   - 更新 `improve/mod.rs` 移除 re-export

2. **重命名 `improve/evolution.rs` → `improve/eval_improvement.rs`**
   - 类型 `SelfEvolution` → `EvalDrivenImprovement`
   - 更新 `improve/mod.rs` re-export

3. **扩展 `improve/curator.rs`**
   - `SkillLifecycle` 新增 `Candidate`, `Draft`, `Deprecated`
   - 新增转换路径和方法
   - 保留现有测试

4. **扩展 `improve/background_review.rs`**
   - `run_review()` 中 `store.put()` → `TypedMemoryStore::put_typed()`
   - 写入时附带 `MemoryMeta` 元数据
   - 保留审查 prompt 和 `ReviewOutcome`

### 验证
- `cargo test -p echo-core` — MemoryMeta serde 往返测试
- `cargo test -p echo-state` — TypedMemoryStore 带 filter 读写搜索
- `cargo test -p echo-agent` — ChangeLog 记录/查询/回滚，密钥扫描捕获 `AKIA...`

---

## 阶段 1：记忆分层 + 写入触发器

**为何**：有了类型化记忆，可以分层（热/暖/冷）并从对话信号自动触发写入，而非仅靠关键词匹配。这是最大的用户体验提升。

### 框架层（echo-agent）

**1. MemoryLayerManager** — `echo-agent/src/evolution/layer.rs`（新建）
- 三层：
  - **热层**（MEMORY.md）：最高价值记忆，始终加载到上下文。路径：`.echo-agent/MEMORY.md`。上限 2000 token。
  - **暖层**（Store KV, `["agent", "typed_memories"]`）：按主题组织，按需加载
  - **冷层**（归档, `.echo-agent/memory/archive/`）：较旧/低置信度，极少加载
- `promote(key)`：冷→暖→热（如果有空间）
- `demote(key)`：热→暖→冷（当热层超出限制时）
- 热层 = 人类可读 markdown（YAML frontmatter + 正文）；暖层/冷层 = Store KV

**2. 写入触发器检测** — `echo-agent/src/evolution/triggers.rs`（新建）
- 触发类型：`UserCorrection`, `ErrorResolution`, `RepeatedWorkflow`, `ExplicitSave`, `AutoExtracted`
- 检测模式：
  - UserCorrection：用户消息包含纠正信号（"不是这样", "不对", "不要", "wrong", "actually", "no,"）且位于助手对同一主题输出之后
  - ErrorResolution：工具失败后以不同方法成功重试
  - RepeatedWorkflow：相同工具序列被观察 ≥3 次（来自遥测）
  - ExplicitSave：`/remember` 命令或 `remember` 工具
- 各触发类型默认置信度：UserCorrection=0.9, ErrorResolution=0.85, RepeatedWorkflow=0.75, ExplicitSave=1.0, AutoExtracted=0.6

**3. Hook 事件扩展** — `echo-core/src/hooks/types.rs`（修改）
- 新增 `HookEvent::PostMemoryWrite` — 任何记忆持久化后
- 新增 `HookEvent::MemoryLayerChange` — 晋升/降级后
- 新增 `HookEventCategory::Evolution` 用于这些新事件
- 更新 `category()`, `as_str()`, `supports_matcher()` match 分支

### 产品层（echo-agent-app-core）

**4. 分层记忆渲染** — 修改 `unified_memory.rs`
- `UnifiedMemory.system_prompt_context()` 现在包含热层 MEMORY.md 内容
- 新工具 `search_layered_memory` 搜索热层 + 暖层
- `InstructionProvider` 在现有 .md 文件旁注入热层内容

### 验证
- 单元：晋升/降级循环，热层大小限制执行，样本对话触发检测
- 集成：包含用户纠正的对话 → 捕获为 `UserCorrection` → 写入热层 → 出现在下次系统 prompt 中

---

## 阶段 2：记忆审查 + 垃圾回收

**为何**：记忆会积累。没有审查，置信度衰减、冲突出现、陈旧条目污染上下文。

### 框架层（echo-agent）

**1. StalenessScorer** — `echo-agent/src/evolution/review.rs`（新建）
```
staleness = age_factor * 0.35 + low_usage_factor * 0.20 + instability_factor * 0.20 + contradiction_factor * 0.20 + source_weakness * 0.05
```
- 状态：<0.40→Active, 0.40-0.65→Stale, 0.65-0.85→Deprecated, ≥0.85→Archived

**2. ConflictDetector** — 同文件
- 发现相同 `topic` + `memory_type` 但矛盾内容的记忆（不同内容哈希）
- 尽可能对照当前仓库状态验证（如检查 `package-lock.json` vs `yarn.lock`）

**3. MemoryMerger** — 同文件
- 合并相似记忆：保留更高置信度为主，合并证据，组合使用次数，更新摘要，旧条目标记 Superseded

**4. MemoryReviewer** — 同文件编排器
- 扫描 → 评分 → 检测冲突 → 合并/替代 → 通过 ChangeAuditor 记录变更

**5. 审查调度** — `echo-agent-app-core/src/evolution/review_integration.rs`（新建）
- 自动触发：每 50 次记忆写入或会话结束
- 手动：`/memory-review` 命令
- 在 `BackgroundReviewer` 完成后做后处理

### 验证
- 单元：陈旧度评分，冲突检测，合并保留更高置信度内容
- 集成：插入 10 条记忆，推进时间，运行审查 → 陈旧降级，冲突合并，变更日志记录所有操作

---

## 阶段 3：扩展技能生命周期 + 技能创建

**为何**：这是最大的自进化价值 — 技能可以从观察到的模式中诞生。

### 框架层（echo-agent）

**1. 扩展 SkillLifecycle** — 修改 `echo-agent/src/improve/curator.rs`
- 新增 `Candidate`, `Draft`, `Deprecated` 状态
- 完整生命周期：`Candidate → Draft → Active → Stale → Deprecated → Archived`
- 可配置阈值的转换规则
- `CuratorStatus` 新增计数器
- 现有 `apply_transitions()` 新增 match 分支

**2. SkillCandidateDetector** — `echo-agent/src/evolution/candidate.rs`（新建）
- 扫描 `TypedMemoryStore` 中的 `MemoryType::WorkflowPattern` / `MemoryType::DebuggingLesson`
- 当 ≥3 条相同主题条目且 `source == RepeatedWorkflow` → 提出候选
- 写入 `["agent", "skill_candidates"]` 命名空间
- 候选包含：name, description, trigger_patterns, tool_sequence, sample_count, confidence

**3. SkillDraftGenerator** — `echo-agent/src/evolution/draft.rs`（新建）
- 从候选生成草稿 SKILL.md（非 LLM 路径使用模板，LLM 路径使用 `PromptGenerator`）
- 草稿 SKILL.md：YAML frontmatter 含 `lifecycle: draft`、name、description、triggers；markdown 正文含合成指令
- 保存到 `.echo-agent/skills/_drafts/<name>/SKILL.md`

### 产品层（echo-agent-app-core）

**4. 技能生命周期命令** — 扩展 CLI
- `/skill-candidates` — 列出所有候选和草稿
- `/skill-promote <name>` — 将 Draft 移至 Active
- `/skill-create` — 从候选交互式创建

### 集成
- `SkillTelemetryStore` → `SkillCandidateDetector` 读取遥测寻找重复工具模式
- `Curator` → 扩展新的生命周期状态
- `SkillRegistry` → `register_descriptor()` 已支持任意 SKILL.md；草稿在子目录中可通过 `lifecycle` 过滤扫描
- `TriggerDetector` → 将 `RepeatedWorkflow` 观察馈入候选

### 验证
- 单元：生命周期状态机转换，候选检测，草稿生成
- 集成：3 次相同调试模式的对话 → SkillCandidate 创建 → 晋升为 Draft → SKILL.md 存在 → 晋升为 Active → 出现在目录中

---

## 阶段 4：技能合并 + 健康 + 补丁

**为何**：随着技能增多，它们会重叠和退化。合并防止重复；健康+补丁保持技能有效。

### 框架层（echo-agent）

**1. SkillSimilarityDetector** — `echo-agent/src/evolution/merge.rs`（新建）
```
similarity = description_overlap * 0.25 + trigger_overlap * 0.30 + scope_overlap * 0.15 + tool_overlap * 0.10 + pitfall_overlap * 0.10 + co_activation_rate * 0.10
```
- ≥0.75 → 合并提案；≥0.90 → 强烈建议

**2. SkillMergeProposal + 合并器** — 同文件
- 保留更高激活次数的技能为主，吸收次要技能的触发器和独特指令
- 需人工审查：合并提案存储但不自动应用
- `/skill-merge <a> <b>` 命令应用

**3. SkillHealthMonitor** — `echo-agent/src/evolution/health.rs`（新建）
```
health = success_rate * 0.30 + recent_success_rate * 0.20 + usage_frequency * 0.10 + freshness * 0.15 + user_approval * 0.15 + command_validity * 0.10
```
- ≥0.75→Healthy, 0.55-0.75→NeedsAttention, <0.55→Unhealthy
- 每次技能停用后读取 `SkillTelemetry`

**4. SkillPatcher** — `echo-agent/src/evolution/patch.rs`（新建）
- 分析遥测中的 `common_failures` → 生成 `SkillPatch`
- 补丁类型：`AddPrecondition`, `AddTool`, `RefineInstruction`, `AddErrorHandling`
- 补丁是建议，不自动应用
- `/skill-patch <name>` 审查并应用

### 验证
- 单元：相似度评分，合并提案，健康评分计算，故障模式补丁建议
- 集成：注册 2 个相似技能 → 运行合并 → 合并后技能工作；持续失败的技能 → 健康检查 → Unhealthy + 补丁建议

---

## 阶段 5：规则晋升 + 安全加固

**为何**：自进化的最高形式 — 学到的知识成为永久规则。安全加固确保生产安全。

### 产品层（echo-agent-app-core）

**1. RulePromoter** — `echo-agent-app-core/src/evolution/rules.rs`（新建）
- 扫描高置信度记忆（confidence≥0.95, stability≥0.9, importance≥8.0, revision_count==0）
- 生成 `RuleProposal { source_memory_key, rule_text, target_tier, reason }`
- 需人工审查通过 `/rule-promote` 命令
- 已应用规则通过 `InstructionProvider` 写入 instruction .md 文件
- 源记忆标记为 `status: Superseded`

**2. AGENTS.md 集成** — 修改 `instruction_provider.rs`
- 新增 `.echo-agent/AGENTS.md` 作为第四层级（在 Project 和 Local 之间）
- 段落：`## Auto-promoted rules`, `## Learned constraints`, `## Deprecated rules`

**3. EvolutionSecurityGuard** — 扩展 `echo-agent/src/evolution/security.rs`
- 预写入：密钥扫描（阶段 0）+ 提示注入检测（如 "ignore previous" 模式）
- 不可信输入隔离：工具输出来源的记忆 → `risk: High`，未经人工批准不可晋升到热层或规则
- 速率限制：每会话最多 50 次记忆写入，每天最多 5 次技能补丁
- 通过 `ChangeAuditor` 回滚

**4. 仪表盘** — `/evolution status` 命令
- 记忆按层级、类型、状态的计数
- 技能生命周期分布
- 待处理合并提案、补丁、规则候选
- 审计日志中的最近变更

### 验证
- 单元：从高置信度记忆生成规则提案，AGENTS.md 写入+重载，速率限制
- 集成：高置信度记忆 → 规则晋升 → 出现在 AGENTS.md → 源记忆 Superseded → 回滚 → 记忆恢复为 Active

---

## 文件布局

```
.echo-agent/
  MEMORY.md                        # 热层（阶段 1）
  AGENTS.md                        # 自动晋升规则（阶段 5）
  project.md                       # 已有
  local.md                         # 已有
  memory/
    topics/*.md                    # 暖层（阶段 1）
    archive/                       # 冷层（阶段 1）
  evolution/
    change-log.jsonl               # 审计日志（阶段 0）
    skill_candidates/              # 候选 SKILL.md 草稿（阶段 3）
    patches/                       # 技能补丁（阶段 4）
    critiques/                     # 已有
  skills/
    _drafts/<name>/SKILL.md        # 草稿技能（阶段 3）
  curator_state.json               # 已有
```

## Store 命名空间

| 命名空间 | 用途 |
|---------|------|
| `["agent", "typed_memories"]` | 新的类型化记忆条目 |
| `["agent", "skill_candidates"]` | 技能候选提案 |
| `["agent", "skill_telemetry"]` | 已有遥测 |
| `["agent", "profile"]` | 已有 Agent 配置 |
| `["agent", "evolution", "patches"]` | 技能补丁 |
| `["agent", "evolution", "merges"]` | 合并提案 |
| `["agent", "evolution", "rules"]` | 规则提案 |
| `["l3_promoted"]` | 已有 L3 晋升事实 |
| `["background_reviews"]` | 已有审查记忆 |

## 新 Hook 事件（阶段 1+）

| 事件 | 类别 | 阶段 |
|-----|------|------|
| `PostMemoryWrite` | Evolution | 1 |
| `MemoryLayerChange` | Evolution | 1 |
| `SkillCandidateDetected` | Evolution | 3 |
| `SkillLifecycleTransition` | Evolution | 3 |
| `SkillHealthCheck` | Evolution | 4 |
| `SkillPatchApplied` | Evolution | 4 |
| `RulePromoted` | Evolution | 5 |

## 需修改的关键文件

| 文件 | 变更 |
|----|------|
| `echo-core/src/memory/types.rs` | 新建：MemoryType, MemoryMeta, MemoryRisk, MemoryStatus, MemorySource |
| `echo-core/src/memory/mod.rs` | 新增 `pub mod types` |
| `echo-core/src/hooks/types.rs` | 新增 HookEvent 变体 + HookEventCategory::Evolution |
| `echo-state/src/memory/typed_store.rs` | 新建：TypedMemoryStore, MemoryFilter |
| `echo-state/src/memory/mod.rs` | 新增 `pub mod typed_store` |
| `echo-state/src/lib.rs` | 导出 TypedMemoryStore |
| `echo-agent/src/evolution/mod.rs` | 新建：所有 evolution 子模块 re-exports |
| `echo-agent/src/evolution/audit.rs` | 新建：ChangeLog, JsonlChangeLog, ChangeAuditor |
| `echo-agent/src/evolution/security.rs` | 新建：SecretScanner, EvolutionSecurityGuard |
| `echo-agent/src/evolution/layer.rs` | 新建：MemoryLayerManager |
| `echo-agent/src/evolution/triggers.rs` | 新建：TriggerDetector, WriteTrigger |
| `echo-agent/src/evolution/review.rs` | 新建：StalenessScorer, ConflictDetector, MemoryMerger, MemoryReviewer |
| `echo-agent/src/evolution/candidate.rs` | 新建：SkillCandidateDetector |
| `echo-agent/src/evolution/draft.rs` | 新建：SkillDraftGenerator |
| `echo-agent/src/evolution/merge.rs` | 新建：SkillSimilarityDetector, SkillMergeProposal |
| `echo-agent/src/evolution/health.rs` | 新建：SkillHealthMonitor, HealthStatus |
| `echo-agent/src/evolution/patch.rs` | 新建：SkillPatcher, SkillPatch |
| `echo-agent/src/improve/curator.rs` | 扩展：SkillLifecycle 增加 Candidate/Draft/Deprecated |
| `echo-agent/src/improve/background_review.rs` | 修改：写入改为 TypedMemoryStore + MemoryMeta |
| `echo-agent/src/improve/eval_improvement.rs` | 从 evolution.rs 重命名，类型 SelfEvolution→EvalDrivenImprovement |
| `echo-agent/src/improve/store.rs` | 删除：被 ChangeLog + TypedMemoryStore 替代 |
| `echo-agent/src/memory_promoter.rs` | 修改：使用 TypedMemoryStore |
| `echo-agent/src/lib.rs` | 新增 `pub mod evolution` |
| `echo-agent-cli/echo-agent-app-core/src/evolution/` | 新建：产品层集成模块 |
| `echo-agent-cli/echo-agent-app-core/src/unified_memory.rs` | 修改：添加层级集成 |
| `echo-agent-cli/echo-agent-app-core/src/instruction_provider.rs` | 修改：添加 AGENTS.md 层级 |

## 工作量估算

| 阶段 | 周数 | 累计价值 |
|------|------|---------|
| 0: 类型化记忆 + 审计 | 2-3 | 结构化、可审计的记忆与安全 |
| 1: 分层 + 触发器 | 2 | 分层记忆，自动写入触发器（最大体验提升） |
| 2: 审查 + GC | 2 | 自维护记忆 |
| 3: 技能生命周期 + 创建 | 3 | Agent 自主创建技能（核心自进化） |
| 4: 合并 + 健康 + 补丁 | 2 | 技能保持健康且不重复 |
| 5: 规则晋升 + 安全 | 2 | 学习 → 永久规则，完整安全 |
| **总计** | **约 13-15** | 完整自进化闭环 |

## 各阶段内实现顺序

每个阶段的实现顺序：
1. **echo-core 类型**（MemoryType, MemoryMeta, HookEvent 变体）— 基础类型
2. **echo-state 实现**（TypedMemoryStore 等）— 具体存储
3. **echo-agent/evolution 模块** — 业务逻辑（评分、检测、审查）
4. **echo-agent-app-core 集成** — 产品层接线、命令、提示词
5. **测试** — 单元 → 集成 → 完整 CI 矩阵

## 各阶段验证策略

每个阶段遵循：`cargo check` → `cargo test --workspace` → `cargo fmt` → `cargo clippy`

- 阶段 0：`cargo test -p echo-core -p echo-state -p echo_agent` — 类型化记忆 + 审计 + 安全测试
- 阶段 1：集成测试 — 触发检测 + 层级晋升/降级 + 系统 prompt 注入
- 阶段 2：集成测试 — 插入记忆，推进时间，审查，验证降级和合并
- 阶段 3：集成测试 — 3 次相似对话 → 候选 → 草稿 → 激活技能
- 阶段 4：集成测试 — 相似技能合并 + 不健康技能补丁
- 阶段 5：集成测试 — 规则晋升 + 回滚 + 安全防护

每个阶段完成前必须通过完整 CI 矩阵。

## 并行化机会

各阶段内，多个模块可并行实现：
- 阶段 0：`types.rs` + `audit.rs` + `security.rs` 相互独立
- 阶段 1：`layer.rs` + `triggers.rs` 相互独立（均仅依赖阶段 0 类型）
- 阶段 2：`StalenessScorer`, `ConflictDetector`, `MemoryMerger` 可并行开发再组合
- 阶段 3：`candidate.rs` + `draft.rs` 顺序（草稿依赖候选）
- 阶段 4：`merge.rs` + `health.rs` + `patch.rs` 相互独立
- 阶段 5：`rules.rs` + 安全扩展 + 仪表盘 相互独立
