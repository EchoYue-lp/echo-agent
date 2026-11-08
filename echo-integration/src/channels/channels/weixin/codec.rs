//! Versioned account/peer identities; provider identifiers never become path components.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use echo_core::error::{ChannelError, ReactError, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WeixinTarget {
    pub account_id: String,
    pub peer: String,
}

pub fn encode_target(account_id: &str, peer: &str) -> Result<String> {
    validate(account_id, peer)?;
    let bytes = serde_json::to_vec(&(account_id, peer)).map_err(|_| invalid())?;
    Ok(format!("weixin:v1:{}", URL_SAFE_NO_PAD.encode(bytes)))
}

pub fn decode_target(value: &str) -> Result<WeixinTarget> {
    let encoded = value.strip_prefix("weixin:v1:").ok_or_else(invalid)?;
    let bytes = URL_SAFE_NO_PAD.decode(encoded).map_err(|_| invalid())?;
    let (account_id, peer): (String, String) =
        serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    validate(&account_id, &peer)?;
    // Reject alternate encodings so session identity has exactly one spelling.
    if encode_target(&account_id, &peer)? != value {
        return Err(invalid());
    }
    Ok(WeixinTarget { account_id, peer })
}

fn validate(account: &str, peer: &str) -> Result<()> {
    if account.trim().is_empty() || peer.trim().is_empty() {
        return Err(invalid());
    }
    Ok(())
}

fn invalid() -> ReactError {
    ReactError::Channel(Box::new(ChannelError::InvalidConfig(
        "invalid canonical Weixin account/peer target".into(),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespaces_round_trip_without_separator_or_unicode_collisions() -> Result<()> {
        let first = encode_target("账户:/甲😀", "乙:丙/丁")?;
        let second = encode_target("账户:/甲😀:乙", "丙/丁")?;
        assert_ne!(first, second);
        assert_eq!(decode_target(&first)?.peer, "乙:丙/丁");
        assert!(decode_target("weixin:v0:invalid").is_err());
        assert!(decode_target("weixin:v1:%%%%").is_err());
        assert!(encode_target("", "person").is_err());
        Ok(())
    }
}
