//! Stable ACP v1 Agent adapter for the `echo_agent` framework.
//!
//! The adapter composes the official ACP Rust runtime with the framework's
//! existing [`crate::runtime::AgentTurnDriver`]. It is transport-neutral: a
//! caller can connect it to stdio, an in-process channel, or another official
//! ACP transport without creating another JSON-RPC parser or Agent loop.
//!
//! Each ACP Session owns an independent framework Agent. Session history stays
//! inside that Agent; this module only owns protocol addressing and the active
//! turn cancellation token.
//!
//! Standard ACP and negotiated extension profiles share one connection-level
//! runtime ([`AcpConnectionServices`]): a single Session map, a single Run
//! authority, a ledger-first event path and one close chain. Extension
//! profiles plug in through [`AcpConnectionProfile`] without forking the
//! official dispatch loop.

mod adapter;
mod extension;
mod projection;
mod prompt;
mod runtime;
mod session;

pub use adapter::{AcpAdapterConfig, AcpAgentAdapter, AcpAgentAdapterWithProfile};
pub use extension::{
    ExtensionInvocationAuthority, ExtensionInvocationLease, ExtensionLeaseError,
    ExtensionSettlement,
};
pub use projection::AcpEventProjector;
pub use runtime::{
    AcpConnectionProfile, AcpConnectionServices, AcpLedgerLimits, ConnectionMode, EventLedger,
    RunEntry, RunEventObserver, RunObserverContext, RunStartSpec, StandardBridgeOutcome,
};
pub use session::{
    AcpSession, AcpSessionContext, AcpSessionFactory, ActiveTurn, ActiveTurnLease, SessionRegistry,
};
