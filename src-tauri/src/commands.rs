use crate::bore::{BoreClient, TunnelStatus};
use crate::config::{self, AppConfig};
use std::sync::Arc;
use tokio::sync::Mutex;
use tauri::State;

pub type TunnelState = Arc<Mutex<BoreClient>>;

#[tauri::command]
pub async fn load_config_cmd() -> Result<AppConfig, String> {
    config::load_config()
}

#[tauri::command]
pub async fn save_config_cmd(config: AppConfig) -> Result<(), String> {
    config::save_config(&config)
}

#[tauri::command]
pub async fn save_secret_cmd(secret: String) -> Result<(), String> {
    config::save_secret(secret.trim())
}

#[tauri::command]
pub async fn has_secret_cmd() -> Result<bool, String> {
    Ok(config::has_secret())
}

#[tauri::command]
pub async fn start_tunnel(
    state: State<'_, TunnelState>,
    config: AppConfig,
    secret: String,
) -> Result<TunnelStatus, String> {
    let secret = secret.trim().to_string();

    // Try to save to keyring for next time (best-effort, don't fail if it doesn't work)
    if !secret.is_empty() {
        let _ = config::save_secret(&secret);
    }

    let mut client = state.lock().await;

    client
        .start(
            &config.bore_server_host,
            config.bore_server_port.unwrap_or(7835),
            config.local_port,
            config.remote_port,
            &secret,
        )
        .await?;

    Ok(client.status().await)
}

#[tauri::command]
pub async fn stop_tunnel(state: State<'_, TunnelState>) -> Result<(), String> {
    let mut client = state.lock().await;
    client.stop().await
}

#[tauri::command]
pub async fn get_status(state: State<'_, TunnelState>) -> Result<TunnelStatus, String> {
    let client = state.lock().await;
    Ok(client.status().await)
}

#[tauri::command]
pub async fn copy_address(state: State<'_, TunnelState>) -> Result<String, String> {
    let client = state.lock().await;
    let s = client.status().await;
    s.remote_address.ok_or_else(|| "No remote address available.".to_string())
}

#[tauri::command]
pub async fn open_config_folder() -> Result<(), String> {
    let dir = config::config_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create config dir: {e}"))?;
    open::that(&dir).map_err(|e| format!("Failed to open folder: {e}"))
}
