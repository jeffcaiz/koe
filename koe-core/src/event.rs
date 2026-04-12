//! Rust-native event channel for koe-core consumers.
//!
//! This provides an alternative to the C FFI callback mechanism.
//! Both systems coexist: C callbacks (for the macOS ObjC app) and
//! the event channel (for Rust consumers like koe-shell) fire
//! independently.

use std::sync::Mutex;
use tokio::sync::mpsc;

/// Events emitted by koe-core during session lifecycle.
#[derive(Debug, Clone)]
pub enum KoeEvent {
    /// Session is ready (recording can begin)
    SessionReady { token: u64 },
    /// Session encountered an error
    SessionError { token: u64, message: String },
    /// Non-fatal warning
    SessionWarning { token: u64, message: String },
    /// Session state changed (for UI status updates)
    /// state: "connecting_asr", "recording_hold", "recording_toggle",
    ///        "finalizing_asr", "correcting", "preparing_paste",
    ///        "cancelled", "failed", "idle"
    StateChanged { token: u64, state: String },
    /// Interim (partial) ASR text during recording
    InterimText { token: u64, text: String },
    /// Final ASR text before LLM correction
    AsrFinalText { token: u64, text: String },
    /// Final corrected text ready for pasting
    FinalText { token: u64, text: String },
    /// Rewrite result from a prompt template
    RewriteText { token: u64, text: String },
    /// Log event (not session-scoped)
    /// level: 0=error, 1=warn, 2=info, 3=debug
    Log { level: i32, message: String },
}

static EVENT_TX: Mutex<Option<mpsc::UnboundedSender<KoeEvent>>> = Mutex::new(None);

/// Register an event channel. Events will be sent through this channel
/// in addition to any registered C FFI callbacks.
pub fn register_event_channel(tx: mpsc::UnboundedSender<KoeEvent>) {
    let mut slot = EVENT_TX.lock().unwrap();
    *slot = Some(tx);
}

/// Remove the registered event channel.
pub fn unregister_event_channel() {
    let mut slot = EVENT_TX.lock().unwrap();
    *slot = None;
}

/// Send an event through the channel if one is registered.
pub(crate) fn emit(event: KoeEvent) {
    let tx = EVENT_TX.lock().unwrap();
    if let Some(ref sender) = *tx {
        let _ = sender.send(event);
    }
}
