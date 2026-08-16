//! ReactAgent subsystem modules
//!
//! Splits ReactAgent's ~25 fields into 4 subsystems by responsibility:
//!
//! | Module | Subsystem | Responsibility |
//! |--------|-----------|----------------|
//! | `tool_exec` | `ToolExecutionSubsystem` | Tool registration/execution, Skill, Hook, MCP, Subagent, Sandbox |
//! | `guard` | `GuardSubsystem` | Guardrails, permission policies, audit logs, circuit breaker |
//! | `memory` | `MemorySubsystem` | Context management, long-term memory, snapshots, Checkpoint, conversation persistence |
//! | `approval` | `ApprovalSubsystem` | Human-in-the-loop approval |

#[cfg(feature = "human-loop")]
pub(crate) mod approval;
pub(crate) mod guard;
pub(crate) mod memory;
pub(crate) mod tool_exec;
