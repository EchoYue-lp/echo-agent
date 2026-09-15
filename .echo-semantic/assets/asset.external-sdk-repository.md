---
schema_version: 1
id: asset.external-sdk-repository
kind: asset
title: External echo-agent SDK repository owner
asset_type: protocol
status: active
risk: high
observed_at: source:6c19670f1c60cd514abba3d30f6293cd385a7df18f8c355fe6fced3e4e6ab8d9
boundary_refs: [boundary.workspace-architecture]
code_refs: [README.md, README.zh.md, docs/adr/0051-extract-sdk-repository.md]
consumer_refs: [https://github.com/EchoYue-lp/echo-agent-sdk/tree/refactor/Echoyue/sdk-source-continuity]
behavior_refs: [behavior.workspace-composition]
rule_refs: [rule.framework-layer-ownership]
evidence_refs: [evidence.sdk-repository-extraction-equivalence, evidence.sdk-repository-extraction-verification]
finding_refs: [finding.sdk-repository-extraction]
candidate_refs: []
---

# External echo-agent SDK repository owner

## 资产身份

独立 `echo-agent-sdk` 仓库是 SDK Host、protocol、合同、三语言 SDK、文档、脚本和 CI 的
当前 owner；framework 只保留边界链接和 ACP/runtime authority。

## 来源与消费者

当前 source-continuity commit `b80cf068b2fb69b62913f23260980b4ce2ebf941` 已推送，保留
initial import 与 filtered framework SDK history；README、ADR0051 和 superproject 后续消费该边界。

## 生命周期

Framework extraction 后 SDK 进入 protocol purity、accepted external contract、Host pin 和
三语言 gate 收敛。

## 候选关系

该 asset 是 SDK-owned 删除路径的 canonical replacement；framework ACP adapter 只替代通用
ACP fixture/runtime 保留路径，不拥有 SDK product。

## 未知与限制

独立 SDK 的完整 runnable/parity 门禁属于后续 outcome，不由本 asset 宣称完成。
