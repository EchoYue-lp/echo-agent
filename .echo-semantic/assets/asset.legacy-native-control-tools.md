---
schema_version: 1
id: asset.legacy-native-control-tools
kind: asset
title: 已批准退役的内置Think/FinalAnswer控制工具历史身份
asset_type: protocol
status: deprecated
risk: high
observed_at: 0e404185c8f252da3e335c35547e52249c74dc65
boundary_refs:
- boundary.agent-session-turn
code_refs:
- src/tools/builtin/think.rs
- src/tools/builtin/answer.rs
consumer_refs:
- src/agent/react/mod.rs
- src/agent/react/run/phases/think.rs
behavior_refs:
- behavior.agent-turn-lifecycle
rule_refs:
- rule.turn-terminal-authority
evidence_refs:
- evidence.native-control-retirement-observations
finding_refs:
- finding.native-control-tools-retirement
candidate_refs:
- asset.agent-execution-contract
---

# 已批准退役的两内置控制工具历史身份

## 资产身份

历史ThinkTool与FinalAnswerTool是两个具名退役的控制包装，不是新的执行状态权威。
本对象按Git `0e404185c8f252da3e335c35547e52249c74dc65` 定位原两源码；当前deprecated记录批准退出，不把旧wire说成当前能力。

## 来源与消费者

原src/tools/builtin/think.rs与answer.rs和旧ReAct消费者形成控制路径；旧默认只注册
FinalAnswerTool，Think未默认注册。旧answer模块pub(crate)，think模块pub；具体OID/SHA
及源码字节保存在`/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.echo-semantic/local/parallel-executors-20261027/framework/native-control-retirement-proof-candidate-20261028/before-source-manifest.json`，17个实际Git来源可回读。

## 生命周期

旧工具接收reasoning/answer参数，旧final_answer名称能派生最终候选；本轮退出这两个
实现/模块与专属控制面。当前替代是既有ReAct原生block/reasoning与最终候选，仍经过
原schema/Guard/Critic/Stop、持久化和物理结算；自定义同名工具不继承旧名称特权。

## 候选关系

既有asset.agent-execution-contract是canonical，原生provider转换只做来源/消息适配。
本历史asset只覆盖两工具，不授权删除其它框架API，不为每个vendor成员制造symbol资产。

## 未知与限制

before只做实际Git源码trace，没有before运行票。旧ConversationStore不保reasoning_blocks，
after冷重载增强不列等价；旧critic耗尽/one-shot Stop与custom名称特权也单列意图变化。
本对象不关闭其它asset needs_review、unknown、旧402或全source资格。
