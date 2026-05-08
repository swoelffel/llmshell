/// Truncate `s` to at most `budget` bytes, walking back to a valid UTF-8
/// char boundary if the budget falls in the middle of a multi-byte sequence.
pub fn truncate_to_byte_budget(s: &str, budget: usize) -> &str {
    if s.len() <= budget {
        return s;
    }
    let mut cap = budget;
    while cap > 0 && !s.is_char_boundary(cap) {
        cap -= 1;
    }
    &s[..cap]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_fits() {
        assert_eq!(truncate_to_byte_budget("hello", 10), "hello");
    }

    #[test]
    fn ascii_truncated() {
        assert_eq!(truncate_to_byte_budget("hello world", 5), "hello");
    }

    #[test]
    fn multibyte_boundary() {
        // '€' is 3 bytes (U+20AC). Budget of 4 on "a€b" (5 bytes) should
        // land on the char boundary before '€' because cap=4 is mid-sequence.
        let s = "a€b";
        assert_eq!(s.len(), 5);
        let result = truncate_to_byte_budget(s, 4);
        assert!(std::str::from_utf8(result.as_bytes()).is_ok());
        assert_eq!(result, "a€");
    }

    #[test]
    fn zero_budget() {
        assert_eq!(truncate_to_byte_budget("hello", 0), "");
    }
}
