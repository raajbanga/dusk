//! Make the panel appear in one frame: switch off DWM's show animation for this window
//! (the native Windows 11 acrylic backdrop itself is applied by Tauri's window effects).

use std::ffi::c_void;

const DWMWA_TRANSITIONS_FORCEDISABLED: u32 = 3;
const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
const DWMWCP_ROUND: u32 = 2;

#[link(name = "dwmapi")]
unsafe extern "system" {
    fn DwmSetWindowAttribute(hwnd: *mut c_void, attr: u32, value: *const c_void, size: u32) -> i32;
}

/// Apply before the window is first shown.
pub fn apply(hwnd: *mut c_void) {
    unsafe {
        let on: u32 = 1;
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_TRANSITIONS_FORCEDISABLED,
            &on as *const _ as _,
            4,
        );
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &DWMWCP_ROUND as *const _ as _,
            4,
        );
    }
}
