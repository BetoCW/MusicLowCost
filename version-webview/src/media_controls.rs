//! Panel multimedia de Windows (SMTC) y teclas multimedia del teclado.
//! Equivalente a los plugins taskbar-mediacontrol + la integracion MPRIS/SMTC de Pear.

use crate::player;
use crate::state::SongInfo;
use souvlaki::{
    MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig,
    SeekDirection,
};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Manager, Runtime, WebviewWindow};

#[derive(Default)]
pub struct MediaState(pub Mutex<Option<MediaControls>>);

pub fn init<R: Runtime>(app: &AppHandle<R>, window: &WebviewWindow<R>) {
    #[cfg(windows)]
    let hwnd = window.hwnd().ok().map(|h| h.0 as *mut std::ffi::c_void);
    #[cfg(not(windows))]
    let hwnd = {
        let _ = window;
        None
    };

    let config = PlatformConfig {
        dbus_name: "youtube_in_rust_web",
        display_name: "YoutubeInRustWeb",
        hwnd,
    };
    let mut controls = match MediaControls::new(config) {
        Ok(c) => c,
        Err(e) => {
            log::warn!("controles multimedia no disponibles: {e:?}");
            return;
        }
    };
    let handle = app.clone();
    let attached = controls.attach(move |event| match event {
        MediaControlEvent::Play => player::simple(&handle, "play"),
        MediaControlEvent::Pause => player::simple(&handle, "pause"),
        MediaControlEvent::Toggle => player::simple(&handle, "toggle"),
        MediaControlEvent::Next => player::simple(&handle, "next"),
        MediaControlEvent::Previous => player::simple(&handle, "previous"),
        MediaControlEvent::Stop => player::simple(&handle, "pause"),
        MediaControlEvent::Seek(dir) | MediaControlEvent::SeekBy(dir, _) => {
            let secs = if matches!(dir, SeekDirection::Forward) { 10 } else { -10 };
            player::command(&handle, "seekBy", secs.into());
        }
        MediaControlEvent::SetPosition(MediaPosition(pos)) => {
            player::command(&handle, "seekTo", pos.as_secs_f64().into());
        }
        MediaControlEvent::Raise => player::show_window(&handle),
        MediaControlEvent::Quit => handle.exit(0),
        _ => {}
    });
    if let Err(e) = attached {
        log::warn!("no se pudieron enlazar los controles multimedia: {e:?}");
        return;
    }
    *app.state::<MediaState>().0.lock().unwrap() = Some(controls);
}

pub fn update_song<R: Runtime>(app: &AppHandle<R>, song: &SongInfo) {
    let state = app.state::<MediaState>();
    let mut guard = state.0.lock().unwrap();
    if let Some(c) = guard.as_mut() {
        let _ = c.set_metadata(MediaMetadata {
            title: Some(&song.title),
            artist: Some(&song.artist),
            album: song.album.as_deref(),
            cover_url: song.thumbnail.as_deref(),
            duration: (song.duration > 0.0).then(|| Duration::from_secs_f64(song.duration)),
        });
    }
}

pub fn update_playback<R: Runtime>(app: &AppHandle<R>, paused: bool, position: f64) {
    let state = app.state::<MediaState>();
    let mut guard = state.0.lock().unwrap();
    if let Some(c) = guard.as_mut() {
        let progress = Some(MediaPosition(Duration::from_secs_f64(position.max(0.0))));
        let playback = if paused {
            MediaPlayback::Paused { progress }
        } else {
            MediaPlayback::Playing { progress }
        };
        let _ = c.set_playback(playback);
    }
}
