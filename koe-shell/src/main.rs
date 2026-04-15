#![windows_subsystem = "windows"]

mod audio;
mod device_pref;
mod feedback;
mod hotkey;
mod overlay;
mod paste;
mod settings;
mod tray;

use koe_core::api;
use koe_core::event::KoeEvent;
use tokio::sync::mpsc;

fn main() {
    let debug = std::env::args().any(|a| a == "--debug");

    // In debug mode on Windows, re-attach to the parent console so logs are visible.
    #[cfg(windows)]
    if debug {
        unsafe { windows_sys::Win32::System::Console::AttachConsole(u32::MAX); }
    }

    init_logging(debug);
    log::info!("koe-shell starting (debug={})", debug);

    // Create event channel
    let (event_tx, event_rx) = mpsc::unbounded_channel::<KoeEvent>();

    // Initialize koe-core
    if let Err(e) = api::create(event_tx) {
        log::error!("failed to initialize koe-core: {e}");
        std::process::exit(1);
    }
    log::info!("koe-core initialized");

    // Strip macOS-only defaults from config (apfel, mlx profiles; fn hotkey)
    sanitize_config();

    // Start the tokio runtime for async event processing
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to create tokio runtime");

    // Spawn event consumer on the tokio runtime
    rt.spawn(event_loop(event_rx));

    // Start settings web server
    settings::start(&rt);

    // Initialize audio stream (runs continuously, gate controls pushing)
    audio::init();

    // Initialize overlay (floating status pill)
    overlay::init();

    // Initialize hotkey (registers global hotkey)
    hotkey::init();

    // The main thread runs the platform event loop.
    // On Windows this is a Win32 message loop; on Linux it's a GLib/X11 loop.
    // global-hotkey and tray-icon both require this.
    log::info!("entering main event loop");
    tray::run_event_loop();

    // Cleanup
    api::destroy();
    log::info!("koe-shell exiting");
}

fn init_logging(debug: bool) {
    if debug {
        // Console output
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    } else {
        // File output to ~/.koe/koe-shell.log
        let log_path = dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".koe")
            .join("koe-shell.log");

        // Ensure ~/.koe/ exists
        if let Some(parent) = log_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&log_path);

        match file {
            Ok(file) => {
                env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
                    .target(env_logger::Target::Pipe(Box::new(file)))
                    .init();
                // Can't log this via log! yet since we just initialized, but it's fine
            }
            Err(_) => {
                // Fallback to stderr if file creation fails
                env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
                    .init();
            }
        }
    }
}

/// Remove macOS-only defaults from config.yaml so the on-disk file makes sense
/// for Windows / Linux. Only touches values that still match the upstream defaults;
/// user-customized configs are left alone.
fn sanitize_config() {
    use koe_core::config;

    let path = config::config_path();
    let raw = match std::fs::read_to_string(&path) {
        Ok(r) => r,
        Err(_) => return,
    };
    let mut doc: serde_yaml::Value = match serde_yaml::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return,
    };

    let mut changed = false;

    // Replace macOS-only / bare upstream profiles with friendly provider presets.
    // Only act when profiles look like the upstream default set (has apfel/mlx,
    // or is the single "openai" profile with empty api_key).
    if let Some(profiles) = doc
        .get_mut("llm")
        .and_then(|l| l.get_mut("profiles"))
        .and_then(|p| p.as_mapping_mut())
    {
        let has_macos = profiles
            .keys()
            .any(|k| matches!(k.as_str(), Some("apfel" | "mlx")));
        let is_bare_openai = profiles.len() == 1
            && profiles
                .get(&serde_yaml::Value::String("openai".into()))
                .and_then(|v| v.get("api_key"))
                .and_then(|v| v.as_str())
                .map_or(false, |k| k.is_empty());

        if has_macos || is_bare_openai {
            // Replace with friendly presets
            *profiles = shell_default_profiles();
            changed = true;
        }
    }

    // Fix active_profile if it pointed to a removed profile
    if let Some(llm) = doc.get_mut("llm") {
        if let Some(active) = llm.get("active_profile").and_then(|v| v.as_str()) {
            if matches!(active, "apfel" | "mlx" | "openai") {
                llm["active_profile"] =
                    serde_yaml::Value::String("doubao".into());
                changed = true;
            }
        }
    }

    // Fix ASR provider if set to macOS-only values
    if let Some(asr) = doc.get_mut("asr") {
        if let Some(provider) = asr.get("provider").and_then(|v| v.as_str()) {
            if provider == "mlx" || provider == "apple-speech" {
                asr["provider"] = serde_yaml::Value::String("doubaoime".into());
                changed = true;
            }
        }
    }

    if changed {
        if let Ok(output) = serde_yaml::to_string(&doc) {
            let _ = config::atomic_write_config(&output);
            log::info!("sanitized config: removed macOS-only defaults");
        }
    }
}

/// Build a YAML mapping of friendly LLM profiles for Chinese cloud providers.
fn shell_default_profiles() -> serde_yaml::Mapping {
    let presets: &[(&str, &str, &str, &str, &str)] = &[
        // (id, name, base_url, model, max_token_parameter)
        (
            "doubao",
            "豆包 (Doubao)",
            "https://ark.cn-beijing.volces.com/api/v3",
            "doubao-1.5-pro-32k",
            "max_tokens",
        ),
        (
            "qwen",
            "通义千问 (Qwen)",
            "https://dashscope.aliyuncs.com/compatible-mode/v1",
            "qwen-turbo",
            "max_tokens",
        ),
        (
            "deepseek",
            "DeepSeek",
            "https://api.deepseek.com/v1",
            "deepseek-chat",
            "max_tokens",
        ),
        (
            "kimi",
            "Kimi (Moonshot)",
            "https://api.moonshot.ai/v1",
            "kimi-k2.5",
            "max_tokens",
        ),
        (
            "glm",
            "智谱 GLM (Zhipu)",
            "https://open.bigmodel.cn/api/paas/v4",
            "glm-4.7-flash",
            "max_tokens",
        ),
        (
            "minimax",
            "MiniMax (海螺)",
            "https://api.minimaxi.com/v1",
            "MiniMax-M2.5-highspeed",
            "max_tokens",
        ),
        (
            "claude",
            "Claude (via OpenRouter)",
            "https://openrouter.ai/api/v1",
            "anthropic/claude-sonnet-4",
            "max_tokens",
        ),
        (
            "openai",
            "OpenAI",
            "https://api.openai.com/v1",
            "gpt-4.1-nano",
            "max_completion_tokens",
        ),
    ];

    let mut profiles = serde_yaml::Mapping::new();
    for (id, name, base_url, model, max_token_param) in presets {
        let mut p = serde_yaml::Mapping::new();
        p.insert("name".into(), (*name).into());
        p.insert("provider".into(), "openai".into());
        p.insert("base_url".into(), (*base_url).into());
        p.insert("api_key".into(), "".into());
        p.insert("model".into(), (*model).into());
        p.insert("max_token_parameter".into(), (*max_token_param).into());
        p.insert("no_reasoning_control".into(), "none".into());
        profiles.insert(
            serde_yaml::Value::String(id.to_string()),
            serde_yaml::Value::Mapping(p),
        );
    }
    profiles
}

async fn event_loop(mut rx: mpsc::UnboundedReceiver<KoeEvent>) {
    while let Some(event) = rx.recv().await {
        match event {
            KoeEvent::FinalText { token: _, text } => {
                log::info!("final text: {text}");
                // Mirror macOS: set "pasting" state, paste, then "idle"
                feedback::on_state_changed("pasting");
                tray::update_status("pasting");
                overlay::update_state("pasting");

                if let Err(e) = paste::paste(&text) {
                    log::error!("paste failed: {e}");
                }

                feedback::on_state_changed("idle");
                tray::update_status("idle");
                overlay::update_state("idle");
            }
            KoeEvent::StateChanged { token: _, state } => {
                log::info!("state: {state}");
                feedback::on_state_changed(&state);
                tray::update_status(&state);
                overlay::update_state(&state);
            }
            KoeEvent::InterimText { token: _, text } => {
                log::debug!("interim: {text}");
                overlay::update_interim_text(&text);
            }
            KoeEvent::AsrFinalText { token: _, text } => {
                log::info!("ASR final: {text}");
                overlay::update_display_text(&text);
            }
            KoeEvent::SessionReady { .. } => {
                log::info!("session ready");
            }
            KoeEvent::SessionError { token: _, message } => {
                log::error!("session error: {message}");
            }
            KoeEvent::SessionWarning { token: _, message } => {
                log::warn!("session warning: {message}");
            }
            KoeEvent::RewriteText { token: _, text } => {
                log::info!("rewrite: {text}");
            }
            KoeEvent::Log { level, message } => {
                match level {
                    0 => log::error!("[core] {message}"),
                    1 => log::warn!("[core] {message}"),
                    2 => log::info!("[core] {message}"),
                    _ => log::debug!("[core] {message}"),
                }
            }
        }
    }
}
