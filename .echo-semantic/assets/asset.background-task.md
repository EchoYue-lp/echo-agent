---
schema_version: 1
id: asset.background-task
kind: asset
title: 进程内 BackgroundTask 与公开快照菜单
asset_type: state_authority
status: active
risk: high
observed_at: source:23e96814f826416af04c0771bde28c0bdf6eafb49fa05eda686b5a42e152b636
boundary_refs:
- boundary.task-subagent-workflow
code_refs:
- echo-orchestration/src/tasks/background_task.rs
- echo-orchestration/src/tasks/background_state.rs
- docs/adr/0039-background-task-terminal-authority.md
- echo-orchestration/src/tasks/mod.rs
consumer_refs:
- docs/en/29-long-running-tasks.md
- docs/zh/29-long-running-tasks.md
behavior_refs:
- behavior.task-subagent-execution
rule_refs: []
evidence_refs:
- evidence.task-subagent-workflow
- evidence.background-task-terminal-authority-repair
- evidence.background-task-terminal-authority-verification
- evidence.factory-background-driven-source-review
- evidence.background-sdk-consumer-relocation-history
finding_refs:
- finding.background-task-wait
candidate_refs:
- asset.command-cell-runtime
- asset.task-graph-authority
---

# 当前代码身份更新

当前来源为原未改 verifier.source_digest(F) 实际返回的 `source:23e96814f826416af04c0771bde28c0bdf6eafb49fa05eda686b5a42e152b636`；不是 49d 工程 manifest 或自算 task。正式目录原24命令已全部自然0、源码无漂移；full05为4262外层通过／0失败／4忽略，93个Cargo运行目标，原78及旧default-path合79个具名用例各一次ok；独立learning doctest4通过、NoDefault和15单feature均通过。原official cargo-deny0.20.2四类自然0，原caller恰49组107身份、policy14通过；lock1442/baselinea5a6及风险合同不改。这些是当前工程范围，不能代签SDK accepted/Host/三语言、新pin、EKO/native、SSH/远端平台、性能、Git/mainCI或整个Discovery。

已采用的三菜单完整角色、caller-owned background与SDK独立迁仓历史关系不改。本次只随当前正式代码稳定后更新正常来源；旧f8观察/原代码源审范围在下文保留，不倒签旧工程或消费者。

# 既有菜单及分范围记录

# 进程内 BackgroundTask 与公开快照菜单

## 资产身份

TaskSpawner/BackgroundTask future handle、公开 BackgroundTaskState DTO 和 BackgroundCheckpointStore 抽象 trait 是相邻但独立的公共能力菜单。私有 BackgroundTaskHandleState 只拥有进程内 handle lifecycle；DTO 可序列化或调用 mark_completed 不证明真实 Future/OS 进程完成。

## 来源与消费者

本仓 Public framework API、文档与 tests 继续消费；没有 root Agent 或 EKO 构造点不构成删除依据。当前本仓 consumer_refs 保留原双语文档；tasks/mod.rs 只补录公共入口，不充当 SDK inventory 或语言 gate。两项 SDK consumer 已迁独立仓，其原 Framework Git/blob、当前 SDK 精确来源、职责及真实父 caller 由 evidence.background-sdk-consumer-relocation-history 单列承接，见下节；限定 source review 与原 handle/DTO 运行范围见 evidence.factory-background-driven-source-review。

## 生命周期

Spawn/admit/run/cancel/deadline/wait/panic/terminal/drop 继续由原 handle state 原子保存 status、单消费者 result 与 typed panic provenance；Clone 只共享观察和取消 scope。取消/执行超时等待原 child abort/join 后发布终态；首个终态 waiter 取 T，其余从已提交终态立即返回，不等待第二个结果。

公开 checkpoint trait 提供由调用方实现的 save/load/load_all/delete 存储菜单；当前未发现内置实现、TaskSpawner writer 或重启恢复装配。框架不自动序列化或恢复 Future；调用方保存 DTO 不等于框架恢复执行。需要持久 Task graph 时沿原 revisioned Task 服务，不新增 store、第二 scheduler 或执行器。

## 候选关系

BackgroundTaskState checkpoint、revisioned Task graph 和 CommandCell typed process lifecycle 保持不同 owner，不自动归并。当前公开菜单及“不提供自动 Future 恢复”边界已有源码复核；长期整合/定位仍在原 Discovery unresolved 中，本次不删除或关闭。

## 未知与限制

Handle terminal authority 已有原 repair、verification 与 rereview；其原证据及旧 observed_at 9d1f3f2b5fdc204c08ecdec32ed22e8df95870e9 的历史身份不随本次文案改称重跑。本次 observed_at 来自原 source_digest(root) caller 的当前内容身份；正常来源验证与全任务完成资格分别记录。两条 obsolete 的 Framework current consumer 路径已按 Root 精确授权迁入独立历史 Evidence；其原职责、身份和当前独立 SDK caller 均保留，不把迁仓误写成责任取消或当前 SDK 运行通过。

本轮仅接管当前进程内 handle 与公开 DTO/抽象存储菜单的 source-reviewed 边界，不声明内置 checkpoint 持久化或 restart recovery 已实现。supervisor JoinHandle 被 drop 后继续运行，当前没有统一 shutdown/restore API；任意第三方 future 自建外部 task 不因 child 被 abort 就获得全部物理清理证明。长期定位、真实 runtime shutdown/owner drop、跨平台 stress、高并发和 native 仍未完整验收。原 SDK inventory/语言计数只保其当时来源，不能替代当前 SDK pin/Host/语言资格。

## 历史 SDK 消费者迁仓

Framework原观察9d1f3f2b5fdc204c08ecdec32ed22e8df95870e9的两项职责是：echo-sdk-protocol/tests/facade_inventory.rs核完整inventory签名、scope/alias分类；scripts/check-language-sdks.sh运行语言合同/Host及quickstart链。它们已随SDK产品迁出Framework，因此不再装作当前本仓consumer；历史Evidence以原9d身份和仅这两条原路径保存，旧Asset/Git原件及原verification/rereview不改。

当前同role定位是独立SDK本机可恢复commit3fcb8d066e8a6b6560d7991f06b4255121f2a49b中的同名[test inventory](https://github.com/EchoYue-lp/echo-agent-sdk/blob/3fcb8d066e8a6b6560d7991f06b4255121f2a49b/echo-sdk-protocol/tests/facade_inventory.rs)与[语言gate](https://github.com/EchoYue-lp/echo-agent-sdk/blob/3fcb8d066e8a6b6560d7991f06b4255121f2a49b/scripts/check-language-sdks.sh)。前者由SDK scripts/check-sdk-contracts.sh:45–51调用，后者由SDK .github/workflows/rust-ci.yml:261调用；完整blob/模式/哈希及精确父caller在历史Evidence正文。这些URL只是已核本机Git的精确定位格式，未验证远端可获取、未发布状态或正式新Framework pin，不是跨仓source_refs语法。

两项职责是显式迁仓承接，不是脚本字节或全部运行行为等价：SDK当前accepted external contract独立阻断，完整Rust inventory及scope计数为非阻断telemetry。Background Clone的一个canonical及三个alias仍标language_intrinsic，不形成三语言新增facade或自动Future恢复承诺。本次未运行SDK gate；当前SDK Host仍pin原f7bac62eee04a963f92a3e4308b6ea2336e91cc0，未对R23或F当前后继新pin验收。本资产的caller-owned/unsupported及原Discovery长期整合未知保持。
