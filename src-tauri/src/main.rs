// Prevent console window from appearing on Windows release builds
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;

use commands::TunnelState;
use spore_tunnel_gui::config;
use spore_tunnel_gui::tunnel::supervisor::TunnelSupervisor;
use std::sync::Arc;
use tokio::sync::Mutex;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let tunnel_state: TunnelState = Arc::new(Mutex::new(TunnelSupervisor::new()));

    let result = tauri::Builder::default()
        .manage(tunnel_state)
        .invoke_handler(tauri::generate_handler![
            commands::load_config_cmd,
            commands::save_config_cmd,
            commands::start_tunnel,
            commands::stop_tunnel,
            commands::get_status,
            commands::copy_address,
            commands::open_config_folder,
        ])
        .run(tauri::generate_context!());

    if let Err(e) = result {
        eprintln!("Fatal error: {e}");
        // Same directory as the rest of the app's files (APP_DIR).
        if let Ok(dir) = config::config_dir() {
            let log_path = dir.join("crash.log");
            let _ = std::fs::create_dir_all(log_path.parent().unwrap());
            let _ = std::fs::write(&log_path, format!("Fatal error: {e}\n"));
        }
        std::process::exit(1);
    }
}

fn main() {
    run();
}
