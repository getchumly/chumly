//! Global hotkeys: toggle microphone and sound output.
//! Work even when the window is in the background, e.g. while gaming.
//!
//! On Windows keys are registered via RegisterHotKey: the system
//! sends an event on press, nothing is polled. Only Chumly receives
//! the combination; other programs will not see it.
//! On other platforms the keyboard is polled every 50 ms, and only
//! if at least one key is assigned.
//!
//! A combination is stored as a string like "Ctrl+Shift+M": modifiers and
//! a key using device_query names. The key may be the middle or a side
//! mouse button instead: "Mouse3", "Mouse4", "Mouse5". On Windows mouse
//! buttons are caught by a low-level mouse hook (WH_MOUSE_LL). It is
//! installed only when something is bound to the mouse and never swallows
//! the click: the browser "back" button keeps working.

use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use device_query::Keycode;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HotkeyAction {
    ToggleMute,
    ToggleDeafen,
}

pub const MOD_ALT: u32 = 1;
pub const MOD_CTRL: u32 = 2;
pub const MOD_SHIFT: u32 = 4;
pub const MOD_WIN: u32 = 8;

/// A keyboard key or a mouse button.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Key {
    Keyboard(Keycode),
    /// Button number as in device_query: 3 is middle, 4 and 5 are side buttons.
    Mouse(u8),
}

impl Key {
    /// Assignable mouse buttons. Left and right are needed for
    /// normal use.
    pub const MOUSE_BUTTONS: [u8; 3] = [3, 4, 5];
}

impl std::fmt::Display for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Key::Keyboard(k) => write!(f, "{k}"),
            Key::Mouse(b) => write!(f, "Mouse{b}"),
        }
    }
}

impl std::str::FromStr for Key {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, ()> {
        if let Some(n) = s.strip_prefix("Mouse") {
            return match n.parse::<u8>() {
                Ok(b) if Key::MOUSE_BUTTONS.contains(&b) => Ok(Key::Mouse(b)),
                _ => Err(()),
            };
        }
        s.parse::<Keycode>().map(Key::Keyboard).map_err(|_| ())
    }
}

/// A parsed combination.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Combo {
    pub mods: u32,
    pub key: Key,
}

impl Combo {
    pub fn parse(s: &str) -> Option<Self> {
        let mut mods = 0;
        let mut key = None;
        for part in s.split('+').map(str::trim).filter(|p| !p.is_empty()) {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => mods |= MOD_CTRL,
                "shift" => mods |= MOD_SHIFT,
                "alt" => mods |= MOD_ALT,
                "win" | "meta" => mods |= MOD_WIN,
                _ if key.is_none() => key = Some(part.parse::<Key>().ok()?),
                _ => return None,
            }
        }
        Some(Self { mods, key: key? })
    }

    /// Builds a combination from currently held keys plus a new key.
    pub fn from_keys(held: &[Keycode], key: Key) -> Self {
        let mut mods = 0;
        for k in held {
            mods |= modifier_flag(*k);
        }
        Self { mods, key }
    }
}

impl std::fmt::Display for Combo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (flag, name) in [(MOD_CTRL, "Ctrl"), (MOD_ALT, "Alt"), (MOD_SHIFT, "Shift"), (MOD_WIN, "Win")] {
            if self.mods & flag != 0 {
                write!(f, "{name}+")?;
            }
        }
        write!(f, "{}", self.key)
    }
}

/// Modifier flag for a modifier key, 0 for other keys.
pub fn modifier_flag(k: Keycode) -> u32 {
    use Keycode::*;
    match k {
        LControl | RControl => MOD_CTRL,
        LShift | RShift => MOD_SHIFT,
        LAlt | RAlt => MOD_ALT,
        LMeta | RMeta => MOD_WIN,
        _ => 0,
    }
}

/// Assigned combinations; `None` if unassigned.
pub type Bindings = Vec<(HotkeyAction, Option<Combo>)>;

pub struct Hotkeys {
    bindings: Arc<Mutex<Bindings>>,
    /// Combinations that failed to register, e.g. because
    /// another program already took them.
    failed: Arc<Mutex<Vec<HotkeyAction>>>,
    events: Receiver<HotkeyAction>,
    platform: platform::Handle,
}

impl Hotkeys {
    pub fn start(ctx: eframe::egui::Context, bindings: Bindings) -> Self {
        let bindings = Arc::new(Mutex::new(bindings));
        let failed = Arc::new(Mutex::new(Vec::new()));
        let (tx, events) = std::sync::mpsc::channel();
        let platform = platform::start(bindings.clone(), failed.clone(), Notify { tx, ctx });
        Self { bindings, failed, events, platform }
    }

    pub fn set(&self, bindings: Bindings) {
        *self.bindings.lock().unwrap_or_else(|e| e.into_inner()) = bindings;
        self.platform.reload();
    }

    /// Pressed since the last call.
    pub fn take(&self) -> Vec<HotkeyAction> {
        self.events.try_iter().collect()
    }

    pub fn failed(&self, action: HotkeyAction) -> bool {
        self.failed.lock().unwrap_or_else(|e| e.into_inner()).contains(&action)
    }
}

struct Notify {
    tx: Sender<HotkeyAction>,
    ctx: eframe::egui::Context,
}

impl Notify {
    fn send(&self, a: HotkeyAction) {
        let _ = self.tx.send(a);
        // The window may be minimized; without this the event would wait for the next frame.
        self.ctx.request_repaint();
    }
}

#[cfg(windows)]
mod platform {
    use std::sync::{Arc, Mutex};

    use super::{Bindings, Combo, HotkeyAction, Key, Notify};

    const WM_HOTKEY: u32 = 0x0312;
    const WH_MOUSE_LL: i32 = 14;
    const WM_MBUTTONDOWN: usize = 0x0207;
    const WM_XBUTTONDOWN: usize = 0x020B;
    const WM_APP_RELOAD: u32 = 0x8000 + 1;
    const PM_NOREMOVE: u32 = 0;
    const MOD_NOREPEAT: u32 = 0x4000;

    #[repr(C)]
    struct Msg {
        hwnd: isize,
        message: u32,
        wparam: usize,
        lparam: isize,
        time: u32,
        pt: [i32; 2],
        private: u32,
    }

    #[repr(C)]
    struct MouseHookData {
        pt: [i32; 2],
        mouse_data: u32,
        flags: u32,
        time: u32,
        extra: usize,
    }

    type HookProc = unsafe extern "system" fn(i32, usize, isize) -> isize;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn SetWindowsHookExW(id: i32, proc: HookProc, module: isize, thread: u32) -> isize;
        fn UnhookWindowsHookEx(hook: isize) -> i32;
        fn CallNextHookEx(hook: isize, code: i32, wparam: usize, lparam: isize) -> isize;
        fn GetAsyncKeyState(vk: i32) -> i16;
        fn RegisterHotKey(hwnd: isize, id: i32, mods: u32, vk: u32) -> i32;
        fn UnregisterHotKey(hwnd: isize, id: i32) -> i32;
        fn GetMessageW(msg: *mut Msg, hwnd: isize, min: u32, max: u32) -> i32;
        fn PeekMessageW(msg: *mut Msg, hwnd: isize, min: u32, max: u32, remove: u32) -> i32;
        fn PostThreadMessageW(thread: u32, msg: u32, wparam: usize, lparam: isize) -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentThreadId() -> u32;
        fn GetModuleHandleW(name: *const u16) -> isize;
    }

    /// Mouse button combinations for the hook. The hook is called by the
    /// system without context, so the state is global.
    struct MouseHook {
        combos: Vec<(HotkeyAction, u32, u8)>,
        notify: Notify,
    }

    static MOUSE: Mutex<Option<MouseHook>> = Mutex::new(None);

    fn held_mods() -> u32 {
        // SAFETY: takes no pointers.
        let down = |vk| unsafe { GetAsyncKeyState(vk) } as u16 & 0x8000 != 0;
        let mut m = 0;
        if down(0x11) {
            m |= super::MOD_CTRL;
        }
        if down(0x10) {
            m |= super::MOD_SHIFT;
        }
        if down(0x12) {
            m |= super::MOD_ALT;
        }
        if down(0x5B) || down(0x5C) {
            m |= super::MOD_WIN;
        }
        m
    }

    /// Called by the system for every mouse event, so it does the bare minimum.
    /// Never swallows the click: always passes it on.
    unsafe extern "system" fn mouse_proc(code: i32, wparam: usize, lparam: isize) -> isize {
        if code >= 0 {
            let button = match wparam {
                WM_MBUTTONDOWN => Some(3),
                WM_XBUTTONDOWN => {
                    // SAFETY: for WH_MOUSE_LL lparam points to MSLLHOOKSTRUCT.
                    let data = unsafe { &*(lparam as *const MouseHookData) };
                    match data.mouse_data >> 16 {
                        1 => Some(4),
                        2 => Some(5),
                        _ => None,
                    }
                }
                _ => None,
            };
            if let Some(button) = button
                && let Ok(guard) = MOUSE.try_lock()
                && let Some(hook) = guard.as_ref()
            {
                let mods = held_mods();
                for (action, m, b) in &hook.combos {
                    if *b == button && *m == mods {
                        hook.notify.send(*action);
                    }
                }
            }
        }
        // SAFETY: passes the event to the next hooks unchanged.
        unsafe { CallNextHookEx(0, code, wparam, lparam) }
    }

    pub struct Handle {
        thread_id: u32,
    }

    impl Handle {
        pub fn reload(&self) {
            if self.thread_id != 0 {
                // SAFETY: a plain thread message with no pointers.
                unsafe { PostThreadMessageW(self.thread_id, WM_APP_RELOAD, 0, 0) };
            }
        }
    }

    pub fn start(bindings: Arc<Mutex<Bindings>>, failed: Arc<Mutex<Vec<HotkeyAction>>>, notify: Notify) -> Handle {
        let (id_tx, id_rx) = std::sync::mpsc::channel();
        let mouse_notify = Notify { tx: notify.tx.clone(), ctx: notify.ctx.clone() };
        let spawned = std::thread::Builder::new().name("hotkeys".into()).spawn(move || {
            let mut msg = Msg { hwnd: 0, message: 0, wparam: 0, lparam: 0, time: 0, pt: [0, 0], private: 0 };
            // SAFETY: creates the thread's message queue; otherwise
            // PostThreadMessageW from another thread would not arrive.
            unsafe { PeekMessageW(&mut msg, 0, 0, 0, PM_NOREMOVE) };
            let _ = id_tx.send(unsafe { GetCurrentThreadId() });
            let mut registered: Vec<(i32, HotkeyAction)> = Vec::new();
            let mut hook: isize = 0;
            *MOUSE.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(MouseHook { combos: Vec::new(), notify: mouse_notify });
            let mut register = |registered: &mut Vec<(i32, HotkeyAction)>| {
                for (id, _) in registered.drain(..) {
                    // SAFETY: unregisters what this thread registered itself.
                    unsafe { UnregisterHotKey(0, id) };
                }
                let mut bad = Vec::new();
                let list = bindings.lock().unwrap_or_else(|e| e.into_inner()).clone();
                let mouse: Vec<(HotkeyAction, u32, u8)> = list
                    .iter()
                    .filter_map(|(a, c)| match c {
                        Some(Combo { mods, key: Key::Mouse(b) }) => Some((*a, *mods, *b)),
                        _ => None,
                    })
                    .collect();
                if let Some(h) = MOUSE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                    h.combos = mouse.clone();
                }
                // The mouse hook is only needed while something is bound to the mouse.
                if mouse.is_empty() && hook != 0 {
                    // SAFETY: removes our own hook.
                    unsafe { UnhookWindowsHookEx(hook) };
                    hook = 0;
                } else if !mouse.is_empty() && hook == 0 {
                    // SAFETY: the hook is installed on a thread with a message loop,
                    // as WH_MOUSE_LL requires.
                    hook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, mouse_proc, GetModuleHandleW(std::ptr::null()), 0) };
                    if hook == 0 {
                        tracing::warn!("failed to install mouse hook");
                        bad.extend(mouse.iter().map(|(a, _, _)| *a));
                    }
                }
                for (i, (action, combo)) in list.iter().enumerate() {
                    let Some(combo) = combo else { continue };
                    if matches!(combo.key, Key::Mouse(_)) {
                        continue;
                    }
                    let id = i as i32 + 1;
                    let ok = crate::ptt::virtual_key(combo.key).is_some_and(|vk| {
                        // SAFETY: registers for this thread's queue (hwnd = 0).
                        unsafe { RegisterHotKey(0, id, combo.mods | MOD_NOREPEAT, vk as u32) != 0 }
                    });
                    if ok {
                        registered.push((id, *action));
                    } else {
                        tracing::warn!("hotkey {combo} not registered: already taken or unsupported");
                        bad.push(*action);
                    }
                }
                *failed.lock().unwrap_or_else(|e| e.into_inner()) = bad;
            };
            register(&mut registered);
            // SAFETY: a regular message loop; 0 means WM_QUIT, -1 means error.
            while unsafe { GetMessageW(&mut msg, 0, 0, 0) } > 0 {
                match msg.message {
                    WM_HOTKEY => {
                        if let Some((_, a)) = registered.iter().find(|(id, _)| *id as usize == msg.wparam) {
                            notify.send(*a);
                        }
                    }
                    WM_APP_RELOAD => register(&mut registered),
                    _ => {}
                }
            }
        });
        let thread_id = match spawned {
            Ok(_) => id_rx.recv().unwrap_or(0),
            Err(e) => {
                tracing::error!("hotkey thread failed to start: {e}");
                0
            }
        };
        Handle { thread_id }
    }
}

#[cfg(not(windows))]
mod platform {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use device_query::{DeviceQuery, DeviceState};

    use super::{Bindings, Combo, HotkeyAction, Key, Notify, modifier_flag};

    pub struct Handle;

    impl Handle {
        pub fn reload(&self) {}
    }

    pub fn start(bindings: Arc<Mutex<Bindings>>, _failed: Arc<Mutex<Vec<HotkeyAction>>>, notify: Notify) -> Handle {
        let _ = std::thread::Builder::new().name("hotkeys".into()).spawn(move || {
            let Some(state) = DeviceState::checked_new() else {
                tracing::warn!("keyboard polling unavailable, hotkeys will not work");
                return;
            };
            let mut down: Vec<HotkeyAction> = Vec::new();
            loop {
                let list = bindings.lock().unwrap_or_else(|e| e.into_inner()).clone();
                if list.iter().all(|(_, c)| c.is_none()) {
                    std::thread::sleep(Duration::from_millis(500));
                    continue;
                }
                let keys = state.get_keys();
                let mouse = state.get_mouse().button_pressed;
                let mods = keys.iter().fold(0, |m, k| m | modifier_flag(*k));
                for (action, combo) in &list {
                    let pressed = combo.is_some_and(|c: Combo| {
                        c.mods == mods
                            && match c.key {
                                Key::Keyboard(k) => keys.contains(&k),
                                Key::Mouse(b) => mouse.get(b as usize) == Some(&true),
                            }
                    });
                    let was = down.contains(action);
                    if pressed && !was {
                        down.push(*action);
                        notify.send(*action);
                    } else if !pressed && was {
                        down.retain(|a| a != action);
                    }
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        Handle
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Registers F13 and presses it via system input. Regular keyboards
    /// lack this key, so it gets in nobody's way.
    #[cfg(windows)]
    #[test]
    fn registered_hotkey_fires() {
        if crate::skip_system_input_on_ci() {
            return;
        }
        let _input = crate::lock_system_input();
        #[link(name = "user32")]
        unsafe extern "system" {
            fn keybd_event(vk: u8, scan: u8, flags: u32, extra: usize);
        }
        let hk = Hotkeys::start(
            eframe::egui::Context::default(),
            vec![(HotkeyAction::ToggleMute, Combo::parse("F13")), (HotkeyAction::ToggleDeafen, Combo::parse("Mouse4"))],
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(!hk.failed(HotkeyAction::ToggleMute), "F13 was not registered");
        assert!(!hk.failed(HotkeyAction::ToggleDeafen), "mouse hook was not installed");
        unsafe {
            keybd_event(0x7C, 0, 0, 0);
            keybd_event(0x7C, 0, 2, 0);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
        let got = hk.take();
        if got.is_empty() {
            // As in the PTT test: input emulation is blocked when an elevated
            // window is focused.
            eprintln!("skipped: emulated key press blocked by the system");
            return;
        }
        assert_eq!(got, vec![HotkeyAction::ToggleMute]);

        // After unassigning, the key no longer fires.
        hk.set(vec![(HotkeyAction::ToggleMute, None)]);
        std::thread::sleep(std::time::Duration::from_millis(100));
        unsafe {
            keybd_event(0x7C, 0, 0, 0);
            keybd_event(0x7C, 0, 2, 0);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(hk.take().is_empty());
    }

    #[test]
    fn combo_roundtrip() {
        let c = Combo::parse("Ctrl+Shift+M").unwrap();
        assert_eq!(c, Combo { mods: MOD_CTRL | MOD_SHIFT, key: Key::Keyboard(Keycode::M) });
        assert_eq!(c.to_string(), "Ctrl+Shift+M");
        assert_eq!(Combo::parse("F9").unwrap().to_string(), "F9");
        assert_eq!(
            Combo::from_keys(&[Keycode::LAlt, Keycode::RControl], Key::Keyboard(Keycode::F5)).to_string(),
            "Ctrl+Alt+F5"
        );
        assert_eq!(Combo::parse("Mouse4").unwrap().key, Key::Mouse(4));
        assert_eq!(Combo::parse("Shift+Mouse5").unwrap().to_string(), "Shift+Mouse5");
        assert!(Combo::parse("Mouse1").is_none(), "left button must not be assignable");
        assert_eq!("Mouse3".parse::<Key>(), Ok(Key::Mouse(3)));
        assert_eq!("Grave".parse::<Key>(), Ok(Key::Keyboard(Keycode::Grave)));
        assert!(Combo::parse("").is_none());
        assert!(Combo::parse("Ctrl+M+N").is_none());
        assert!(Combo::parse("Ctrl").is_none());
    }
}
