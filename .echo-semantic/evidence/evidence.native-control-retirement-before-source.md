---
schema_version: 1
id: evidence.native-control-retirement-before-source
kind: evidence
title: 控制工具退役前的实际Git源码事实
observed_at: 0e404185c8f252da3e335c35547e52249c74dc65
source_refs:
- echo-core/src/agent/mod.rs
- echo-core/src/llm/types.rs
- echo-integration/src/providers/anthropic.rs
- src/agent/react/mod.rs
- src/agent/react/run/pipeline.rs
- src/agent/react/run/processor.rs
- src/agent/react/run/stream_channel.rs
- src/agent/react/run/phases/think.rs
- src/agent/react/run/phases/verify.rs
- src/agent/react/run/phases/finalize.rs
- src/agent/react/run/phases/tools.rs
- src/agent/snapshot.rs
- src/state/file.rs
- src/tools/builtin/mod.rs
- src/tools/builtin/think.rs
- src/tools/builtin/answer.rs
- echo-state/src/memory/conversation.rs
supports:
- finding.native-control-tools-retirement
limitations:
- 这是可恢复Git源码/旧测试定义trace，不是before测试执行。
- 旧默认仅FinalAnswer、按名称特殊终态、critic上限及one-shot Stop均诚实保留为before差异。
---

# 退役前的实际Git来源

## 支持的结论

17个真实Git blob/OID/SHA支持六个具名保留不变量的before源码trace，同时揭示退出面
及旧缺陷；它不是before测试执行证据，不是全部API/wire等价。

## 来源与范围

observed_at与before_revision定位Git `0e404185c8f252da3e335c35547e52249c74dc65`。原两控制工具、既有Agent/String API、
Message reasoning字段/JSON、Anthropic payload、schema/guard、首次Stop、File CAS及普通
ToolResult投影逐行可查。完整原字节/OID/SHA与逐行锚见`/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.echo-semantic/local/parallel-executors-20261027/framework/native-control-retirement-proof-candidate-20261028/before-source-manifest.json`
和retained-contract-matrix.json。旧ConversationStore仅reasoning_content，不保reasoning_blocks。

## 已知缺口

没有运行before Cargo；源码trace不能冒充双版本运行。旧默认仅FinalAnswer、名称终态
特权、critic耗尽接受、one-shot Stop及签名冷重载增强均在普通after观察中明确分离。
本来源证据不证明当前新test profile全套重跑、全仓closure或SDK正式Gitpin。
