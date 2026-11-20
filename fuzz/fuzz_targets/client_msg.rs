//! Any bytes as a client message: decoding never panics, and a decoded
//! message survives encoding and decoding again unchanged.
#![no_main]

use libfuzzer_sys::fuzz_target;
use shared::ClientMsg;
use shared::wire::Wire;

fuzz_target!(|data: &[u8]| {
    if let Ok(Some(msg)) = ClientMsg::decode_wire(data) {
        let again = ClientMsg::decode_wire(&msg.encode_wire()).expect("re-encoded message decodes");
        assert_eq!(again, Some(msg));
    }
});
