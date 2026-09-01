// Prevent console window from appearing on Windows release builds
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app_events;
mod commands;

use spore_tunnel_gui::config::{KeyringStore, SecretStore};
use spore_tunnel_gui::tunnel::manager::TunnelManager;
use std::sync::Arc;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let result = tauri::Builder::default()
        .setup(|app| {
            let sink = Arc::new(app_events::AppEventSink::new(app.handle().clone()));
            let tunnel_manager = Arc::new(TunnelManager::new(sink));
            let secret_store: Arc<dyn SecretStore> = Arc::new(KeyringStore);
            app.manage(tunnel_manager);
            app.manage(secret_store);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_profiles,
            commands::save_profile,
            commands::set_active_profile,
            commands::set_profile_secret,
            commands::delete_profile,
            commands::import_legacy,
            commands::has_legacy_config,
            commands::start_tunnel,
            commands::stop_tunnel,
            commands::get_status,
            commands::get_all_status,
            commands::get_tunnel_log,
            commands::copy_address,
            commands::open_config_folder,
            commands::detect_local_service,
            commands::get_ui_prefs,
            commands::update_ui_prefs,
        ])
        .run(tauri::generate_context!());

    if let Err(e) = result {
        eprintln!("Fatal error: {e}");
        // Same directory as the rest of the app's files (APP_DIR).
        if let Ok(dir) = spore_tunnel_gui::config::config_dir() {
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
