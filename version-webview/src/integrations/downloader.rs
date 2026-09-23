//! Descargar la cancion actual (port simplificado de plugins/downloader).
//! Pear usa youtubei.js + ffmpeg empaquetado; aqui se delega en yt-dlp y ffmpeg
//! instalados en el sistema para no cargar nada extra en memoria.

use crate::state::AppState;
use crate::util;
use tauri::{AppHandle, Manager, Runtime};

fn music_dir() -> String {
    std::env::var("USERPROFILE")
        .map(|h| format!("{h}\\Music"))
        .unwrap_or_else(|_| ".".into())
}

pub fn download_current<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let state = app.state::<AppState>();
    let song = state.current_song().ok_or("No hay ninguna cancion sonando")?;
    let cfg = state.config().downloader;
    let folder = if cfg.folder.trim().is_empty() { music_dir() } else { cfg.folder.clone() };
    let format = if cfg.format.trim().is_empty() { "mp3".to_string() } else { cfg.format.clone() };

    let mut cmd = std::process::Command::new("yt-dlp");
    cmd.args([
        "-x",
        "--audio-format",
        &format,
        "--embed-metadata",
        "--embed-thumbnail",
        "--no-playlist",
        "-P",
        &folder,
        "-o",
        "%(artist,uploader)s - %(title)s.%(ext)s",
        &song.url(),
    ]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(util::CREATE_NO_WINDOW);
    }

    let mut child = cmd.spawn().map_err(|_| {
        "No se encontro yt-dlp. Instalalo con: winget install yt-dlp.yt-dlp (incluye ffmpeg)".to_string()
    })?;

    util::notify(app, "Descargando", &format!("{} - {}", song.artist, song.title));
    let app = app.clone();
    std::thread::spawn(move || {
        let ok = child.wait().map(|s| s.success()).unwrap_or(false);
        let msg = if ok {
            format!("Guardada en {folder}")
        } else {
            "yt-dlp fallo (¿falta ffmpeg?)".to_string()
        };
        util::notify(&app, &format!("{} - {}", song.artist, song.title), &msg);
    });
    Ok(())
}
