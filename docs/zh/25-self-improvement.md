# 自进化系统 — 分析、演化、技能自创建

## 概述

自进化系统让 Agent 从运行经验中**持续改进自身**：分析失败、积累结构化记忆、从重复模式中**自动创建技能**、合并/维护已过时的技能，并把高置信度的知识晋升为永久规则。

它由两个互补的模块组成：

- [`improve`](../../src/improve) — **评估驱动**的离线改进：分析轨迹、生成提示词建议、迭代调优。
- [`evolution`](../../src/evolution) — **运行时演化**闭环：分层记忆、变更审计、技能生命周期（候选→草稿→激活）、技能合并/健康/补丁、规则晋升、安全防护。

```
运行 Agent
   │
   ├─ 运行中 ──────── TriggerDetector（在线发现新记忆）──┐
   ├─ 压缩淘汰 ────── memory_promoter（生命周期管理）──┤
   ├─ 用户显式/应用调度 ─ BackgroundReviewer ─ ReviewCandidate（提案）
   └─ 已确认/显式记忆证据 ───────────────────────────┤
                                                       ▼
                                    MemoryLayerManager（热/暖/冷分层）
                                                       │
                           ┌───────────────┬──────────┴──────────┬───────────────┐
                           ▼               ▼                     ▼               ▼
                     MemoryReviewer    SkillCandidate      SkillHealth/      RulePromoter
                     （审查/合并/GC）  Detector→Draft      Merge/Patch      （→ AGENTS.md）
```

---

## 解决的问题

随着任务变难和边缘情况累积，Agent 性能会逐渐退化。没有系统化改进：

- **失败模式重复**：Agent 不读文件就写入、过度重试失败工具、遗漏明显工具
- **无反馈循环**：过去的失败不影响未来行为
- **无经验记忆**：每次会话从零开始
- **技能老化/重复**：技能过时却未被发现，或多个技能功能重叠
- **知识无法沉淀**：学到的高价值经验永远是临时记忆，进不了永久规则

---

## 安全模型

语义提案需要人工审查。确定性的记忆维护、用户显式保存/纠正路径可以自动写入，但系统**不会自动**：

- 修改核心运行时代码
- 放松安全策略 / 更改权限规则
- 应用技能合并、补丁或规则晋升（仅生成提案，由人通过命令应用）
- 把来自不可信来源（工具输出）的记忆晋升到热层或规则

所有对记忆/技能/规则的变更都写入变更审计日志（`change-log.jsonl`），可查询、可回滚。写入时还会进行密钥扫描与提示注入检测。

---

## 模块全景

| 模块 | 职责 | 位置 |
|------|------|------|
| **Analyzer** | 检测运行轨迹中的失败模式 | `improve/` |
| **ImprovementLoop** | 评估→批判→建议→重新评估的迭代调优 | `improve/` |
| **EvalDrivenImprovement** | 一键启动完整评估改进循环（原 `SelfEvolution`） | `improve/` |
| **PromptGenerator** | LLM 驱动的提示词改进 | `improve/` |
| **TrajectorySaver** | 将运行转为 ShareGPT 微调数据 | `improve/` |
| **TypedMemoryStore** | 带元数据的结构化记忆读写 | `echo-state` |
| **MemoryLayerManager** | 热/暖/冷三层记忆管理 | `evolution/` |
| **ChangeLog** | 变更审计与回滚 | `evolution/` |
| **TriggerDetector** | 在线对话信号→新记忆 | `evolution/` |
| **MemoryReviewer** | 陈旧评分、冲突检测、合并、归档（GC） | `evolution/` |
| **Curator** | 技能生命周期状态机 | `evolution/` |
| **SkillCandidateDetector** | 从重复模式发现技能候选 | `evolution/` |
| **SkillDraftGenerator** | 从候选生成草稿 SKILL.md | `evolution/` |
| **SkillSimilarityDetector / SkillMerger** | 检测重叠技能并合并 | `evolution/` |
| **SkillHealthMonitor** | 技能健康评分（驱动弃用） | `evolution/` |
| **SkillPatcher** | 从失败遥测生成技能补丁 | `evolution/` |
| **RulePromoter** | 高置信度记忆→产品自有 learned rules | `echo-agent-app-core` |
| **ReviewIntegration / Dashboard** | 产品层审查调度与状态仪表盘 | `echo-agent-app-core` |

---

## 第一部分：评估驱动改进（`improve`）

### Analyzer — 失败检测

`Analyzer` 检查已完成的运行轨迹，检测常见失败模式：

```rust
use echo_agent::improve::Analyzer;

let critique = Analyzer::analyze(&run);
println!("{}", critique.format_report());
```

输出：

```
运行: run_abc123
成功: false (得分: 0.65)

发现的问题:
  - 先写后读: write 被调用 2 次，之前未调用 read_file
  - 过度重试: shell 被重试 4 次

建议:
  [提示词] tools: 添加指令: '编辑文件前务必先用 read_file 读取'
  [策略] force_read_before_edit: true — 原因: Agent 先写后读 2 次
```

#### 检测的问题类型

| 问题 | 检测逻辑 | 建议 |
|------|----------|------|
| `WriteWithoutRead` | 对同一文件先调用写工具再调用读工具 | 添加先读后写提示词 |
| `ExcessiveRetries` | 同一工具错误 > 2 次 | 添加"尝试不同方法"指令 |
| `ToolErrorPattern` | 同一工具反复失败 | 为该工具生成评估用例 |
| `ContextOverflow` | 触发了上下文压缩 | 建议上下文感知提示 |
| `MissingTool` | 未使用预期工具 | 建议添加工具指令 |
| `ExcessiveToolCalls` | 一次运行 > 20 次工具调用 | 添加效率指令 |

### ImprovementLoop — 迭代改进

```rust
use echo_agent::improve::ImprovementLoop;

let loop_runner = ImprovementLoop {
    max_iterations: 5,
    improvement_threshold: 0.95,  // 测试得分 >= 95% 时停止
    holdout_ratio: 0.4,           // 40% 测试，60% 训练
};

let result = loop_runner.run(&cases, agent_factory, &run_store).await;
println!("最优得分: {:.2}，第 {} 次迭代", result.best_score, result.best_iteration);
```

工作原理：按标准类型分层拆分用例防止过拟合 → 训练集评估 → `Analyzer` 批判 → 生成去重建议 → 留出集盲测 → 追踪最优 → 达到阈值提前停止。

### EvalDrivenImprovement — 一键启动

> **注意**：原 `SelfEvolution` 类型已**重命名**为 `EvalDrivenImprovement`（避免与新 `evolution` 模块命名冲突）。

```rust
use echo_agent::improve::EvalDrivenImprovement;

let result = EvalDrivenImprovement::new()
    .with_eval_cases(cases)
    .with_run_store(run_store)
    .max_iterations(5)
    .with_report_dir("./eval_reports")
    .enable()
    .run(|| create_agent())
    .await;
```

### PromptGenerator 与 TrajectorySaver

- `PromptGenerator` — 基于 `Analyzer` 的失败分析，用 LLM 生成改进的系统提示词。
- `TrajectorySaver` — 把完成的运行转为 ShareGPT JSONL，用于模型微调：

```rust
use echo_agent::improve::TrajectorySaver;

let saver = TrajectorySaver::default_dir()?;
saver.save(&run, "qwen3-max").await?;
let entries = saver.list(Some("2026-05-29")).await?;
```

---

## 第二部分：运行时演化闭环（`evolution`）

这是 `v0.2.x` 新增的核心自进化能力，让 Agent 在运行过程中持续积累并复用知识。

### 类型化记忆 — `TypedMemoryStore`

每条记忆都带结构化元数据 `MemoryMeta`：类型、置信度、稳定性、风险、状态、来源、主题。向后兼容——未类型化的旧条目读取时自动获得默认元数据。

```rust
use echo_agent::memory::typed_store::{TypedMemoryStore, MemoryFilter};
use echo_agent::prelude::{MemoryMeta, MemorySource, MemoryType, MemoryStatus};

let store = TypedMemoryStore::new(arc_store);

// 写入带元数据的记忆
let meta = MemoryMeta::new(MemoryType::ProjectFact, MemorySource::UserCorrection, "build-tool")
    .with_confidence(0.9)
    .with_stability(0.8);
store
    .put_typed(&["agent", "typed_memories"], "build:java8", "项目用 Java 8", meta)
    .await?;

// 按条件过滤检索
let filter = MemoryFilter::new()
    .with_type(MemoryType::ProjectFact)
    .with_min_confidence(0.7);
let entries = store.list_typed(&["agent", "typed_memories"], &filter).await?;
```

#### MemoryType 分类

`UserPreference | ProjectFact | ArchitectureDecision | DebuggingLesson | ErrorResolution | CommandPattern | ToolUsage | WorkflowPattern | SkillCandidate | DeprecatedNote`

#### MemorySource 与默认置信度

| 来源 | 含义 | 默认置信度 |
|------|------|-----------|
| `ExplicitSave` | `/remember` 或 `remember` 工具显式保存 | 1.0 |
| `UserCorrection` | 检测到用户纠正 Agent | 0.9 |
| `ErrorResolution` | 工具失败后以不同方法成功重试 | 0.85 |
| `RepeatedWorkflow` | 相同工具序列被观察 ≥3 次 | 0.75 |
| `AutoExtracted` | AutoMemory 从会话归档提取 | 0.6 |

### 三层记忆管理 — `MemoryLayerManager`

记忆按价值分层，热层始终加载进上下文，暖/冷层按需检索：

- **热层**（`.echo-agent/MEMORY.md`）：最高价值，YAML frontmatter + markdown 正文，上限 ~2000 token，人类与 Agent 都可编辑。
- **暖层**（Store KV `["agent","typed_memories"]`）：按主题组织，按需加载。
- **冷层**（Store KV `["agent","cold_memories"]`）：归档旧/低置信度记忆。

```rust
use echo_agent::evolution::{MemoryLayerManager, JsonlChangeLog, MemoryMeta, MemorySource, MemoryType};
use std::path::PathBuf;

let mgr = MemoryLayerManager::new(
    PathBuf::from(".echo-agent"),
    arc_store,
    Box::new(JsonlChangeLog::new(PathBuf::from(".echo-agent/evolution/change-log.jsonl"))?),
);

// 写入（自动扫描密钥/注入，并按置信度判断是否进热层）
let meta = MemoryMeta::new(MemoryType::ProjectFact, MemorySource::ExplicitSave, "deploy")
    .with_confidence(0.95);
mgr.write_memory("deploy:prod-script", "部署用 pnpm build", meta).await?;

// 晋升/降级
mgr.promote("some-key").await?;          // 冷→暖→热
mgr.demote("some-key", "stale").await?;  // 热→暖→冷

// 跨层搜索
let hits = mgr.search_layered("deploy", 10).await?;
```

### 变更审计 — `ChangeLog`

所有对记忆/技能/规则的变更都记录到 append-only JSONL，支持过滤查询：

```rust
use echo_agent::evolution::{ChangeFilter, ChangeType, EntityType};

let filter = ChangeFilter::new()
    .with_entity_type(EntityType::Memory)
    .with_change_type(ChangeType::Promote)
    .with_limit(50);
// 日志文件：.echo-agent/evolution/change-log.jsonl
```

### 记忆审查与确定性维护

记忆会积累。`MemoryReviewer` 只对暖层做陈旧度评分和冲突检测，不修改内容或状态；`Dreaming` 独立执行基于 recall/inactivity 的确定性晋升、复活和归档，并返回可解释的 decision report。语义冲突必须由产品层显式采纳后再调用合并原语：

```
staleness = age·0.35 + low_usage·0.20 + instability·0.20 + contradiction·0.20 + source_weakness·0.05
```

| 陈旧度 | 状态 |
|--------|------|
| < 0.35 | Active |
| 0.35–0.50 | Active（建议审查） |
| 0.50–0.65 | Superseded 候选 |
| ≥ 0.65 | Archived 候选 |

```rust
use echo_agent::evolution::{MemoryReviewer, ReviewConfig};

let reviewer = MemoryReviewer::new();
let report = reviewer
    .review(&typed_store, &ReviewConfig::default())
    .await?;
// report.staleness_suggestions / report.conflict_proposals
```

`ReviewConfig::default()` 默认关闭 session-end review，单次最多返回 10 个冲突建议、每个建议最多 16 条成员，以限制 JSONL 和上下文增长。框架保留显式 `MemoryMerger`，但 reviewer 不会自行调用；应用应在用户确认后执行，并保存 before snapshot 以支持撤销。

### 带证据的运行回顾 — `BackgroundReviewer`

`BackgroundReviewer` 把 run transcript 当作不可信证据，只接受包含精确引用的严格 JSON，返回结构化 `ReviewCandidate`。默认只提案，不写长期记忆。只有框架复用方显式开启 `auto_persist_user_preferences` 时，才可能把高置信用户偏好写成 Draft memory。单次回顾输出上限为 512 token。

### 技能生命周期与自创建

#### 完整生命周期（Curator 状态机）

```
Candidate → Draft → Active → Stale → Deprecated → Archived
```

`Curator`（位于 `evolution/`）管理这些状态转换：

```rust
use echo_agent::evolution::{Curator, CuratorConfig, SkillLifecycle};

let curator = Curator::new(
    CuratorConfig { stale_days: 30, archive_days: 90, enabled: true },
    "~/.echo-agent/curator_state.json",
);
curator.register_candidate("cargo-build")?;   // 候选
curator.promote_to_draft("cargo-build")?;      // → 草稿
curator.promote_to_active("cargo-build")?;     // → 激活
curator.pin_skill("critical-skill")?;          // 固定免于自动转换
let transitions = curator.apply_transitions()?; // 按闲置时间自动转换
```

#### 从观察模式自动创建技能

1. **`SkillCandidateDetector`** 扫描 `TypedMemoryStore` 中 `WorkflowPattern`/`DebuggingLesson` 记忆；当同一主题 ≥3 条且来源为 `RepeatedWorkflow` → 提出技能候选。

   ```rust
   use echo_agent::evolution::SkillCandidateDetector;
   let detector = SkillCandidateDetector::new();
   let report = detector.detect(&typed_store, &change_log).await?;
   // report.new_candidates / report.reinforced
   ```

2. **`SkillDraftGenerator`** 从候选用模板生成草稿 `SKILL.md`，保存到消费方传入的 evolution root 下 `skills/_drafts/<name>/SKILL.md`。

   ```rust
   use echo_agent::evolution::SkillDraftGenerator;
   let gen = SkillDraftGenerator::new("<application-data>".into(), &change_log);
   let result = gen.generate_from_candidate(&candidate).await?;
   // result.skill_md_path 指向生成的草稿
   ```

   embedding application 当前传入 `<application-data>`，因此草稿位于 `<application-data>/skills/_drafts/<name>/SKILL.md`。这一产品路径应以 [embedding application app-core 源码](https://github.com/EchoYue-lp/echo-agent-cli/tree/main/echo-agent-app-core/src) 为准。

3. 人工审查后通过 `/skill-promote <name>` 将 Draft 移至 Active，技能即出现在技能目录中。

### 技能合并、健康、补丁

| 组件 | 评分公式 / 行为 |
|------|----------------|
| **SkillSimilarityDetector** | `description·0.25 + trigger·0.30 + scope·0.15 + tool·0.10 + pitfall·0.10 + co_activation·0.10`；≥0.75 出合并提案，≥0.90 强烈建议 |
| **SkillMerger** | 应用合并提案：保留激活次数更高的为主，吸收次要技能的触发器与独特指令；需 `/skill-merge <a> <b>` 人工应用 |
| **SkillHealthMonitor** | `success_rate·0.30 + recent_success·0.20 + usage·0.10 + freshness·0.15 + approval·0.15 + cmd_validity·0.10`；≥0.75 健康，<0.55 不健康 |
| **SkillPatcher** | 分析遥测 `common_failures` → 生成 `SkillPatch`（加前置条件/工具/错误处理）；需 `/skill-patch <name>` 人工应用 |

```rust
use echo_agent::evolution::{SkillSimilarityDetector, SkillHealthMonitor};

let detector = SkillSimilarityDetector::new(arc_store.clone());
// 传入当前所有技能描述符；≥0.75 相似度产出合并提案，需 `/skill-merge` 人工应用
let proposals = detector.scan_and_propose(&skill_descriptors, &change_log).await?;

let monitor = SkillHealthMonitor::new(arc_store);
for report in monitor.analyze_all_skills().await? {
    println!("{}: {:?}", report.skill_name, report.status);
}
```

### 规则晋升（产品层）

`RulePromoter` 是 embedding application 的产品策略，不是框架持久化契约。embedding application 当前会先审查高置信度记忆提案，再把批准的规则写入 `<application-data>/learned-rules.md`；权威阈值与工作流应以 [embedding application app-core 源码](https://github.com/EchoYue-lp/echo-agent-cli/tree/main/echo-agent-app-core/src) 为准。

### 安全加固 — `EvolutionSecurityGuard`

- **写入前**：密钥扫描（AWS `AKIA...`、GitHub `ghp_...`、`BEGIN PRIVATE KEY` 等，匹配项替换为 `[REDACTED]`）+ 提示注入检测（如 "ignore previous" 模式）
- **不可信输入隔离**：工具输出来源的记忆 `risk = High`，未经人工批准不可晋升到热层或规则
- **速率限制**：每会话最多 50 次记忆写入，每天最多 5 次技能补丁
- 所有变更经 `ChangeLog` 可回滚

---

## 自动记忆责任边界

系统中有三条自动记忆路径，职责严格划分以避免重复系统：

| 系统 | 主要职责 | 不应承担 |
|------|---------|---------|
| `TriggerDetector`（运行时） | 在线轻量发现并附带精确来源摘录；框架默认可直接持久化，产品也可安装 `MemoryTriggerSink` 接管 | 会话归档总结、产品审阅策略 |
| 应用 observation policy | 应用可以提取观察，并通过 typed-memory API 提交已采纳事实 | 压缩淘汰、运行时策略调度 |
| `memory_promoter`（压缩路径） | 因 token 压力被压缩/淘汰的消息的生命周期管理（长期化、淘汰、降级） | 新偏好发现、UI 触发的提取 |
| `BackgroundReviewer`（显式/应用调度） | 从已完成 run 生成带证据 JSON 候选，默认只提案 | 自动长期写入或产品调度策略 |

> 关键约束：任何被接受并进入运行时 recall 的 typed memory，**必须**统一走框架的 `MemoryLayerManager::write_memory`。提取与审阅策略归上层应用。

---

## 文件布局

```
.echo-agent/
  MEMORY.md                        # 热层（人类可读，Agent 与人类都可编辑）
  AGENTS.md                        # 自动晋升的规则
  project.md / local.md            # 已有的静态提示文件
  memory/
    topics/*.md                    # 暖层主题文件
    archive/                       # 冷层归档
  evolution/
    change-log.jsonl               # 变更审计日志
    skill_candidates/              # 候选提案
    patches/                       # 技能补丁
  skills/
    _drafts/<name>/SKILL.md        # 草稿技能
  curator_state.json               # Curator 状态
```

框架复用方可以选择其它路径。embedding application 注入 workspace scope，使用 `<application-data>/evolution/evidence-candidates.jsonl` 与 `<application-data>/evolution/curator-state.json`。

## Store 命名空间

| 命名空间 | 用途 |
|---------|------|
| `["agent", "typed_memories"]` | 类型化记忆（暖层） |
| `["agent", "cold_memories"]` | 归档记忆（冷层） |
| `["agent", "skill_candidates"]` | 技能候选提案 |
| `["agent", "skill_telemetry"]` | 技能遥测 |
| `["agent", "profile"]` | Agent 配置 |
| `["agent", "evolution", "patches"]` | 技能补丁 |
| `["agent", "evolution", "merges"]` | 合并提案 |
| `["agent", "evolution", "rules"]` | 规则提案 |

---

## Feature 开关与使用模式

`evolution` 默认随框架启用；`improve` 与 `eval` 通过 feature flag 开启：

```toml
[dependencies]
echo_agent = { version = "0.2", features = ["improve"] }
```

基础 `improve` feature 提供显式轨迹导出和兼容 re-export。`BackgroundReviewer` 与 `Curator` 属于默认 `evolution` 模块。评测驱动分析需要同时启用两个 feature：

```toml
echo_agent = { version = "0.2", features = ["improve", "eval"] }
```

自进化系统**不内置于 Agent 循环**，而是作为独立的分析/演化 pass 运行，保持 Agent 轻量：

```
生产环境（无自进化）：   agent.execute("执行任务").await
自进化（独立 pass）：    EvalDrivenImprovement::new()...run(agent_factory).await
运行时演化：            MemoryLayerManager / TriggerDetector / ReviewIntegration 集成
```

### 与 Self-Reflection 的区别

| 维度 | Self-Reflection | 自进化 |
|------|----------------|--------|
| 时机 | Agent 执行期间 | Agent 执行之后 / 运行时持续 |
| 范围 | 单任务 | 跨任务模式 |
| 反馈 | 语言（LLM 批判） | 结构化（记忆、技能、规则） |
| Feature flag | `self-reflection` | `improve` + `evolution` |
| 集成方式 | 内置于 Agent 循环 | 外部批处理 / 运行时演化 |

另见：[24 - 评估系统](./24-eval-system.md) 了解驱动 `improve` 流水线的评估框架；[03 - 记忆系统](./03-memory.md) 了解底层 Store。
