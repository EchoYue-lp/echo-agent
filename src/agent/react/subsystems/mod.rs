//! ReactAgent 子系统模块
//!
//! 将 ReactAgent 的 ~25 个字段按职责拆分为 4 个子系统：
//!
//! | 模块 | 子系统 | 职责 |
//! |------|--------|------|
//! | `tool_exec` | `ToolExecutionSubsystem` | 工具注册/执行、Skill、Hook、MCP、SubAgent、Sandbox |
//! | `guard` | `GuardSubsystem` | 护栏、权限策略、审计日志、熔断器 |
//! | `memory` | `MemorySubsystem` | 上下文管理、长期记忆、快照、Checkpoint、对话持久化 |
//! | `approval` | `ApprovalSubsystem` | 人工介入审批（human-in-the-loop） |

pub(crate) mod approval;
pub(crate) mod guard;
pub(crate) mod memory;
pub(crate) mod tool_exec;
