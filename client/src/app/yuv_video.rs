//! Drawing a screen share frame straight from its YUV planes: they go to the
//! GPU as three one-channel textures (1.5 bytes per pixel instead of 4) and a
//! shader converts them to colors. Before this, the decoder thread converted
//! every frame to RGBA on the CPU and the UI uploaded the full RGBA image.
//!
//! One renderer lives for the whole app and keeps its GL objects, so nothing
//! has to be freed from another thread. If the GL side cannot be set up (an
//! old OpenGL without the shader features used here), [`YuvRenderer::failed`]
//! turns true and the UI falls back to converting on the CPU.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use eframe::egui;
use eframe::egui_glow::{self, ShaderVersion};
use eframe::glow::{self, HasContext};

use crate::screen::YuvFrame;

/// Shared between the UI (which hands over frames) and the paint callback
/// (which runs on the GL thread when the frame is painted).
#[derive(Clone, Default)]
pub(super) struct YuvRenderer {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    /// The newest frame, not uploaded yet.
    pending: Mutex<Option<YuvFrame>>,
    gl: Mutex<Option<GlState>>,
    failed: AtomicBool,
    /// Time spent uploading frames, and how many, for the diagnostics line.
    upload_us: AtomicU64,
    uploads: AtomicU64,
}

struct GlState {
    program: glow::Program,
    vao: glow::VertexArray,
    /// Y, U, V.
    textures: [glow::Texture; 3],
    /// Sizes the textures were allocated with: (width, height) per plane.
    sizes: [(i32, i32); 3],
    u_tint: Option<glow::UniformLocation>,
}

impl YuvRenderer {
    /// The GL side could not be set up: convert frames on the CPU instead.
    pub(super) fn failed(&self) -> bool {
        self.inner.failed.load(Ordering::Relaxed)
    }

    /// Hands over a new frame; it is uploaded when the video is next painted.
    pub(super) fn set_frame(&self, frame: YuvFrame) {
        *self.inner.pending.lock().unwrap_or_else(|e| e.into_inner()) = Some(frame);
    }

    /// Drops a frame not shown yet (watching stopped).
    pub(super) fn clear(&self) {
        self.inner.pending.lock().unwrap_or_else(|e| e.into_inner()).take();
    }

    /// Total upload time in microseconds and number of uploads so far.
    pub(super) fn upload_stats(&self) -> (u64, u64) {
        (self.inner.upload_us.load(Ordering::Relaxed), self.inner.uploads.load(Ordering::Relaxed))
    }

    /// Paints the current frame into `rect`, colors multiplied by `tint`.
    pub(super) fn paint(&self, painter: &egui::Painter, rect: egui::Rect, tint: egui::Color32) {
        let inner = self.inner.clone();
        // Gamma space, like the vertex colors egui's own shader multiplies by.
        let tint = tint.to_normalized_gamma_f32();
        let callback = egui_glow::CallbackFn::new(move |_info, painter| {
            inner.paint(painter.gl(), tint);
        });
        painter.add(egui::PaintCallback { rect, callback: Arc::new(callback) });
    }
}

impl Inner {
    fn paint(&self, gl: &glow::Context, tint: [f32; 4]) {
        if self.failed.load(Ordering::Relaxed) {
            return;
        }
        let mut state = self.gl.lock().unwrap_or_else(|e| e.into_inner());
        if state.is_none() {
            // SAFETY: called from egui's paint callback, with its GL context current.
            match unsafe { GlState::new(gl) } {
                Ok(s) => *state = Some(s),
                Err(e) => {
                    tracing::warn!("GPU video drawing unavailable, converting on the CPU: {e}");
                    self.failed.store(true, Ordering::Relaxed);
                    return;
                }
            }
        }
        let Some(state) = state.as_mut() else { return };
        let frame = self.pending.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(frame) = frame {
            let t = Instant::now();
            // SAFETY: as above.
            unsafe { state.upload(gl, &frame) };
            self.upload_us.fetch_add(t.elapsed().as_micros() as u64, Ordering::Relaxed);
            self.uploads.fetch_add(1, Ordering::Relaxed);
        }
        // SAFETY: as above.
        unsafe { state.draw(gl, tint) };
    }
}

const VERTEX: &str = r#"
out vec2 v_uv;
void main() {
    // A full-viewport quad from the vertex index alone: no vertex buffer.
    vec2 pos = vec2(float(gl_VertexID & 1), float((gl_VertexID >> 1) & 1));
    v_uv = vec2(pos.x, 1.0 - pos.y);
    gl_Position = vec4(pos * 2.0 - 1.0, 0.0, 1.0);
}
"#;

// Limited-range BT.601 with OpenH264's coefficients, as `screen::bt601`.
// egui paints in gamma space, so the result goes out as is.
const FRAGMENT: &str = r#"
in vec2 v_uv;
uniform sampler2D u_y;
uniform sampler2D u_u;
uniform sampler2D u_v;
uniform vec4 u_tint;
out vec4 out_color;
void main() {
    float y = (texture(u_y, v_uv).r * 255.0 - 16.0) * (255.0 / 219.0);
    float u = texture(u_u, v_uv).r * 255.0 - 128.0;
    float v = texture(u_v, v_uv).r * 255.0 - 128.0;
    float k = 255.0 / 224.0;
    vec3 rgb = vec3(
        y + k * 1.402 * v,
        y - k * 1.402 * 0.299 / 0.587 * v - k * 1.772 * 0.114 / 0.587 * u,
        y + k * 1.772 * u
    );
    out_color = vec4(clamp(rgb / 255.0, 0.0, 1.0), 1.0) * u_tint;
}
"#;

impl GlState {
    unsafe fn new(gl: &glow::Context) -> Result<Self, String> {
        let version = ShaderVersion::get(gl);
        // Needs `in`/`out`, `texture()`, `gl_VertexID` and one-channel
        // textures: GLSL 1.40 / ES 3.00 and later.
        if !version.is_new_shader_interface() {
            return Err(format!("shader version {version:?} is too old"));
        }
        let header = match version {
            ShaderVersion::Es300 => "#version 300 es\nprecision mediump float;\n",
            _ => "#version 140\n",
        };
        unsafe {
            let program = gl.create_program()?;
            let mut shaders = Vec::new();
            for (kind, src) in [(glow::VERTEX_SHADER, VERTEX), (glow::FRAGMENT_SHADER, FRAGMENT)] {
                let shader = gl.create_shader(kind)?;
                gl.shader_source(shader, &format!("{header}{src}"));
                gl.compile_shader(shader);
                if !gl.get_shader_compile_status(shader) {
                    let log = gl.get_shader_info_log(shader);
                    gl.delete_shader(shader);
                    gl.delete_program(program);
                    return Err(format!("shader: {log}"));
                }
                gl.attach_shader(program, shader);
                shaders.push(shader);
            }
            gl.link_program(program);
            for s in shaders {
                gl.detach_shader(program, s);
                gl.delete_shader(s);
            }
            if !gl.get_program_link_status(program) {
                let log = gl.get_program_info_log(program);
                gl.delete_program(program);
                return Err(format!("program: {log}"));
            }
            gl.use_program(Some(program));
            for (i, name) in ["u_y", "u_u", "u_v"].into_iter().enumerate() {
                gl.uniform_1_i32(gl.get_uniform_location(program, name).as_ref(), i as i32);
            }
            let u_tint = gl.get_uniform_location(program, "u_tint");
            let vao = gl.create_vertex_array()?;
            let mut textures = Vec::new();
            for _ in 0..3 {
                let tex = gl.create_texture()?;
                gl.bind_texture(glow::TEXTURE_2D, Some(tex));
                for (param, value) in [
                    (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                    (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                    (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                    (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
                ] {
                    gl.tex_parameter_i32(glow::TEXTURE_2D, param, value as i32);
                }
                textures.push(tex);
            }
            let textures = [textures[0], textures[1], textures[2]];
            Ok(Self { program, vao, textures, sizes: [(0, 0); 3], u_tint })
        }
    }

    unsafe fn upload(&mut self, gl: &glow::Context, f: &YuvFrame) {
        let (cw, ch) = f.chroma_size();
        let planes = [(&f.y, f.strides.0, f.width, f.height), (&f.u, f.strides.1, cw, ch), (&f.v, f.strides.2, cw, ch)];
        unsafe {
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            for (i, (data, stride, w, h)) in planes.into_iter().enumerate() {
                if data.len() < stride * h.saturating_sub(1) + w {
                    tracing::warn!("screen share frame plane {i} too short");
                    continue;
                }
                let size = (w as i32, h as i32);
                gl.bind_texture(glow::TEXTURE_2D, Some(self.textures[i]));
                // Rows are `stride` bytes long; upload them without repacking.
                gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, stride as i32);
                let pixels = glow::PixelUnpackData::Slice(Some(data));
                if self.sizes[i] == size {
                    gl.tex_sub_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        0,
                        0,
                        size.0,
                        size.1,
                        glow::RED,
                        glow::UNSIGNED_BYTE,
                        pixels,
                    );
                } else {
                    gl.tex_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        glow::R8 as i32,
                        size.0,
                        size.1,
                        0,
                        glow::RED,
                        glow::UNSIGNED_BYTE,
                        pixels,
                    );
                    self.sizes[i] = size;
                }
            }
            gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, 0);
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
        }
    }

    unsafe fn draw(&self, gl: &glow::Context, tint: [f32; 4]) {
        if self.sizes[0] == (0, 0) {
            return;
        }
        unsafe {
            gl.use_program(Some(self.program));
            gl.uniform_4_f32_slice(self.u_tint.as_ref(), &tint);
            for (i, tex) in self.textures.iter().enumerate() {
                gl.active_texture(glow::TEXTURE0 + i as u32);
                gl.bind_texture(glow::TEXTURE_2D, Some(*tex));
            }
            gl.active_texture(glow::TEXTURE0);
            gl.bind_vertex_array(Some(self.vao));
            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            gl.bind_vertex_array(None);
        }
    }
}
