//! Capture on Windows: Windows Graphics Capture, for a monitor or a window.

use std::time::Instant;

use windows_capture::capture::{CaptureControl, Context, GraphicsCaptureApiHandler};
use windows_capture::frame::Frame;
use windows_capture::graphics_capture_api::InternalCaptureControl;
use windows_capture::monitor::Monitor;
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings, GraphicsCaptureItemType,
    MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
};
use windows_capture::window::Window;

use super::{Capture, FrameSink};
use crate::screen::{Source, SourceId};

/// Monitors and windows available for sharing.
pub fn sources() -> Vec<Source> {
    let mut out = Vec::new();
    if let Ok(monitors) = Monitor::enumerate() {
        for (i, m) in monitors.iter().enumerate() {
            let meta = match (m.width(), m.height()) {
                (Ok(w), Ok(h)) => format!("{w}×{h}"),
                _ => String::new(),
            };
            out.push(Source { id: SourceId::Monitor(i + 1), label: (i + 1).to_string(), meta });
        }
    }
    let me = std::process::id();
    if let Ok(windows) = Window::enumerate() {
        for w in windows {
            let Ok(title) = w.title() else { continue };
            if title.trim().is_empty() || w.process_id().is_ok_and(|p| p == me) {
                continue;
            }
            let meta = w.process_name().unwrap_or_default();
            out.push(Source { id: SourceId::Window(w.as_raw_hwnd() as isize), label: title, meta });
        }
    }
    out
}

struct Handler {
    sink: FrameSink,
    last: Option<Instant>,
}

impl GraphicsCaptureApiHandler for Handler {
    type Flags = FrameSink;
    type Error = String;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        Ok(Self { sink: ctx.flags, last: None })
    }

    fn on_frame_arrived(&mut self, frame: &mut Frame, _control: InternalCaptureControl) -> Result<(), Self::Error> {
        if !self.sink.take_now(&mut self.last) {
            return Ok(());
        }
        let t = Instant::now();
        let mut buf = frame.buffer().map_err(|e| e.to_string())?;
        let (sw, sh, pitch) = (buf.width(), buf.height(), buf.row_pitch() as usize);
        self.sink.push(buf.as_raw_buffer(), sw, sh, pitch, t)
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        self.sink.closed();
        Ok(())
    }
}

type Control = CaptureControl<Handler, String>;

fn start_capture<T>(item: T, sink: FrameSink) -> Result<Control, String>
where
    T: TryInto<GraphicsCaptureItemType> + Send + Copy + 'static,
{
    // No yellow border and a frame rate cap. Only available on
    // newer Windows builds; older ones start with default settings.
    let tuned = Settings::new(
        item,
        CursorCaptureSettings::WithCursor,
        DrawBorderSettings::WithoutBorder,
        SecondaryWindowSettings::Default,
        MinimumUpdateIntervalSettings::Custom(sink.interval()),
        DirtyRegionSettings::Default,
        ColorFormat::Bgra8,
        sink.clone(),
    );
    match Handler::start_free_threaded(tuned) {
        Ok(c) => Ok(c),
        Err(first) => {
            tracing::debug!("capture with settings failed ({first}), retrying with defaults");
            let plain = Settings::new(
                item,
                CursorCaptureSettings::Default,
                DrawBorderSettings::Default,
                SecondaryWindowSettings::Default,
                MinimumUpdateIntervalSettings::Default,
                DirtyRegionSettings::Default,
                ColorFormat::Bgra8,
                sink,
            );
            Handler::start_free_threaded(plain).map_err(|e| e.to_string())
        }
    }
}

/// A running Windows capture. Dropping it stops capturing.
struct WinCapture {
    control: Option<Control>,
    /// The shared window; `None` for a monitor.
    hwnd: Option<isize>,
    /// The shared window's process, for its audio.
    process: Option<u32>,
}

/// Starts capturing `source`.
pub(in crate::screen) fn start(source: SourceId, sink: FrameSink) -> Result<Box<dyn Capture>, String> {
    let (control, hwnd, process) = match source {
        // Only Wayland picks through a system dialog.
        SourceId::Portal => return Err("not available on Windows".into()),
        SourceId::Monitor(index) => {
            let m = Monitor::from_index(index).map_err(|e| e.to_string())?;
            (start_capture(m, sink)?, None, None)
        }
        SourceId::Window(hwnd) => {
            let w = Window::from_raw_hwnd(hwnd as *mut std::ffi::c_void);
            if !w.is_valid() {
                return Err("window already closed".into());
            }
            let process = w.process_id().inspect_err(|e| tracing::warn!("failed to get window process: {e}")).ok();
            (start_capture(w, sink)?, Some(hwnd), process)
        }
    };
    Ok(Box::new(WinCapture { control: Some(control), hwnd, process }))
}

impl Capture for WinCapture {
    fn finished(&self) -> bool {
        self.control.as_ref().is_none_or(|c| c.is_finished())
    }

    fn source_minimized(&self) -> bool {
        // SAFETY: IsIconic only reads the window state; a stale handle gives 0.
        self.hwnd.is_some_and(|h| unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsIconic(h as _) } != 0)
    }

    fn audio_process(&self) -> Option<u32> {
        self.process
    }
}

impl Drop for WinCapture {
    fn drop(&mut self) {
        if let Some(c) = self.control.take()
            && let Err(e) = c.stop()
        {
            tracing::debug!("stopping capture: {e}");
        }
    }
}
