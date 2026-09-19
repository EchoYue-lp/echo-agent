---
schema_version: 1
id: evidence.skill-candidate-audit-verification
kind: evidence
observed_at: source:512b2adda3fbd65e8d7e3c2f4d23a036338d495ab4c3b276f09a15af58ed99f9
source_refs:
  - echo-core/src/memory/store.rs
  - echo-state/src/memory/store.rs
  - echo-state/src/memory/typed_store.rs
  - echo-state/src/memory/sqlite_store.rs
  - echo-state/src/memory/embedding_store.rs
  - src/evolution/candidate.rs
  - src/evolution/curator.rs
  - src/evolution/audit.rs
  - echo-state/src/journal/file.rs
  - echo-agent-learning/tests/example_contracts/demo51_self_improvement.rs
supports: [finding.skill-candidate-reinforcement-audit-gap, behavior.eval-evolution]
limitations:
  - Full task-branch gate, independent rereview and remote-main delivery remain pending
  - Fault injection covers deterministic audit failure and restart, not a physical power cut
---

# Issue 94 focused verification

## 支持的结论

首个 RED 在 reinforcement 后精确得到 audit len 1、预期 2。首次候选通过 10/10 后，独立复审
以 authority rebinding、非原子 CAS、Curator lineage 三项 Important 阻断。后续两轮复审又定位
public shape、derived-index CAS 与 private path alias。最终 candidate suite 18/18 通过；Curator
path identity 1/1、默认 Store CAS 3/3、sqlite feature Store CAS 4/4 通过。
demo51 candidate与external SkillMeta contract 2/2、root lib 与 sqlite feature focused clippy `-D warnings`、
formatter check、semantic strict/change-evidence 均退出 0。

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

命令在独立 `fix/Echoyue/issue-94-candidate-audit` worktree 的当前未提交候选上执行；来源限定为
candidate owner、通用 journal/audit 原语与 demo51 公共用法。

## 已知缺口

focused 证据不替代 `scripts/verify.sh`、独立复审、PR CI 或 remote-main delivery。
