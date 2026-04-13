mod audio;
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
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    log::info!("koe-shell starting");

    // Create event channel
    let (event_tx, event_rx) = mpsc::unbounded_channel::<KoeEvent>();

    // Initialize koe-core
    if let Err(e) = api::create(event_tx) {
        log::error!("failed to initialize koe-core: {e}");
        std::process::exit(1);
    }
    log::info!("koe-core initialized");

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

async fn event_loop(mut rx: mpsc::UnboundedReceiver<KoeEvent>) {
    while let Some(event) = rx.recv().await {
        match event {
            KoeEvent::FinalText { token: _, text } => {
                log::info!("final text: {text}");
                if let Err(e) = paste::paste(&text) {
                    log::error!("paste failed: {e}");
                }
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
