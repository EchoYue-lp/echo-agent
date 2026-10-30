---
schema_version: 1
id: evidence.factory-background-driven-source-review
kind: evidence
title: 三项当前公共菜单的限定源码接管
observed_at: source:23e96814f826416af04c0771bde28c0bdf6eafb49fa05eda686b5a42e152b636
source_refs:
- echo-core/src/agent/factory.rs
- src/agent/default_factory.rs
- src/agent/subagent/registry.rs
- src/agent/subagent/executor.rs
- echo-orchestration/src/tasks/background_task.rs
- echo-orchestration/src/tasks/background_state.rs
- echo-orchestration/src/tasks/mod.rs
- echo-orchestration/src/runtime/turn_driver.rs
- echo-core/src/agent/event_envelope.rs
- src/channels.rs
- src/eval/runner.rs
- docs/zh/29-long-running-tasks.md
- docs/adr/0039-background-task-terminal-authority.md
supports:
- asset.agent-factory-contracts
- asset.background-task
- asset.driven-turn-authority
limitations:
- 这是普通source-review Evidence；当前来源由原caller实际输出绑定，内容身份不代替后续正常source/final或全任务完成。
- 独立source review、R20历史运行与R15当前工程票分列；本页不产生专门runtimeverification/Audit或全生命周期、native、外部服务、当前SDK及全局闭合资格。
- Background只有进程内handle及公开DTO/调用方存储trait；不声明框架自动保存或重启恢复Future。
- 原Discovery长期命名/整合未知保留；其它五个父Asset及其源码问题另按真实修复、运行和复审接管，不由本页关闭。
- 两项SDK consumer已按Root精确授权由独立历史Evidence承接原Framework身份与当前SDK同role caller；迁仓不证明当前SDK gate、新Framework pin或远端发布已通过。
---

# 当前代码身份更新

当前来源为原未改 verifier.source_digest(F) 实际返回的 `source:23e96814f826416af04c0771bde28c0bdf6eafb49fa05eda686b5a42e152b636`；不是 49d 工程 manifest 或自算 task。正式目录原24命令已全部自然0、源码无漂移；full05为4262外层通过／0失败／4忽略，93个Cargo运行目标，原78及旧default-path合79个具名用例各一次ok；独立learning doctest4通过、NoDefault和15单feature均通过。原official cargo-deny0.20.2四类自然0，原caller恰49组107身份、policy14通过；lock1442/baselinea5a6及风险合同不改。这些是当前工程范围，不能代签SDK accepted/Host/三语言、新pin、EKO/native、SSH/远端平台、性能、Git/mainCI或整个Discovery。

已采用的三菜单完整角色、caller-owned background与SDK独立迁仓历史关系不改。本次只随当前正式代码稳定后更新正常来源；旧f8观察/原代码源审范围在下文保留，不倒签旧工程或消费者。

# 既有菜单及分范围记录

# 三项当前公共菜单的限定源码接管

## 支持的结论

Factory同步config-based构造与异步registry factory、get_agent缓存与dispatch fresh-instance合同独立；当前记录代次及caller deadline/第三方副作用边界明确，保留两种公共API，不新增接口或合并执行器。

Background进程内Future handle的status、take-once result、取消/超时与原child结算有既有修复和具名观察；公开状态DTO/抽象checkpoint trait是供调用方使用的独立菜单。源码未发现内置store实现、TaskSpawner持久化writer或restart/resume装配，当前框架不自动恢复Future，不能据DTO状态签物理完成。

Driven标准AgentChannelHandler已接原driver；raw Agent API及自定义MessageHandler仍是独立合同。FinalAnswer通知自身非terminal，driver receipt分别表示producer结算和sink接纳，Delivered不代表外部网络ACK。此项纠正父文案中的标准Channel陈旧表述，不把所有consumer/native一并标通过。

## 来源与范围

本次当前来源为 `source:f8a2f672e77ee87c38d36a30a22dc7008a2c1a2f291943c1650341af8729d42f`，由已安装且未改的原校验器 `source_digest(root)` 在 Framework 实际根返回；task为 `framework-native-current-integration-20261029`，拥有者为 `01a17d6c-d8ad-7900-869b-ee9e6a559473`。该来源不是工程manifest，绑定和正常preflight/引用/source/final结果分别留原件。资产旧observed_at与Evidence/Audit仍保历史范围；当前菜单声明只按本次来源负责。

准备输入为Root已实读的R23完整manifest e3852f08／2836成员及所指green.root，工程Rust与R20/39a相同。独立报告“R23：Factory、BackgroundTask与Driven Turn独立源码复核”（reviewer /root/frontend_milestone_review）已由Root索引00393af0登记，报告SHA为011c728e70cf0c9fd25e6987f50f0e012272170e98f17e0764b8cb27209f64b1。报告结论source-only C0/I0/M1，唯一Minor为Driven父文案陈旧。r01四候选由/root/quality_asset_review在three-asset-candidate-review-r01报告（SHA a102f88873437eaf265ef5c1b6492a87c65bb5ef5439cd44adb01a92f1ad2beb）独立复审，内容表达层C0/I0/M0；这是不同于原源码reviewer的材料复核，不是新运行或正式Audit。r02两条迁仓引用及历史关系已由同reviewer在 three-asset-candidate-review-r02 报告（SHA b48a8db69b2fc2806103fb300ca324617a22ba0469e62c2c69227113abd9cf1e）定向source/材料复审，C0/I0/M0；本次再绑定真实来源并核完整相对增量，正常机器验证结果单列。

既有原链包括evidence.subagent-factory-singleflight-verification及audit.subagent-factory-singleflight-rereview、evidence.background-task-terminal-authority-verification及audit.background-task-terminal-authority-rereview、evidence.channel-driven-turn-verification及audit.channel-driven-turn-rereview。它们各保原revision/feature/范围，不将历史resolved数量推为父层全闭合。

原R20 heavy-05-core-5的具名raw已回读，原caller自然0、未截断。以下只是对应fixture的原实际ok，不是本次新运行，不相加冒全矩阵，不证明真实网络或第三方资源清理：

- Factory：`agent::default_factory::tests::test_factory_build_agent_with_mock`（原raw:750，ok）；`agent::subagent::registry::tests::cancelled_factory_attempt_allows_next_resolution`（原raw:1424，ok）；`agent::subagent::registry::tests::factory_result_is_published_before_another_creation_can_start`（原raw:1425，ok）；`agent::subagent::registry::tests::stale_factory_result_does_not_replace_new_registration`（原raw:1428，ok）。
- Background：`tasks::background_task::tests::concurrent_waiters_both_reach_a_terminal_observation`（原raw:4001，ok）；`tasks::background_task::tests::execution_cancel_waits_for_child_task_drop`（原raw:4002，ok）；`tasks::background_task::tests::queued_cancellation_settles_without_starting_the_future`（原raw:4005，ok）；`tasks::background_task::tests::zero_concurrency_settles_as_configuration_failure`（原raw:4018，ok）；`tasks::background_state::tests::test_background_task_state_mark_completed`（原raw:3997，ok）。
- Driven：`channels::tests::channel_handler_replies_only_after_driven_turn_completion`（原raw:1486，ok）；`channels::tests::channel_sink_failure_is_separate_from_the_producer_terminal`（原raw:1489，ok）；`channels::tests::channel_turn_exposes_one_driven_receipt_with_identity_and_usage`（原raw:1491，ok）；`channels::tests::session_reset_cancels_a_real_channel_turn_before_cleanup`（原raw:1496，ok）；`runtime::turn_driver::canonical_turn_kernel_contract_tests::canonical_final_answer_notification_is_not_a_turn_completion`（原raw:3871，ok）；`runtime::turn_driver::tests::native_source_terminal_delivery_waits_for_original_raw_eof_before_receipt`（原raw:3894，ok）。

原raw SHA为a0fd480f8fd7933d42d038bc015e0d2f4a5a4f499f0876d2512ac107b111c2f4。它与报告/完整manifest只作为本工作区可审原件定位，在本目录取件说明记录绝对路径；不把ignored路径当source_refs，也不把manifest SHA当正式observed_at或task值。

## 当前工程观察（与历史原件分列）

最终R15原24工程命令在同一冻结源上全部自然0，原full05外层4215 passed/0 failed/4 ignored，learning doctest4 passed；它们是独立工程票，不将R20历史原件改称当前重跑。与本页18条当前code/consumer/source引用相关的文件逐SHA均与已审R23一致，Source-only复审在该不变闭包内复用；未涉及其它五父Asset的新源码增量。当前原件位于本工作区 `.echo-semantic/local/source8-repair-validation-20261030/`，工程qualified SHA `cd912e627cbc1c662bf2522ff1d22237a41a1b3e6434d1906080d4113a534483`；源码身份仍使用本页observed_at，不使用该manifest或qualified SHA。

## 已知缺口

Source review只覆盖当前菜单及已核调用/owner边界；第三方factory自建任务/补偿、递归resolve、真实provider、跨进程强杀、Background全shutdown/owner drop/长期高并发、外部IM网络ACK、完整SDK pin/Host/native和远端main继续未验。公开checkpoint长期关系及Factory长期命名仍由原Discovery保留，不能通过active文案消除。

三项资产即使由正常流程正式接管，也不会关闭其它五资产的源码问题接管、前七项新删除合同或删除聚合中的Discovery unknown。若发现当前真实动态调用/owner未审或违反已确认承诺，继续needs_review/对应Finding处理，不能改称unsupported凑绿。

本页没有行为等价scenario_results，不建立新的运行verification、Audit或rereview收据；候选的独立复审、相对R23增量、当前来源绑定及历史/外仓职责关系须由正常合同实际核验。单份材料结构通过不代表整个source/final、删除合同或消费者验收已经完成。

### Background 两项迁仓关系

本仓当前consumer列表仅移出已证迁仓的两条旧SDK路径，完整原9d Git/blob与当前SDK@3fcb8d066e8a6b6560d7991f06b4255121f2a49b同role/父caller由evidence.background-sdk-consumer-relocation-history承接。该独立历史Evidence只在自己的observed_at=原Framework9d下解析两条旧source_refs；SDK路径、精确commit与链接只放正文，不伪装成Framework当前source或inline版本语法。旧Evidence/Audit原事实及Background其它所有refs、风险和Discovery未知不变。

同role不代表旧整库scope计数和当前accepted合同门禁完全等价，也不代表SDK新Framework pin/Host/三语言已验。若后继正常机器连续性要求额外当前职责关系，须由两仓唯一writer在各自source身份下通过原合同表达；不能为消除缺引用恢复旧SDK目录、删旧义务或造新的runtime verification。
