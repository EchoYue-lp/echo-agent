pub mod feishu;
pub mod qq;

use super::types::*;
use echo_core::error::{ChannelError, ReactError, Result};
use futures::StreamExt;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

pub(crate) struct DeliveryRequest {
    pub message: OutboundMessage,
    pub receipt: oneshot::Sender<Result<()>>,
    pub delivery_permit: Option<ChannelDeliveryPermit>,
}

pub(crate) type DeliverySender = mpsc::Sender<DeliveryRequest>;

async fn deliver(send_tx: &DeliverySender, message: OutboundMessage) -> Result<()> {
    let delivery_permit = message.begin_delivery()?;
    let (receipt, delivered) = oneshot::channel();
    send_tx
        .send(DeliveryRequest {
            message,
            receipt,
            delivery_permit,
        })
        .await
        .map_err(|error| {
            ReactError::Channel(Box::new(ChannelError::SendError(format!(
                "Failed to enqueue reply: {error}"
            ))))
        })?;
    delivered.await.map_err(|_| {
        ReactError::Channel(Box::new(ChannelError::SendError(
            "Message delivery task stopped before acknowledging the reply".to_string(),
        )))
    })?
}

/// 消费 inner handler 的流式分段(`handle_stream`),逐 chunk 经 `send_tx` 投递到 IM。
///
/// 返回**空 text 占位** `OutboundMessage`:gateway 在 `handle` 返回后会再调 `reply`,
/// `reply_with_empty_guard` 对空 text no-op,防止最后一段 chunk 重复发送(spec D2-5)。
///
/// QQ 和飞书 wrapper 共用此函数。
pub(crate) async fn dispatch_stream_to_send_tx(
    inner: &Arc<dyn MessageHandler>,
    send_tx: &DeliverySender,
    msg: InboundMessage,
) -> Result<OutboundMessage> {
    let placeholder = OutboundMessage::new(&msg.channel_id, msg.reply_target(), msg.chat_type, "");
    let mut stream = match inner.handle_stream(msg).await {
        Ok(s) => s,
        Err(e) => return Err(e),
    };
    while let Some(item) = stream.next().await {
        let chunk = item?;
        deliver(send_tx, chunk).await?;
    }
    Ok(placeholder)
}

/// `reply` 的双发防护:空 text 视为流式占位 no-op(防 gateway 再 reply 导致最后一段双发);
/// 非空 text(向后兼容:直接 reply 一段完整文本)正常 `send_tx`。
pub(crate) async fn reply_with_empty_guard(
    send_tx: &DeliverySender,
    msg: OutboundMessage,
) -> Result<()> {
    if msg.text.is_empty() {
        return Ok(());
    }
    deliver(send_tx, msg).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use futures::stream::{BoxStream, StreamExt};
    use std::sync::Arc;
    use tokio::sync::Notify;
    use tokio::time::{Duration, timeout};

    use crate::channels::session::{ChannelSessionInstance, SessionConfig, SessionHandler};

    /// inner:override handle_stream 产 N 条分段
    struct ChunkInner {
        chunks: Vec<String>,
    }

    struct ParkAfterFirstChunk {
        release: Arc<Notify>,
    }
    #[async_trait]
    impl MessageHandler for ChunkInner {
        async fn handle(&self, msg: InboundMessage) -> Result<OutboundMessage> {
            Ok(OutboundMessage::new(
                &msg.channel_id,
                msg.reply_target(),
                msg.chat_type,
                "full",
            ))
        }
        async fn reply(&self, _msg: OutboundMessage) -> Result<()> {
            Ok(())
        }
        async fn handle_stream<'a>(
            &'a self,
            msg: InboundMessage,
        ) -> Result<BoxStream<'a, Result<OutboundMessage>>> {
            let (ch, to, ct) = (msg.channel_id, msg.chat_id, msg.chat_type);
            let items: Vec<Result<OutboundMessage>> = self
                .chunks
                .iter()
                .map(|c| Ok(OutboundMessage::new(&ch, &to, ct, c)))
                .collect();
            Ok(futures::stream::iter(items).boxed())
        }
    }

    #[async_trait]
    impl MessageHandler for ParkAfterFirstChunk {
        async fn handle(&self, msg: InboundMessage) -> Result<OutboundMessage> {
            Ok(OutboundMessage::new(
                &msg.channel_id,
                msg.reply_target(),
                msg.chat_type,
                "old-full",
            ))
        }

        async fn reply(&self, _msg: OutboundMessage) -> Result<()> {
            Ok(())
        }

        async fn handle_stream<'a>(
            &'a self,
            msg: InboundMessage,
        ) -> Result<BoxStream<'a, Result<OutboundMessage>>> {
            let release = Arc::clone(&self.release);
            let (channel_id, to, chat_type) = (msg.channel_id, msg.chat_id, msg.chat_type);
            Ok(async_stream::stream! {
                yield Ok(OutboundMessage::new(
                    &channel_id,
                    &to,
                    chat_type,
                    "old-first",
                ));
                release.notified().await;
                yield Ok(OutboundMessage::new(
                    &channel_id,
                    &to,
                    chat_type,
                    "old-second",
                ));
            }
            .boxed())
        }
    }

    #[tokio::test]
    async fn dispatch_sends_each_chunk_returns_empty_placeholder() {
        let (tx, mut rx) = mpsc::channel::<DeliveryRequest>(16);
        let inner: Arc<dyn MessageHandler> = Arc::new(ChunkInner {
            chunks: vec!["a".into(), "b".into()],
        });
        let msg = InboundMessage::new("qq", "u1", "c1", ChatType::Direct, "hi", "m1");
        let dispatch =
            tokio::spawn(async move { dispatch_stream_to_send_tx(&inner, &tx, msg).await });
        let mut got = Vec::new();
        for _ in 0..2 {
            let request = rx.recv().await.expect("delivery request");
            got.push(request.message.text);
            let _ = request.receipt.send(Ok(()));
        }
        let ret = dispatch.await.expect("dispatch task").expect("dispatch ok");
        assert_eq!(ret.text, "", "returns empty placeholder");
        assert_eq!(got, vec!["a".to_string(), "b".to_string()]);
    }

    #[tokio::test]
    async fn reply_guard_empty_is_noop_nonempty_sends() {
        let (tx, mut rx) = mpsc::channel::<DeliveryRequest>(16);
        // 空 text:no-op,不 send
        reply_with_empty_guard(&tx, OutboundMessage::new("qq", "u1", ChatType::Direct, ""))
            .await
            .unwrap();
        assert!(rx.try_recv().is_err(), "empty text must not be sent");
        // 非空 text:正常 send
        let reply = tokio::spawn(async move {
            reply_with_empty_guard(&tx, OutboundMessage::new("qq", "u1", ChatType::Direct, "x"))
                .await
        });
        let request = rx.recv().await.expect("delivery request");
        assert_eq!(request.message.text, "x");
        let _ = request.receipt.send(Ok(()));
        assert!(reply.await.expect("reply task").is_ok());
    }

    #[tokio::test]
    async fn dispatch_with_no_chunks_returns_empty() {
        let (tx, mut rx) = mpsc::channel::<DeliveryRequest>(16);
        let inner: Arc<dyn MessageHandler> = Arc::new(ChunkInner { chunks: vec![] });
        let msg = InboundMessage::new("qq", "u1", "c1", ChatType::Direct, "hi", "m1");
        let ret = dispatch_stream_to_send_tx(&inner, &tx, msg).await.unwrap();
        assert_eq!(ret.text, "");
        assert!(rx.try_recv().is_err(), "no chunks → nothing sent");
    }

    #[tokio::test]
    async fn reset_waits_for_admitted_delivery_and_fences_later_old_chunks() -> Result<()> {
        let release = Arc::new(Notify::new());
        let session_handler: Arc<dyn MessageHandler> = Arc::new(SessionHandler::new(
            SessionConfig::default()
                .with_reset_keywords(vec!["reset-now".to_string()])
                .with_command_prefix(None),
            {
                let release = Arc::clone(&release);
                move |_instance: &ChannelSessionInstance| {
                    Box::new(ParkAfterFirstChunk {
                        release: Arc::clone(&release),
                    }) as Box<dyn MessageHandler>
                }
            },
        ));
        let (send_tx, mut send_rx) = mpsc::channel::<DeliveryRequest>(4);

        let old_handler = Arc::clone(&session_handler);
        let old_send_tx = send_tx.clone();
        let old_delivery = tokio::spawn(async move {
            dispatch_stream_to_send_tx(
                &old_handler,
                &old_send_tx,
                InboundMessage::new(
                    "qq",
                    "sender",
                    "conversation",
                    ChatType::Direct,
                    "old-turn",
                    "old-message",
                ),
            )
            .await
        });
        let admitted = timeout(Duration::from_secs(2), send_rx.recv())
            .await
            .map_err(|_| ReactError::Other("old delivery was not admitted".to_string()))?
            .ok_or_else(|| ReactError::Other("delivery queue closed unexpectedly".to_string()))?;
        let DeliveryRequest {
            message,
            receipt,
            delivery_permit,
        } = admitted;
        if message.text != "old-first" {
            return Err(ReactError::Other(
                "unexpected first old-generation output".to_string(),
            ));
        }

        let reset_started = Arc::new(Notify::new());
        let reset_handler = Arc::clone(&session_handler);
        let reset_started_task = Arc::clone(&reset_started);
        let reset = tokio::spawn(async move {
            reset_started_task.notify_one();
            let mut stream = reset_handler
                .handle_stream(InboundMessage::new(
                    "qq",
                    "sender",
                    "conversation",
                    ChatType::Direct,
                    "reset-now",
                    "reset-message",
                ))
                .await?;
            stream.next().await.ok_or_else(|| {
                ReactError::Other("reset stream closed without acknowledgement".to_string())
            })?
        });
        reset_started.notified().await;
        tokio::task::yield_now().await;
        if reset.is_finished() {
            return Err(ReactError::Other(
                "reset acknowledged before admitted old delivery settled".to_string(),
            ));
        }

        drop(delivery_permit);
        receipt.send(Ok(())).map_err(|_| {
            ReactError::Other("old delivery stopped waiting for its receipt".to_string())
        })?;
        let reset_reply = timeout(Duration::from_secs(2), reset)
            .await
            .map_err(|_| ReactError::Other("reset did not settle".to_string()))?
            .map_err(|error| ReactError::Other(format!("reset task failed: {error}")))??;
        if reset_reply.text != SessionConfig::default().reset_reply {
            return Err(ReactError::Other(
                "reset returned an unexpected acknowledgement".to_string(),
            ));
        }

        release.notify_waiters();
        let old_result = timeout(Duration::from_secs(2), old_delivery)
            .await
            .map_err(|_| ReactError::Other("retired stream did not stop".to_string()))?
            .map_err(|error| ReactError::Other(format!("old delivery task failed: {error}")))??;
        if !old_result.text.is_empty() {
            return Err(ReactError::Other(
                "retired stream did not return its delivery placeholder".to_string(),
            ));
        }
        if send_rx.try_recv().is_ok() {
            return Err(ReactError::Other(
                "retired generation enqueued output after reset".to_string(),
            ));
        }
        Ok(())
    }
}
