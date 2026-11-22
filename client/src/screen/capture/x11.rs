//! Capture on Linux under X11: a monitor (a part of the root window) or a
//! window (its offscreen contents through Composite, so it can be covered).
//!
//! X11 does not push frames: a capture thread grabs one per frame interval
//! through shared memory (MIT-SHM, plain GetImage if that is unavailable),
//! draws the pointer on it (XFixes) and skips frames identical to the last
//! one, so a static screen costs the encoder nothing, as on Windows.
//! Under Wayland the X server only sees X11 windows, so sharing is off there
//! (see `screen::can_share`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::composite::{ConnectionExt as _, Redirect};
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::shm::{self, ConnectionExt as _};
use x11rb::protocol::xfixes::ConnectionExt as _;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, Drawable, ImageFormat, Window};
use x11rb::rust_connection::RustConnection;

use super::{Capture, FrameSink};
use crate::screen::{Source, SourceId};

/// How often the window state (minimized or not) is checked.
const STATE_INTERVAL: Duration = Duration::from_millis(500);

/// Whether this session can be captured: a real X11 session, not Wayland
/// (where the X server only sees X11 apps and the desktop would be black).
pub fn session_supported() -> bool {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|t| t.eq_ignore_ascii_case("wayland"));
    std::env::var_os("DISPLAY").is_some() && !wayland
}

/// Monitors and windows available for sharing.
pub fn sources() -> Vec<Source> {
    match list_sources() {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("listing X11 sources: {e}");
            Vec::new()
        }
    }
}

fn list_sources() -> Result<Vec<Source>, String> {
    let (conn, screen) = x11rb::connect(None).map_err(|e| e.to_string())?;
    let root = conn.setup().roots[screen].root;
    let mut out = Vec::new();
    for (i, m) in monitors(&conn, root)?.into_iter().enumerate() {
        out.push(Source {
            id: SourceId::Monitor(i + 1), label: (i + 1).to_string(), meta: format!("{}×{}", m.2, m.3)
        });
    }
    let atoms = Atoms::new(&conn)?;
    let me = std::process::id();
    for w in prop32(&conn, root, atoms.client_list, AtomEnum::WINDOW.into()) {
        let types = prop32(&conn, w, atoms.window_type, AtomEnum::ATOM.into());
        if types.iter().any(|t| *t == atoms.type_dock || *t == atoms.type_desktop) {
            continue;
        }
        let Some(title) = window_title(&conn, &atoms, w) else { continue };
        let pid = prop32(&conn, w, atoms.pid, AtomEnum::CARDINAL.into()).first().copied();
        if title.trim().is_empty() || pid == Some(me) {
            continue;
        }
        let meta = pid.and_then(process_name).unwrap_or_default();
        out.push(Source { id: SourceId::Window(w as isize), label: title, meta });
    }
    Ok(out)
}

/// Monitors as (x, y, width, height) on the root window, primary first.
/// Without RandR: the whole root window as one monitor.
fn monitors(conn: &RustConnection, root: Window) -> Result<Vec<(i16, i16, u16, u16)>, String> {
    let from_randr = conn.randr_get_monitors(root, true).ok().and_then(|c| c.reply().ok()).map(|r| {
        let mut list = r.monitors;
        list.sort_by_key(|m| !m.primary);
        list.into_iter().map(|m| (m.x, m.y, m.width, m.height)).collect::<Vec<_>>()
    });
    match from_randr {
        Some(list) if !list.is_empty() => Ok(list),
        _ => {
            let g = conn.get_geometry(root).map_err(|e| e.to_string())?.reply().map_err(|e| e.to_string())?;
            Ok(vec![(0, 0, g.width, g.height)])
        }
    }
}

struct Atoms {
    client_list: u32,
    window_type: u32,
    type_dock: u32,
    type_desktop: u32,
    net_name: u32,
    utf8: u32,
    pid: u32,
    state: u32,
    hidden: u32,
}

impl Atoms {
    fn new(conn: &RustConnection) -> Result<Self, String> {
        let get = |name: &[u8]| -> Result<u32, String> {
            Ok(conn.intern_atom(false, name).map_err(|e| e.to_string())?.reply().map_err(|e| e.to_string())?.atom)
        };
        Ok(Self {
            client_list: get(b"_NET_CLIENT_LIST")?,
            window_type: get(b"_NET_WM_WINDOW_TYPE")?,
            type_dock: get(b"_NET_WM_WINDOW_TYPE_DOCK")?,
            type_desktop: get(b"_NET_WM_WINDOW_TYPE_DESKTOP")?,
            net_name: get(b"_NET_WM_NAME")?,
            utf8: get(b"UTF8_STRING")?,
            pid: get(b"_NET_WM_PID")?,
            state: get(b"_NET_WM_STATE")?,
            hidden: get(b"_NET_WM_STATE_HIDDEN")?,
        })
    }
}

/// A property of 32-bit values; empty if missing.
fn prop32(conn: &RustConnection, w: Window, prop: u32, kind: u32) -> Vec<u32> {
    conn.get_property(false, w, prop, kind, 0, 1024)
        .ok()
        .and_then(|c| c.reply().ok())
        .and_then(|r| r.value32().map(|v| v.collect()))
        .unwrap_or_default()
}

fn window_title(conn: &RustConnection, atoms: &Atoms, w: Window) -> Option<String> {
    let get = |prop: u32, kind: u32| {
        let r = conn.get_property(false, w, prop, kind, 0, 1024).ok()?.reply().ok()?;
        (!r.value.is_empty()).then(|| String::from_utf8_lossy(&r.value).into_owned())
    };
    get(atoms.net_name, atoms.utf8).or_else(|| get(AtomEnum::WM_NAME.into(), AtomEnum::STRING.into()))
}

/// The executable name of a process, as the Windows list shows it.
fn process_name(pid: u32) -> Option<String> {
    Some(std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?.trim().to_owned())
}

/// What is being captured.
#[derive(Clone, Copy)]
enum Target {
    /// A rectangle of the root window.
    Area {
        x: i16,
        y: i16,
        width: u16,
        height: u16,
    },
    Window(Window),
}

/// A running X11 capture. Dropping it stops the capture thread.
struct X11Capture {
    stop: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
    minimized: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    process: Option<u32>,
}

/// Starts capturing `source`.
pub(in crate::screen) fn start(source: SourceId, sink: FrameSink) -> Result<Box<dyn Capture>, String> {
    let (conn, screen) = x11rb::connect(None).map_err(|e| format!("connecting to X11: {e}"))?;
    let root = conn.setup().roots[screen].root;
    // Frames must be 32-bit BGRX for the encoder: true for any normal
    // 24/32-bit desktop. Others (16-bit) are refused up front.
    let depth = conn.setup().roots[screen].root_depth;
    if depth < 24 {
        return Err(format!("unsupported screen depth {depth}"));
    }
    let atoms = Atoms::new(&conn)?;
    let (target, process) = match source {
        // Picked through the portal, never here (see `capture::start`).
        SourceId::Portal => return Err("not an X11 source".into()),
        SourceId::Monitor(index) => {
            let list = monitors(&conn, root)?;
            let &(x, y, width, height) = list.get(index.wrapping_sub(1)).ok_or("monitor not found")?;
            (Target::Area { x, y, width, height }, None)
        }
        SourceId::Window(w) => {
            let w = w as Window;
            if conn.get_geometry(w).ok().and_then(|c| c.reply().ok()).is_none() {
                return Err("window already closed".into());
            }
            (Target::Window(w), prop32(&conn, w, atoms.pid, AtomEnum::CARDINAL.into()).first().copied())
        }
    };
    let mut grabber = Grabber::new(conn, root, atoms, target)?;
    let stop = Arc::new(AtomicBool::new(false));
    let finished = Arc::new(AtomicBool::new(false));
    let minimized = Arc::new(AtomicBool::new(false));
    let (st, fin, min) = (stop.clone(), finished.clone(), minimized.clone());
    let thread = std::thread::Builder::new()
        .name("screen-capture".into())
        .spawn(move || {
            if let Err(e) = grabber.run(&sink, &st, &min) {
                tracing::info!("screen capture ended: {e}");
                sink.closed();
            }
            fin.store(true, Ordering::Relaxed);
        })
        .map_err(|e| e.to_string())?;
    Ok(Box::new(X11Capture { stop, finished, minimized, thread: Some(thread), process }))
}

impl Capture for X11Capture {
    fn finished(&self) -> bool {
        self.finished.load(Ordering::Relaxed)
    }

    fn source_minimized(&self) -> bool {
        self.minimized.load(Ordering::Relaxed)
    }

    fn audio_process(&self) -> Option<u32> {
        self.process
    }
}

impl Drop for X11Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// A System V shared memory segment attached to the X server: GetImage
/// writes the frame straight into it instead of sending it over the socket.
struct Shm {
    seg: shm::Seg,
    addr: *mut u8,
    size: usize,
}

// SAFETY: the mapping belongs to this struct alone and is only used by the
// thread that owns the grabber; moving it to that thread is fine.
unsafe impl Send for Shm {}

impl Shm {
    fn new(conn: &RustConnection, size: usize) -> Result<Self, String> {
        // SAFETY: plain System V calls; the result is checked before use.
        unsafe {
            let id = libc::shmget(libc::IPC_PRIVATE, size, libc::IPC_CREAT | 0o600);
            if id < 0 {
                return Err("shmget failed".into());
            }
            let addr = libc::shmat(id, std::ptr::null(), 0);
            // Freed once both we and the X server have detached, even if we crash.
            libc::shmctl(id, libc::IPC_RMID, std::ptr::null_mut());
            if addr as isize == -1 {
                return Err("shmat failed".into());
            }
            let seg = conn.generate_id().map_err(|e| e.to_string())?;
            let attached = conn
                .shm_attach(seg, id as u32, false)
                .map_err(|e| e.to_string())
                .and_then(|c| c.check().map_err(|e| e.to_string()));
            if let Err(e) = attached {
                libc::shmdt(addr);
                return Err(e);
            }
            Ok(Self { seg, addr: addr as *mut u8, size })
        }
    }

    fn data(&self, len: usize) -> &[u8] {
        // SAFETY: `addr` is a live mapping of `size` bytes until drop.
        unsafe { std::slice::from_raw_parts(self.addr, len.min(self.size)) }
    }

    fn release(self, conn: &RustConnection) {
        let _ = conn.shm_detach(self.seg);
        let _ = conn.flush();
        // SAFETY: detaching our own mapping; nothing borrows it any more.
        unsafe { libc::shmdt(self.addr as *const _) };
    }
}

/// Grabs frames of one target on the capture thread.
struct Grabber {
    conn: RustConnection,
    root: Window,
    atoms: Atoms,
    target: Target,
    shm: Option<Shm>,
    /// MIT-SHM works with this server (local connection, extension present).
    use_shm: bool,
    cursor: bool,
    composite: bool,
    /// The frame being built and the last one sent, to skip unchanged frames.
    frame: Vec<u8>,
    sent: Vec<u8>,
    sent_size: (u16, u16),
}

impl Grabber {
    fn new(conn: RustConnection, root: Window, atoms: Atoms, target: Target) -> Result<Self, String> {
        let has = |name: &'static str| conn.extension_information(name).ok().flatten().is_some();
        let use_shm =
            has(shm::X11_EXTENSION_NAME) && conn.shm_query_version().ok().and_then(|c| c.reply().ok()).is_some();
        let cursor = has(x11rb::protocol::xfixes::X11_EXTENSION_NAME)
            && conn.xfixes_query_version(4, 0).ok().and_then(|c| c.reply().ok()).is_some();
        let composite = has(x11rb::protocol::composite::X11_EXTENSION_NAME)
            && conn.composite_query_version(0, 2).ok().and_then(|c| c.reply().ok()).is_some();
        if let Target::Window(w) = target
            && composite
        {
            // The window keeps an offscreen copy we can read even when covered.
            let _ = conn.composite_redirect_window(w, Redirect::AUTOMATIC);
        }
        Ok(Self {
            conn,
            root,
            atoms,
            target,
            shm: None,
            use_shm,
            cursor,
            composite,
            frame: Vec::new(),
            sent: Vec::new(),
            sent_size: (0, 0),
        })
    }

    fn run(&mut self, sink: &FrameSink, stop: &AtomicBool, minimized: &AtomicBool) -> Result<(), String> {
        let mut last = None;
        let mut state_at = Instant::now() - STATE_INTERVAL;
        let mut next = Instant::now();
        while !stop.load(Ordering::Relaxed) {
            next += sink.interval();
            let now = Instant::now();
            if next > now {
                std::thread::sleep(next - now);
            } else {
                // Fell behind (a slow grab): start counting from now.
                next = now;
            }
            if let Target::Window(w) = self.target
                && state_at.elapsed() >= STATE_INTERVAL
            {
                state_at = Instant::now();
                // A closed window ends the share.
                self.conn.get_geometry(w).map_err(|e| e.to_string())?.reply().map_err(|_| "window closed")?;
                let state = prop32(&self.conn, w, self.atoms.state, AtomEnum::ATOM.into());
                minimized.store(state.contains(&self.atoms.hidden), Ordering::Relaxed);
            }
            if minimized.load(Ordering::Relaxed) || !sink.take_now(&mut last) {
                continue;
            }
            let t = Instant::now();
            let Some((w, h)) = self.grab()? else { continue };
            // Unchanged since the last frame: nothing new for the encoder.
            if (w, h) == self.sent_size && self.frame == self.sent {
                continue;
            }
            sink.push(&self.frame, w as u32, h as u32, w as usize * 4, t)?;
            std::mem::swap(&mut self.frame, &mut self.sent);
            self.sent_size = (w, h);
        }
        if let Some(shm) = self.shm.take() {
            shm.release(&self.conn);
        }
        Ok(())
    }

    /// Grabs one frame into `self.frame` (tightly packed BGRX) and draws the
    /// pointer on it. `None`: nothing to grab right now (window unmapped).
    fn grab(&mut self) -> Result<Option<(u16, u16)>, String> {
        let (drawable, x, y, w, h, origin) = match self.target {
            Target::Area { x, y, width, height } => (self.root, x, y, width, height, (x, y)),
            Target::Window(win) => {
                let Ok(g) = self.conn.get_geometry(win).map_err(|e| e.to_string())?.reply() else {
                    return Err("window closed".into());
                };
                let origin = self
                    .conn
                    .translate_coordinates(win, self.root, 0, 0)
                    .ok()
                    .and_then(|c| c.reply().ok())
                    .map_or((0, 0), |r| (r.dst_x, r.dst_y));
                (win, 0, 0, g.width, g.height, origin)
            }
        };
        if w < 2 || h < 2 {
            return Ok(None);
        }
        // A covered window is read from its offscreen pixmap.
        let pixmap = match self.target {
            Target::Window(win) if self.composite => {
                let p = self.conn.generate_id().map_err(|e| e.to_string())?;
                let named = self.conn.composite_name_window_pixmap(win, p).map_err(|e| e.to_string())?.check();
                named.is_ok().then_some(p)
            }
            _ => None,
        };
        let source: Drawable = pixmap.unwrap_or(drawable);
        let got = self.read_image(source, x, y, w, h);
        if let Some(p) = pixmap {
            let _ = self.conn.free_pixmap(p);
        }
        if !got? {
            return Ok(None);
        }
        if self.cursor {
            self.draw_cursor(origin, w, h);
        }
        Ok(Some((w, h)))
    }

    /// Reads a w × h image at (x, y) of `drawable` into `self.frame`.
    /// `false` if the server refused (e.g. the window is not viewable).
    fn read_image(&mut self, drawable: Drawable, x: i16, y: i16, w: u16, h: u16) -> Result<bool, String> {
        let len = w as usize * h as usize * 4;
        if self.use_shm {
            if self.shm.as_ref().is_none_or(|s| s.size < len) {
                if let Some(old) = self.shm.take() {
                    old.release(&self.conn);
                }
                match Shm::new(&self.conn, len) {
                    Ok(s) => self.shm = Some(s),
                    Err(e) => {
                        // Remote X server or no SysV shm: plain GetImage from now on.
                        tracing::info!("X11 shared memory unavailable ({e}), using plain GetImage");
                        self.use_shm = false;
                    }
                }
            }
            if let Some(shm) = &self.shm {
                let reply = self
                    .conn
                    .shm_get_image(drawable, x, y, w, h, !0, ImageFormat::Z_PIXMAP.into(), shm.seg, 0)
                    .map_err(|e| e.to_string())?
                    .reply();
                if reply.is_err() {
                    return Ok(false);
                }
                self.frame.clear();
                self.frame.extend_from_slice(shm.data(len));
                return Ok(true);
            }
        }
        let Ok(reply) =
            self.conn.get_image(ImageFormat::Z_PIXMAP, drawable, x, y, w, h, !0).map_err(|e| e.to_string())?.reply()
        else {
            return Ok(false);
        };
        if reply.data.len() < len {
            return Ok(false);
        }
        self.frame.clear();
        self.frame.extend_from_slice(&reply.data[..len]);
        Ok(true)
    }

    /// Draws the pointer onto the frame; `origin`: the frame's top-left
    /// corner on the root window.
    fn draw_cursor(&mut self, origin: (i16, i16), w: u16, h: u16) {
        let Some(c) = self.conn.xfixes_get_cursor_image().ok().and_then(|c| c.reply().ok()) else { return };
        let left = i32::from(c.x) - i32::from(c.xhot) - i32::from(origin.0);
        let top = i32::from(c.y) - i32::from(c.yhot) - i32::from(origin.1);
        blend_cursor(
            &mut self.frame,
            w as usize,
            h as usize,
            &c.cursor_image,
            c.width as usize,
            c.height as usize,
            left,
            top,
        );
    }
}

/// Blends a premultiplied ARGB cursor image (`cw` × `ch`, top-left at
/// `left`, `top`) onto a BGRX frame, clipped to the frame.
#[allow(clippy::too_many_arguments)]
fn blend_cursor(frame: &mut [u8], w: usize, h: usize, cursor: &[u32], cw: usize, ch: usize, left: i32, top: i32) {
    for cy in 0..ch {
        let fy = top + cy as i32;
        if fy < 0 || fy >= h as i32 {
            continue;
        }
        for cx in 0..cw {
            let fx = left + cx as i32;
            if fx < 0 || fx >= w as i32 {
                continue;
            }
            let Some(&argb) = cursor.get(cy * cw + cx) else { return };
            let a = argb >> 24;
            if a == 0 {
                continue;
            }
            let px = &mut frame[(fy as usize * w + fx as usize) * 4..][..3];
            // Source is premultiplied: out = src + dst × (1 − a).
            for (i, shift) in [(0, 0), (1, 8), (2, 16)] {
                let s = (argb >> shift) & 0xff;
                px[i] = (s + u32::from(px[i]) * (255 - a) / 255).min(255) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_blends_and_clips() {
        // A 2×2 gray frame; a 2×2 cursor half off the left edge: an opaque
        // white pixel and a half-transparent black one land on column 0.
        let mut frame = vec![100u8; 2 * 2 * 4];
        let white = 0xffff_ffff;
        let half_black = 0x8000_0000;
        let cursor = [0, white, 0, half_black];
        blend_cursor(&mut frame, 2, 2, &cursor, 2, 2, -1, 0);
        assert_eq!(&frame[0..3], &[255, 255, 255]);
        // 100 × (255 − 128) / 255 = 49.
        assert_eq!(&frame[8..11], &[49, 49, 49]);
        // Column 1 untouched.
        assert_eq!(&frame[4..7], &[100, 100, 100]);
    }

    /// Lists sources and captures the first monitor through the whole
    /// sharer: needs an X server (`DISPLAY`), so it does not run by default.
    /// In WSL: `Xvfb :99 &` then `DISPLAY=:99 WAYLAND_DISPLAY= cargo test x11 -- --ignored`.
    #[test]
    #[ignore = "needs an X server"]
    fn x11_capture() {
        let list = sources();
        println!("sources: {:?}", list.iter().map(|s| (&s.label, &s.meta)).collect::<Vec<_>>());
        assert!(list.iter().any(|s| matches!(s.id, SourceId::Monitor(1))), "no monitor");
        let (tx, mut rx) = tokio::sync::mpsc::channel(4096);
        let sharer = crate::screen::Sharer::start(
            SourceId::Monitor(1),
            crate::screen::Quality::P720F30,
            false,
            tx,
            Default::default(),
            eframe::egui::Context::default(),
        )
        .unwrap();
        // The first frame always differs from "nothing sent yet".
        let deadline = Instant::now() + Duration::from_secs(5);
        while rx.try_recv().is_err() {
            assert!(Instant::now() < deadline, "no frame captured");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!sharer.ended());
        let windows: Vec<_> = list.iter().filter(|s| matches!(s.id, SourceId::Window(_))).collect();
        if let Some(win) = windows.first() {
            drop(sharer);
            let (tx, mut rx) = tokio::sync::mpsc::channel(4096);
            let sharer = crate::screen::Sharer::start(
                win.id,
                crate::screen::Quality::P720F30,
                false,
                tx,
                Default::default(),
                eframe::egui::Context::default(),
            )
            .unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while rx.try_recv().is_err() {
                assert!(Instant::now() < deadline, "no frame of window {:?}", win.label);
                std::thread::sleep(Duration::from_millis(20));
            }
            assert!(!sharer.ended());
            println!("window {:?} captured", win.label);
        }
    }
}
