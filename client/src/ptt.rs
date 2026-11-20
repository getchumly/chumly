//! Global push-to-talk key. Keyboard polling works even when the
//! client window is not focused, e.g. while gaming.
//! macOS requires the Accessibility permission, Linux requires X11.
//!
//! Polling only runs in push-to-talk mode or while assigning a key.
//! On Windows a single assigned key is checked with one system call.
//! A full keyboard scan via device_query costs 256 calls and at
//! 100 polls per second noticeably loads the CPU.
//!
//! The middle or side mouse buttons can be assigned instead of a key.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use device_query::{DeviceQuery, DeviceState, Keycode};

use crate::audio::AudioShared;
use crate::hotkeys::{Combo, Key, modifier_flag};

#[cfg(windows)]
pub use platform::virtual_key;

/// Polling rate while a key is held: up to 10 ms latency is inaudible.
const POLL_ACTIVE: Duration = Duration::from_millis(10);
/// While push-to-talk is off, the thread only checks whether it was turned on.
const POLL_IDLE: Duration = Duration::from_millis(100);

pub struct PttWatcher {
    key: Arc<Mutex<Option<Key>>>,
    capture: Arc<AtomicBool>,
    captured: Arc<Mutex<Option<Key>>>,
    /// Hotkey assignment: waiting for a combination with modifiers.
    capture_combo: Arc<AtomicBool>,
    captured_combo: Arc<Mutex<Option<Combo>>>,
    pub available: Arc<AtomicBool>,
}

/// A newly pressed assignable mouse button: middle or side.
/// Left and right are excluded since they are used to click "Assign".
fn new_mouse_button(now: &[bool], prev: &[bool]) -> Option<Key> {
    Key::MOUSE_BUTTONS
        .into_iter()
        .find(|&b| now.get(b as usize) == Some(&true) && prev.get(b as usize) != Some(&true))
        .map(Key::Mouse)
}

impl PttWatcher {
    pub fn start(shared: Arc<AudioShared>, key_name: &str) -> Self {
        let w = Self {
            key: Arc::new(Mutex::new(key_name.parse().ok())),
            capture: Arc::new(AtomicBool::new(false)),
            captured: Arc::new(Mutex::new(None)),
            capture_combo: Arc::new(AtomicBool::new(false)),
            captured_combo: Arc::new(Mutex::new(None)),
            available: Arc::new(AtomicBool::new(true)),
        };
        let (key, capture, captured, available) =
            (w.key.clone(), w.capture.clone(), w.captured.clone(), w.available.clone());
        let (capture_combo, captured_combo) = (w.capture_combo.clone(), w.captured_combo.clone());
        let spawned = std::thread::Builder::new().name("ptt".into()).spawn(move || {
            let Some(state) = DeviceState::checked_new() else {
                tracing::warn!("keyboard polling unavailable, push-to-talk will not work");
                available.store(false, Ordering::Relaxed);
                return;
            };
            let mut prev: Vec<Keycode> = Vec::new();
            let mut prev_mouse: Vec<bool> = Vec::new();
            loop {
                let capturing_combo = capture_combo.load(Ordering::Relaxed);
                let capturing = capture.load(Ordering::Relaxed) || capturing_combo;
                if capturing {
                    // Key assignment needs a full scan to see any key.
                    let keys = state.get_keys();
                    let mouse = state.get_mouse().button_pressed;
                    // On the first poll, buttons held before assignment started are ignored.
                    let clicked = if prev_mouse.is_empty() { None } else { new_mouse_button(&mouse, &prev_mouse) };
                    if capturing_combo {
                        // Modifiers alone are not a combination: wait for a regular key.
                        let pressed = keys
                            .iter()
                            .find(|k| !prev.contains(k) && modifier_flag(**k) == 0)
                            .map(|k| Key::Keyboard(*k))
                            .or(clicked);
                        if let Some(k) = pressed {
                            *captured_combo.lock().unwrap() = Some(Combo::from_keys(&keys, k));
                            capture_combo.store(false, Ordering::Relaxed);
                        }
                    } else {
                        let pressed = keys.iter().find(|k| !prev.contains(k)).map(|k| Key::Keyboard(*k)).or(clicked);
                        if let Some(k) = pressed {
                            *captured.lock().unwrap() = Some(k);
                            capture.store(false, Ordering::Relaxed);
                        }
                    }
                    prev = keys;
                    prev_mouse = mouse;
                } else {
                    prev.clear();
                    prev_mouse.clear();
                }

                if !shared.ptt_mode.load(Ordering::Relaxed) {
                    shared.ptt_pressed.store(false, Ordering::Relaxed);
                    std::thread::sleep(if capturing { POLL_ACTIVE } else { POLL_IDLE });
                    continue;
                }

                let bound = *key.lock().unwrap();
                let pressed = match bound {
                    None => false,
                    Some(k) => match platform::is_pressed(k) {
                        Some(p) => p,
                        None => match k {
                            Key::Keyboard(k) => state.get_keys().contains(&k),
                            Key::Mouse(b) => state.get_mouse().button_pressed.get(b as usize) == Some(&true),
                        },
                    },
                };
                shared.ptt_pressed.store(pressed, Ordering::Relaxed);
                std::thread::sleep(POLL_ACTIVE);
            }
        });
        if spawned.is_err() {
            w.available.store(false, Ordering::Relaxed);
        }
        w
    }

    pub fn set_key(&self, key_name: &str) {
        *self.key.lock().unwrap() = key_name.parse().ok();
    }

    /// The next pressed key will be returned from `take_captured`.
    pub fn begin_capture(&self) {
        *self.captured.lock().unwrap() = None;
        self.capture.store(true, Ordering::Relaxed);
    }

    pub fn is_capturing(&self) -> bool {
        self.capture.load(Ordering::Relaxed)
    }

    pub fn cancel_capture(&self) {
        self.capture.store(false, Ordering::Relaxed);
        self.capture_combo.store(false, Ordering::Relaxed);
    }

    /// The next combination will be returned from `take_combo`.
    pub fn begin_combo_capture(&self) {
        *self.captured_combo.lock().unwrap() = None;
        self.capture.store(false, Ordering::Relaxed);
        self.capture_combo.store(true, Ordering::Relaxed);
    }

    pub fn is_capturing_combo(&self) -> bool {
        self.capture_combo.load(Ordering::Relaxed)
    }

    pub fn take_combo(&self) -> Option<Combo> {
        self.captured_combo.lock().unwrap().take()
    }

    pub fn take_captured(&self) -> Option<String> {
        self.captured.lock().unwrap().take().map(|k| k.to_string())
    }
}

#[cfg(windows)]
mod platform {
    use device_query::Keycode;

    use crate::hotkeys::Key;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetAsyncKeyState(vk: i32) -> i16;
    }

    /// `None` if the key has no known code: the caller then
    /// falls back to a full scan.
    pub fn is_pressed(k: Key) -> Option<bool> {
        let vk = virtual_key(k)?;
        // The high bit means the key is currently held down.
        // SAFETY: takes no pointers and accepts any key code.
        let state = unsafe { GetAsyncKeyState(vk) };
        Some(state as u16 & 0x8000 != 0)
    }

    /// Windows virtual key codes (VK_*), including mouse buttons.
    pub fn virtual_key(k: Key) -> Option<i32> {
        let k = match k {
            // VK_MBUTTON, VK_XBUTTON1, VK_XBUTTON2.
            Key::Mouse(3) => return Some(0x04),
            Key::Mouse(4) => return Some(0x05),
            Key::Mouse(5) => return Some(0x06),
            Key::Mouse(_) => return None,
            Key::Keyboard(k) => k,
        };
        use Keycode::*;
        let letters = [A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P, Q, R, S, T, U, V, W, X, Y, Z];
        if let Some(i) = letters.iter().position(|&x| x == k) {
            return Some(0x41 + i as i32);
        }
        let digits = [Key0, Key1, Key2, Key3, Key4, Key5, Key6, Key7, Key8, Key9];
        if let Some(i) = digits.iter().position(|&x| x == k) {
            return Some(0x30 + i as i32);
        }
        let fkeys = [F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12, F13, F14, F15, F16, F17, F18, F19, F20];
        if let Some(i) = fkeys.iter().position(|&x| x == k) {
            return Some(0x70 + i as i32);
        }
        let numpad = [Numpad0, Numpad1, Numpad2, Numpad3, Numpad4, Numpad5, Numpad6, Numpad7, Numpad8, Numpad9];
        if let Some(i) = numpad.iter().position(|&x| x == k) {
            return Some(0x60 + i as i32);
        }
        Some(match k {
            Escape => 0x1B,
            Space => 0x20,
            LControl => 0xA2,
            RControl => 0xA3,
            LShift => 0xA0,
            RShift => 0xA1,
            LAlt => 0xA4,
            RAlt => 0xA5,
            LMeta => 0x5B,
            RMeta => 0x5C,
            Enter => 0x0D,
            Up => 0x26,
            Down => 0x28,
            Left => 0x25,
            Right => 0x27,
            Backspace => 0x08,
            CapsLock => 0x14,
            Tab => 0x09,
            Home => 0x24,
            End => 0x23,
            PageUp => 0x21,
            PageDown => 0x22,
            Insert => 0x2D,
            Delete => 0x2E,
            NumpadSubtract => 0x6D,
            NumpadAdd => 0x6B,
            NumpadDivide => 0x6F,
            NumpadMultiply => 0x6A,
            NumpadDecimal => 0x6E,
            Grave => 0xC0,
            Minus => 0xBD,
            Equal => 0xBB,
            LeftBracket => 0xDB,
            RightBracket => 0xDD,
            BackSlash => 0xDC,
            Semicolon => 0xBA,
            Apostrophe => 0xDE,
            Comma => 0xBC,
            Dot => 0xBE,
            Slash => 0xBF,
            _ => return None,
        })
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn keybd_event(vk: u8, scan: u8, flags: u32, extra: usize);
    }

    /// Presses F13 via system input: regular keyboards lack this key,
    /// so it types nothing into open windows.
    #[test]
    fn fast_path_matches_full_poll() {
        if crate::skip_system_input_on_ci() {
            return;
        }
        let _input = crate::lock_system_input();
        let state = DeviceState::new();
        assert_eq!(platform::is_pressed(Key::Keyboard(Keycode::F13)), Some(false));
        unsafe { keybd_event(0x7C, 0, 0, 0) };
        std::thread::sleep(Duration::from_millis(30));
        let fast = platform::is_pressed(Key::Keyboard(Keycode::F13));
        let full = state.get_keys().contains(&Keycode::F13);
        unsafe { keybd_event(0x7C, 0, 2, 0) };
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(fast, Some(full), "fast and full polling disagree");
        if !full {
            // Windows blocks input emulation when an elevated window is focused.
            // Nothing to test then; this is not a failure.
            eprintln!("skipped: emulated F13 press blocked by the system");
            return;
        }
        assert_eq!(platform::is_pressed(Key::Keyboard(Keycode::F13)), Some(false));
    }
}

#[cfg(not(windows))]
mod platform {
    use crate::hotkeys::Key;

    /// Other platforms have no fast path; a full scan is used.
    pub fn is_pressed(_k: Key) -> Option<bool> {
        None
    }
}
