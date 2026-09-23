//! Estado compartido de la app: config + cancion actual (equivalente a providers/song-info de Pear).

use crate::config::Config;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Mutex, RwLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SongInfo {
    pub video_id: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    /// Segundos.
    pub duration: f64,
    pub thumbnail: Option<String>,
}

impl SongInfo {
    pub fn url(&self) -> String {
        format!("https://music.youtube.com/watch?v={}", self.video_id)
    }
}

#[derive(Debug)]
pub struct Playback {
    pub song: Option<SongInfo>,
    pub paused: bool,
    /// Posicion reportada por la pagina en `updated_at`.
    pub elapsed: f64,
    pub updated_at: Instant,
    /// Unix timestamp en que empezo la cancion (para scrobbling).
    pub started_unix: u64,
    /// Segundos realmente escuchados (sin contar pausas ni saltos).
    pub listened: f64,
    pub scrobbled: bool,
}

impl Default for Playback {
    fn default() -> Self {
        Self {
            song: None,
            paused: true,
            elapsed: 0.0,
            updated_at: Instant::now(),
            started_unix: 0,
            listened: 0.0,
            scrobbled: false,
        }
    }
}

impl Playback {
    /// Posicion estimada ahora mismo.
    pub fn position(&self) -> f64 {
        if self.paused {
            self.elapsed
        } else {
            self.elapsed + self.updated_at.elapsed().as_secs_f64()
        }
    }
}

pub struct AppState {
    pub config_dir: PathBuf,
    pub config: RwLock<Config>,
    pub playback: Mutex<Playback>,
    pub http: reqwest::Client,
}

impl AppState {
    pub fn config(&self) -> Config {
        self.config.read().unwrap().clone()
    }

    pub fn current_song(&self) -> Option<SongInfo> {
        self.playback.lock().unwrap().song.clone()
    }
}

pub fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}
