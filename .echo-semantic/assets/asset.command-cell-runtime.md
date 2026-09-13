---
schema_version: 1
id: asset.command-cell-runtime
kind: asset
title: CommandCell Process Runtime
asset_type: state_authority
status: active
risk: high
observed_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
boundary_refs: [boundary.task-subagent-workflow, boundary.tool-permission-sandbox]
code_refs: [echo-core/src/tools/cell.rs, echo-orchestration/src/tasks/command_cell.rs]
consumer_refs: [src/agent/react/mod.rs, src/agent/react/run/pipeline.rs, docs/adr/0025-deterministic-command-cell-watcher.md]
behavior_refs: [behavior.task-subagent-execution, behavior.effect-permission-execution]
rule_refs: [rule.permission-effect-order]
evidence_refs: [evidence.task-subagent-workflow, evidence.effects-extensions]
finding_refs: []
candidate_refs: [asset.background-task]
---

# CommandCell Process Runtime

## 资产身份

Background command prepare/start/drain/finalize、typed snapshot/cursor 与 retained watcher 的 process runtime authority。

## 来源与消费者

Agent shell/wait/stop/list tools 和 embedding applications 消费；应用侧 delivery/addressing 不是本资产权威。

## 生命周期

Prepare/admit/start/observe/cancel/drain/finalize/shutdown，绝对 deadline 覆盖排队到 artifact finalize。

## 候选关系

CommandCell 是有 typed terminal 的进程 runtime，不与通用 BackgroundTask future handle 合并。

## 未知与限制

跨应用持久 delivery 与 owner metadata 由应用适配边界负责。
