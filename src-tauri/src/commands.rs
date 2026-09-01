use crate::config::{self, AppConfig};
use spore_tunnel_gui::tunnel::client::TunnelConfig;
use spore_tunnel_gui::tunnel::supervisor::{SupervisorConfig, TunnelStatus, TunnelSupervisor};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tauri::State;

pub type TunnelState = Arc<Mutex<TunnelSupervisor>>;

/// How long `start_tunnel` waits for the first connect attempt to settle
/// before returning the (still starting) status.
const CONNECT_RESULT_WAIT: Duration = Duration::from_secs(3);

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

    let supervisor_config = SupervisorConfig::new(
        TunnelConfig {
            server: config.bore_server_host.clone(),
            control_port: config.bore_server_port.unwrap_or(7835),
            remote_port: config.remote_port,
        },
        secret,
        config.local_host.clone(),
        config.local_port,
    );

    let supervisor = state.lock().await;
    supervisor.start(supervisor_config).await?;

    Ok(supervisor
        .wait_for(&["connected", "failed"], CONNECT_RESULT_WAIT)
        .await)
}

#[tauri::command]
pub async fn stop_tunnel(state: State<'_, TunnelState>) -> Result<(), String> {
    let supervisor = state.lock().await;
    supervisor.stop().await;
    Ok(())
}

#[tauri::command]
pub async fn get_status(state: State<'_, TunnelState>) -> Result<TunnelStatus, String> {
    let supervisor = state.lock().await;
    Ok(supervisor.status())
}

#[tauri::command]
pub async fn copy_address(state: State<'_, TunnelState>) -> Result<String, String> {
    let supervisor = state.lock().await;
    let status = supervisor.status();
    status
        .remote_address
        .ok_or_else(|| "No remote address available.".to_string())
}

#[tauri::command]
pub async fn open_config_folder() -> Result<(), String> {
    let dir = config::config_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create config dir: {e}"))?;
    open::that(&dir).map_err(|e| format!("Failed to open folder: {e}"))
}
