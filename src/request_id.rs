/// Returns whether a request ID satisfies the crate's baseline policy.
///
/// Valid IDs contain 1–128 ASCII URI-unreserved characters.
#[must_use]
pub fn is_valid_request_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~'))
}

#[cfg(test)]
mod tests {
    use super::is_valid_request_id;

    #[test]
    fn accepts_every_baseline_character_at_length_boundaries() {
        assert!(is_valid_request_id("a"));
        assert!(is_valid_request_id(&"aZ09-._~".repeat(16)));
    }

    #[test]
    fn rejects_empty_oversized_and_non_unreserved_values() {
        assert!(!is_valid_request_id(""));
        assert!(!is_valid_request_id(&"a".repeat(129)));
        for invalid in ["has space", "slash/value", "ümlaut", "line\nfeed"] {
            assert!(!is_valid_request_id(invalid), "accepted {invalid:?}");
        }
    }
}
