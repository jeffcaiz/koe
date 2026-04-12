//! Localhost HTTP server for configuration UI.
//!
//! GET  /           → HTML settings page
//! GET  /api/config → { raw: "yaml text", config: {parsed json} }
//! POST /api/config/form → merge JSON fields into existing config.yaml
//! POST /api/config/yaml → overwrite config.yaml with raw text
//! POST /api/reload → reload koe-core config

use axum::extract::Json;
use axum::http::StatusCode;
use axum::response::Html;
use axum::routing::{get, post};
use axum::Router;
use koe_core::config;
use std::net::SocketAddr;
use tower_http::cors::CorsLayer;

const PORT: u16 = 9876;

pub fn start(rt: &tokio::runtime::Runtime) {
    rt.spawn(async move {
        let app = Router::new()
            .route("/", get(page_handler))
            .route("/api/config", get(get_config))
            .route("/api/config/form", post(save_form))
            .route("/api/config/yaml", post(save_yaml))
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

pub fn open_in_browser() {
    let url = format!("http://127.0.0.1:{PORT}");
    if let Err(e) = open::that(&url) {
        log::error!("failed to open browser: {e}");
    }
}

// -- Handlers --

async fn page_handler() -> Html<&'static str> {
    Html(include_str!("settings.html"))
}

/// Return both raw YAML and parsed JSON.
async fn get_config() -> Result<axum::response::Json<serde_json::Value>, (StatusCode, String)> {
    let path = config::config_path();
    let raw = std::fs::read_to_string(&path).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("read failed: {e}"))
    })?;

    let parsed: serde_yaml::Value = serde_yaml::from_str(&raw)
        .unwrap_or(serde_yaml::Value::Mapping(Default::default()));
    let json = serde_json::to_value(&parsed)
        .unwrap_or(serde_json::Value::Object(Default::default()));

    Ok(axum::response::Json(serde_json::json!({
        "raw": raw,
        "config": json,
    })))
}

/// Save from form: receive JSON partial, merge into existing YAML, write back.
async fn save_form(
    Json(patch): Json<serde_json::Value>,
) -> Result<String, (StatusCode, String)> {
    let path = config::config_path();
    let raw = std::fs::read_to_string(&path).unwrap_or_default();

    // Parse existing config as a YAML mapping
    let mut root: serde_yaml::Value = if raw.trim().is_empty() {
        serde_yaml::Value::Mapping(Default::default())
    } else {
        serde_yaml::from_str(&raw).map_err(|e| {
            (StatusCode::INTERNAL_SERVER_ERROR, format!("existing config parse error: {e}"))
        })?
    };

    // Convert the JSON patch to a YAML value and deep-merge
    let patch_yaml: serde_yaml::Value = serde_json::from_value::<serde_yaml::Value>(patch)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid patch: {e}")))?;

    deep_merge(&mut root, &patch_yaml);

    // Write back
    let output = serde_yaml::to_string(&root)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("serialize error: {e}")))?;

    config::atomic_write_config(&output)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("write error: {e}")))?;

    let _ = koe_core::api::reload_config();
    Ok(r#"{"ok":true}"#.to_string())
}

/// Save from YAML tab: overwrite config.yaml with raw text.
async fn save_yaml(
    Json(payload): Json<SaveYamlRequest>,
) -> Result<String, (StatusCode, String)> {
    // Validate YAML
    let _: serde_yaml::Value = serde_yaml::from_str(&payload.content)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid YAML: {e}")))?;

    config::atomic_write_config(&payload.content)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("write error: {e}")))?;

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
struct SaveYamlRequest {
    content: String,
}

/// Deep-merge `patch` into `base`. Patch values overwrite base values;
/// mappings are merged recursively; non-mapping patch values replace base.
fn deep_merge(base: &mut serde_yaml::Value, patch: &serde_yaml::Value) {
    match (base, patch) {
        (serde_yaml::Value::Mapping(base_map), serde_yaml::Value::Mapping(patch_map)) => {
            for (key, patch_val) in patch_map {
                if let Some(base_val) = base_map.get_mut(key) {
                    deep_merge(base_val, patch_val);
                } else {
                    base_map.insert(key.clone(), patch_val.clone());
                }
            }
        }
        (base, patch) => {
            *base = patch.clone();
        }
    }
}
