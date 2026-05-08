//! Context-window lookup by model id family.
//! Source-of-truth values are conservative: the *real* limit is
//! enforced server-side; we only need a usable denominator for the UI.

/// Returns the maximum context tokens for a known model family,
/// falling back to a conservative 8 192 for unknown ids.
pub fn context_window_for(model: &str) -> u32 {
    let m = strip_provider_prefix(model);
    if m.starts_with("gpt-5") {
        400_000
    } else if m.starts_with("gpt-4.1") {
        1_000_000
    } else if m.starts_with("gpt-4o") {
        128_000
    } else if m.starts_with("o1") || m.starts_with("o3") {
        200_000
    } else if m.starts_with("gpt-3.5") {
        16_385
    } else if m.starts_with("chatgpt-") {
        128_000
    } else {
        8_192
    }
}

fn strip_provider_prefix(m: &str) -> &str {
    m.split_once(':').map(|(_, r)| r).unwrap_or(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_families_mapped() {
        assert_eq!(context_window_for("gpt-4o-mini"), 128_000);
        assert_eq!(context_window_for("gpt-4.1"), 1_000_000);
        assert_eq!(context_window_for("gpt-5.4-mini"), 400_000);
        assert_eq!(context_window_for("o3-mini"), 200_000);
        assert_eq!(context_window_for("gpt-3.5-turbo"), 16_385);
    }

    #[test]
    fn unknown_falls_back_to_8k() {
        assert_eq!(context_window_for("totally-unknown-model"), 8_192);
    }

    #[test]
    fn provider_prefix_stripped() {
        assert_eq!(context_window_for("openai:gpt-4o-mini"), 128_000);
    }
}
