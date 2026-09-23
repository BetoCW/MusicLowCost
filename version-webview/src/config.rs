//! Configuracion persistente (equivalente al `config.json` de Pear).
//! Se guarda en %APPDATA%\dev.portafolio.youtubeinrustweb\config.json

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    pub general: General,
    pub adblock: bool,
    pub performance: bool,
    pub notifications: bool,
    pub discord: Discord,
    pub lastfm: LastFm,
    pub listenbrainz: ListenBrainz,
    pub lyrics: Lyrics,
    pub sponsorblock: SponsorBlock,
    pub precise_volume: PreciseVolume,
    pub exponential_volume: bool,
    pub skip_silences: SkipSilences,
    pub disable_autoplay: bool,
    pub skip_disliked: bool,
    pub playback_speed: f64,
    pub equalizer: Equalizer,
    pub compressor: bool,
    pub blur_nav_bar: bool,
    pub hide_video: bool,
    pub shortcuts: Shortcuts,
    pub api_server: ApiServer,
    pub downloader: Downloader,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            general: General::default(),
            adblock: true,
            performance: true,
            notifications: false,
            discord: Discord::default(),
            lastfm: LastFm::default(),
            listenbrainz: ListenBrainz::default(),
            lyrics: Lyrics::default(),
            sponsorblock: SponsorBlock::default(),
            precise_volume: PreciseVolume::default(),
            exponential_volume: false,
            skip_silences: SkipSilences::default(),
            disable_autoplay: false,
            skip_disliked: false,
            playback_speed: 1.0,
            equalizer: Equalizer::default(),
            compressor: false,
            blur_nav_bar: false,
            hide_video: false,
            shortcuts: Shortcuts::default(),
            api_server: ApiServer::default(),
            downloader: Downloader::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct General {
    /// Al cerrar la ventana se va a la bandeja en vez de salir.
    pub close_to_tray: bool,
    pub start_minimized: bool,
    /// Pide a WebView2 que libere memoria cuando la ventana esta oculta/minimizada.
    pub low_memory_when_hidden: bool,
    /// Flags extra de Chromium para gastar menos RAM (requiere reiniciar la app).
    pub low_memory_flags: bool,
}

impl Default for General {
    fn default() -> Self {
        Self {
            close_to_tray: true,
            start_minimized: false,
            low_memory_when_hidden: true,
            low_memory_flags: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Discord {
    pub enabled: bool,
    pub hide_when_paused: bool,
    pub show_button: bool,
}

impl Default for Discord {
    fn default() -> Self {
        Self { enabled: false, hide_when_paused: false, show_button: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LastFm {
    pub enabled: bool,
    pub api_key: String,
    pub secret: String,
    pub session_key: Option<String>,
}

impl Default for LastFm {
    fn default() -> Self {
        // Mismas credenciales publicas que usa Pear Desktop (plugins/scrobbler).
        Self {
            enabled: false,
            api_key: "04d76faaac8726e60988e14c105d421a".into(),
            secret: "a5d2a36fdf64819290f6982481eaffa2".into(),
            session_key: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ListenBrainz {
    pub enabled: bool,
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Lyrics {
    pub enabled: bool,
    /// Si no hay letra exacta, buscar solo por titulo.
    pub show_inexact: bool,
}

impl Default for Lyrics {
    fn default() -> Self {
        Self { enabled: true, show_inexact: false }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SponsorBlock {
    pub enabled: bool,
    pub api_url: String,
    pub categories: Vec<String>,
}

impl Default for SponsorBlock {
    fn default() -> Self {
        Self {
            enabled: false,
            api_url: "https://sponsor.ajay.app".into(),
            categories: ["sponsor", "intro", "outro", "interaction", "selfpromo", "music_offtopic"]
                .into_iter()
                .map(String::from)
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PreciseVolume {
    pub enabled: bool,
    /// Porcentaje por cada paso de la rueda del mouse.
    pub step: u8,
}

impl Default for PreciseVolume {
    fn default() -> Self {
        Self { enabled: true, step: 2 }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SkipSilences {
    pub enabled: bool,
    pub only_beginning: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Equalizer {
    pub enabled: bool,
    /// Ganancia en dB para 60, 170, 310, 600, 1k, 3k, 6k, 12k, 14k, 16k Hz.
    pub gains: [f32; 10],
}

impl Default for Equalizer {
    fn default() -> Self {
        Self { enabled: false, gains: [0.0; 10] }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Shortcuts {
    pub enabled: bool,
    pub play_pause: String,
    pub next: String,
    pub previous: String,
    pub show_hide: String,
}

impl Default for Shortcuts {
    fn default() -> Self {
        Self {
            enabled: true,
            play_pause: "CommandOrControl+Shift+Space".into(),
            next: "CommandOrControl+Shift+Right".into(),
            previous: "CommandOrControl+Shift+Left".into(),
            show_hide: "CommandOrControl+Shift+Y".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ApiServer {
    pub enabled: bool,
    pub port: u16,
}

impl Default for ApiServer {
    fn default() -> Self {
        Self { enabled: false, port: 26538 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Downloader {
    /// Carpeta destino; vacio = carpeta Musica del usuario.
    pub folder: String,
    /// mp3, m4a, opus, flac...
    pub format: String,
}

impl Default for Downloader {
    fn default() -> Self {
        Self { folder: String::new(), format: "mp3".into() }
    }
}

pub fn config_path(dir: &PathBuf) -> PathBuf {
    dir.join("config.json")
}

pub fn load(dir: &PathBuf) -> Config {
    std::fs::read_to_string(config_path(dir))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(dir: &PathBuf, cfg: &Config) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join("config.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(cfg)?)?;
    std::fs::rename(tmp, config_path(dir))
}
