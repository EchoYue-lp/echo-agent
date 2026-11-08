//! Protocol restoration only: the injected Store does not own messages or task execution.

use super::{auth::WeixinCredentials, codec::encode_target};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use echo_core::{
    error::{ChannelError, ReactError, Result},
    memory::store::{Store, StoreCompareAndPutOutcome},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// Stable reference to one exact provider context. Contains no bearer/context token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WeixinContextRef {
    pub account_id: String,
    pub generation: String,
    pub peer: String,
    pub reference: String,
}

#[derive(Serialize, Deserialize)]
struct MessageContextAssociation {
    peer: String,
    message_id: String,
    context: Option<WeixinContextRef>,
}

#[derive(Clone)]
pub(crate) struct ProtocolState {
    store: Arc<dyn Store>,
    account: String,
    generation: String,
}

impl ProtocolState {
    pub(crate) fn new(store: Arc<dyn Store>, credentials: &WeixinCredentials) -> Self {
        Self {
            store,
            account: credentials.account_id.clone(),
            generation: credentials.generation.clone(),
        }
    }

    fn namespace(&self) -> [String; 3] {
        [
            "weixin-protocol-v1".into(),
            URL_SAFE_NO_PAD.encode(self.account.as_bytes()),
            URL_SAFE_NO_PAD.encode(self.generation.as_bytes()),
        ]
    }

    pub(crate) async fn cursor(&self) -> Result<String> {
        let namespace = self.namespace();
        let parts: Vec<&str> = namespace.iter().map(String::as_str).collect();
        match self.store.get(&parts, "cursor").await? {
            None => Ok(String::new()),
            Some(item) => item
                .value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| state_error("invalid persisted cursor")),
        }
    }

    pub(crate) async fn save_cursor(&self, cursor: &str) -> Result<()> {
        let namespace = self.namespace();
        let parts: Vec<&str> = namespace.iter().map(String::as_str).collect();
        self.store
            .put(&parts, "cursor", serde_json::Value::String(cursor.into()))
            .await
    }

    pub(crate) async fn clear(&self) -> Result<()> {
        let namespace = self.namespace();
        let parts: Vec<&str> = namespace.iter().map(String::as_str).collect();
        for item in self.store.list(&parts).await? {
            if item.namespace.as_slice() != namespace.as_slice() {
                return Err(state_error(
                    "Store returned protocol state outside the requested namespace",
                ));
            }
            self.store.delete(&parts, &item.key).await?;
        }
        Ok(())
    }

    /// Freeze the first observation, including an explicitly absent context.
    /// The association is protocol restoration metadata, never message admission.
    pub(crate) async fn remember_message_context(
        &self,
        peer: &str,
        message_id: &str,
        token: Option<&str>,
    ) -> Result<Option<WeixinContextRef>> {
        if message_id.is_empty() || peer.is_empty() || token.is_some_and(str::is_empty) {
            return Err(state_error("missing provider context identity"));
        }
        let namespace = self.namespace();
        let parts: Vec<&str> = namespace.iter().map(String::as_str).collect();
        let key = message_context_key(peer, message_id)?;
        if let Some(item) = self.store.get(&parts, &key).await? {
            let context = self.decode_association(peer, message_id, item.value)?;
            if let Some(context) = context.as_ref() {
                self.restore_latest_if_missing(context).await?;
            }
            return Ok(context);
        }
        let context = match token {
            Some(token) => {
                let bytes = serde_json::to_vec(&(peer, message_id, token))
                    .map_err(|_| state_error("context encoding failed"))?;
                let context = WeixinContextRef {
                    account_id: self.account.clone(),
                    generation: self.generation.clone(),
                    peer: peer.into(),
                    reference: format!("context:{:x}", Sha256::digest(bytes)),
                };
                // The content-addressed token record is committed before publishing its reference.
                self.store
                    .put(
                        &parts,
                        &context.reference,
                        serde_json::json!({"peer": peer, "token": token}),
                    )
                    .await?;
                Some(context)
            }
            None => None,
        };
        let association = MessageContextAssociation {
            peer: peer.into(),
            message_id: message_id.into(),
            context: context.clone(),
        };
        let value = serde_json::to_value(association)
            .map_err(|_| state_error("context association encoding failed"))?;
        match self
            .store
            .compare_and_put(&parts, &key, None, value)
            .await?
        {
            StoreCompareAndPutOutcome::Applied => {
                if let Some(context) = context.as_ref() {
                    let latest_key = format!("latest:{}", encode_target(&self.account, peer)?);
                    self.store
                        .put(
                            &parts,
                            &latest_key,
                            serde_json::to_value(context)
                                .map_err(|_| state_error("context reference encoding failed"))?,
                        )
                        .await?;
                }
                Ok(context)
            }
            StoreCompareAndPutOutcome::Mismatch => {
                let winner =
                    self.store.get(&parts, &key).await?.ok_or_else(|| {
                        state_error("context association changed during publication")
                    })?;
                self.decode_association(peer, message_id, winner.value)
            }
        }
    }

    /// Resolve only this exact provider message. Missing/explicit None never uses latest.
    pub(crate) async fn message_context(
        &self,
        peer: &str,
        message_id: &str,
    ) -> Result<Option<WeixinContextRef>> {
        let namespace = self.namespace();
        let parts: Vec<&str> = namespace.iter().map(String::as_str).collect();
        let key = message_context_key(peer, message_id)?;
        self.store
            .get(&parts, &key)
            .await?
            .map(|item| self.decode_association(peer, message_id, item.value))
            .transpose()
            .map(Option::flatten)
    }

    /// Prove this exact persisted association, keeping missing and explicit
    /// absence distinct. Validation never repairs projections or reads latest.
    pub(crate) async fn validate_message_context(
        &self,
        peer: &str,
        message_id: &str,
        expected: Option<&WeixinContextRef>,
    ) -> Result<()> {
        let namespace = self.namespace();
        let parts: Vec<&str> = namespace.iter().map(String::as_str).collect();
        let key = message_context_key(peer, message_id)?;
        let association =
            self.store.get(&parts, &key).await?.ok_or_else(|| {
                state_error("original message context association is unavailable")
            })?;
        let actual = self.decode_association(peer, message_id, association.value)?;
        if actual.as_ref() != expected {
            return Err(state_error("original message context association changed"));
        }
        if let Some(context) = actual.as_ref() {
            // Discard the secret locally; callers receive only validation.
            let token = self.context_token(context).await?;
            let encoded = serde_json::to_vec(&(peer, message_id, &token))
                .map_err(|_| state_error("context validation encoding failed"))?;
            if context.reference != format!("context:{:x}", Sha256::digest(encoded)) {
                return Err(state_error("original context token record changed"));
            }
        }
        Ok(())
    }

    fn decode_association(
        &self,
        peer: &str,
        message_id: &str,
        value: serde_json::Value,
    ) -> Result<Option<WeixinContextRef>> {
        // Serde's Option default alone cannot distinguish a damaged missing
        // field from the persisted explicit absence used for recovery.
        if value.get("context").is_none() {
            return Err(state_error("persisted message context field is missing"));
        }
        let association: MessageContextAssociation = serde_json::from_value(value)
            .map_err(|_| state_error("invalid persisted message context association"))?;
        if association.peer != peer
            || association.message_id != message_id
            || association.context.as_ref().is_some_and(|context| {
                context.account_id != self.account
                    || context.generation != self.generation
                    || context.peer != peer
                    || context.reference.is_empty()
            })
        {
            return Err(state_error(
                "message context association has a different identity",
            ));
        }
        Ok(association.context)
    }

    async fn restore_latest_if_missing(&self, context: &WeixinContextRef) -> Result<()> {
        let namespace = self.namespace();
        let parts: Vec<&str> = namespace.iter().map(String::as_str).collect();
        let key = format!("latest:{}", encode_target(&self.account, &context.peer)?);
        if self.store.get(&parts, &key).await?.is_none() {
            // Repair a crash between association and latest projection, without
            // replacing a more recent message's already published context.
            self.store
                .compare_and_put(
                    &parts,
                    &key,
                    None,
                    serde_json::to_value(context)
                        .map_err(|_| state_error("context reference encoding failed"))?,
                )
                .await?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) async fn remember_context(
        &self,
        peer: &str,
        message_id: &str,
        token: &str,
    ) -> Result<WeixinContextRef> {
        self.remember_message_context(peer, message_id, Some(token))
            .await?
            .ok_or_else(|| state_error("message has an explicitly absent context"))
    }

    pub(crate) async fn latest_context(&self, peer: &str) -> Result<Option<WeixinContextRef>> {
        let namespace = self.namespace();
        let parts: Vec<&str> = namespace.iter().map(String::as_str).collect();
        let key = format!("latest:{}", encode_target(&self.account, peer)?);
        self.store
            .get(&parts, &key)
            .await?
            .map(|item| {
                serde_json::from_value(item.value)
                    .map_err(|_| state_error("invalid persisted context reference"))
            })
            .transpose()
    }

    pub(crate) async fn context_token(&self, context: &WeixinContextRef) -> Result<String> {
        if context.account_id != self.account || context.generation != self.generation {
            return Err(state_error("Weixin credential generation changed"));
        }
        let namespace = self.namespace();
        let parts: Vec<&str> = namespace.iter().map(String::as_str).collect();
        let item = self
            .store
            .get(&parts, &context.reference)
            .await?
            .ok_or_else(|| state_error("Weixin context is unavailable"))?;
        if item.value.get("peer").and_then(serde_json::Value::as_str) != Some(context.peer.as_str())
        {
            return Err(state_error(
                "Weixin context reference belongs to another peer",
            ));
        }
        item.value
            .get("token")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| state_error("Weixin context is unavailable"))
    }
}

fn state_error(message: &str) -> ReactError {
    ReactError::Channel(Box::new(ChannelError::Other(message.into())))
}

fn message_context_key(peer: &str, message_id: &str) -> Result<String> {
    if peer.is_empty() || message_id.is_empty() {
        return Err(state_error("missing message context identity"));
    }
    let bytes = serde_json::to_vec(&(peer, message_id))
        .map_err(|_| state_error("message context identity encoding failed"))?;
    Ok(format!("message-context:{:x}", Sha256::digest(bytes)))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use echo_core::{error::MemoryError, memory::store::StoreItem};
    use futures::future::BoxFuture;
    use std::{collections::HashMap, sync::Mutex};

    #[derive(Default)]
    struct TestStore(Mutex<HashMap<(Vec<String>, String), serde_json::Value>>);

    fn storage_error() -> ReactError {
        MemoryError::Unsupported("test Store lock poisoned".into()).into()
    }

    impl Store for TestStore {
        fn compare_and_put<'a>(
            &'a self,
            namespace: &'a [&'a str],
            key: &'a str,
            expected: Option<serde_json::Value>,
            value: serde_json::Value,
        ) -> BoxFuture<'a, Result<StoreCompareAndPutOutcome>> {
            Box::pin(async move {
                let mut values = self.0.lock().map_err(|_| storage_error())?;
                let key = (
                    namespace.iter().map(|part| (*part).to_owned()).collect(),
                    key.to_owned(),
                );
                if values.get(&key).cloned() != expected {
                    return Ok(StoreCompareAndPutOutcome::Mismatch);
                }
                values.insert(key, value);
                Ok(StoreCompareAndPutOutcome::Applied)
            })
        }
        fn put<'a>(
            &'a self,
            namespace: &'a [&'a str],
            key: &'a str,
            value: serde_json::Value,
        ) -> BoxFuture<'a, Result<()>> {
            Box::pin(async move {
                self.0.lock().map_err(|_| storage_error())?.insert(
                    (
                        namespace.iter().map(|part| (*part).to_owned()).collect(),
                        key.to_owned(),
                    ),
                    value,
                );
                Ok(())
            })
        }
        fn get<'a>(
            &'a self,
            namespace: &'a [&'a str],
            key: &'a str,
        ) -> BoxFuture<'a, Result<Option<StoreItem>>> {
            Box::pin(async move {
                let namespace: Vec<String> =
                    namespace.iter().map(|part| (*part).to_owned()).collect();
                Ok(self
                    .0
                    .lock()
                    .map_err(|_| storage_error())?
                    .get(&(namespace.clone(), key.to_owned()))
                    .cloned()
                    .map(|value| StoreItem::new(namespace, key.to_owned(), value)))
            })
        }
        fn delete<'a>(
            &'a self,
            namespace: &'a [&'a str],
            key: &'a str,
        ) -> BoxFuture<'a, Result<bool>> {
            Box::pin(async move {
                Ok(self
                    .0
                    .lock()
                    .map_err(|_| storage_error())?
                    .remove(&(
                        namespace.iter().map(|part| (*part).to_owned()).collect(),
                        key.to_owned(),
                    ))
                    .is_some())
            })
        }
        fn search<'a>(
            &'a self,
            _namespace: &'a [&'a str],
            _query: &'a str,
            _limit: usize,
        ) -> BoxFuture<'a, Result<Vec<StoreItem>>> {
            Box::pin(async { Ok(Vec::new()) })
        }
        fn list_namespaces<'a>(
            &'a self,
            _prefix: Option<&'a [&'a str]>,
        ) -> BoxFuture<'a, Result<Vec<Vec<String>>>> {
            Box::pin(async { Ok(Vec::new()) })
        }
        fn list<'a>(&'a self, namespace: &'a [&'a str]) -> BoxFuture<'a, Result<Vec<StoreItem>>> {
            Box::pin(async move {
                let namespace: Vec<String> =
                    namespace.iter().map(|part| (*part).to_owned()).collect();
                Ok(self
                    .0
                    .lock()
                    .map_err(|_| storage_error())?
                    .iter()
                    .filter(|((candidate, _), _)| candidate == &namespace)
                    .map(|((candidate, key), value)| {
                        StoreItem::new(candidate.clone(), key.clone(), value.clone())
                    })
                    .collect())
            })
        }
    }

    pub(crate) fn store() -> Arc<dyn Store> {
        Arc::new(TestStore::default())
    }

    #[tokio::test]
    async fn frozen_context_survives_new_messages_and_restart_without_cross_generation()
    -> Result<()> {
        let credentials = WeixinCredentials {
            account_id: "account".into(),
            bot_token: "secret".into(),
            base_url: "https://example.invalid".into(),
            user_id: "peer".into(),
            generation: "generation-a".into(),
        };
        let store = store();
        let state = ProtocolState::new(store.clone(), &credentials);
        let frozen = state
            .remember_context("peer", "message-1", "token-1")
            .await?;
        let newer = state
            .remember_context("peer", "message-2", "token-2")
            .await?;
        assert_ne!(frozen.reference, newer.reference);
        let restored = ProtocolState::new(store.clone(), &credentials);
        assert_eq!(restored.context_token(&frozen).await?, "token-1");
        assert_eq!(restored.latest_context("peer").await?, Some(newer));
        let mut forged = frozen.clone();
        forged.peer = "other-peer".into();
        assert!(restored.context_token(&forged).await.is_err());
        let mut renewed = credentials.clone();
        renewed.generation = "generation-b".into();
        let renewed_state = ProtocolState::new(store, &renewed);
        assert!(renewed_state.context_token(&frozen).await.is_err());
        assert!(renewed_state.latest_context("peer").await?.is_none());
        state.save_cursor("cursor-1").await?;
        assert_eq!(restored.cursor().await?, "cursor-1");
        assert_eq!(renewed_state.cursor().await?, "");
        renewed_state.save_cursor("other-generation").await?;
        state.clear().await?;
        assert_eq!(restored.cursor().await?, "");
        assert!(restored.context_token(&frozen).await.is_err());
        assert_eq!(renewed_state.cursor().await?, "other-generation");
        Ok(())
    }

    #[tokio::test]
    async fn exact_message_context_keeps_first_token_and_explicit_none_after_new_messages()
    -> Result<()> {
        let credentials = WeixinCredentials {
            account_id: "account".into(),
            bot_token: "secret".into(),
            base_url: "https://example.invalid".into(),
            user_id: "peer".into(),
            generation: "generation-a".into(),
        };
        let store = store();
        let state = ProtocolState::new(store.clone(), &credentials);
        let first = state
            .remember_message_context("peer", "first", Some("first-token"))
            .await?
            .ok_or_else(|| state_error("fixture needs first context"))?;
        assert!(
            state
                .remember_message_context("peer", "without-token", None)
                .await?
                .is_none()
        );
        let newest = state
            .remember_message_context("peer", "newest", Some("newest-token"))
            .await?;
        assert_eq!(
            state
                .remember_message_context("peer", "first", Some("replayed-new-token"))
                .await?,
            Some(first.clone())
        );
        assert!(
            state
                .remember_message_context(
                    "peer",
                    "without-token",
                    Some("late-token-must-not-rebind")
                )
                .await?
                .is_none()
        );
        assert_eq!(state.latest_context("peer").await?, newest);
        let restored = ProtocolState::new(store, &credentials);
        assert_eq!(
            restored.message_context("peer", "first").await?,
            Some(first.clone())
        );
        assert_eq!(restored.context_token(&first).await?, "first-token");
        assert!(
            restored
                .message_context("peer", "without-token")
                .await?
                .is_none()
        );
        assert!(
            restored
                .message_context("peer", "unobserved")
                .await?
                .is_none()
        );
        assert!(
            restored
                .message_context("other-peer", "first")
                .await?
                .is_none()
        );
        Ok(())
    }

    #[tokio::test]
    async fn concurrent_first_observations_share_one_cas_winner() -> Result<()> {
        let credentials = WeixinCredentials {
            account_id: "account".into(),
            bot_token: "secret".into(),
            base_url: "https://example.invalid".into(),
            user_id: "peer".into(),
            generation: "generation-a".into(),
        };
        let state = ProtocolState::new(store(), &credentials);
        let other = state.clone();
        let (first, second) = tokio::join!(
            state.remember_message_context("peer", "same-message", None),
            other.remember_message_context("peer", "same-message", Some("token"))
        );
        let first = first?;
        assert_eq!(first, second?);
        assert_eq!(state.message_context("peer", "same-message").await?, first);
        Ok(())
    }

    #[tokio::test]
    async fn recovery_context_validation_proves_presence_none_reference_and_original_token()
    -> Result<()> {
        let credentials = WeixinCredentials {
            account_id: "account".into(),
            bot_token: "secret".into(),
            base_url: "https://example.invalid".into(),
            user_id: "peer".into(),
            generation: "generation-a".into(),
        };
        let storage = store();
        let state = ProtocolState::new(storage.clone(), &credentials);
        let namespace = state.namespace();
        let parts: Vec<&str> = namespace.iter().map(String::as_str).collect();
        assert!(
            state
                .validate_message_context("peer", "never-observed", None)
                .await
                .is_err()
        );
        state
            .remember_message_context("peer", "absent", None)
            .await?;
        state
            .validate_message_context("peer", "absent", None)
            .await?;
        let first = state
            .remember_message_context("peer", "original", Some("first-token"))
            .await?
            .ok_or_else(|| state_error("fixture needs first reference"))?;
        let latest = state
            .remember_message_context("peer", "newer", Some("latest-token"))
            .await?;
        state
            .validate_message_context("peer", "original", Some(&first))
            .await?;
        assert!(
            state
                .validate_message_context("peer", "original", None)
                .await
                .is_err()
        );
        assert!(
            state
                .validate_message_context("peer", "absent", Some(&first))
                .await
                .is_err()
        );
        assert!(
            state
                .validate_message_context("peer", "newer", Some(&first))
                .await
                .is_err()
        );
        let restored = ProtocolState::new(storage.clone(), &credentials);
        restored
            .validate_message_context("peer", "absent", None)
            .await?;
        restored
            .validate_message_context("peer", "original", Some(&first))
            .await?;
        assert_eq!(restored.latest_context("peer").await?, latest);
        assert!(storage.delete(&parts, &first.reference).await?);
        assert!(
            restored
                .validate_message_context("peer", "original", Some(&first))
                .await
                .is_err()
        );
        // A different nonempty token under the old key is not a valid reference.
        storage
            .put(
                &parts,
                &first.reference,
                serde_json::json!({"peer": "peer", "token": "changed-token"}),
            )
            .await?;
        assert!(
            restored
                .validate_message_context("peer", "original", Some(&first))
                .await
                .is_err()
        );
        let absent = message_context_key("peer", "absent")?;
        storage
            .put(
                &parts,
                &absent,
                serde_json::json!({"peer": "peer", "message_id": "absent"}),
            )
            .await?;
        assert!(
            restored
                .validate_message_context("peer", "absent", None)
                .await
                .is_err()
        );
        storage
            .put(
                &parts,
                &absent,
                serde_json::json!({"peer": "other", "message_id": "absent", "context": null}),
            )
            .await?;
        assert!(
            restored
                .validate_message_context("peer", "absent", None)
                .await
                .is_err()
        );
        assert_eq!(restored.latest_context("peer").await?, latest);
        let mut renewed = credentials;
        renewed.generation = "generation-b".into();
        assert!(
            ProtocolState::new(storage, &renewed)
                .validate_message_context("peer", "original", Some(&first))
                .await
                .is_err()
        );
        Ok(())
    }
}
