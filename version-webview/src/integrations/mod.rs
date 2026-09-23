//! Reparte los eventos del reproductor (cambio de cancion, play/pausa) a todas las
//! integraciones: panel de Windows, Discord, scrobblers, notificaciones y bandeja.

pub mod api_server;
pub mod discord;
pub mod downloader;
pub mod lyrics;
pub mod scrobbler;
pub mod sponsorblock;

use crate::state::{AppState, Playback, SongInfo, unix_now};
use crate::{media_controls, tray, util};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, Runtime};

/// Avanza el reloj interno: suma lo escuchado y actualiza la posicion estimada.
fn tick(pb: &mut Playback) {
    if !pb.paused {
        let dt = pb.updated_at.elapsed().as_secs_f64();
        pb.listened += dt.min(30.0);
        pb.elapsed += dt;
    }
    pb.updated_at = Instant::now();
}

/// Devuelve la cancion a scrobblear si ya cumple la regla y no se ha enviado.
fn take_scrobble(pb: &mut Playback) -> Option<(SongInfo, u64)> {
    let song = pb.song.as_ref()?;
    if !pb.scrobbled && scrobbler::should_scrobble(song.duration, pb.listened) {
        pb.scrobbled = true;
        return Some((song.clone(), pb.started_unix));
    }
    None
}

fn spawn_scrobble<R: Runtime>(app: &AppHandle<R>, song: SongInfo, started: u64) {
    let state = app.state::<AppState>();
    let cfg = state.config();
    if !cfg.lastfm.enabled && !cfg.listenbrainz.enabled {
        return;
    }
    let http = state.http.clone();
    tauri::async_runtime::spawn(scrobbler::scrobble(http, cfg, song, started));
}

fn update_discord<R: Runtime>(app: &AppHandle<R>, song: &SongInfo, paused: bool, position: f64) {
    let cfg = app.state::<AppState>().config().discord;
    let d = app.state::<discord::Discord>();
    if cfg.enabled {
        d.send(discord::Msg::Update {
            song: song.clone(),
            paused,
            position,
            hide_when_paused: cfg.hide_when_paused,
            button: cfg.show_button,
        });
    } else {
        d.send(discord::Msg::Clear);
    }
}

pub fn on_song_changed<R: Runtime>(app: &AppHandle<R>, song: SongInfo) {
    let state = app.state::<AppState>();
    let previous = {
        let mut pb = state.playback.lock().unwrap();
        if pb.song.as_ref().is_some_and(|s| s.video_id == song.video_id) {
            // Misma cancion (la pagina a veces avisa dos veces): solo refrescar datos.
            pb.song = Some(song.clone());
            None
        } else {
            tick(&mut pb);
            let prev = take_scrobble(&mut pb);
            *pb = Playback {
                song: Some(song.clone()),
                paused: false,
                started_unix: unix_now(),
                ..Default::default()
            };
            Some(prev)
        }
    };
    let Some(prev) = previous else {
        media_controls::update_song(app, &song);
        return;
    };
    if let Some((s, t)) = prev {
        spawn_scrobble(app, s, t);
    }
    log::info!("sonando: {} - {} ({:.0}s)", song.artist, song.title, song.duration);

    media_controls::update_song(app, &song);
    media_controls::update_playback(app, false, 0.0);
    update_discord(app, &song, false, 0.0);
    tray::set_now_playing(app, Some(&song), false);

    let cfg = state.config();
    if cfg.notifications {
        util::notify(app, &song.title, &song.artist);
    }
    if cfg.lastfm.enabled || cfg.listenbrainz.enabled {
        tauri::async_runtime::spawn(scrobbler::now_playing(state.http.clone(), cfg, song));
    }
}

pub fn on_play_state<R: Runtime>(app: &AppHandle<R>, paused: bool, elapsed: f64) {
    let state = app.state::<AppState>();
    let song = {
        let mut pb = state.playback.lock().unwrap();
        tick(&mut pb);
        pb.paused = paused;
        pb.elapsed = elapsed;
        pb.song.clone()
    };
    media_controls::update_playback(app, paused, elapsed);
    if let Some(song) = song {
        update_discord(app, &song, paused, elapsed);
        tray::set_now_playing(app, Some(&song), paused);
    }
}

/// Revisa cada 10 s si la cancion actual ya se puede scrobblear.
pub fn spawn_ticker<R: Runtime>(app: AppHandle<R>) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        loop {
            interval.tick().await;
            let pending = {
                let state = app.state::<AppState>();
                let mut pb = state.playback.lock().unwrap();
                tick(&mut pb);
                take_scrobble(&mut pb)
            };
            if let Some((s, t)) = pending {
                spawn_scrobble(&app, s, t);
            }
        }
    });
}
