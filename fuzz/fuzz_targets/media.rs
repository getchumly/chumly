//! Any bytes as a media datagram (client and server format) and as a
//! screen share fragment: never panics; what decodes encodes back the same.
#![no_main]

use libfuzzer_sys::fuzz_target;
use shared::media::{self, ScreenFragment};

fuzz_target!(|data: &[u8]| {
    if let Some(d) = media::decode_client(data) {
        assert_eq!(media::encode_client(d.kind, d.seq, d.payload), data);
        if let Some(f) = ScreenFragment::decode(d.payload) {
            assert!(f.index < f.count);
            assert_eq!(ScreenFragment::decode(&f.encode()), Some(f));
        }
    }
    if let Some(d) = media::decode_server(data) {
        assert_eq!(media::encode_server(d.kind, d.user_id, d.seq, d.payload), data);
    }
});
