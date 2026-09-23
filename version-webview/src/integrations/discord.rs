//! Discord Rich Presence ("Escuchando ..."), port de plugins/discord de Pear.
//! Corre en su propio hilo porque la conexion IPC con Discord es bloqueante.

use crate::state::SongInfo;
use discord_rich_presence::{DiscordIpc, DiscordIpcClient, activity};
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

/// Mismo client id que usa Pear Desktop (plugins/discord/constants.ts).
const CLIENT_ID: &str = "1177081335727267940";

pub enum Msg {
    Update { song: SongInfo, paused: bool, position: f64, hide_when_paused: bool, button: bool },
    Clear,
}

pub struct Discord(pub Mutex<Sender<Msg>>);

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
        // Nos quedamos solo con el ultimo mensaje.
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
                // Discord no esta abierto; reintentamos despues.
                continue;
            }
        }

        let c = client.as_mut().unwrap();
        let ok = match msg {
            Msg::Clear => c.clear_activity().is_ok(),
            Msg::Update { paused: true, hide_when_paused: true, .. } => c.clear_activity().is_ok(),
            Msg::Update { song, paused, position, button, .. } => {
                let url = song.url();
                let url = button.then_some(url.as_str());
                c.set_activity(build(song, *paused, *position, url)).is_ok()
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

fn build<'a>(
    song: &'a SongInfo,
    paused: bool,
    position: f64,
    url: Option<&'a str>,
) -> activity::Activity<'a> {
    let mut assets = activity::Assets::new();
    if let Some(t) = song.thumbnail.as_deref() {
        assets = assets.large_image(t);
    }
    assets = assets.large_text(if paused { "⏸ En pausa" } else { song.album.as_deref().unwrap_or("YouTube Music") });

    let mut act = activity::Activity::new()
        .activity_type(activity::ActivityType::Listening)
        .details(&song.title)
        .state(&song.artist)
        .assets(assets);

    if !paused && song.duration > 0.0 {
        let now = crate::state::unix_now() as i64;
        let start = now - position as i64;
        act = act.timestamps(
            activity::Timestamps::new().start(start).end(start + song.duration as i64),
        );
    }
    if let Some(url) = url {
        act = act.buttons(vec![activity::Button::new("Escuchar en YouTube Music", url)]);
    }
    act
}
