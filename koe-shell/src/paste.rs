//! Clipboard write + paste simulation (Ctrl+V).

use arboard::Clipboard;
use enigo::{Direction, Enigo, Key, Keyboard, Settings};
use std::thread;
use std::time::Duration;

/// Write text to clipboard and simulate Ctrl+V to paste it.
pub fn paste(text: &str) -> Result<(), Box<dyn std::error::Error>> {
    // Write to clipboard
    let mut clipboard = Clipboard::new()?;
    clipboard.set_text(text)?;

    // Small delay to ensure clipboard content is ready
    thread::sleep(Duration::from_millis(50));

    // Simulate Ctrl+V
    let mut enigo = Enigo::new(&Settings::default())?;
    enigo.key(Key::Control, Direction::Press)?;
    enigo.key(Key::Unicode('v'), Direction::Click)?;
    enigo.key(Key::Control, Direction::Release)?;

    log::info!("pasted {} chars", text.len());
    Ok(())
}
