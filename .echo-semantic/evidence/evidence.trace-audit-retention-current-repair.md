---
schema_version: 1
id: evidence.trace-audit-retention-current-repair
kind: evidence
observed_at: source:3a8aba9cee4bdf94c17e2039bb9bc22ebcf6fb112bfe26ce5686002569409849
source_refs:
  - echo-core/src/utils/retention.rs
  - src/security.rs
  - echo-core/src/audit.rs
  - echo-state/src/audit/mod.rs
  - echo-state/src/audit/memory.rs
  - echo-state/src/audit/file.rs
  - src/trace/mod.rs
  - src/agent/react/mod.rs
  - src/agent/snapshot.rs
  - src/agent/react/run/pipeline.rs
  - docs/adr/0053-trace-audit-persistence-visibility.md
supports: [finding.trace-audit-secret-boundary, behavior.observation-persistence, rule.fact-projection-separation]
limitations:
  - Content redaction is not a claim that every possible secret format can be recognized
  - Typed identity, paths, hashes and numeric metadata are preserved and must be assessed separately for privacy
  - Final marker fix has focused tests and independent targeted rereview, but final-digest full gates and cross-repository consumer tests remain pending
---

# Issue 103 trace/audit retention repair candidate

## 支持的结论

公共 `ContentRetentionPolicy` 收敛了安全扫描器与 JSON/text retention；JSON key 本身也被
清洗；对象的 entry 数量复用 collection limit，key 也受字符上限约束，截断碰撞使用
原 key 的 bounded hash 区分，超限用稳定 marker 表达而不继续无界增长。只有 reserved
primary/collision key 配合 sentinel value 才识别 synthetic marker；普通字段值恰为
`[TRUNCATED OBJECT]` 仍按用户内容保留。`AuditEvent::apply_retention` 和
`RunEvent::apply_retention` 只处理承载内容的字段，不通过 JSON round-trip 截断 typed
结构。生产者在交给可自定义 AuditLogger/RunStore 之前清洗，内存和文件 backend 在接纳
边界再次应用相同策略；FileAuditLogger 与 JSONL trace 的解码/隔离错误不打印原始
不可信 payload。通用 typed ID 与工具路径为诊断寻址保留，不属于这里的 secret 文本扫描。

本策略限制内容字符串、数组项、对象 entry 与对象 key；不等价于运行总记录数、文件大小
或全局时间 retention。synthetic marker 的重复清洗保持幂等。

## 来源与范围

核心 scanner/retention 位于 `echo-core`，framework producer 与 echo-state backend 使用同一策略；
自定义 Store/Logger 的实现在其外部 consumer 负责，但 framework 生产者先清洗副本。

## 已知缺口

最终-digest 全门禁和独立复审未完成，模式扫描不构成任意 credential 格式的保证；
bounded hash 只用于同一 retained object 内碰撞消解，不是秘密摘要或跨记录 identity。
