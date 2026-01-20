use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use ratatui::style::Color;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use crate::logger::log;

pub const DEFAULT_API_BASE_URL: &str = "https://risu-api.laiosys.dev";
pub const APP_VERSION: &str = concat!("v", env!("CARGO_PKG_VERSION"));

pub fn get_api_base_url() -> String {
    std::env::var("RISU_API_URL").unwrap_or_else(|_| DEFAULT_API_BASE_URL.to_string())
}

pub fn get_user_id_from_token(token: &str) -> anyhow::Result<String> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(anyhow::anyhow!("Invalid token format"));
    }

    let payload = parts[1];
    let decoded = URL_SAFE_NO_PAD.decode(payload)?;
    let claims: serde_json::Value = serde_json::from_slice(&decoded)?;

    let sub = claims["sub"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No sub in token"))?;
    Ok(sub.to_string())
}

pub fn get_user_email_from_token(token: &str) -> anyhow::Result<String> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(anyhow::anyhow!("Invalid token format"));
    }

    let payload = parts[1];
    let decoded = URL_SAFE_NO_PAD.decode(payload)?;
    let claims: serde_json::Value = serde_json::from_slice(&decoded)?;

    let email = claims["email"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No email in token"))?;
    Ok(email.to_string())
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum TokenSource {
    File,
    LegacyFile,
    #[default]
    None,
}

impl std::fmt::Display for TokenSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TokenSource::File => write!(f, "File (JSON)"),
            TokenSource::LegacyFile => write!(f, "File (Legacy)"),
            TokenSource::None => write!(f, "None"),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct TokenData {
    pub id_token: String,
    pub refresh_token: String,
    #[serde(skip)]
    pub source: TokenSource,
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct AppConfig {
    #[serde(default)]
    pub general: GeneralConfig,
    #[serde(default)]
    pub theme: ThemeConfig,
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct GeneralConfig {
    #[serde(default)]
    pub offline_mode: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ThemeConfig {
    pub background: Color,
    pub foreground: Color,
    pub border_active: Color,
    pub border_inactive: Color,
    pub selection_bg: Color,
    pub selection_fg: Color,
    pub search_border: Color,
    pub logo: Color,
    pub header: Color,
    pub sync_synced: Color,
    pub sync_syncing: Color,
    pub sync_error: Color,
    pub sync_payment_required: Color,
    pub sync_offline: Color,
    pub mode_normal: Color,
    pub mode_insert: Color,
    pub editor_cursor_line: Color,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            background: Color::Reset,
            foreground: Color::Rgb(248, 248, 242),
            border_active: Color::Rgb(255, 121, 198),
            border_inactive: Color::Rgb(98, 114, 164),
            selection_bg: Color::Rgb(68, 71, 90),
            selection_fg: Color::Rgb(255, 121, 198),
            search_border: Color::Rgb(139, 233, 253),
            logo: Color::Rgb(189, 147, 249),
            header: Color::Rgb(255, 121, 198),
            sync_synced: Color::Rgb(80, 250, 123),
            sync_syncing: Color::Rgb(255, 184, 108),
            sync_error: Color::Rgb(255, 85, 85),
            sync_payment_required: Color::Magenta,
            sync_offline: Color::Rgb(139, 233, 253),
            mode_normal: Color::Rgb(189, 147, 249),
            mode_insert: Color::Rgb(80, 250, 123),
            editor_cursor_line: Color::DarkGray,
        }
    }
}

pub fn get_config_dir() -> PathBuf {
    let mut path = dirs::home_dir().expect("Could not find home directory");
    path.push(".risu");
    path
}

pub fn load_config() -> AppConfig {
    let mut path = get_config_dir();
    fs::create_dir_all(&path).ok();
    path.push("config.toml");

    if !path.exists() {
        let default_config = AppConfig::default();
        if let Ok(toml_str) = toml::to_string_pretty(&default_config) {
            let mut options = OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                options.mode(0o600);
            }
            if let Ok(mut file) = options.open(&path) {
                let _ = file.write_all(toml_str.as_bytes());
            }
        }
        return default_config;
    }

    match fs::read_to_string(&path) {
        Ok(content) => match toml::from_str(&content) {
            Ok(config) => config,
            Err(e) => {
                eprintln!("Failed to parse config.toml: {}.", e);
                let backup_path = path.with_extension("toml.bak");
                if let Err(backup_err) = fs::rename(&path, &backup_path) {
                    eprintln!("Failed to backup corrupted config: {}", backup_err);
                } else {
                    eprintln!("Corrupted config backed up to {:?}", backup_path);
                }
                eprintln!("Using default configuration.");
                AppConfig::default()
            }
        },
        Err(e) => {
            eprintln!("Failed to read config file: {}. Using default.", e);
            AppConfig::default()
        }
    }
}

pub fn get_token_data() -> TokenData {
    log("get_token_data: Start");

    let config_dir = get_config_dir();
    let mut path = config_dir.clone();
    path.push("token.json");

    if let Ok(content) = fs::read_to_string(&path) {
        if let Ok(mut data) = serde_json::from_str::<TokenData>(&content) {
            log("get_token_data: Loaded from token.json");
            data.source = TokenSource::File;
            return data;
        }
    }

    // Try legacy "token" file (Migration)
    let mut legacy_path = config_dir;
    legacy_path.push("token");

    if let Ok(content) = fs::read_to_string(&legacy_path) {
        log("get_token_data: Loaded from legacy token file");
        if let Ok(mut data) = serde_json::from_str::<TokenData>(&content) {
            data.source = TokenSource::LegacyFile;
            return data;
        }
        return TokenData {
            id_token: content.trim().to_string(),
            refresh_token: String::new(),
            source: TokenSource::LegacyFile,
        };
    }

    log("get_token_data: No token found in any storage");
    TokenData::default()
}

pub fn get_token() -> String {
    get_token_data().id_token
}

pub fn save_token_data(id_token: &str, refresh_token: &str) -> anyhow::Result<()> {
    log("save_token_data: Start");
    let data = TokenData {
        id_token: id_token.to_string(),
        refresh_token: refresh_token.to_string(),
        source: TokenSource::File,
    };
    let json = serde_json::to_string(&data)?;

    save_token_to_file(&json)?;
    Ok(())
}

fn save_token_to_file(json: &str) -> anyhow::Result<()> {
    let config_dir = get_config_dir();
    fs::create_dir_all(&config_dir)?;

    let mut token_path = config_dir;
    token_path.push("token.json");

    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);

    #[cfg(unix)]
    {
        options.mode(0o600);
    }

    match options.open(&token_path) {
        Ok(mut file) => {
            file.write_all(json.as_bytes())?;
            log("save_token_data: Saved to token.json");
            Ok(())
        }
        Err(e) => {
            let msg = format!("save_token_data: Failed to save to token.json: {}", e);
            log(&msg);
            Err(e.into())
        }
    }
}

pub fn delete_token_data() -> anyhow::Result<()> {
    log("delete_token_data: Start");
    let config_dir = get_config_dir();

    let mut path = config_dir.clone();
    path.push("token.json");
    if path.exists() {
        fs::remove_file(path)?;
        log("delete_token_data: token.json deleted");
    }

    let mut legacy_path = config_dir;
    legacy_path.push("token");
    if legacy_path.exists() {
        fs::remove_file(legacy_path)?;
        log("delete_token_data: legacy token file deleted");
    }

    Ok(())
}

// --- E2E Passphrase Management ---

pub fn save_passphrase(passphrase: &str) -> anyhow::Result<()> {
    let config_dir = get_config_dir();
    fs::create_dir_all(&config_dir)?;

    let mut path = config_dir;
    path.push("passphrase");

    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);

    #[cfg(unix)]
    {
        options.mode(0o600);
    }

    let mut file = options.open(&path)?;
    file.write_all(passphrase.as_bytes())?;
    Ok(())
}

pub fn get_passphrase() -> anyhow::Result<Option<String>> {
    let mut path = get_config_dir();
    path.push("passphrase");

    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(&path)?;
    Ok(Some(content.trim().to_string()))
}

pub fn delete_passphrase() -> anyhow::Result<()> {
    let mut path = get_config_dir();
    path.push("passphrase");

    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

pub fn set_offline_mode(offline: bool) -> anyhow::Result<()> {
    let mut config = load_config();
    config.general.offline_mode = offline;
    let path = get_config_dir().join("config.toml");
    let toml_str = toml::to_string_pretty(&config)?;

    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        options.mode(0o600);
    }
    let mut file = options.open(&path)?;
    file.write_all(toml_str.as_bytes())?;
    Ok(())
}
