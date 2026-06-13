//! Deterministic hash utilities.

/// FNV-1a 64-bit hash — deterministic across process restarts.
///
/// Used for content-based deduplication where the same input must
/// always produce the same output, regardless of process lifetime.
pub fn fnv1a_64(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &byte in data {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fnv1a_deterministic() {
        let a = fnv1a_64(b"hello world");
        let b = fnv1a_64(b"hello world");
        assert_eq!(a, b);
    }

    #[test]
    fn test_fnv1a_different_inputs() {
        assert_ne!(fnv1a_64(b"hello"), fnv1a_64(b"world"));
    }
}
