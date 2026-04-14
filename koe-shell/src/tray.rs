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

    // "Launch at Startup" check menu item (Windows only)
    #[cfg(windows)]
    let autostart_item = {
        use tray_icon::menu::CheckMenuItem;
        CheckMenuItem::new("Launch at Startup", true, autostart::is_enabled(), None)
    };

    let menu = Menu::new();
    let _ = menu.append(&settings_item);
    let _ = menu.append(&reload_item);
    #[cfg(windows)]
    let _ = menu.append(&autostart_item);
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
    #[cfg(windows)]
    let autostart_id = autostart_item.id().clone();

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
                #[cfg(windows)]
                if event.id() == &autostart_id {
                    let now_checked = autostart_item.is_checked();
                    log::info!("autostart toggled: {now_checked}");
                    if now_checked {
                        autostart::enable();
                    } else {
                        autostart::disable();
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

/// Windows auto-start via the `Run` registry key.
#[cfg(windows)]
mod autostart {
    use std::ffi::c_void;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::*;

    const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
    const VALUE_NAME: &str = "Koe";

    /// Remembers the installed path so toggling off→on restores the original
    /// value instead of writing the current (possibly dev) exe path.
    static SAVED_PATH: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Read the current auto-start path from the registry, if any.
    fn read_value() -> Option<String> {
        unsafe {
            let mut hkey: *mut c_void = std::ptr::null_mut();
            if RegOpenKeyExW(
                HKEY_CURRENT_USER,
                wide(RUN_KEY).as_ptr(),
                0,
                KEY_READ,
                &mut hkey as *mut _ as *mut *mut c_void,
            ) != ERROR_SUCCESS
            {
                return None;
            }
            let mut size: u32 = 0;
            let rc = RegQueryValueExW(
                hkey,
                wide(VALUE_NAME).as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut size,
            );
            if rc != ERROR_SUCCESS || size == 0 {
                RegCloseKey(hkey);
                return None;
            }
            let mut buf = vec![0u16; (size as usize) / 2];
            let rc = RegQueryValueExW(
                hkey,
                wide(VALUE_NAME).as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut(),
                buf.as_mut_ptr() as *mut u8,
                &mut size,
            );
            RegCloseKey(hkey);
            if rc != ERROR_SUCCESS {
                return None;
            }
            // Trim trailing null
            if buf.last() == Some(&0) {
                buf.pop();
            }
            Some(String::from_utf16_lossy(&buf))
        }
    }

    /// Check whether the auto-start registry value exists.
    pub fn is_enabled() -> bool {
        read_value().is_some()
    }

    /// Enable auto-start. If a previously saved path exists, restore it;
    /// otherwise fall back to the current exe path.
    pub fn enable() {
        // Use the installed path if we remembered it, otherwise current exe.
        let path = SAVED_PATH
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| format!("\"{}\"", std::env::current_exe().unwrap_or_default().display()));
        unsafe {
            let data = wide(&path);
            let mut hkey: *mut c_void = std::ptr::null_mut();
            if RegOpenKeyExW(
                HKEY_CURRENT_USER,
                wide(RUN_KEY).as_ptr(),
                0,
                KEY_WRITE,
                &mut hkey as *mut _ as *mut *mut c_void,
            ) != ERROR_SUCCESS
            {
                log::error!("failed to open Run key for writing");
                return;
            }
            let rc = RegSetValueExW(
                hkey,
                wide(VALUE_NAME).as_ptr(),
                0,
                REG_SZ,
                data.as_ptr() as *const u8,
                (data.len() * 2) as u32,
            );
            RegCloseKey(hkey);
            if rc != ERROR_SUCCESS {
                log::error!("failed to set autostart value: {rc}");
            }
        }
    }

    /// Disable auto-start by removing the registry value.
    /// Saves the current path so `enable()` can restore it.
    pub fn disable() {
        if let Some(path) = read_value() {
            *SAVED_PATH.lock().unwrap() = Some(path);
        }
        unsafe {
            let mut hkey: *mut c_void = std::ptr::null_mut();
            if RegOpenKeyExW(
                HKEY_CURRENT_USER,
                wide(RUN_KEY).as_ptr(),
                0,
                KEY_WRITE,
                &mut hkey as *mut _ as *mut *mut c_void,
            ) != ERROR_SUCCESS
            {
                return;
            }
            RegDeleteValueW(hkey, wide(VALUE_NAME).as_ptr());
            RegCloseKey(hkey);
        }
    }
}
