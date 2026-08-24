//! Process-wide admission and ordering for blocking file operations.
//!
//! File-backed async traits submit owned closures here instead of calling
//! `std::fs` on a Tokio runtime thread. Once admitted, an operation is owned by
//! an internal task and completes even when its caller is dropped. Operations
//! with the same key execute in submission order; unrelated keys may execute
//! concurrently within the process-wide bound.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};

use futures::future::{BoxFuture, FutureExt, Shared, ready};
use tokio::sync::{Semaphore, oneshot};

/// Maximum number of file operations accepted but not yet settled.
pub const PROCESS_FILE_OPERATION_CAPACITY: usize = 64;
/// Maximum number of closures simultaneously occupying blocking threads.
pub const PROCESS_FILE_OPERATION_CONCURRENCY: usize = 8;

static ADMISSION: LazyLock<Arc<Semaphore>> =
    LazyLock::new(|| Arc::new(Semaphore::new(PROCESS_FILE_OPERATION_CAPACITY)));
static EXECUTION: LazyLock<Arc<Semaphore>> =
    LazyLock::new(|| Arc::new(Semaphore::new(PROCESS_FILE_OPERATION_CONCURRENCY)));
static PROCESS_OWNER: LazyLock<Result<tokio::runtime::Runtime, std::io::Error>> =
    LazyLock::new(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("echo-file-owner")
            .build()
    });
static KEY_TAILS: LazyLock<Mutex<HashMap<BlockingFileOperationKey, KeyTail>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

type SharedCompletion = Shared<BoxFuture<'static, ()>>;

struct KeyTail {
    generation: u64,
    completion: SharedCompletion,
}

/// Typed scope for a blocking file operation.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum BlockingFileOperationScope {
    /// Operations against one independently ordered record.
    Entity(String),
    /// Operations against a store-wide projection or scan.
    Collection(String),
}

/// Collision-free ordering key for a file-backed store operation.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct BlockingFileOperationKey {
    namespace: String,
    canonical_root: PathBuf,
    scope: BlockingFileOperationScope,
}

impl BlockingFileOperationKey {
    /// Build a key from independently hashed components.
    ///
    /// `canonical_root` must already be canonical or otherwise identity-stable.
    /// This constructor does not touch the filesystem because it is called from
    /// async admission paths. Passing aliases for one directory would create
    /// separate ordering domains.
    pub fn new(
        namespace: impl Into<String>,
        canonical_root: impl Into<PathBuf>,
        scope: BlockingFileOperationScope,
    ) -> Self {
        Self {
            namespace: namespace.into(),
            canonical_root: canonical_root.into(),
            scope,
        }
    }
}

/// Infrastructure failure outside the submitted file closure.
#[derive(Debug, thiserror::Error)]
pub enum BlockingFileOperationError {
    #[error("blocking file operation admission is closed")]
    AdmissionClosed,
    #[error("blocking file operation execution is closed")]
    ExecutionClosed,
    #[error("blocking file operation task failed: {0}")]
    Join(String),
    #[error("blocking file operation owner ended before publishing its result")]
    OwnerClosed,
    #[error("blocking file operation key registry is poisoned")]
    RegistryPoisoned,
    #[error("blocking file operation requires an active Tokio runtime")]
    RuntimeUnavailable,
    #[error("process file operation owner is unavailable: {0}")]
    ProcessOwnerUnavailable(String),
    #[error("blocking file operation key generation is exhausted")]
    KeyGenerationExhausted,
}

/// Run an owned blocking file closure under one process-wide bound.
///
/// The future may wait for admission. After admission succeeds, no cancellation
/// point exists before the owner task is spawned. Dropping the returned future
/// after that point only drops the result receiver; the closure, key ordering,
/// and permits remain owned until settlement.
pub async fn run_keyed_file_operation<T, F>(
    key: BlockingFileOperationKey,
    operation: F,
) -> Result<T, BlockingFileOperationError>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    tokio::runtime::Handle::try_current()
        .map_err(|_| BlockingFileOperationError::RuntimeUnavailable)?;
    let owner = PROCESS_OWNER
        .as_ref()
        .map_err(|error| BlockingFileOperationError::ProcessOwnerUnavailable(error.to_string()))?;
    let admission = ADMISSION
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| BlockingFileOperationError::AdmissionClosed)?;
    let (previous, completion_tx, generation) = enqueue_key(&key)?;
    let (result_tx, result_rx) = oneshot::channel();
    let owner_key = key.clone();
    owner.spawn(async move {
        previous.await;
        let outcome = match EXECUTION.clone().acquire_owned().await {
            Ok(execution) => tokio::task::spawn_blocking(move || {
                let _execution = execution;
                operation()
            })
            .await
            .map_err(|error| BlockingFileOperationError::Join(error.to_string())),
            Err(_) => Err(BlockingFileOperationError::ExecutionClosed),
        };
        let _ignored = completion_tx.send(());
        finish_key(&owner_key, generation);
        drop(admission);
        let _ignored = result_tx.send(outcome);
    });
    result_rx
        .await
        .map_err(|_| BlockingFileOperationError::OwnerClosed)?
}

fn enqueue_key(
    key: &BlockingFileOperationKey,
) -> Result<(SharedCompletion, oneshot::Sender<()>, u64), BlockingFileOperationError> {
    let mut tails = KEY_TAILS
        .lock()
        .map_err(|_| BlockingFileOperationError::RegistryPoisoned)?;
    let (previous, generation) = match tails.get(key) {
        Some(tail) => (
            tail.completion.clone(),
            tail.generation
                .checked_add(1)
                .ok_or(BlockingFileOperationError::KeyGenerationExhausted)?,
        ),
        None => (ready(()).boxed().shared(), 1),
    };
    let (completion_tx, completion_rx) = oneshot::channel();
    let completion = completion_rx.map(|_| ()).boxed().shared();
    tails.insert(
        key.clone(),
        KeyTail {
            generation,
            completion,
        },
    );
    Ok((previous, completion_tx, generation))
}

fn finish_key(key: &BlockingFileOperationKey, generation: u64) {
    let Ok(mut tails) = KEY_TAILS.lock() else {
        return;
    };
    if tails
        .get(key)
        .is_some_and(|tail| tail.generation == generation)
    {
        tails.remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    static ASYNC_TEST_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    fn entity_key(scope: impl Into<String>) -> BlockingFileOperationKey {
        BlockingFileOperationKey::new(
            "blocking-test",
            PathBuf::from("process-owner"),
            BlockingFileOperationScope::Entity(scope.into()),
        )
    }

    #[test]
    fn missing_runtime_returns_typed_error_without_panicking() -> Result<(), String> {
        let outcome =
            futures::executor::block_on(run_keyed_file_operation(entity_key("no-runtime"), || ()));
        assert!(matches!(
            outcome,
            Err(BlockingFileOperationError::RuntimeUnavailable)
        ));
        Ok(())
    }

    #[test]
    fn exhausted_key_generation_fails_closed() -> Result<(), String> {
        let key = entity_key("generation-exhaustion");
        {
            let mut tails = KEY_TAILS.lock().map_err(|error| error.to_string())?;
            tails.insert(
                key.clone(),
                KeyTail {
                    generation: u64::MAX,
                    completion: ready(()).boxed().shared(),
                },
            );
        }
        let outcome = enqueue_key(&key);
        assert!(matches!(
            outcome,
            Err(BlockingFileOperationError::KeyGenerationExhausted)
        ));
        let mut tails = KEY_TAILS.lock().map_err(|error| error.to_string())?;
        tails.remove(&key);
        Ok(())
    }

    #[test]
    fn typed_keys_do_not_alias_delimiters_or_collection_names() {
        let entity = BlockingFileOperationKey::new(
            "conversation:store",
            PathBuf::from("root:one"),
            BlockingFileOperationScope::Entity("__list__".to_string()),
        );
        let collection = BlockingFileOperationKey::new(
            "conversation:store",
            PathBuf::from("root:one"),
            BlockingFileOperationScope::Collection("list".to_string()),
        );
        let split_differently = BlockingFileOperationKey::new(
            "conversation",
            PathBuf::from("store:root:one"),
            BlockingFileOperationScope::Entity("__list__".to_string()),
        );
        assert_ne!(entity, collection);
        assert_ne!(entity, split_differently);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn blocking_operation_does_not_stall_runtime_heartbeat() -> Result<(), String> {
        let _serial = ASYNC_TEST_SERIAL.lock().await;
        let (entered_tx, entered_rx) = oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let operation = tokio::spawn(run_keyed_file_operation(
            entity_key("heartbeat"),
            move || {
                let _ignored = entered_tx.send(());
                release_rx
                    .recv_timeout(Duration::from_secs(2))
                    .map_err(|error| error.to_string())
            },
        ));
        entered_rx.await.map_err(|error| error.to_string())?;
        tokio::time::timeout(Duration::from_millis(250), async {
            for _ in 0..64 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| "runtime heartbeat stalled".to_string())?;
        release_tx.send(()).map_err(|error| error.to_string())?;
        operation
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    #[tokio::test]
    async fn caller_abort_does_not_cancel_admitted_operation() -> Result<(), String> {
        let _serial = ASYNC_TEST_SERIAL.lock().await;
        let completed = Arc::new(AtomicBool::new(false));
        let completed_by_operation = Arc::clone(&completed);
        let (entered_tx, entered_rx) = oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let caller = tokio::spawn(run_keyed_file_operation(entity_key("abort"), move || {
            let _ignored = entered_tx.send(());
            let _ignored = release_rx.recv_timeout(Duration::from_secs(2));
            completed_by_operation.store(true, Ordering::Release);
        }));
        entered_rx.await.map_err(|error| error.to_string())?;
        caller.abort();
        release_tx.send(()).map_err(|error| error.to_string())?;
        tokio::time::timeout(Duration::from_secs(1), async {
            while !completed.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| "aborted caller cancelled owned operation".to_string())?;
        Ok(())
    }

    #[tokio::test]
    async fn same_key_is_ordered_and_different_keys_are_bounded() -> Result<(), String> {
        let _serial = ASYNC_TEST_SERIAL.lock().await;
        let order = Arc::new(Mutex::new(Vec::new()));
        let first_finished = Arc::new(AtomicBool::new(false));
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let first_order = Arc::clone(&order);
        let first_finished_by_operation = Arc::clone(&first_finished);
        let first = tokio::spawn(run_keyed_file_operation(entity_key("ordered"), move || {
            if let Ok(mut order) = first_order.lock() {
                order.push(1);
            }
            let _ignored = release_rx.recv_timeout(Duration::from_secs(2));
            first_finished_by_operation.store(true, Ordering::Release);
        }));
        tokio::task::yield_now().await;
        let second_order = Arc::clone(&order);
        let first_finished_for_second = Arc::clone(&first_finished);
        let ordering_violation = Arc::new(AtomicBool::new(false));
        let ordering_violation_by_operation = Arc::clone(&ordering_violation);
        let second = tokio::spawn(run_keyed_file_operation(entity_key("ordered"), move || {
            if !first_finished_for_second.load(Ordering::Acquire) {
                ordering_violation_by_operation.store(true, Ordering::Release);
            }
            if let Ok(mut order) = second_order.lock() {
                order.push(2);
            }
        }));
        tokio::task::yield_now().await;
        release_tx.send(()).map_err(|error| error.to_string())?;
        first
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        second
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        assert_eq!(
            order.lock().map_err(|error| error.to_string())?.as_slice(),
            &[1, 2]
        );
        assert!(!ordering_violation.load(Ordering::Acquire));

        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let release = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let mut operations = Vec::new();
        for index in 0..PROCESS_FILE_OPERATION_CONCURRENCY.saturating_mul(2) {
            let active = Arc::clone(&active);
            let maximum = Arc::clone(&maximum);
            let release = Arc::clone(&release);
            operations.push(tokio::spawn(run_keyed_file_operation(
                entity_key(format!("parallel-{index}")),
                move || {
                    let current = active.fetch_add(1, Ordering::AcqRel).saturating_add(1);
                    maximum.fetch_max(current, Ordering::AcqRel);
                    let (released, notification) = &*release;
                    if let Ok(mut released) = released.lock() {
                        while !*released {
                            match notification.wait(released) {
                                Ok(next) => released = next,
                                Err(_) => break,
                            }
                        }
                    }
                    active.fetch_sub(1, Ordering::AcqRel);
                },
            )));
        }
        tokio::time::timeout(Duration::from_secs(2), async {
            while maximum.load(Ordering::Acquire) < PROCESS_FILE_OPERATION_CONCURRENCY {
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| "blocking operation concurrency did not fill its bound".to_string())?;
        let (released, notification) = &*release;
        {
            let mut released = released.lock().map_err(|error| error.to_string())?;
            *released = true;
            notification.notify_all();
        }
        for operation in operations {
            operation
                .await
                .map_err(|error| error.to_string())?
                .map_err(|error| error.to_string())?;
        }
        let observed = maximum.load(Ordering::Acquire);
        assert_eq!(observed, PROCESS_FILE_OPERATION_CONCURRENCY);
        Ok(())
    }

    #[test]
    fn caller_runtime_shutdown_does_not_release_same_key_tail() -> Result<(), String> {
        let _serial = ASYNC_TEST_SERIAL.blocking_lock();
        let key = entity_key("runtime-shutdown");
        let first_finished = Arc::new(AtomicBool::new(false));
        let first_finished_by_operation = Arc::clone(&first_finished);
        let (entered_tx, entered_rx) = oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let runtime_a = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .map_err(|error| error.to_string())?;
        let first_key = key.clone();
        let _first = runtime_a.spawn(run_keyed_file_operation(first_key, move || {
            let _ignored = entered_tx.send(());
            let _ignored = release_rx.recv_timeout(Duration::from_secs(2));
            first_finished_by_operation.store(true, Ordering::Release);
        }));
        runtime_a
            .block_on(entered_rx)
            .map_err(|error| error.to_string())?;
        drop(runtime_a);

        let runtime_b = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .map_err(|error| error.to_string())?;
        let second_started = Arc::new(AtomicBool::new(false));
        let second_started_by_operation = Arc::clone(&second_started);
        let ordering_violation = Arc::new(AtomicBool::new(false));
        let ordering_violation_by_operation = Arc::clone(&ordering_violation);
        let second_key = key.clone();
        let second = runtime_b.spawn(run_keyed_file_operation(second_key, move || {
            if !first_finished.load(Ordering::Acquire) {
                ordering_violation_by_operation.store(true, Ordering::Release);
            }
            second_started_by_operation.store(true, Ordering::Release);
        }));
        runtime_b.block_on(async {
            tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    let generation = KEY_TAILS
                        .lock()
                        .map_err(|error| error.to_string())?
                        .get(&key)
                        .map(|tail| tail.generation);
                    if generation == Some(2) {
                        return Ok::<(), String>(());
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .map_err(|_| "second runtime did not enqueue its operation".to_string())??;
            assert!(!second_started.load(Ordering::Acquire));
            release_tx.send(()).map_err(|error| error.to_string())?;
            second
                .await
                .map_err(|error| error.to_string())?
                .map_err(|error| error.to_string())?;
            Ok::<(), String>(())
        })?;
        assert!(!ordering_violation.load(Ordering::Acquire));
        assert!(
            !KEY_TAILS
                .lock()
                .map_err(|error| error.to_string())?
                .contains_key(&key)
        );
        Ok(())
    }

    #[tokio::test]
    async fn panicking_operation_releases_key_and_permits() -> Result<(), String> {
        let _serial = ASYNC_TEST_SERIAL.lock().await;
        let key = entity_key("panic-release");
        let failed = run_keyed_file_operation(key.clone(), || -> () {
            // Fault injection verifies that the owner settles a panicked blocking closure.
            std::panic::resume_unwind(Box::new("injected file operation panic"));
        })
        .await;
        assert!(matches!(failed, Err(BlockingFileOperationError::Join(_))));
        run_keyed_file_operation(key.clone(), || ())
            .await
            .map_err(|error| error.to_string())?;
        assert!(
            !KEY_TAILS
                .lock()
                .map_err(|error| error.to_string())?
                .contains_key(&key)
        );
        Ok(())
    }
}
