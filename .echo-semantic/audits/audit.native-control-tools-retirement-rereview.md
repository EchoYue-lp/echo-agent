---
schema_version: 1
id: audit.native-control-tools-retirement-rereview
kind: audit
title: 控制工具退役候选的限定事实独立复核
lens: contract_evidence
freshness: examined
revision: source:69225014cbf5d5b28fa8e8bff9fd368792c1ad199efbb8afb55b5b688f01ac5f
boundary_ref: boundary.agent-session-turn
finding_refs:
- finding.native-control-tools-retirement
challenges:
  before_surface_invented:
    revision: 0e404185c8f252da3e335c35547e52249c74dc65
    source_refs:
    - src/agent/react/mod.rs
    - src/tools/builtin/mod.rs
    - src/agent/react/run/phases/tools.rs
    - echo-state/src/memory/conversation.rs
    evidence_refs:
    - evidence.native-control-retirement-before-source
    result: 默认仅FinalAnswer；Think不默认注册、answer为crate-private；旧按名称终态与旧Store不保签名均已诚实分开。
  preserved_contract_and_intentional_change_mixed:
    revision: source:69225014cbf5d5b28fa8e8bff9fd368792c1ad199efbb8afb55b5b688f01ac5f
    source_refs:
    - echo-core/src/llm/types.rs
    - echo-integration/src/providers/anthropic.rs
    - src/agent/react/run/native_message_contract_tests.rs
    - src/state/file.rs
    evidence_refs:
    - evidence.native-control-retained-contracts-equivalence
    - evidence.native-control-retirement-observations
    result: 六个限定不变量有source-before和actual-after支持；五组退出/改正/新增不列matched。
  whole_source_profile_or_task_digest_overclaimed:
    revision: source:69225014cbf5d5b28fa8e8bff9fd368792c1ad199efbb8afb55b5b688f01ac5f
    source_refs:
    - Cargo.toml
    - docs/adr/0101-native-message-execution.md
    evidence_refs:
    - evidence.native-control-retained-contracts-equivalence
    - evidence.native-control-retirement-observations
    result: 原open/null候选阶段没有绑定；本轮用实际官方task355a8528…c66ff绑定，专项Finding仅在具名退役范围resolved；原unoptimized/optimized与9/17输入差异保留，完整source/#10独立。
---


# 两控制工具项目删除证明专项复核

## 审查范围

独立review_contracts_transport对已批准退役的六个限定保留不变量、17个before Git来源、
14条after源观察/11去重源、实际原运行和五组有意变化进行只读复核。原结论为限定内容
pass C0/I0/M1；唯一四/五组计数Minor已作非语义修正。本轮六份正式正文和A/B结构再获独审C0/I0/M0；实际官方task绑定与完整source
仍分别留原件，不把这些专项票当全部任务通过。

## 实际实现路径与证据

真实before OID/blob、source-before行锚和after原run/log/input SHA/profile保存在
`/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.echo-semantic/local/parallel-executors-20261027/framework/native-control-retirement-proof-candidate-20261028`。普通before/after、限定Eq、historical asset与canonical执行合同分开。
Root本轮metadata-only准入允许六对象+原map导航，代码/manifest/lock/vendor/HEAD/index/
1490+35冻结；不创建Cargo或runtime reader。

## 已检查故障假设

已检查虚构旧默认Think注册、误称answer公开模块、旧名称特权被说成保留行为、旧Store
签名持久化被冒称等价、critic耗尽/Stop变化藏入matched、source/profile/task值混用。
旧Store反例据17号真实来源收窄，Signed比较改为Message/Anthropic payload；全部意图
变化单列。A/B正常record和原官方validate_task presentation保原errors与失败出口。

## 问题记录

独审最初发现reasoning持久化比较过宽，已按旧Store实际不保reasoning_blocks收窄；
最终只余一处四组/五组陈旧计数，已改并记录主模型非语义增量。首轮正式工具另外报告
正文必需章节缺失和after_revision未绑定，原失败保存在当前采用目录binding-v01原件，
本次补齐本章节结构及具体事实，不通过改验证器/排除绕过。

## 未检查项

没有before运行票，没有重新跑原native/Provider全套或新profile矩阵；未验main CI、
原生host、远端冷恢复、SDK正式新Gitpin。第10项全loaded needs_review并未专项关尽。

## 残余风险

只处理两具名控制工具退役和六个限定不变量；本轮官方task绑定/正常repairRefs按精确
当前闭包实际检验并留存版本化原件。即便第9项闭合，第10项保持阻断，不能标绿其它Asset/unknown/backlog。
本Audit不授予全source通过，Root仍统一控制提交与交付。
