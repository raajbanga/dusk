// No console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod backdrop;
mod hdr;
mod nightlight;
mod reg;

use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::utils::config::WindowEffectsConfig;
use tauri::window::Effect;
use tauri::{
    AppHandle, Manager, PhysicalPosition, Position, RunEvent, WebviewUrl, WebviewWindowBuilder,
    WindowEvent,
};

const PANEL: &str = "main";
const MARGIN: f64 = 12.0; // gap from screen edge, in logical px (matches Quick Settings)
/// How long a pre-warmed (hovered but not clicked) panel is kept before it is destroyed.
const PREWARM_TTL: Duration = Duration::from_secs(3);

/// The panel window only exists around the time it is used. It is created (hidden) when the
/// pointer enters the tray icon, so WebView2's ~200 ms start-up overlaps with the user moving
/// to click; it is destroyed when closed or when the pointer leaves without clicking.
/// At idle only this small process remains: no WebView2 processes, no timers.
#[derive(Default)]
struct Panel {
    life: Mutex<Life>,
    closed_at: Mutex<Option<Instant>>,
    light_taskbar: Mutex<Option<bool>>,
}

#[derive(Default)]
struct Life {
    /// A window has been requested (it may still be starting up).
    exists: bool,
    /// The user clicked: show as soon as the page is ready.
    want_open: bool,
    /// Logical size reported by the page once it has rendered.
    size: Option<(f64, f64)>,
    anchor: PhysicalPosition<f64>,
    /// Bumped on every pointer-leave, so only the latest leave's timer can destroy.
    leave_gen: u64,
    /// Pointer is over the tray icon.
    hovering: bool,
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

#[tauri::command]
fn set_night_light(value: u8, commit: bool) -> Result<(), String> {
    nightlight::set(value, commit)
}

#[tauri::command]
fn set_brightness(value: u8, commit: bool) -> Result<(), String> {
    hdr::set(value, commit)
}

/// Called by the page once it has rendered and measured itself.
#[tauri::command]
fn panel_ready(app: AppHandle, width: f64, height: f64) -> Result<(), String> {
    let state = app.state::<Panel>();
    let mut life = state.life.lock().unwrap();
    life.size = Some((width, height));
    if life.want_open {
        let anchor = life.anchor;
        drop(life);
        show(&app, anchor, width, height)?;
    }
    Ok(())
}

/// Sizes the panel and pins it to the bottom-right of the work area of the monitor that was clicked.
fn show(
    app: &AppHandle,
    anchor: PhysicalPosition<f64>,
    width: f64,
    height: f64,
) -> Result<(), String> {
    let win = app.get_webview_window(PANEL).ok_or("panel closed")?;
    let monitor = app
        .monitor_from_point(anchor.x, anchor.y)
        .ok()
        .flatten()
        .or_else(|| win.primary_monitor().ok().flatten())
        .ok_or("no monitor")?;
    let scale = monitor.scale_factor();
    let area = monitor.work_area();
    let (w, h) = ((width * scale).round(), (height * scale).round());
    let x = area.position.x as f64 + area.size.width as f64 - w - MARGIN * scale;
    let y = area.position.y as f64 + area.size.height as f64 - h - MARGIN * scale;

    win.set_size(tauri::PhysicalSize::new(w, h))
        .map_err(|e| e.to_string())?;
    // set_position moves the outer frame, which includes Windows' invisible resize border
    // (kept for the native shadow). Offset by it so the visible panel lands exactly on (x, y).
    let border = match (win.outer_position(), win.inner_position()) {
        (Ok(o), Ok(i)) => (i.x - o.x, i.y - o.y),
        _ => (0, 0),
    };
    win.set_position(Position::Physical(PhysicalPosition::new(
        x as i32 - border.0,
        y as i32 - border.1,
    )))
    .map_err(|e| e.to_string())?;
    win.show().map_err(|e| e.to_string())?;
    win.set_focus().map_err(|e| e.to_string())
}

/// Starts building the (hidden) panel window if there isn't one.
fn ensure_window(app: &AppHandle, life: &mut Life) {
    if life.exists {
        return;
    }
    life.exists = true;
    life.size = None;
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
            .always_on_top(true)
            .shadow(true)
            .effects(WindowEffectsConfig {
                effects: vec![Effect::Acrylic],
                ..Default::default()
            })
            .build();
        match built {
            Ok(win) => {
                if let Ok(hwnd) = win.hwnd() {
                    backdrop::apply(hwnd.0 as _);
                }
            }
            Err(_) => {
                let state = app.state::<Panel>();
                let mut life = state.life.lock().unwrap();
                life.exists = false;
                life.want_open = false;
            }
        }
    });
}

fn on_click(app: &AppHandle, at: PhysicalPosition<f64>) {
    let state = app.state::<Panel>();
    let mut life = state.life.lock().unwrap();
    if let Some(win) = app.get_webview_window(PANEL)
        && win.is_visible().unwrap_or(false)
    {
        drop(life);
        let _ = win.destroy();
        return;
    }
    // Clicking the tray icon while open first blurs (and closes) the panel; don't immediately reopen.
    if state
        .closed_at
        .lock()
        .unwrap()
        .is_some_and(|t| t.elapsed() < Duration::from_millis(250))
    {
        return;
    }
    life.want_open = true;
    life.anchor = at;
    ensure_window(app, &mut life);
    if let Some((w, h)) = life.size {
        drop(life);
        let _ = show(app, at, w, h);
    }
}

fn on_enter(app: &AppHandle, at: PhysicalPosition<f64>) {
    let state = app.state::<Panel>();
    let mut life = state.life.lock().unwrap();
    life.leave_gen += 1; // cancel any pending destroy
    life.hovering = true;
    if !life.exists {
        life.anchor = at;
        ensure_window(app, &mut life);
    }
}

fn on_leave(app: &AppHandle) {
    let ticket = {
        let state = app.state::<Panel>();
        let mut life = state.life.lock().unwrap();
        life.leave_gen += 1;
        life.hovering = false;
        life.leave_gen
    };
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(PREWARM_TTL);
        let state = app.state::<Panel>();
        let life = state.life.lock().unwrap();
        if life.leave_gen == ticket && !life.want_open {
            drop(life);
            if let Some(win) = app.get_webview_window(PANEL) {
                let _ = win.destroy();
            }
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
    let state = tray.app_handle().state::<Panel>();
    let mut current = state.light_taskbar.lock().unwrap();
    if *current == Some(light) {
        return;
    }
    *current = Some(light);
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
        .manage(Panel::default())
        .invoke_handler(tauri::generate_handler![
            get_state,
            set_night_light,
            set_brightness,
            panel_ready
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
            let quit = MenuItem::with_id(app, "quit", "Quit Dusk", true, None::<&str>)?;
            let menu_item = autostart.clone();
            let tray = TrayIconBuilder::with_id("tray")
                .tooltip("Dusk")
                .menu(&Menu::with_items(
                    app,
                    &[&autostart, &PredefinedMenuItem::separator(app)?, &quit],
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
            // Only once shown: a hidden, pre-warmed window can report a focus loss.
            WindowEvent::Focused(false) if win.is_visible().unwrap_or(false) => {
                *win.state::<Panel>().closed_at.lock().unwrap() = Some(Instant::now());
                let _ = win.destroy();
            }
            WindowEvent::Destroyed => {
                let state = win.state::<Panel>();
                let mut life = state.life.lock().unwrap();
                *life = Life {
                    leave_gen: life.leave_gen,
                    hovering: life.hovering,
                    ..Life::default()
                };
                // Closed while the pointer is still on the icon (e.g. clicked it to close):
                // pre-warm the next panel straight away so reopening is instant.
                if life.hovering {
                    ensure_window(win.app_handle(), &mut life);
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
