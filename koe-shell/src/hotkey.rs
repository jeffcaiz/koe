//! Global hotkey via raw key event listening.
//!
//! Uses rdev to listen for key press/release events. Supports single
//! modifier keys (e.g. Right Alt) as hotkeys.

use koe_core::api::{self, SessionContext, SessionMode};
use rdev::{listen, Event, EventType, Key};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;

static RECORDING: AtomicBool = AtomicBool::new(false);
static SESSION_TOKEN: AtomicU64 = AtomicU64::new(1);

// The trigger key — MVP: hardcoded to Right Alt
const TRIGGER_KEY: Key = Key::Alt;

/// Start the key listener in a background thread.
/// Must be called before the main event loop.
pub fn init() {
    thread::spawn(|| {
        log::info!("hotkey registered: Right Alt (toggle mode)");
        if let Err(e) = listen(on_event) {
            log::error!("key listener failed: {e:?}");
        }
    });
}

/// No-op for compatibility with tray.rs poll loop.
pub fn poll_events() {}

fn on_event(event: Event) {
    // Only react to key press, not release
    if let EventType::KeyPress(key) = event.event_type {
        if key == TRIGGER_KEY {
            toggle_session();
        }
    }
}

fn toggle_session() {
    let was_recording = RECORDING.fetch_xor(true, Ordering::SeqCst);

    if !was_recording {
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
        crate::audio::stop();

        if let Err(e) = api::session_end() {
            log::error!("session_end failed: {e}");
        }

        log::info!("recording stopped, processing...");
    }
}
