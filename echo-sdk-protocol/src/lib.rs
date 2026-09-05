//! Source-only `_echo_agent/*` extension contract for the echo-agent SDK.
//!
//! This crate freezes the SDK contract layer over the official stable
//! [ACP](https://agentclientprotocol.com) v1 baseline:
//!
//! - the official `agent-client-protocol` / `agent-client-protocol-schema`
//!   crates are the *only* wire authority for JSON-RPC framing,
//!   `initialize`, Session, Prompt, ContentBlock, updates and stop reasons;
//!   this crate never re-defines those types;
//! - everything here describes the echo-agent extension profile only: the
//!   namespaced capability published under `initialize` `_meta`, the
//!   `_echo_agent/*` method catalog, extension errors, lossless scalars,
//!   handles, the full `EventEnvelope` event view and replay/gap contracts,
//!   plus the deterministic generation of the extension JSON Schema and the
//!   facade parity manifest consumed by future Host and language SDKs.
//!
//! The crate is `publish = false` and ships source only: consumers build the
//! contract tools from the same Git revision (see `docs/sdk/README.md`).
//!
//! Current status of the SDK program is **Contract** (see design §20.6):
//! contracts and gates exist, but no ACP agent adapter, Host, or language SDK
//! is implemented by this crate.

pub mod capability;
pub mod catalog;
pub mod error;
pub mod event;
pub mod handle;
pub mod inventory;
pub mod methods;
pub mod scalar;
pub mod schema;

/// ACP extension method namespace. Every custom request/notification the SDK
/// profile defines lives under this prefix (ACP extensibility requires the
/// leading underscore and a capability declaration).
pub const EXTENSION_NAMESPACE: &str = "_echo_agent";
/// Extension protocol version, governed independently from the ACP wire
/// protocol version and the official crate/schema artifact versions
/// (design §18).
pub const EXTENSION_PROTOCOL_VERSION: u32 = 1;
