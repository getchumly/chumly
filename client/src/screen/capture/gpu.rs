//! Preparing captured frames on the GPU (Windows): the D3D11 video processor
//! downscales the captured BGRA texture and converts it to NV12 in one pass,
//! so only the finished frame at the stream's size (~3 MB at 1080p instead
//! of ~15 MB at 2560×1440) is read back to memory.
//!
//! Colours: limited-range BT.601, the same as OpenH264's own BGRA → YUV
//! conversion on the CPU path and the viewer's shader.

use std::mem::ManuallyDrop;

use openh264::formats::YUVSource;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_CPU_ACCESS_READ, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_TEX2D_VPIV, D3D11_TEX2D_VPOV, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    D3D11_USAGE_STAGING, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE, D3D11_VIDEO_PROCESSOR_COLOR_SPACE,
    D3D11_VIDEO_PROCESSOR_CONTENT_DESC, D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_INPUT,
    D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_OUTPUT, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC,
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC,
    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_STREAM, D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
    D3D11_VPIV_DIMENSION_TEXTURE2D, D3D11_VPOV_DIMENSION_TEXTURE2D, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
    ID3D11VideoContext, ID3D11VideoDevice, ID3D11VideoProcessor, ID3D11VideoProcessorEnumerator,
    ID3D11VideoProcessorInputView, ID3D11VideoProcessorOutputView,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};
use windows::core::Interface;

/// Everything for one source size → stream size; rebuilt when either changes
/// (a shared window was resized).
struct Pipeline {
    src: (u32, u32),
    dst: (u32, u32),
    format: DXGI_FORMAT,
    processor: ID3D11VideoProcessor,
    /// Our copy of the captured frame: the capture's own textures come from a
    /// small pool that is reused, and may not allow a video processor view.
    input: ID3D11Texture2D,
    input_view: ID3D11VideoProcessorInputView,
    output: ID3D11Texture2D,
    output_view: ID3D11VideoProcessorOutputView,
    /// CPU-readable copy of `output`.
    staging: ID3D11Texture2D,
}

/// Converts captured textures to I420 frames at the stream's size. Lives on
/// the capture thread, which owns the device's immediate context.
pub(super) struct GpuPrep {
    device: ID3D11Device,
    video_device: ID3D11VideoDevice,
    video_context: ID3D11VideoContext,
    pipeline: Option<Pipeline>,
}

impl GpuPrep {
    pub(super) fn new(device: &ID3D11Device, context: &ID3D11DeviceContext) -> Result<Self, String> {
        let video_device: ID3D11VideoDevice = device.cast().map_err(|e| format!("no video device: {e}"))?;
        let video_context: ID3D11VideoContext = context.cast().map_err(|e| format!("no video context: {e}"))?;
        Ok(Self { device: device.clone(), video_device, video_context, pipeline: None })
    }

    /// Downscales `texture` (BGRA, `src` = its width and height) to `dst` and
    /// returns it as tightly packed NV12: the Y plane, then interleaved U and V.
    pub(super) fn convert(
        &mut self,
        context: &ID3D11DeviceContext,
        texture: &ID3D11Texture2D,
        format: DXGI_FORMAT,
        src: (u32, u32),
        dst: (u32, u32),
        out: &mut Vec<u8>,
    ) -> Result<(), String> {
        let stale = self.pipeline.as_ref().is_none_or(|p| p.src != src || p.dst != dst || p.format != format);
        if stale {
            self.pipeline = None;
            self.pipeline = Some(self.build(format, src, dst).map_err(|e| format!("video processor setup: {e}"))?);
        }
        let p = self.pipeline.as_ref().expect("built above");
        // SAFETY: all objects come from the same device; the immediate
        // context is only used on this (the capture) thread.
        unsafe {
            context.CopyResource(&p.input, texture);
            let stream = D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: true.into(),
                pInputSurface: ManuallyDrop::new(Some(p.input_view.clone())),
                ..Default::default()
            };
            let mut streams = [stream];
            let blt = self.video_context.VideoProcessorBlt(&p.processor, &p.output_view, 0, &streams);
            ManuallyDrop::drop(&mut streams[0].pInputSurface);
            blt.map_err(|e| format!("video processor: {e}"))?;
            context.CopyResource(&p.staging, &p.output);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            context.Map(&p.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).map_err(|e| format!("map: {e}"))?;
            let pitch = mapped.RowPitch as usize;
            let (w, h) = (dst.0 as usize, dst.1 as usize);
            // NV12: the Y rows, then the interleaved UV rows, `pitch` apart.
            let data = std::slice::from_raw_parts(mapped.pData as *const u8, pitch * h * 3 / 2);
            strip_pitch(data, pitch, w, h, out);
            context.Unmap(&p.staging, 0);
        }
        Ok(())
    }

    fn build(&self, format: DXGI_FORMAT, src: (u32, u32), dst: (u32, u32)) -> windows::core::Result<Pipeline> {
        let rate = DXGI_RATIONAL { Numerator: 60, Denominator: 1 };
        let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: rate,
            InputWidth: src.0,
            InputHeight: src.1,
            OutputFrameRate: rate,
            OutputWidth: dst.0,
            OutputHeight: dst.1,
            Usage: D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
        };
        // SAFETY: plain D3D11 object creation with valid descriptions.
        unsafe {
            let vd = &self.video_device;
            let enumerator: ID3D11VideoProcessorEnumerator = vd.CreateVideoProcessorEnumerator(&content)?;
            let input_ok =
                enumerator.CheckVideoProcessorFormat(format)? & D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_INPUT.0 as u32;
            let output_ok = enumerator.CheckVideoProcessorFormat(DXGI_FORMAT_NV12)?
                & D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_OUTPUT.0 as u32;
            if input_ok == 0 || output_ok == 0 {
                return Err(windows::core::Error::new(
                    windows::Win32::Foundation::E_NOTIMPL,
                    "the video processor cannot convert this format to NV12",
                ));
            }
            let processor = vd.CreateVideoProcessor(&enumerator, 0)?;
            let vc = &self.video_context;
            // No driver "enhancements" (sharpening, noise reduction, ...).
            vc.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false);
            vc.VideoProcessorSetStreamFrameFormat(&processor, 0, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE);
            // Input: full-range RGB. Output: BT.601, limited range (16–235).
            // Bits: Usage, RGB_Range, YCbCr_Matrix, YCbCr_xvYCC, Nominal_Range (2 bits).
            vc.VideoProcessorSetStreamColorSpace(&processor, 0, &D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: 0 });
            vc.VideoProcessorSetOutputColorSpace(&processor, &D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: 1 << 4 });
            let src_rect = RECT { left: 0, top: 0, right: src.0 as i32, bottom: src.1 as i32 };
            let dst_rect = RECT { left: 0, top: 0, right: dst.0 as i32, bottom: dst.1 as i32 };
            vc.VideoProcessorSetStreamSourceRect(&processor, 0, true, Some(&src_rect));
            vc.VideoProcessorSetStreamDestRect(&processor, 0, true, Some(&dst_rect));
            vc.VideoProcessorSetOutputTargetRect(&processor, true, Some(&dst_rect));

            let bind = (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32;
            let input = self.texture(src, format, D3D11_USAGE_DEFAULT.0, bind, 0)?;
            let input_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                FourCC: 0,
                ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VPIV { MipSlice: 0, ArraySlice: 0 },
                },
            };
            let mut input_view = None;
            vd.CreateVideoProcessorInputView(&input, &enumerator, &input_desc, Some(&mut input_view))?;

            let output =
                self.texture(dst, DXGI_FORMAT_NV12, D3D11_USAGE_DEFAULT.0, D3D11_BIND_RENDER_TARGET.0 as u32, 0)?;
            let output_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 } },
            };
            let mut output_view = None;
            vd.CreateVideoProcessorOutputView(&output, &enumerator, &output_desc, Some(&mut output_view))?;

            let staging =
                self.texture(dst, DXGI_FORMAT_NV12, D3D11_USAGE_STAGING.0, 0, D3D11_CPU_ACCESS_READ.0 as u32)?;
            let missing = || windows::core::Error::from(windows::Win32::Foundation::E_POINTER);
            Ok(Pipeline {
                src,
                dst,
                format,
                processor,
                input,
                input_view: input_view.ok_or_else(missing)?,
                output,
                output_view: output_view.ok_or_else(missing)?,
                staging,
            })
        }
    }

    fn texture(
        &self,
        (w, h): (u32, u32),
        format: DXGI_FORMAT,
        usage: i32,
        bind: u32,
        cpu: u32,
    ) -> windows::core::Result<ID3D11Texture2D> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: windows::Win32::Graphics::Direct3D11::D3D11_USAGE(usage),
            BindFlags: bind,
            CPUAccessFlags: cpu,
            MiscFlags: 0,
        };
        let mut tex = None;
        // SAFETY: a valid description, no initial data.
        unsafe { self.device.CreateTexture2D(&desc, None, Some(&mut tex))? };
        tex.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_POINTER))
    }
}

/// Whether the capture's texture format can go through the video processor.
pub(super) fn supported_format(format: DXGI_FORMAT) -> bool {
    format == DXGI_FORMAT_B8G8R8A8_UNORM
}

/// NV12 with rows `pitch` bytes apart (both planes) → tightly packed NV12.
fn strip_pitch(nv12: &[u8], pitch: usize, w: usize, h: usize, out: &mut Vec<u8>) {
    out.clear();
    out.reserve(w * h * 3 / 2);
    for row in nv12[..pitch * h * 3 / 2].chunks_exact(pitch) {
        out.extend_from_slice(&row[..w]);
    }
}

/// Tightly packed NV12 → tightly packed I420 (Y, then U, then V), the
/// software encoder's input.
pub(in crate::screen) fn nv12_to_i420(nv12: &[u8], w: usize, h: usize, out: &mut Vec<u8>) {
    let luma = w * h;
    out.clear();
    out.extend_from_slice(&nv12[..luma]);
    out.resize(luma * 3 / 2, 0);
    let (u, v) = out[luma..].split_at_mut(luma / 4);
    for (i, pair) in nv12[luma..luma * 3 / 2].as_chunks::<2>().0.iter().enumerate() {
        u[i] = pair[0];
        v[i] = pair[1];
    }
}

/// A tightly packed I420 frame (Y, then U, then V) as the software encoder's input.
pub(in crate::screen) struct I420Frame<'a> {
    pub(in crate::screen) data: &'a [u8],
    pub(in crate::screen) width: usize,
    pub(in crate::screen) height: usize,
}

impl YUVSource for I420Frame<'_> {
    fn dimensions(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    fn strides(&self) -> (usize, usize, usize) {
        (self.width, self.width / 2, self.width / 2)
    }

    fn y(&self) -> &[u8] {
        &self.data[..self.width * self.height]
    }

    fn u(&self) -> &[u8] {
        let luma = self.width * self.height;
        &self.data[luma..luma + luma / 4]
    }

    fn v(&self) -> &[u8] {
        let luma = self.width * self.height;
        &self.data[luma + luma / 4..]
    }
}

/// A tightly packed NV12 frame (`w` × `h`, both even) as BGRA, for the
/// sharer's own preview.
pub(in crate::screen) fn nv12_to_bgra(nv12: &[u8], w: usize, h: usize) -> Vec<u8> {
    let (y_plane, uv) = nv12.split_at(w * h);
    let mut out = Vec::with_capacity(w * h * 4);
    for (row, ys) in y_plane.chunks_exact(w).enumerate() {
        let c = (row / 2) * w;
        for (x, &y) in ys.iter().enumerate() {
            let i = c + (x & !1);
            let [r, g, b] = crate::screen::view::bt601(y, uv[i], uv[i + 1]);
            out.extend_from_slice(&[b, g, r, 255]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use openh264::formats::{BgraSliceU8, YUVBuffer};
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
    use windows::Win32::Graphics::Direct3D11::{
        D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11_SUBRESOURCE_DATA, D3D11CreateDevice,
    };

    use super::*;

    #[test]
    fn nv12_preview_matches_colours() {
        // 4x2: left half limited-range black, right half white; chroma neutral.
        let nv12 = [16, 16, 235, 235, 16, 16, 235, 235, 128, 128, 128, 128];
        let bgra = nv12_to_bgra(&nv12, 4, 2);
        assert_eq!(bgra.len(), 4 * 2 * 4);
        for (i, px) in bgra.chunks(4).enumerate() {
            let want = if i % 4 < 2 { 0 } else { 255 };
            assert_eq!(px, [want, want, want, 255], "pixel {i}");
        }
    }

    #[test]
    fn nv12_padding_and_planes() {
        // 4x2 frame, rows padded to 6 bytes.
        let padded = [1, 2, 3, 4, 0, 0, 5, 6, 7, 8, 0, 0, 10, 20, 11, 21, 0, 0];
        let mut nv12 = Vec::new();
        strip_pitch(&padded, 6, 4, 2, &mut nv12);
        assert_eq!(nv12, [1, 2, 3, 4, 5, 6, 7, 8, 10, 20, 11, 21]);
        let mut i420 = Vec::new();
        nv12_to_i420(&nv12, 4, 2, &mut i420);
        assert_eq!(i420, [1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 20, 21]);
    }

    /// The GPU conversion gives the same colours as OpenH264's own (the CPU
    /// path), so viewers see no difference. Skipped without a GPU that has a
    /// video processor (CI).
    #[test]
    fn gpu_conversion_matches_openh264() {
        let mut device = None;
        let mut context = None;
        // SAFETY: plain device creation.
        let created = unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                Default::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        };
        let (Ok(()), Some(device), Some(context)) = (created, device, context) else {
            eprintln!("no hardware D3D11 device, skipped");
            return;
        };
        let Ok(mut prep) = GpuPrep::new(&device, &context) else {
            eprintln!("no video processor, skipped");
            return;
        };
        // Vertical colour bands, each 16 pixels wide, so the 2:1 downscale
        // and chroma subsampling see solid colour away from the edges.
        let colors: [[u8; 4]; 8] = [
            [0, 0, 0, 255],
            [255, 255, 255, 255],
            [0, 0, 255, 255],
            [0, 255, 0, 255],
            [255, 0, 0, 255],
            [30, 140, 200, 255],
            [128, 128, 128, 255],
            [200, 60, 90, 255],
        ];
        let (sw, sh, band) = (256usize, 64usize, 32usize);
        let mut bgra = Vec::with_capacity(sw * sh * 4);
        for _ in 0..sh {
            for x in 0..sw {
                bgra.extend_from_slice(&colors[x / band]);
            }
        }
        let desc = D3D11_TEXTURE2D_DESC {
            Width: sw as u32,
            Height: sh as u32,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: 0,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let init =
            D3D11_SUBRESOURCE_DATA { pSysMem: bgra.as_ptr().cast(), SysMemPitch: (sw * 4) as u32, SysMemSlicePitch: 0 };
        let mut tex = None;
        // SAFETY: `init` points at `bgra`, which outlives the call.
        unsafe { device.CreateTexture2D(&desc, Some(&init), Some(&mut tex)) }.unwrap();
        let tex = tex.unwrap();

        let (dw, dh) = (sw / 2, sh / 2);
        let mut nv12 = Vec::new();
        prep.convert(
            &context,
            &tex,
            DXGI_FORMAT_B8G8R8A8_UNORM,
            (sw as u32, sh as u32),
            (dw as u32, dh as u32),
            &mut nv12,
        )
        .unwrap();
        assert_eq!(nv12.len(), dw * dh * 3 / 2);
        let mut i420 = Vec::new();
        nv12_to_i420(&nv12, dw, dh, &mut i420);

        // Reference: OpenH264's conversion of the same picture at the target size.
        let small: Vec<u8> = (0..dh).flat_map(|_| (0..dw).flat_map(|x| colors[x * 2 / band])).collect();
        let mut reference = YUVBuffer::new(dw, dh);
        reference.read_bgra8(BgraSliceU8::new(&small, (dw, dh)));
        let (cw, half) = (dw / 2, band / 2);
        let (u, v) = i420[dw * dh..].split_at(cw * (dh / 2));
        for (i, color) in colors.iter().enumerate() {
            // The middle of the band, away from the filtered edges.
            let (x, cx, y) = (i * half + half / 2, (i * half + half / 2) / 2, dh / 2);
            let got = [i420[y * dw + x], u[(y / 2) * cw + cx], v[(y / 2) * cw + cx]];
            let want = [
                reference.y()[y * dw + x],
                reference.u()[(y / 2) * reference.strides().1 + cx],
                reference.v()[(y / 2) * reference.strides().2 + cx],
            ];
            for (g, w) in got.iter().zip(want) {
                assert!(g.abs_diff(w) <= 3, "colour {color:?}: GPU {got:?}, OpenH264 {want:?}");
            }
        }
    }
}
