---
schema_version: 1
id: asset.approved-a2a-lsp-source-retirement
kind: asset
title: 已批准A2A与LSP专用源码退出的历史身份
asset_type: protocol
status: deprecated
risk: high
observed_at: 0e404185c8f252da3e335c35547e52249c74dc65
boundary_refs:
- boundary.agent-session-turn
code_refs:
- echo-agent-learning/examples/demo23_a2a.rs
- echo-agent-learning/tests/example_contracts/demo55_lsp_tools.rs
- echo-core/src/lsp/client.rs
- echo-core/src/lsp/mod.rs
- echo-core/src/lsp/types.rs
- echo-integration/src/lsp/client.rs
- echo-integration/src/lsp/config.rs
- echo-integration/src/lsp/jsonrpc.rs
- echo-integration/src/lsp/manager.rs
- echo-integration/src/lsp/mod.rs
- src/a2a/auth.rs
- src/a2a/client.rs
- src/a2a/mod.rs
- src/a2a/serve.rs
- src/a2a/server.rs
- src/a2a/types.rs
- src/lsp.rs
- src/tools/lsp.rs
consumer_refs: []
behavior_refs:
- behavior.agent-turn-lifecycle
rule_refs:
- rule.turn-terminal-authority
evidence_refs:
- evidence.native-control-retained-contracts-equivalence
finding_refs:
- finding.native-control-tools-retirement
candidate_refs:
- asset.agent-execution-contract
---

# A2A/LSP具名源码退出的历史定位

## 资产身份

本对象只证明已批准18个专用源的before身份及当前源树退出，不新增A2A/LSP执行权威
或证明它们与canonical ReAct功能相同。原20路径集合中的think/answer仍由原历史asset
承载，合理Framework API、多Store、ACP/MCP/channels/Subagent及运行时Final通知保留。

## 来源与消费者

所有code_refs仅按observed_at的真实0e4 Git定位，当前均已删除；Blob/SHA由git show/
rev-parse实际取得，没有before运行票。审批为Root既有F30/Frame15准入与ADR0090，
不是新的人类风险接受或未来删除通行证。SDK/CLI消费者与正式pin资格另行维持。

| 路径 | Git blob | SHA256 |
| --- | --- | --- |
| echo-agent-learning/examples/demo23_a2a.rs | 8a9798bd9d33a9372e0fe0eb577711ff6e66afeb | c75c1d9b696846fb9483c551e6a592b13382b23f4e65775bbc524d6c1ae891a0 |
| echo-agent-learning/tests/example_contracts/demo55_lsp_tools.rs | ffd3e74af4d77248ef5b7d533b0585a8ecff12e9 | 2643811df66d5c3c6a7a824e0268b09ff8ceb2cd6c2789a3e3e8e53c445abdbd |
| echo-core/src/lsp/client.rs | 29a16cd3ea49252c8686abc25bdb302c6b975c4d | 8466cea8ca5fed181f8b5bbaec90f10bc28b44747caf12284c2c69b9238cd6b4 |
| echo-core/src/lsp/mod.rs | a127bbbd66cbe61d5d4ffd09ea8ad106a06b25cc | 0c976d43bc8296d11c053e36f2161f653c2ea90008ba7582dc19f074c5df0a62 |
| echo-core/src/lsp/types.rs | 6ba33914a8854963d63ad7d5741d5b51f3ee0b31 | a8cbfaaf56a5e5661b8af243eb01436f6de79247cf8831d90f5734415bfc3fdb |
| echo-integration/src/lsp/client.rs | 410c882cd1e00748dab3a03f3c1cbc6f681bcf01 | a5e1da637ace5f3793478358fbdf0019116aad49433c2e6e7bf8ab53c01b49e7 |
| echo-integration/src/lsp/config.rs | 4995f502ba35d943a84b23c5d758d3dd1bb5e41b | 32ebd699108aec4a2381141065cc85a83fb9f904b499d4c3a50564464f20e21d |
| echo-integration/src/lsp/jsonrpc.rs | e33da9d46606672d433a67c3bb08d97295504e61 | a7588fe7aeaca1e7dc2c945b60a1c8dfcf197912ce806492f11790a0a248e350 |
| echo-integration/src/lsp/manager.rs | 21bc67973cbe66b94dc7ea9c62ef801eff190476 | ad7495db9c11e77c7df39051f0308ed6758ed46d7c2769f2b28cb40b93c9d129 |
| echo-integration/src/lsp/mod.rs | b3222d4f4d03b8ae227ae5f3b4e3363289e35043 | bce696bd4eddd1cb201a17c8106f1be8691dd876b04c91bd6c05f45ca420d635 |
| src/a2a/auth.rs | cf357d667b1fb823aaac089bdd1613c6d3e939ef | 20e0cf8ecaf8233a6504f39417b6680b3310ded2760869b2606eeffa0edd82b6 |
| src/a2a/client.rs | 334118fbe7d931a304e7b1c4a6399b75da132102 | 6f68182ea3901c02e15ef8bc407e53e2c48b1d00d26e0dd1a06c1363e38e8a09 |
| src/a2a/mod.rs | 4cd6c1b36647d8299a7c6ebd8d117aaa5cbe13b4 | a736b7106eb3368d76f3c3bc8e2f104d4b997110292c378c730277057c18b0e6 |
| src/a2a/serve.rs | ebbbddef285f9da8db336fb04e2541622e946592 | b701566a2c6507196cccb575a5371b349cdc5c1064418fdfce606108c8d57c46 |
| src/a2a/server.rs | a13f1870974d306df7cbf8600c1accb4fb2fa330 | 9ac32c009c53e8180494b6c0d290f6aab33221c960e555578dabf75cbc92e980 |
| src/a2a/types.rs | 096e1266189e5e5a4a36f22cfc81ceef450105f7 | e2e7a5dfabc8a541a0655c9abacec0426813d8b19f6ee563439f692382ced663 |
| src/lsp.rs | 63d56e41d84087e745b5d87d39c092f1ef7df8ca | 071a397ee84d7674dffa24f11d89f7d6bb201bedb955b15527c17d4d078d8108 |
| src/tools/lsp.rs | fab82bece207af3b4694772a1306cb68b8dc59f0 | 530719d2c20c140414020c61d522e788b108b2b8429addb10a2e0658d9369a65 |

## 生命周期

当前同源3957顶层测试+2子控制、原Clippy/strict及限定feature/依赖票支持保留功能的
已运行范围；两个旧feature实际缺失拒绝不等runtime全API等价。这里不做兼容shim或
旧来源猜测，不改变其它Asset的unknown/needs_review或外部#10关联限制。

## 候选关系

既有asset.agent-execution-contract只标继续保留的通用执行合同；source准入集合
不把A2A/LSP两种专用协议归并成它的新功能。

## 未知与限制

没有新before执行、SDK正式newGitPin/native/MainCI结果。新NoCompat后继旧wrapper/
schema/control-history删除未在本维护切点实施；保存原日志、恢复包、tag和用户数据。
