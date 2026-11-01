# ADR 0103：统一当前存储格式，取消旧记录和推断恢复

## 当前状态（2026-10-29）

方案已在 Framework 正式工作区采用，尚未提交发布。最新完整冻结组合
`f327f63e34b7b4fea30d1c34f8559431fd2228172a2d427d8f1eac5912ee75c4`
含 2831 个文件；同源 `cargo fmt --all`、格式检查以及
`cargo check -p echo_agent --features subagent,testing --lib --locked` 均自然退出 0，
编译零警告。此项只证明所选 feature 的当前根库及依赖可编译，不替代全部存储行为、
SQLite 条件矩阵、examples、消费者、完整工程门禁、真实 native 或 main CI。
其它 focused 结果保各自源码身份，不合并为本组合的全量通过。

### 初稿阶段状态（历史）

状态：已批准 NoCompat 目标的 ignored 组合实现；尚未编译、测试或正式接纳。

## 背景

框架提供可独立复用的运行状态和会话 Store。旧记录格式、旧计划文本和缺身份的恢复分支混在当前路径中，容易把兼容接纳误当成当前运行许可。已批准的退役清单取消这些旧义务，同时保留 raw Store CRUD、当前消息历史、原生签名、context-only lineage、普通 CAS 和稳定身份删除。

本次属于通用存储机制，归框架；EKO 的删除 tombstone、产品历史和 SDK 的 wire/语言投影由各自 owner 同步。已有版本枚举、日志 manifest、project/restore 函数和 managed 删除协调器足以表达当前行为，不新增 validator、Store、模式字段或执行 owner。

## 候选方案与决策

保留多版本 decoder 会继续承担已取消的义务。新增 mode 字段或并行恢复 owner 会重复已有权威。因此使用既有 writer/decoder 和当前类型，删除兼容分支。

1. 检查点 payload 只写读显式 `schema_version: 3`。裸 `Vec<Message>`、缺版本和版本 1/2 拒绝。cursor、pending effect、`context_only` 仍按当前语义可选；无 cursor 的 runtime-only/raw 保存合法。版本号不代替 transcript receipt。
2. 文件运行状态 owner 只写读版本 2，必填现有 `RuntimeStateVersion`。raw writer 写 `Unmanaged { digest }`，同一 digest 可由原 CAS 采用为 `Managed { revision: 1 }`；managed/retired 使用原 revision、epoch、pending/ack 校验。raw `Deleting + checkpoint: None + Unmanaged` 崩溃切点保留。scope/index 的版本 1 是当前格式，继续保留。
3. `AgentCheckpoint.current_plan` 从类型、比较、SQLite 列及读写中删除。计划权威仍为 revisioned TaskRun graph。消息、技能、blocked reason、工作目录、时间和原生内容继续保存。
4. `ConversationRecord` 的 manifest 必填当前 `message_log`，内存 `messages` 不序列化；旧内嵌聚合布局不读、不自动迁移。当前 append、edit/shorten/Fork generation 替换、冷开、消息 ID 回收、提交字节前沿、孤儿日志清理和 managed receipt 顺序保留。
5. `project_message` 为包括纯文本在内的当前 runtime Message 写完整 marker。`restore_message` 必须读到当前 marker；缺 marker、坏 JSON、缺 tool ID/name 或相互冲突的 tool 身份均拒绝，不补 `unknown_tool_call` 或空名称。带 marker 的消费者扩展仍由消费者拥有，恢复不改写原件。raw Store 仍可保存仅供展示的应用记录；保存本身不授予 runtime 身份。
6. SQLite 当前 `CREATE TABLE` 直接声明完整当前列，删除旧 `ALTER` 补列和 duplicate-column 忽略。框架 SQLite backend、事务和普通 CAS 保留；EKO 不启用 SQLite。
7. 删除无稳定 delete identity 的 `delete_persisted_conversation`、对应 context 包装及旧组合删除/清除推断分支。保留带调用者持久化 `ManagedConversationDelete` 的 managed 删除及原 incarnation clear 协调器；后者继续使用 deadline、settlement、原 scope owner/CAS 和 tombstone。raw CRUD 的显式 `delete_conversation`、`clear_runtime_state`、`clear_runtime_state_scope` 保留。

## 保留约束与跨仓影响

control 退役包已先合入。本次按函数合流，不恢复旧 RecoveryArchive、inspect/commit adapter 或工具来源猜测；`snapshot::derive_context_view_checkpoint` 及当前调用保持。普通 CAS 的 expected epoch/version、stale 拒绝、幂等 replay、pending effect 非重放、ack proof、scope retirement journal、锁、deadline、typed unknown 和原准备/producer owner 保留。

SDK owner 必须同步 checkpoint wire、Host projection、fixture 和生成的语言合同，并解除已删除 helper 的 source-operation 路由。EKO owner 必须移除 checkpoint initializer 中旧字段；缺持久化 managed-delete identity 的旧 tombstone 不得临时制造新身份，须按当前产品恢复策略明确拒绝。框架 snapshot initializer、stream 混合 fixture 与框架/官网说明由各文件 owner 同步。最小提案及精确 base/after SHA 位于 storage-integration/proposals，不直接改跨仓正式文件。

## 验证边界（初稿阶段历史）

旧接纳和迁移测试义务记为 `retired_by_goal`，不计通过，不新增 skip。当前 cold/history/skills/resource/trace、字段保真、工具配对、签名/Parts/approved `None` 与空数组、digest 腐坏、CAS/replay、清除不重放和 owner-first crash cuts 保留或迁为当前格式。

本轮仅可执行 standalone rustfmt、隔离 apply 和只读源码审查；没有 Cargo、类型检查、测试或应用运行证据。正式接纳前应先合齐所有 owner 的调用变化，在冻结组合快照按测试分类执行真实 tests-first 和当前回归；不得借用先前主批或旧候选结果。

## 文件后端物理文件名预算修复（2026-11-01，局部证据已保全）

自动 Cron 的原会话 ID 为 121 字节，现有完整 hex key 为 245 字节，加消息日志后缀后 262 字节；原 atomic sibling 临时名又增加 42 字节，真实文件 API 已返回 `ENAMETOOLONG`。手动 92 字节 ID 的临时名可写，不构成自动 ID 结论。

此修复属于通用 Framework 文件持久化机制。EKO 保留原 logical conversation、occurrence 和 TaskRun 身份，不缩 ID、不新建 store 或 executor，也不重放既有失败 Run。全局 `encode_utf8_path_identity` / `encode_path_segment_identity` 保持无损原契约，因为它们也用于非文件名 scope 和 runtime-state。

选定方案只调整内部物理地址：FileConversationStore 沿既有路径校验后，对不超过 219 字节的 ASCII 完整 hex key 保持原路径；超过预算时使用完整 UTF-8 原 ID 的固定 SHA-256 key。219 为 255 字节文件分量扣除 `.messages.<u64最大20位>.jsonl` 的 36 字节。manifest、消息日志、append、replacement 和 orphan cleanup 继续共享同一 safe_segment，无第二 fallback、decoder 或逻辑 ID 迁移。读取 manifest 必须匹配请求的原 logical id；不靠哈希路径推断内容身份。

`atomic_write` 同目录临时名改为固定 UUID 形式，不再携带目标 basename。原 existing permissions、write/sync、replace、parent fsync 以及错误后的临时文件清理顺序全部保留。既有 manual hex 路径继续可写，long key 与完整 generation 后缀获得有界物理名；没有新增公开签名、schema、依赖或配置。

本地已保全 Core 22 个与 State 45 个 distinct focused 回归，共 67 个原件；manual current-format seed 另有 1 passed。初始 State 的 44 passed / 1 failed 是错误 manual seed 目录造成的历史原件，修复 fixture 后的 regression-state-r02 为 45 passed / 0 failed。正式 clean 自然退出 0，target 已移除，程序、fingerprint、`.d`、raw 与 receipt 均完成 clean 后回读。上述证据只覆盖 Framework 文件后端局部行为，不冒充自动 Cron、App99009 之外的产品运行、SDK、main CI、native 或完整工程门禁。

本 ADR 小节只校准文件后端物理文件名预算修复的当前时态，不改变既有 NoCompat 退役决定、旧记录/推断恢复边界或其它 ADR 结论。examples 与官网没有新的公开调用方式或产品能力；本变更不修改 SDK、EKO 正式源码、锁、风险或 AGENTS。
