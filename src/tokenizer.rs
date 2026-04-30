//! Tokenizer façade.
//!
//! Thin re-exports of `echo_core::tokenizer`.
//! The authoritative implementation lives in `echo_core`.

/// Direct re-exports from `echo_core::tokenizer`.
pub mod core {
    pub use echo_core::tokenizer::*;
}

pub use echo_core::tokenizer::*;
