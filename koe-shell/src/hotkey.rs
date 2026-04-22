//! Global hotkey with Typeless-style hold+toggle combo mode.
//!
//! - Short press (tap): toggle mode — tap to start, tap again to stop
//! - Long press (hold): hold mode — press to start, release to stop
//! - Threshold: 300ms separates a "tap" from a "hold"
//!
//! Uses rdev for raw key event listening, supporting single modifier keys.

use koe_core::api::{self, SessionContext, SessionMode};
use rdev::{grab, Event, EventType, Key};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

static SESSION_TOKEN: AtomicU64 = AtomicU64::new(1);
static STATE: Mutex<HotkeyState> = Mutex::new(HotkeyState::new());

const HOLD_THRESHOLD: Duration = Duration::from_millis(300);

/// Internal state machine for the combo hotkey.
///
/// ```text
/// Idle ──KeyPress──→ Pressed (start recording, record time)
///                         │
///                    KeyRelease
///                    ┌────┴────┐
///               short press   long press
///                    │           │
///              ToggleRecording   Idle (stop recording)
///                    │
///               KeyPress (2nd tap)
///                    │
///                   Idle (stop recording)
/// ```
struct HotkeyState {
    phase: Phase,
    trigger_key: Key,
    key_physically_down: bool,
    press_start: Option<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Not recording
    Idle,
    /// Key is held down, recording started
    Pressed,
    /// Key was tapped (short press), recording continues, waiting for 2nd tap
    ToggleRecording,
}

impl HotkeyState {
    const fn new() -> Self {
        Self {
            phase: Phase::Idle,
            trigger_key: Key::AltGr, // default, overwritten by init()
            key_physically_down: false,
            press_start: None,
        }
    }
}

/// Initialize the hotkey system. Reads trigger_key from config.
pub fn init() {
    // Read config
    let hotkey_config = koe_core::api::hotkey_config();
    let key = map_config_key(&hotkey_config.trigger_key);
    let key_name = hotkey_config.trigger_key.clone();

    {
        let mut state = STATE.lock().unwrap();
        state.trigger_key = key;
    }

    thread::spawn(move || {
        log::info!("hotkey registered: {key_name} → {key:?} (combo mode: tap=toggle, hold=release-to-stop)");
        if let Err(e) = grab(on_event_grab) {
            log::error!("key listener failed: {e:?}");
        }
    });
}

/// No-op — rdev runs its own event loop in the background thread.
pub fn poll_events() {}

/// Grab callback: returns `None` to suppress the trigger key (preventing it
/// from reaching other apps like Chrome), `Some(event)` to pass through.
fn on_event_grab(event: Event) -> Option<Event> {
    let mut state = STATE.lock().unwrap();
    let trigger = state.trigger_key;

    match event.event_type {
        EventType::KeyPress(key) if key == trigger => {
            // Filter out key repeat events (key already physically down)
            if state.key_physically_down {
                return None; // suppress repeats too
            }
            state.key_physically_down = true;

            match state.phase {
                Phase::Idle => {
                    // Start recording
                    state.press_start = Some(Instant::now());
                    state.phase = Phase::Pressed;
                    drop(state); // release lock before calling koe-core
                    start_recording();
                }
                Phase::ToggleRecording => {
                    // 2nd tap — stop recording
                    state.phase = Phase::Idle;
                    state.press_start = None;
                    drop(state);
                    stop_recording();
                }
                Phase::Pressed => {
                    // shouldn't happen (filtered by key_physically_down), ignore
                }
            }
            None // suppress trigger key from reaching other apps
        }
        EventType::KeyRelease(key) if key == trigger => {
            state.key_physically_down = false;

            match state.phase {
                Phase::Pressed => {
                    let duration = state
                        .press_start
                        .map(|t| t.elapsed())
                        .unwrap_or(Duration::ZERO);

                    if duration >= HOLD_THRESHOLD {
                        // Long press — stop recording (hold mode)
                        state.phase = Phase::Idle;
                        state.press_start = None;
                        drop(state);
                        stop_recording();
                    } else {
                        // Short press — switch to toggle mode, keep recording
                        state.phase = Phase::ToggleRecording;
                        drop(state);
                        log::info!("toggle mode: tap again to stop");
                    }
                }
                _ => {
                    // Release in other states — ignore
                }
            }
            None // suppress trigger key release too
        }
        _ => Some(event), // pass all other keys through
    }
}

fn start_recording() {
    // 1. Create session (creates audio channel)
    let token = SESSION_TOKEN.fetch_add(1, Ordering::SeqCst);
    let ctx = SessionContext {
        mode: SessionMode::Toggle,
        session_token: token,
    };

    if let Err(e) = api::session_begin(ctx) {
        log::error!("session_begin failed: {e}");
        reset_to_idle();
        return;
    }

    // 2. Open the audio gate — instant, stream is already running
    crate::audio::start();

    log::info!("recording started (token={token})");
}

fn stop_recording() {
    crate::audio::stop();
    if let Err(e) = api::session_end() {
        log::error!("session_end failed: {e}");
    }
    log::info!("recording stopped, processing...");
}

fn reset_to_idle() {
    let mut state = STATE.lock().unwrap();
    state.phase = Phase::Idle;
    state.press_start = None;
}

/// Map a config key name to an rdev Key.
/// Supports macOS-style names (for config compatibility) and common key names.
fn map_config_key(name: &str) -> Key {
    match name.to_lowercase().as_str() {
        // Modifier keys
        "alt" | "left_alt" | "left_option" | "option" => Key::Alt,
        "right_alt" | "right_option" | "altgr" => Key::AltGr,
        "control" | "left_control" | "ctrl" => Key::ControlLeft,
        "right_control" | "right_ctrl" => Key::ControlRight,
        "shift" | "left_shift" => Key::ShiftLeft,
        "right_shift" => Key::ShiftRight,
        "meta" | "super" | "left_command" | "command" | "win" => Key::MetaLeft,
        "right_command" | "right_win" => Key::MetaRight,
        "caps_lock" | "capslock" => Key::CapsLock,

        // Function keys
        "f1" => Key::F1,
        "f2" => Key::F2,
        "f3" => Key::F3,
        "f4" => Key::F4,
        "f5" => Key::F5,
        "f6" => Key::F6,
        "f7" => Key::F7,
        "f8" => Key::F8,
        "f9" => Key::F9,
        "f10" => Key::F10,
        "f11" => Key::F11,
        "f12" => Key::F12,

        // Common keys
        "space" => Key::Space,
        "escape" | "esc" => Key::Escape,
        "tab" => Key::Tab,
        "backquote" | "`" => Key::BackQuote,

        // Default fallback
        other => {
            log::warn!("unknown hotkey '{other}', falling back to AltGr (Right Alt)");
            Key::AltGr
        }
    }
}
