//! Tauri commands for the local profile.
//!
//! Thin on purpose: every one of these resolves the manager from `AppState`,
//! converts base64 at the boundary and hands back the view shape. The seed
//! never crosses this line — the frontend can ask for a signature, never for
//! the key that made it.
//!
//! Errors are the stable codes from `crate::profile`: `ERR_PROFILE_NICKNAME`,
//! `ERR_PROFILE_STORE`, `ERR_PROFILE_SECRET`, `ERR_PROFILE_SIGN`, optionally
//! followed by `": "` and a detail, so the UI can map on the prefix.

use tauri::State;

use crate::profile::{identity, ProfilePublic, ProfileView};
use crate::AppState;

#[tauri::command]
pub async fn profile_get(state: State<'_, AppState>) -> Result<ProfileView, String> {
    state.profile.get().map(|p| p.view())
}

#[tauri::command]
pub async fn profile_set_nickname(
    state: State<'_, AppState>,
    nick: String,
) -> Result<ProfileView, String> {
    state.profile.set_nickname(&nick).map(|p| p.view())
}

#[tauri::command]
pub async fn profile_set_skin(
    state: State<'_, AppState>,
    id: String,
    tint: [u8; 3],
) -> Result<ProfileView, String> {
    state.profile.set_skin(&id, tint).map(|p| p.view())
}

/// Sign an arbitrary payload with the local identity. Base64 in, base64 out:
/// the message is bytes, not text, and JSON has no way to carry those.
#[tauri::command]
pub async fn profile_sign(state: State<'_, AppState>, msg_b64: String) -> Result<String, String> {
    let msg = identity::decode_b64(&msg_b64)?;
    let sig = state.profile.sign(&msg)?;
    Ok(identity::encode_signature(&sig))
}

#[tauri::command]
pub async fn profile_export_public(state: State<'_, AppState>) -> Result<ProfilePublic, String> {
    state.profile.public()
}
