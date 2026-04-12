# koe-desktop: Cross-Platform Desktop Shell

## Overview

A minimal cross-platform desktop shell for Koe, reusing the existing `koe-core` Rust library.
Targets Windows and Linux (macOS continues to use the native `KoeApp`).

## Architecture

```
koe-desktop (new Rust binary, ~600 lines)
┌─────────────────────────────────────────────┐
│                                             │
│  ┌────────────┐ ┌──────────┐ ┌───────────┐ │
│  │ Global     │ │ Audio    │ │ Clipboard  │ │
│  │ Hotkey     │ │ Capture  │ │ + Paste    │ │
│  │            │ │ (cpal)   │ │ Simulation │ │
│  └─────┬──────┘ └────┬─────┘ └─────▲─────┘ │
│        │              │             │       │
│  ┌─────▼──────────────▼─────────────┴─────┐ │
│  │       koe-core (direct Rust API)       │ │
│  └────────────────────────────────────────┘ │
│                                             │
│  ┌────────────┐ ┌───────────────────────┐   │
│  │ System     │ │ Config                │   │
│  │ Tray       │ │ = edit config.yaml    │   │
│  │            │ │ (or localhost web UI)  │   │
│  └────────────┘ └───────────────────────┘   │
└─────────────────────────────────────────────┘
                    │
                    │ direct Rust call (no FFI)
                    ▼
┌─────────────────────────────────────────────┐
│ koe-core (existing, untouched)              │
│ ASR: Doubao / Qwen (cloud) + sherpa-onnx    │
│ LLM: OpenAI-compatible HTTP                 │
└─────────────────────────────────────────────┘
```

## Core Cycle

```
Hotkey pressed
  → sp_core_session_begin()
  → audio_stream.resume()
  → loop: sp_core_push_audio(pcm_frames)
Hotkey released
  → audio_stream.pause()
  → sp_core_session_end()
  → [koe-core: ASR → LLM correction → callback]
  → on_final_text_ready:
      clipboard.set_text(final_text)
      simulate Ctrl+V
```

## Dependencies

All crates are pure Rust, cross-platform, and handle platform differences internally:

| Module          | Crate          | Windows Backend        | Linux Backend            |
|-----------------|----------------|------------------------|--------------------------|
| Audio capture   | `cpal`         | WASAPI                 | ALSA / PulseAudio / JACK |
| Clipboard       | `arboard`      | Win32 Clipboard        | X11 / Wayland            |
| System tray     | `tray-icon`    | Shell_NotifyIcon       | libappindicator          |
| Global hotkey   | `global-hotkey`| RegisterHotKey (Win32) | XGrabKey (X11)           |
| Paste simulate  | `enigo`        | SendInput (Ctrl+V)     | XTest (X11)              |
| Core logic      | `koe-core`     | (pure Rust, no platform code)                    |

No C compiler needed. No FFI boundary. No platform-specific conditional compilation
in koe-desktop code.

## Scope: What's In / What's Out

### In (MVP)

- [x] Global hotkey (hold-to-record)
- [x] Microphone capture → 16kHz mono PCM16
- [x] Push audio to koe-core session
- [x] Receive final text via callback
- [x] Write to clipboard + simulate paste
- [x] System tray icon with status (idle / recording / processing)
- [x] Right-click menu: quit, status display
- [x] Config via `config.yaml` (manual edit or browser)

### Out (later)

- Settings GUI (use config.yaml directly, or add a localhost web UI later)
- Floating overlay / status pill
- Permission management (Windows/Linux don't need macOS-style permission flow)
- Auto-update
- Audio cue sounds
- History database
- Toggle mode (hold-to-record is enough for MVP)
- MLX inference (Apple Silicon only; use sherpa-onnx or cloud ASR instead)
- Apple Speech (macOS only)

## Project Structure

```
koe/
├── koe-asr/                  # existing - ASR providers
├── koe-core/                 # existing - core logic
├── koe-cli/                  # existing - CLI model manager
├── koe-desktop/              # NEW
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs           # entry point, message loop, orchestration
│       ├── hotkey.rs          # global hotkey registration + events
│       ├── audio.rs           # cpal mic capture, PCM conversion
│       ├── clipboard.rs       # write text + simulate Ctrl+V / Ctrl+Z
│       └── tray.rs            # system tray icon + menu
├── KoeApp/                   # existing - macOS native shell (untouched)
├── Packages/                 # existing - Swift packages (untouched)
└── Cargo.toml                # add "koe-desktop" to workspace members
```

## Changes to Existing Code

Minimal:

1. **Cargo.toml (workspace root)**: add `"koe-desktop"` to `members` list
2. **koe-core/src/lib.rs**: expose `KoeCore` struct and key methods as `pub`
   (currently only accessible through C FFI wrappers)
3. **koe-core/src/config.rs**: add `#[cfg(windows)]` path for
   `%APPDATA%\koe\` instead of `~/.koe/`

Everything else stays untouched.

## koe-desktop/Cargo.toml

```toml
[package]
name = "koe-desktop"
description = "Cross-platform desktop shell for Koe"
version.workspace = true
edition.workspace = true
license.workspace = true
publish.workspace = true
rust-version.workspace = true

[lints]
workspace = true

[[bin]]
name = "koe"
path = "src/main.rs"

[dependencies]
koe-core = { path = "../koe-core", default-features = false, features = ["sherpa-onnx"] }
log.workspace = true
env_logger.workspace = true
tokio = { workspace = true, features = ["rt-multi-thread", "macros", "sync"] }

# cross-platform desktop
cpal = "0.15"
arboard = "3"
tray-icon = "0.19"
global-hotkey = "0.6"
enigo = "0.13"
```

## Estimated Effort

| Phase                        | Work                                           | Time    |
|------------------------------|-------------------------------------------------|---------|
| Scaffold                     | Create crate, wire up workspace, compile        | 0.5 day |
| Expose koe-core Rust API     | Make KoeCore pub, add config path cfg           | 0.5 day |
| Hotkey + message loop        | global-hotkey registration, event handling       | 0.5 day |
| Audio capture                | cpal stream, resample to 16kHz mono PCM16       | 1 day   |
| Core integration             | session begin/push/end + callback wiring         | 1 day   |
| Clipboard + paste            | arboard write + enigo Ctrl+V                    | 0.5 day |
| System tray                  | tray-icon with status + quit menu               | 0.5 day |
| End-to-end testing           | Full cycle: hotkey → record → ASR → LLM → paste | 1 day   |
| **Total**                    |                                                 | **~5 days** |

## Performance Notes

All crates are thin wrappers over native platform APIs. The overhead vs writing
platform-specific C/C++ code is effectively zero:

- Audio: cpal calls WASAPI/ALSA directly, PCM buffer is zero-copy from kernel
- Hotkey: OS delivers a message to your event loop, same as native
- Clipboard + paste: single system call, microsecond-level operation
- The real latency is ASR (500-2000ms) + LLM (300-1000ms), not the desktop shell

## Wayland Caveat

On Linux with Wayland, global hotkeys and input simulation have limited support
due to Wayland's security model (no global input grabbing by design).
X11 works fully. This is a known ecosystem limitation, not specific to this project.
