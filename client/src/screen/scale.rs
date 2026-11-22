//! Downscaling captured frames to the stream's size.

/// Frame size after downscaling: at most `max_h` tall, same aspect
/// ratio, even dimensions (required by H.264 4:2:0).
pub fn target_size(w: u32, h: u32, max_h: u32) -> (u32, u32) {
    let (mut tw, mut th) = if h > max_h {
        let tw = (w as u64 * max_h as u64 / h as u64) as u32;
        (tw, max_h)
    } else {
        (w, h)
    };
    // OpenH264 cannot encode wider than 3840.
    if tw > 3840 {
        th = (th as u64 * 3840 / tw as u64) as u32;
        tw = 3840;
    }
    ((tw & !1).max(2), (th & !1).max(2))
}

/// Downscales a BGRA image (bilinear filter, SIMD). `pitch` is the source
/// row length in bytes: for captured frames it can exceed width × 4.
///
/// Runs on the calling thread on purpose. It used to split rows across
/// threads spawned for every frame, and in the running client (not in
/// headless tests) the OpenGL driver kept memory for every thread created:
/// about 80 MB per 5-second share from a 2560×1440 monitor.
pub fn scale_bgra(src: &[u8], sw: usize, sh: usize, pitch: usize, dw: usize, dh: usize) -> Vec<u8> {
    use fast_image_resize::images::{CroppedImage, Image, ImageRef};
    use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};

    let mut dst = vec![0u8; dw * dh * 4];
    if sw == dw && sh == dh {
        for (y, row) in dst.chunks_exact_mut(dw * 4).enumerate() {
            row.copy_from_slice(&src[y * pitch..y * pitch + dw * 4]);
        }
        return dst;
    }
    thread_local! {
        // Keeps its scratch buffers between frames.
        static RESIZER: std::cell::RefCell<Resizer> = std::cell::RefCell::new(Resizer::new());
    }
    let mut resize = || -> Result<(), String> {
        // The source as an image as wide as a whole row (padding included),
        // then a view of its visible part. A view, not `ResizeOptions::crop`:
        // with crop the filter still reads pixels past the edge, and the
        // padding showed up as a stripe along the right side.
        let whole = ImageRef::new((pitch / 4) as u32, sh as u32, src, PixelType::U8x4).map_err(|e| e.to_string())?;
        let visible = CroppedImage::new(&whole, 0, 0, sw as u32, sh as u32).map_err(|e| e.to_string())?;
        let mut dst_img =
            Image::from_slice_u8(dw as u32, dh as u32, &mut dst, PixelType::U8x4).map_err(|e| e.to_string())?;
        let options = ResizeOptions::new()
            .resize_alg(ResizeAlg::Convolution(FilterType::Bilinear))
            // Capture alpha is not meaningful: no premultiplication.
            .use_alpha(false);
        RESIZER.with_borrow_mut(|r| r.resize(&visible, &mut dst_img, &options)).map_err(|e| e.to_string())
    };
    if let Err(e) = resize() {
        tracing::warn!("downscaling frame: {e}");
    }
    dst
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_size_keeps_aspect_and_parity() {
        assert_eq!(target_size(3840, 2160, 1080), (1920, 1080));
        assert_eq!(target_size(1280, 720, 1080), (1280, 720));
        assert_eq!(target_size(2560, 1440, 720), (1280, 720));
        assert_eq!(target_size(801, 601, 1080), (800, 600));
        assert_eq!(target_size(7680, 1080, 1080), (3840, 540));
    }

    #[test]
    fn scale_identity_strips_padding() {
        // 2x2 with a 12-byte row pitch instead of 8.
        let src = [1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 0, 0, 9, 10, 11, 12, 13, 14, 15, 16, 0, 0, 0, 0];
        assert_eq!(scale_bgra(&src, 2, 2, 12, 2, 2), (1..=16).collect::<Vec<u8>>());
    }

    #[test]
    fn scale_down_ignores_row_padding() {
        // 8x4 visible pixels of one color, rows padded to 10 pixels with white.
        let (sw, sh, pitch) = (8, 4, 10 * 4);
        let mut src = Vec::new();
        for _ in 0..sh {
            src.extend([10u8, 20, 30, 255].repeat(sw));
            src.extend([255u8; 8]);
        }
        let out = scale_bgra(&src, sw, sh, pitch, 4, 2);
        assert_eq!(out.len(), 4 * 2 * 4);
        for px in out.chunks(4) {
            assert!(px[..3].iter().zip([10, 20, 30]).all(|(a, b)| a.abs_diff(b) <= 1), "padding leaked: {px:?}");
        }
    }

    #[test]
    fn scale_down_averages() {
        // A solid 4x4 downscales to a 2x2 of the same color.
        let src: Vec<u8> = [10u8, 20, 30, 255].repeat(16);
        let out = scale_bgra(&src, 4, 4, 16, 2, 2);
        for px in out.chunks(4) {
            assert!(px.iter().zip([10, 20, 30, 255]).all(|(a, b)| a.abs_diff(b) <= 1), "{px:?}");
        }
    }
}
