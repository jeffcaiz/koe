//! Input device preference persistence.
//!
//! Stores the user's microphone choice in `config_dir()/input-device.txt`.
//! Content is either `"auto"` or the exact device name string.

use std::path::PathBuf;

fn pref_path() -> PathBuf {
    koe_core::config::config_dir().join("input-device.txt")
}

/// Load the saved device preference. Returns `None` if unset.
pub fn load() -> Option<String> {
    std::fs::read_to_string(pref_path())
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Save the device preference (`"auto"` or a device name).
pub fn save(pref: &str) {
    let path = pref_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&path, pref) {
        log::error!("failed to save device preference: {e}");
    }
}

/// Returns `true` if the preference is "auto" or unset.
pub fn is_auto(pref: &Option<String>) -> bool {
    match pref {
        None => true,
        Some(s) => s == "auto",
    }
}
