//! Generic runtime primitives for driving finite Agent invocations.
//!
//! | Module | Description |
//! |--------|-------------|
//! | [`turn_driver`] | One finite Agent turn: `AgentTurnDriver`, `TurnRequest`, `TurnOutcome`, `EventSink` |

pub mod turn_driver;

pub use turn_driver::{
    AgentTurnDriver, EventSink, SinkControl, TurnInput, TurnInputReceipt, TurnInputState, TurnMode,
    TurnOutcome, TurnReceipt, TurnRequest,
};

/// Re-exported result and transport types used by the turn runtime.
pub use echo_core::agent::{EventEnvelope, EventIdentity, ExecutionUsage};
