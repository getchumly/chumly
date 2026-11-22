//! The picture of our own share for the UI: the thumbnail in the "me"
//! block and the "Your screen" window.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, Ordering};

use eframe::egui::{Color32, ColorImage};

use super::scale::{scale_bgra, target_size};

/// What the UI wants to see of our own share.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PreviewMode {
    /// Nothing: no share, or the preview is hidden.
    Off,
    /// The thumbnail in the "me" block: small and a few times a second.
    Thumbnail,
    /// The "Your screen" window: every frame at the stream's resolution.
    Full,
}

/// Thumbnail frames are downscaled to at most this height: the thumbnail is
/// under 260 px tall even in the widest panel, 360 leaves room for 1.5x screens.
pub const THUMBNAIL_MAX_HEIGHT: u32 = 360;
/// The thumbnail is updated at most this often.
pub const THUMBNAIL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

/// The picture of our own share for the UI. The encoder thread puts the
/// frames it captured, before encoding: nothing goes over the network, there
/// is no delay and no sound.
pub struct Preview {
    mode: AtomicU8,
    frame: Mutex<Option<ColorImage>>,
}

impl Default for Preview {
    fn default() -> Self {
        Self { mode: AtomicU8::new(PreviewMode::Off as u8), frame: Mutex::new(None) }
    }
}

impl Preview {
    pub fn set_mode(&self, mode: PreviewMode) {
        self.mode.store(mode as u8, Ordering::Relaxed);
        if mode == PreviewMode::Off {
            self.frame.lock().unwrap_or_else(|e| e.into_inner()).take();
        }
    }

    pub fn mode(&self) -> PreviewMode {
        match self.mode.load(Ordering::Relaxed) {
            m if m == PreviewMode::Thumbnail as u8 => PreviewMode::Thumbnail,
            m if m == PreviewMode::Full as u8 => PreviewMode::Full,
            _ => PreviewMode::Off,
        }
    }

    /// The newest frame since the last call.
    pub fn take_frame(&self) -> Option<ColorImage> {
        self.frame.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    pub(super) fn put(&self, image: ColorImage) {
        let mut frame = self.frame.lock().unwrap_or_else(|e| e.into_inner());
        // Turned off meanwhile (the share stopped): keep nothing for the next one.
        if self.mode() != PreviewMode::Off {
            *frame = Some(image);
        }
    }
}

/// A captured BGRA frame (`w` × `h`, tightly packed) as a preview image:
/// downscaled for the thumbnail, opaque (capture alpha is not meaningful).
pub(super) fn preview_image(bgra: &[u8], w: usize, h: usize, mode: PreviewMode) -> ColorImage {
    let scaled;
    let (src, w, h) = if mode == PreviewMode::Thumbnail && h > THUMBNAIL_MAX_HEIGHT as usize {
        let (tw, th) = target_size(w as u32, h as u32, THUMBNAIL_MAX_HEIGHT);
        let (tw, th) = (tw as usize, th as usize);
        scaled = scale_bgra(bgra, w, h, w * 4, tw, th);
        (scaled.as_slice(), tw, th)
    } else {
        (bgra, w, h)
    };
    let pixels = src.as_chunks::<4>().0.iter().map(|px| Color32::from_rgb(px[2], px[1], px[0])).collect();
    ColorImage::new([w, h], pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_swaps_channels_and_ignores_alpha() {
        // One blue-ish pixel in BGRA with zero alpha.
        let img = preview_image(&[10, 20, 30, 0], 1, 1, PreviewMode::Full);
        assert_eq!(img.size, [1, 1]);
        assert_eq!(img.pixels[0], Color32::from_rgb(30, 20, 10));
    }

    #[test]
    fn preview_thumbnail_is_downscaled_full_is_not() {
        let (w, h) = (1920, 1080);
        let src = vec![128u8; w * h * 4];
        let thumb = preview_image(&src, w, h, PreviewMode::Thumbnail);
        assert_eq!(thumb.size, [640, THUMBNAIL_MAX_HEIGHT as usize]);
        let full = preview_image(&src, w, h, PreviewMode::Full);
        assert_eq!(full.size, [w, h]);
        // A small frame is not upscaled for the thumbnail.
        let small = preview_image(&vec![0u8; 320 * 200 * 4], 320, 200, PreviewMode::Thumbnail);
        assert_eq!(small.size, [320, 200]);
    }

    #[test]
    fn preview_off_drops_the_frame() {
        let p = Preview::default();
        p.set_mode(PreviewMode::Thumbnail);
        p.put(ColorImage::filled([1, 1], Color32::RED));
        p.set_mode(PreviewMode::Off);
        assert!(p.take_frame().is_none());
        assert_eq!(p.mode(), PreviewMode::Off);
    }
}
