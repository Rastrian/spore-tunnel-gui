// Prevent console window from appearing on Windows release builds
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod bore;
mod commands;
mod config;

use bore::BoreClient;
use commands::TunnelState;
use std::sync::Arc;
use tokio::sync::Mutex;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let tunnel_state: TunnelState = Arc::new(Mutex::new(BoreClient::new()));

    let result = tauri::Builder::default()
        .manage(tunnel_state)
        .invoke_handler(tauri::generate_handler![
            commands::load_config_cmd,
            commands::save_config_cmd,
            commands::save_secret_cmd,
            commands::has_secret_cmd,
            commands::start_tunnel,
            commands::stop_tunnel,
            commands::get_status,
            commands::copy_address,
            commands::open_config_folder,
        ])
        .run(tauri::generate_context!());

    if let Err(e) = result {
        eprintln!("Fatal error: {e}");
        if let Some(dir) = dirs::config_dir() {
            let log_path = dir.join("bore-minecraft-tunnel").join("crash.log");
            let _ = std::fs::create_dir_all(log_path.parent().unwrap());
            let _ = std::fs::write(&log_path, format!("Fatal error: {e}\n"));
        }
        std::process::exit(1);
    }
}

fn main() {
    run();
}
