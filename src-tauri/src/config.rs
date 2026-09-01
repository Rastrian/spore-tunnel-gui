//! Multi-profile configuration.
//!
//! * [`AppConfig`] — pretty-printed `config.json` under the app config dir
//!   (`%APPDATA%/spore-tunnel-gui` on Windows). A missing file is defaults,
//!   never an error.
//! * [`SecretStore`] — per-profile secrets live in the OS keyring, behind a
//!   trait so tests use [`MemorySecretStore`] and never touch the OS store.
//! * [`import_legacy`] — explicit one-shot import from the old
//!   `bore-minecraft-tunnel` app (the frontend decides when to offer it;
//!   it is NEVER run automatically).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use uuid::Uuid;

const APP_DIR: &str = "spore-tunnel-gui";
/// File name inside the app config dir (and inside the legacy dir).
pub const CONFIG_FILE: &str = "config.json";
/// Keyring service holding per-profile tunnel secrets.
pub const KEYRING_SERVICE: &str = "spore-tunnel-gui";
/// Legacy app constants (bore-tunnel-gui) — only used by [`import_legacy`].
pub const LEGACY_KEYRING_SERVICE: &str = "bore-minecraft-tunnel";
pub const LEGACY_KEYRING_USER: &str = "bore-secret";
const LEGACY_APP_DIR: &str = "bore-minecraft-tunnel";
/// Legacy configs omitted `bore_server_port`; bore's default control port.
const LEGACY_DEFAULT_SERVER_PORT: u16 = 7835;

/// One tunnel destination. Serialized camelCase in `config.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Profile {
    /// Fresh random id when missing from the JSON.
    pub id: Uuid,
    pub name: String,
    pub server_host: String,
    pub server_port: u16,
    pub local_host: String,
    pub local_port: u16,
    /// 0 = let the server assign a random remote port.
    pub remote_port: u16,
    pub autostart: bool,
    pub auto_reconnect: bool,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            name: String::new(),
            server_host: String::new(),
            server_port: 7835,
            local_host: "127.0.0.1".to_string(),
            local_port: 25565,
            remote_port: 0,
            autostart: false,
            auto_reconnect: true,
        }
    }
}

/// UI theme preference.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Dark,
    Light,
    #[default]
    System,
}

/// Window/tray behavior preferences.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UiPrefs {
    pub theme: Theme,
    pub start_minimized: bool,
    pub close_to_tray: bool,
}

impl Default for UiPrefs {
    fn default() -> Self {
        Self {
            theme: Theme::System,
            start_minimized: false,
            close_to_tray: true,
        }
    }
}

/// Root of `config.json`. Every field defaults, so `{}` and older partial
/// files always load.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppConfig {
    pub profiles: Vec<Profile>,
    pub active_profile_id: Option<Uuid>,
    pub ui: UiPrefs,
}

impl AppConfig {
    /// Load from an explicit path (tests use a tempdir). A missing file is
    /// the default config, never an error.
    pub fn load_from(path: &Path) -> Result<Self, String> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let data =
            fs::read_to_string(path).map_err(|e| format!("Failed to read config: {e}"))?;
        serde_json::from_str(&data).map_err(|e| format!("Failed to parse config: {e}"))
    }

    /// Pretty-printed JSON, creating the parent directory when needed.
    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("Failed to create config dir: {e}"))?;
        }
        let data = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize config: {e}"))?;
        fs::write(path, data).map_err(|e| format!("Failed to write config: {e}"))
    }
}

pub fn config_dir() -> Result<PathBuf, String> {
    let dir = dirs::config_dir().ok_or("Cannot find config directory")?;
    Ok(dir.join(APP_DIR))
}

fn config_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join(CONFIG_FILE))
}

pub fn load_config() -> Result<AppConfig, String> {
    AppConfig::load_from(&config_path()?)
}

pub fn save_config(config: &AppConfig) -> Result<(), String> {
    config.save_to(&config_path()?)
}

/// Keyring user name for a profile's secret: `profile:<uuid>`.
pub fn profile_secret_user(id: Uuid) -> String {
    format!("profile:{id}")
}

/// Persistent secret storage. Production uses the OS keyring; tests use
/// [`MemorySecretStore`] so they never touch it.
pub trait SecretStore: Send + Sync {
    fn set_secret(&self, service: &str, user: &str, secret: &str) -> Result<(), String>;
    /// `Ok(None)` when no secret is stored.
    fn get_secret(&self, service: &str, user: &str) -> Result<Option<String>, String>;
    /// `Ok(())` when no secret is stored.
    fn delete_secret(&self, service: &str, user: &str) -> Result<(), String>;
}

/// OS-keyring backed [`SecretStore`] (production).
pub struct KeyringStore;

impl SecretStore for KeyringStore {
    fn set_secret(&self, service: &str, user: &str, secret: &str) -> Result<(), String> {
        let entry = keyring::Entry::new(service, user)
            .map_err(|e| format!("Failed to create keyring entry: {e}"))?;
        entry
            .set_password(secret)
            .map_err(|e| format!("Failed to save secret: {e}"))
    }

    fn get_secret(&self, service: &str, user: &str) -> Result<Option<String>, String> {
        let entry = keyring::Entry::new(service, user)
            .map_err(|e| format!("Failed to create keyring entry: {e}"))?;
        match entry.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(format!("Failed to read secret: {e}")),
        }
    }

    fn delete_secret(&self, service: &str, user: &str) -> Result<(), String> {
        let entry = keyring::Entry::new(service, user)
            .map_err(|e| format!("Failed to create keyring entry: {e}"))?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(format!("Failed to delete secret: {e}")),
        }
    }
}

/// In-memory [`SecretStore`] for tests.
#[derive(Default)]
pub struct MemorySecretStore {
    entries: Mutex<HashMap<(String, String), String>>,
}

impl MemorySecretStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl SecretStore for MemorySecretStore {
    fn set_secret(&self, service: &str, user: &str, secret: &str) -> Result<(), String> {
        self.entries
            .lock()
            .unwrap()
            .insert((service.to_string(), user.to_string()), secret.to_string());
        Ok(())
    }

    fn get_secret(&self, service: &str, user: &str) -> Result<Option<String>, String> {
        Ok(self
            .entries
            .lock()
            .unwrap()
            .get(&(service.to_string(), user.to_string()))
            .cloned())
    }

    fn delete_secret(&self, service: &str, user: &str) -> Result<(), String> {
        self.entries
            .lock()
            .unwrap()
            .remove(&(service.to_string(), user.to_string()));
        Ok(())
    }
}

/// Legacy config dir of the old app (`%APPDATA%/bore-minecraft-tunnel`).
pub fn legacy_config_dir() -> Result<PathBuf, String> {
    let dir = dirs::config_dir().ok_or("Cannot find config directory")?;
    Ok(dir.join(LEGACY_APP_DIR))
}

/// Old flat schema written by bore-tunnel-gui. Missing optional fields
/// take the legacy app's own defaults.
#[derive(Debug, Deserialize)]
struct LegacyConfig {
    #[serde(default)]
    bore_server_host: String,
    #[serde(default)]
    bore_server_port: Option<u16>,
    #[serde(default = "default_local_host")]
    local_host: String,
    #[serde(default = "default_local_port")]
    local_port: u16,
    // 0 = random remote port, same meaning as today.
    #[serde(default)]
    remote_port: u16,
    // Accepted for forward compatibility; the import always names the
    // profile deterministically ("Imported - <host>").
    #[serde(default)]
    #[allow(dead_code)]
    profile_name: Option<String>,
}

fn default_local_host() -> String {
    "127.0.0.1".to_string()
}

fn default_local_port() -> u16 {
    25565
}

/// Import the legacy app's config (explicit only — the frontend decides).
///
/// * Missing `config.json` or an empty `bore_server_host` → `Ok(None)`.
/// * Otherwise builds a fresh profile (new id, name `Imported - <host>`)
///   and, when the legacy keyring holds a secret, copies it to
///   `profile:<id>` under [`KEYRING_SERVICE`].
///
/// This never writes the app config and never starts anything — it just
/// returns the profile for the caller to append.
pub fn import_legacy(legacy_dir: &Path, store: &dyn SecretStore) -> Result<Option<Profile>, String> {
    let path = legacy_dir.join(CONFIG_FILE);
    if !path.exists() {
        return Ok(None);
    }
    let data = fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read legacy config: {e}"))?;
    let legacy: LegacyConfig = serde_json::from_str(&data)
        .map_err(|e| format!("Failed to parse legacy config: {e}"))?;
    if legacy.bore_server_host.trim().is_empty() {
        return Ok(None);
    }

    let id = Uuid::new_v4();
    let profile = Profile {
        id,
        name: format!("Imported - {}", legacy.bore_server_host),
        server_host: legacy.bore_server_host.clone(),
        server_port: legacy.bore_server_port.unwrap_or(LEGACY_DEFAULT_SERVER_PORT),
        local_host: legacy.local_host,
        local_port: legacy.local_port,
        remote_port: legacy.remote_port,
        autostart: false,
        auto_reconnect: true,
    };

    if let Some(secret) = store.get_secret(LEGACY_KEYRING_SERVICE, LEGACY_KEYRING_USER)? {
        if !secret.is_empty() {
            store.set_secret(KEYRING_SERVICE, &profile_secret_user(id), &secret)?;
        }
    }
    Ok(Some(profile))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> std::path::PathBuf {
        tempfile::tempdir().unwrap().keep()
    }

    fn sample_profile() -> Profile {
        Profile {
            id: Uuid::new_v4(),
            name: "Minecraft".to_string(),
            server_host: "bore.example.com".to_string(),
            server_port: 7835,
            local_host: "127.0.0.1".to_string(),
            local_port: 25565,
            remote_port: 0,
            autostart: true,
            auto_reconnect: false,
        }
    }

    #[test]
    fn roundtrip_save_load_preserves_everything() {
        let dir = temp_dir();
        let path = dir.join(CONFIG_FILE);
        let cfg = AppConfig {
            profiles: vec![sample_profile()],
            active_profile_id: Some(sample_profile().id),
            ui: UiPrefs {
                theme: Theme::Dark,
                start_minimized: true,
                close_to_tray: false,
            },
        };
        cfg.save_to(&path).unwrap();

        let raw = fs::read_to_string(&path).unwrap();
        // camelCase + pretty-printed storage format.
        assert!(raw.contains("\"serverHost\""));
        assert!(raw.contains("\"autoReconnect\""));
        assert!(raw.contains("\"activeProfileId\""));
        assert!(raw.contains("  \"profiles\": ["));

        let loaded = AppConfig::load_from(&path).unwrap();
        assert_eq!(loaded.profiles, cfg.profiles);
        assert_eq!(loaded.active_profile_id, cfg.active_profile_id);
        assert_eq!(loaded.ui, cfg.ui);
    }

    #[test]
    fn missing_file_and_empty_json_load_defaults() {
        let dir = temp_dir();
        let missing = AppConfig::load_from(&dir.join("nope.json")).unwrap();
        assert_eq!(missing, AppConfig::default());
        assert!(missing.profiles.is_empty());
        assert_eq!(missing.active_profile_id, None);
        assert_eq!(missing.ui.theme, Theme::System);
        assert!(missing.ui.close_to_tray);

        fs::write(dir.join(CONFIG_FILE), "{}").unwrap();
        let empty = AppConfig::load_from(&dir.join(CONFIG_FILE)).unwrap();
        assert_eq!(empty, AppConfig::default());
    }

    #[test]
    fn partial_profile_json_takes_field_defaults() {
        let dir = temp_dir();
        let path = dir.join(CONFIG_FILE);
        fs::write(
            &path,
            r#"{"profiles": [{"id": "6f2a2c3e-1111-4000-8000-000000000000"}]}"#,
        )
        .unwrap();
        let cfg = AppConfig::load_from(&path).unwrap();
        let p = &cfg.profiles[0];
        assert_eq!(p.id.to_string(), "6f2a2c3e-1111-4000-8000-000000000000");
        assert_eq!(p.name, "");
        assert_eq!(p.server_host, "");
        assert_eq!(p.server_port, 7835);
        assert_eq!(p.local_host, "127.0.0.1");
        assert_eq!(p.local_port, 25565);
        assert_eq!(p.remote_port, 0);
        assert!(!p.autostart);
        assert!(p.auto_reconnect);
    }

    #[test]
    fn missing_profile_id_gets_a_fresh_uuid() {
        let dir = temp_dir();
        let path = dir.join(CONFIG_FILE);
        fs::write(
            &path,
            r#"{"profiles": [{"name": "a"}, {"name": "b"}]}"#,
        )
        .unwrap();
        let cfg = AppConfig::load_from(&path).unwrap();
        assert_ne!(cfg.profiles[0].id, cfg.profiles[1].id);
        assert_ne!(cfg.profiles[0].id, Uuid::nil());
    }

    #[test]
    fn profile_secret_user_is_profile_prefix() {
        let id = Uuid::new_v4();
        assert_eq!(profile_secret_user(id), format!("profile:{id}"));
    }

    #[test]
    fn memory_store_roundtrip_and_delete_absent() {
        let store = MemorySecretStore::new();
        assert_eq!(store.get_secret("s", "u").unwrap(), None);
        store.set_secret("s", "u", "hunter2").unwrap();
        assert_eq!(store.get_secret("s", "u").unwrap(), Some("hunter2".into()));
        store.delete_secret("s", "u").unwrap();
        assert_eq!(store.get_secret("s", "u").unwrap(), None);
        // Deleting an absent entry is fine.
        store.delete_secret("s", "u").unwrap();
    }

    fn write_legacy(dir: &Path, json: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join(CONFIG_FILE), json).unwrap();
    }

    #[test]
    fn import_legacy_without_dir_or_host_is_none() {
        let store = MemorySecretStore::new();
        // No directory at all.
        let dir = temp_dir().join("bore-minecraft-tunnel");
        assert_eq!(import_legacy(&dir, &store).unwrap(), None);
        // Directory exists but no config.json.
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(import_legacy(&dir, &store).unwrap(), None);
        // Config exists but the host is empty / whitespace.
        write_legacy(&dir, r#"{"bore_server_host": ""}"#);
        assert_eq!(import_legacy(&dir, &store).unwrap(), None);
        write_legacy(&dir, r#"{"bore_server_host": "   "}"#);
        assert_eq!(import_legacy(&dir, &store).unwrap(), None);
    }

    #[test]
    fn import_legacy_with_secret_copies_it_to_the_profile_entry() {
        let dir = temp_dir().join("legacy");
        write_legacy(
            &dir,
            r#"{
                "bore_server_host": "bore.example.com",
                "bore_server_port": 9000,
                "local_port": 25566,
                "profile_name": "old"
            }"#,
        );
        let store = MemorySecretStore::new();
        store.set_secret(LEGACY_KEYRING_SERVICE, LEGACY_KEYRING_USER, "s3cret").unwrap();

        let profile = import_legacy(&dir, &store).unwrap().expect("profile");
        assert_eq!(profile.name, "Imported - bore.example.com");
        assert_eq!(profile.server_host, "bore.example.com");
        assert_eq!(profile.server_port, 9000);
        assert_eq!(profile.local_host, "127.0.0.1");
        assert_eq!(profile.local_port, 25566);
        assert_eq!(profile.remote_port, 0);
        assert!(!profile.autostart);
        assert!(profile.auto_reconnect);
        // Secret copied under the app service, keyed by profile id.
        assert_eq!(
            store
                .get_secret(KEYRING_SERVICE, &profile_secret_user(profile.id))
                .unwrap(),
            Some("s3cret".to_string())
        );
        // The legacy entry itself is untouched (no destructive migration).
        assert_eq!(
            store
                .get_secret(LEGACY_KEYRING_SERVICE, LEGACY_KEYRING_USER)
                .unwrap(),
            Some("s3cret".to_string())
        );
    }

    #[test]
    fn import_legacy_without_secret_imports_profile_anyway() {
        let dir = temp_dir().join("legacy");
        write_legacy(&dir, r#"{"bore_server_host": "spore.internal"}"#);
        let store = MemorySecretStore::new();

        let profile = import_legacy(&dir, &store).unwrap().expect("profile");
        assert_eq!(profile.name, "Imported - spore.internal");
        // Port falls back to the bore default when omitted.
        assert_eq!(profile.server_port, 7835);
        assert_eq!(profile.local_port, 25565);
        assert_eq!(
            store
                .get_secret(KEYRING_SERVICE, &profile_secret_user(profile.id))
                .unwrap(),
            None
        );
    }
}
