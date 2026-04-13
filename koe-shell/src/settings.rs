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
use std::sync::atomic::{AtomicU16, Ordering};
use tower_http::cors::CorsLayer;

const PORT_BASE: u16 = 9876;
const PORT_TRIES: u16 = 10;

/// The port the settings server actually bound to (0 = not running).
static BOUND_PORT: AtomicU16 = AtomicU16::new(0);

pub fn start(rt: &tokio::runtime::Runtime) {
    rt.spawn(async move {
        let app = Router::new()
            .route("/", get(page_handler))
            .route("/api/config", get(get_config))
            .route("/api/config/form", post(save_form))
            .route("/api/config/yaml", post(save_yaml))
            .route("/api/reload", post(reload_config))
            .route("/api/file/{name}", get(get_file).post(save_file))
            .route("/api/test/llm", post(test_llm))
            .layer(CorsLayer::permissive());

        // Try a range of ports starting from PORT_BASE
        let mut listener = None;
        for offset in 0..PORT_TRIES {
            let port = PORT_BASE + offset;
            let addr = SocketAddr::from(([127, 0, 0, 1], port));
            match tokio::net::TcpListener::bind(addr).await {
                Ok(l) => {
                    log::info!("settings server listening on http://{addr}");
                    BOUND_PORT.store(port, Ordering::Relaxed);
                    listener = Some(l);
                    break;
                }
                Err(e) => {
                    log::warn!("settings server port {port} unavailable: {e}");
                }
            }
        }

        let Some(listener) = listener else {
            log::error!(
                "settings server failed to bind any port in {PORT_BASE}..{}",
                PORT_BASE + PORT_TRIES - 1
            );
            return;
        };

        if let Err(e) = axum::serve(listener, app).await {
            log::error!("settings server error: {e}");
        }
    });
}

pub fn open_in_browser() {
    let port = BOUND_PORT.load(Ordering::Relaxed);
    if port == 0 {
        log::warn!("settings server not running, cannot open browser");
        return;
    }
    let url = format!("http://127.0.0.1:{port}");
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

// -- File read/write for dictionary.txt and system_prompt.txt --

async fn get_file(
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Result<String, (StatusCode, String)> {
    let path = resolve_file_name(&name)?;
    match std::fs::read_to_string(&path) {
        Ok(content) => Ok(content),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, format!("read error: {e}"))),
    }
}

async fn save_file(
    axum::extract::Path(name): axum::extract::Path<String>,
    body: String,
) -> Result<String, (StatusCode, String)> {
    let path = resolve_file_name(&name)?;
    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&path, &body)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("write error: {e}")))?;
    Ok(r#"{"ok":true}"#.to_string())
}

fn resolve_file_name(name: &str) -> Result<std::path::PathBuf, (StatusCode, String)> {
    match name {
        "dictionary" => {
            let cfg = config::load_config().unwrap_or_default();
            Ok(config::resolve_dictionary_path(&cfg))
        }
        "system_prompt" => {
            let cfg = config::load_config().unwrap_or_default();
            Ok(config::resolve_system_prompt_path(&cfg))
        }
        _ => Err((StatusCode::BAD_REQUEST, format!("unknown file: {name}"))),
    }
}

// -- LLM test endpoint --

#[derive(serde::Deserialize)]
struct TestLlmRequest {
    base_url: String,
    api_key: String,
    model: String,
    max_token_parameter: Option<String>,
    no_reasoning_control: Option<String>,
}

async fn test_llm(
    Json(req): Json<TestLlmRequest>,
) -> axum::response::Json<serde_json::Value> {
    use koe_core::config::{
        LlmMaxTokenParameter, LlmNoReasoningControl, LlmProfileRuntimeConfig,
    };
    use koe_core::llm::openai_compatible::{self, build_http_client};
    use koe_core::{dictionary, prompt};

    let max_token_parameter = match req.max_token_parameter.as_deref() {
        Some("max_tokens") => LlmMaxTokenParameter::MaxTokens,
        _ => LlmMaxTokenParameter::MaxCompletionTokens,
    };
    let no_reasoning_control = match req.no_reasoning_control.as_deref() {
        Some("thinking") => LlmNoReasoningControl::Thinking,
        Some("none") => LlmNoReasoningControl::None,
        _ => LlmNoReasoningControl::ReasoningEffort,
    };

    let cfg = config::load_config().unwrap_or_default();

    let profile = LlmProfileRuntimeConfig {
        id: "test".into(),
        name: "Test".into(),
        provider: "openai".into(),
        base_url: req.base_url,
        api_key: req.api_key,
        model: req.model,
        max_token_parameter,
        no_reasoning_control,
        mlx: Default::default(),
    };

    let system_prompt = prompt::load_system_prompt(&config::resolve_system_prompt_path(&cfg));
    let user_prompt_template =
        prompt::load_user_prompt_template(&config::resolve_user_prompt_path(&cfg));
    let dict_path = config::resolve_dictionary_path(&cfg);
    let dictionary = dictionary::load_dictionary(&dict_path).unwrap_or_default();
    let candidates =
        prompt::filter_dictionary_candidates(&dictionary, "", cfg.llm.dictionary_max_candidates);

    let test_asr = "so umm i installed this program called koe on my computer and like, \
                     i wanna know, you know, how much CPU and memory its using basically";
    let user_prompt =
        prompt::render_user_prompt(&user_prompt_template, test_asr, &candidates, &[]);

    let client = match build_http_client(cfg.llm.timeout_ms) {
        Ok(c) => c,
        Err(e) => {
            return axum::response::Json(serde_json::json!({
                "success": false,
                "elapsed_ms": 0,
                "message": format!("Failed to create HTTP client: {e}"),
            }));
        }
    };

    let (result, elapsed) = openai_compatible::test_correction(
        client,
        &profile,
        cfg.llm.temperature,
        cfg.llm.top_p,
        cfg.llm.max_output_tokens,
        &system_prompt,
        &user_prompt,
    )
    .await;

    let elapsed_ms = elapsed.as_millis() as u64;
    match result {
        Ok(msg) => axum::response::Json(serde_json::json!({
            "success": true,
            "elapsed_ms": elapsed_ms,
            "message": msg,
        })),
        Err(e) => axum::response::Json(serde_json::json!({
            "success": false,
            "elapsed_ms": elapsed_ms,
            "message": format!("{e}"),
        })),
    }
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
