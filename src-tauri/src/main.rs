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
        // "Start Spore Tunnel at login" — registered disabled; the settings
        // toggle (JS side) enables/disables the OS launch entry.
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
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

            // Launch auto-connect: start every profile marked `autostart`,
            // one after another (config order) so tray/status updates land
            // in a predictable sequence. Window visibility is governed by
            // start_minimized above — tunnels connect either way, and a
            // local service that is not up yet is fine: profiles default
            // to auto_reconnect, which retries with backoff until it is.
            if auto_connect_allowed() {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let cfg = config::load_config().unwrap_or_default();
                    for profile in profiles_to_auto_connect(&cfg) {
                        let manager = handle.state::<Arc<TunnelManager>>().inner().clone();
                        let store = handle.state::<Arc<dyn SecretStore>>().inner().clone();
                        if let Err(e) =
                            commands::start_profile(profile.id, None, &manager, &store).await
                        {
                            eprintln!("Autostart connect failed for {}: {e}", profile.name);
                        }
                    }
                });
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

/// Profiles to connect on launch, in config order: exactly those with
/// `autostart: true`.
fn profiles_to_auto_connect(cfg: &config::AppConfig) -> Vec<config::Profile> {
    cfg.profiles.iter().filter(|p| p.autostart).cloned().collect()
}

/// Whether launch-time auto-connect may run. Off under CI and when
/// `SPORE_NO_AUTOSTART` is set, so test/e2e environments running the real
/// binary never dial configured tunnel servers.
fn auto_connect_allowed() -> bool {
    std::env::var_os("CI").is_none() && std::env::var_os("SPORE_NO_AUTOSTART").is_none()
}

fn main() {
    run();
}

#[cfg(test)]
mod tests {
    use super::*;
    use spore_tunnel_gui::config::Profile;

    fn profile(name: &str, autostart: bool) -> Profile {
        Profile {
            id: Uuid::new_v4(),
            name: name.to_string(),
            autostart,
            ..Profile::default()
        }
    }

    #[test]
    fn auto_connect_selects_autostart_profiles_in_config_order() {
        let cfg = config::AppConfig {
            profiles: vec![
                profile("mc", true),
                profile("web", false),
                profile("game", true),
            ],
            ..config::AppConfig::default()
        };
        let picked = profiles_to_auto_connect(&cfg);
        assert_eq!(
            picked.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            vec!["mc", "game"],
            "config order preserved, non-autostart skipped"
        );
    }

    #[test]
    fn auto_connect_with_no_autostart_profiles_is_empty() {
        let cfg = config::AppConfig {
            profiles: vec![profile("a", false)],
            ..config::AppConfig::default()
        };
        assert!(profiles_to_auto_connect(&cfg).is_empty());
        assert!(profiles_to_auto_connect(&config::AppConfig::default()).is_empty());
    }
}
