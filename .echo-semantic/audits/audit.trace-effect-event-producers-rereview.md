---
schema_version: 1
id: audit.trace-effect-event-producers-rereview
kind: audit
boundary_ref: boundary.observation-persistence-delivery
lens: contract_evidence
freshness: examined
revision: source:8aeadcf34eb574d68a0fdd14a61337195d71c9a6b8cc6cc2d5bc387b168de3fe
finding_refs: [finding.trace-effect-event-producers]
challenges:
  event-producer-coverage:
    revision: source:8aeadcf34eb574d68a0fdd14a61337195d71c9a6b8cc6cc2d5bc387b168de3fe
    source_refs: [src/trace/mod.rs, docs/en/27-tracing.md, docs/zh/27-tracing.md, docs/adr/0059-observed-tool-effects-and-background-dispatch.md]
    evidence_refs: [evidence.trace-effect-producers-current-repair, evidence.trace-effect-producers-current-verification]
---

# Trace event producer contract rereview

The integrated snapshot has an exhaustive 17-variant discriminator test and an
authoritative producer matrix. Permission, file, test, error, and subagent
events are tied to real producer paths; generic shell output is not interpreted
as a file or test effect.

The focused producer/order tests and independent rereview found no new framework
blocker. Process-abort recovery for detached background dispatch remains the
separate #38/#61 embedding-owner responsibility.

## 审查范围

审查 17 个 RunEvent discriminator、真实 producer 路径、effect-before-PostToolUse
顺序、Eval TestRun 和 background Subagent terminal 投影。

## 已检查故障假设

- producer 只存在于 enum/context factory 而不在生产路径；
- generic shell 被错误推断为 FileEdit/TestRun；
- launch ack 被误记为 Subagent terminal；
- detached background dispatch 被本 Finding 重造为第二持久权威。

## 实际实现路径与证据

exhaustive contract test、producer matrix、typed effect ordering tests 和 ADR 0059
共同证明真实 producer 与不推断副作用的边界。

## 残余风险

进程 abort 后的 background recovery 仍由 #38/#61 embedding owner 负责。

## 问题记录

本次复审未发现 framework blocker。

## 未检查项

未执行真实外部进程 abort recovery 或跨仓库 consumer 验证。
