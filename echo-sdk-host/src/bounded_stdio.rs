//! Bounded stdio transport (supreme plan 05, todo
//! `deliver-events-replay-recovery` step 5).
//!
//! The official `Stdio` transport has no frame size limit, so this module
//! wraps the raw input in a newline-frame byte counter and feeds the official
//! [`ByteStreams`]. Design rules:
//!
//! - only newline-delimited frames are counted — no JSON-RPC parsing,
//!   framing or writing is replicated here;
//! - the stdout side stays the official ACP writer (single writer, protocol
//!   only), so stdout framing and secret discipline are untouched;
//! - an oversized frame poisons the reader *before any business handler
//!   runs*: the connection fails with a bounded diagnostic and the standard
//!   close chain performs the bounded shutdown.

use agent_client_protocol::ByteStreams;
use futures::io::AsyncRead;
use std::io::ErrorKind;
use std::pin::Pin;
use std::task::{Context, Poll};

/// Exact per-frame byte accounting across arbitrary read boundaries: every
/// byte belongs to exactly one frame, frames end at `\n`, and a frame whose
/// total (including `\n`) exceeds the bound poisons the reader.
#[derive(Default)]
struct FrameAccountant {
    current_frame_bytes: u64,
}

impl FrameAccountant {
    fn account(&mut self, max_frame_bytes: u64, read: &[u8]) -> Result<(), std::io::Error> {
        for byte in read {
            if *byte == b'\n' {
                // This frame ends (newline included in its size).
                self.current_frame_bytes =
                    self.current_frame_bytes.checked_add(1).ok_or_else(|| {
                        std::io::Error::new(
                            ErrorKind::InvalidData,
                            "stdin frame byte counter overflow",
                        )
                    })?;
                if self.current_frame_bytes > max_frame_bytes {
                    return Err(std::io::Error::new(
                        ErrorKind::InvalidData,
                        format!("stdin frame exceeds the configured {max_frame_bytes} byte limit"),
                    ));
                }
                self.current_frame_bytes = 0;
            } else {
                self.current_frame_bytes =
                    self.current_frame_bytes.checked_add(1).ok_or_else(|| {
                        std::io::Error::new(
                            ErrorKind::InvalidData,
                            "stdin frame byte counter overflow",
                        )
                    })?;
                if self.current_frame_bytes > max_frame_bytes {
                    return Err(std::io::Error::new(
                        ErrorKind::InvalidData,
                        format!("stdin frame exceeds the configured {max_frame_bytes} byte limit"),
                    ));
                }
            }
        }
        Ok(())
    }
}

/// The production reader used by the transport: a thin AsyncRead wrapper
/// around [`FrameAccountant`] so a frame split across reads is still
/// measured exactly once.
pub(crate) struct CountingFrameReader<R> {
    inner: R,
    max_frame_bytes: u64,
    accountant: FrameAccountant,
}

impl<R> CountingFrameReader<R> {
    pub(crate) fn new(inner: R, max_frame_bytes: u64) -> Self {
        Self {
            inner,
            max_frame_bytes,
            accountant: FrameAccountant::default(),
        }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for CountingFrameReader<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = &mut *self;
        match Pin::new(&mut this.inner).poll_read(cx, buf) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Ready(Ok(0)) => Poll::Ready(Ok(0)),
            Poll::Ready(Ok(read)) => {
                // Account only what was delivered; a mid-frame rejection
                // fails before the official parser sees the frame.
                let Some(frame) = buf.get(..read) else {
                    return Poll::Ready(Err(std::io::Error::new(
                        ErrorKind::InvalidData,
                        "reader returned an invalid byte count",
                    )));
                };
                if let Err(error) = this.accountant.account(this.max_frame_bytes, frame) {
                    return Poll::Ready(Err(error));
                }
                Poll::Ready(Ok(read))
            }
        }
    }
}

/// Build the official [`ByteStreams`] pair with bounded stdin. The stdout
/// half is the plain official writer path.
pub(crate) fn bounded_stdio(
    max_frame_bytes: u64,
) -> ByteStreams<
    tokio_util::compat::Compat<tokio::io::Stdout>,
    CountingFrameReader<tokio_util::compat::Compat<tokio::io::Stdin>>,
> {
    // The counting reader implements `futures::io::AsyncRead` directly, so
    // the official ByteStreams consume it without a second adapter; the
    // inner tokio stdin is bridged once via the official compat util.
    let reader = CountingFrameReader::new(
        tokio_util::compat::TokioAsyncReadCompatExt::compat(tokio::io::stdin()),
        max_frame_bytes,
    );
    ByteStreams::new(
        tokio_util::compat::TokioAsyncWriteCompatExt::compat_write(tokio::io::stdout()),
        reader,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::io::AsyncReadExt as _;

    fn bounded(bytes: &[u8], max: u64) -> CountingFrameReader<futures::io::Cursor<Vec<u8>>> {
        CountingFrameReader::new(futures::io::Cursor::new(bytes.to_vec()), max)
    }

    #[tokio::test]
    async fn frames_within_the_bound_pass_through() {
        let mut reader = bounded(b"{\"jsonrpc\":\"2.0\"}\n{\"a\":1}\n", 64);
        let mut out = Vec::new();
        reader.read_to_end(&mut out).await.unwrap_or_default();
        assert_eq!(out, b"{\"jsonrpc\":\"2.0\"}\n{\"a\":1}\n");
    }

    #[tokio::test]
    async fn oversized_frames_fail_before_delivery_completes() {
        let oversized = [b'x'; 200];
        let mut framed = oversized.to_vec();
        framed.push(b'\n');
        let mut reader = bounded(&framed, 64);
        let mut out = Vec::new();
        let result = reader.read_to_end(&mut out).await;
        assert!(result.is_err(), "oversized frame must fail the reader");
    }

    #[tokio::test]
    async fn frames_split_across_reads_are_measured_exactly() {
        // 40 bytes + newline, limit 32: no single read may let it through.
        let payload = vec![b'y'; 40];
        let mut framed = payload;
        framed.push(b'\n');
        let mut reader = bounded(&framed, 32);
        let mut buffer = [0_u8; 16];
        let mut saw_error = false;
        loop {
            match reader.read(&mut buffer).await {
                Ok(0) => break,
                Ok(_) => continue,
                Err(_) => {
                    saw_error = true;
                    break;
                }
            }
        }
        assert!(saw_error, "a 41-byte frame must exceed the 32-byte limit");
    }

    #[tokio::test]
    async fn utf8_multibyte_frames_are_byte_counted_not_char_counted() {
        // "你好" is 6 bytes; a limit of 5 bytes must reject it even though
        // it is only 2 characters.
        let mut frame = "你好".as_bytes().to_vec();
        frame.push(b'\n');
        let mut reader = bounded(&frame, 5);
        let mut out = Vec::new();
        let result = reader.read_to_end(&mut out).await;
        assert!(result.is_err(), "byte counting must not approximate chars");
    }
}
