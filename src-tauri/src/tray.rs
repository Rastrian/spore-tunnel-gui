//! System tray: per-tunnel start/stop without opening the main window.
//!
//! Layout: `Open` / separator / one `Start <name>` or `Stop <name>` item
//! per configured profile / separator / `Quit`. The menu (and the
//! tooltip's running count) is rebuilt only when the live state actually
//! moved away from what was built — the signature is the sorted list of
//! `(profile name, is_running)` pairs — so the 1/s status ticks of running
//! tunnels cost nothing. Refreshes are driven by the tunnel event stream
//! through [`TraySink`], which is fanned out next to the webview sink in
//! `main.rs`.
//!
//! Manual test checklist (a tray needs a desktop; not CI-testable):
//!
//! 1. App starts with a tray icon and tooltip "Spore Tunnel".
//! 2. Left-click the tray icon -> the main window shows, unminimizes and
//!    takes focus (also restores from a close-to-tray hide).
//! 3. Menu > Open -> same as 2.
//! 4. With one profile configured the item reads "Start <name>"; clicking
//!    it starts the tunnel and the item flips to "Stop <name>" within
//!    about a second (status events are coalesced to 1/s).
//! 5. "Stop <name>" stops the tunnel; the item flips back to
//!    "Start <name>".
//! 6. Start two tunnels -> the tooltip reads "Spore Tunnel — 2 running";
//!    stop both -> back to plain "Spore Tunnel".
//! 7. With close_to_tray on, the window's X hides the app; tray > Open
//!    restores it. With it off, the X quits the app.
//! 8. Quit while tunnels run: the process only exits after every control
//!    connection was closed (no ghost ports left on the server).
//!
//! Known limitation (by design, driven by events only): profiles created
//! or deleted while the app runs are picked up on the next state change
//! of any tunnel, not immediately.

use spore_tunnel_gui::config::{self, SecretStore};
use spore_tunnel_gui::tunnel::events::EventSink;
use spore_tunnel_gui::tunnel::manager::TunnelManager;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};
use uuid::Uuid;

/// Tray icon id (`tray_by_id` target).
const TRAY_ID: &str = "main";
/// Base tooltip; a running count is appended while tunnels run.
const TOOLTIP_BASE: &str = "Spore Tunnel";

// Menu item ids.
const ID_OPEN: &str = "open";
const ID_QUIT: &str = "quit";
const PREFIX_START: &str = "start:";
const PREFIX_STOP: &str = "stop:";

/// What a menu item asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrayAction {
    Open,
    Start(Uuid),
    Stop(Uuid),
    Quit,
}

/// Whether a tunnel state counts as "running" for tray labels, mirroring
/// `TunnelSupervisor::is_running` (a reconnecting tunnel is `starting`).
fn is_running_state(state: &str) -> bool {
    state == "starting" || state == "connected"
}

/// Menu label for a profile: `Stop <name>` while its tunnel runs,
/// `Start <name>` otherwise.
fn menu_label(name: &str, running: bool) -> String {
    format!("{} {name}", if running { "Stop" } else { "Start" })
}

/// Menu item id for a profile, embedding the action to take.
fn item_id(id: &Uuid, running: bool) -> String {
    format!("{}{id}", if running { PREFIX_STOP } else { PREFIX_START })
}

/// Tooltip text: `Spore Tunnel` idle, `Spore Tunnel — N running` while
/// tunnels run.
fn tooltip(running: usize) -> String {
    if running == 0 {
        TOOLTIP_BASE.to_string()
    } else {
        format!("{TOOLTIP_BASE} — {running} running")
    }
}

/// Identity of the menu contents: `(name, running)` pairs, sorted so the
/// signature is order-stable regardless of config order. Rebuild only
/// when this changes.
fn menu_signature(entries: &[(Uuid, String, bool)]) -> Vec<(String, bool)> {
    let mut signature: Vec<(String, bool)> = entries
        .iter()
        .map(|(_, name, running)| (name.clone(), *running))
        .collect();
    signature.sort();
    signature
}

/// Parse a menu item id into the action it requests; `None` for foreign
/// or malformed ids (never panic on a bad uuid).
fn parse_action(id: &str) -> Option<TrayAction> {
    match id {
        ID_OPEN => Some(TrayAction::Open),
        ID_QUIT => Some(TrayAction::Quit),
        _ => {
            if let Some(id) = id.strip_prefix(PREFIX_START) {
                Uuid::parse_str(id).ok().map(TrayAction::Start)
            } else {
                id.strip_prefix(PREFIX_STOP)
                    .and_then(|id| Uuid::parse_str(id).ok())
                    .map(TrayAction::Stop)
            }
        }
    }
}

/// Config-order `(id, name, running)` triples for every configured
/// profile; profiles without a tunnel in the manager are not running.
async fn collect_entries(
    cfg: &config::AppConfig,
    manager: &TunnelManager,
) -> Vec<(Uuid, String, bool)> {
    let running: HashMap<Uuid, bool> = manager
        .all_statuses()
        .await
        .into_iter()
        .map(|(id, status)| (id, is_running_state(&status.state)))
        .collect();
    cfg.profiles
        .iter()
        .map(|p| (p.id, p.name.clone(), running.get(&p.id).copied().unwrap_or(false)))
        .collect()
}

/// Build the tray menu for the given entries.
fn build_menu(
    app: &impl Manager<tauri::Wry>,
    entries: &[(Uuid, String, bool)],
) -> tauri::Result<Menu<tauri::Wry>> {
    let menu = Menu::with_items(
        app,
        &[
            &MenuItem::with_id(app, ID_OPEN, "Open", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
        ],
    )?;
    for (id, name, running) in entries {
        let item = MenuItem::with_id(
            app,
            item_id(id, *running),
            menu_label(name, *running),
            true,
            None::<&str>,
        )?;
        menu.append(&item)?;
    }
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(app, ID_QUIT, "Quit", true, None::<&str>)?)?;
    Ok(menu)
}

/// What the tray currently shows; the refresh path compares against it.
#[derive(Default)]
struct MenuState {
    built: Mutex<Built>,
}

#[derive(Default)]
struct Built {
    menu_signature: Vec<(String, bool)>,
    running_count: usize,
}

impl MenuState {
    fn lock(&self) -> MutexGuard<'_, Built> {
        self.built.lock().expect("tray menu state poisoned")
    }
}

/// Install the tray icon and its initial menu. Never fatal: on desktops
/// without a tray area (bare Linux WMs) this logs and the app keeps
/// running windowed. Called from `setup`, so the manager is brand new and
/// nothing can be running yet — the initial menu marks every profile
/// `Start <name>`.
pub fn init(app: &tauri::App) {
    if let Err(e) = try_init(app) {
        eprintln!("System tray unavailable, continuing without it: {e}");
    }
}

fn try_init(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let entries: Vec<(Uuid, String, bool)> = config::load_config()?
        .profiles
        .iter()
        .map(|p| (p.id, p.name.clone(), false))
        .collect();

    let menu = build_menu(app, &entries)?;
    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or("app has no default window icon")?;

    let state = MenuState::default();
    {
        let mut built = state.lock();
        built.menu_signature = menu_signature(&entries);
    }
    app.manage(state);

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .tooltip(TOOLTIP_BASE)
        .menu(&menu)
        // Left click opens the window (see on_tray_icon_event); the menu
        // opens on right click, as Windows users expect.
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| handle_menu_event(app, event.id.as_ref()))
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

/// [`EventSink`] feeding the tray: remembers each profile's last known
/// running bit (cheap) and only spawns a rebuild task when that bit
/// actually flips. Log lines and stats ticks can't change the menu, so
/// they are ignored.
pub struct TraySink {
    app: AppHandle,
    running: Mutex<HashMap<Uuid, bool>>,
}

impl TraySink {
    pub fn new(app: AppHandle) -> Self {
        Self {
            app,
            running: Mutex::new(HashMap::new()),
        }
    }
}

impl EventSink for TraySink {
    fn emit_status(
        &self,
        profile_id: &Uuid,
        status: &spore_tunnel_gui::tunnel::supervisor::TunnelStatus,
    ) {
        let running = is_running_state(&status.state);
        let mut seen = self.running.lock().expect("tray sink state poisoned");
        if seen.get(profile_id) == Some(&running) {
            return; // same running bit as before: the menu can't change
        }
        seen.insert(*profile_id, running);
        let app = self.app.clone();
        tauri::async_runtime::spawn(async move {
            refresh_menu(&app).await;
        });
    }

    fn emit_log(&self, _profile_id: &Uuid, _entry: &spore_tunnel_gui::tunnel::events::LogEntry) {}

    fn emit_stats(
        &self,
        _profile_id: &Uuid,
        _stats: &spore_tunnel_gui::tunnel::events::StatsSnapshot,
    ) {
    }
}

/// Re-read config + live statuses and rebuild the menu/tooltip, but only
/// if they differ from what was installed. Runs on the async runtime;
/// concurrent invocations collapse on the `MenuState` lock.
async fn refresh_menu(app: &AppHandle) {
    let Ok(cfg) = config::load_config() else {
        return;
    };
    let manager = app.state::<Arc<TunnelManager>>().inner().clone();
    let entries = collect_entries(&cfg, &manager).await;
    let signature = menu_signature(&entries);
    let running_count = entries.iter().filter(|(_, _, running)| *running).count();

    let Some(state) = app.try_state::<MenuState>() else {
        return;
    };
    let mut built = state.lock();
    if built.menu_signature != signature {
        let Some(tray) = app.tray_by_id(TRAY_ID) else {
            return;
        };
        match build_menu(app, &entries) {
            Ok(menu) => {
                let _ = tray.set_menu(Some(menu));
                built.menu_signature = signature;
            }
            Err(e) => eprintln!("Failed to rebuild tray menu: {e}"),
        }
    }
    if built.running_count != running_count {
        if let Some(tray) = app.tray_by_id(TRAY_ID) {
            let _ = tray.set_tooltip(Some(tooltip(running_count)));
        }
        built.running_count = running_count;
    }
}

fn handle_menu_event(app: &AppHandle, id: &str) {
    match parse_action(id) {
        Some(TrayAction::Open) => show_main_window(app),
        Some(TrayAction::Start(profile)) => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let manager = app.state::<Arc<TunnelManager>>().inner().clone();
                let store = app.state::<Arc<dyn SecretStore>>().inner().clone();
                if let Err(e) =
                    crate::commands::start_profile(profile, None, &manager, &store).await
                {
                    eprintln!("Tray start failed: {e}");
                }
            });
        }
        Some(TrayAction::Stop(profile)) => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                app.state::<Arc<TunnelManager>>().inner().stop(&profile).await;
            });
        }
        Some(TrayAction::Quit) => {
            // Stop every tunnel first: dropping the control connections
            // without the supervisors' goodbye leaves ghost ports on the
            // server. Then exit.
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                app.state::<Arc<TunnelManager>>().inner().stop_all().await;
                app.exit(0);
            });
        }
        None => {}
    }
}

/// Show, unminimize and focus the main window (also restores it from a
/// close-to-tray hide).
fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_label_follows_the_running_bit() {
        assert_eq!(menu_label("MC", false), "Start MC");
        assert_eq!(menu_label("MC", true), "Stop MC");
    }

    #[test]
    fn item_id_roundtrips_through_parse_action() {
        let id = Uuid::new_v4();
        assert_eq!(parse_action(&item_id(&id, false)), Some(TrayAction::Start(id)));
        assert_eq!(parse_action(&item_id(&id, true)), Some(TrayAction::Stop(id)));
        assert_eq!(parse_action(ID_OPEN), Some(TrayAction::Open));
        assert_eq!(parse_action(ID_QUIT), Some(TrayAction::Quit));
    }

    #[test]
    fn parse_action_ignores_foreign_and_malformed_ids() {
        assert_eq!(parse_action("start:not-a-uuid"), None);
        assert_eq!(parse_action("stop:"), None);
        assert_eq!(parse_action("whatever"), None);
        assert_eq!(parse_action(""), None);
    }

    #[test]
    fn tooltip_plain_when_idle_and_counted_when_running() {
        assert_eq!(tooltip(0), "Spore Tunnel");
        assert_eq!(tooltip(1), "Spore Tunnel — 1 running");
        assert_eq!(tooltip(2), "Spore Tunnel — 2 running");
    }

    #[test]
    fn is_running_state_matches_supervisor_semantics() {
        assert!(is_running_state("starting"));
        assert!(is_running_state("connected"));
        assert!(!is_running_state("idle"));
        assert!(!is_running_state("failed"));
        assert!(!is_running_state("stopped"));
    }

    fn entries(pairs: &[(&str, bool)]) -> Vec<(Uuid, String, bool)> {
        pairs
            .iter()
            .map(|(name, running)| (Uuid::new_v4(), name.to_string(), *running))
            .collect()
    }

    #[test]
    fn menu_signature_is_sorted_and_ignores_ids() {
        let a = entries(&[("MC", true), ("Web", false)]);
        let b = entries(&[("Web", false), ("MC", true)]);
        assert_eq!(menu_signature(&a), menu_signature(&b));
        assert_eq!(
            menu_signature(&a),
            vec![("MC".to_string(), true), ("Web".to_string(), false)]
        );
    }

    #[test]
    fn menu_signature_changes_only_on_name_or_running_changes() {
        let base = entries(&[("MC", false), ("Web", true)]);
        let sig = menu_signature(&base);
        // Fresh ids, same (name, running) pairs: same signature.
        assert_eq!(menu_signature(&entries(&[("MC", false), ("Web", true)])), sig);
        // A running bit flips the signature (this is the rebuild trigger).
        assert_ne!(menu_signature(&entries(&[("MC", true), ("Web", true)])), sig);
        // A renamed profile flips it too.
        assert_ne!(menu_signature(&entries(&[("MC2", false), ("Web", true)])), sig);
        // An added or removed profile flips it.
        assert_ne!(menu_signature(&entries(&[("MC", false)])), sig);
    }
}
