//! `Cow<'a, str>` avoids allocating when borrowed text is already normalized.

use std::borrow::Cow;

pub fn normalize_prompt(input: &str) -> Cow<'_, str> {
    let trimmed = input.trim();
    if trimmed == input {
        Cow::Borrowed(input)
    } else {
        Cow::Owned(trimmed.to_string())
    }
}

/// `to_mut` clones borrowed data only when mutation is actually required.
pub fn ensure_period(mut input: Cow<'_, str>) -> Cow<'_, str> {
    if !input.ends_with('.') {
        input.to_mut().push('.');
    }
    input
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn borrows_clean_input_and_owns_modified_input() {
        assert!(matches!(normalize_prompt("hello"), Cow::Borrowed(_)));
        assert!(matches!(normalize_prompt(" hello "), Cow::Owned(_)));
    }

    #[test]
    fn to_mut_allocates_only_for_a_change() {
        assert!(matches!(
            ensure_period(Cow::Borrowed("done.")),
            Cow::Borrowed(_)
        ));
        assert_eq!(ensure_period(Cow::Borrowed("done")), "done.");
    }
}
