//! Discord Rich Presence ("Escuchando ..."), port de plugins/discord de Pear.
//! Corre en su propio hilo porque la conexion IPC con Discord es bloqueante.

use crate::model::Track;
use discord_rich_presence::{DiscordIpc, DiscordIpcClient, activity};
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

/// Mismo client id que usa Pear Desktop (plugins/discord/constants.ts).
const CLIENT_ID: &str = "1177081335727267940";

pub enum Msg {
    Update { track: Track, paused: bool, position: f64, hide_when_paused: bool },
    Clear,
}

pub struct Discord(Mutex<Sender<Msg>>);

impl Discord {
    pub fn spawn() -> Self {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("discord-rpc".into())
            .stack_size(256 * 1024)
            .spawn(move || run(rx))
            .expect("hilo discord");
        Self(Mutex::new(tx))
    }

    pub fn send(&self, msg: Msg) {
        let _ = self.0.lock().unwrap().send(msg);
    }
}

fn run(rx: Receiver<Msg>) {
    let mut client: Option<DiscordIpcClient> = None;
    let mut pending: Option<Msg> = None;
    let mut last_try = Instant::now() - Duration::from_secs(60);

    loop {
        match rx.recv_timeout(Duration::from_secs(15)) {
            Ok(msg) => pending = Some(msg),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        while let Ok(msg) = rx.try_recv() {
            pending = Some(msg);
        }
        let Some(msg) = pending.as_ref() else { continue };

        if client.is_none() {
            if matches!(msg, Msg::Clear) {
                pending = None;
                continue;
            }
            if last_try.elapsed() < Duration::from_secs(10) {
                continue;
            }
            last_try = Instant::now();
            let mut c = DiscordIpcClient::new(CLIENT_ID);
            if c.connect().is_ok() {
                client = Some(c);
            } else {
                continue; // Discord cerrado; se reintenta luego.
            }
        }

        let c = client.as_mut().unwrap();
        let ok = match msg {
            Msg::Clear => c.clear_activity().is_ok(),
            Msg::Update { paused: true, hide_when_paused: true, .. } => c.clear_activity().is_ok(),
            Msg::Update { track, paused, position, .. } => {
                let url = track.url();
                c.set_activity(build(track, *paused, *position, &url)).is_ok()
            }
        };
        if ok {
            pending = None;
        } else {
            let _ = c.close();
            client = None;
        }
    }
}

fn build<'a>(t: &'a Track, paused: bool, position: f64, url: &'a str) -> activity::Activity<'a> {
    let mut assets = activity::Assets::new()
        .large_text(if paused { "⏸ En pausa" } else { t.album.as_deref().unwrap_or("YouTube Music") });
    if let Some(thumb) = t.thumb.as_deref() {
        assets = assets.large_image(thumb);
    }
    let mut act = activity::Activity::new()
        .activity_type(activity::ActivityType::Listening)
        .details(&t.title)
        .state(&t.artists)
        .assets(assets)
        .buttons(vec![activity::Button::new("Escuchar en YouTube Music", url)]);
    if !paused && t.duration > 0 {
        let start = crate::util::unix_now() as i64 - position as i64;
        act = act.timestamps(activity::Timestamps::new().start(start).end(start + t.duration as i64));
    }
    act
}
