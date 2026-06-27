pub mod feishu;
pub mod qq;

use super::types::*;
use echo_core::error::{ChannelError, ReactError, Result};
use futures::StreamExt;
use std::sync::Arc;
use tokio::sync::mpsc;

/// 消费 inner handler 的流式分段(`handle_stream`),逐 chunk 经 `send_tx` 投递到 IM。
///
/// 返回**空 text 占位** `OutboundMessage`:gateway 在 `handle` 返回后会再调 `reply`,
/// `reply_with_empty_guard` 对空 text no-op,防止最后一段 chunk 重复发送(spec D2-5)。
///
/// QQ 和飞书 wrapper 共用此函数。
pub(crate) async fn dispatch_stream_to_send_tx(
    inner: &Arc<dyn MessageHandler>,
    send_tx: &mpsc::Sender<OutboundMessage>,
    msg: InboundMessage,
) -> Result<OutboundMessage> {
    let placeholder = OutboundMessage::new(&msg.channel_id, &msg.sender_id, msg.chat_type, "");
    let mut stream = match inner.handle_stream(msg).await {
        Ok(s) => s,
        Err(e) => return Err(e),
    };
    while let Some(item) = stream.next().await {
        let chunk = item?;
        // 忽略 send 错误(后台 send_task 关闭等),不阻断后续 chunk
        let _ = send_tx.send(chunk).await;
    }
    Ok(placeholder)
}

/// `reply` 的双发防护:空 text 视为流式占位 no-op(防 gateway 再 reply 导致最后一段双发);
/// 非空 text(向后兼容:直接 reply 一段完整文本)正常 `send_tx`。
pub(crate) async fn reply_with_empty_guard(
    send_tx: &mpsc::Sender<OutboundMessage>,
    msg: OutboundMessage,
) -> Result<()> {
    if msg.text.is_empty() {
        return Ok(());
    }
    send_tx.send(msg).await.map_err(|e| {
        ReactError::Channel(Box::new(ChannelError::SendError(format!(
            "Failed to send reply: {e}"
        ))))
    })
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
                &msg.sender_id,
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
            let (ch, to, ct) = (msg.channel_id, msg.sender_id, msg.chat_type);
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
        let (tx, mut rx) = mpsc::channel::<OutboundMessage>(16);
        let inner: Arc<dyn MessageHandler> = Arc::new(ChunkInner {
            chunks: vec!["a".into(), "b".into()],
        });
        let msg = InboundMessage::new("qq", "u1", "c1", ChatType::Direct, "hi", "m1");
        let ret = dispatch_stream_to_send_tx(&inner, &tx, msg)
            .await
            .expect("dispatch ok");
        assert_eq!(ret.text, "", "returns empty placeholder");

        let mut got = Vec::new();
        while let Ok(m) = rx.try_recv() {
            got.push(m.text);
        }
        assert_eq!(got, vec!["a".to_string(), "b".to_string()]);
    }

    #[tokio::test]
    async fn reply_guard_empty_is_noop_nonempty_sends() {
        let (tx, mut rx) = mpsc::channel::<OutboundMessage>(16);
        // 空 text:no-op,不 send
        reply_with_empty_guard(&tx, OutboundMessage::new("qq", "u1", ChatType::Direct, ""))
            .await
            .unwrap();
        assert!(rx.try_recv().is_err(), "empty text must not be sent");
        // 非空 text:正常 send
        reply_with_empty_guard(&tx, OutboundMessage::new("qq", "u1", ChatType::Direct, "x"))
            .await
            .unwrap();
        assert_eq!(rx.try_recv().unwrap().text, "x");
    }

    #[tokio::test]
    async fn dispatch_with_no_chunks_returns_empty() {
        let (tx, mut rx) = mpsc::channel::<OutboundMessage>(16);
        let inner: Arc<dyn MessageHandler> = Arc::new(ChunkInner { chunks: vec![] });
        let msg = InboundMessage::new("qq", "u1", "c1", ChatType::Direct, "hi", "m1");
        let ret = dispatch_stream_to_send_tx(&inner, &tx, msg).await.unwrap();
        assert_eq!(ret.text, "");
        assert!(rx.try_recv().is_err(), "no chunks → nothing sent");
    }
}
