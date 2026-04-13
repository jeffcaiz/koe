//! Feedback sound playback for session state changes.
//!
//! Reads the feedback config from koe-core via the key-path API
//! and plays system sounds on start/stop/error events.

use koe_core::api;

/// Read a boolean feedback config value.
fn feedback_enabled(key: &str) -> bool {
    api::config_get(key)
        .map(|v| v == "true")
        .unwrap_or(false)
}

/// Handle a state change event and play appropriate feedback sound.
pub fn on_state_changed(state: &str) {
    match state {
        "recording_hold" | "recording_toggle" => {
            if feedback_enabled("feedback.start_sound") {
                platform::play_start();
            }
        }
        "idle" | "cancelled" => {
            if feedback_enabled("feedback.stop_sound") {
                platform::play_stop();
            }
        }
        "failed" => {
            if feedback_enabled("feedback.error_sound") {
                platform::play_error();
            }
        }
        _ => {}
    }
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBeep, MB_ICONASTERISK, MB_ICONHAND, MB_OK};

    pub fn play_start() {
        unsafe { MessageBeep(MB_OK); }
    }

    pub fn play_stop() {
        unsafe { MessageBeep(MB_ICONASTERISK); }
    }

    pub fn play_error() {
        unsafe { MessageBeep(MB_ICONHAND); }
    }
}

#[cfg(not(windows))]
mod platform {
    pub fn play_start() {
        log::debug!("feedback: start sound (no-op on this platform)");
    }

    pub fn play_stop() {
        log::debug!("feedback: stop sound (no-op on this platform)");
    }

    pub fn play_error() {
        log::debug!("feedback: error sound (no-op on this platform)");
    }
}
