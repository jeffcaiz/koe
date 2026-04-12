//! Localhost HTTP server for configuration UI.
//!
//! Serves a web UI at http://localhost:9876 and provides REST API
//! endpoints for reading/writing config.yaml.

use axum::extract::Json;
use axum::http::StatusCode;
use axum::response::Html;
use axum::routing::{get, post};
use axum::Router;
use koe_core::config;
use std::net::SocketAddr;
use tower_http::cors::CorsLayer;

const PORT: u16 = 9876;

/// Start the settings HTTP server in the background.
pub fn start(rt: &tokio::runtime::Runtime) {
    rt.spawn(async move {
        let app = Router::new()
            .route("/", get(page_handler))
            .route("/api/config", get(get_config))
            .route("/api/config", post(save_config))
            .route("/api/reload", post(reload_config))
            .layer(CorsLayer::permissive());

        let addr = SocketAddr::from(([127, 0, 0, 1], PORT));
        log::info!("settings server listening on http://{addr}");

        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        if let Err(e) = axum::serve(listener, app).await {
            log::error!("settings server error: {e}");
        }
    });
}

/// Open the settings page in the default browser.
pub fn open_in_browser() {
    let url = format!("http://127.0.0.1:{PORT}");
    if let Err(e) = open::that(&url) {
        log::error!("failed to open browser: {e}");
    }
}

// ─── Handlers ──────────────────────────────────────────────

async fn page_handler() -> Html<&'static str> {
    Html(include_str!("settings.html"))
}

async fn get_config() -> Result<String, (StatusCode, String)> {
    let path = config::config_path();
    std::fs::read_to_string(&path).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("failed to read config: {e}"))
    })
}

async fn save_config(Json(payload): Json<SaveConfigRequest>) -> Result<String, (StatusCode, String)> {
    // Validate YAML before saving
    let _: serde_yaml::Value = serde_yaml::from_str(&payload.content).map_err(|e| {
        (StatusCode::BAD_REQUEST, format!("invalid YAML: {e}"))
    })?;

    let path = config::config_path();
    std::fs::write(&path, &payload.content).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("failed to write config: {e}"))
    })?;

    // Reload config in koe-core
    let _ = koe_core::api::reload_config();

    Ok(r#"{"ok":true}"#.to_string())
}

async fn reload_config() -> Result<String, (StatusCode, String)> {
    koe_core::api::reload_config().map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("reload failed: {e}"))
    })?;
    Ok(r#"{"ok":true}"#.to_string())
}

#[derive(serde::Deserialize)]
struct SaveConfigRequest {
    content: String,
}
