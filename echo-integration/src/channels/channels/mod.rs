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
}

pub(crate) type DeliverySender = mpsc::Sender<DeliveryRequest>;

async fn deliver(send_tx: &DeliverySender, message: OutboundMessage) -> Result<()> {
    let (receipt, delivered) = oneshot::channel();
    send_tx
        .send(DeliveryRequest { message, receipt })
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

    /// inner:override handle_stream 产 N 条分段
    struct ChunkInner {
        chunks: Vec<String>,
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
}
