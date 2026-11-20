//! Any text as the GitHub release answer, and as versions to compare:
//! never panics; a version is never newer than itself.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else { return };
    let _ = shared::release::Release::parse(text);
    let (a, b) = text.split_once('\n').unwrap_or((text, "0.1.0"));
    let _ = shared::release::is_newer(a, b);
    assert!(!shared::release::is_newer(a, a));
});
