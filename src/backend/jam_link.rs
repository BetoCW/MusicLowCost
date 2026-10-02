//! Pegamento entre el Jam (crate::jam) y el reproductor.
//! - Anfitrion: cada cancion, pausa, seek y cambio de cola se reparte a los invitados, y
//!   lo que piden los invitados entra como si lo hubiera pedido el usuario.
//! - Invitado: no usa su cola; toca lo que manda el anfitrion y corrige la posicion.

use super::{Backend, Cmd, load_lyrics, spawn};
use crate::desktop;
use crate::jam::{self, Event, Guest, Host, PlayState, Request};
use crate::model::Track;
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

    /// Nueva cancion en el anfitrion. `data` = None si no se puede repartir (Spotify).
    pub(super) fn jam_host_track(&self, track: &Track, data: Option<Arc<[u8]>>) {
        self.with_host(|h| h.set_track(track, data));
    }

    pub(super) fn jam_host_state(&self) {
        if !self.is_jam_host() {
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

    pub(super) fn jam_start_host(self: &Rc<Self>, name: String, port: u16) {
        self.jam_leave(None);
        let name = jam::clean_name(&name, "Anfitrión");
        {
            let mut st = self.state.borrow_mut();
            st.cfg.jam_name = name.clone();
            st.cfg.jam_port = port;
        }
        crate::config::save(&self.dir, &self.cfg());
        let id = jam::next_id();
        let host = match Host::start(port, &name, jam::new_code(), self.tx.clone(), id) {
            Ok(h) => h,
            Err(e) => return self.jam_status(e),
        };
        let (code, port) = (host.code().to_string(), host.port());
        *self.jam.borrow_mut() = Jam::Host { id, host };
        self.sync_fx();
        // Lo que ya esta sonando se comparte de inmediato.
        let (cur, data) = {
            let st = self.state.borrow();
            (st.current.clone(), st.current_data.clone())
        };
        if let Some(t) = cur.filter(|_| self.engine.has_track()) {
            self.jam_host_track(&t, data);
            self.jam_host_state();
        }
        self.jam_host_queue();
        let address = match jam::local_ip() {
            Some(ip) => format!("{ip}:{port}"),
            None => format!("(tu IP):{port}"),
        };
        log::info!("jam: creado en {address}");
        self.ui.state(move |s| {
            s.set_jam_role("host".into());
            s.set_jam_code(code.into());
            s.set_jam_address(address.into());
            s.set_jam_peers(strings(vec![]));
            s.set_jam_queue(strings(vec![]));
            s.set_jam_status("Jam creado. Comparte la dirección y el código con quien quieras invitar.".into());
        });
    }

    /// Lo que pide un invitado: en el orden de la cola, despues de lo que ya pidieron otros.
    fn jam_add(self: &Rc<Self>, who: &str, track: Track) {
        let title = track.title.clone();
        let (at, play_now) = {
            let mut st = self.state.borrow_mut();
            let len = st.queue.len();
            let mut at = st.index.map(|i| i + 1).unwrap_or(len).min(len);
            while at < st.queue.len() && st.jam_added.contains(&st.queue[at].id) {
                at += 1;
            }
            st.jam_added.insert(track.id.clone());
            st.queue.insert(at, track);
            (at, !self.engine.has_track() && !st.buffering)
        };
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

    pub(super) fn jam_join(self: &Rc<Self>, name: String, addr: String, code: String) {
        self.jam_leave(None);
        let name = jam::clean_name(&name, "Invitado");
        if addr.trim().is_empty() || code.trim().is_empty() {
            return self.jam_status("Escribe la dirección y el código que te pasó el anfitrión.");
        }
        {
            let mut st = self.state.borrow_mut();
            st.cfg.jam_name = name.clone();
            st.cfg.jam_last_address = addr.trim().to_string();
            // Se cancela lo que se estuviera cargando y se guarda donde ibas.
            st.play_seq += 1;
            st.buffering = false;
            if self.engine.has_track() {
                st.restore_position = Some(self.engine.position());
                st.resuming = true;
            }
        }
        crate::config::save(&self.dir, &self.cfg());
        self.engine.stop();
        self.spotify.stop();
        self.on_play_state();
        let id = jam::next_id();
        let guest = Guest::start(addr, code, name, self.tx.clone(), id);
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
            Jam::Host { .. } => {
                self.state.borrow_mut().jam_added.clear();
            }
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
            self.jam_status("El anfitrión está escuchando algo de Spotify: eso no se comparte en el Jam.");
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
        if let Err(e) = self.engine.load(data, true) {
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
            if !restart || self.engine.load(data, true).is_err() {
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
