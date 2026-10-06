//! Configuracion persistente: %APPDATA%\YoutubeInRustWeb\config.json

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    pub restore_session: bool,
    pub autoplay_radio: bool,
    pub notifications: bool,
    /// 0 = alta (AAC 128 kbps), 1 = ahorro (AAC 48 kbps)
    pub quality: u8,
    /// Codigo ISO del pais para las listas (vacio = segun Windows).
    pub country: String,
    pub volume: f32,
    pub exponential_volume: bool,
    pub skip_silence: bool,
    pub sponsorblock: bool,
    pub synced_lyrics: bool,
    pub eq_enabled: bool,
    pub eq: [f32; 10],
    pub discord: bool,
    pub discord_hide_paused: bool,
    pub lastfm: LastFm,
    pub listenbrainz: bool,
    pub listenbrainz_token: String,
    pub api_server: bool,
    pub api_port: u16,
    pub shortcuts: bool,
    pub sc_play: String,
    pub sc_next: String,
    pub sc_prev: String,
    pub sc_show: String,
    pub browser: u8,
    pub logged_in: bool,
    /// Nombre con el que te ven en un Jam.
    pub jam_name: String,
    /// Ultimo Jam (nombre de la sala) que creaste o al que te uniste.
    pub jam_room: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            restore_session: true,
            autoplay_radio: true,
            notifications: false,
            quality: 0,
            country: String::new(),
            volume: 80.0,
            exponential_volume: false,
            skip_silence: false,
            sponsorblock: true,
            synced_lyrics: true,
            eq_enabled: false,
            eq: [0.0; 10],
            discord: false,
            discord_hide_paused: false,
            lastfm: LastFm::default(),
            listenbrainz: false,
            listenbrainz_token: String::new(),
            api_server: false,
            api_port: 26538,
            shortcuts: true,
            sc_play: "Control+Shift+Space".into(),
            sc_next: "Control+Shift+ArrowRight".into(),
            sc_prev: "Control+Shift+ArrowLeft".into(),
            sc_show: "Control+Shift+KeyY".into(),
            browser: 0,
            logged_in: false,
            jam_name: std::env::var("USERNAME").unwrap_or_default(),
            jam_room: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LastFm {
    pub enabled: bool,
    pub api_key: String,
    pub secret: String,
    pub session_key: Option<String>,
    pub user: Option<String>,
}

impl Default for LastFm {
    fn default() -> Self {
        // Mismas credenciales publicas que usa Pear Desktop (plugins/scrobbler).
        Self {
            enabled: false,
            api_key: "04d76faaac8726e60988e14c105d421a".into(),
            secret: "a5d2a36fdf64819290f6982481eaffa2".into(),
            session_key: None,
            user: None,
        }
    }
}

pub fn data_dir() -> PathBuf {
    // Carpeta alternativa para pruebas (no toca la configuracion real del usuario).
    if let Some(d) = std::env::var_os("YIR_DATA_DIR") {
        return PathBuf::from(d);
    }
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    base.join("YoutubeInRustWeb")
}

pub fn load(dir: &Path) -> Config {
    std::fs::read_to_string(dir.join("config.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(dir: &Path, cfg: &Config) {
    let _ = std::fs::create_dir_all(dir);
    let tmp = dir.join("config.json.tmp");
    if let Ok(bytes) = serde_json::to_vec_pretty(cfg) {
        if std::fs::write(&tmp, bytes).is_ok() {
            let _ = std::fs::rename(tmp, dir.join("config.json"));
        }
    }
}
