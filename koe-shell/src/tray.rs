//! System tray icon and menu.

use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::TrayIconBuilder;

use std::sync::Mutex;

static STATUS: Mutex<String> = Mutex::new(String::new());

/// Update the current status displayed in the tray tooltip.
pub fn update_status(state: &str) {
    let mut s = STATUS.lock().unwrap();
    *s = state.to_string();
}

/// Run the main event loop. This blocks the main thread.
/// Handles tray menu events and pumps Win32 messages.
pub fn run_event_loop() {
    // Build tray menu
    let settings_item = MenuItem::new("Settings...", true, None);
    let reload_item = MenuItem::new("Reload Config", true, None);
    let quit_item = MenuItem::new("Quit", true, None);

    let menu = Menu::new();
    let _ = menu.append(&settings_item);
    let _ = menu.append(&reload_item);
    let _ = menu.append(&quit_item);

    // On Linux, GTK must be initialized before creating the tray icon
    #[cfg(target_os = "linux")]
    if let Err(e) = gtk::init() {
        log::warn!("GTK init failed: {e} — running without system tray");
        // Fall through to headless event loop
        loop {
            crate::hotkey::poll_events();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    // Create tray icon (optional — may fail on headless / missing libs)
    let icon = load_default_icon();
    let _tray = match TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Koe - Voice Input")
        .with_icon(icon)
        .build()
    {
        Ok(t) => {
            log::info!("tray icon created");
            Some(t)
        }
        Err(e) => {
            log::warn!("tray icon unavailable: {e} — running without system tray");
            None
        }
    };

    let settings_id = settings_item.id().clone();
    let quit_id = quit_item.id().clone();
    let reload_id = reload_item.id().clone();

    // Main event loop
    loop {
        // Pump Win32 messages — required for tray icon to respond to clicks
        #[cfg(windows)]
        pump_win32_messages();

        // Poll hotkey events
        crate::hotkey::poll_events();

        // Poll tray menu events (only meaningful when tray is active)
        if _tray.is_some() {
            if let Ok(event) = MenuEvent::receiver().try_recv() {
                if event.id() == &settings_id {
                    log::info!("opening settings in browser");
                    crate::settings::open_in_browser();
                } else if event.id() == &quit_id {
                    log::info!("quit requested from tray");
                    break;
                } else if event.id() == &reload_id {
                    log::info!("reload config requested from tray");
                    if let Err(e) = koe_core::api::reload_config() {
                        log::error!("reload failed: {e}");
                    }
                }
            }
        }

        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Pump pending Win32 messages so the tray icon's hidden window can process clicks.
#[cfg(windows)]
fn pump_win32_messages() {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    unsafe {
        let mut msg = std::mem::zeroed();
        while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Load the embedded Koe app icon (32x32 RGBA).
fn load_default_icon() -> tray_icon::Icon {
    let rgba = include_bytes!("tray_icon.bin");
    tray_icon::Icon::from_rgba(rgba.to_vec(), 32, 32).expect("failed to create icon")
}
