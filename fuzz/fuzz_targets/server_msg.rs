//! Any bytes as a server message: decoding never panics, and a decoded
//! message survives encoding and decoding again unchanged.
#![no_main]

use libfuzzer_sys::fuzz_target;
use shared::ServerMsg;
use shared::wire::Wire;

fuzz_target!(|data: &[u8]| {
    if let Ok(Some(msg)) = ServerMsg::decode_wire(data) {
        let again = ServerMsg::decode_wire(&msg.encode_wire()).expect("re-encoded message decodes");
        assert_eq!(again, Some(msg));
    }
});
