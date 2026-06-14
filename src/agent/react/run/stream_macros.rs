//! Channel-yield macros shared by `run_core_loop` and its phase functions.
//!
//! These macros wrap `mpsc::Sender` ergonomics so phase functions don't need
//! to spell out the early-return-on-closed-channel pattern at every call site.
//!
//! All three macros short-circuit the **enclosing function** with `return Ok(())`
//! when the receiver is gone. Every phase function therefore returns
//! `Result<…>` so this short-circuit means "abandon the stream gracefully".

/// Default stream buffer capacity, used in the `Full` arm warning message.
pub(crate) const DEFAULT_STREAM_BUFFER: usize = 256;

/// Like `yield_final_event_or!` but uses blocking `send().await` and
/// returns `Ok(())` from the caller. This is the variant the loop driver
/// uses for terminal events when its own return type is `Result<()>`.
macro_rules! yield_final_event {
    ($tx:expr, $event:expr) => {
        if $tx.send(Ok($event)).await.is_err() {
            return Ok(());
        }
    };
}

#[allow(unused_imports)]
pub(crate) use yield_final_event;

// ── Phase-fn variants ────────────────────────────────────────────────
//
// Phase functions return `Result<SomeOutcome>` rather than `Result<()>`, so
// the bare `return Ok(())` short-circuit in the macros above can't be used:
// it wouldn't typecheck. The `_or!` family takes an extra `$bail` expression
// and short-circuits with `return Ok($bail)` instead — typically a
// `…::Abandoned` variant of the phase's outcome enum.

/// Like `yield_event!` but on a closed channel returns `Ok($bail)` from the
/// caller rather than `Ok(())`. Use in phase fns whose outcome enum has an
/// `Abandoned` variant.
macro_rules! yield_event_or {
    ($tx:expr, $event:expr, $bail:expr) => {
        match $tx.try_send(Ok($event)) {
            Ok(()) => {}
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                tracing::warn!(
                    "Stream buffer full ({}), dropping event",
                    $crate::agent::react::run::stream_macros::DEFAULT_STREAM_BUFFER
                );
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                return Ok($bail);
            }
        }
    };
}

/// Like `yield_final_event!` but returns `Ok($bail)` on receiver-dropped.
macro_rules! yield_final_event_or {
    ($tx:expr, $event:expr, $bail:expr) => {
        if $tx.send(Ok($event)).await.is_err() {
            return Ok($bail);
        }
    };
}

/// Like `try_send!` but returns `Ok($bail)` after forwarding the error to tx.
macro_rules! try_send_or {
    ($tx:expr, $fallible:expr, $bail:expr) => {
        match $fallible {
            Ok(v) => v,
            Err(e) => {
                let _ = $tx.try_send(Err(e.into()));
                return Ok($bail);
            }
        }
    };
}

pub(crate) use try_send_or;
pub(crate) use yield_event_or;
pub(crate) use yield_final_event_or;
