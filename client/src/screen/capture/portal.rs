//! Capture on Linux under Wayland: the desktop portal (xdg-desktop-portal,
//! ScreenCast) asks the user what to share in a system dialog and hands over
//! a PipeWire stream of it.
//!
//! Everything runs on the capture thread, so the UI does not freeze while the
//! system dialog is open. Cancelling the dialog ends the capture, which ends
//! the share like a closed window does. The compositor sends frames only
//! when the picture changes, with the pointer drawn in.

use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use ashpd::desktop::PersistMode;
use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
use pipewire as pw;
use pw::spa;
use pw::spa::param::video::{VideoFormat, VideoInfoRaw};
use pw::spa::pod::Pod;

use super::{Capture, FrameSink};
use crate::screen::{Source, SourceId};

/// Whether this is a Wayland session, where the portal is the way to capture.
pub fn session_supported() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|t| t.eq_ignore_ascii_case("wayland"))
}

/// The system dialog picks the monitor or window: one entry for it.
pub fn sources() -> Vec<Source> {
    vec![Source { id: SourceId::Portal, label: String::new(), meta: String::new() }]
}

/// A running portal capture. Dropping it stops the capture thread.
struct PortalCapture {
    stop: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
    /// Quits the PipeWire loop once streaming; `None` until then.
    quit: Arc<Mutex<Option<pw::channel::Sender<()>>>>,
}

/// Starts the portal handshake and, once the user picked something, the
/// PipeWire stream, all on a new thread.
pub(in crate::screen) fn start(sink: FrameSink) -> Result<Box<dyn Capture>, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let finished = Arc::new(AtomicBool::new(false));
    let quit = Arc::new(Mutex::new(None));
    let (st, fin, q) = (stop.clone(), finished.clone(), quit.clone());
    std::thread::Builder::new()
        .name("screen-capture".into())
        .spawn(move || {
            if let Err(e) = run(&sink, &st, &q)
                && !st.load(Ordering::Relaxed)
            {
                tracing::info!("screen capture ended: {e}");
                sink.closed();
            }
            fin.store(true, Ordering::Relaxed);
        })
        .map_err(|e| e.to_string())?;
    Ok(Box::new(PortalCapture { stop, finished, quit }))
}

impl Capture for PortalCapture {
    fn finished(&self) -> bool {
        self.finished.load(Ordering::Relaxed)
    }

    /// The portal does not tell.
    fn source_minimized(&self) -> bool {
        false
    }

    /// The portal does not say which app was picked: all audio but ours.
    fn audio_process(&self) -> Option<u32> {
        None
    }
}

impl Drop for PortalCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(quit) = self.quit.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = quit.send(());
        }
        // Not joined: while the system dialog is open the thread waits for the
        // user. It sees `stop` afterwards and exits on its own.
    }
}

/// The portal handshake: the system dialog, then the stream's PipeWire node
/// and a connection to it.
struct Picked {
    fd: OwnedFd,
    node: u32,
    proxy: Screencast,
    session: ashpd::desktop::Session<Screencast>,
}

async fn pick() -> ashpd::Result<Picked> {
    let proxy = Screencast::new().await?;
    let session = proxy.create_session(Default::default()).await?;
    let modes = proxy.available_cursor_modes().await.unwrap_or_default();
    let cursor = if modes.contains(CursorMode::Embedded) { CursorMode::Embedded } else { CursorMode::Hidden };
    proxy
        .select_sources(
            &session,
            SelectSourcesOptions::default()
                .set_cursor_mode(cursor)
                .set_sources(SourceType::Monitor | SourceType::Window)
                .set_multiple(false)
                .set_persist_mode(PersistMode::DoNot),
        )
        .await?
        .response()?;
    let streams = proxy.start(&session, None, Default::default()).await?.response()?;
    let node = streams.streams().first().map(|s| s.pipe_wire_node_id()).ok_or(ashpd::Error::NoResponse)?;
    let fd = proxy.open_pipe_wire_remote(&session, Default::default()).await?;
    Ok(Picked { fd, node, proxy, session })
}

/// One runtime for all portal calls, alive as long as the app. ashpd keeps a
/// single D-Bus connection for the whole process, and its background tasks
/// run on the runtime it was opened on. With a runtime per share, the first
/// share's runtime took the connection's tasks with it when it ended, and the
/// second share hung forever waiting on the dead connection.
fn portal_runtime() -> Result<&'static tokio::runtime::Runtime, String> {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    if let Some(rt) = RUNTIME.get() {
        return Ok(rt);
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .thread_name("portal")
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    Ok(RUNTIME.get_or_init(|| rt))
}

fn run(sink: &FrameSink, stop: &AtomicBool, quit: &Mutex<Option<pw::channel::Sender<()>>>) -> Result<(), String> {
    let rt = portal_runtime()?;
    let picked = rt.block_on(pick()).map_err(|e| format!("portal: {e}"))?;
    if stop.load(Ordering::Relaxed) {
        let _ = rt.block_on(picked.session.close());
        return Ok(());
    }
    let result = stream(sink, picked.fd, picked.node, quit);
    let _ = rt.block_on(picked.session.close());
    drop(picked.proxy);
    result
}

/// What the PipeWire callbacks share.
struct StreamData {
    sink: FrameSink,
    format: VideoInfoRaw,
    last: Option<Instant>,
    /// A frame with red and blue swapped, for RGB-ordered formats.
    swapped: Vec<u8>,
    /// Why the stream stopped, if it failed.
    error: Option<String>,
}

/// Receives the PipeWire stream until it ends or `quit` is signalled.
fn stream(
    sink: &FrameSink,
    fd: OwnedFd,
    node: u32,
    quit: &Mutex<Option<pw::channel::Sender<()>>>,
) -> Result<(), String> {
    let err = |e: pw::Error| e.to_string();
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(err)?;
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(err)?;
    let core = context.connect_fd_rc(fd, None).map_err(err)?;
    let stream = pw::stream::StreamBox::new(
        &core,
        "chumly-screen",
        pw::properties::properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )
    .map_err(err)?;

    let data = StreamData {
        sink: sink.clone(),
        format: VideoInfoRaw::default(),
        last: None,
        swapped: Vec::new(),
        error: None,
    };
    let ml_state = mainloop.clone();
    let _listener = stream
        .add_local_listener_with_user_data(data)
        .state_changed(move |_, data, old, new| match new {
            pw::stream::StreamState::Error(e) => {
                data.error = Some(e);
                ml_state.quit();
            }
            // The compositor ended the stream: the window was closed or
            // the user stopped sharing from the system menu.
            pw::stream::StreamState::Unconnected if !matches!(old, pw::stream::StreamState::Unconnected) => {
                data.error = Some("the stream was closed".into());
                ml_state.quit();
            }
            _ => {}
        })
        .param_changed(|_, data, id, param| {
            let Some(param) = param else { return };
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            if data.format.parse(param).is_err() {
                tracing::warn!("screen capture: unreadable video format");
            }
        })
        .process(|stream, data| {
            let Some(mut buffer) = stream.dequeue_buffer() else { return };
            if !data.sink.take_now(&mut data.last) {
                return;
            }
            let t = Instant::now();
            let size = data.format.size();
            let (w, h) = (size.width as usize, size.height as usize);
            let rgb_order = match data.format.format() {
                VideoFormat::BGRx | VideoFormat::BGRA => false,
                VideoFormat::RGBx | VideoFormat::RGBA => true,
                _ => return,
            };
            let datas = buffer.datas_mut();
            let Some(d) = datas.first_mut() else { return };
            let chunk = d.chunk();
            let (offset, stride) = (chunk.offset() as usize, chunk.stride());
            let pitch = if stride > 0 { stride as usize } else { w * 4 };
            if w < 2 || h < 2 || chunk.size() == 0 {
                return;
            }
            let Some(bytes) = d.data() else { return };
            let Some(frame) = bytes.get(offset..offset + pitch * (h - 1) + w * 4) else { return };
            let result = if rgb_order {
                data.swapped.clear();
                for row in 0..h {
                    for px in frame[row * pitch..][..w * 4].as_chunks::<4>().0 {
                        data.swapped.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
                    }
                }
                data.sink.push(&data.swapped, w as u32, h as u32, w * 4, t)
            } else {
                data.sink.push(frame, w as u32, h as u32, pitch, t)
            };
            if let Err(e) = result {
                tracing::debug!("screen capture: {e}");
            }
        })
        .register()
        .map_err(err)?;

    let params = format_params(sink)?;
    let mut params = [Pod::from_bytes(&params).ok_or("bad format parameters")?];
    stream
        .connect(
            spa::utils::Direction::Input,
            Some(node),
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .map_err(err)?;

    let (tx, rx) = pw::channel::channel::<()>();
    let ml_quit = mainloop.clone();
    let _quit = rx.attach(mainloop.loop_(), move |()| ml_quit.quit());
    *quit.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);

    mainloop.run();
    Ok(())
}

/// The formats we accept: 32-bit RGB in either byte order, any size, up to
/// the stream's frame rate.
fn format_params(sink: &FrameSink) -> Result<Vec<u8>, String> {
    use spa::param::format::{FormatProperties, MediaSubtype, MediaType};
    let fps = (1.0 / sink.interval().as_secs_f64()).round() as u32;
    let obj = spa::pod::object!(
        spa::utils::SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        spa::pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
        spa::pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
        spa::pod::property!(
            FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::BGRx,
            VideoFormat::BGRx,
            VideoFormat::BGRA,
            VideoFormat::RGBx,
            VideoFormat::RGBA
        ),
        spa::pod::property!(
            FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            spa::utils::Rectangle { width: 1920, height: 1080 },
            spa::utils::Rectangle { width: 1, height: 1 },
            spa::utils::Rectangle { width: 8192, height: 8192 }
        ),
        spa::pod::property!(
            FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            spa::utils::Fraction { num: fps, denom: 1 },
            spa::utils::Fraction { num: 0, denom: 1 },
            spa::utils::Fraction { num: fps, denom: 1 }
        ),
    );
    let bytes =
        spa::pod::serialize::PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &spa::pod::Value::Object(obj))
            .map_err(|e| format!("{e:?}"))?
            .0
            .into_inner();
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use super::*;

    /// The format offer we send PipeWire is a well-formed object: a broken
    /// one would only show up on a real Wayland desktop as a refused stream.
    #[test]
    fn format_offer_is_valid() {
        let (tx, _rx) = std::sync::mpsc::sync_channel(1);
        let flag = || Arc::new(AtomicBool::new(false));
        let sink = FrameSink::new(
            tx,
            flag(),
            flag(),
            eframe::egui::Context::default(),
            Default::default(),
            crate::screen::Quality::P1080F60,
        );
        let bytes = format_params(&sink).unwrap();
        let pod = Pod::from_bytes(&bytes).expect("parses as a pod");
        assert!(pod.is_object());
        let (_, value) = spa::pod::deserialize::PodDeserializer::deserialize_any_from(&bytes).unwrap();
        let spa::pod::Value::Object(obj) = value else { panic!("not an object") };
        // Media type, subtype, format, size and frame rate.
        assert_eq!(obj.properties.len(), 5);
    }

    /// Two shares one after another, each from its own capture thread, both
    /// reach the portal. A runtime per share made the second one hang (the
    /// shared D-Bus connection died with the first share's runtime). Needs a
    /// D-Bus session bus (WSL has one); without a portal service the calls
    /// fail fast instead, which still shows they did not hang.
    #[test]
    #[ignore = "needs a D-Bus session bus"]
    fn portal_works_for_a_second_share() {
        for share in 1..=2 {
            let result = std::thread::spawn(|| {
                portal_runtime().unwrap().block_on(async {
                    tokio::time::timeout(std::time::Duration::from_secs(5), Screencast::new()).await.map(|r| r.is_ok())
                })
            })
            .join()
            .unwrap();
            assert!(result.is_ok(), "share {share}: the portal call hung");
        }
    }
}
