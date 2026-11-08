//! Personal Weixin direct-text protocol, QR authentication and original-owner transport.

pub mod api;
pub mod auth;
pub mod channel;
pub mod codec;
mod state;
pub mod wire;

pub use api::WeixinApi;
pub use auth::{QrChallenge, QrStatus, WeixinCredentials};
pub use channel::{WeixinChannel, WeixinConfig, WeixinDeliveryHandle};
pub use codec::{WeixinTarget, decode_target, encode_target};
pub use state::WeixinContextRef;
pub use wire::{WeixinMessage, WeixinPollStatus, WeixinSendOutcome, WeixinUpdates};
