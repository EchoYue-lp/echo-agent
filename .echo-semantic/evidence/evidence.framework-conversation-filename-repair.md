---
schema_version: 1
id: evidence.framework-conversation-filename-repair
kind: evidence
observed_at: source:052f25a01148de087f1ca9c1644e0c6084467ae07f3f4a964720037011187cd3
source_refs:
  - echo-core/src/utils/fs.rs
  - echo-state/src/memory/file_conversation.rs
  - .echo-semantic/assets/asset.conversation-store.md
  - docs/adr/0103-current-storage-records.md
supports:
  - rule.context-persistence-separation
  - rule.fact-projection-separation
limitations:
  - 本证据只覆盖 Framework 文件后端物理文件名预算与 atomic_write 临时名修复；不改变旧 NoCompat 退役决定。
  - 当前 67 个 distinct focused 回归与 manual seed consumer 仅绑定本仓保全制品；不冒充自动 Cron、SDK、GUI/App、native、main CI 或完整矩阵验收。
  - formal Source/Final 结果由本任务后续正常命令单独绑定；Source 失败时不进入 Final。
---

## 支持的结论

本证据支持当前 Framework 文件后端物理文件名预算修复的局部源码身份、focused 67 distinct 回归、manual seed 与官方 clean0 保全结果。

## 来源与范围

来源为当前三路径源码、ADR0103、ConversationStore 资产、两条持久化规则、focused-final-delivery 保全材料和本任务正常 Source/Final 回执；范围限于 Framework 文件后端物理地址与 transcript projection 物理持久化边界。

## 已知缺口

自动 Cron 真机、App99009 之外的产品运行、SDK、native、main CI、完整矩阵和历史 closure/Map 全局闭合不由本证据证明。

# Framework 文件后端物理文件名预算修复

## 当前源身份与问题

当前正式 Framework worktree 为 F340 `340993201cc937fc8e4783b66be399ef9f14e3d5`，本任务只处理以下三条已有差异；两份 Rust 源字节不再改写：

- `echo-core/src/utils/fs.rs`：`27206f8cfa108484ed4f6f0e9c8fb12356917e4e135034e17bfda03e9fe0dc34`。
- `echo-state/src/memory/file_conversation.rs`：`ac24ff9d9d78b4eb5ef1e921a3d74691db1fbc1aef363962388eaa8dff63c158`。
- `docs/adr/0103-current-storage-records.md`：当前校准前 `943ecdee041a5d7d71cb85dd779193b902f27b44a283c8fd91a9341abfc842c3`。

自动 Cron 会话 ID 的真实形状为 121 字节；既有完整 hex key 为 245 字节，加消息日志后缀后为 262 字节；旧 atomic sibling 临时名还会追加 42 字节并触发 `ENAMETOOLONG`。手动 92 字节 ID 的临时名可写不构成自动 ID 结论。

## 修复边界与唯一权威

`FileConversationStore` 继续拥有 transcript 文件后端，`ConversationStore` 仍是用户 transcript projection 的唯一 authority；不新增 Store、executor、logical ID 迁移、schema、公开签名或配置。

- 不超过 219 字节的 ASCII 完整 hex key 保持原物理路径。
- 超过预算的完整 UTF-8 logical ID 使用固定 SHA-256 物理 key；manifest 仍必须匹配请求的原 logical ID，不从哈希路径推断身份。
- `atomic_write` 同目录临时名改为固定 UUID 形状，不再携带目标 basename；existing permissions、write/sync、replace、parent fsync 和失败清理顺序保持。
- 原 `encode_utf8_path_identity` / `encode_path_segment_identity` 无损契约保持，因为它们还服务非文件名 scope 与 runtime-state。

## 已保全的 focused 证据

保全交付文件为顶层 `.echo-semantic/local/parallel-executors-20261027/im-cron/framework-conversation-filename-repair-20261101-r01/execution-r01/focused-final-delivery.json`。其中 `current_source_digest=4106f7fb0e00e3761ee9bccea990e1c83738f8db8562d465a5e0864e72555dba` 是 focused 工程材料的历史身份，只绑定当时保全结果，不表示当前观察；原 ADR SHA 为 `943ecdee041a5d7d71cb85dd779193b902f27b44a283c8fd91a9341abfc842c3`。当前 canonical source identity 已由官方 `verify_semantic.py` 的 `source_digest(root)` query 读取为 `source:052f25a01148de087f1ca9c1644e0c6084467ae07f3f4a964720037011187cd3`，对应当前 F340 worktree 与 ADR SHA `b2d360fa9d0a5f8eb1c51999e3a1653d5c3449ae33e6064c630f9e7302b35770`。source 文件数 2850，锁 SHA-256 为 `1442bf309239cca9354070fee0d6278bb53cb8aeee0cafa436e97c64a93324af`，F340 before/after digest 对账为 true，正式 clean 自然退出 0，target 已消失，保全程序、fingerprint、`.d`、raw 与 receipt 均完成 clean 后回读。

当前 distinct focused 回归为 Core 22 passed、State 45 passed，共 67 个原件；manual current-format seed 另有 1 passed。初始 State 回归的 44 passed / 1 failed 是错误 manual seed 目录造成的历史原件，修复 fixture 后的 `regression-state-r02` 为 45 passed / 0 failed。该边界不扩展到自动 Cron 真机、App99009 之外的产品运行、SDK/main CI 或完整工程矩阵；App99009 只记录已编译 consumer，不冒充真人自动 Cron 或 SDK 验收。

## 关系与限制

本证据绑定现有 `asset.conversation-store`、`behavior.context-memory-lifecycle`、`rule.context-persistence-separation` 和 `rule.fact-projection-separation`。它只说明文件物理地址预算与 transcript 物理持久化边界的局部修复，不新造 `asset.atomic-write`，不升级既有 Behavior 状态，也不改旧场景、unknown 或观察结论。

examples、SDK、EKO 正式源码、GUI/App、native、main CI、发布和其它历史 closure 均不由本证据接纳。formal Source 必须使用当前真实三路径与本 Evidence/Map 绑定的正常预检；若历史 4 retired / 117 object Map / 6 behavior Map 阻断仍出现，保留真实失败，不删引用、不挂假 Map、不恢复退役源码、不改工具或 ignore/scope。
