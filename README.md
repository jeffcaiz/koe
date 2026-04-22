# Koe Shell

> **⚠️ 项目已停止开发 / Project Discontinued**
>
> 本项目不再继续开发，已迁移到新项目 **[Air Talk](https://github.com/jeffcaiz/airtalk)**，请前往新仓库获取最新进展。
>
> This project is no longer maintained. Development has moved to **[Air Talk](https://github.com/jeffcaiz/airtalk)** — please head there for the latest work.

> Fork of [Koe (声)](https://github.com/missuo/koe) — bringing voice-to-text input to **Windows**.

The original Koe is a macOS-native voice input tool built with Objective-C + Rust. This fork adds **Koe Shell** (`koe-shell`), a pure Rust desktop shell that replaces the macOS Objective-C layer, making Koe work on Windows with the same core engine.

## What Changed from Upstream

| | Upstream (missuo/koe) | This Fork |
|---|---|---|
| **Platform** | macOS only (Objective-C shell) | Windows (Rust shell) |
| **UI** | Native macOS menu bar + overlay | System tray + Win32 overlay |
| **Binary** | `Koe.app` (Xcode build) | `koe.exe` single binary (cargo build) |
| **Core engine** | Same `koe-core` Rust library | Same `koe-core` Rust library |
| **ASR** | All providers | Cloud + sherpa-onnx (no MLX/Apple Speech) |

The key insight: `koe-core` (ASR, LLM, config, session management) is already pure Rust. Only the thin "shell" layer needed to be rewritten for Windows.

For configuration, prompts, dictionary, local models, and other features shared with upstream, see the [upstream README](https://github.com/missuo/koe).

## Installation

### Download Release

Download the latest binary from [GitHub Releases](https://github.com/jeffcaiz/koe/releases/latest):

- **Windows**: `koe-<version>-x86_64-pc-windows-msvc.zip`

Unzip and run `koe.exe`. No installation needed.

### Build from Source

Prerequisites:
- Rust toolchain (`rustup`)
- Visual Studio Build Tools (for MSVC)

```bash
git clone https://github.com/jeffcaiz/koe.git
cd koe
cargo build --release --package koe-shell
# Binary at: target/release/koe.exe
```

## Usage

1. Run `koe.exe` — a system tray icon appears
2. Press the hotkey (default: **Right Alt**) to start recording
3. Speak — audio streams to the ASR service in real-time
4. Release the hotkey — corrected text is pasted into the active input field

First-time setup is done through the Settings UI, accessible from the system tray menu.

### Configuration

All config lives in `%APPDATA%\koe\`, same format as the original Koe. See the [upstream documentation](https://github.com/missuo/koe) for full config reference.

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

This triggers CI to build Windows binaries and publish them as a GitHub Release.

## License

MIT — same as upstream.
