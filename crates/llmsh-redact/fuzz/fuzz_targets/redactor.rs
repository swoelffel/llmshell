//! Fuzz target — verifies that the redactor never panics and is idempotent:
//! redact(redact(x)) == redact(x). Catches catastrophic regex backtracking via
//! libFuzzer timeout, and idempotency bugs where a marker like `[REDACTED:foo]`
//! itself matches another pattern.

#![no_main]

use libfuzzer_sys::fuzz_target;
use once_cell::sync::Lazy;

static R: Lazy<llmsh_redact::Redactor> = Lazy::new(llmsh_redact::Redactor::default);

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };
    if s.len() > 8192 {
        return;
    }
    let once = R.redact(s);
    let twice = R.redact(&once);
    assert_eq!(
        once,
        twice,
        "redactor not idempotent for input of {} bytes",
        s.len()
    );
});
