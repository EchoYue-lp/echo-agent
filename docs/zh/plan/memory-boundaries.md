# 自动记忆责任边界

本文用于约束三套自动记忆路径，避免后续继续扩张成重复系统。

## TriggerDetector

框架运行时能力，位于 `echo_agent::evolution`。

- 负责：在线对话过程中的轻量“新记忆发现”。
- 输入：当前用户消息、上一轮 assistant 回复、近期工具失败/成功、近期工具序列。
- 写入：只能通过 `MemoryLayerManager::write_memory` 写入可 recall 的 typed memory。
- 适合捕捉：用户偏好、用户纠正、已验证的错误解决方案、重复工作流模式。
- 不负责：会话结束后的完整 transcript 总结、`.echo-agent/project.md` 等静态产品提示文件。

## AutoMemory

框架提取原语 + 应用生命周期集成。

- 负责：会话结束或手动触发时的“归档总结”。
- 框架职责：`Observation`、`ObservationCategory`、观察提取、分类到 `MemoryType`、稳定 typed key、typed-memory 写入 helper。
- 应用职责：CLI/TUI/GUI 开关、session end/manual trigger 时机、`.echo-agent/project.md` 产品提示文件写入。
- 写入：产品层可以写 `.echo-agent/project.md`；任何需要进入 runtime recall 的 typed memory 必须走框架 `MemoryLayerManager::write_memory`。
- 禁止重复：GUI/TUI/CLI/app-core 不再各自维护 category/type/key/write 规则。

## memory_promoter

框架压缩/淘汰路径。

- 负责：已有上下文和已有记忆的“生命周期管理”。
- 输入：因 token 压力从 active context 中被压缩或淘汰的消息/摘要。
- 写入：长期 store，用于后续 recall。
- 适合处理：压缩后的重要上下文保留、长期化、淘汰、降级。
- 不负责：用户显式偏好、BackgroundReviewer 结果、GUI/TUI/CLI 触发的 AutoMemory 观察提取。

## BackgroundReviewer

框架 review 能力，由应用调度。

- 负责：完成 run/trajectory 后的异步深度回顾，提取高价值记忆和改进信号。
- 写入：只能通过 `MemoryLayerManager::write_memory`。
- 应用职责：决定何时运行 reviewer，并提供运行时 `ReviewIntegration`。
- 兼容：短期允许 `improve` 保留旧路径；新应用代码优先使用 `echo_agent::evolution` re-export。

## 总结

| 系统 | 主要职责 | 不应承担 |
| --- | --- | --- |
| `TriggerDetector` | 在线轻量新记忆发现 | 会话归档、项目文件写入 |
| `AutoMemory` | 会话结束/手动触发的归档总结 | 压缩淘汰、运行时策略调度 |
| `memory_promoter` | 已有上下文/记忆的生命周期管理 | 新偏好发现、UI 触发提取 |
| `BackgroundReviewer` | 异步深度回顾和演化信号 | GUI/TUI/CLI 产品调度策略 |
