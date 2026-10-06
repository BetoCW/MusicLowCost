//! Pegamento entre el Jam (crate::jam) y el reproductor.
//! - Anfitrion: cada cancion, pausa, seek y cambio de cola se reparte a los invitados, y
//!   lo que piden los invitados entra como si lo hubiera pedido el usuario.
//! - Invitado: no usa su cola; toca lo que manda el anfitrion y corrige la posicion.

use super::{Backend, Cmd, load_lyrics, spawn};
use crate::desktop;
use crate::jam::{self, Event, Guest, Host, PlayState, Request};
use crate::model::Track;
use crate::stream::Growing;
use crate::ui::ListKind;
use slint::{ModelRc, SharedString, VecModel};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// Desfase maximo con el anfitrion antes de corregir la posicion.
const TOLERANCE: f64 = 0.12;
/// Entre correcciones (salvo que llegue un estado nuevo del anfitrion).
const FIX_COOLDOWN: Duration = Duration::from_secs(2);

pub(super) enum Jam {
    Off,
    Host { id: u32, host: Host },
    Guest(Box<GuestJam>),
}

pub(super) struct GuestJam {
    id: u32,
    guest: Guest,
    tag: u32,
    track: Option<Track>,
    data: Option<Arc<[u8]>>,
    state: Option<PlayState>,
    last_fix: Instant,
}

impl Jam {
    fn id(&self) -> Option<u32> {
        match self {
            Jam::Off => None,
            Jam::Host { id, .. } => Some(*id),
            Jam::Guest(g) => Some(g.id),
        }
    }
}

fn strings(v: Vec<String>) -> ModelRc<SharedString> {
    ModelRc::from(Rc::new(VecModel::from(v.into_iter().map(SharedString::from).collect::<Vec<_>>())))
}

impl Backend {
    pub(super) fn is_jam_guest(&self) -> bool {
        matches!(*self.jam.borrow(), Jam::Guest(_))
    }

    fn is_jam_host(&self) -> bool {
        matches!(*self.jam.borrow(), Jam::Host { .. })
    }

    /// En un Jam no se salta el silencio inicial: moveria la posicion respecto a los demas.
    pub(super) fn sync_fx(&self) {
        let skip = self.state.borrow().cfg.skip_silence && matches!(*self.jam.borrow(), Jam::Off);
        self.engine.fx.skip_silence.store(skip, Ordering::Relaxed);
    }

    fn jam_status(&self, msg: impl Into<String>) {
        let msg: String = msg.into();
        self.ui.state(move |s| s.set_jam_status(msg.into()));
    }

    // ---------- anfitrion ----------

    fn with_host(&self, f: impl FnOnce(&Host)) {
        if let Jam::Host { host, .. } = &*self.jam.borrow() {
            f(host);
        }
    }

    /// Nueva cancion en el anfitrion. `data` = None si no se puede repartir.
    fn jam_host_track(&self, track: &Track, data: Option<Arc<[u8]>>) {
        self.state.borrow_mut().jam_ready = true;
        self.with_host(|h| h.set_track(track, data));
    }

    /// A los invitados se les manda la cancion completa: si todavia se esta bajando, en
    /// cuanto termine (mientras tanto siguen en pausa, ver `jam_host_hold`).
    pub(super) fn jam_share_when_ready(self: &Rc<Self>, track: &Track, data: &Arc<Growing>, seq: u64) {
        if !self.is_jam_host() {
            return;
        }
        if let Some(full) = data.full() {
            return self.jam_host_track(track, Some(full));
        }
        let (b, track, data) = (self.clone(), track.clone(), data.clone());
        spawn(async move {
            let ok = data.wait_done().await.is_ok();
            if b.state.borrow().play_seq == seq {
                b.jam_host_track(&track, if ok { data.full() } else { None });
                b.jam_host_state();
            }
        });
    }

    pub(super) fn jam_host_state(&self) {
        if !self.is_jam_host() || !self.state.borrow().jam_ready {
            return;
        }
        let playing = self.engine.has_track() && !self.engine.is_paused() && !self.state.borrow().buffering;
        let pos = self.engine.position();
        self.with_host(|h| h.set_state(playing, pos));
    }

    /// Antes de cargar otra cancion: que los invitados no sigan con la anterior.
    pub(super) fn jam_host_hold(&self) {
        let pos = self.engine.position();
        self.with_host(|h| h.set_state(false, pos));
    }

    pub(super) fn jam_host_queue(&self) {
        if !self.is_jam_host() {
            return;
        }
        let upcoming: Vec<Track> = {
            let st = self.state.borrow();
            let start = st.index.map(|i| i + 1).unwrap_or(0);
            st.queue.iter().skip(start).take(30).cloned().collect()
        };
        self.with_host(|h| h.set_queue(upcoming));
    }

    pub(super) fn jam_start_host(self: &Rc<Self>, name: String, room: String, password: String) {
        self.jam_leave(None);
        let Some((name, room)) = self.jam_check_fields(&name, "Anfitrión", &room, &password) else { return };
        let id = jam::next_id();
        let host = Host::start(&room, &password, &name, self.tx.clone(), id);
        *self.jam.borrow_mut() = Jam::Host { id, host };
        self.sync_fx();
        // Lo que ya esta sonando se comparte de inmediato.
        let (cur, data) = {
            let st = self.state.borrow();
            (st.current.clone(), st.current_data.clone())
        };
        if let (Some(t), Some(data)) = (cur.filter(|_| self.engine.has_track()), data) {
            let seq = self.state.borrow().play_seq;
            self.jam_share_when_ready(&t, &data, seq);
            self.jam_host_state();
        }
        self.jam_host_queue();
        log::info!("jam: abriendo la sala «{room}»");
        self.ui.state(move |s| {
            s.set_jam_role("host".into());
            s.set_jam_peers(strings(vec![]));
            s.set_jam_queue(strings(vec![]));
            s.set_jam_status("Abriendo el Jam en internet…".into());
        });
    }

    /// Valida nombre, sala y contraseña y los recuerda. Devuelve (nombre, sala) limpios.
    fn jam_check_fields(&self, name: &str, fallback: &str, room: &str, password: &str) -> Option<(String, String)> {
        let name = jam::clean_name(name, fallback);
        let room = jam::clean_name(room, "");
        if room.is_empty() || password.chars().count() < 4 {
            self.jam_status("Escribe el nombre del Jam y una contraseña de al menos 4 caracteres.");
            return None;
        }
        {
            let mut st = self.state.borrow_mut();
            st.cfg.jam_name = name.clone();
            st.cfg.jam_room = room.clone();
        }
        crate::config::save(&self.dir, &self.cfg());
        Some((name, room))
    }

    /// Lo que pide un invitado: en el orden de la cola, despues de lo que ya pidieron otros.
    fn jam_add(self: &Rc<Self>, who: &str, track: Track) {
        let title = track.title.clone();
        let (at, play_now) = self.insert_next(track, true);
        self.push_queue();
        self.clear_prefetch();
        if play_now {
            self.play_index(at, true);
        } else {
            self.prefetch_next();
        }
        self.ui.status(format!("{who} agregó «{title}» a la cola"));
    }

    // ---------- invitado ----------

    pub(super) fn jam_join(self: &Rc<Self>, name: String, room: String, password: String) {
        self.jam_leave(None);
        let Some((name, room)) = self.jam_check_fields(&name, "Invitado", &room, &password) else { return };
        {
            let mut st = self.state.borrow_mut();
            // Se cancela lo que se estuviera cargando y se guarda donde ibas.
            st.play_seq += 1;
            st.buffering = false;
            if self.engine.has_track() {
                st.restore_position = Some(self.engine.position());
                st.resuming = true;
            }
        }
        self.engine.stop();
        self.on_play_state();
        let id = jam::next_id();
        let guest = Guest::start(room, password, name, self.tx.clone(), id);
        *self.jam.borrow_mut() = Jam::Guest(Box::new(GuestJam {
            id,
            guest,
            tag: 0,
            track: None,
            data: None,
            state: None,
            last_fix: Instant::now(),
        }));
        self.sync_fx();
        self.ui.state(|s| {
            s.set_jam_role("guest".into());
            s.set_jam_peers(strings(vec![]));
            s.set_jam_queue(strings(vec![]));
        });
    }

    pub(super) fn jam_leave(self: &Rc<Self>, reason: Option<String>) {
        let old = std::mem::replace(&mut *self.jam.borrow_mut(), Jam::Off);
        match old {
            Jam::Off => {}
            Jam::Host { .. } => {}
            Jam::Guest(_) => {
                // Se suelta lo del anfitrion; tu cola queda como estaba.
                self.engine.stop();
                {
                    let mut st = self.state.borrow_mut();
                    st.play_seq += 1;
                    st.current = None;
                    st.buffering = false;
                }
                self.ui.mark_current(None);
                self.ui.state(|s| {
                    s.set_has_song(false);
                    s.set_buffering(false);
                    s.set_now_cover(Default::default());
                    s.set_position(0.0);
                    s.set_position_text("0:00".into());
                });
                self.on_play_state();
            }
        }
        drop(old);
        self.sync_fx();
        let msg = reason.unwrap_or_default();
        self.ui.state(move |s| {
            s.set_jam_role("off".into());
            s.set_jam_status(msg.into());
            s.set_jam_sync("".into());
            s.set_jam_peers(strings(vec![]));
            s.set_jam_queue(strings(vec![]));
        });
    }

    /// Si se es invitado, lo que el usuario pide se le manda al anfitrion.
    /// Devuelve true si el comando ya se atendio.
    pub(super) fn jam_intercept(self: &Rc<Self>, cmd: &Cmd) -> bool {
        let playing = match &*self.jam.borrow() {
            Jam::Guest(g) => g.state.is_some_and(|s| s.playing),
            _ => return false,
        };
        let req = match cmd {
            Cmd::TogglePlay => Some(Request::TogglePlay),
            Cmd::Play => (!playing).then_some(Request::TogglePlay),
            Cmd::Pause => playing.then_some(Request::TogglePlay),
            Cmd::Next => Some(Request::Next),
            Cmd::Previous => Some(Request::Previous),
            Cmd::Seek(secs) => Some(Request::Seek { secs: *secs }),
            Cmd::PlayTrack(list, i) | Cmd::Enqueue(list, i) => {
                let t = ListKind::parse(list).and_then(|k| self.state.borrow().lists.get(&k).and_then(|l| l.get(*i).cloned()));
                match t {
                    Some(t) if jam::is_youtube_id(&t.id) => {
                        self.ui.status(format!("Enviada al Jam: «{}»", t.title));
                        Some(Request::Add { track: t })
                    }
                    Some(_) => {
                        self.ui.status("En el Jam solo se comparten canciones de YouTube Music.");
                        None
                    }
                    None => None,
                }
            }
            Cmd::PlayDetail(_) => {
                self.ui.status("En el Jam se agregan canciones de una en una (toca la que quieras).");
                None
            }
            Cmd::ToggleShuffle | Cmd::CycleRepeat => {
                self.ui.status("En el Jam, el aleatorio y la repetición los controla el anfitrión.");
                None
            }
            _ => return false,
        };
        if let (Some(r), Jam::Guest(g)) = (req, &*self.jam.borrow()) {
            g.guest.request(r);
        }
        true
    }

    pub(super) fn jam_event(self: &Rc<Self>, id: u32, ev: Event) {
        if self.jam.borrow().id() != Some(id) {
            return; // de un Jam anterior
        }
        match ev {
            Event::Status(s) => self.jam_status(s),
            Event::Peers(names) => self.ui.state(move |s| s.set_jam_peers(strings(names))),
            Event::Request(who, r) => {
                if !self.is_jam_host() {
                    return;
                }
                log::info!("jam: {who} pide {r:?}");
                match r {
                    Request::TogglePlay => self.handle(Cmd::TogglePlay),
                    Request::Next => self.handle(Cmd::Next),
                    Request::Previous => self.handle(Cmd::Previous),
                    Request::Seek { secs } => self.handle(Cmd::Seek(secs.max(0.0))),
                    Request::Add { track } => self.jam_add(&who, track),
                }
            }
            Event::Joined(host) => self.jam_status(format!("Conectado al Jam de {host}. Lo que toques se agrega a su cola.")),
            Event::Track { tag, track, shared } => self.guest_track(tag, track, shared),
            Event::Audio { tag, data } => self.guest_audio(tag, data),
            Event::State(s) => {
                if let Jam::Guest(g) = &mut *self.jam.borrow_mut() {
                    if g.tag != s.tag {
                        return;
                    }
                    g.state = Some(s);
                }
                self.jam_guest_sync(true);
            }
            Event::Queue(tracks) => {
                let lines = tracks.into_iter().map(|t| format!("{} — {}", t.title, t.artists)).collect();
                self.ui.state(move |s| s.set_jam_queue(strings(lines)));
            }
            Event::Ended(reason) => {
                log::info!("jam: fin: {reason}");
                self.jam_leave(Some(reason));
            }
        }
    }

    fn guest_track(self: &Rc<Self>, tag: u32, track: Track, shared: bool) {
        if let Jam::Guest(g) = &mut *self.jam.borrow_mut() {
            g.tag = tag;
            g.track = Some(track.clone());
            g.data = None;
            g.state = None;
        }
        let seq = {
            let mut st = self.state.borrow_mut();
            st.play_seq += 1;
            st.buffering = shared;
            st.play_seq
        };
        self.engine.stop();
        self.start_track_ui(&track);
        if !shared {
            self.ui.state(|s| s.set_buffering(false));
            self.jam_status("Esta canción del anfitrión no se puede compartir en el Jam.");
        }
        spawn(load_lyrics(self.clone(), track, seq));
    }

    fn guest_audio(self: &Rc<Self>, tag: u32, data: Arc<[u8]>) {
        let track = match &mut *self.jam.borrow_mut() {
            Jam::Guest(g) if g.tag == tag => {
                g.data = Some(data.clone());
                g.track.clone()
            }
            _ => return,
        };
        if let Err(e) = self.engine.load(&Growing::complete(data), true) {
            return self.ui.status(e);
        }
        {
            let mut st = self.state.borrow_mut();
            st.buffering = false;
            st.last_tick = Instant::now();
        }
        self.ui.state(|s| s.set_buffering(false));
        if let Some(t) = track {
            self.ui.run(move |_| desktop::smtc_metadata(&t));
        }
        self.jam_guest_sync(true);
    }

    /// Lleva el reproductor a donde va el anfitrion. `force`: hay un estado nuevo.
    pub(super) fn jam_guest_sync(self: &Rc<Self>, force: bool) {
        let (data, state, duration, host_now, rtt, last_fix) = {
            let j = self.jam.borrow();
            let Jam::Guest(g) = &*j else { return };
            let (Some(data), Some(state)) = (g.data.clone(), g.state) else { return };
            let duration = g.track.as_ref().map(|t| t.duration as f64).unwrap_or(0.0);
            let clock = g.guest.clock.borrow();
            (data, state, duration, clock.host_now(), clock.rtt_ms(), g.last_fix)
        };
        let expected = state.expected(host_now);
        if !self.engine.has_track() || self.engine.finished() {
            // Termino aqui pero el anfitrion la sigue (repetir una, o volvio atras).
            let restart = state.playing && if duration > 0.0 { expected + 1.0 < duration } else { expected < 2.0 };
            if !restart || self.engine.load(&Growing::complete(data), true).is_err() {
                return;
            }
        }
        let paused = self.engine.is_paused();
        let drift = self.engine.position() - expected;
        if force && state.playing && !paused {
            let text = match rtt {
                Some(rtt) => format!("Latencia con el anfitrión: {rtt:.0} ms · desfase {:+.0} ms", drift * 1000.0),
                None => "Sincronizando el reloj…".to_string(),
            };
            self.ui.state(move |s| s.set_jam_sync(text.into()));
        }
        let mut fixed = false;
        if state.playing {
            if paused {
                self.engine.play();
                self.engine.seek(expected);
                fixed = true;
            } else if drift.abs() > TOLERANCE && (force || last_fix.elapsed() >= FIX_COOLDOWN) {
                log::debug!("jam: corrigiendo {:.0} ms", drift * 1000.0);
                self.engine.seek(expected);
                fixed = true;
            }
        } else {
            if !paused {
                self.engine.pause();
            }
            if drift.abs() > 0.25 {
                self.engine.seek(expected);
            }
        }
        if fixed {
            if let Jam::Guest(g) = &mut *self.jam.borrow_mut() {
                g.last_fix = Instant::now();
            }
            self.state.borrow_mut().lyrics_idx = -2;
        }
        if paused == state.playing {
            self.on_play_state();
        }
    }
}
