# Koe Shell

> Cross-platform fork of [Koe (声)](https://github.com/missuo/koe) — bringing voice-to-text input to **Windows** and **Linux**.

The original Koe is a macOS-native voice input tool built with Objective-C + Rust. This fork adds **Koe Shell** (`koe-shell`), a pure Rust desktop shell that replaces the macOS Objective-C layer, making Koe work on Windows (and Linux) with the same core engine.

## What Changed from Upstream

| | Upstream (missuo/koe) | This Fork |
|---|---|---|
| **Platform** | macOS only (Objective-C shell) | Windows, Linux (Rust shell) |
| **UI** | Native macOS menu bar + overlay | System tray + Win32/X11 overlay |
| **Binary** | `Koe.app` (Xcode build) | `koe` single binary (cargo build) |
| **Core engine** | Same `koe-core` Rust library | Same `koe-core` Rust library |
| **ASR** | All providers | Cloud + sherpa-onnx (no MLX/Apple Speech) |

The key insight: `koe-core` (ASR, LLM, config, session management) is already pure Rust and cross-platform. Only the thin "shell" layer needed to be rewritten.

For configuration, prompts, dictionary, local models, and other features shared with upstream, see the [upstream README](https://github.com/missuo/koe).

## Installation

### Download Release

Download the latest binary from [GitHub Releases](https://github.com/jeffcaiz/koe/releases/latest):

- **Windows**: `koe-<version>-x86_64-pc-windows-msvc.zip`
- **Linux**: `koe-<version>-x86_64-unknown-linux-gnu.tar.gz`

Unzip and run `koe` (or `koe.exe` on Windows). No installation needed.

### Build from Source

Prerequisites:
- Rust toolchain (`rustup`)
- **Windows**: Visual Studio Build Tools (for MSVC)
- **Linux**: `libasound2-dev libxdo-dev libxtst-dev libevdev-dev`

```bash
git clone https://github.com/jeffcaiz/koe.git
cd koe
cargo build --release --package koe-shell
# Binary at: target/release/koe (or koe.exe on Windows)
```

## Usage

1. Run `koe` — a system tray icon appears
2. Press the hotkey (default: **Right Alt**) to start recording
3. Speak — audio streams to the ASR service in real-time
4. Release the hotkey — corrected text is pasted into the active input field

First-time setup is done through the Settings UI, accessible from the system tray menu.

### Configuration

All config lives in `~/.koe/` (or `%USERPROFILE%\.koe\` on Windows), same format as the original Koe. See the [upstream documentation](https://github.com/missuo/koe) for full config reference.

## Architecture

```
┌─────────────────────────────────────────┐
│  Koe Shell (Rust)                       │
│  ┌──────────┐ ┌────────┐ ┌───────────┐ │
│  │ Hotkey   │ │ Audio  │ │ Clipboard │ │
│  │ (rdev)   │ │ (cpal) │ │ + Paste   │ │
│  └────┬─────┘ └───┬────┘ └─────▲─────┘ │
│       │            │            │       │
│  ┌────▼────────────▼────────────┴─────┐ │
│  │         koe-core (Rust)            │ │
│  │  ASR · LLM · Config · Sessions    │ │
│  └────────────────────────────────────┘ │
│                                         │
│  ┌──────────┐ ┌──────────┐ ┌─────────┐ │
│  │ Tray     │ │ Overlay  │ │ Settings│ │
│  │(tray-icon)│ │ (Win32)  │ │ (axum)  │ │
│  └──────────┘ └──────────┘ └─────────┘ │
└─────────────────────────────────────────┘
```

## Release

Releases are automated via GitHub Actions. To create a new release:

```bash
git tag v0.x.x
git push fork v0.x.x
```

This triggers CI to build Windows and Linux binaries and publish them as a GitHub Release.

## License

MIT — same as upstream.
