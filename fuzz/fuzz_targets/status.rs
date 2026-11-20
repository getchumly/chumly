//! Any bytes as a server status answer: never panics.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = shared::wire::decode_status(data);
});
