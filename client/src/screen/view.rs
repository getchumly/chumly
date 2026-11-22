//! Watching a screen share, on every platform: reassembling frames from
//! fragments, OpenH264 decoding, handing the picture to the UI.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;
use openh264::decoder::Decoder;
use openh264::formats::YUVSource;
use shared::ClientMsg;
use shared::media::ScreenFragment;

use super::{Counters, ScreenShared, Timing};
use crate::net::NetCommand;

/// A viewer does not request keyframes more often than this.
const KEYFRAME_RETRY: Duration = Duration::from_millis(500);

/// Watching someone's screen share. Dropping it stops decoding.
pub struct Viewer {
    shared: Arc<ScreenShared>,
    frame: Arc<Mutex<Option<egui::ColorImage>>>,
    counters: Arc<Counters>,
}

impl Viewer {
    pub fn start(
        user_id: shared::UserId,
        shared: Arc<ScreenShared>,
        net: tokio::sync::mpsc::UnboundedSender<NetCommand>,
        ctx: egui::Context,
    ) -> Result<Self, String> {
        let decoder = Decoder::new().map_err(|e| e.to_string())?;
        let (tx, rx) = std::sync::mpsc::channel();
        let frame = Arc::new(Mutex::new(None));
        let slot = frame.clone();
        let counters = Arc::new(Counters::default());
        let c = counters.clone();
        std::thread::Builder::new()
            .name("screen-decoder".into())
            .spawn(move || decode_loop(user_id, decoder, rx, slot, c, net, ctx))
            .map_err(|e| e.to_string())?;
        shared.attach(user_id, tx);
        Ok(Self { shared, frame, counters })
    }

    /// A new frame, if one arrived since the last call.
    pub fn take_frame(&self) -> Option<egui::ColorImage> {
        self.frame.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    /// frames: displayed frames, lost: lost in the network,
    /// waiting_key: picture is frozen until a keyframe arrives.
    pub fn counters(&self) -> &Counters {
        &self.counters
    }
}

impl Drop for Viewer {
    fn drop(&mut self) {
        // The channel closes and the decoder thread exits on its own.
        self.shared.detach();
    }
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

fn decode_loop(
    user_id: shared::UserId,
    mut decoder: Decoder,
    rx: Receiver<Vec<u8>>,
    slot: Arc<Mutex<Option<egui::ColorImage>>>,
    counters: Arc<Counters>,
    net: tokio::sync::mpsc::UnboundedSender<NetCommand>,
    ctx: egui::Context,
) {
    let mut asm = Assembler::default();
    let mut last_id: Option<u32> = None;
    let mut need_key = true;
    let mut last_request: Option<Instant> = None;
    let mut rgba = Vec::new();
    let request_key = |last: &mut Option<Instant>| {
        if last.is_none_or(|t| t.elapsed() >= KEYFRAME_RETRY) {
            *last = Some(Instant::now());
            let _ = net.send(NetCommand::Send(ClientMsg::RequestKeyframe { user_id }));
        }
    };

    while let Ok(payload) = rx.recv() {
        let Some(fragment) = ScreenFragment::decode(&payload) else { continue };
        let Some(frame) = asm.push(fragment) else { continue };
        // A frame was skipped: subsequent frames will decode with errors without it.
        if let Some(l) = last_id
            && frame.frame_id != l.wrapping_add(1)
        {
            counters.lost.fetch_add(frame.frame_id.wrapping_sub(l).wrapping_sub(1), Ordering::Relaxed);
            if !frame.keyframe {
                need_key = true;
            }
        }
        counters.waiting_key.store(need_key && !frame.keyframe, Ordering::Relaxed);
        last_id = Some(frame.frame_id);
        if need_key && !frame.keyframe {
            request_key(&mut last_request);
            continue;
        }
        let t = Instant::now();
        let image = match decoder.decode(&frame.data) {
            Ok(Some(yuv)) => {
                Timing::add(&counters.timing.decode_us, t);
                let t = Instant::now();
                let (w, h) = yuv.dimensions();
                rgba.resize(w * h * 4, 0);
                yuv.write_rgba8(&mut rgba);
                let image = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
                Timing::add(&counters.timing.convert_us, t);
                image
            }
            Ok(None) => continue,
            Err(e) => {
                tracing::debug!("decoding screen share frame: {e}");
                need_key = true;
                request_key(&mut last_request);
                continue;
            }
        };
        need_key = false;
        counters.waiting_key.store(false, Ordering::Relaxed);
        counters.frames.fetch_add(1, Ordering::Relaxed);
        *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(image);
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use openh264::OpenH264API;
    use openh264::encoder::Encoder;
    use openh264::formats::{BgraSliceU8, YUVBuffer};
    use shared::media::SCREEN_FRAGMENT_DATA;

    use super::super::{Quality, encoder_config};
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

    /// The sharer's exact encoder settings (as on Windows), through
    /// fragments and reassembly, into the viewer's decoder. Runs on every
    /// platform: this is what a Linux viewer gets from a Windows sharer.
    #[test]
    fn sharer_stream_decodes() {
        let (w, h) = (1280usize, 720usize);
        let quality = Quality::P720F30;
        let mut enc = Encoder::with_api_config(OpenH264API::from_source(), encoder_config(quality)).unwrap();
        let mut dec = Decoder::new().unwrap();
        let mut asm = Assembler::default();
        let mut yuv = YUVBuffer::new(w, h);
        let mut decoded = 0;
        for frame_id in 0..10u32 {
            // A moving gradient, so every frame has new content.
            let bgra: Vec<u8> =
                (0..w * h).flat_map(|i| [(i % w) as u8, (i / w) as u8, (frame_id * 20) as u8, 255]).collect();
            yuv.read_bgra8(BgraSliceU8::new(&bgra, (w, h)));
            let stream = enc.encode(&yuv).unwrap();
            let keyframe = frame_id == 0;
            let data = stream.to_vec();
            let count = data.len().div_ceil(SCREEN_FRAGMENT_DATA) as u16;
            // Fragments go through their wire encoding and arrive in reverse order.
            let wire: Vec<Vec<u8>> = data
                .chunks(SCREEN_FRAGMENT_DATA)
                .enumerate()
                .map(|(index, chunk)| {
                    ScreenFragment { frame_id, index: index as u16, count, keyframe, data: chunk }.encode()
                })
                .collect();
            let mut assembled = None;
            for p in wire.iter().rev() {
                assembled = asm.push(ScreenFragment::decode(p).unwrap()).or(assembled);
            }
            let frame = assembled.expect("frame reassembled");
            assert_eq!(frame.frame_id, frame_id);
            if let Some(out) = dec.decode(&frame.data).unwrap() {
                assert_eq!(out.dimensions(), (w, h));
                decoded += 1;
            }
        }
        assert_eq!(decoded, 10, "every frame decodes");
    }
}
