//! Media is sent as unreliable QUIC datagrams.
//! The first byte is the stream kind. The server forwards datagrams by kind
//! without inspecting the payload.
//!
//! Client -> server: `[kind: u8][seq: u32 LE][payload]`
//! Server -> client: `[kind: u8][user_id: u32 LE][seq: u32 LE][payload]`
//!
//! Screen share payloads start with their own fragment header,
//! see [`ScreenFragment`]: an H.264 frame does not fit in one datagram.

use serde::{Deserialize, Serialize};

use crate::protocol::UserId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum MediaKind {
    /// Opus, 48 kHz, mono, 20 ms frames.
    Voice = 1,
    /// Screen share: H.264, frames split into fragments.
    Screen = 2,
    /// Screen share audio: Opus, 48 kHz, stereo, 20 ms frames.
    /// Delivered to the same viewers as the video.
    ScreenAudio = 3,
}

impl MediaKind {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::Voice),
            2 => Some(Self::Screen),
            3 => Some(Self::ScreenAudio),
            _ => None,
        }
    }
}

pub const VOICE_SAMPLE_RATE: u32 = 48_000;
/// 20 ms at 48 kHz.
pub const VOICE_FRAME_SAMPLES: usize = 960;

/// Largest datagram a client sends. The server prepends a 4-byte
/// author id and the result still fits in the minimum QUIC packet
/// size even with a poor MTU.
pub const MAX_CLIENT_DATAGRAM: usize = 1100;

const CLIENT_HEADER: usize = 1 + 4;
const SERVER_HEADER: usize = 1 + 4 + 4;

#[derive(Debug, PartialEq, Eq)]
pub struct ClientDatagram<'a> {
    pub kind: MediaKind,
    pub seq: u32,
    pub payload: &'a [u8],
}

#[derive(Debug, PartialEq, Eq)]
pub struct ServerDatagram<'a> {
    pub kind: MediaKind,
    pub user_id: UserId,
    pub seq: u32,
    pub payload: &'a [u8],
}

pub fn encode_client(kind: MediaKind, seq: u32, payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(CLIENT_HEADER + payload.len());
    v.push(kind as u8);
    v.extend_from_slice(&seq.to_le_bytes());
    v.extend_from_slice(payload);
    v
}

pub fn decode_client(data: &[u8]) -> Option<ClientDatagram<'_>> {
    if data.len() < CLIENT_HEADER {
        return None;
    }
    Some(ClientDatagram {
        kind: MediaKind::from_u8(data[0])?,
        seq: u32::from_le_bytes(data[1..5].try_into().ok()?),
        payload: &data[CLIENT_HEADER..],
    })
}

/// The server repacks a client datagram, prepending the author id.
pub fn encode_server(kind: MediaKind, user_id: UserId, seq: u32, payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(SERVER_HEADER + payload.len());
    v.push(kind as u8);
    v.extend_from_slice(&user_id.to_le_bytes());
    v.extend_from_slice(&seq.to_le_bytes());
    v.extend_from_slice(payload);
    v
}

pub fn decode_server(data: &[u8]) -> Option<ServerDatagram<'_>> {
    if data.len() < SERVER_HEADER {
        return None;
    }
    Some(ServerDatagram {
        kind: MediaKind::from_u8(data[0])?,
        user_id: u32::from_le_bytes(data[1..5].try_into().ok()?),
        seq: u32::from_le_bytes(data[5..9].try_into().ok()?),
        payload: &data[SERVER_HEADER..],
    })
}

/// A fragment of a screen share frame.
///
/// Layout: `[frame_id: u32 LE][index: u16 LE][count: u16 LE][flags: u8][data]`.
/// A frame is complete once all `count` fragments with the same `frame_id` arrive.
#[derive(Debug, PartialEq, Eq)]
pub struct ScreenFragment<'a> {
    pub frame_id: u32,
    pub index: u16,
    pub count: u16,
    /// Keyframe: decoding can start from it.
    pub keyframe: bool,
    pub data: &'a [u8],
}

pub const SCREEN_FRAGMENT_HEADER: usize = 4 + 2 + 2 + 1;
/// How many frame bytes fit in one datagram.
pub const SCREEN_FRAGMENT_DATA: usize = MAX_CLIENT_DATAGRAM - CLIENT_HEADER - SCREEN_FRAGMENT_HEADER;
const FLAG_KEYFRAME: u8 = 1;

impl ScreenFragment<'_> {
    pub fn encode(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(SCREEN_FRAGMENT_HEADER + self.data.len());
        v.extend_from_slice(&self.frame_id.to_le_bytes());
        v.extend_from_slice(&self.index.to_le_bytes());
        v.extend_from_slice(&self.count.to_le_bytes());
        v.push(if self.keyframe { FLAG_KEYFRAME } else { 0 });
        v.extend_from_slice(self.data);
        v
    }

    pub fn decode(payload: &[u8]) -> Option<ScreenFragment<'_>> {
        if payload.len() < SCREEN_FRAGMENT_HEADER {
            return None;
        }
        let f = ScreenFragment {
            frame_id: u32::from_le_bytes(payload[0..4].try_into().ok()?),
            index: u16::from_le_bytes(payload[4..6].try_into().ok()?),
            count: u16::from_le_bytes(payload[6..8].try_into().ok()?),
            keyframe: payload[8] & FLAG_KEYFRAME != 0,
            data: &payload[SCREEN_FRAGMENT_HEADER..],
        };
        (f.count > 0 && f.index < f.count).then_some(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_roundtrip() {
        let enc = encode_client(MediaKind::Voice, 42, &[1, 2, 3]);
        let d = decode_client(&enc).unwrap();
        assert_eq!(d, ClientDatagram { kind: MediaKind::Voice, seq: 42, payload: &[1, 2, 3] });
    }

    #[test]
    fn server_roundtrip() {
        let enc = encode_server(MediaKind::Screen, 7, u32::MAX, &[9]);
        let d = decode_server(&enc).unwrap();
        assert_eq!(d, ServerDatagram { kind: MediaKind::Screen, user_id: 7, seq: u32::MAX, payload: &[9] });
    }

    #[test]
    fn screen_fragment_roundtrip() {
        let f = ScreenFragment { frame_id: 9, index: 2, count: 3, keyframe: true, data: &[7, 8] };
        assert_eq!(ScreenFragment::decode(&f.encode()), Some(f));
        let bad = ScreenFragment { frame_id: 1, index: 3, count: 3, keyframe: false, data: &[] };
        assert_eq!(ScreenFragment::decode(&bad.encode()), None);
        let full = vec![0; SCREEN_FRAGMENT_DATA];
        let f = ScreenFragment { frame_id: 1, index: 0, count: 1, keyframe: false, data: &full };
        assert_eq!(encode_client(MediaKind::Screen, 0, &f.encode()).len(), MAX_CLIENT_DATAGRAM);
    }

    #[test]
    fn garbage_rejected() {
        assert!(decode_client(&[1, 2]).is_none());
        assert!(decode_client(&[99, 0, 0, 0, 0]).is_none());
        assert!(decode_server(&[1; 8]).is_none());
    }
}
