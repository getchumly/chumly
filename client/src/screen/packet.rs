//! An encoded frame does not fit in one datagram: the sharer splits it into
//! fragments ([`split`]), viewers put them back together ([`Assembler`]).

use std::collections::HashMap;

use shared::media::{SCREEN_FRAGMENT_DATA, ScreenFragment};

/// The frame as encoded fragments, ready to send. `None` if it needs more
/// fragments than the header can count.
pub(super) fn split(frame_id: u32, keyframe: bool, data: &[u8]) -> Option<impl Iterator<Item = Vec<u8>> + '_> {
    let count = u16::try_from(data.len().div_ceil(SCREEN_FRAGMENT_DATA)).ok()?;
    Some(data.chunks(SCREEN_FRAGMENT_DATA).enumerate().map(move |(index, chunk)| {
        ScreenFragment { frame_id, index: index as u16, count, keyframe, data: chunk }.encode()
    }))
}

/// Reassembles frames from fragments. Fragments may arrive out of order
/// or get lost: then the frame never completes and is discarded.
#[derive(Default)]
pub(super) struct Assembler {
    partial: HashMap<u32, Partial>,
    /// Last completed frame: anything older is no longer needed.
    newest: Option<u32>,
}

struct Partial {
    keyframe: bool,
    parts: Vec<Option<Vec<u8>>>,
    received: usize,
}

pub(super) struct Assembled {
    pub(super) frame_id: u32,
    pub(super) keyframe: bool,
    pub(super) data: Vec<u8>,
}

/// `a` is newer than `b`, accounting for counter wraparound.
fn newer(a: u32, b: u32) -> bool {
    a != b && a.wrapping_sub(b) < u32::MAX / 2
}

impl Assembler {
    pub(super) fn push(&mut self, f: ScreenFragment<'_>) -> Option<Assembled> {
        if self.newest.is_some_and(|n| !newer(f.frame_id, n)) {
            return None;
        }
        let p = self.partial.entry(f.frame_id).or_insert_with(|| Partial {
            keyframe: f.keyframe,
            parts: vec![None; f.count as usize],
            received: 0,
        });
        let slot = p.parts.get_mut(f.index as usize)?;
        if slot.is_none() {
            *slot = Some(f.data.to_vec());
            p.received += 1;
        }
        if p.received < p.parts.len() {
            // Do not accumulate incomplete frames: keep only the last 8.
            if self.partial.len() > 16 {
                let id = f.frame_id;
                self.partial.retain(|k, _| !newer(id, *k) || id.wrapping_sub(*k) < 8);
            }
            return None;
        }
        let p = self.partial.remove(&f.frame_id)?;
        self.newest = Some(f.frame_id);
        let id = f.frame_id;
        self.partial.retain(|k, _| newer(*k, id));
        let data = p.parts.into_iter().flatten().flatten().collect();
        Some(Assembled { frame_id: id, keyframe: p.keyframe, data })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frag(frame_id: u32, index: u16, count: u16, data: &[u8]) -> ScreenFragment<'_> {
        ScreenFragment { frame_id, index, count, keyframe: index == 0 && frame_id == 0, data }
    }

    #[test]
    fn assembles_out_of_order_and_drops_stale() {
        let mut a = Assembler::default();
        assert!(a.push(frag(0, 1, 2, b"cd")).is_none());
        let f = a.push(frag(0, 0, 2, b"ab")).unwrap();
        assert_eq!((f.frame_id, f.data.as_slice()), (0, &b"abcd"[..]));
        // Frame 1 lost a fragment, frame 2 completed: frame 1 is discarded.
        assert!(a.push(frag(1, 0, 2, b"x")).is_none());
        assert_eq!(a.push(frag(2, 0, 1, b"y")).unwrap().frame_id, 2);
        assert!(a.push(frag(1, 1, 2, b"z")).is_none());
        assert!(a.partial.is_empty());
    }

    #[test]
    fn split_then_assemble_roundtrip() {
        let data: Vec<u8> = (0..SCREEN_FRAGMENT_DATA * 3 + 17).map(|i| i as u8).collect();
        let wire: Vec<Vec<u8>> = split(7, true, &data).unwrap().collect();
        assert_eq!(wire.len(), 4);
        let mut a = Assembler::default();
        let mut done = None;
        for p in wire.iter().rev() {
            done = a.push(ScreenFragment::decode(p).unwrap()).or(done);
        }
        let f = done.expect("reassembled");
        assert_eq!((f.frame_id, f.keyframe), (7, true));
        assert_eq!(f.data, data);
    }

    #[test]
    fn split_refuses_oversized_frames() {
        let data = vec![0u8; SCREEN_FRAGMENT_DATA * (u16::MAX as usize + 1)];
        assert!(split(0, false, &data).is_none());
    }
}
