//! Channel-yield macros shared by `run_core_loop` and its phase functions.
//!
//! These macros wrap `mpsc::Sender` ergonomics so phase functions don't need
//! to spell out the early-return-on-closed-channel pattern at every call site.
//!
//! All three macros short-circuit the **enclosing function** with `return Ok(())`
//! when the receiver is gone. Every phase function therefore returns
//! `Result<…>` so this short-circuit means "abandon the stream gracefully".

// ── Phase-fn variants ────────────────────────────────────────────────
//
// Phase functions return `Result<SomeOutcome>` rather than `Result<()>`, so
// the bare `return Ok(())` short-circuit in the macros above can't be used:
// it wouldn't typecheck. The `_or!` family takes an extra `$bail` expression
// and short-circuits with `return Ok($bail)` instead — typically a
// `…::Abandoned` variant of the phase's outcome enum.

/// Send an ordered event with backpressure. A closed receiver abandons the
/// invocation through the phase-specific outcome.
macro_rules! yield_event_or {
    ($tx:expr, $event:expr, $bail:expr) => {
        if $tx.send(Ok($event)).await.is_err() {
            return Ok($bail);
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

/// Forward an error reliably and return `Ok($bail)` from the phase.
macro_rules! try_send_or {
    ($tx:expr, $fallible:expr, $bail:expr) => {
        match $fallible {
            Ok(v) => v,
            Err(e) => {
                let error: crate::error::ReactError = e.into();
                let _ = $tx
                    .send(Ok(crate::agent::AgentEvent::from_error(
                        "react_loop",
                        &error,
                    )))
                    .await;
                return Ok($bail);
            }
        }
    };
}

pub(crate) use try_send_or;
pub(crate) use yield_event_or;
pub(crate) use yield_final_event_or;
