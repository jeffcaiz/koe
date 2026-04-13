//! Feedback sound playback for session state changes.
//!
//! Reads the feedback config from koe-core via the key-path API
//! and plays system sounds on start/stop/error events.
//!
//! On Windows 11, uses the built-in Speech Recognition sounds
//! ("Speech On", "Speech Off", "Speech Misrecognition").

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
    use std::ptr;
    use std::thread;
    use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_FILENAME, SND_SYNC};

    /// Encode a &str as a null-terminated UTF-16 Vec.
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Play a wav file on a dedicated thread so it doesn't get cancelled
    /// by other PlaySoundW calls. SND_SYNC blocks within the thread until
    /// the sound finishes, isolating it from the main event loop.
    fn play_wav(path: &'static str) {
        thread::spawn(move || {
            let w = wide(path);
            unsafe {
                PlaySoundW(w.as_ptr(), ptr::null_mut(), SND_FILENAME | SND_SYNC);
            }
        });
    }

    pub fn play_start() {
        play_wav(r"C:\Windows\Media\Speech On.wav");
    }

    pub fn play_stop() {
        play_wav(r"C:\Windows\Media\Speech Off.wav");
    }

    pub fn play_error() {
        play_wav(r"C:\Windows\Media\Speech Misrecognition.wav");
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
