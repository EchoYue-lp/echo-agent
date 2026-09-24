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

分层记忆变更先写入持久恢复操作，再修改 Store 或 `MEMORY.md`；已提交的业务变更可在 `change-log.jsonl` 查询。canonical 分层记忆支持按已结算 `ChangeId` 或 `BatchId` 事后回滚；Skill 与 Rule 回滚仍由独立契约负责（#54/#94/host owner）。记忆写入还会进行密钥扫描与提示注入检测。

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
| **ChangeLog** | append-only 业务变更审计；`MemoryLayerManager` 负责带 generation fencing 的记忆事后回滚 | `evolution/` |
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

### 分层记忆管理 — `MemoryLayerManager`

记忆按价值分层，热层始终加载进上下文，暖层按需检索：

- **热层**（`.echo-agent/MEMORY.md`）：最高价值，YAML frontmatter + markdown 正文，上限 ~2000 token，人类与 Agent 都可编辑。
- **暖层**（Store KV `["agent","memories"]`）：统一类型化存储，`Archived` 条目仍在此层并按衰减权重召回。
- **冷层**（可选公共 API `["agent","cold_memories"]`）：默认路径不使用独立冷存储；有独立归档需求的复用方可自行接入。

```rust
use echo_agent::evolution::{MemoryLayerManager, JsonlChangeLog, MemoryMeta, MemorySource, MemoryType};
use std::path::PathBuf;

let mgr = MemoryLayerManager::try_new(
    PathBuf::from(".echo-agent"),
    arc_store,
    Box::new(JsonlChangeLog::new(PathBuf::from(".echo-agent/evolution/change-log.jsonl"))?),
)?;
mgr.reconcile_pending().await?; // 向独立 Store 读者开放前先恢复

// 写入（自动扫描密钥/注入，并按置信度判断是否进热层）
let meta = MemoryMeta::new(MemoryType::ProjectFact, MemorySource::ExplicitSave, "deploy")
    .with_confidence(0.95);
mgr.write_memory("deploy:prod-script", "部署用 pnpm build", meta).await?;

// 晋升/降级
mgr.promote("some-key").await?;          // 符合条件的暖→热
mgr.demote("some-key", "stale").await?;  // 热→暖，或暖层原位归档

// 跨层搜索
let hits = mgr.search_layered("deploy", 10).await?;
```

### 变更审计 — `ChangeLog`

已提交的分层记忆变更记录到可过滤查询的 append-only JSONL；其它演化写入分别遵循自身审计合同：

```rust
use echo_agent::evolution::{ChangeFilter, ChangeType, EntityType};

let filter = ChangeFilter::new()
    .with_entity_type(EntityType::Memory)
    .with_change_type(ChangeType::Promote)
    .with_limit(50);
// 日志文件：.echo-agent/evolution/change-log.jsonl
```

对 `MemoryLayerManager`，`.echo-agent/evolution/memory-operations.jsonl` 是独立的崩溃恢复权威。操作先以 `SyncData` 确认含固定 change ID 和目标值的 prepare，再写 Store/`MEMORY.md` 投影，以同一 ID 幂等写入业务 `ChangeLog`，最后记录 settlement。prepare 后任一步失败都会返回未知完成结果；重启调用 `reconcile_pending().await?` 可补齐且不会重复业务审计。恢复还会核对已结算 key 的最新 journal 目标，修复 Store 因持久化屏障 degraded 而回退的投影。同步热层读在启动恢复前或待恢复时返回错误，manager 的异步读先恢复；直接读取 Store 或文件的调用者仍可能暂见中间态，必须等启动恢复完成。observer 仅在实时提交后触发，不跨重启重放。见 [ADR 0065](../adr/0065-evolution-memory-audit-reconciliation.md)。

含换行或首尾空白的热层内容在 `MEMORY.md` frontmatter 标记 `content_json: true`，正文 bullet 使用单行 JSON 字符串，无损恢复原文；旧的普通 bullet 仍可读取。晋升或降级若基于过期读取，另一 manager 已改同一 key，则在 prepare 前失败，不覆盖较新的值。

### 带证据的 Draft 与激活

`MemorySource` 表示抽取机制；`MemoryProvenance` 单独保存 user、assistant、tool
的精确原文及据此得到的信任分类。自动写入和分层 `remember` 工具先持久化
Draft。`write_memory` 不接受调用方传入的 `Active` 或 approval 作为激活依据。
调用方沿用同一 manager 审阅：

```rust,no_run
use echo_agent::prelude::MemoryApproval;
use echo_agent::evolution::MemoryLayerManager;

# async fn review(mgr: &MemoryLayerManager) -> echo_agent::error::Result<()> {
if let Some(proposal) = mgr.preview_activation("candidate-key").await? {
    let approval = MemoryApproval::new("review-123", "reviewer", 1_750_000_000);
    mgr.activate_draft(&proposal, approval).await?;
}
# Ok(())
# }
```

proposal 绑定准确内容、metadata 和 journal generation；过期内容及 A→B→A
改写会在 mutation 前失败，同一已结算批准的重试返回幂等结果。失败或取消后的
journal debt 由 `reconcile_pending` 结算。只有已批准 Active/Archived 记忆可
召回；缺少 provenance 的旧 typed/hot 记录仍可审阅，但不进入模型 context。
Recall 计数使用 Store CAS，不能把并发修改过的状态或来源写回。见
[ADR 0070](../adr/0070-memory-provenance-and-recall-authority.md)。

已批准的 `MemoryMerger` 现在绑定同一 manager：

```rust
use echo_agent::evolution::MemoryMerger;
let outcome = MemoryMerger::new(&mgr).merge_group(&reviewed_group).await?;
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

`Curator`（位于 `evolution/`）保存生命周期状态；mutation 统一提交给
`SkillMutationAuthority`，旧的 Curator 直接 mutation 方法不再公开。host 审阅
exact preview digest 后提供一次性 approval artifact：

```rust
use echo_agent::evolution::{SkillApprovalArtifact, SkillMutationAuthority};

let authority = Arc::new(SkillMutationAuthority::open(
    curator, change_log.clone(),
)?);
let preview = authority.preview(&request)?;
let approval = SkillApprovalArtifact::new(
    approval_id, &preview.operation_digest, approver, approved_at,
);
let receipt = authority.apply(request, approval).await?;
```

#### 从观察模式自动创建技能

1. **`SkillCandidateDetector`** 扫描 `TypedMemoryStore` 中 `WorkflowPattern`/`DebuggingLesson` 记忆；当同一主题 ≥3 条且来源为 `RepeatedWorkflow` → 提出技能候选。

   ```rust
   use echo_agent::evolution::{Curator, CuratorConfig, SkillCandidateDetector};
   let curator = Curator::new(CuratorConfig::default(), "<application-data>/evolution/curator-state.json");
   let detector = SkillCandidateDetector::new(curator);
   let report = detector.detect(&typed_store, &change_log).await?;
   // report.new_candidates / report.reinforced
   ```

   create 与 reinforce 共用一个私有 durable operation journal。每次 detect 会先恢复已 prepare
   的 candidate payload，并用固定 ID 幂等补齐 `ChangeLog`，再开始本轮扫描。只有 settled 后才会
   发布 report；观察数量没有增长时不写 payload 或 audit。`TypedMemoryStore`、`Curator`、
   `ChangeLog` 仍分别是 payload、lifecycle 与 append-only audit 权威。Store 与 ChangeLog 的
   reserved marker 把 journal 绑定到具体权威；Store 投影使用 exact atomic compare-and-put；
   Curator 在合法 Draft/Active 晋升中保留 candidate authority lineage。不支持原子 CAS 的 Store
   会被拒绝，不会退回有竞态的 get-then-put；`EmbeddingStore` 的派生向量索引无法与 inner
   payload 共用一次原子提交，因此明确属于 Unsupported。详见
   [ADR 0068](../adr/0068-skill-candidate-mutation-audit-reconciliation.md)。

2. **`SkillDraftGenerator`** 从候选用模板生成草稿 `SKILL.md`，保存到消费方传入的 evolution root 下 `skills/_drafts/<name>/SKILL.md`。

   ```rust
   use echo_agent::evolution::{SkillApprovalArtifact, SkillDraftGenerator};
   let gen = SkillDraftGenerator::new("<application-data>".into(), authority.clone());
   let preview = gen.preview_generate_from_candidate(&candidate, request_id).await?;
   let approval = SkillApprovalArtifact::new(
       approval_id, &preview.preview.operation_digest, approver, approved_at,
   );
   let result = gen.generate_from_preview(preview, approval).await?;
   // result.skill_md_path 指向生成的草稿
   ```

   Draft、Merge 与 Patch 共用同一个 prepare → projection → 幂等 audit → settle
   owner。later rollback 以 retained change/batch 为目标，对每个文件和 lifecycle
   identity 做 journal generation fencing，再写入新的 inverse batch。Rule rollback
   返回 typed `HostOwned`，framework 不从 ChangeLog 猜测 Rule 状态。见
   [ADR 0069](../adr/0069-skill-lifecycle-mutation-authority.md)。

   一个 authority 永久绑定 business `ChangeLog` 的 canonical durable destination
   identity；复制到其它路径的 marker 和无法提供该 identity 的 log 都 fail closed。
   apply/reconcile/rollback 不接受其它 log。SKILL.md 使用 canonical absolute path，`after` bytes
   必须是 UTF-8。exact bytes 只保存在 private recovery journal；business audit
   仅记录 path、hash、length 和有界的 secret-redacted summary。Curator 投影按
   Skill entity 做 merge CAS，因此不会覆盖无关 candidate insert。

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
- `ChangeLog` 保持 append-only 审计。记忆事后回滚由
  `MemoryLayerManager::preview_rollback` 与 `rollback_memory` 负责：目标可用
  `ChangeId` 或完整 `BatchId`，所有受影响 key 必须仍处于该批次最新 journal
  generation，随后写入一个持久 inverse batch。结果区分 `Ready`、`Conflict`
  和 `HistoryUnavailable`；稳定 request ID 让重试返回原 receipt。append-only
  audit 记录真实 inverse 类型和精确 before/after warm/hot 投影。merge 成员始终
  整批回滚。Skill/Rule 回滚仍属于 #54/#94/host owner，不在本记忆契约内。

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
