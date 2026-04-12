//! System tray icon and menu.

use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::TrayIconBuilder;

use std::sync::Mutex;

static STATUS: Mutex<String> = Mutex::new(String::new());

/// Update the current status displayed in the tray tooltip.
pub fn update_status(state: &str) {
    let mut s = STATUS.lock().unwrap();
    *s = state.to_string();
    // TODO: update tray icon based on state (idle/recording/processing)
}

/// Run the main event loop. This blocks the main thread.
/// Handles tray menu events and hotkey polling.
pub fn run_event_loop() {
    // Build tray menu
    let quit_item = MenuItem::new("Quit", true, None);
    let reload_item = MenuItem::new("Reload Config", true, None);

    let menu = Menu::new();
    let _ = menu.append(&reload_item);
    let _ = menu.append(&quit_item);

    // Create tray icon with a simple built-in icon
    let icon = load_default_icon();
    let _tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Koe - Voice Input")
        .with_icon(icon)
        .build()
        .expect("failed to create tray icon");

    let quit_id = quit_item.id().clone();
    let reload_id = reload_item.id().clone();

    log::info!("tray icon created");

    // Main event loop: poll hotkey events and tray menu events
    loop {
        // Poll hotkey events
        crate::hotkey::poll_events();

        // Poll tray menu events
        if let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id() == &quit_id {
                log::info!("quit requested from tray");
                break;
            } else if event.id() == &reload_id {
                log::info!("reload config requested from tray");
                if let Err(e) = koe_core::api::reload_config() {
                    log::error!("reload failed: {e}");
                }
            }
        }

        // Sleep briefly to avoid busy-looping
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Create a minimal default icon (1x1 RGBA pixel).
fn load_default_icon() -> tray_icon::Icon {
    // A tiny 16x16 blue square as placeholder icon
    let size = 16u32;
    let rgba: Vec<u8> = vec![0x33, 0x99, 0xFF, 0xFF].repeat((size * size) as usize);
    tray_icon::Icon::from_rgba(rgba, size, size).expect("failed to create icon")
}
