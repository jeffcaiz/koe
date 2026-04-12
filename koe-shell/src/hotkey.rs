//! Global hotkey registration and session lifecycle management.
//!
//! MVP: toggle mode only (press once to start, press again to stop).

use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState, hotkey::HotKey};
use koe_core::api::{self, SessionContext, SessionMode};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static RECORDING: AtomicBool = AtomicBool::new(false);
static SESSION_TOKEN: AtomicU64 = AtomicU64::new(1);

/// Initialize the global hotkey system.
/// Must be called from the main thread before the event loop starts.
pub fn init() {
    let manager = GlobalHotKeyManager::new().expect("failed to create hotkey manager");

    // MVP: Left Alt as toggle hotkey.
    // TODO: read from config and map key names to platform codes
    let hotkey = HotKey::new(None, global_hotkey::hotkey::Code::AltLeft);

    manager
        .register(hotkey)
        .expect("failed to register hotkey");

    log::info!("hotkey registered: Left Alt (toggle mode)");

    // Leak the manager so it lives for the duration of the process.
    // GlobalHotKeyManager unregisters hotkeys on drop, so we need it alive.
    std::mem::forget(manager);
}

/// Poll and handle hotkey events. Call this from the main event loop.
pub fn poll_events() {
    if let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
        if event.state() == HotKeyState::Pressed {
            toggle_session();
        }
    }
}

fn toggle_session() {
    let was_recording = RECORDING.fetch_xor(true, Ordering::SeqCst);

    if !was_recording {
        // Start recording
        let token = SESSION_TOKEN.fetch_add(1, Ordering::SeqCst);
        let ctx = SessionContext {
            mode: SessionMode::Toggle,
            session_token: token,
            llm_invert_modifier_active: false,
        };

        if let Err(e) = api::session_begin(ctx) {
            log::error!("session_begin failed: {e}");
            RECORDING.store(false, Ordering::SeqCst);
            return;
        }

        if let Err(e) = crate::audio::start() {
            log::error!("audio start failed: {e}");
            let _ = api::session_cancel();
            RECORDING.store(false, Ordering::SeqCst);
            return;
        }

        log::info!("recording started (token={token})");
    } else {
        // Stop recording
        crate::audio::stop();

        if let Err(e) = api::session_end() {
            log::error!("session_end failed: {e}");
        }

        log::info!("recording stopped, processing...");
    }
}
