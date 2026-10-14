---
schema_version: 1
id: evidence.acp-turn-steering-identity-repair
kind: evidence
observed_at: c87c5ec9e6fdb02fabcd8e59e06a662da671c12c
source_refs:
- src/acp/runtime.rs
- echo-core/src/agent/mod.rs
- echo-orchestration/src/runtime/turn_driver.rs
- tests/acp_agent_adapter.rs
- docs/adr/0046-turn-execution-delivery-settlement.md
- docs/adr/0061-acp-recovered-turn-identity.md
- docs/adr/0084-acp-turn-steering-identity.md
- docs/evidence/acp-turn-steering-identity/README.md
- docs/evidence/acp-turn-steering-identity/review-ready-source-manifest.json
- docs/evidence/acp-turn-steering-identity/evidence-sha256.json
- docs/evidence/acp-turn-steering-identity/fixed-runtime-capture-tests-run.json
- docs/evidence/acp-turn-steering-identity/after-core-default-run.json
- docs/evidence/acp-turn-steering-identity/after-text-only-acp-run.json
- docs/evidence/acp-turn-steering-identity/final-core-lib-run.json
- docs/evidence/acp-turn-steering-identity/final-existing-driver-structured-contract-run.json
- docs/evidence/acp-turn-steering-identity/recording-fixture-corrected-all-acp-adapter-run.json
- docs/evidence/acp-turn-steering-identity/current-all-acp-lib-run.json
- docs/evidence/acp-turn-steering-identity/final-affected-lib-clippy-run.json
- docs/evidence/acp-turn-steering-identity/final-workspace-fmt-run.json
- docs/evidence/acp-turn-steering-identity/final-workspace-fmt-check-run.json
- docs/evidence/acp-turn-steering-identity/before-chat-fallback-core-agent.rs
- docs/evidence/acp-turn-steering-identity/before-fix-runtime-with-capture-tests.rs
- docs/evidence/acp-turn-steering-identity/before-lower-fallback-with-tests.rs
- docs/evidence/acp-turn-steering-identity/before-recording-fixture-pure-text-delegation.rs
- docs/evidence/acp-turn-steering-identity/before-runtime.rs
- docs/evidence/acp-turn-steering-identity/before-text-only-regression-acp-tests.rs
supports:
- behavior.protocol-projection
- behavior.agent-turn-lifecycle
- rule.turn-terminal-authority
limitations:
- This is the bounded committed framework candidate c87c5ec9, not framework main or final combined-source delivery.
- Native Host eb00038a proves the earlier runtime-only fix; it predates the final legacy fallback and RecordingAgent fixture correction.
- ACP callers implementing both legacy text and structured overrides can observe the existing structured dispatch and different output; no all-third-party output equivalence is promised.
- Full workspace and conditional gates, telemetry integration, combined Host/native acceptance and exact SDK pin/main delivery remain separate.
- No accepted SDK identity membership, new Rust public API obligation or five-second physical-settlement policy is inferred from this repair.
---

# ACP canonical Turn 传递与精确 legacy text fallback 修复

## 支持的结论

本次观察绑定真实提交 `c87c5ec9e6fdb02fabcd8e59e06a662da671c12c`，基于已发布 `be5f0a8405b593c4ef964cd6fd2df4beabf4956c`。原 ACP Session lease 已分配 Turn，`AcpConnectionServices::prepare_run` 将同一个身份传入原 `AgentInvocationContext.runtime.turn_id`。RunStartSpec.run_id、TurnRequest 与 active Turn 不符或调用方 runtime.turn_id 冲突时，在 registry publication 前明确失败；匹配值和调用方 TaskRun run_id、lineage、工具、历史、cwd、取消与 guards 保留。没有第二身份映射、TaskRun、scheduler 或终态权威；原 AgentTurnDriver/TurnReceipt 与 ADR 0046/0061 不变。

提交中的直接源摘要与取证清单一致：

| 源路径 | SHA-256 |
| --- | --- |
| src/acp/runtime.rs | bee15cec31068a0dc384e81bd1690558853f2ccf6192b38f57f738f869ffda1f |
| echo-core/src/agent/mod.rs | 480c01934ca407a2d3d0577745ed97c1e8cff199af3452c4cc3aa964259ec2c6 |
| tests/acp_agent_adapter.rs | 442a15bd80516985e4484baf23c71ac3aa85e0b85a6d80e2ccccd4a2fcd638d4 |

原 Text Chat + invocation 分支继续调用既有 structured 方法。精确 legacy fallback 只在原 lower default `Agent::chat_stream_message_with_cancel` 处理 Role::User、MessageContent::Text，且五项可选 metadata 均为 None。Some(empty) 仍为存在，不可约成缺失；非 User、Parts/ResourceLink/Empty 或 metadata-bearing 消息继续明确 unsupported，不做 as_text 扁平化。已有 structured override 优先；fallback 保有原文字至 stream poll，原 setup error 成为原 stream error，再由唯一 driver 分类 Failed，不包装为成功。

## 来源与范围

128 项正式取证文件的 checksum 清单 SHA 为 `8adad776c6dc8e29585c5f88e4312bfb61d08fa5c7d2576e7989649b8ef43e84`。本次只读逐项核对原字节、提交中的三份源及正式 receipt；没有重跑编译或业务测试。原失败、准备错误及中断尝试均保留，不能被后继成功覆盖。

| 明确场景与原命令切点 | 实际结果 |
| --- | --- |
| 原 production 的 Agent context capture | 0 pass / 5 fail：实际 Agent invocation 缺 reserved Turn |
| 修复后身份 capture | 8 pass：Chat/Execute、text/structured、原调用方字段与发布前冲突拒绝 |
| 原 lower default core | 2 pass / 2 fail：legacy owned text 与原 setup error 缺口 |
| 修复后 lower default core / text-only ACP | 4 pass / 2 pass，均零失败或忽略 |
| 完整 echo_core library | 422 pass，零失败/忽略 |
| 原 driver structured-contract 用例 | 1 pass，保留既有 dispatch 合同 |
| 首轮完整 ACP adapter / fixture 修正后同命令 | 原 17 pass / 9 fail 保留；后 26 pass，零失败/忽略 |
| 当前 ACP library 子集 | 38 pass，零失败/忽略 |
| 受影响三个 library 严格 Clippy | exit 0，warnings、unwrap/expect/panic/unreachable 均按原命令拒绝 |
| workspace fmt 与 fmt --check | 两条原命令各 exit 0、零输出，三份已审源 hash 不变 |

RecordingAgent 的原取消、barrier、tool、failure 脚本原先只在 legacy 方法，structured override 有不同结果。审查接受的 fixture 修正仅让其精确纯 User Text 复用原脚本，原断言与 timeout 未改；这不是许诺任意第三方两个 override 输出相同。ADR 0084 已明确这项可观察 dispatch 变化和失败进入 stream 的时点。提交前父任务派发的独立源复审为 PASS、零 Critical/Important/Minor；该结论只覆盖清单中的实际源与精确兼容义务，不是全项目无缺陷声明。

六份 before-*.rs 原件是同一128项取证包里的受控源码/测试快照：before-chat-fallback-core-agent.rs、before-fix-runtime-with-capture-tests.rs、before-lower-fallback-with-tests.rs、before-recording-fixture-pure-text-delegation.rs、before-runtime.rs、before-text-only-regression-acp-tests.rs。它们各自保留原反例/夹具切点，不是新增运行入口、第二业务权威或完整旧仓重建；source_refs 逐文件列出用于原始证据可追溯，不能把其 public 声明推成新接受接口。

## 三语言诊断证据的旧 epoch

原正式 be5 Host `bcb0389ae8d8a946fab9bb123945100722711029bafa8d9d6cadf95a76c9c345` 在原 HTTP request 屏障处使公开 Run.steer 拒绝 canonical 身份。三个语言在诊断 Host `eb00038a0be7977f936514c7df8e06e329928f6735d466f292f10201a38abbf5` 各通过：accepted/nonempty steer ID、第二份真实模型请求含 Unicode steering、原 user/draft、两个并发 wait 得同 receipt/terminal、原 Run/stream 与单一 final，以及 late/empty 拒绝。

该诊断二进制使用独立 workspace/lock 的八个临时 path patch，早于最终 lower-default 兼容修正及 RecordingAgent fixture 调整。上述 native 成功只证明前一 runtime 修复机制，不是本提交完整兼容路径或后继 telemetry 组合的实际 Host 验收；最终发布、精确 pin 和新完整 native gate 仍由主任务收口。

## 已知缺口

ADR 0084 和原 ADR 0061 已记录身份及兼容边界。本元数据切片只新增本 Evidence，未改 runtime、ADR、归档、examples、website、branch、index 或 baseline；没有新增公开接口需要新示例。已有示例仍属最终可执行门禁。能力/Rule 的 needs_review 状态、12 项接受身份选择和物理结算选择均不因本局部修复提升或自动裁决。
