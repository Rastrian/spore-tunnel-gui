use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

const APP_DIR: &str = "bore-minecraft-tunnel";
const CONFIG_FILE: &str = "config.json";
const KEYRING_SERVICE: &str = "bore-minecraft-tunnel";
const KEYRING_USERNAME: &str = "bore-secret";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub bore_server_host: String,
    #[serde(default)]
    pub bore_server_port: Option<u16>,
    #[serde(default = "default_local_host")]
    pub local_host: String,
    #[serde(default = "default_local_port")]
    pub local_port: u16,
    #[serde(default = "default_remote_port")]
    pub remote_port: u16,
    #[serde(default)]
    pub profile_name: Option<String>,
}

fn default_local_host() -> String {
    "127.0.0.1".to_string()
}
fn default_local_port() -> u16 {
    25565
}
fn default_remote_port() -> u16 {
    0
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            bore_server_host: String::new(),
            bore_server_port: None,
            local_host: default_local_host(),
            local_port: default_local_port(),
            remote_port: default_remote_port(),
            profile_name: None,
        }
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
    let path = config_path()?;
    if !path.exists() {
        return Ok(AppConfig::default());
    }
    let data = fs::read_to_string(&path).map_err(|e| format!("Failed to read config: {e}"))?;
    serde_json::from_str(&data).map_err(|e| format!("Failed to parse config: {e}"))
}

pub fn save_config(config: &AppConfig) -> Result<(), String> {
    let dir = config_dir()?;
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create config dir: {e}"))?;
    let data = serde_json::to_string_pretty(config).map_err(|e| format!("Failed to serialize config: {e}"))?;
    fs::write(config_path()?, data).map_err(|e| format!("Failed to write config: {e}"))
}

pub fn save_secret(secret: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USERNAME)
        .map_err(|e| format!("Failed to create keyring entry: {e}"))?;
    entry.set_password(secret).map_err(|e| format!("Failed to save secret: {e}"))
}

#[allow(dead_code)]
pub fn load_secret() -> Result<String, String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USERNAME)
        .map_err(|e| format!("Failed to create keyring entry: {e}"))?;
    entry.get_password().map_err(|e| format!("No secret stored: {e}"))
}

pub fn has_secret() -> bool {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USERNAME);
    match entry {
        Ok(e) => e.get_password().is_ok(),
        Err(_) => false,
    }
}

#[allow(dead_code)]
pub fn delete_secret() -> Result<(), String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USERNAME)
        .map_err(|e| format!("Failed to create keyring entry: {e}"))?;
    entry.delete_credential().map_err(|e| format!("Failed to delete secret: {e}"))
}
