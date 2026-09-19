//! `llm_*` commands: the Local tab — what is running on this machine, the
//! managed `llama-server`, the GGUF catalogue and the OpenAI-compatible bridge
//! switch. Owned by f2-llm-commands.
//!
//! Every one of these touches the network or spawns a process only when the
//! user clicks; nothing here runs on its own.

use omniget_core::core::llm::local_servers;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, State};

use crate::AppState;

/// Progress of a model or binary download, so the Local tab can show a bar.
pub const EVENT_LOCAL_PROGRESS: &str = "llm://local-progress";

fn progress_fn(app: AppHandle) -> omniget_core::core::tools::ProgressFn {
    std::sync::Arc::new(move |p: omniget_core::core::tools::ToolProgress| {
        let _ = app.emit(
            EVENT_LOCAL_PROGRESS,
            json!({
                "id": p.id,
                "stage": p.stage,
                "done": p.done,
                "total": p.total,
                "message": p.message,
            }),
        );
    })
}

#[tauri::command]
pub async fn llm_local_status() -> Result<Value, String> {
    let status = local_servers::detect(None).await;
    serde_json::to_value(status).map_err(|e| e.to_string())
}

/// Downloads and installs the managed `llama-server` for one build variant.
#[tauri::command]
pub async fn llm_local_install_llama(app: AppHandle, variant: String) -> Result<Value, String> {
    let path = local_servers::install(&variant, progress_fn(app)).await?;
    Ok(json!({ "ok": true, "path": path.to_string_lossy() }))
}

/// The GGUF catalogue with what is already on disk marked as installed.
#[tauri::command]
pub async fn llm_local_models() -> Result<Value, String> {
    serde_json::to_value(local_servers::catalog()).map_err(|e| e.to_string())
}

/// Downloads one catalogue model. New command: declared in the handoff.
#[tauri::command]
pub async fn llm_local_download_model(app: AppHandle, model_id: String) -> Result<Value, String> {
    let path = local_servers::download_model(&model_id, progress_fn(app)).await?;
    Ok(json!({ "ok": true, "path": path.to_string_lossy() }))
}

/// Starts the managed `llama-server` on one downloaded model.
/// New command: declared in the handoff.
#[tauri::command]
pub async fn llm_local_start_llama(model_id: String, port: Option<u16>) -> Result<Value, String> {
    let model = local_servers::catalog()
        .into_iter()
        .find(|m| m.id == model_id)
        .ok_or_else(|| {
            format!(
                "{}: unknown model {model_id}",
                local_servers::ERR_LOCAL_MODEL
            )
        })?;
    let path = model.path.ok_or_else(|| {
        format!(
            "{}: {model_id} is not downloaded",
            local_servers::ERR_LOCAL_MODEL
        )
    })?;
    let url = local_servers::start(
        std::path::Path::new(&path),
        port.unwrap_or(local_servers::LLAMA_SERVER_PORT),
    )
    .await?;
    Ok(json!({ "ok": true, "url": url }))
}

/// Stops the managed `llama-server`. New command: declared in the handoff.
#[tauri::command]
pub async fn llm_local_stop_llama() -> Result<Value, String> {
    Ok(json!({ "stopped": local_servers::stop().await }))
}

/// Reads (`set` absent) or flips the OpenAI-compatible surface on the local
/// bridge. Off by default: with it on, anything holding the bridge token can
/// spend the user's tokens.
#[tauri::command]
pub async fn llm_bridge_openai_enabled(
    state: State<'_, AppState>,
    set: Option<bool>,
) -> Result<Value, String> {
    let enabled = match set {
        Some(value) => state.llm.set_bridge_openai(value),
        None => state.llm.bridge_openai_enabled(),
    };
    Ok(json!({ "enabled": enabled }))
}
