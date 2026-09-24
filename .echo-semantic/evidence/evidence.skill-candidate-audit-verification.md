---
schema_version: 1
id: evidence.skill-candidate-audit-verification
kind: evidence
observed_at: 733d352fc719f922b21bab1cd46206139564367f
source_refs:
  - echo-core/src/memory/store.rs
  - echo-state/src/memory/store.rs
  - echo-state/src/memory/typed_store.rs
  - echo-state/src/memory/sqlite_store.rs
  - echo-state/src/memory/embedding_store.rs
  - echo-state/src/audit/mod.rs
  - echo-execution/src/sandbox/docker.rs
  - src/evolution/candidate.rs
  - src/evolution/curator.rs
  - src/evolution/audit.rs
  - echo-state/src/journal/file.rs
  - echo-agent-learning/tests/example_contracts/demo51_self_improvement.rs
supports: [finding.skill-candidate-reinforcement-audit-gap, behavior.eval-evolution]
limitations:
  - Fault injection covers deterministic audit failure and restart, not a physical power cut
  - Skill promotion, approval, file mutation and later rollback remain Issue 54
---

# Issue 94 focused verification

## 支持的结论

首个 RED 在 reinforcement 后精确得到 audit len 1、预期 2。首次候选通过 10/10 后，独立复审
以 authority rebinding、非原子 CAS、Curator lineage 三项 Important 阻断。后续两轮复审又定位
public shape、derived-index CAS 与 private path alias。最终 candidate suite 18/18 通过；Curator
path identity 1/1、默认 Store CAS 3/3、sqlite feature Store CAS 4/4 通过。
demo51 candidate与external SkillMeta contract 2/2、root lib 与 sqlite feature focused clippy `-D warnings`、
formatter check、semantic strict/change-evidence 均退出 0。

最终冻结代码通过两套 workspace Clippy、all-target/all-feature workspace tests、no-default
workspace check 与 17 项独立 feature matrix。第四轮独立实现复审为 0 Critical、0 Important、
0 Minor；本 Evidence 更新未改变生产代码或测试语义。

PR #143 首轮 Linux foundations 暴露 `tracing-core 0.1.36` 已知的单 Dispatch callsite
interest-cache race（tokio-rs/tracing#3611）：后台 diagnostic thread 可在 Capture thread 前把
静态 error callsite 缓存为 never。测试 helper 采用 upstream issue 的 multi-Dispatch workaround，
并加入无 subscriber thread 先触发同一 callsite 的确定性交错回归；该变化只稳定测试观察，
不改变 production diagnostic delivery。

第二轮 Linux foundations 暴露 Docker fake 依赖外部 `sleep` 的环境敏感性：命令解析失败时
fake 会在 100ms control timeout 前退出。fixture 改用 POSIX shell builtin 无限循环，并在
断言中携带实际错误。目标测试在正常环境及 `PATH=/definitely-missing` 下均通过，完整
`echo_execution` 325 项测试通过；production Docker timeout/error contract 未改变。

测试覆盖 create/reinforce stable audit、A journal 对 B Store/ChangeLog 重绑拒绝、默认 Unsupported
Store 拒绝、read/CAS 之间注入外部更新仍保留外部值、Curator missing 恢复、同 lineage
Draft/Active 保留及无 lineage 同名冲突。原有 audit restart、observer、unknown external payload
与 no-growth Store/journal/audit 零变化测试继续通过。

第二轮独立复审发现 public `SkillMeta` shape 与 EmbeddingStore half-atomic CAS 两项 Important。
修订候选恢复原 `SkillMeta` struct literal，以 Curator 私有 sidecar 保存 lineage；外部 learning
crate 编译 contract 覆盖该 shape。EmbeddingStore 继承 default Unsupported，focused 测试验证失败
前后 payload 与 vector index 均不变化。

同 stem 不同 extension 的 Curator 顺序与并行回归均通过；直接路径合同确认 operation journal、
lineage sidecar、lock 与 state temporary file 使用完整文件名 suffix 且互不别名。

## 来源与范围

命令在独立 `fix/Echoyue/issue-94-candidate-audit` task branch 上执行；来源限定为 candidate
owner、通用 journal/audit 原语与 demo51 公共用法。

## 已知缺口

PR #143 七项 CI 全绿，并以 GitHub verified squash commit `d0d1e975` 进入远端 main。
post-merge closure rereview 未发现 Critical、Important 或 Minor 问题。该证据关闭 candidate
create/reinforcement 的 mutation/audit 缺口；Skill promotion、approval、file mutation 与
later rollback 仍由 Issue #54 负责。
