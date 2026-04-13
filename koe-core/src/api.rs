//! Public Rust API for koe-core.
//!
//! This module provides a safe, idiomatic Rust interface to koe-core,
//! intended for Rust consumers like koe-shell. It uses the same global
//! Core state as the C FFI but avoids unsafe code and C string conversions.

use crate::config::HotkeySection;
use crate::event::{self, KoeEvent};
use crate::ffi::SPSessionMode;
use crate::CORE;

use std::ffi::CString;
use tokio::sync::mpsc;

/// Rust-native session context (replaces the C FFI `SPSessionContext`).
pub struct SessionContext {
    pub mode: SessionMode,
    pub session_token: u64,
}

/// Session mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMode {
    Hold,
    Toggle,
}

impl From<SessionMode> for SPSessionMode {
    fn from(mode: SessionMode) -> Self {
        match mode {
            SessionMode::Hold => SPSessionMode::Hold,
            SessionMode::Toggle => SPSessionMode::Toggle,
        }
    }
}

/// Initialize koe-core with an event channel for receiving events.
/// Call this once at startup.
pub fn create(event_tx: mpsc::UnboundedSender<KoeEvent>) -> Result<(), String> {
    event::register_event_channel(event_tx);

    let result = unsafe { crate::sp_core_create(std::ptr::null()) };
    if result == 0 {
        Ok(())
    } else {
        Err("failed to initialize koe-core".into())
    }
}

/// Shut down koe-core and release all resources.
pub fn destroy() {
    crate::sp_core_destroy();
    event::unregister_event_channel();
}

/// Begin a voice input session.
pub fn session_begin(ctx: SessionContext) -> Result<(), String> {
    let ffi_ctx = crate::ffi::SPSessionContext {
        mode: ctx.mode.into(),
        frontmost_bundle_id: std::ptr::null(),
        frontmost_pid: 0,
        session_token: ctx.session_token,
    };

    let result = crate::sp_core_session_begin(ffi_ctx);
    if result == 0 {
        Ok(())
    } else {
        Err("session_begin failed".into())
    }
}

/// Push audio data into the current session.
/// `pcm_bytes` should be PCM16 LE, 16kHz, mono.
pub fn push_audio(pcm_bytes: &[u8]) -> Result<(), String> {
    if pcm_bytes.is_empty() {
        return Ok(());
    }

    let result = unsafe {
        crate::sp_core_push_audio(pcm_bytes.as_ptr(), pcm_bytes.len() as u32, 0)
    };
    if result == 0 {
        Ok(())
    } else {
        Err("push_audio failed".into())
    }
}

/// End the current session (triggers ASR finalization + LLM correction).
pub fn session_end() -> Result<(), String> {
    let result = crate::sp_core_session_end();
    if result == 0 {
        Ok(())
    } else {
        Err("session_end failed".into())
    }
}

/// Cancel the current session without producing output.
pub fn session_cancel() -> Result<(), String> {
    let result = crate::sp_core_session_cancel();
    if result == 0 {
        Ok(())
    } else {
        Err("session_cancel failed".into())
    }
}

/// Reload configuration, dictionary, and prompts from disk.
pub fn reload_config() -> Result<(), String> {
    let result = crate::sp_core_reload_config();
    if result == 0 {
        Ok(())
    } else {
        Err("reload_config failed".into())
    }
}

/// Get the current hotkey configuration.
pub fn hotkey_config() -> HotkeySection {
    let global = CORE.lock().unwrap();
    if let Some(ref core) = *global {
        core.config.hotkey.clone()
    } else {
        HotkeySection::default()
    }
}

/// Get the current trigger mode ("hold" or "toggle").
pub fn trigger_mode() -> String {
    let global = CORE.lock().unwrap();
    if let Some(ref core) = *global {
        core.config.hotkey.trigger_mode.clone()
    } else {
        "hold".into()
    }
}

/// Get a config value by dot-separated key path.
pub fn config_get(key: &str) -> Option<String> {
    let c_key = CString::new(key).ok()?;
    let ptr = unsafe { crate::sp_config_get(c_key.as_ptr()) };
    if ptr.is_null() {
        return None;
    }
    let result = unsafe { std::ffi::CStr::from_ptr(ptr) }
        .to_str()
        .ok()
        .map(|s| s.to_string());
    unsafe { crate::sp_core_free_string(ptr) };
    result
}

/// Set a config value by dot-separated key path.
pub fn config_set(key: &str, value: &str) -> Result<(), String> {
    let c_key = CString::new(key).map_err(|e| e.to_string())?;
    let c_value = CString::new(value).map_err(|e| e.to_string())?;
    let result = unsafe { crate::sp_config_set(c_key.as_ptr(), c_value.as_ptr()) };
    if result == 0 {
        Ok(())
    } else {
        Err("config_set failed".into())
    }
}
