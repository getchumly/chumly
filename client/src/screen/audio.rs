//! The share's audio: one backend per platform.

#[cfg(windows)]
mod win;
#[cfg(windows)]
pub(super) use win::{AudioTarget, StreamAudio};

#[cfg(not(windows))]
pub(super) use stub::{AudioTarget, StreamAudio};

/// No share audio on this platform yet.
#[cfg(not(windows))]
mod stub {
    use shared::media::MediaKind;

    /// What to capture.
    #[derive(Clone, Copy, Debug)]
    pub enum AudioTarget {
        /// Audio of the process and all its children.
        Only(#[allow(dead_code)] u32),
        /// All audio except the process and its children.
        AllExcept(#[allow(dead_code)] u32),
    }

    pub struct StreamAudio;

    impl StreamAudio {
        pub fn start(
            _target: AudioTarget,
            _out: tokio::sync::mpsc::Sender<(MediaKind, Vec<u8>)>,
        ) -> Result<Self, String> {
            Err("sharing audio is not available on this platform yet".into())
        }
    }
}
