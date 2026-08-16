//! Background command cells (Codex-style `shell(background=true)` + `wait`).
//!
//! [`BackgroundCommandManager`] implements the
//! [`echo_core::tools::cell::CommandCellRegistry`] contract: long-running
//! commands (hour-scale builds/tests) are launched as *cells* that return a
//! `cell_id` immediately; callers long-poll via `wait(cell_id, cursor,
//! yield_ms)` for incremental output and the terminal state.
//!
//! # Why not reuse `BackgroundTask::wait`?
//!
//! `BackgroundTask`'s result cell is `take()`n once — a single consumer gets
//! the outcome. Cells must support **multiple waiters** (main agent + awaiter
//! subagent) that can each re-read the same terminal state, so a cell keeps
//! its own repeatedly-readable state (`RwLock<CellState>` + tail-retained
//! output buffer + `Notify::notify_waiters` fan-out).

use dashmap::DashMap;
use echo_core::sandbox::{
    SandboxCommand, SandboxExecutor, SandboxOutputChannel, SandboxStreamEvent,
};
use echo_core::tools::ToolOutputChannel;
use echo_core::tools::artifact::{ToolOutputArtifactRef, ToolOutputArtifactWriter};
use echo_core::tools::cell::{
    CommandCellDelta, CommandCellPhase, CommandCellRegistry, CommandCellRequest,
    CommandCellSnapshot,
};
use futures::StreamExt;
use futures::future::BoxFuture;
use std::future::Future;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::sync::{Notify, RwLock, Semaphore};
use tokio_util::sync::CancellationToken;

/// UTF-8 safe preview length for a cell's display name (chars).
const NAME_PREVIEW_CHARS: usize = 80;
/// Per-wait round byte cap for `new_output`.
///
/// The cursor is also byte-based, so advancing by exactly the returned raw
/// byte count makes every retained byte drainable across repeated waits.
const MAX_DELTA_BYTES: usize = 16 * 1024;
/// Reader task chunk size (bytes).
const READ_CHUNK_BYTES: usize = 16 * 1024;

// ── Config ──────────────────────────────────────────────────────────

/// Configuration for [`BackgroundCommandManager`].
#[derive(Debug, Clone)]
pub struct BackgroundCommandManagerConfig {
    /// Maximum number of concurrently running cells.
    pub max_concurrent: usize,
    /// Default cell lifetime in seconds (`timeout_secs: None`).
    pub default_timeout_secs: u64,
    /// Upper bound for any cell lifetime.
    pub max_timeout_secs: u64,
    /// In-memory output retention per cell (tail bytes kept).
    pub max_retained_output_bytes: usize,
    /// Maximum number of terminal cells retained for `list`/`wait`.
    /// Running cells are never removed by retention.
    pub max_terminal_history: usize,
}

impl Default for BackgroundCommandManagerConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 4,
            default_timeout_secs: 3600,
            max_timeout_secs: 86400,
            max_retained_output_bytes: 1024 * 1024,
            max_terminal_history: 256,
        }
    }
}

// ── Output buffer ───────────────────────────────────────────────────

/// Tail-retained output buffer.
///
/// `total_bytes` counts every byte ever written (monotonic cursor space);
/// `retained` keeps at most `max_retained_output_bytes` tail bytes — bytes
/// dropped from the head are still reflected in `total_bytes` and flagged
/// via `truncated`.
#[derive(Default)]
struct OutputBuffer {
    retained: Vec<u8>,
    total_bytes: u64,
    truncated: bool,
}

impl OutputBuffer {
    /// Append bytes, keeping only the tail within `max_retained`.
    fn push(&mut self, bytes: &[u8], max_retained: usize) {
        self.total_bytes = self
            .total_bytes
            .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        self.retained.extend_from_slice(bytes);
        let overflow = self.retained.len().saturating_sub(max_retained);
        if overflow > 0 {
            // 只保留尾部: 丢弃头部 overflow 字节(字节级运算, &mut Vec 不会 panic)
            self.retained.drain(..overflow);
            self.truncated = true;
        }
    }
}

// ── Cell handle ─────────────────────────────────────────────────────

/// Phase + exit code, atomically updated together.
#[derive(Debug, Clone, Copy)]
struct CellState {
    phase: CommandCellPhase,
    exit_code: Option<i32>,
}

/// Shared state of one command cell.
pub struct CommandCellHandle {
    cell_id: String,
    name: String,
    /// Terminal-phase + exit code (write-once at completion).
    state: RwLock<CellState>,
    /// Tail-retained output; std Mutex (short critical sections, no await).
    output: Mutex<OutputBuffer>,
    /// Complete-output spill writer. The lock is held only for one chunk.
    artifact_writer: Mutex<Option<ToolOutputArtifactWriter>>,
    /// Final artifact reference, repeatedly readable with terminal snapshots.
    output_artifact: Mutex<Option<ToolOutputArtifactRef>>,
    /// Fan-out wakeup for waiters (output appended / phase finalized).
    notify: Notify,
    /// Kill switch for the child process.
    cancel: CancellationToken,
    /// Sync-readable terminal marker for history pruning.
    terminal_flag: AtomicBool,
    /// Monotonic registration order for bounded terminal retention.
    sequence: u64,
}

impl CommandCellHandle {
    /// Whether the cell has reached a terminal phase (sync, lock-free).
    fn is_terminal(&self) -> bool {
        self.terminal_flag.load(Ordering::Acquire)
    }

    /// Current state (phase + exit code), cheap to copy.
    async fn current_state(&self) -> CellState {
        *self.state.read().await
    }

    /// Non-blocking snapshot (state + output counters).
    async fn snapshot(&self) -> CommandCellSnapshot {
        let state = self.current_state().await;
        let output = self
            .output
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let artifact = self
            .output_artifact
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        build_snapshot(&self.cell_id, &self.name, state, &output, artifact)
    }
}

fn build_snapshot(
    cell_id: &str,
    name: &str,
    state: CellState,
    output: &OutputBuffer,
    output_artifact: Option<ToolOutputArtifactRef>,
) -> CommandCellSnapshot {
    CommandCellSnapshot {
        cell_id: cell_id.to_string(),
        name: name.to_string(),
        phase: state.phase,
        exit_code: state.exit_code,
        total_output_bytes: output.total_bytes,
        output_truncated: output.truncated,
        output_artifact,
    }
}

/// Read the cell's output under lock and build the delta for `cursor`.
/// (Keeps the `MutexGuard` scope minimal so callers can stay in async code.)
async fn snapshot_delta(
    handle: &CommandCellHandle,
    cursor: u64,
    state: CellState,
) -> CommandCellDelta {
    let output = handle
        .output
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let artifact = handle
        .output_artifact
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    build_delta(
        &handle.cell_id,
        &handle.name,
        cursor,
        state,
        &output,
        artifact,
    )
}

/// Build one wait-round delta from a locked output view.
///
/// All slicing is byte-level on `&[u8]` (never panics via `get`). Valid UTF-8
/// output is capped only at a character boundary; arbitrary process bytes are
/// still supported through lossy conversion.
fn build_delta(
    cell_id: &str,
    name: &str,
    cursor: u64,
    state: CellState,
    output: &OutputBuffer,
    output_artifact: Option<ToolOutputArtifactRef>,
) -> CommandCellDelta {
    let total = output.total_bytes;
    // 非法(超前)cursor 一律收敛为 total, 返回空增量而不是报错。
    let effective_cursor = cursor.min(total);
    let retained_len = u64::try_from(output.retained.len()).unwrap_or(u64::MAX);
    let retained_start = total.saturating_sub(retained_len);

    let mut output_elided = false;
    let mut prefix = String::new();
    let (bytes, output_start): (&[u8], u64) = if effective_cursor < retained_start {
        // cursor 早于保留窗口起点: 头部字节已被丢弃, 只能从 retained 开头取,
        // 并显式告知调用方该段增量之前有被丢弃的字节。
        output_elided = true;
        prefix = format!(
            "[... {} earlier output bytes were discarded (buffer retention cap) ...]\n",
            retained_start - effective_cursor
        );
        (output.retained.as_slice(), retained_start)
    } else {
        let offset =
            usize::try_from(effective_cursor - retained_start).unwrap_or(output.retained.len());
        (
            output.retained.get(offset..).unwrap_or(&[]),
            effective_cursor,
        )
    };

    let consumed = capped_delta_len(bytes);
    let returned = bytes.get(..consumed).unwrap_or_default();
    let text = String::from_utf8_lossy(returned).into_owned();
    if consumed < bytes.len() {
        output_elided = true;
    }
    let consumed_u64 = u64::try_from(consumed).unwrap_or(u64::MAX);
    let next_cursor = output_start.saturating_add(consumed_u64).min(total);

    CommandCellDelta {
        snapshot: build_snapshot(cell_id, name, state, output, output_artifact),
        new_output: format!("{prefix}{text}"),
        next_cursor,
        output_elided,
    }
}

fn capped_delta_len(bytes: &[u8]) -> usize {
    let candidate = bytes.len().min(MAX_DELTA_BYTES);
    let prefix = bytes.get(..candidate).unwrap_or_default();
    match std::str::from_utf8(prefix) {
        Ok(_) => candidate,
        Err(error) if error.error_len().is_none() && error.valid_up_to() > 0 => error.valid_up_to(),
        // Invalid process bytes, or a retained window beginning mid-character,
        // must still make byte-cursor progress.
        Err(_) => candidate,
    }
}

// ── Manager ─────────────────────────────────────────────────────────

/// Registry of background command cells (see module docs).
///
/// `launch` must be called from within a tokio runtime (it spawns the runner
/// task). All methods are non-blocking except `wait`, which returns within
/// the caller's yield budget.
pub struct BackgroundCommandManager {
    cells: DashMap<String, Arc<CommandCellHandle>>,
    semaphore: Arc<Semaphore>,
    config: BackgroundCommandManagerConfig,
    /// Optional executor used for launches that must preserve foreground
    /// sandbox semantics.
    sandbox: Option<Arc<dyn SandboxExecutor>>,
    /// Monotonic registration sequence for deterministic terminal retention.
    next_sequence: AtomicU64,
}

impl Default for BackgroundCommandManager {
    fn default() -> Self {
        Self::new(BackgroundCommandManagerConfig::default())
    }
}

impl BackgroundCommandManager {
    pub fn new(config: BackgroundCommandManagerConfig) -> Self {
        let semaphore = Arc::new(Semaphore::new(config.max_concurrent));
        Self {
            cells: DashMap::new(),
            semaphore,
            config,
            sandbox: None,
            next_sequence: AtomicU64::new(0),
        }
    }

    /// Create a manager whose cells execute through the supplied sandbox.
    pub fn new_with_sandbox(
        config: BackgroundCommandManagerConfig,
        sandbox: Arc<dyn SandboxExecutor>,
    ) -> Self {
        let mut manager = Self::new(config);
        manager.sandbox = Some(sandbox);
        manager
    }

    /// Retain at most `max_terminal_history` terminal cells (oldest first).
    /// Running cells are never selected. Mirrors `TaskSpawner::prune_terminal_history`.
    fn prune_terminal_history(&self) {
        let max_terminal_history = self.config.max_terminal_history;
        let mut terminal = self
            .cells
            .iter()
            .filter(|entry| entry.value().is_terminal())
            .map(|entry| (entry.value().sequence, entry.key().clone()))
            .collect::<Vec<_>>();
        let remove_count = terminal.len().saturating_sub(max_terminal_history);
        if remove_count == 0 {
            return;
        }
        terminal.sort_by_key(|(sequence, _)| *sequence);
        for (_, id) in terminal.into_iter().take(remove_count) {
            self.cells.remove(&id);
        }
    }
}

impl CommandCellRegistry for BackgroundCommandManager {
    fn launch(&self, request: CommandCellRequest) -> std::result::Result<String, String> {
        if request.command.trim().is_empty() {
            return Err("command must not be empty".to_string());
        }
        if request.require_sandbox && self.sandbox.is_none() {
            return Err(
                "background command requires a sandbox executor, but the cell registry has none"
                    .to_string(),
            );
        }
        self.prune_terminal_history();

        let cell_id = uuid::Uuid::new_v4().to_string();
        let name: String = request.command.chars().take(NAME_PREVIEW_CHARS).collect();
        let handle = Arc::new(CommandCellHandle {
            cell_id: cell_id.clone(),
            name,
            state: RwLock::new(CellState {
                phase: CommandCellPhase::Running,
                exit_code: None,
            }),
            output: Mutex::new(OutputBuffer::default()),
            artifact_writer: Mutex::new(
                request
                    .output_artifacts
                    .clone()
                    .zip(request.artifact_identity.clone())
                    .map(|(config, identity)| ToolOutputArtifactWriter::new(config, identity)),
            ),
            output_artifact: Mutex::new(None),
            notify: Notify::new(),
            cancel: CancellationToken::new(),
            terminal_flag: AtomicBool::new(false),
            sequence: self.next_sequence.fetch_add(1, Ordering::Relaxed),
        });

        // 超时: 未指定用默认值, 一律 clamp 到上限; 0 = 不限时。
        let timeout_secs = request
            .timeout_secs
            .unwrap_or(self.config.default_timeout_secs)
            .min(self.config.max_timeout_secs);
        let timeout = if timeout_secs == 0 {
            None
        } else {
            Some(Duration::from_secs(timeout_secs))
        };
        let working_dir = request.working_dir.map(PathBuf::from);
        let semaphore = self.semaphore.clone();
        let sandbox = self.sandbox.clone();
        let max_retained = self.config.max_retained_output_bytes;
        let runner = handle.clone();
        let run_spec = CellRunSpec {
            command: request.command,
            working_dir,
            timeout,
            sandbox,
            owner_cancel: request.cancel,
            max_retained,
        };

        tokio::spawn(async move {
            let (phase, exit_code) = run_cell(runner.clone(), semaphore, run_spec).await;
            finish_output_artifact(&runner);
            *runner.state.write().await = CellState { phase, exit_code };
            runner.terminal_flag.store(true, Ordering::Release);
            // 唤醒所有等待者(多等待者语义, 终态可被重复读取)。
            runner.notify.notify_waiters();
            tracing::info!(
                cell_id = %runner.cell_id,
                phase = phase.as_str(),
                "command cell finished"
            );
        });

        self.cells.insert(cell_id.clone(), handle);
        Ok(cell_id)
    }

    fn wait(
        &self,
        cell_id: &str,
        cursor: u64,
        yield_ms: u64,
    ) -> BoxFuture<'_, std::result::Result<CommandCellDelta, String>> {
        let cell_id = cell_id.to_string();
        Box::pin(async move {
            let handle = self
                .cells
                .get(&cell_id)
                .map(|entry| entry.value().clone())
                .ok_or_else(|| format!("cell '{cell_id}' not found"))?;

            loop {
                // 先注册 waiter 再快照: 否则 snapshot 与 await 之间的
                // notify_waiters() 会丢失唤醒。用 noop waker 强制完成一次
                // poll 以完成注册。
                let mut notified = std::pin::pin!(handle.notify.notified());
                let registered_pending = {
                    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
                    notified.as_mut().poll(&mut cx).is_pending()
                };

                let state = handle.current_state().await;
                // 锁只在块内持有, 避免 MutexGuard 跨 await。
                let ready = {
                    let output = handle
                        .output
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let bytes_since = output.total_bytes.saturating_sub(cursor);
                    state.phase.is_terminal() || bytes_since > 0
                };

                if ready {
                    return Ok(snapshot_delta(&handle, cursor, state).await);
                }
                if yield_ms == 0 || !registered_pending {
                    // 非阻塞 poll, 或 waiter 已带信号(罕见): 直接返回当前快照。
                    return Ok(snapshot_delta(&handle, cursor, state).await);
                }

                match tokio::time::timeout(Duration::from_millis(yield_ms), notified).await {
                    Ok(()) => continue,
                    Err(_) => {
                        // yield 超时: 重读一次快照(与唤醒可能竞争, 以最后读取为准)。
                        let state = handle.current_state().await;
                        return Ok(snapshot_delta(&handle, cursor, state).await);
                    }
                }
            }
        })
    }

    fn stop(&self, cell_id: &str) -> bool {
        match self.cells.get(cell_id) {
            Some(entry) => {
                entry.value().cancel.cancel();
                true
            }
            None => false,
        }
    }

    fn list(&self) -> BoxFuture<'_, Vec<CommandCellSnapshot>> {
        Box::pin(async move {
            let mut snapshots = Vec::with_capacity(self.cells.len());
            for entry in self.cells.iter() {
                snapshots.push(entry.value().snapshot().await);
            }
            snapshots
        })
    }
}

// ── Runner ──────────────────────────────────────────────────────────

/// Run one cell to completion and return its final phase + exit code.
///
/// Every path (normal exit / timeout / cancel / launch failure / semaphore
/// closed) funnels into a single return so the caller can write the terminal
/// state exactly once.
struct CellRunSpec {
    command: String,
    working_dir: Option<PathBuf>,
    timeout: Option<Duration>,
    sandbox: Option<Arc<dyn SandboxExecutor>>,
    owner_cancel: Option<Arc<CancellationToken>>,
    max_retained: usize,
}

async fn run_cell(
    handle: Arc<CommandCellHandle>,
    semaphore: Arc<Semaphore>,
    spec: CellRunSpec,
) -> (CommandCellPhase, Option<i32>) {
    let CellRunSpec {
        command,
        working_dir,
        timeout,
        sandbox,
        owner_cancel,
        max_retained,
    } = spec;
    // 并发许可: 满则排队等待(不阻塞 launch 调用方)。取消必须能打断
    // 排队本身,不能等前一个小时级命令释放许可后才进入终态。
    let acquire = semaphore.acquire_owned();
    tokio::pin!(acquire);
    let _permit = match tokio::select! {
        _ = cancellation_requested(&handle.cancel, owner_cancel.as_ref()) => {
            return (CommandCellPhase::Cancelled, None);
        }
        result = &mut acquire => result,
    } {
        Ok(permit) => permit,
        Err(_) => {
            tracing::warn!(cell_id = %handle.cell_id, "cell semaphore closed");
            return (CommandCellPhase::LaunchFailed, None);
        }
    };

    if handle.cancel.is_cancelled()
        || owner_cancel
            .as_ref()
            .is_some_and(|cancel| cancel.is_cancelled())
    {
        return (CommandCellPhase::Cancelled, None);
    }

    if let Some(sandbox) = sandbox {
        return run_sandbox_cell(
            handle,
            command,
            working_dir,
            timeout,
            sandbox,
            owner_cancel,
            max_retained,
        )
        .await;
    }

    // env_clear + allowlist: 防止把密钥泄漏给后台进程; PATH/HOME 必需,
    // LANG/LC_ALL 保证 UTF-8 输出不被 C locale 破坏, TMPDIR/TZ 是安全的
    // 功能性变量。(与旧 spawn_task 的策略一致。)
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c").arg(&command).env_clear();
    for var in ["PATH", "HOME", "LANG", "LC_ALL", "TMPDIR", "TZ"] {
        if let Ok(val) = std::env::var(var) {
            cmd.env(var, val);
        }
    }
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    if let Some(dir) = &working_dir {
        cmd.current_dir(dir);
    }

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(error) => {
            tracing::warn!(cell_id = %handle.cell_id, error = %error, "cell spawn failed");
            return (CommandCellPhase::LaunchFailed, None);
        }
    };

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_join = stdout.map(|pipe| {
        let handle = handle.clone();
        tokio::spawn(async move {
            read_pipe(pipe, handle, max_retained, ToolOutputChannel::Stdout).await
        })
    });
    let stderr_join = stderr.map(|pipe| {
        let handle = handle.clone();
        tokio::spawn(async move {
            read_pipe(pipe, handle, max_retained, ToolOutputChannel::Stderr).await
        })
    });

    let (phase, exit_code) = tokio::select! {
        _ = cancellation_requested(&handle.cancel, owner_cancel.as_ref()) => {
            kill_process_group(&mut child).await;
            (CommandCellPhase::Cancelled, None)
        }
        _ = async {
            match timeout {
                Some(duration) => tokio::time::sleep(duration).await,
                None => std::future::pending().await,
            }
        } => {
            // 超时视为失败(exit_code = None)。
            kill_process_group(&mut child).await;
            (CommandCellPhase::Failed, None)
        }
        status = child.wait() => match status {
            Ok(status) => {
                let phase = if status.success() {
                    CommandCellPhase::Succeeded
                } else {
                    CommandCellPhase::Failed
                };
                (phase, status.code())
            }
            Err(_) => (CommandCellPhase::Failed, None),
        },
    };

    // 收尾: 等 reader 把管道剩余数据全部入账, 保证终态可见的输出是完整的。
    if let Some(join) = stdout_join {
        let _ = join.await;
    }
    if let Some(join) = stderr_join {
        let _ = join.await;
    }

    (phase, exit_code)
}

async fn run_sandbox_cell(
    handle: Arc<CommandCellHandle>,
    command: String,
    working_dir: Option<PathBuf>,
    timeout: Option<Duration>,
    sandbox: Arc<dyn SandboxExecutor>,
    owner_cancel: Option<Arc<CancellationToken>>,
    max_retained: usize,
) -> (CommandCellPhase, Option<i32>) {
    // SandboxCommand currently has a concrete timeout. The outer deadline is
    // authoritative; a None cell timeout maps to a practically unbounded
    // backend deadline while remaining cancellable through the stream drop.
    let backend_timeout = timeout.unwrap_or_else(|| Duration::from_secs(100 * 365 * 24 * 60 * 60));
    let mut sandbox_command = SandboxCommand::shell(command);
    sandbox_command.timeout = backend_timeout;
    if let Some(dir) = &working_dir {
        sandbox_command = sandbox_command.with_working_dir(dir);
    }

    let deadline = async {
        match timeout {
            Some(duration) => tokio::time::sleep(duration).await,
            None => std::future::pending().await,
        }
    };
    tokio::pin!(deadline);

    let stream = tokio::select! {
        _ = cancellation_requested(&handle.cancel, owner_cancel.as_ref()) => {
            return (CommandCellPhase::Cancelled, None);
        }
        _ = &mut deadline => {
            return (CommandCellPhase::Failed, None);
        }
        result = sandbox.execute_stream(sandbox_command) => result,
    };
    let mut stream = match stream {
        Ok(stream) => stream,
        Err(error) => {
            let message = format!("Sandbox execution failed: {error}\n");
            append_output(
                &handle,
                message.as_bytes(),
                max_retained,
                ToolOutputChannel::Stderr,
            );
            return (CommandCellPhase::LaunchFailed, None);
        }
    };
    let mut saw_stream_output = false;

    loop {
        let event = tokio::select! {
            _ = cancellation_requested(&handle.cancel, owner_cancel.as_ref()) => {
                return (CommandCellPhase::Cancelled, None);
            }
            _ = &mut deadline => {
                return (CommandCellPhase::Failed, None);
            }
            event = stream.next() => event,
        };
        let Some(event) = event else {
            return (CommandCellPhase::Failed, None);
        };
        match event {
            SandboxStreamEvent::Output { channel, chunk } => {
                saw_stream_output = true;
                let channel = match channel {
                    SandboxOutputChannel::Stdout => ToolOutputChannel::Stdout,
                    SandboxOutputChannel::Stderr => ToolOutputChannel::Stderr,
                };
                append_output(&handle, chunk.as_bytes(), max_retained, channel);
            }
            SandboxStreamEvent::Complete(result) => {
                if !saw_stream_output {
                    if !result.stdout.is_empty() {
                        append_output(
                            &handle,
                            result.stdout.as_bytes(),
                            max_retained,
                            ToolOutputChannel::Stdout,
                        );
                    }
                    if !result.stderr.is_empty() {
                        append_output(
                            &handle,
                            result.stderr.as_bytes(),
                            max_retained,
                            ToolOutputChannel::Stderr,
                        );
                    }
                }
                if result.output_truncated {
                    let logical_bytes = result.stdout_bytes.saturating_add(result.stderr_bytes);
                    let mut output = handle
                        .output
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    output.total_bytes = output.total_bytes.max(logical_bytes);
                    output.truncated = true;
                }
                if result.cancelled {
                    return (CommandCellPhase::Cancelled, None);
                }
                if result.timed_out {
                    return (CommandCellPhase::Failed, None);
                }
                let phase = if result.success() {
                    CommandCellPhase::Succeeded
                } else {
                    CommandCellPhase::Failed
                };
                return (phase, Some(result.exit_code));
            }
        }
    }
}

/// Read a pipe to EOF, appending chunks into the cell's output buffer and
/// waking all waiters after each append.
async fn read_pipe<R: tokio::io::AsyncRead + Unpin>(
    mut pipe: R,
    handle: Arc<CommandCellHandle>,
    max_retained: usize,
    channel: ToolOutputChannel,
) {
    let mut buffer = vec![0_u8; READ_CHUNK_BYTES];
    loop {
        match pipe.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(count) => {
                let bytes = buffer.get(..count).unwrap_or_default();
                append_output(&handle, bytes, max_retained, channel);
            }
        }
    }
}

async fn cancellation_requested(
    cell_cancel: &CancellationToken,
    owner_cancel: Option<&Arc<CancellationToken>>,
) {
    tokio::select! {
        _ = cell_cancel.cancelled() => {}
        _ = async {
            match owner_cancel {
                Some(cancel) => cancel.cancelled().await,
                None => std::future::pending().await,
            }
        } => {}
    }
}

fn append_output(
    handle: &CommandCellHandle,
    bytes: &[u8],
    max_retained: usize,
    channel: ToolOutputChannel,
) {
    {
        let mut output = handle
            .output
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        output.push(bytes, max_retained);
    }
    let text = String::from_utf8_lossy(bytes);
    let mut writer = handle
        .artifact_writer
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(active) = writer.as_mut()
        && let Err(error) = active.push_channel(channel, &text)
    {
        tracing::warn!(cell_id = %handle.cell_id, %error, "cell output artifact write failed");
        *writer = None;
    }
    drop(writer);
    handle.notify.notify_waiters();
}

fn finish_output_artifact(handle: &CommandCellHandle) {
    let writer = handle
        .artifact_writer
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    let Some(writer) = writer else {
        return;
    };
    match writer.finish() {
        Ok(Some(artifact)) => {
            *handle
                .output_artifact
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(artifact);
        }
        Ok(None) => {}
        Err(error) => {
            tracing::warn!(cell_id = %handle.cell_id, %error, "cell output artifact finalize failed");
        }
    }
}

/// Kill the child's whole process group, then reap the child. Best-effort:
/// errors are ignored (the cell still converges to a terminal phase).
async fn kill_process_group(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        let _ = std::process::Command::new("kill")
            .args(["-KILL", &format!("-{pid}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::sandbox::{ExecutionResult, IsolationLevel};
    use echo_core::tools::cell::CommandCellRegistry;

    struct TestSandbox {
        executions: AtomicU64,
    }

    impl SandboxExecutor for TestSandbox {
        fn name(&self) -> &str {
            "cell-test-sandbox"
        }

        fn isolation_level(&self) -> IsolationLevel {
            IsolationLevel::Process
        }

        fn is_available(&self) -> BoxFuture<'_, bool> {
            Box::pin(async { true })
        }

        fn execute(
            &self,
            _command: SandboxCommand,
        ) -> BoxFuture<'_, echo_core::error::Result<ExecutionResult>> {
            self.executions.fetch_add(1, Ordering::Relaxed);
            Box::pin(async {
                Ok(ExecutionResult {
                    exit_code: 0,
                    stdout: "sandbox-cell-ok\n".to_string(),
                    stderr: String::new(),
                    duration: Duration::from_millis(1),
                    sandbox_type: "cell-test-sandbox".to_string(),
                    timed_out: false,
                    cancelled: false,
                    output_truncated: false,
                    stdout_bytes: 16,
                    stderr_bytes: 0,
                })
            })
        }
    }

    fn manager() -> BackgroundCommandManager {
        BackgroundCommandManager::new(BackgroundCommandManagerConfig::default())
    }

    fn request(command: &str) -> CommandCellRequest {
        CommandCellRequest {
            command: command.to_string(),
            working_dir: None,
            timeout_secs: None,
            ..Default::default()
        }
    }

    /// 循环 wait 直到终态, 拼接全部增量输出。wait 会在"有新输出"时就返回
    /// (可能尚未终态), 因此测试里用它收集完整结果。
    async fn drain_to_terminal(
        manager: &BackgroundCommandManager,
        cell_id: &str,
    ) -> (String, CommandCellDelta) {
        let mut cursor = 0_u64;
        let mut combined = String::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            let delta = manager.wait(cell_id, cursor, 10_000).await.unwrap();
            assert!(
                tokio::time::Instant::now() < deadline,
                "cell {cell_id} did not reach terminal phase in time"
            );
            combined.push_str(&delta.new_output);
            cursor = delta.next_cursor;
            if delta.snapshot.phase.is_terminal() && cursor >= delta.snapshot.total_output_bytes {
                return (combined, delta);
            }
        }
    }

    #[tokio::test]
    async fn echo_cell_completes_and_captures_output() {
        let manager = manager();
        let cell_id = manager.launch(request("echo hello-cell")).unwrap();

        let (combined, delta) = drain_to_terminal(&manager, &cell_id).await;
        assert_eq!(delta.snapshot.phase, CommandCellPhase::Succeeded);
        assert_eq!(delta.snapshot.exit_code, Some(0));
        assert!(combined.contains("hello-cell"), "got: {combined}");
        assert!(delta.next_cursor > 0);
    }

    #[tokio::test]
    async fn wait_short_yield_then_retry_reaches_terminal() {
        let manager = manager();
        let cell_id = manager
            .launch(request("sleep 0.4; echo done-retry"))
            .unwrap();

        // 短 yield: cell 仍在运行, 返回空增量(retry-safe, 不消费任何状态)。
        let first = manager.wait(&cell_id, 0, 50).await.unwrap();
        if !first.snapshot.phase.is_terminal() {
            assert!(first.new_output.is_empty());
        }

        // 再次 wait(长 yield): 继续推进直到终态, 拿到完整输出。
        let (combined, last) = drain_to_terminal(&manager, &cell_id).await;
        assert!(combined.contains("done-retry"));

        // 终态可重复读取(多等待者语义)。
        let third = manager.wait(&cell_id, last.next_cursor, 0).await.unwrap();
        assert_eq!(third.snapshot.phase, CommandCellPhase::Succeeded);
        assert!(third.new_output.is_empty());
    }

    #[tokio::test]
    async fn incremental_output_spliced_across_waits() {
        let manager = manager();
        let cell_id = manager
            .launch(request("echo one; sleep 1; echo two"))
            .unwrap();

        let mut combined = String::new();
        let mut cursor = 0_u64;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            let delta = manager.wait(&cell_id, cursor, 10_000).await.unwrap();
            combined.push_str(&delta.new_output);
            cursor = delta.next_cursor;
            if delta.snapshot.phase.is_terminal() {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "cell did not finish in time"
            );
        }

        assert!(combined.contains("one"), "combined: {combined}");
        assert!(combined.contains("two"), "combined: {combined}");
    }

    #[tokio::test]
    async fn stop_cancels_long_running_cell() {
        let manager = manager();
        let cell_id = manager.launch(request("sleep 30")).unwrap();

        let running = manager.wait(&cell_id, 0, 200).await.unwrap();
        assert_eq!(running.snapshot.phase, CommandCellPhase::Running);

        assert!(manager.stop(&cell_id));
        let cancelled = manager
            .wait(&cell_id, running.next_cursor, 10_000)
            .await
            .unwrap();
        assert_eq!(cancelled.snapshot.phase, CommandCellPhase::Cancelled);

        // 未知 cell → false。
        assert!(!manager.stop("no-such-cell"));
    }

    #[tokio::test]
    async fn unknown_cell_wait_returns_error() {
        let manager = manager();
        let result = manager.wait("does-not-exist", 0, 10).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn long_unicode_output_is_incrementally_drainable_without_panic() {
        let manager = manager();
        // 多字节输出超过单轮 16 KiB；每一轮都必须推进真实字节游标，不能
        // 因终态或字符截断直接跳到 total 而永久漏掉中间输出。
        let cell_id = manager
            .launch(request(
                "for i in $(seq 1 3000); do echo \"中文输出🦀行$i\"; done",
            ))
            .unwrap();

        let (combined, last) = drain_to_terminal(&manager, &cell_id).await;
        assert_eq!(last.snapshot.phase, CommandCellPhase::Succeeded);
        assert!(combined.contains('🦀'));

        let first = manager.wait(&cell_id, 0, 0).await.unwrap();
        assert_eq!(first.snapshot.phase, CommandCellPhase::Succeeded);
        assert!(
            first.new_output.len() <= MAX_DELTA_BYTES.saturating_add(80),
            "byte cap exceeded: {}",
            first.new_output.len()
        );
        assert!(
            first.output_elided,
            "expected elided flag for capped output"
        );
        assert!(first.next_cursor < first.snapshot.total_output_bytes);
        assert!(!first.new_output.contains('\u{fffd}'));

        let mut cursor = first.next_cursor;
        let mut rounds = 1_u32;
        while cursor < first.snapshot.total_output_bytes {
            let delta = manager.wait(&cell_id, cursor, 0).await.unwrap();
            assert!(!delta.new_output.contains('\u{fffd}'));
            assert!(
                delta.next_cursor > cursor,
                "cursor must advance on every drain"
            );
            cursor = delta.next_cursor;
            rounds = rounds.saturating_add(1);
        }
        assert!(rounds > 1);
        assert_eq!(cursor, first.snapshot.total_output_bytes);
    }

    #[tokio::test]
    async fn queued_cell_cancellation_does_not_wait_for_a_permit() {
        let manager = BackgroundCommandManager::new(BackgroundCommandManagerConfig {
            max_concurrent: 1,
            ..Default::default()
        });
        let occupying = manager.launch(request("sleep 30")).unwrap();
        let first = manager.wait(&occupying, 0, 200).await.unwrap();
        assert_eq!(first.snapshot.phase, CommandCellPhase::Running);

        let queued = manager.launch(request("echo should-not-run")).unwrap();
        assert!(manager.stop(&queued));
        let cancelled = manager.wait(&queued, 0, 1_000).await.unwrap();
        assert_eq!(cancelled.snapshot.phase, CommandCellPhase::Cancelled);

        let occupying_state = manager.wait(&occupying, 0, 0).await.unwrap();
        assert_eq!(occupying_state.snapshot.phase, CommandCellPhase::Running);
        manager.stop(&occupying);
        let _ = manager.wait(&occupying, 0, 10_000).await;
    }

    #[tokio::test]
    async fn owning_run_cancellation_propagates_to_the_cell() {
        let manager = manager();
        let cancel = Arc::new(CancellationToken::new());
        let mut launch = request("sleep 30");
        launch.cancel = Some(cancel.clone());
        let cell_id = manager.launch(launch).unwrap();
        let running = manager.wait(&cell_id, 0, 200).await.unwrap();
        assert_eq!(running.snapshot.phase, CommandCellPhase::Running);

        cancel.cancel();
        let terminal = manager.wait(&cell_id, 0, 10_000).await.unwrap();
        assert_eq!(terminal.snapshot.phase, CommandCellPhase::Cancelled);
    }

    #[tokio::test]
    async fn complete_output_spills_to_the_existing_artifact_writer() {
        let root = tempfile::tempdir().unwrap();
        let manager = manager();
        let mut launch = request("printf 'artifact-output-abcdefghijklmnopqrstuvwxyz'");
        launch.output_artifacts = Some(
            echo_core::tools::artifact::ToolOutputArtifactConfig::new(root.path(), "test")
                .threshold_bytes(8),
        );
        launch.artifact_identity = Some(echo_core::tools::artifact::ToolOutputArtifactIdentity {
            conversation_id: Some("conversation".to_string()),
            run_id: Some("run".to_string()),
            call_id: "call".to_string(),
            tool_name: "shell".to_string(),
        });
        let cell_id = manager.launch(launch).unwrap();
        let (_, terminal) = drain_to_terminal(&manager, &cell_id).await;
        let artifact = terminal
            .snapshot
            .output_artifact
            .as_ref()
            .expect("large cell output must spill");
        assert!(artifact.path.exists());
        let persisted = std::fs::read_to_string(&artifact.path).unwrap();
        assert!(persisted.contains("artifact-output-abcdefghijklmnopqrstuvwxyz"));
    }

    #[tokio::test]
    async fn sandbox_required_launch_uses_the_configured_executor() {
        let sandbox = Arc::new(TestSandbox {
            executions: AtomicU64::new(0),
        });
        let manager = BackgroundCommandManager::new_with_sandbox(
            BackgroundCommandManagerConfig::default(),
            sandbox.clone(),
        );
        let mut launch = request("echo ignored-by-test-executor");
        launch.require_sandbox = true;
        let cell_id = manager.launch(launch).unwrap();
        let (output, terminal) = drain_to_terminal(&manager, &cell_id).await;
        assert_eq!(terminal.snapshot.phase, CommandCellPhase::Succeeded);
        assert!(output.contains("sandbox-cell-ok"));
        assert_eq!(sandbox.executions.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn sandbox_required_launch_never_silently_downgrades() {
        let manager = manager();
        let mut launch = request("echo must-not-run-directly");
        launch.require_sandbox = true;
        assert!(manager.launch(launch).is_err());
    }

    #[tokio::test]
    async fn retained_buffer_tail_only_marks_truncation() {
        let manager = BackgroundCommandManager::new(BackgroundCommandManagerConfig {
            max_retained_output_bytes: 1024,
            ..Default::default()
        });
        // ~7 bytes × 400 行 ≈ 2.8 KB > 1 KB retention。
        let cell_id = manager.launch(request("seq 1 400")).unwrap();
        let delta = manager.wait(&cell_id, 0, 20_000).await.unwrap();

        assert!(delta.snapshot.output_truncated);
        assert!(delta.snapshot.total_output_bytes > 1024);
        // cursor=0 早于 retained 起点: 有被丢弃字节的提示。
        assert!(delta.new_output.contains("discarded"));
        assert!(delta.output_elided);

        let (_, last) = drain_to_terminal(&manager, &cell_id).await;
        assert_eq!(last.snapshot.phase, CommandCellPhase::Succeeded);
    }

    #[tokio::test]
    async fn timeout_marks_cell_failed() {
        let manager = BackgroundCommandManager::new(BackgroundCommandManagerConfig {
            default_timeout_secs: 1,
            ..Default::default()
        });
        let cell_id = manager.launch(request("sleep 30")).unwrap();
        let delta = manager.wait(&cell_id, 0, 10_000).await.unwrap();
        assert_eq!(delta.snapshot.phase, CommandCellPhase::Failed);
        assert_eq!(delta.snapshot.exit_code, None);
    }

    #[tokio::test]
    async fn bad_working_dir_marks_launch_failed() {
        let manager = manager();
        let cell_id = manager
            .launch(CommandCellRequest {
                command: "echo x".to_string(),
                working_dir: Some("/nonexistent-cell-dir-xyz".to_string()),
                timeout_secs: None,
                ..Default::default()
            })
            .unwrap();
        let delta = manager.wait(&cell_id, 0, 10_000).await.unwrap();
        assert_eq!(delta.snapshot.phase, CommandCellPhase::LaunchFailed);
    }

    #[tokio::test]
    async fn launch_rejects_empty_command() {
        let manager = manager();
        assert!(manager.launch(request("   ")).is_err());
    }

    #[tokio::test]
    async fn list_contains_running_and_terminal_cells() {
        let manager = manager();
        let quick = manager.launch(request("echo quick")).unwrap();
        let slow = manager.launch(request("sleep 30")).unwrap();

        let (_, done) = drain_to_terminal(&manager, &quick).await;
        assert_eq!(done.snapshot.phase, CommandCellPhase::Succeeded);

        let listed = manager.list().await;
        let quick_snapshot = listed.iter().find(|s| s.cell_id == quick);
        let slow_snapshot = listed.iter().find(|s| s.cell_id == slow);
        assert!(quick_snapshot.is_some_and(|s| s.phase.is_terminal()));
        assert!(slow_snapshot.is_some_and(|s| !s.phase.is_terminal()));

        manager.stop(&slow);
        let _ = manager.wait(&slow, 0, 10_000).await;
    }
}
