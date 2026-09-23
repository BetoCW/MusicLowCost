const COMMANDS: &[&str] = &[
    "yir_log",
    "yir_get_config",
    "yir_set_config",
    "yir_song_changed",
    "yir_play_state",
    "yir_fetch_lyrics",
    "yir_sponsor_segments",
    "yir_lastfm_connect",
    "yir_download_current",
];

fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("tauri-build fallo");
}
