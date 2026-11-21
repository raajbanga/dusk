// No console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod flyout;
mod hdr;
mod nightlight;
mod reg;

use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::utils::config::WindowEffectsConfig;
use tauri::window::Effect;
use tauri::{
    AppHandle, Manager, PhysicalPosition, RunEvent, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
};

const PANEL: &str = "main";
const MARGIN: f64 = 12.0; // gap from screen edge, in logical px (matches Quick Settings)
/// How long a pre-warmed (hovered but not clicked) panel is kept before it is destroyed.
const PREWARM_TTL: Duration = Duration::from_secs(3);

/// The panel window only exists around the time it is used. It is created (cloaked) when the
/// pointer enters the tray icon, so WebView2's ~200 ms start-up overlaps with the user moving
/// to click; it is destroyed when closed or when the pointer leaves without clicking.
/// At idle only this small process remains: no WebView2 processes, no timers.
#[derive(Default)]
struct Shared {
    /// State of the current panel window; reset when it is destroyed.
    win: Win,
    /// Bumped on every pointer-leave, so only the latest leave's timer can destroy.
    leave_gen: u64,
    /// Bumped by every slide, so a newer one (e.g. close during open) stops the older.
    slide_gen: u64,
    /// Pointer is over the tray icon.
    hovering: bool,
    closed_at: Option<Instant>,
    light_taskbar: Option<bool>,
}

#[derive(Default)]
struct Win {
    /// A window has been requested (it may still be starting up).
    exists: bool,
    /// The user clicked: show as soon as the page is ready.
    want_open: bool,
    /// Logical size reported by the page once it has rendered.
    size: Option<(f64, f64)>,
    anchor: PhysicalPosition<f64>,
    /// On screen (uncloaked) and not closing.
    open: bool,
    /// Outer-frame (x, y) that puts the panel fully behind the taskbar.
    hidden: (i32, i32),
}

fn shared(app: &AppHandle) -> MutexGuard<'_, Shared> {
    app.state::<Mutex<Shared>>().inner().lock().unwrap()
}

fn panel(app: &AppHandle) -> Option<(WebviewWindow, flyout::Hwnd)> {
    let win = app.get_webview_window(PANEL)?;
    let hwnd = win.hwnd().ok()?.0 as _;
    Some((win, hwnd))
}

fn destroy(app: &AppHandle) {
    if let Some((win, _)) = panel(app) {
        let _ = win.destroy();
    }
}

#[derive(serde::Serialize)]
struct State {
    night_light: Option<u8>,
    brightness: Option<u8>,
    accent: Option<Accent>,
}

/// The shades of the user's accent colour Windows uses for controls such as the volume slider:
/// Light2 on dark backgrounds, Dark1 on light backgrounds.
#[derive(serde::Serialize)]
struct Accent {
    light: String,
    dark: String,
}

fn accent() -> Option<Accent> {
    let p = reg::get(
        r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent",
        "AccentPalette",
    )?;
    let hex = |i: usize| format!("#{:02x}{:02x}{:02x}", p[i * 4], p[i * 4 + 1], p[i * 4 + 2]);
    (p.len() >= 32).then(|| Accent {
        light: hex(1),
        dark: hex(4),
    })
}

#[tauri::command]
fn get_state() -> State {
    State {
        night_light: nightlight::get(),
        brightness: hdr::get(),
        accent: accent(),
    }
}

/// `setting` is a `State` field name. `commit=false` previews while dragging, `true` saves.
#[tauri::command]
fn set_value(setting: &str, value: u8, commit: bool) -> Result<(), String> {
    match setting {
        "night_light" => nightlight::set(value, commit),
        "brightness" => hdr::set(value, commit),
        _ => Err(format!("unknown setting {setting}")),
    }
}

/// Called by the page once it has rendered and measured itself.
#[tauri::command]
fn panel_ready(app: AppHandle, width: f64, height: f64) -> Result<(), String> {
    let mut s = shared(&app);
    s.win.size = Some((width, height));
    let (want_open, anchor) = (s.win.want_open, s.win.anchor);
    drop(s);
    if want_open {
        show(&app, anchor, width, height)?;
    }
    Ok(())
}

#[tauri::command]
fn close_panel(app: AppHandle) {
    close(&app);
}

/// Slides the panel vertically on its own thread, cancelling any slide still running. Closing
/// ends by destroying the window.
fn slide(app: &AppHandle, x: i32, from: i32, to: i32, opening: bool) {
    let Some((_, hwnd)) = panel(app) else { return };
    let hwnd = hwnd as usize;
    let ticket = {
        let mut s = shared(app);
        s.slide_gen += 1;
        s.slide_gen
    };
    let app = app.clone();
    std::thread::spawn(move || {
        let hwnd = hwnd as _;
        let motion = if opening { flyout::OPEN } else { flyout::CLOSE };
        let done = flyout::slide(hwnd, x, from, to, motion, || {
            shared(&app).slide_gen == ticket
        });
        if done && !opening {
            destroy(&app);
        }
    });
}

/// Slides the panel back behind the taskbar, then destroys it.
fn close(app: &AppHandle) {
    let mut s = shared(app);
    if !s.win.open {
        return;
    }
    s.win.open = false;
    s.closed_at = Some(Instant::now());
    let (x, hidden) = s.win.hidden;
    drop(s);
    if let Some((win, _)) = panel(app)
        && let Ok(pos) = win.outer_position()
    {
        slide(app, x, pos.y, hidden, false);
    }
}

/// Sizes the panel, then slides it up into the bottom-right of the work area of the monitor
/// that was clicked.
fn show(
    app: &AppHandle,
    anchor: PhysicalPosition<f64>,
    width: f64,
    height: f64,
) -> Result<(), String> {
    let (win, hwnd) = panel(app).ok_or("panel closed")?;
    let monitor = app
        .monitor_from_point(anchor.x, anchor.y)
        .ok()
        .flatten()
        .or_else(|| win.primary_monitor().ok().flatten())
        .ok_or("no monitor")?;
    let scale = monitor.scale_factor();
    let area = monitor.work_area();
    let (w, h) = (
        (width * scale).round() as i32,
        (height * scale).round() as i32,
    );
    let margin = (MARGIN * scale).round() as i32;

    win.set_size(tauri::PhysicalSize::new(w, h))
        .map_err(|e| e.to_string())?;
    let x = area.position.x + area.size.width as i32 - w - margin;
    // Fully hidden: top edge on the taskbar's top edge.
    let hidden = area.position.y + area.size.height as i32;
    {
        let s = &mut shared(app).win;
        s.open = true;
        s.hidden = (x, hidden);
    }
    flyout::place(hwnd, x, hidden);
    flyout::reveal(hwnd);
    slide(app, x, hidden, hidden - h - margin, true);
    Ok(())
}

/// Starts building the (cloaked) panel window if there isn't one.
fn ensure_window(app: &AppHandle, win: &mut Win) {
    if win.exists {
        return;
    }
    win.exists = true;
    // Must not be built on the event-loop thread: on Windows that deadlocks (tauri#4121).
    let app = app.clone();
    std::thread::spawn(move || {
        let built = WebviewWindowBuilder::new(&app, PANEL, WebviewUrl::default())
            .title("Dusk")
            .inner_size(320.0, 120.0)
            .visible(false)
            .decorations(false)
            .transparent(true)
            .resizable(false)
            .skip_taskbar(true)
            .shadow(false)
            .effects(WindowEffectsConfig {
                effects: vec![Effect::Acrylic],
                ..Default::default()
            })
            .build();
        match built.ok().and_then(|_| panel(&app)) {
            Some((_, hwnd)) => flyout::prewarm(hwnd),
            None => shared(&app).win = Win::default(),
        }
    });
}

fn on_click(app: &AppHandle, at: PhysicalPosition<f64>) {
    let mut s = shared(app);
    if s.win.open {
        drop(s);
        return close(app);
    }
    // Clicking the tray icon while open first blurs (and closes) the panel; don't immediately reopen.
    if s.closed_at
        .is_some_and(|t| t.elapsed() < Duration::from_millis(250))
    {
        return;
    }
    s.win.want_open = true;
    s.win.anchor = at;
    ensure_window(app, &mut s.win);
    if let Some((w, h)) = s.win.size {
        drop(s);
        let _ = show(app, at, w, h);
    }
}

fn on_enter(app: &AppHandle, at: PhysicalPosition<f64>) {
    let mut s = shared(app);
    s.leave_gen += 1; // cancel any pending destroy
    s.hovering = true;
    if !s.win.exists {
        s.win.anchor = at;
        ensure_window(app, &mut s.win);
    }
}

fn on_leave(app: &AppHandle) {
    let ticket = {
        let mut s = shared(app);
        s.leave_gen += 1;
        s.hovering = false;
        s.leave_gen
    };
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(PREWARM_TTL);
        let s = shared(&app);
        if s.leave_gen == ticket && !s.win.want_open {
            drop(s);
            destroy(&app);
        }
    });
}

/// Monochrome tray glyph matching the taskbar: white on a dark taskbar, black on a light one.
/// Re-checked when the pointer enters the tray icon, so a theme switch is picked up without a window or timer.
fn refresh_tray_icon(tray: &TrayIcon) {
    let light = reg::get(
        r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
        "SystemUsesLightTheme",
    )
    .is_some_and(|v| v.first() == Some(&1));
    if shared(tray.app_handle()).light_taskbar.replace(light) == Some(light) {
        return;
    }
    let png: &[u8] = if light {
        include_bytes!("../icons/tray-black.png")
    } else {
        include_bytes!("../icons/tray-white.png")
    };
    let _ = tray.set_icon(tauri::image::Image::from_bytes(png).ok());
}

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const APP_KEY: &str = r"Software\Dusk";

fn autostart_enabled() -> bool {
    reg::get(RUN_KEY, "Dusk").is_some()
}

fn set_autostart(on: bool) -> Result<(), String> {
    if on {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        reg::set_string(RUN_KEY, "Dusk", &format!("\"{}\"", exe.display()))
    } else {
        reg::delete(RUN_KEY, "Dusk")
    }
}

/// Release builds turn autostart on the first time they run; after that the tray menu decides.
/// If it's on, re-point it at this exe in case the app moved.
fn init_autostart() {
    if cfg!(debug_assertions) {
        return;
    }
    if reg::get(APP_KEY, "AutostartInitialized").is_none() {
        if set_autostart(true).is_ok() {
            let _ = reg::set(
                APP_KEY,
                "AutostartInitialized",
                reg::REG_DWORD,
                &1u32.to_le_bytes(),
            );
        }
    } else if autostart_enabled() {
        let _ = set_autostart(true);
    }
}

fn main() {
    tauri::Builder::default()
        .manage(Mutex::new(Shared::default()))
        .invoke_handler(tauri::generate_handler![
            get_state,
            set_value,
            panel_ready,
            close_panel
        ])
        .setup(|app| {
            init_autostart();

            let autostart = CheckMenuItem::with_id(
                app,
                "autostart",
                "Start with Windows",
                true,
                autostart_enabled(),
                None::<&str>,
            )?;
            let display =
                MenuItem::with_id(app, "display", "Display settings", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit Dusk", true, None::<&str>)?;
            let menu_item = autostart.clone();
            let tray = TrayIconBuilder::with_id("tray")
                .tooltip("Dusk")
                .menu(&Menu::with_items(
                    app,
                    &[
                        &display,
                        &autostart,
                        &PredefinedMenuItem::separator(app)?,
                        &quit,
                    ],
                )?)
                .show_menu_on_left_click(false)
                .on_menu_event(move |app, e| match e.id().as_ref() {
                    // The item toggles its own check mark; keep it in sync with the registry.
                    "autostart" => {
                        let on = menu_item.is_checked().unwrap_or(false);
                        if set_autostart(on).is_err() {
                            let _ = menu_item.set_checked(autostart_enabled());
                        }
                    }
                    "display" => {
                        let _ = std::process::Command::new("explorer")
                            .arg("ms-settings:display")
                            .spawn();
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(move |tray, e| match e {
                    TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        position,
                        ..
                    } => on_click(tray.app_handle(), position),
                    TrayIconEvent::Enter { position, .. } => {
                        refresh_tray_icon(tray);
                        // Picks up changes made outside the app (e.g. `reg delete`).
                        let _ = autostart.set_checked(autostart_enabled());
                        on_enter(tray.app_handle(), position);
                    }
                    TrayIconEvent::Leave { .. } => on_leave(tray.app_handle()),
                    _ => {}
                })
                .build(app)?;
            refresh_tray_icon(&tray);
            Ok(())
        })
        .on_window_event(|win, e| match e {
            // No-op unless open: a pre-warmed window can report a focus loss.
            WindowEvent::Focused(false) => close(win.app_handle()),
            WindowEvent::Destroyed => {
                let mut s = shared(win.app_handle());
                s.win = Win::default();
                // Closed while the pointer is still on the icon (e.g. clicked it to close):
                // pre-warm the next panel straight away so reopening is instant.
                if s.hovering {
                    ensure_window(win.app_handle(), &mut s.win);
                }
            }
            _ => {}
        })
        .build(tauri::generate_context!())
        .expect("error while building Dusk")
        .run(|_, e| {
            // Keep running in the tray after the panel (the only window) is destroyed.
            if let RunEvent::ExitRequested {
                code: None, api, ..
            } = e
            {
                api.prevent_exit();
            }
        });
}
