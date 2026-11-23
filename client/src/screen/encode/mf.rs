//! Hardware H.264 encoding on Windows through Media Foundation. The graphics
//! card's driver registers its encoder there (NVIDIA NVENC, AMD AMF, Intel
//! Quick Sync), so one code path covers them all. Input: NV12 frames from the
//! GPU frame preparation (`capture/gpu.rs`).
//!
//! The stream must stay decodable by every viewer's OpenH264 decoder:
//! baseline profile, no B-frames, SPS/PPS before every keyframe (added here
//! when the encoder leaves them out).

use std::mem::ManuallyDrop;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{E_NOTIMPL, VARIANT_TRUE};
use windows::Win32::Media::MediaFoundation::{
    CODECAPI_AVEncCommonMeanBitRate, CODECAPI_AVEncCommonRateControlMode, CODECAPI_AVEncMPVDefaultBPictureCount,
    CODECAPI_AVEncMPVGOPSize, CODECAPI_AVEncVideoForceKeyFrame, CODECAPI_AVLowLatencyMode, ICodecAPI, IMFActivate,
    IMFMediaEventGenerator, IMFSample, IMFShutdown, IMFTransform, METransformHaveOutput, METransformNeedInput,
    MF_E_NO_EVENTS_AVAILABLE, MF_E_NOTACCEPTING, MF_E_TRANSFORM_NEED_MORE_INPUT, MF_E_TRANSFORM_STREAM_CHANGE,
    MF_EVENT_FLAG_NO_WAIT, MF_LOW_LATENCY, MF_MT_AVG_BITRATE, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE,
    MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_MPEG2_PROFILE, MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE,
    MF_TRANSFORM_ASYNC, MF_TRANSFORM_ASYNC_UNLOCK, MF_VERSION, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample,
    MFMediaType_Video, MFSTARTUP_LITE, MFStartup, MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_FLAG, MFT_ENUM_FLAG_HARDWARE,
    MFT_ENUM_FLAG_SORTANDFILTER, MFT_FRIENDLY_NAME_Attribute, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING,
    MFT_MESSAGE_NOTIFY_END_STREAMING, MFT_MESSAGE_NOTIFY_START_OF_STREAM, MFT_OUTPUT_DATA_BUFFER,
    MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES, MFT_OUTPUT_STREAM_PROVIDES_SAMPLES, MFT_REGISTER_TYPE_INFO, MFTEnumEx,
    MFVideoFormat_H264, MFVideoFormat_NV12, MFVideoInterlace_Progressive, eAVEncCommonRateControlMode_CBR,
    eAVEncH264VProfile_Base,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize};
use windows::Win32::System::Variant::{VARIANT, VT_BOOL, VT_UI4};
use windows::core::{GUID, Interface, PWSTR};

use crate::screen::Quality;

/// How long to wait for the encoder's output after handing it a frame,
/// when it already asks for the next one (it keeps frames in flight).
const OUTPUT_WAIT: Duration = Duration::from_millis(30);

/// The encoder thread's hardware encoder: opened for the stream's frame size
/// on first use, reopened when the size changes (a resized window). Once it
/// fails, the share continues with OpenH264.
pub(super) struct Hardware {
    on: bool,
    quality: Quality,
    encoder: Option<MfEncoder>,
}

impl Hardware {
    /// `CHUMLY_SCREEN_SOFT_ENCODER=1` keeps OpenH264 (for comparisons).
    pub(super) fn new(on: bool, quality: Quality) -> Self {
        let on = on && std::env::var_os("CHUMLY_SCREEN_SOFT_ENCODER").is_none();
        Self { on, quality, encoder: None }
    }

    /// Encodes an NV12 frame, or returns `None`: then the caller encodes it
    /// with OpenH264 (hardware encoding is off, unavailable or failed). A
    /// fresh OpenH264 encoder starts with a keyframe, so viewers switch over
    /// without waiting.
    pub(super) fn encode(
        &mut self,
        nv12: &[u8],
        width: usize,
        height: usize,
        keyframe: bool,
        counters: &crate::screen::Counters,
    ) -> Option<Vec<Encoded>> {
        if !self.on {
            return None;
        }
        if self.encoder.as_ref().is_none_or(|e| e.size() != (width, height)) {
            self.encoder = None;
            match MfEncoder::new(width, height, self.quality) {
                Ok(e) => {
                    tracing::info!("screen share: hardware encoder {} ({width}×{height})", e.name);
                    counters.set_encoder(&e.name);
                    self.encoder = Some(e);
                }
                Err(e) => {
                    tracing::warn!("screen share: no hardware encoder, using OpenH264: {e}");
                    self.fall_back(counters);
                    return None;
                }
            }
        }
        let encoder = self.encoder.as_mut()?;
        match encoder.encode(nv12, keyframe) {
            Ok(frames) => Some(frames),
            Err(e) => {
                tracing::warn!("screen share: hardware encoder {} failed, using OpenH264: {e}", encoder.name);
                self.fall_back(counters);
                None
            }
        }
    }

    fn fall_back(&mut self, counters: &crate::screen::Counters) {
        self.on = false;
        self.encoder = None;
        counters.set_encoder("OpenH264");
    }
}

/// One encoded frame: Annex B H.264 and whether it is a keyframe.
pub(super) struct Encoded {
    pub(super) data: Vec<u8>,
    pub(super) keyframe: bool,
}

/// A hardware H.264 encoder for one frame size. Lives on the encoder thread.
pub(super) struct MfEncoder {
    transform: IMFTransform,
    /// Where `transform` came from: it keeps the encoder until `ShutdownObject`.
    activate: IMFActivate,
    /// Hardware encoders work asynchronously and say through events when
    /// they want input or have output. `None`: a synchronous one.
    events: Option<IMFMediaEventGenerator>,
    codec: Option<ICodecAPI>,
    input_id: u32,
    output_id: u32,
    /// The encoder allocates its output samples; otherwise we do, `output_size` bytes.
    provides_samples: bool,
    output_size: u32,
    width: usize,
    height: usize,
    fps: u32,
    frame: u64,
    /// How many inputs the encoder has asked for and not yet received.
    wants_input: u32,
    /// SPS and PPS of the last keyframe that had them.
    parameter_sets: Vec<u8>,
    /// The encoder's name, for the log ("NVIDIA H.264 Encoder MFT").
    pub(super) name: String,
    /// Declared last: COM and Media Foundation stop after the encoder is released.
    _session: Session,
}

impl MfEncoder {
    /// Opens the first hardware H.264 encoder that accepts these settings.
    pub(super) fn new(width: usize, height: usize, quality: Quality) -> Result<Self, String> {
        // SAFETY: COM and Media Foundation start-up on this thread for the
        // listing; each encoder holds its own session (they are counted).
        let _session = unsafe { Session::start()? };
        let activates = unsafe { hardware_encoders()? };
        if activates.is_empty() {
            return Err("no hardware H.264 encoder".into());
        }
        let mut errors = Vec::new();
        for activate in &activates {
            // SAFETY: plain Media Foundation calls on objects we own.
            let name = unsafe { friendly_name(activate) };
            match unsafe { Self::open(activate, width, height, quality, name.clone()) } {
                Ok(enc) => return Ok(enc),
                Err(e) => errors.push(format!("{name}: {e}")),
            }
        }
        Err(errors.join("; "))
    }

    unsafe fn open(
        activate: &IMFActivate,
        width: usize,
        height: usize,
        quality: Quality,
        name: String,
    ) -> Result<Self, String> {
        fn e(what: &'static str) -> impl Fn(windows::core::Error) -> String {
            move |err| format!("{what}: {err}")
        }
        let fps = quality.fps();
        let bitrate = quality.bitrate();
        // SAFETY (whole body): Media Foundation calls with valid arguments on
        // objects created here.
        unsafe {
            let session = Session::start()?;
            let transform: IMFTransform = activate.ActivateObject().map_err(e("activate"))?;
            let attrs = transform.GetAttributes().ok();
            let is_async = attrs.as_ref().is_some_and(|a| a.GetUINT32(&MF_TRANSFORM_ASYNC).unwrap_or(0) != 0);
            if let Some(a) = &attrs {
                if is_async {
                    a.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1).map_err(e("unlock"))?;
                }
                let _ = a.SetUINT32(&MF_LOW_LATENCY, 1);
            }
            let (mut input_id, mut output_id) = (0u32, 0u32);
            if let Err(err) =
                transform.GetStreamIDs(std::slice::from_mut(&mut input_id), std::slice::from_mut(&mut output_id))
                && err.code() != E_NOTIMPL
            {
                return Err(format!("stream ids: {err}"));
            }
            let codec: Option<ICodecAPI> = transform.cast().ok();
            if let Some(c) = &codec {
                // Best effort: not every encoder supports every setting.
                set_u32(c, &CODECAPI_AVEncCommonRateControlMode, eAVEncCommonRateControlMode_CBR.0 as u32);
                set_u32(c, &CODECAPI_AVEncCommonMeanBitRate, bitrate);
                // A keyframe every 10 seconds just in case; normally viewers request them.
                set_u32(c, &CODECAPI_AVEncMPVGOPSize, fps * 10);
                set_u32(c, &CODECAPI_AVEncMPVDefaultBPictureCount, 0);
                set_bool(c, &CODECAPI_AVLowLatencyMode);
            }

            let size = (width as u64) << 32 | height as u64;
            let rate = (fps as u64) << 32 | 1;
            let out = MFCreateMediaType().map_err(e("media type"))?;
            out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(e("output type"))?;
            out.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264).map_err(e("output type"))?;
            out.SetUINT32(&MF_MT_AVG_BITRATE, bitrate).map_err(e("output type"))?;
            out.SetUINT64(&MF_MT_FRAME_SIZE, size).map_err(e("output type"))?;
            out.SetUINT64(&MF_MT_FRAME_RATE, rate).map_err(e("output type"))?;
            out.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, 1 << 32 | 1).map_err(e("output type"))?;
            out.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32).map_err(e("output type"))?;
            out.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Base.0 as u32).map_err(e("output type"))?;
            transform.SetOutputType(output_id, &out, 0).map_err(e("set output type"))?;

            let mut index = 0;
            loop {
                let t = transform.GetInputAvailableType(input_id, index).map_err(e("no NV12 input"))?;
                index += 1;
                if t.GetGUID(&MF_MT_SUBTYPE).ok() != Some(MFVideoFormat_NV12) {
                    continue;
                }
                t.SetUINT64(&MF_MT_FRAME_SIZE, size).map_err(e("input type"))?;
                t.SetUINT64(&MF_MT_FRAME_RATE, rate).map_err(e("input type"))?;
                t.SetUINT32(&MF_MT_DEFAULT_STRIDE, width as u32).map_err(e("input type"))?;
                t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32).map_err(e("input type"))?;
                transform.SetInputType(input_id, &t, 0).map_err(e("set input type"))?;
                break;
            }

            let info = transform.GetOutputStreamInfo(output_id).map_err(e("output info"))?;
            let provides = (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0) as u32;
            let events = if is_async { Some(transform.cast().map_err(e("events"))?) } else { None };
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0).map_err(e("begin streaming"))?;
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0).map_err(e("start of stream"))?;
            Ok(Self {
                transform,
                activate: activate.clone(),
                events,
                codec,
                input_id,
                output_id,
                provides_samples: info.dwFlags & provides != 0,
                output_size: info.cbSize.max(1 << 20),
                width,
                height,
                fps,
                frame: 0,
                wants_input: 0,
                parameter_sets: Vec::new(),
                name,
                _session: session,
            })
        }
    }

    pub(super) fn size(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    /// Encodes one NV12 frame (tightly packed, this encoder's size). Returns
    /// the frames that came out: usually this one; an encoder that keeps
    /// frames in flight may return it with the next call.
    pub(super) fn encode(&mut self, nv12: &[u8], keyframe: bool) -> Result<Vec<Encoded>, String> {
        let mut out = Vec::new();
        // SAFETY: Media Foundation calls on objects owned by this encoder,
        // on the thread that created it.
        unsafe {
            if keyframe && let Some(c) = &self.codec {
                set_u32(c, &CODECAPI_AVEncVideoForceKeyFrame, 1);
            }
            let sample = self.sample(nv12)?;
            if self.events.is_some() {
                self.encode_async(&sample, &mut out)?;
            } else {
                self.encode_sync(&sample, &mut out)?;
            }
        }
        self.frame += 1;
        Ok(out)
    }

    unsafe fn encode_async(&mut self, sample: &IMFSample, out: &mut Vec<Encoded>) -> Result<(), String> {
        // SAFETY: see `encode`.
        unsafe {
            // Whatever happened since the last frame (late output, input requests).
            while self.next_event(false, out)? {}
            while self.wants_input == 0 {
                self.next_event(true, out)?;
            }
            self.wants_input -= 1;
            let before = out.len();
            self.transform.ProcessInput(self.input_id, sample, 0).map_err(|e| format!("input: {e}"))?;
            let deadline = Instant::now() + OUTPUT_WAIT;
            while out.len() == before {
                if self.wants_input == 0 {
                    // It must answer with output or a request for more input.
                    self.next_event(true, out)?;
                } else if !self.next_event(false, out)? {
                    if Instant::now() >= deadline {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        }
        Ok(())
    }

    unsafe fn encode_sync(&mut self, sample: &IMFSample, out: &mut Vec<Encoded>) -> Result<(), String> {
        // SAFETY: see `encode`.
        unsafe {
            if let Err(e) = self.transform.ProcessInput(self.input_id, sample, 0) {
                if e.code() != MF_E_NOTACCEPTING {
                    return Err(format!("input: {e}"));
                }
                self.drain(out, false)?;
                self.transform.ProcessInput(self.input_id, sample, 0).map_err(|e| format!("input: {e}"))?;
            }
            self.drain(out, false)
        }
    }

    /// Handles one event of an asynchronous encoder. `wait`: block until one
    /// comes. Returns whether there was one.
    unsafe fn next_event(&mut self, wait: bool, out: &mut Vec<Encoded>) -> Result<bool, String> {
        let Some(events) = &self.events else { return Ok(false) };
        let flags = if wait { Default::default() } else { MF_EVENT_FLAG_NO_WAIT };
        // SAFETY: see `encode`.
        unsafe {
            let event = match events.GetEvent(flags) {
                Ok(ev) => ev,
                Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => return Ok(false),
                Err(e) => return Err(format!("event: {e}")),
            };
            let kind = event.GetType().map_err(|e| format!("event type: {e}"))?;
            if kind == METransformNeedInput.0 as u32 {
                self.wants_input += 1;
            } else if kind == METransformHaveOutput.0 as u32 {
                self.drain(out, true)?;
            }
        }
        Ok(true)
    }

    /// Takes output from the encoder: one frame (`once`, asynchronous
    /// encoders get one event per frame) or all it has.
    unsafe fn drain(&mut self, out: &mut Vec<Encoded>, once: bool) -> Result<(), String> {
        loop {
            // SAFETY: see `encode`. The output buffer's COM fields are taken
            // back out right after the call, so nothing leaks.
            unsafe {
                let sample = if self.provides_samples { None } else { Some(self.empty_sample()?) };
                let mut buffer = MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: self.output_id,
                    pSample: ManuallyDrop::new(sample),
                    dwStatus: 0,
                    pEvents: ManuallyDrop::new(None),
                };
                let mut status = 0;
                let result = self.transform.ProcessOutput(0, std::slice::from_mut(&mut buffer), &mut status);
                let sample = ManuallyDrop::take(&mut buffer.pSample);
                drop(ManuallyDrop::take(&mut buffer.pEvents));
                match result {
                    Ok(()) => {
                        if let Some(s) = sample {
                            let encoded = self.read(&s)?;
                            if !encoded.data.is_empty() {
                                out.push(encoded);
                            }
                        }
                        if once {
                            return Ok(());
                        }
                    }
                    Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(()),
                    Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                        let t = self.transform.GetOutputAvailableType(self.output_id, 0).map_err(|e| e.to_string())?;
                        self.transform.SetOutputType(self.output_id, &t, 0).map_err(|e| e.to_string())?;
                    }
                    Err(e) => return Err(format!("output: {e}")),
                }
            }
        }
    }

    unsafe fn read(&mut self, sample: &IMFSample) -> Result<Encoded, String> {
        // SAFETY: the locked buffer is valid for `len` bytes until Unlock.
        let mut data = unsafe {
            let buffer = sample.ConvertToContiguousBuffer().map_err(|e| e.to_string())?;
            let mut ptr = std::ptr::null_mut();
            let mut len = 0u32;
            buffer.Lock(&mut ptr, None, Some(&mut len)).map_err(|e| e.to_string())?;
            let data = std::slice::from_raw_parts(ptr, len as usize).to_vec();
            buffer.Unlock().map_err(|e| e.to_string())?;
            data
        };
        let keyframe = complete_parameter_sets(&mut data, &mut self.parameter_sets);
        Ok(Encoded { data, keyframe })
    }

    unsafe fn sample(&self, nv12: &[u8]) -> Result<IMFSample, String> {
        let e = |err: windows::core::Error| format!("input sample: {err}");
        let len = (self.width * self.height * 3 / 2) as u32;
        if nv12.len() != len as usize {
            return Err(format!("frame of {} bytes, expected {len}", nv12.len()));
        }
        let duration = 10_000_000 / self.fps as i64;
        // SAFETY: the locked buffer has room for `len` bytes until Unlock.
        unsafe {
            let buffer = MFCreateMemoryBuffer(len).map_err(e)?;
            let mut ptr = std::ptr::null_mut();
            buffer.Lock(&mut ptr, None, None).map_err(e)?;
            std::ptr::copy_nonoverlapping(nv12.as_ptr(), ptr, nv12.len());
            buffer.Unlock().map_err(e)?;
            buffer.SetCurrentLength(len).map_err(e)?;
            let sample = MFCreateSample().map_err(e)?;
            sample.AddBuffer(&buffer).map_err(e)?;
            sample.SetSampleTime(self.frame as i64 * duration).map_err(e)?;
            sample.SetSampleDuration(duration).map_err(e)?;
            Ok(sample)
        }
    }

    unsafe fn empty_sample(&self) -> Result<IMFSample, String> {
        // SAFETY: plain object creation.
        unsafe {
            let buffer = MFCreateMemoryBuffer(self.output_size).map_err(|e| e.to_string())?;
            let sample = MFCreateSample().map_err(|e| e.to_string())?;
            sample.AddBuffer(&buffer).map_err(|e| e.to_string())?;
            Ok(sample)
        }
    }
}

impl Drop for MfEncoder {
    fn drop(&mut self) {
        // SAFETY: stops the encoder this struct owns; its fields, and the
        // session last, are released after this.
        unsafe {
            let _ = self.transform.ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
            if let Ok(s) = self.transform.cast::<IMFShutdown>() {
                let _ = s.Shutdown();
            }
            let _ = self.activate.ShutdownObject();
        }
    }
}

/// COM started on this thread (and Media Foundation in the process); COM
/// stops when dropped.
struct Session;

impl Session {
    unsafe fn start() -> Result<Self, String> {
        // Media Foundation starts once per process and stays: starting and
        // stopping it for every share left memory behind in the running client.
        static STARTED: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
        // SAFETY: one MFStartup for the process; never shut down.
        STARTED
            .get_or_init(|| {
                unsafe { MFStartup(MF_VERSION, MFSTARTUP_LITE) }.map_err(|e| format!("Media Foundation: {e}"))
            })
            .clone()?;
        // SAFETY: COM start-up on the encoder thread; S_FALSE (already
        // started) still needs the matching CoUninitialize.
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok().map_err(|e| format!("COM: {e}"))? };
        Ok(Self)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: matches `start`.
        unsafe { CoUninitialize() };
    }
}

/// Hardware H.264 encoders that take NV12, best first.
unsafe fn hardware_encoders() -> Result<Vec<IMFActivate>, String> {
    let input = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_NV12 };
    let output = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_H264 };
    let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    let flags = MFT_ENUM_FLAG(MFT_ENUM_FLAG_HARDWARE.0 | MFT_ENUM_FLAG_SORTANDFILTER.0);
    // SAFETY: MFTEnumEx fills an array of `count` activation objects, which we
    // take over one by one before freeing the array itself.
    unsafe {
        MFTEnumEx(MFT_CATEGORY_VIDEO_ENCODER, flags, Some(&input), Some(&output), &mut list, &mut count)
            .map_err(|e| format!("listing encoders: {e}"))?;
        if list.is_null() {
            return Ok(Vec::new());
        }
        let found = std::slice::from_raw_parts_mut(list, count as usize).iter_mut().filter_map(Option::take).collect();
        CoTaskMemFree(Some(list as *const _));
        Ok(found)
    }
}

unsafe fn friendly_name(activate: &IMFActivate) -> String {
    let mut name = PWSTR::null();
    let mut len = 0u32;
    // SAFETY: the string is allocated by Media Foundation and freed here.
    unsafe {
        if activate.GetAllocatedString(&MFT_FRIENDLY_NAME_Attribute, &mut name, &mut len).is_err() || name.is_null() {
            return "hardware encoder".into();
        }
        let s = name.to_string().unwrap_or_default();
        CoTaskMemFree(Some(name.0 as *const _));
        s
    }
}

unsafe fn set_u32(codec: &ICodecAPI, api: &GUID, value: u32) {
    let mut v = VARIANT::default();
    // SAFETY: a plain VT_UI4 variant.
    unsafe {
        (*v.Anonymous.Anonymous).vt = VT_UI4;
        (*v.Anonymous.Anonymous).Anonymous.ulVal = value;
        let _ = codec.SetValue(api, &v);
    }
}

unsafe fn set_bool(codec: &ICodecAPI, api: &GUID) {
    let mut v = VARIANT::default();
    // SAFETY: a plain VT_BOOL variant (true).
    unsafe {
        (*v.Anonymous.Anonymous).vt = VT_BOOL;
        (*v.Anonymous.Anonymous).Anonymous.boolVal = VARIANT_TRUE;
        let _ = codec.SetValue(api, &v);
    }
}

/// NAL unit types in an Annex B stream, with each unit's start offset
/// (including its start code).
fn nal_units(data: &[u8]) -> Vec<(u8, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 3 < data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            let start = if i > 0 && data[i - 1] == 0 { i - 1 } else { i };
            out.push((data[i + 3] & 0x1f, start));
            i += 3;
        } else {
            i += 1;
        }
    }
    out
}

/// Whether `frame` is a keyframe (has an IDR slice). Remembers its SPS/PPS
/// in `saved`, or puts the saved ones in front when a keyframe lacks them:
/// a viewer joining mid-stream needs them to start decoding.
fn complete_parameter_sets(frame: &mut Vec<u8>, saved: &mut Vec<u8>) -> bool {
    const IDR: u8 = 5;
    const SPS: u8 = 7;
    const PPS: u8 = 8;
    let units = nal_units(frame);
    if !units.iter().any(|(t, _)| *t == IDR) {
        return false;
    }
    let has_sps = units.iter().any(|(t, _)| *t == SPS);
    let has_pps = units.iter().any(|(t, _)| *t == PPS);
    if has_sps && has_pps {
        // Everything before the first slice: SPS, PPS (and any SEI).
        let first_slice = units.iter().find(|(t, _)| matches!(*t, 1 | IDR)).map_or(frame.len(), |(_, s)| *s);
        saved.clear();
        saved.extend_from_slice(&frame[..first_slice]);
    } else if !saved.is_empty() {
        frame.splice(0..0, saved.iter().copied());
    }
    true
}

#[cfg(test)]
mod tests {
    use openh264::decoder::Decoder;
    use openh264::formats::YUVSource;

    use super::*;

    #[test]
    fn keyframes_get_parameter_sets() {
        let sps_pps = [0, 0, 0, 1, 0x67, 1, 2, 0, 0, 0, 1, 0x68, 3];
        let mut first: Vec<u8> = sps_pps.to_vec();
        first.extend([0, 0, 1, 0x65, 9, 9]);
        let mut saved = Vec::new();
        assert!(complete_parameter_sets(&mut first, &mut saved));
        assert_eq!(saved, sps_pps);

        let mut delta = vec![0, 0, 1, 0x41, 7];
        assert!(!complete_parameter_sets(&mut delta, &mut saved));
        assert_eq!(delta, [0, 0, 1, 0x41, 7]);

        let mut bare_key = vec![0, 0, 1, 0x65, 8];
        assert!(complete_parameter_sets(&mut bare_key, &mut saved));
        assert!(bare_key.starts_with(&sps_pps) && bare_key.ends_with(&[0, 0, 1, 0x65, 8]));
    }

    /// The hardware encoder's stream decodes with OpenH264, as every viewer
    /// (Windows and Linux) decodes it; each keyframe can start decoding on
    /// its own. Skipped where there is no hardware encoder (CI).
    #[test]
    fn hardware_stream_decodes_with_openh264() {
        let (w, h) = (1280usize, 720usize);
        let mut enc = match MfEncoder::new(w, h, Quality::P720F30) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("no hardware encoder ({e}), skipped");
                return;
            }
        };
        eprintln!("encoder: {}", enc.name);
        let mut frames = Vec::new();
        for i in 0..90usize {
            // A moving gradient, so frames differ.
            let mut nv12 = vec![0u8; w * h * 3 / 2];
            for y in 0..h {
                for x in 0..w {
                    nv12[y * w + x] = ((x + y + i * 4) % 220 + 16) as u8;
                }
            }
            for (j, c) in nv12[w * h..].iter_mut().enumerate() {
                *c = if j % 2 == 0 { 90 } else { (128 + i) as u8 };
            }
            // Keyframes on request, as when a viewer joins.
            frames.extend(enc.encode(&nv12, i == 0 || i == 45).unwrap());
        }
        assert!(frames.len() >= 80, "only {} of 90 frames came out", frames.len());
        let keys: Vec<usize> = frames.iter().enumerate().filter(|(_, f)| f.keyframe).map(|(i, _)| i).collect();
        assert!(keys.len() >= 2 && keys[0] == 0, "keyframes at {keys:?}");

        // From the start, and from the second keyframe as a viewer joining late.
        for start in [0, keys[1]] {
            let mut dec = Decoder::new().unwrap();
            let mut decoded = 0;
            for f in &frames[start..] {
                if let Ok(Some(yuv)) = dec.decode(&f.data) {
                    assert_eq!(yuv.dimensions(), (w, h));
                    decoded += 1;
                }
            }
            assert!(decoded >= frames.len() - start - 2, "from frame {start}: decoded {decoded}");
        }
    }
}
