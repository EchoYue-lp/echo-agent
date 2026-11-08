//! 可选原生操作资源准入；策略归宿主，许可保有归本次物理操作。

use std::sync::Arc;
use std::time::Instant;

use futures::future::BoxFuture;
use tokio_util::sync::CancellationToken;

use super::{InvocationResourceGuard, ToolContext};
use crate::error::{AgentError, Result, ToolError};
use crate::sandbox::SandboxCommand;

/// 原生操作尚未启动时的准入结果，不表示已有物理副作用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeExecutionAdmissionError {
    Cancelled,
    Deadline,
    Closed,
}

impl std::fmt::Display for NativeExecutionAdmissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Cancelled => "native execution admission cancelled",
            Self::Deadline => "native execution admission deadline elapsed",
            Self::Closed => "native execution admission closed",
        })
    }
}

impl std::error::Error for NativeExecutionAdmissionError {}

/// 宿主提供资源政策，不替换原执行器或建立第二个命令队列。
///
/// 等待必须响应取消和绝对期限，future 被撤回时不得遗留已取得资源。
/// 返回的 guard 仅交给该次 command 的物理 owner；不得存回父 ToolContext。
pub trait NativeExecutionResourceAdmission: Send + Sync {
    fn admit<'a>(
        &'a self,
        cancel: &'a CancellationToken,
        deadline: Instant,
    ) -> BoxFuture<'a, std::result::Result<InvocationResourceGuard, NativeExecutionAdmissionError>>;
}

/// 将同一 native provider 薄转换到原 cell Queued admission；调用这里只传引用，不取得许可。
pub fn command_cell_resource_admission(
    provider: Arc<dyn NativeExecutionResourceAdmission>,
) -> Arc<dyn super::cell::CommandCellResourceAdmission> {
    Arc::new(NativeCommandCellAdmission { provider })
}

struct NativeCommandCellAdmission {
    provider: Arc<dyn NativeExecutionResourceAdmission>,
}

impl super::cell::CommandCellResourceAdmission for NativeCommandCellAdmission {
    fn admit<'a>(
        &'a self,
        _cell_id: &'a str,
        cancel: &'a CancellationToken,
        deadline: Instant,
    ) -> BoxFuture<
        'a,
        std::result::Result<super::cell::CommandCellResourceLease, super::cell::CommandCellError>,
    > {
        Box::pin(async move {
            use super::cell::{CommandCellError, CommandCellResourceLease};
            let guard =
                self.provider
                    .admit(cancel, deadline)
                    .await
                    .map_err(|error| match error {
                        NativeExecutionAdmissionError::Cancelled => CommandCellError::Cancelled,
                        NativeExecutionAdmissionError::Deadline => {
                            CommandCellError::CapacityDeadline
                        }
                        NativeExecutionAdmissionError::Closed => CommandCellError::Shutdown,
                    })?;
            Ok(CommandCellResourceLease::new(move || drop(guard))
                .requiring_physical_resource_ownership())
        })
    }
}

/// 在原工具 future 内准入，并把资源移交本次命令。
///
/// 无 provider 时不改变原行为。等待消耗同一次命令 timeout；调用方仍须保持
/// 原 ToolManager attempt timer，并避免 limits 再覆盖已扣减的 wall-clock timeout。
pub async fn admit_native_execution(
    context: &ToolContext,
    command: &mut SandboxCommand,
    tool_name: &str,
) -> Result<()> {
    let Some(provider) = context.native_execution_admission.as_ref() else {
        return Ok(());
    };
    command.require_physical_resource_ownership = true;
    let deadline =
        Instant::now()
            .checked_add(command.timeout)
            .ok_or_else(|| ToolError::InvalidParameter {
                name: "timeout".to_string(),
                message: "Native operation deadline exceeds the monotonic clock range".to_string(),
            })?;
    let cancel = context
        .cancel
        .clone()
        .unwrap_or_else(|| Arc::new(CancellationToken::new()));
    let timeout_error = || ToolError::Timeout(tool_name.to_string());
    let cancelled_error = || AgentError::Cancelled(format!("tool '{tool_name}' admission"));
    if cancel.is_cancelled() {
        return Err(cancelled_error().into());
    }
    if deadline <= Instant::now() {
        return Err(timeout_error().into());
    }
    let guard = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(cancelled_error().into()),
        _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {
            return Err(timeout_error().into());
        }
        result = provider.admit(&cancel, deadline) => {
            match result {
                Ok(guard) => guard,
                Err(NativeExecutionAdmissionError::Cancelled) => return Err(cancelled_error().into()),
                Err(NativeExecutionAdmissionError::Deadline) => return Err(timeout_error().into()),
                Err(NativeExecutionAdmissionError::Closed) => return Err(ToolError::ExecutionFailed {
                    tool: tool_name.to_string(),
                    message: "Native execution admission is closed".to_string(),
                }.into()),
            }
        }
    };
    // 防止 provider 在取消／期限之后迟到交回许可而继续启动。
    if cancel.is_cancelled() {
        return Err(cancelled_error().into());
    }
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(timeout_error)?;
    command.timeout = remaining;
    command.resource_guards.push(guard);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::sync::{Notify, Semaphore};

    struct Admission {
        capacity: Arc<Semaphore>,
        acquired: Option<Arc<Notify>>,
    }

    impl NativeExecutionResourceAdmission for Admission {
        fn admit<'a>(
            &'a self,
            cancel: &'a CancellationToken,
            deadline: Instant,
        ) -> BoxFuture<
            'a,
            std::result::Result<InvocationResourceGuard, NativeExecutionAdmissionError>,
        > {
            Box::pin(async move {
                let permit = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return Err(NativeExecutionAdmissionError::Cancelled),
                    _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => return Err(NativeExecutionAdmissionError::Deadline),
                    permit = self.capacity.clone().acquire_owned() => permit.map_err(|_| NativeExecutionAdmissionError::Closed)?,
                };
                if let Some(acquired) = &self.acquired {
                    acquired.notify_one();
                    std::future::pending::<()>().await;
                }
                Ok(InvocationResourceGuard::new(permit))
            })
        }
    }

    #[tokio::test]
    async fn background_bridge_only_acquires_in_original_admission_and_retains_the_requirement()
    -> Result<()> {
        let capacity = Arc::new(Semaphore::new(1));
        let provider = Arc::new(Admission {
            capacity: capacity.clone(),
            acquired: None,
        });
        let bridge = command_cell_resource_admission(provider);
        assert_eq!(
            capacity.available_permits(),
            1,
            "constructing a background launch policy acquired foreground capacity"
        );
        let cancel = CancellationToken::new();
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(1))
            .ok_or_else(|| crate::error::ReactError::Other("test deadline overflow".to_string()))?;
        let lease = bridge
            .admit("original-cell", &cancel, deadline)
            .await
            .map_err(|error| crate::error::ReactError::Other(error.to_string()))?;
        assert!(lease.requires_physical_resource_ownership());
        assert_eq!(capacity.available_permits(), 0);
        drop(lease);
        assert_eq!(capacity.available_permits(), 1);
        assert!(
            !super::super::cell::CommandCellResourceLease::new(|| {})
                .requires_physical_resource_ownership()
        );
        Ok(())
    }

    #[tokio::test]
    async fn native_guard_belongs_only_to_the_original_command() -> Result<()> {
        let capacity = Arc::new(Semaphore::new(1));
        let context = ToolContext {
            native_execution_admission: Some(Arc::new(Admission {
                capacity: capacity.clone(),
                acquired: None,
            })),
            ..ToolContext::default()
        };
        let mut command = SandboxCommand::program("echo", vec!["effect".to_string()])
            .with_timeout(Duration::from_secs(1));
        admit_native_execution(&context, &mut command, "shell").await?;
        assert_eq!(capacity.available_permits(), 0);
        assert!(context.resource_guards.is_empty());
        assert!(command.timeout <= Duration::from_secs(1));
        drop(command);
        assert_eq!(capacity.available_permits(), 1);
        // 下一操作复用同一个长寿命 context，也不会继承上一操作的许可。
        let mut next = SandboxCommand::program("echo", vec![]);
        admit_native_execution(&context, &mut next, "shell").await?;
        drop(next);
        assert_eq!(capacity.available_permits(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn withdrawing_after_capacity_acquisition_cannot_leave_a_late_guard() -> Result<()> {
        let capacity = Arc::new(Semaphore::new(1));
        let acquired = Arc::new(Notify::new());
        let cancel = Arc::new(CancellationToken::new());
        let context = ToolContext {
            native_execution_admission: Some(Arc::new(Admission {
                capacity: capacity.clone(),
                acquired: Some(acquired.clone()),
            })),
            cancel: Some(cancel.clone()),
            ..ToolContext::default()
        };
        let pending = tokio::spawn(async move {
            let mut command = SandboxCommand::program("echo", vec![]);
            let result = admit_native_execution(&context, &mut command, "shell").await;
            (result, command.resource_guards.len())
        });
        acquired.notified().await;
        assert_eq!(capacity.available_permits(), 0);
        cancel.cancel();
        let (result, guards) = pending
            .await
            .map_err(|error| crate::error::ReactError::Other(error.to_string()))?;
        assert!(
            matches!(result, Err(crate::error::ReactError::Agent(error)) if matches!(*error, AgentError::Cancelled(_)))
        );
        assert_eq!(guards, 0);
        assert_eq!(capacity.available_permits(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn admission_deadline_and_zero_do_not_restart_a_command_budget() -> Result<()> {
        let capacity = Arc::new(Semaphore::new(1));
        let acquired = Arc::new(Notify::new());
        let context = ToolContext {
            native_execution_admission: Some(Arc::new(Admission {
                capacity: capacity.clone(),
                acquired: Some(acquired),
            })),
            ..ToolContext::default()
        };
        let mut command =
            SandboxCommand::program("echo", vec![]).with_timeout(Duration::from_millis(20));
        let result = admit_native_execution(&context, &mut command, "shell").await;
        assert!(
            matches!(result, Err(crate::error::ReactError::Tool(error)) if matches!(*error, ToolError::Timeout(_)))
        );
        assert!(command.resource_guards.is_empty());
        assert_eq!(capacity.available_permits(), 1);
        command.timeout = Duration::ZERO;
        assert!(
            admit_native_execution(&context, &mut command, "shell")
                .await
                .is_err()
        );
        assert_eq!(capacity.available_permits(), 1);
        let mut unconfigured = SandboxCommand::program("echo", vec![]).with_timeout(Duration::ZERO);
        admit_native_execution(&ToolContext::default(), &mut unconfigured, "shell").await?;
        assert_eq!(unconfigured.timeout, Duration::ZERO);
        Ok(())
    }
}
