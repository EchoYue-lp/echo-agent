//! Moves, borrowing, cloning, and explicit lifetimes.

/// Takes ownership of a vector and returns it after mutation.
pub fn append_label(mut labels: Vec<String>, label: impl Into<String>) -> Vec<String> {
    labels.push(label.into());
    labels
}

/// Borrows a string instead of taking ownership.
pub fn character_count(value: &str) -> usize {
    value.chars().count()
}

/// The returned reference may come from either input, so the lifetime is named.
pub fn first_non_empty<'a>(primary: &'a str, fallback: &'a str) -> &'a str {
    if primary.trim().is_empty() {
        fallback
    } else {
        primary
    }
}

/// Cloning is explicit when the caller needs an independently owned value.
pub fn owned_title(value: &str) -> String {
    value.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn borrowing_keeps_the_original_available() {
        let title = String::from("学习所有权");
        assert_eq!(character_count(&title), 5);
        assert_eq!(title, "学习所有权");
    }
}
