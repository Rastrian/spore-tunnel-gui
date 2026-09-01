use spore_tunnel_gui::config::{self, AppConfig, Profile};
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

/// The profile the single-tunnel UI acts on: the configured active one,
/// else the first profile.
fn active_profile(cfg: &AppConfig) -> Result<Profile, String> {
    cfg.profiles
        .iter()
        .find(|p| Some(p.id) == cfg.active_profile_id)
        .or_else(|| cfg.profiles.first())
        .cloned()
        .ok_or_else(|| "No profile configured.".to_string())
}

#[tauri::command]
pub async fn load_config_cmd() -> Result<AppConfig, String> {
    config::load_config()
}

#[tauri::command]
pub async fn save_config_cmd(config: AppConfig) -> Result<(), String> {
    config::save_config(&config)
}

#[tauri::command]
pub async fn start_tunnel(
    state: State<'_, TunnelState>,
    config: AppConfig,
    secret: String,
) -> Result<TunnelStatus, String> {
    let profile = active_profile(&config)?;
    let secret = secret.trim().to_string();

    let mut supervisor_config = SupervisorConfig::new(
        TunnelConfig {
            server: profile.server_host.clone(),
            control_port: profile.server_port,
            remote_port: profile.remote_port,
        },
        secret,
        profile.local_host.clone(),
        profile.local_port,
    );
    supervisor_config.auto_reconnect = profile.auto_reconnect;

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
