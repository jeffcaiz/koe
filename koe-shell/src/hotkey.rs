//! Global hotkey via raw key event listening.
//!
//! Uses rdev to listen for key press/release events. Supports single
//! modifier keys as hotkeys. Only triggers on key release to avoid
//! repeat-firing from long press.

use koe_core::api::{self, SessionContext, SessionMode};
use rdev::{listen, Event, EventType, Key};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;

static RECORDING: AtomicBool = AtomicBool::new(false);
static SESSION_TOKEN: AtomicU64 = AtomicU64::new(1);

/// Start the key listener in a background thread.
pub fn init() {
    thread::spawn(|| {
        log::info!("hotkey registered: Right Alt / AltGr (toggle mode)");
        if let Err(e) = listen(on_event) {
            log::error!("key listener failed: {e:?}");
        }
    });
}

/// No-op for compatibility with tray.rs poll loop.
pub fn poll_events() {}

fn is_trigger_key(key: &Key) -> bool {
    // Right Alt shows up as AltGr on Windows
    matches!(key, Key::AltGr)
}

fn on_event(event: Event) {
    // Trigger on key RELEASE only — avoids repeat-firing from long press
    if let EventType::KeyRelease(key) = event.event_type {
        if is_trigger_key(&key) {
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
