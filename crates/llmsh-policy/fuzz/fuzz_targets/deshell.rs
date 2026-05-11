//! Fuzz target — feeds arbitrary byte strings as a `bash -c <payload>` argv
//! into the policy classifier, asserting no panic and deterministic output.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(payload) = std::str::from_utf8(data) else {
        return;
    };
    if payload.len() > 4096 {
        return;
    }

    let args = vec!["-c".to_string(), payload.to_string()];
    let a1 = llmsh_policy::safe_commands::is_read_only_invocation("bash", &args);
    let a2 = llmsh_policy::safe_commands::is_read_only_invocation("bash", &args);
    assert_eq!(a1, a2, "non-deterministic classification");
});
