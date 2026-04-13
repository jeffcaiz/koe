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

### Configuration

All config lives in `~/.koe/` (or `%USERPROFILE%\.koe\` on Windows), same format as the original Koe. See the [upstream documentation](https://koe.li) for full config reference.

Key files:
```
~/.koe/
├── config.yaml          # Main configuration (ASR, LLM, hotkey, etc.)
├── dictionary.txt       # Custom vocabulary for ASR + LLM
├── system_prompt.txt    # LLM correction prompt
└── models/              # Local ASR models (sherpa-onnx)
```

### Settings UI

Koe Shell includes a built-in web-based settings page. Access it from the system tray menu or open `http://localhost:19199` in your browser.

## How It Works

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
git tag v0.2.0
git push fork v0.2.0
```

Since the dictionary is just a text file, you can version-control it, share it across machines, or script its maintenance however you like.

### Prompts

The LLM correction behavior is fully customizable via two prompt files:

- **`~/.koe/system_prompt.txt`** — defines the correction rules (capitalization, spacing, punctuation, filler word removal, etc.)
- **`~/.koe/user_prompt.txt`** — template that assembles the ASR output, interim history, and dictionary into the final LLM request

The Setup Wizard edits `system_prompt.txt` directly. `user_prompt.txt` is still
supported, but it is an advanced manual-edit file rather than a first-class UI pane.

Available template placeholders in `user_prompt.txt`:

| Placeholder | Description |
|---|---|
| `{{asr_text}}` | The final ASR transcript text |
| `{{interim_history}}` | ASR interim revision history — shows how the transcript changed over time, helping the LLM identify uncertain words |
| `{{dictionary_entries}}` | Filtered dictionary entries for LLM context |

The default prompts are tuned for software developers working in mixed Chinese-English, but you can adapt them for any language or domain.
If either prompt file is missing or empty, Koe falls back to the built-in defaults
compiled into `koe-core`.

### Prompt Templates

Koe can optionally keep the overlay visible after the default correction and show
rewrite templates above the result bubble. The Templates pane lets you add, edit,
remove, reorder, and enable or disable up to 9 templates.

```yaml
llm:
  prompt_templates_enabled: true

prompt_templates:
  - name: "翻译英文"
    enabled: true
    shortcut: 1
    system_prompt: "将用户的语音输入翻译为流畅的英文。保持原意，不要添加额外内容。只输出翻译结果。"

  - name: "邮件润色"
    enabled: true
    shortcut: 2
    system_prompt: "将用户的语音输入整理为简洁、礼貌、自然的英文邮件内容。只输出最终邮件正文。"
```

- `llm.prompt_templates_enabled` is the global switch for showing template buttons in the overlay.
- `enabled` controls whether a specific template is shown.
- `shortcut` is the contextual overlay slot (`1-9`) after reordering.
- `system_prompt` and `system_prompt_path` are mutually exclusive.

After the normal correction is pasted, you can hover or click a template button,
or press `1-9`, to run a second-pass rewrite. Rewrite results are copied to the
clipboard instead of being auto-pasted, so you can decide whether to use them.

## Usage Statistics

Koe automatically tracks your voice input usage in a local SQLite database at `~/.koe/history.db`. You can view a summary directly in the menu bar dropdown — it shows total characters, words, recording time, session count, and input speed.

### Database Schema

```sql
CREATE TABLE sessions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp INTEGER NOT NULL,   -- Unix timestamp
    duration_ms INTEGER NOT NULL, -- Recording duration in milliseconds
    text TEXT NOT NULL,            -- Final transcribed text
    char_count INTEGER NOT NULL,  -- CJK character count
    word_count INTEGER NOT NULL   -- English word count
);
```

### Querying Your Data

You can query the database directly with `sqlite3`:

```bash
# View all sessions
sqlite3 ~/.koe/history.db "SELECT * FROM sessions ORDER BY timestamp DESC LIMIT 10;"

# Total stats
sqlite3 ~/.koe/history.db "SELECT COUNT(*) as sessions, SUM(duration_ms)/1000 as total_seconds, SUM(char_count) as chars, SUM(word_count) as words FROM sessions;"

# Daily breakdown
sqlite3 ~/.koe/history.db "SELECT date(timestamp, 'unixepoch', 'localtime') as day, COUNT(*) as sessions, SUM(char_count) as chars, SUM(word_count) as words FROM sessions GROUP BY day ORDER BY day DESC;"
```

You can also build your own dashboard or visualization on top of this database — it's just a standard SQLite file.

## Local ASR

Koe supports three on-device speech recognition providers:

- **Apple Speech** (macOS 26+) — uses Apple's built-in SpeechAnalyzer. Select the provider and language in the Setup Wizard — speech assets are managed by macOS and downloaded automatically on first use (or manually via the Setup Wizard). No API key needed. Dictionary entries are automatically passed as contextual strings for vocabulary bias.
- **MLX** (Apple Silicon) — runs Qwen3-ASR models via the MLX framework. Requires model download (~680 MB–1.5 GB).
- **sherpa-onnx** (CPU) — runs streaming zipformer models. Requires model download (~189 MB–735 MB).

MLX and sherpa-onnx models are managed through `.koe-manifest.json` files under `~/.koe/models/`. You can manage them in two ways:

1. **Setup Wizard** — select a local provider in the ASR tab, pick a model from the dropdown, and click the download button. Progress is shown inline with a progress bar.
2. **koe CLI** — command-line model management (see below).

### koe CLI

The `koe` CLI tool manages local models:

```bash
# List all discovered models and their status
koe model list

# Download a model
koe model pull mlx/Qwen3-ASR-0.6B-4bit

# Check model status
koe model status mlx/Qwen3-ASR-0.6B-4bit

# Remove downloaded files (keeps manifest for re-download)
koe model remove mlx/Qwen3-ASR-0.6B-4bit

# Generate manifest from a HuggingFace repo
koe manifest generate mlx-community/Qwen3-ASR-0.6B-4bit \
    --provider mlx --description "Qwen3 ASR 0.6B 4-bit"
```

### Available Models

**MLX ASR (Apple Silicon)**:
- `mlx/Qwen3-ASR-0.6B-4bit` — Qwen3 ASR 0.6B 4-bit (~680 MB, fast)
- `mlx/Qwen3-ASR-1.7B-4bit` — Qwen3 ASR 1.7B 4-bit (~1.5 GB, higher accuracy)

**MLX LLM (Apple Silicon)**:
- `mlx/Qwen3-0.6B-4bit` — Qwen3 LLM 0.6B 4-bit (~335 MB, fast)
- `mlx/Qwen3-1.7B-4bit` — Qwen3 LLM 1.7B 4-bit (~938 MB, higher accuracy)

**sherpa-onnx (CPU)**:
- `sherpa-onnx/bilingual-zh-en` — Bilingual Chinese-English (~189 MB)
- `sherpa-onnx/multilingual-8lang` — 8-language multilingual (~322 MB)
- `sherpa-onnx/zh-xlarge` — Chinese extra-large (~735 MB, best accuracy)

### Fully Offline Mode

When both ASR and LLM are set to local MLX providers, Koe runs entirely on-device with no network access required — ideal for privacy-sensitive use cases. GPU memory usage depends on the model combination:

- **Lightest** (ASR 0.6B + LLM 0.6B): ~1.2 GB — runs comfortably on any Apple Silicon Mac
- **Heaviest** (ASR 1.7B + LLM 1.7B): ~2.9 GB — still fits easily in 8 GB unified memory

APFEL also runs the model outside Koe and exposes it through a local
OpenAI-compatible HTTP endpoint. Koe only calls that endpoint; it does not manage
the APFEL service lifecycle.

### Model Manifest

Each model directory contains a `.koe-manifest.json` describing the model and its files:

```json
{
  "provider": "mlx",
  "mode": "asr",
  "description": "Qwen3 ASR 0.6B 4-bit (fast, lightweight)",
  "repo": "mlx-community/Qwen3-ASR-0.6B-4bit",
  "files": [
    {"name": "config.json", "size": 7187, "sha256": "...", "url": "https://huggingface.co/..."}
  ]
}
```

The `mode` field (`"asr"` or `"llm"`) determines where the model appears in the Setup Wizard.

Default manifests are installed automatically on first launch. `koe model pull` downloads the actual model files using the URLs and verifies them with sha256 checksums.

## AI-Assisted Setup

Koe provides a skill that works with any AI coding agent (Claude Code, Codex, etc.) to guide you through the entire setup process interactively.

### Install the Skill

```bash
npx skills add missuo/koe
```

The command will let you choose which AI coding tool to install the skill for.

### What It Does

Once installed, the `koe-setup` skill will:

1. Check your installation and permissions
2. Walk you through ASR and LLM credential setup
3. Ask about your profession and generate a **personalized dictionary** tailored to your domain
4. Customize the **system prompt** based on your use case
5. Help you configure the trigger key and sound feedback

This is especially useful for first-time users who want a guided, interactive setup experience.

## Build Variants

Koe ships multiple Xcode schemes for different use cases:

| Scheme | App | Zip | Idle Memory | Description |
|--------|-----|-----|--------|-------------|
| **Koe** | ~86 MB | ~24 MB | ~40 MB | Full build (arm64). All providers. |
| **Koe-lite** | ~19 MB | ~7 MB | ~13 MB | Lightweight (arm64). Cloud + Apple Speech only. |
| **Koe-x86** | — | — | — | Intel build (x86_64). No MLX. |

```bash
# Full build (default, Apple Silicon)
make build

# Lite build (cloud + Apple Speech only, ~78% smaller)
make build-lite

# Intel build
make build-x86_64
```

The lite build excludes MLX and sherpa-onnx, producing a smaller app that doesn't require downloading on-device ASR models (~189 MB–1.5 GB). Cloud providers (Doubao, Qwen) work on all macOS versions; Apple Speech requires macOS 26+.

Local ASR providers are controlled by Rust feature flags in `koe-core/Cargo.toml`: `mlx`, `apple-speech`, `sherpa-onnx` (all enabled by default). Each Xcode scheme passes the appropriate `--features` flags to `cargo build`.

## Architecture

Koe is built as a native macOS app with two layers:

- **Objective-C shell** — handles macOS integration: hotkey detection, audio capture, clipboard management, paste simulation, menu bar UI, and usage statistics (SQLite)
- **Rust core library** — handles ASR (cloud WebSocket streaming + local MLX/sherpa-onnx/Apple Speech), LLM correction (OpenAI-compatible profile endpoints + local MLX), config management, model management, transcript aggregation, and session orchestration
- **Swift KoeMLX package** — bridges MLX inference to Rust via C FFI for on-device ASR (Qwen3-ASR) and LLM text correction (Qwen3) on Apple Silicon
- **Swift KoeAppleSpeech package** — bridges Apple's SpeechAnalyzer to Rust via C FFI for zero-config on-device ASR (macOS 26+)

The two layers communicate via C FFI (Foreign Function Interface). The Rust core is compiled as a static library (`libkoe_core.a`) and linked into the Xcode project.

```
┌──────────────────────────────────────────────────┐
│  macOS (Objective-C)                             │
│  ┌──────────┐ ┌──────────┐ ┌───────────────────┐│
│  │ Hotkey   │ │ Audio    │ │ Clipboard + Paste ││
│  │ Monitor  │ │ Capture  │ │                   ││
│  └────┬─────┘ └────┬─────┘ └────────▲──────────┘│
│       │             │                │           │
│  ┌────▼─────────────▼────────────────┴─────────┐ │
│  │           SPRustBridge (FFI)                 │ │
│  └────────────────┬────────────────────────────┘ │
│                   │                              │
│  ┌────────────────┴───────┐  ┌────────────────┐  │
│  │ Menu Bar + Status Bar  │  │ History Store  │  │
│  │ (SPStatusBarManager)   │  │ (SQLite)       │  │
│  └────────────────────────┘  └────────────────┘  │
└───────────────────┼──────────────────────────────┘
                    │ C ABI
┌───────────────────▼──────────────────────────────┐
│  Rust Core (libkoe_core.a)                       │
│  ┌──────────────────────────┐ ┌────────────────┐  │
│  │ ASR Providers            │ │ Config + Dict  │  │
│  │ ┌────────┐ ┌───────────┐ │ │ + Prompts      │  │
│  │ │ Doubao │ │ Qwen      │ │ │ + Models       │  │
│  │ │ (WS)   │ │ (WS)      │ │ └────────────────┘  │
│  │ ├────────┤ ├───────────┤ │ ┌────────────────┐  │
│  │ │ MLX    │ │ sherpa-   │ │ │ LLM            │  │
│  │ │ (FFI)  │ │ onnx(CPU) │ │ │ HTTP or MLX    │  │
│  │ ├────────┤ ├───────────┤ │ └───────▲────────┘  │
│  │ │ Apple  │ │           │ │                     │
│  │ │ Speech │ │           │ │                     │
│  │ └────────┘ └───────────┘ │                     │
│  └──────────┬───────────────┘         │           │
│  ┌──────────▼─────────────────────────┴────────┐  │
│  │ TranscriptAggregator                        │  │
│  │ (interim → definite → final + history)      │  │
│  └─────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────┘
```

### ASR Pipeline

Cloud providers (Doubao, Qwen):

1. Audio streams via WebSocket to the cloud ASR service
2. First-pass streaming results arrive in real-time (`Interim` events) and are displayed in the overlay
3. Second-pass re-recognition confirms segments with higher accuracy (`Definite` events)

Local providers (Apple Speech, MLX, sherpa-onnx):

1. Audio is processed on-device — Apple Speech via SpeechAnalyzer (macOS 26+), MLX via Swift FFI on Apple Silicon, sherpa-onnx via a dedicated CPU worker thread
2. Streaming results are emitted through the same `Interim`/`Definite`/`Final` event model

All providers:

4. `TranscriptAggregator` merges all results and tracks interim revision history
5. Final transcript + interim history + dictionary are sent to the LLM for correction (cloud API or local MLX)

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for commit conventions, PR guidelines, and the full contributor workflow.

## Contributors

[![Contributors](https://contrib.rocks/image?repo=missuo/koe)][contributors]

[contributors]: https://github.com/missuo/koe/graphs/contributors

- Vincent Yang — creator and maintainer
- Zhang Erning ([@erning](https://github.com/erning)) — local ASR stack (MLX + sherpa-onnx), Apple Speech provider, local MLX LLM, Koe-lite build variant, and a large body of core/runtime fixes
- luolei ([@foru17](https://github.com/foru17)) — 1.0.14 release cycle: prompt templates, shortcut workflow, overlay diff animation, and settings/overlay interaction polish
- Davy ([@thedavidweng](https://github.com/thedavidweng)) — multi-profile LLM system, configurable LLM invert modifier, NSAlert-based permission prompts, and audio/logging fixes
- hyspace ([@hyspace](https://github.com/hyspace)) — LLM HTTP client reuse and HTTP/2, connection warm-up, and GPT-5 reasoning effort handling
- Macyou ([@nmvr2600](https://github.com/nmvr2600)) — Qwen ASR provider
- Simon Mau ([@simonxmau](https://github.com/simonxmau)) — auto-wrapping interim transcription display and jitter fixes

This triggers CI to build Windows and Linux binaries and publish them as a GitHub Release.

## License

MIT — same as upstream.
