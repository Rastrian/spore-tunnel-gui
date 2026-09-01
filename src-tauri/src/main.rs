// Prevent console window from appearing on Windows release builds
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app_events;
mod commands;
mod tray;

use spore_tunnel_gui::config::{self, KeyringStore, SecretStore};
use spore_tunnel_gui::tunnel::events::{EventSink, LogEntry, StatsSnapshot};
use spore_tunnel_gui::tunnel::manager::TunnelManager;
use spore_tunnel_gui::tunnel::supervisor::TunnelStatus;
use std::sync::Arc;
use tauri::Manager;
use uuid::Uuid;

/// Fans every tunnel event out to all consumers (webview + tray), so
/// [`TunnelManager`] keeps its single-sink constructor.
struct FanOutSink {
    sinks: Vec<Arc<dyn EventSink>>,
}

impl EventSink for FanOutSink {
    fn emit_status(&self, profile_id: &Uuid, status: &TunnelStatus) {
        for sink in &self.sinks {
            sink.emit_status(profile_id, status);
        }
    }

    fn emit_log(&self, profile_id: &Uuid, entry: &LogEntry) {
        for sink in &self.sinks {
            sink.emit_log(profile_id, entry);
        }
    }

    fn emit_stats(&self, profile_id: &Uuid, stats: &StatsSnapshot) {
        for sink in &self.sinks {
            sink.emit_stats(profile_id, stats);
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let result = tauri::Builder::default()
        .setup(|app| {
            let handle = app.handle().clone();
            let sink: Arc<dyn EventSink> = Arc::new(FanOutSink {
                sinks: vec![
                    Arc::new(app_events::AppEventSink::new(handle.clone())),
                    Arc::new(tray::TraySink::new(handle)),
                ],
            });
            let tunnel_manager = Arc::new(TunnelManager::new(sink));
            let secret_store: Arc<dyn SecretStore> = Arc::new(KeyringStore);
            app.manage(tunnel_manager);
            app.manage(secret_store);

            // Non-fatal: desktops without a tray area keep running windowed.
            tray::init(app);

            if start_minimized() {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.hide();
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            let tauri::WindowEvent::CloseRequested { api, .. } = event else {
                return;
            };
            // Rare event, a config file read is fine here.
            let close_to_tray = config::load_config()
                .map(|cfg| cfg.ui.close_to_tray)
                .unwrap_or(false);
            if close_to_tray {
                api.prevent_close();
                let _ = window.hide();
            }
            // Otherwise let the close proceed: process exit drops the
            // control TCP connections, which the servers notice. The
            // supervisors get no chance to finish reconnect backoffs, but
            // nothing is left listening locally.
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
            commands::check_for_updates,
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

/// Whether the main window should start hidden to the tray. Config read
/// failures fall back to "show the window" (the visible default).
fn start_minimized() -> bool {
    config::load_config()
        .map(|cfg| cfg.ui.start_minimized)
        .unwrap_or(false)
}

fn main() {
    run();
}
