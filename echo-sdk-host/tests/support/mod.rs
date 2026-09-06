use std::io;
use tokio::io::{AsyncRead, AsyncReadExt};

const MAX_REQUEST_BYTES: usize = 8 * 1024 * 1024;
const HEADER_TERMINATOR: &[u8] = b"\r\n\r\n";

/// Drain a fixture HTTP request before writing the response.
///
/// Full-feature builds can produce a JSON request larger than one socket
/// read. Responding after a partial read makes the client race its own upload
/// and turns a valid model response into a spurious body-decoding failure.
pub async fn read_http_request<S>(socket: &mut S) -> io::Result<()>
where
    S: AsyncRead + Unpin,
{
    let mut request = Vec::with_capacity(16 * 1024);
    let mut chunk = [0_u8; 16 * 1024];
    let header_end = loop {
        let read = socket.read(&mut chunk).await?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "fixture model received EOF before request headers",
            ));
        }
        let bytes = chunk.get(..read).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "fixture read exceeded buffer")
        })?;
        let next_len = request.len().checked_add(bytes.len()).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "fixture request length overflow",
            )
        })?;
        if next_len > MAX_REQUEST_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "fixture request exceeds maximum size",
            ));
        }
        request.extend_from_slice(bytes);
        if let Some(position) = request
            .windows(HEADER_TERMINATOR.len())
            .position(|window| window == HEADER_TERMINATOR)
        {
            break position
                .checked_add(HEADER_TERMINATOR.len())
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "fixture header length overflow")
                })?;
        }
    };

    let headers = request.get(..header_end).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "fixture header boundary is invalid",
        )
    })?;
    let header_text = std::str::from_utf8(headers).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("fixture request headers are not UTF-8: {error}"),
        )
    })?;
    let content_length = header_text.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("content-length")
            .then_some(value.trim())
    });
    let body_length = content_length
        .map(|value| {
            value.parse::<usize>().map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("fixture content-length is invalid: {error}"),
                )
            })
        })
        .transpose()?
        .unwrap_or(0);
    if body_length > MAX_REQUEST_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "fixture request body exceeds maximum size",
        ));
    }

    let mut body_read = request.len().checked_sub(header_end).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "fixture body boundary is invalid",
        )
    })?;
    while body_read < body_length {
        let remaining = body_length - body_read;
        let read_limit = remaining.min(chunk.len());
        let read = socket
            .read(chunk.get_mut(..read_limit).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "fixture body limit is invalid")
            })?)
            .await?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "fixture model received EOF before request body",
            ));
        }
        body_read = body_read.checked_add(read).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "fixture body length overflow")
        })?;
    }
    Ok(())
}
