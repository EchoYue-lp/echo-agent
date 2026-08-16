//! Incremental UTF-8 decoding for byte streams whose read boundaries may
//! split a multi-byte scalar value.

const DEFAULT_STREAM_CHUNK_BYTES: usize = 16 * 1024;

/// Stateful decoder that preserves incomplete UTF-8 suffixes between reads.
///
/// Invalid byte sequences are replaced with U+FFFD, while incomplete suffixes
/// remain buffered until the next `push` or `finish`. Returned chunks never
/// exceed the configured byte limit unless one scalar itself exceeds it
/// (which cannot happen for valid UTF-8 with a positive limit).
#[derive(Debug)]
pub struct IncrementalUtf8Decoder {
    pending: Vec<u8>,
    max_chunk_bytes: usize,
}

impl Default for IncrementalUtf8Decoder {
    fn default() -> Self {
        Self::new(DEFAULT_STREAM_CHUNK_BYTES)
    }
}

impl IncrementalUtf8Decoder {
    pub fn new(max_chunk_bytes: usize) -> Self {
        Self {
            pending: Vec::new(),
            max_chunk_bytes: max_chunk_bytes.max(1),
        }
    }

    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.pending.extend_from_slice(bytes);
        let mut output = String::new();
        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(valid) => {
                    output.push_str(valid);
                    self.pending.clear();
                    break;
                }
                Err(error) => {
                    let valid_len = error.valid_up_to();
                    if let Some(valid_bytes) = self.pending.get(..valid_len)
                        && let Ok(valid) = std::str::from_utf8(valid_bytes)
                    {
                        output.push_str(valid);
                    }
                    match error.error_len() {
                        Some(invalid_len) => {
                            output.push('\u{fffd}');
                            let consumed = valid_len.saturating_add(invalid_len);
                            self.pending.drain(..consumed.min(self.pending.len()));
                        }
                        None => {
                            self.pending.drain(..valid_len.min(self.pending.len()));
                            break;
                        }
                    }
                }
            }
        }
        split_utf8_chunks(output, self.max_chunk_bytes)
    }

    pub fn finish(&mut self) -> Option<String> {
        if self.pending.is_empty() {
            return None;
        }
        let output = String::from_utf8_lossy(&self.pending).to_string();
        self.pending.clear();
        Some(output)
    }
}

/// Split valid UTF-8 text into byte-capped chunks without slicing through a
/// scalar value.
pub fn split_utf8_chunks(text: String, max_chunk_bytes: usize) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let max_chunk_bytes = max_chunk_bytes.max(1);
    if text.len() <= max_chunk_bytes {
        return vec![text];
    }
    let mut chunks = Vec::new();
    let mut current = String::new();
    for character in text.chars() {
        if current.len().saturating_add(character.len_utf8()) > max_chunk_bytes
            && !current.is_empty()
        {
            chunks.push(std::mem::take(&mut current));
        }
        current.push(character);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_a_scalar_split_across_reads() {
        let mut decoder = IncrementalUtf8Decoder::new(16);
        assert!(decoder.push(&[0xe4, 0xb8]).is_empty());
        assert_eq!(decoder.push(&[0xad]), vec!["中".to_string()]);
        assert!(decoder.finish().is_none());
    }

    #[test]
    fn chunks_only_at_character_boundaries() {
        let mut decoder = IncrementalUtf8Decoder::new(5);
        let chunks = decoder.push("中文ab".as_bytes());
        assert_eq!(chunks, vec!["中".to_string(), "文ab".to_string()]);
    }

    #[test]
    fn replaces_invalid_sequences_and_flushes_incomplete_suffix() {
        let mut decoder = IncrementalUtf8Decoder::new(16);
        assert_eq!(decoder.push(&[b'a', 0xff]), vec!["a\u{fffd}".to_string()]);
        assert!(decoder.push(&[0xf0, 0x9f]).is_empty());
        assert_eq!(decoder.finish(), Some("\u{fffd}".to_string()));
    }
}
