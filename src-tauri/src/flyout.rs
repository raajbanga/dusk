//! Win32/DWM bits that make the panel behave like the Windows 11 tray flyouts (Quick Settings, volume).
//!
//! - The pre-warmed window is shown *cloaked*: DWM composes it (so WebView2 paints and the acrylic
//!   backdrop settles) but nothing is on screen. Opening only uncloaks it, so there is no empty,
//!   grey first frame.
//! - It slides up its full height from behind the taskbar on open, and back down on close.
//!   Timings were measured from Quick Settings frame by frame.

use std::ffi::c_void;
use std::time::{Duration, Instant};

/// Duration and cubic-bezier curve: a strong decelerate in, Fluent's accelerate out.
pub type Motion = (Duration, [f64; 4]);
pub const OPEN: Motion = (Duration::from_millis(230), [0.1, 0.7, 0.0, 1.0]);
pub const CLOSE: Motion = (Duration::from_millis(155), [1.0, 0.0, 1.0, 1.0]);

pub type Hwnd = *mut c_void;

#[link(name = "dwmapi")]
unsafe extern "system" {
    fn DwmSetWindowAttribute(hwnd: Hwnd, attr: u32, value: *const c_void, size: u32) -> i32;
    fn DwmFlush() -> i32;
}

#[link(name = "user32")]
unsafe extern "system" {
    fn ShowWindow(hwnd: Hwnd, cmd: i32) -> i32;
    fn SetForegroundWindow(hwnd: Hwnd) -> i32;
    fn keybd_event(vk: u8, scan: u8, flags: u32, extra: usize);
    fn SetWindowPos(hwnd: Hwnd, after: Hwnd, x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> i32;
    fn SystemParametersInfoW(action: u32, param: u32, value: *mut c_void, ini: u32) -> i32;
}

fn dwm_set(hwnd: Hwnd, attr: u32, value: u32) {
    unsafe { DwmSetWindowAttribute(hwnd, attr, &value as *const u32 as _, 4) };
}

/// Right after creation: rounded corners, no DWM show/hide fade, then show it cloaked without
/// taking focus.
pub fn prewarm(hwnd: Hwnd) {
    const DWMWA_TRANSITIONS_FORCEDISABLED: u32 = 3;
    const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
    const DWMWCP_ROUND: u32 = 2;
    const SW_SHOWNOACTIVATE: i32 = 4;
    dwm_set(hwnd, DWMWA_TRANSITIONS_FORCEDISABLED, 1);
    dwm_set(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND);
    cloak(hwnd, true);
    unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
}

/// Uncloaks and activates. tao's `set_focus` skips windows it didn't show itself, so this does
/// what it does: activate, and only if Windows refuses (the tray click can leave the shell in
/// the foreground), tap Alt to lift the foreground lock and try again.
pub fn reveal(hwnd: Hwnd) {
    const VK_LMENU: u8 = 0xA4;
    const KEYEVENTF_EXTENDEDKEY: u32 = 1;
    const KEYEVENTF_KEYUP: u32 = 2;
    cloak(hwnd, false);
    unsafe {
        if SetForegroundWindow(hwnd) == 0 {
            keybd_event(VK_LMENU, 0, KEYEVENTF_EXTENDEDKEY, 0);
            keybd_event(VK_LMENU, 0, KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP, 0);
            SetForegroundWindow(hwnd);
        }
    }
}

fn cloak(hwnd: Hwnd, on: bool) {
    const DWMWA_CLOAK: u32 = 13;
    dwm_set(hwnd, DWMWA_CLOAK, on as u32);
}

/// Moves the window (outer frame, physical px) and/or sets whether it is topmost, without
/// activating it.
pub fn place(hwnd: Hwnd, pos: Option<(i32, i32)>, topmost: Option<bool>) {
    const SWP_NOSIZE: u32 = 0x1;
    const SWP_NOMOVE: u32 = 0x2;
    const SWP_NOZORDER: u32 = 0x4;
    const SWP_NOACTIVATE: u32 = 0x10;
    let (x, y) = pos.unwrap_or_default();
    let mut flags = SWP_NOSIZE | SWP_NOACTIVATE;
    if pos.is_none() {
        flags |= SWP_NOMOVE;
    }
    let after = match topmost {
        Some(true) => -1isize, // HWND_TOPMOST
        Some(false) => -2,     // HWND_NOTOPMOST
        None => {
            flags |= SWP_NOZORDER;
            0
        }
    };
    unsafe { SetWindowPos(hwnd, after as Hwnd, x, y, 0, 0, flags) };
}

/// Settings > Accessibility > Visual effects > Animation effects. Native flyouts don't slide when it's off.
fn animations_enabled() -> bool {
    const SPI_GETCLIENTAREAANIMATION: u32 = 0x1042;
    let mut on: i32 = 1;
    unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, &mut on as *mut i32 as _, 0) };
    on != 0
}

/// CSS-style cubic-bezier(x1, y1, x2, y2) evaluated at progress `x`.
fn bezier([x1, y1, x2, y2]: [f64; 4], x: f64) -> f64 {
    let at = |a: f64, b: f64, s: f64| {
        3.0 * (1.0 - s) * (1.0 - s) * s * a + 3.0 * (1.0 - s) * s * s * b + s * s * s
    };
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..30 {
        let s = (lo + hi) / 2.0;
        if at(x1, x2, s) < x { lo = s } else { hi = s }
    }
    at(y1, y2, (lo + hi) / 2.0)
}

/// Moves the window vertically from `from` to `to` (physical y), one step per composed frame.
/// Blocks until done; stops early, returning false, once `current()` says a newer slide took over.
pub fn slide(
    hwnd: Hwnd,
    x: i32,
    from: i32,
    to: i32,
    (time, ease): Motion,
    current: impl Fn() -> bool,
) -> bool {
    let animate = animations_enabled();
    let start = Instant::now();
    loop {
        if !current() {
            return false;
        }
        let t = if animate {
            (start.elapsed().as_secs_f64() / time.as_secs_f64()).min(1.0)
        } else {
            1.0
        };
        let y = from + ((to - from) as f64 * bezier(ease, t)).round() as i32;
        place(hwnd, Some((x, y)), None);
        if t >= 1.0 {
            return true;
        }
        // Wait for the next DWM frame so each step lands on its own refresh.
        if unsafe { DwmFlush() } < 0 {
            std::thread::sleep(Duration::from_millis(8));
        }
    }
}
