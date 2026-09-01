//! Tauri command surface: profiles (config + keyring) and tunnels
//! (manager), mapping the old single-tunnel UI onto the ACTIVE profile.

use spore_tunnel_gui::config::{self, Profile, SecretStore, UiPrefs, CONFIG_FILE, KEYRING_SERVICE};
use spore_tunnel_gui::discover;
use spore_tunnel_gui::tunnel::events::LogEntry;
use spore_tunnel_gui::tunnel::manager::TunnelManager;
use spore_tunnel_gui::tunnel::supervisor::TunnelStatus;
use std::sync::Arc;
use std::time::Duration;
use tauri::State;

/// How long `start_tunnel` waits for the first connect attempt to settle
/// before returning the (still starting) status.
const CONNECT_RESULT_WAIT: Duration = Duration::from_secs(3);

/// One entry of `get_all_status`: a configured profile plus its tunnel
/// status (idle default when it has never been started).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileStatus {
    pub profile_id: uuid::Uuid,
    pub status: TunnelStatus,
}

// ---------------------------------------------------------------------
// Profiles
// ---------------------------------------------------------------------

#[tauri::command]
pub async fn list_profiles() -> Result<Vec<Profile>, String> {
    Ok(config::load_config()?.profiles)
}

/// Upsert a profile by id, with validation. Trims `name`/`server_host`.
#[tauri::command]
pub async fn save_profile(profile: Profile) -> Result<Profile, String> {
    let mut cfg = config::load_config()?;
    let profile = normalize_profile(profile, &cfg.profiles)?;
    match cfg.profiles.iter_mut().find(|p| p.id == profile.id) {
        Some(existing) => *existing = profile.clone(),
        None => cfg.profiles.push(profile.clone()),
    }
    if cfg.active_profile_id.is_none() {
        cfg.active_profile_id = Some(profile.id);
    }
    config::save_config(&cfg)?;
    Ok(profile)
}

#[tauri::command]
pub async fn set_active_profile(profile_id: uuid::Uuid) -> Result<(), String> {
    let mut cfg = config::load_config()?;
    if !cfg.profiles.iter().any(|p| p.id == profile_id) {
        return Err(format!("Profile {profile_id} not found."));
    }
    cfg.active_profile_id = Some(profile_id);
    config::save_config(&cfg)
}

/// Store (or, when empty/whitespace, delete) a profile's tunnel secret.
#[tauri::command]
pub async fn set_profile_secret(
    profile_id: uuid::Uuid,
    secret: String,
    store: State<'_, Arc<dyn SecretStore>>,
) -> Result<(), String> {
    let secret = secret.trim();
    let user = config::profile_secret_user(profile_id);
    if secret.is_empty() {
        // Absent entries are already "deleted"; ignore store errors here.
        let _ = store.delete_secret(KEYRING_SERVICE, &user);
        Ok(())
    } else {
        store.set_secret(KEYRING_SERVICE, &user, secret)
    }
}

#[tauri::command]
pub async fn delete_profile(
    profile_id: uuid::Uuid,
    manager: State<'_, Arc<TunnelManager>>,
    store: State<'_, Arc<dyn SecretStore>>,
) -> Result<(), String> {
    if manager.is_running(&profile_id).await {
        return Err("Stop the tunnel for this profile before deleting it.".to_string());
    }
    let mut cfg = config::load_config()?;
    let before = cfg.profiles.len();
    cfg.profiles.retain(|p| p.id != profile_id);
    if cfg.profiles.len() == before {
        return Err(format!("Profile {profile_id} not found."));
    }
    if cfg.active_profile_id == Some(profile_id) {
        cfg.active_profile_id = None;
    }
    config::save_config(&cfg)?;
    // Best-effort: a failing keyring must not block the delete.
    let _ = store.delete_secret(KEYRING_SERVICE, &config::profile_secret_user(profile_id));
    Ok(())
}

/// Import the legacy bore-tunnel-gui config as a new profile. Explicit
/// only (the frontend offers it); NEVER starts a tunnel.
#[tauri::command]
pub async fn import_legacy(
    store: State<'_, Arc<dyn SecretStore>>,
) -> Result<Option<Profile>, String> {
    let legacy_dir = config::legacy_config_dir()?;
    let Some(profile) = config::import_legacy(&legacy_dir, store.inner().as_ref())? else {
        return Ok(None);
    };
    let mut cfg = config::load_config()?;
    cfg.profiles.push(profile.clone());
    if cfg.active_profile_id.is_none() {
        cfg.active_profile_id = Some(profile.id);
    }
    config::save_config(&cfg)?;
    Ok(Some(profile))
}

#[tauri::command]
pub async fn has_legacy_config() -> Result<bool, String> {
    Ok(config::legacy_config_dir()?.join(CONFIG_FILE).exists())
}

// ---------------------------------------------------------------------
// Tunnels
// ---------------------------------------------------------------------

/// Start the tunnel for a profile. `secret` (when non-empty) is stored
/// best-effort and used; otherwise the stored secret ("" when none).
#[tauri::command]
pub async fn start_tunnel(
    profile_id: uuid::Uuid,
    secret: Option<String>,
    manager: State<'_, Arc<TunnelManager>>,
    store: State<'_, Arc<dyn SecretStore>>,
) -> Result<TunnelStatus, String> {
    start_profile(profile_id, secret, manager.inner(), store.inner()).await
}

/// Body of [`start_tunnel`], shared with the tray's `start:<uuid>` menu
/// item so both paths behave identically. Resolves the profile from the
/// config, resolves its secret (argument > keyring > empty) and starts
/// it, then waits briefly for the first connect attempt to settle.
pub(crate) async fn start_profile(
    profile_id: uuid::Uuid,
    secret: Option<String>,
    manager: &Arc<TunnelManager>,
    store: &Arc<dyn SecretStore>,
) -> Result<TunnelStatus, String> {
    let cfg = config::load_config()?;
    let profile = cfg
        .profiles
        .iter()
        .find(|p| p.id == profile_id)
        .cloned()
        .ok_or_else(|| format!("Profile {profile_id} not found."))?;

    let user = config::profile_secret_user(profile_id);
    let secret = match secret.as_deref().map(str::trim) {
        Some(s) if !s.is_empty() => {
            let s = s.to_string();
            // Remember it for next time, but never fail the start on it.
            let _ = store.set_secret(KEYRING_SERVICE, &user, &s);
            s
        }
        _ => store.get_secret(KEYRING_SERVICE, &user)?.unwrap_or_default(),
    };

    manager.start(profile, secret).await?;
    manager
        .wait_for(&profile_id, &["connected", "failed"], CONNECT_RESULT_WAIT)
        .await
        .ok_or_else(|| "Tunnel failed to start.".to_string())
}

/// `None` targets the active profile. Errors when there is neither an
/// explicit id nor an active profile, or when the tunnel is unknown.
#[tauri::command]
pub async fn stop_tunnel(
    profile_id: Option<uuid::Uuid>,
    manager: State<'_, Arc<TunnelManager>>,
) -> Result<(), String> {
    let id = resolve_target(profile_id)?;
    if !manager.stop(&id).await {
        return Err("No tunnel is running for this profile.".to_string());
    }
    Ok(())
}

/// `None` targets the active profile. Unknown/never-started profiles
/// report the idle default so the legacy UI can keep polling cleanly.
#[tauri::command]
pub async fn get_status(
    profile_id: Option<uuid::Uuid>,
    manager: State<'_, Arc<TunnelManager>>,
) -> Result<TunnelStatus, String> {
    let id = resolve_target(profile_id)?;
    Ok(manager.status_of(&id).await.unwrap_or_default())
}

#[tauri::command]
pub async fn get_all_status(
    manager: State<'_, Arc<TunnelManager>>,
) -> Result<Vec<ProfileStatus>, String> {
    let cfg = config::load_config()?;
    let mut out = Vec::with_capacity(cfg.profiles.len());
    for profile in &cfg.profiles {
        let status = manager.status_of(&profile.id).await.unwrap_or_default();
        out.push(ProfileStatus {
            profile_id: profile.id,
            status,
        });
    }
    Ok(out)
}

#[tauri::command]
pub async fn get_tunnel_log(
    profile_id: uuid::Uuid,
    since_index: Option<u64>,
    manager: State<'_, Arc<TunnelManager>>,
) -> Result<Vec<LogEntry>, String> {
    manager
        .logs_of(&profile_id, since_index)
        .await
        .ok_or_else(|| format!("No tunnel logs for profile {profile_id}."))
}

/// `None` targets the active profile.
#[tauri::command]
pub async fn copy_address(
    profile_id: Option<uuid::Uuid>,
    manager: State<'_, Arc<TunnelManager>>,
) -> Result<String, String> {
    let id = resolve_target(profile_id)?;
    manager
        .status_of(&id)
        .await
        .and_then(|s| s.remote_address)
        .ok_or_else(|| "No remote address available.".to_string())
}

#[tauri::command]
pub async fn open_config_folder() -> Result<(), String> {
    let dir = config::config_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create config dir: {e}"))?;
    open::that(&dir).map_err(|e| format!("Failed to open folder: {e}"))
}

// ---------------------------------------------------------------------
// Local service detection
// ---------------------------------------------------------------------

/// Probe the well-known local ports on 127.0.0.1 (wizard step 2).
#[tauri::command]
pub async fn detect_local_service() -> Result<Vec<discover::DetectedService>, String> {
    Ok(discover::detect_local_service().await)
}

// ---------------------------------------------------------------------
// UI preferences
// ---------------------------------------------------------------------

/// Current window/tray behavior prefs. Read once at startup (theme,
/// start-minimized) and whenever the settings view is opened.
#[tauri::command]
pub async fn get_ui_prefs() -> Result<UiPrefs, String> {
    Ok(config::load_config()?.ui)
}

/// Replace the prefs wholesale. Taking `UiPrefs` as the arg type means
/// serde validates it for free — an unknown `theme` string never reaches
/// this body, it fails the invoke instead.
#[tauri::command]
pub async fn update_ui_prefs(prefs: UiPrefs) -> Result<UiPrefs, String> {
    let mut cfg = config::load_config()?;
    apply_ui_prefs(&mut cfg, prefs);
    config::save_config(&cfg)?;
    Ok(cfg.ui)
}

// ---------------------------------------------------------------------
// Pure helpers (unit-tested)
// ---------------------------------------------------------------------

/// Trim `name`/`server_host` and enforce the profile rules:
/// non-empty name (≤ 64 chars, unique), non-empty host, non-zero
/// server/local ports.
fn normalize_profile(profile: Profile, others: &[Profile]) -> Result<Profile, String> {
    let mut profile = profile;
    profile.name = profile.name.trim().to_string();
    profile.server_host = profile.server_host.trim().to_string();

    if profile.name.is_empty() {
        return Err("Profile name is required.".to_string());
    }
    if profile.name.chars().count() > 64 {
        return Err("Profile name must be 64 characters or fewer.".to_string());
    }
    if others
        .iter()
        .any(|p| p.id != profile.id && p.name.eq_ignore_ascii_case(&profile.name))
    {
        return Err(format!("A profile named \"{}\" already exists.", profile.name));
    }
    if profile.server_host.is_empty() {
        return Err("Server host is required.".to_string());
    }
    if profile.server_port == 0 {
        return Err("Server port cannot be 0.".to_string());
    }
    if profile.local_port == 0 {
        return Err("Local port cannot be 0.".to_string());
    }
    Ok(profile)
}

/// Resolve the command target: an explicit id wins; otherwise the active
/// profile must exist (loaded from the real config).
fn resolve_target(profile_id: Option<uuid::Uuid>) -> Result<uuid::Uuid, String> {
    let active = config::load_config()?.active_profile_id;
    resolve_target_with(profile_id, active)
}

/// Pure core of [`resolve_target`].
fn resolve_target_with(
    requested: Option<uuid::Uuid>,
    active: Option<uuid::Uuid>,
) -> Result<uuid::Uuid, String> {
    match (requested, active) {
        (Some(id), _) => Ok(id),
        (None, Some(id)) => Ok(id),
        (None, None) => Err("No active profile.".to_string()),
    }
}

/// Pure core of [`update_ui_prefs`]: swap the config's `ui` section for
/// the given prefs (the command then persists and returns them).
fn apply_ui_prefs(config: &mut config::AppConfig, prefs: UiPrefs) {
    config.ui = prefs;
}

#[cfg(test)]
mod tests {
    use super::*;
    use spore_tunnel_gui::config::Theme;
    use uuid::Uuid;

    fn profile(name: &str, host: &str) -> Profile {
        Profile {
            id: Uuid::new_v4(),
            name: name.to_string(),
            server_host: host.to_string(),
            ..Profile::default()
        }
    }

    #[test]
    fn normalize_trims_and_accepts_a_valid_profile() {
        let p = normalize_profile(profile("  MC  ", "  bore.pub  "), &[]).unwrap();
        assert_eq!(p.name, "MC");
        assert_eq!(p.server_host, "bore.pub");
    }

    #[test]
    fn normalize_rejects_bad_input_descriptively() {
        let cases: [(Profile, &str); 5] = [
            (profile("   ", "bore.pub"), "name is required"),
            (profile(&"x".repeat(65), "bore.pub"), "64 characters"),
            (profile("mc", ""), "Server host is required"),
            (
                Profile {
                    server_port: 0,
                    ..profile("mc", "bore.pub")
                },
                "Server port cannot be 0",
            ),
            (
                Profile {
                    local_port: 0,
                    ..profile("mc", "bore.pub")
                },
                "Local port cannot be 0",
            ),
        ];
        for (input, needle) in cases {
            let err = normalize_profile(input, &[]).unwrap_err();
            assert!(err.contains(needle), "expected \"{needle}\", got \"{err}\"");
        }
    }

    #[test]
    fn normalize_rejects_duplicate_names_but_allows_itself() {
        let existing = profile("mc", "bore.pub");
        let dup = Profile {
            id: Uuid::new_v4(),
            ..existing.clone()
        };
        assert!(normalize_profile(dup, std::slice::from_ref(&existing)).is_err());
        // Same id (an edit keeping its name) is fine.
        assert!(normalize_profile(existing.clone(), &[existing]).is_ok());
    }

    #[test]
    fn resolve_target_prefers_the_explicit_id_then_the_active_profile() {
        let id = Uuid::new_v4();
        let other = Uuid::new_v4();
        assert_eq!(resolve_target_with(Some(id), Some(other)).unwrap(), id);
        assert_eq!(
            resolve_target_with(None, Some(id)).unwrap(),
            id,
            "active profile must be used when no explicit id is given"
        );
        assert_eq!(
            resolve_target_with(None, None).unwrap_err(),
            "No active profile."
        );
    }

    fn temp_dir() -> std::path::PathBuf {
        tempfile::tempdir().unwrap().keep()
    }

    #[test]
    fn ui_prefs_apply_and_roundtrip_through_save_load() {
        let dir = temp_dir();
        let path = dir.join(CONFIG_FILE);
        let prefs = UiPrefs {
            theme: Theme::Dark,
            start_minimized: true,
            close_to_tray: false,
        };

        let mut cfg = config::AppConfig::default();
        apply_ui_prefs(&mut cfg, prefs);
        assert_eq!(cfg.ui, prefs, "apply must replace the ui section");
        cfg.save_to(&path).unwrap();

        // camelCase + lowercase theme in the stored JSON.
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("\"theme\": \"dark\""));
        assert!(raw.contains("\"startMinimized\": true"));
        assert!(raw.contains("\"closeToTray\": false"));

        let loaded = config::AppConfig::load_from(&path).unwrap();
        assert_eq!(loaded.ui, prefs);
    }

    #[test]
    fn ui_prefs_theme_variants_roundtrip() {
        let dir = temp_dir();
        let path = dir.join(CONFIG_FILE);
        for (theme, wire) in
            [(Theme::Dark, "dark"), (Theme::Light, "light"), (Theme::System, "system")]
        {
            let prefs = UiPrefs {
                theme,
                ..UiPrefs::default()
            };
            // Wire shape: camelCase keys, lowercase theme — exactly what
            // the frontend sends and `get_ui_prefs` returns.
            assert_eq!(
                serde_json::to_value(prefs).unwrap(),
                serde_json::json!({
                    "theme": wire,
                    "startMinimized": false,
                    "closeToTray": true,
                })
            );

            let mut cfg = config::AppConfig::default();
            apply_ui_prefs(&mut cfg, prefs);
            cfg.save_to(&path).unwrap();
            assert_eq!(
                config::AppConfig::load_from(&path).unwrap().ui,
                prefs,
                "{wire:?} must survive a save/load cycle"
            );
        }
    }

    #[test]
    fn ui_prefs_reject_unknown_theme() {
        // This is the validation the invoke gets for free via the arg type.
        assert!(serde_json::from_str::<UiPrefs>(r#"{"theme": "blue"}"#).is_err());
        // ...and every valid theme string deserializes.
        for wire in ["dark", "light", "system"] {
            let json = format!(r#"{{"theme": "{wire}"}}"#);
            let prefs: UiPrefs = serde_json::from_str(&json).unwrap();
            assert_eq!(
                serde_json::to_value(prefs.theme).unwrap(),
                serde_json::json!(wire)
            );
        }
    }
}
