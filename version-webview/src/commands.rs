//! Comandos que la pagina de YouTube Music puede llamar (y solo estos:
//! ver capabilities/youtube-music.json).

use crate::config::{self, Config};
use crate::integrations::{self, downloader, lyrics, scrobbler, sponsorblock};
use crate::shortcuts;
use crate::state::{AppState, SongInfo};
use tauri::{AppHandle, Runtime, State};

#[tauri::command]
pub fn yir_log(level: String, msg: String) {
    match level.as_str() {
        "error" => log::error!("[pagina] {msg}"),
        "warn" => log::warn!("[pagina] {msg}"),
        _ => log::info!("[pagina] {msg}"),
    }
}

#[tauri::command]
pub fn yir_get_config(state: State<'_, AppState>) -> Config {
    state.config()
}

#[tauri::command]
pub fn yir_set_config<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
    config: Config,
) -> Result<(), String> {
    // La sesion de Last.fm no se puede cambiar desde la pagina, solo con yir_lastfm_connect.
    let mut config = config;
    config.lastfm.session_key = state.config().lastfm.session_key;
    config::save(&state.config_dir, &config).map_err(|e| e.to_string())?;
    let old = std::mem::replace(&mut *state.config.write().unwrap(), config.clone());

    if old.shortcuts.enabled != config.shortcuts.enabled
        || old.shortcuts.play_pause != config.shortcuts.play_pause
        || old.shortcuts.next != config.shortcuts.next
        || old.shortcuts.previous != config.shortcuts.previous
        || old.shortcuts.show_hide != config.shortcuts.show_hide
    {
        shortcuts::apply(&app, &config.shortcuts);
    }
    if config.api_server.enabled && !old.api_server.enabled {
        integrations::api_server::spawn(app.clone(), config.api_server.port);
    }
    Ok(())
}

#[tauri::command]
pub fn yir_song_changed<R: Runtime>(app: AppHandle<R>, song: SongInfo) {
    integrations::on_song_changed(&app, song);
}

#[tauri::command]
pub fn yir_play_state<R: Runtime>(app: AppHandle<R>, paused: bool, elapsed: f64) {
    integrations::on_play_state(&app, paused, elapsed);
}

#[tauri::command]
pub async fn yir_fetch_lyrics(
    state: State<'_, AppState>,
    title: String,
    artist: String,
    album: Option<String>,
    duration: f64,
) -> Result<Option<lyrics::Lyrics>, String> {
    let cfg = state.config().lyrics;
    lyrics::fetch(&state.http, &title, &artist, album.as_deref(), duration, cfg.show_inexact).await
}

#[tauri::command]
pub async fn yir_sponsor_segments(
    state: State<'_, AppState>,
    video_id: String,
) -> Result<Vec<[f64; 2]>, String> {
    let cfg = state.config().sponsorblock;
    Ok(sponsorblock::fetch(&state.http, &cfg.api_url, &cfg.categories, &video_id).await)
}

#[tauri::command]
pub async fn yir_lastfm_connect<R: Runtime>(app: AppHandle<R>) -> Result<String, String> {
    scrobbler::connect_lastfm(app).await
}

#[tauri::command]
pub fn yir_download_current<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    downloader::download_current(&app)
}
