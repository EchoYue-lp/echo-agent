//! Common utility functions for search providers

/// URL encoding (percent-encoding, compliant with RFC 3986 unreserved character set)
pub fn urlencode(input: &str) -> String {
    input
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                char::from(b).to_string()
            }
            _ => format!("%{:02X}", b),
        })
        .collect()
}

/// Safely truncate by character count (won't split in the middle of multi-byte UTF-8 characters)
pub fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

/// Percent-decoding
pub fn percent_decode(input: &str) -> String {
    let mut result = Vec::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let high = decode_hex(bytes[i + 1]);
            let low = decode_hex(bytes[i + 2]);
            if let (Some(high), Some(low)) = (high, low) {
                let byte = high.saturating_mul(16).saturating_add(low);
                result.push(byte);
                i += 3;
                continue;
            }
        } else if bytes[i] == b'+' {
            result.push(b' ');
            i += 1;
            continue;
        }
        result.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&result).to_string()
}

fn decode_hex(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_urlencode() {
        assert_eq!(urlencode("hello world"), "hello%20world");
        assert_eq!(urlencode("rust-lang"), "rust-lang");
        assert_eq!(urlencode("café"), "caf%C3%A9");
    }

    #[test]
    fn test_truncate_chars() {
        assert_eq!(truncate_chars("hello", 10), "hello");
        assert_eq!(truncate_chars("Hello World", 2), "He");
        // Won't split in the middle of multibyte characters
        let s = "a🌍b".repeat(50);
        let truncated = truncate_chars(&s, 10);
        assert!(std::str::from_utf8(truncated.as_bytes()).is_ok());
    }

    #[test]
    fn test_percent_decode() {
        assert_eq!(
            percent_decode("https%3A%2F%2Fexample.com"),
            "https://example.com"
        );
        assert_eq!(percent_decode("hello+world"), "hello world");
        assert_eq!(percent_decode("%中文"), "%中文");
        assert_eq!(percent_decode("%E4%B8%AD%E6%96%87"), "中文");
    }
}
