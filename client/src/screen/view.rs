//! Watching a screen share, on every platform: reassembling frames from
//! fragments, OpenH264 decoding, handing the picture to the UI.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;
use openh264::decoder::Decoder;
use openh264::formats::YUVSource;
use shared::ClientMsg;
use shared::media::ScreenFragment;

use super::packet::Assembler;
use super::{Counters, ScreenShared, Timing};
use crate::net::NetCommand;

/// A viewer does not request keyframes more often than this.
const KEYFRAME_RETRY: Duration = Duration::from_millis(500);

/// A decoded frame as the decoder gives it: YUV 4:2:0, full-size Y and
/// half-size U and V planes, each row `stride` bytes long. The UI converts it
/// to colors on the GPU (or on the CPU if that is not available).
pub struct YuvFrame {
    pub width: usize,
    pub height: usize,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
    /// Row lengths in bytes of Y, U and V.
    pub strides: (usize, usize, usize),
}

impl YuvFrame {
    /// Width and height of the U and V planes.
    pub fn chroma_size(&self) -> (usize, usize) {
        (self.width.div_ceil(2), self.height.div_ceil(2))
    }

    /// Converts to RGBA on the CPU: the fallback when the GPU path is not
    /// available. Limited-range BT.601, the same as the GPU shader and OpenH264.
    pub fn to_color_image(&self) -> egui::ColorImage {
        let (w, h) = (self.width, self.height);
        let mut pixels = Vec::with_capacity(w * h);
        for row in 0..h {
            let ys = &self.y[row * self.strides.0..][..w];
            let us = &self.u[(row / 2) * self.strides.1..];
            let vs = &self.v[(row / 2) * self.strides.2..];
            for (col, &y) in ys.iter().enumerate() {
                let [r, g, b] = bt601(y, us[col / 2], vs[col / 2]);
                pixels.push(egui::Color32::from_rgb(r, g, b));
            }
        }
        egui::ColorImage::new([w, h], pixels)
    }
}

/// One limited-range BT.601 pixel to RGB, with OpenH264's coefficients.
pub fn bt601(y: u8, u: u8, v: u8) -> [u8; 3] {
    let y = (f32::from(y) - 16.0) * (255.0 / 219.0);
    let u = f32::from(u) - 128.0;
    let v = f32::from(v) - 128.0;
    let k = 255.0 / 224.0;
    let r = y + k * 1.402 * v;
    let g = y - k * 1.402 * 0.299 / 0.587 * v - k * 1.772 * 0.114 / 0.587 * u;
    let b = y + k * 1.772 * u;
    [r, g, b].map(|c| c.clamp(0.0, 255.0) as u8)
}

/// A decoded frame for the UI.
pub enum Frame {
    /// YUV planes, for the GPU renderer.
    Yuv(YuvFrame),
    /// Already converted on the decoder thread: when the GPU path is not
    /// available (see [`Viewer::convert_on_cpu`]).
    Rgba(egui::ColorImage),
}

impl Frame {
    pub fn size(&self) -> [usize; 2] {
        match self {
            Frame::Yuv(f) => [f.width, f.height],
            Frame::Rgba(img) => img.size,
        }
    }
}

/// Watching someone's screen share. Dropping it stops decoding.
pub struct Viewer {
    shared: Arc<ScreenShared>,
    frame: Arc<Mutex<Option<Frame>>>,
    counters: Arc<Counters>,
    rgba: Arc<AtomicBool>,
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
        let counters = Arc::new(Counters::default());
        let rgba = Arc::new(AtomicBool::new(false));
        let out = Output { slot: frame.clone(), counters: counters.clone(), rgba: rgba.clone() };
        std::thread::Builder::new()
            .name("screen-decoder".into())
            .spawn(move || decode_loop(user_id, decoder, rx, out, net, ctx))
            .map_err(|e| e.to_string())?;
        shared.attach(user_id, tx);
        Ok(Self { shared, frame, counters, rgba })
    }

    /// A new frame, if one arrived since the last call.
    pub fn take_frame(&self) -> Option<Frame> {
        self.frame.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    /// From now on the decoder thread converts frames to RGBA itself (with
    /// OpenH264's converter, as before the GPU path): the GPU path failed.
    pub fn convert_on_cpu(&self) {
        self.rgba.store(true, Ordering::Relaxed);
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

/// Where the decoder thread puts its frames, and how.
struct Output {
    slot: Arc<Mutex<Option<Frame>>>,
    counters: Arc<Counters>,
    /// Convert to RGBA here instead of handing over YUV planes.
    rgba: Arc<AtomicBool>,
}

fn decode_loop(
    user_id: shared::UserId,
    mut decoder: Decoder,
    rx: Receiver<Vec<u8>>,
    out: Output,
    net: tokio::sync::mpsc::UnboundedSender<NetCommand>,
    ctx: egui::Context,
) {
    let Output { slot, counters, rgba: want_rgba } = out;
    let mut rgba = Vec::new();
    let mut asm = Assembler::default();
    let mut last_id: Option<u32> = None;
    let mut need_key = true;
    let mut last_request: Option<Instant> = None;
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
                let (width, height) = yuv.dimensions();
                let image = if want_rgba.load(Ordering::Relaxed) {
                    rgba.resize(width * height * 4, 0);
                    yuv.write_rgba8(&mut rgba);
                    Frame::Rgba(egui::ColorImage::from_rgba_unmultiplied([width, height], &rgba))
                } else {
                    // Only copy the planes out of the decoder's buffer: colors
                    // are converted later, on the GPU.
                    Frame::Yuv(YuvFrame {
                        width,
                        height,
                        y: yuv.y().to_vec(),
                        u: yuv.u().to_vec(),
                        v: yuv.v().to_vec(),
                        strides: yuv.strides(),
                    })
                };
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

    use super::super::Quality;
    use super::super::encode::encoder_config;
    use super::super::packet::split;
    use super::*;

    /// The sharer's exact encoder settings and fragmenting, through
    /// reassembly, into the viewer's decoder. Runs on every platform: this
    /// is what a Linux viewer gets from a Windows sharer.
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
            let data = stream.to_vec();
            // Fragments go through their wire encoding and arrive in reverse order.
            let wire: Vec<Vec<u8>> = split(frame_id, frame_id == 0, &data).unwrap().collect();
            let mut assembled = None;
            for p in wire.iter().rev() {
                assembled = asm.push(ScreenFragment::decode(p).unwrap()).or(assembled);
            }
            let frame = assembled.expect("frame reassembled");
            assert_eq!(frame.frame_id, frame_id);
            if let Some(out) = dec.decode(&frame.data).unwrap() {
                assert_eq!(out.dimensions(), (w, h));
                decoded += 1;
                if frame_id == 9 {
                    // Our conversion (the GPU shader uses the same formulas)
                    // matches OpenH264's on a real decoded frame.
                    let ours = YuvFrame {
                        width: w,
                        height: h,
                        y: out.y().to_vec(),
                        u: out.u().to_vec(),
                        v: out.v().to_vec(),
                        strides: out.strides(),
                    }
                    .to_color_image();
                    let mut theirs = vec![0u8; w * h * 4];
                    out.write_rgba8(&mut theirs);
                    for (a, b) in ours.pixels.iter().zip(theirs.chunks(4)) {
                        let d = [a.r().abs_diff(b[0]), a.g().abs_diff(b[1]), a.b().abs_diff(b[2])];
                        assert!(d.iter().all(|&d| d <= 1), "{a:?} vs {b:?}");
                    }
                }
            }
        }
        assert_eq!(decoded, 10, "every frame decodes");
    }

    #[test]
    fn bt601_range() {
        assert_eq!(bt601(16, 128, 128), [0, 0, 0]);
        assert_eq!(bt601(235, 128, 128), [255, 255, 255]);
        // OpenH264's own test value.
        assert_eq!(bt601(235, 240, 240), [255, 120, 255]);
    }
}
