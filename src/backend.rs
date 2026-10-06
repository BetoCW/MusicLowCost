//! Logica de la app: cola, reproduccion, radio, busqueda, letras, cuenta e integraciones.
//! Corre en un solo hilo con un runtime tokio "current_thread" (menos memoria que multi-hilo).

use crate::audio::{Engine, volume_curve};
use crate::config::{self, Config};
use crate::images::Images;
use crate::integrations::{api_server, discord, lyrics, scrobbler, sponsorblock};
use crate::model::{self, Card, CardKind, Track, fmt_time};
use crate::stream::{self, Growing};
use crate::ui::{CardListKind, ListKind, Ui};
use crate::{AppState, Cfg, desktop};
use rustypipe::client::RustyPipe;
use rustypipe::param::Country;
use serde::{Deserialize, Serialize};
use slint::{ComponentHandle, Model};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

mod jam_link;
use jam_link::Jam;

#[derive(Debug)]
pub enum Cmd {
    Navigate(String),
    /// Flechas de atras / adelante.
    Back,
    Forward,
    /// Buscar ya (Enter).
    Search(String),
    /// Buscar mientras se escribe (espera a que se deje de escribir).
    SearchTyped(String),
    PlayTrack(String, usize),
    Enqueue(String, usize),
    OpenCard(String, String),
    OpenArtistOfCurrent,
    PlayDetail(bool),
    TogglePlay,
    Play,
    Pause,
    Next,
    Previous,
    Seek(f64),
    SetVolume(f32),
    VolumeWheel(f32),
    ToggleShuffle,
    CycleRepeat,
    QueueRemove(usize),
    /// Arrastrar una cancion de la cola: de, a.
    QueueMove(usize, usize),
    QueueClear,
    SaveSettings(UiSettings),
    /// Como SaveSettings pero sin escribir config.json (ecualizador mientras se arrastra).
    PreviewSettings(UiSettings),
    Login(String),
    Logout,
    LastfmConnect,
    SaveSession,
    /// Boton UpDate: instala la version nueva (o revisa si hay una).
    UpdateClick,
    /// Crear un Jam: tu nombre, nombre de la sala, contraseña.
    JamHost(String, String, String),
    /// Unirse a un Jam: tu nombre, nombre de la sala, contraseña.
    JamJoin(String, String, String),
    JamLeave,
    /// Aviso del Jam con ese id.
    Jam(u32, crate::jam::Event),
}

/// Una pantalla del historial (para las flechas de atras / adelante).
#[derive(Debug, Clone, PartialEq)]
enum Nav {
    Page(String),
    Detail(CardKind, String),
}

/// Se guarda en session.json para retomar la cola al abrir.
#[derive(Serialize, Deserialize, Default)]
struct Session {
    queue: Vec<Track>,
    index: Option<usize>,
    position: f64,
}

struct State {
    cfg: Config,
    lists: HashMap<ListKind, Vec<Track>>,
    cards: HashMap<CardListKind, Vec<Card>>,
    detail_tracks_id: Option<(CardKind, String)>,
    /// Cola que esta sonando.
    queue: Vec<Track>,
    index: Option<usize>,
    /// Cambia cada vez que se reemplaza la cola (para descartar radios que llegan tarde).
    queue_gen: u64,
    shuffle: bool,
    repeat: u8,
    current: Option<Track>,
    /// Audio de la cancion actual (para repetir sin volver a descargar).
    current_data: Option<Arc<Growing>>,
    play_seq: u64,
    buffering: bool,
    restore_position: Option<f64>,
    /// true solo al retomar la cancion guardada de la sesion anterior.
    resuming: bool,
    // scrobbling
    started_unix: u64,
    listened: f64,
    last_tick: Instant,
    scrobbled: bool,
    // extras
    lyrics: Vec<(u64, String)>,
    lyrics_idx: i32,
    segments: Vec<[f64; 2]>,
    prefetch: Option<(String, Arc<Growing>)>,
    prefetching: Option<String>,
    radio_loading: bool,
    last_paused: bool,
    errors_in_row: u32,
    last_snapshot: Instant,
    /// Canciones que se agregaron a la cola a mano (o invitados del Jam) y todavia no
    /// suenan: las nuevas van despues de estas, en orden de llegada.
    added: std::collections::HashSet<String>,
    /// Seek pedido mientras el audio todavia se descargaba.
    pending_seek: Option<f64>,
    /// Cuando empezo la ultima cancion (para no correr yt-dlp por cada salto rapido).
    last_play_at: Instant,
    /// El Jam ya tiene la cancion actual (si no, no se le manda el estado).
    jam_ready: bool,
    search_gen: u64,
    /// Version nueva publicada (si hay) y si ya se esta instalando.
    update: Option<crate::update::Release>,
    updating: bool,
    nav: Nav,
    back: Vec<Nav>,
    forward: Vec<Nav>,
}

pub struct Backend {
    rp: RustyPipe,
    http: reqwest::Client,
    ui: Ui,
    engine: Arc<Engine>,
    images: Rc<Images>,
    discord: discord::Discord,
    dir: PathBuf,
    state: RefCell<State>,
    tx: UnboundedSender<Cmd>,
    pub visible: Arc<AtomicBool>,
    api_snapshot: api_server::Snapshot,
    api_enabled: Arc<AtomicBool>,
    api_started: RefCell<bool>,
    jam: RefCell<Jam>,
}

type B = Rc<Backend>;

pub fn run(
    ui: Ui,
    cfg: Config,
    dir: PathBuf,
    engine: Arc<Engine>,
    tx: UnboundedSender<Cmd>,
    mut rx: UnboundedReceiver<Cmd>,
    visible: Arc<AtomicBool>,
) {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async move {
        let http = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) YoutubeInRustWeb/0.2")
            .timeout(Duration::from_secs(20))
            .build()
            .expect("http");
        let rp = RustyPipe::builder()
            .storage_dir(dir.clone())
            .build()
            .expect("rustypipe");
        let b: B = Rc::new(Backend {
            rp,
            images: Images::new(http.clone()),
            http,
            ui,
            engine,
            discord: discord::Discord::spawn(),
            dir,
            tx,
            visible,
            api_snapshot: Arc::new(Mutex::new(serde_json::json!({}))),
            api_enabled: Arc::new(AtomicBool::new(cfg.api_server)),
            api_started: RefCell::new(false),
            jam: RefCell::new(Jam::Off),
            state: RefCell::new(State {
                shuffle: false,
                repeat: 0,
                lists: HashMap::new(),
                cards: HashMap::new(),
                detail_tracks_id: None,
                queue: Vec::new(),
                index: None,
                queue_gen: 0,
                current: None,
                current_data: None,
                play_seq: 0,
                buffering: false,
                restore_position: None,
                resuming: false,
                started_unix: 0,
                listened: 0.0,
                last_tick: Instant::now(),
                scrobbled: false,
                lyrics: Vec::new(),
                lyrics_idx: -1,
                segments: Vec::new(),
                prefetch: None,
                prefetching: None,
                radio_loading: false,
                last_paused: true,
                errors_in_row: 0,
                last_snapshot: Instant::now(),
                added: Default::default(),
                pending_seek: None,
                last_play_at: Instant::now() - Duration::from_secs(10),
                jam_ready: false,
                search_gen: 0,
                update: None,
                updating: false,
                nav: Nav::Page("home".into()),
                back: Vec::new(),
                forward: Vec::new(),
                cfg,
            }),
        });

        b.apply_settings(true);
        b.restore_session();
        spawn(load_home(b.clone()));
        spawn(load_library(b.clone()));
        spawn(ticker(b.clone()));
        spawn(update_loop(b.clone()));
        spawn(remove_login_webview());
        // Preparar yt-dlp en segundo plano (lo descarga la primera vez) y mantenerlo al dia.
        {
            let b2 = b.clone();
            spawn(async move {
                let ui = b2.ui.clone();
                match stream::ensure_tools(&b2.http, move |m| ui.status(m)).await {
                    Ok(_) => {
                        b2.ui.status("");
                        let _ = tokio::task::spawn_blocking(stream::warm_up).await;
                        stream::maybe_self_update(&b2.http).await;
                    }
                    Err(e) => b2.ui.status(format!("No se pudo preparar yt-dlp: {e}")),
                }
            });
        }

        while let Some(cmd) = rx.recv().await {
            b.handle(cmd);
        }
    });
}

fn spawn<F: std::future::Future<Output = ()> + 'static>(f: F) {
    tokio::task::spawn_local(f);
}

impl Backend {
    fn cfg(&self) -> Config {
        self.state.borrow().cfg.clone()
    }

    fn query(&self) -> rustypipe::client::RustyPipeQuery {
        let cfg_country = self.state.borrow().cfg.country.clone();
        let code = if cfg_country.trim().is_empty() {
            crate::util::system_country().unwrap_or_else(|| "US".into())
        } else {
            cfg_country.trim().to_uppercase()
        };
        // Con sesion, todo va autenticado: listas privadas, «Me gusta» (LM) y recomendaciones.
        let q = if self.rp.user_auth_cookie_header().is_some() { self.rp.query().authenticated() } else { self.rp.query() };
        match serde_json::from_value::<Country>(serde_json::Value::String(code)) {
            Ok(c) => q.country(c),
            Err(_) => q,
        }
    }

    fn handle(self: &Rc<Self>, cmd: Cmd) {
        if self.jam_intercept(&cmd) {
            return;
        }
        match cmd {
            Cmd::Navigate(page) => self.go(Nav::Page(page), true),
            Cmd::Back => self.history_step(true),
            Cmd::Forward => self.history_step(false),
            Cmd::Search(q) => self.start_search(q, false),
            Cmd::SearchTyped(q) => self.start_search(q, true),
            Cmd::PlayTrack(list, i) => self.play_from_list(&list, i),
            Cmd::Enqueue(list, i) => {
                let Some(kind) = ListKind::parse(&list) else { return };
                let t = self.state.borrow().lists.get(&kind).and_then(|l| l.get(i).cloned());
                if let Some(t) = t {
                    let title = t.title.clone();
                    let (at, play_now) = self.insert_next(t, false);
                    self.push_queue();
                    self.clear_prefetch();
                    if play_now {
                        self.play_index(at, true);
                    } else {
                        self.prefetch_next();
                        self.ui.status(format!("Sonará después de la actual: {title}"));
                    }
                }
            }
            Cmd::OpenCard(kind, id) => {
                if let Some(k) = CardKind::parse(&kind) {
                    self.go(Nav::Detail(k, id), true);
                }
            }
            Cmd::OpenArtistOfCurrent => {
                let id = self.state.borrow().current.as_ref().and_then(|t| t.artist_id.clone());
                if let Some(id) = id {
                    self.go(Nav::Detail(CardKind::Artist, id), true);
                }
            }
            Cmd::PlayDetail(shuffle) => {
                let mut tracks = self.state.borrow().lists.get(&ListKind::Detail).cloned().unwrap_or_default();
                if tracks.is_empty() {
                    return;
                }
                if shuffle {
                    shuffle_vec(&mut tracks);
                }
                self.set_queue(tracks, 0);
            }
            Cmd::TogglePlay => {
                if !self.engine.has_track() {
                    let idx = self.state.borrow().index;
                    if let Some(i) = idx {
                        self.state.borrow_mut().resuming = true;
                        self.play_index(i, true);
                    }
                } else if self.engine.is_paused() {
                    self.engine.play();
                } else {
                    self.engine.pause();
                }
                self.on_play_state();
            }
            Cmd::Play => {
                if !self.engine.has_track() {
                    self.handle(Cmd::TogglePlay);
                } else {
                    self.engine.play();
                    self.on_play_state();
                }
            }
            Cmd::Pause => {
                self.engine.pause();
                self.on_play_state();
            }
            Cmd::Next => self.next(true),
            Cmd::Previous => {
                if self.engine.position() > 3.0 {
                    self.seek_to(0.0);
                    self.on_play_state();
                } else {
                    let idx = self.state.borrow().index;
                    match idx {
                        Some(i) if i > 0 => self.play_index(i - 1, true),
                        _ => {
                            self.seek_to(0.0);
                            self.on_play_state();
                        }
                    }
                }
            }
            Cmd::Seek(secs) => {
                if self.engine.has_track() {
                    self.seek_to(secs);
                    self.on_play_state();
                } else {
                    self.state.borrow_mut().restore_position = Some(secs);
                }
            }
            Cmd::SetVolume(v) => self.set_volume(v),
            Cmd::VolumeWheel(d) => {
                let v = self.state.borrow().cfg.volume;
                let step = if d > 0.0 { 2.0 } else { -2.0 };
                self.set_volume(v + step);
            }
            Cmd::ToggleShuffle => {
                let on = {
                    let mut st = self.state.borrow_mut();
                    st.shuffle = !st.shuffle;
                    if st.shuffle {
                        // Como YouTube Music: se mezcla lo que falta por sonar.
                        let start = st.index.map(|i| i + 1).unwrap_or(0);
                        if start < st.queue.len() {
                            shuffle_vec(&mut st.queue[start..]);
                        }
                    }
                    st.shuffle
                };
                self.push_queue();
                self.ui.state(move |s| s.set_shuffle(on));
                self.clear_prefetch();
                self.prefetch_next();
            }
            Cmd::CycleRepeat => {
                let r = {
                    let mut st = self.state.borrow_mut();
                    st.repeat = (st.repeat + 1) % 3;
                    st.repeat
                };
                self.ui.state(move |s| s.set_repeat(r as i32));
            }
            Cmd::QueueRemove(i) => {
                {
                    let mut st = self.state.borrow_mut();
                    if i >= st.queue.len() {
                        return;
                    }
                    st.queue.remove(i);
                    if let Some(cur) = st.index {
                        if i < cur {
                            st.index = Some(cur - 1);
                        } else if i == cur {
                            st.index = if st.queue.is_empty() { None } else { Some(cur.min(st.queue.len() - 1)) };
                        }
                    }
                }
                self.push_queue();
                self.clear_prefetch();
                self.prefetch_next();
            }
            Cmd::QueueMove(from, to) => {
                {
                    let mut st = self.state.borrow_mut();
                    let len = st.queue.len();
                    if from >= len || to >= len || from == to {
                        return;
                    }
                    let t = st.queue.remove(from);
                    st.queue.insert(to, t);
                    st.index = st.index.map(|cur| moved_index(cur, from, to));
                }
                self.push_queue();
                self.clear_prefetch();
                self.prefetch_next();
            }
            Cmd::QueueClear => {
                {
                    let mut st = self.state.borrow_mut();
                    // Se conserva la cancion actual.
                    let cur = st.index.and_then(|i| st.queue.get(i).cloned());
                    st.queue = cur.into_iter().collect();
                    st.index = if st.queue.is_empty() { None } else { Some(0) };
                    st.added.clear();
                    // La radio que estuviera llegando ya no se agrega.
                    st.queue_gen += 1;
                }
                self.push_queue();
                self.clear_prefetch();
            }
            Cmd::SaveSettings(s) => {
                s.apply(&mut self.state.borrow_mut().cfg);
                config::save(&self.dir, &self.cfg());
                self.apply_settings(false);
            }
            Cmd::PreviewSettings(s) => {
                s.apply(&mut self.state.borrow_mut().cfg);
                let cfg = self.cfg();
                self.engine.fx.set_eq(cfg.eq_enabled, cfg.eq);
            }
            Cmd::Login(browser) => spawn(login(self.clone(), browser)),
            Cmd::Logout => spawn(logout(self.clone())),
            Cmd::LastfmConnect => spawn(lastfm_connect(self.clone())),
            Cmd::SaveSession => self.save_session(),
            Cmd::UpdateClick => spawn(update_click(self.clone())),
            Cmd::JamHost(name, room, pass) => self.jam_start_host(name, room, pass),
            Cmd::JamJoin(name, room, pass) => self.jam_join(name, room, pass),
            Cmd::JamLeave => self.jam_leave(None),
            Cmd::Jam(id, ev) => self.jam_event(id, ev),
        }
    }

    // ---------- navegacion ----------

    fn go(self: &Rc<Self>, nav: Nav, record: bool) {
        match nav.clone() {
            Nav::Page(page) => {
                if page == "library" && self.cfg().logged_in {
                    spawn(load_library(self.clone()));
                }
                if record {
                    self.record_nav(nav);
                }
                self.ui.state(move |s| {
                    s.set_page(page.into());
                    s.set_status("".into());
                });
            }
            Nav::Detail(kind, id) => spawn(open_card(self.clone(), kind, id, record)),
        }
    }

    /// Nueva pantalla: la anterior queda para la flecha de atras.
    fn record_nav(&self, nav: Nav) {
        {
            let mut st = self.state.borrow_mut();
            if st.nav == nav {
                return;
            }
            let old = std::mem::replace(&mut st.nav, nav);
            st.back.push(old);
            if st.back.len() > 50 {
                st.back.remove(0);
            }
            st.forward.clear();
        }
        self.push_nav_ui();
    }

    fn history_step(self: &Rc<Self>, back: bool) {
        let target = {
            let mut st = self.state.borrow_mut();
            let Some(t) = (if back { st.back.pop() } else { st.forward.pop() }) else { return };
            let cur = std::mem::replace(&mut st.nav, t.clone());
            if back {
                st.forward.push(cur);
            } else {
                st.back.push(cur);
            }
            t
        };
        self.push_nav_ui();
        self.go(target, false);
    }

    fn push_nav_ui(&self) {
        let (b, f) = {
            let st = self.state.borrow();
            (!st.back.is_empty(), !st.forward.is_empty())
        };
        self.ui.state(move |s| {
            s.set_can_back(b);
            s.set_can_forward(f);
        });
    }

    /// Busqueda: al escribir se espera a que se deje de teclear; las respuestas viejas se
    /// descartan (`search_gen`).
    fn start_search(self: &Rc<Self>, q: String, typed: bool) {
        let generation = {
            let mut st = self.state.borrow_mut();
            st.search_gen += 1;
            st.search_gen
        };
        let q = q.trim().to_string();
        if q.chars().count() < if typed { 2 } else { 1 } {
            return;
        }
        let b = self.clone();
        spawn(async move {
            if typed {
                tokio::time::sleep(Duration::from_millis(350)).await;
                if b.state.borrow().search_gen != generation {
                    return;
                }
            }
            search(b, q, generation).await;
        });
    }

    // ---------- ajustes ----------

    fn apply_settings(self: &Rc<Self>, first: bool) {
        let cfg = self.cfg();
        self.engine.fx.set_eq(cfg.eq_enabled, cfg.eq);
        self.sync_fx();
        self.engine.set_volume(volume_curve(cfg.volume, cfg.exponential_volume));
        if !cfg.discord {
            self.discord.send(discord::Msg::Clear);
        } else {
            self.update_discord();
        }
        self.api_enabled.store(cfg.api_server, Ordering::Relaxed);
        if cfg.api_server && !*self.api_started.borrow() {
            *self.api_started.borrow_mut() = true;
            api_server::spawn(cfg.api_port, self.tx.clone(), self.api_snapshot.clone(), self.api_enabled.clone());
        }
        let shortcut_cfg = cfg.clone();
        self.ui.run(move |_| desktop::apply_shortcuts(&shortcut_cfg));
        if first {
            let cfg = cfg.clone();
            self.ui.run(move |ui| push_cfg_to_ui(ui, &cfg));
        }
    }

    fn set_volume(&self, v: f32) {
        let v = v.clamp(0.0, 100.0);
        let exp = {
            let mut st = self.state.borrow_mut();
            st.cfg.volume = v;
            st.cfg.exponential_volume
        };
        self.engine.set_volume(volume_curve(v, exp));
        self.ui.state(move |s| s.set_volume(v));
        config::save(&self.dir, &self.cfg());
    }

    // ---------- cola y reproduccion ----------

    fn play_from_list(self: &Rc<Self>, list: &str, i: usize) {
        let Some(kind) = ListKind::parse(list) else { return };
        if kind == ListKind::Queue {
            self.play_index(i, true);
            return;
        }
        let tracks = self.state.borrow().lists.get(&kind).cloned().unwrap_or_default();
        if kind == ListKind::Detail {
            self.set_queue(tracks, i);
            return;
        }
        let Some(t) = tracks.get(i).cloned() else { return };
        self.set_queue(vec![t.clone()], 0);
        // Como YouTube Music: al tocar una cancion suelta se arma su "radio" (si esta activado).
        if self.cfg().autoplay_radio {
            spawn(extend_with_radio(self.clone(), t, true));
        }
    }

    fn set_queue(self: &Rc<Self>, queue: Vec<Track>, start: usize) {
        {
            let mut st = self.state.borrow_mut();
            st.queue = queue;
            st.shuffle = false;
            st.queue_gen += 1;
            st.added.clear();
        }
        self.ui.state(|s| s.set_shuffle(false));
        self.clear_prefetch();
        self.play_index(start, true);
    }

    fn push_queue(&self) {
        let queue = self.state.borrow().queue.clone();
        let current = self.state.borrow().current.as_ref().map(|t| t.id.clone());
        self.state.borrow_mut().lists.insert(ListKind::Queue, queue.clone());
        self.show_tracks(ListKind::Queue, queue, current);
        self.jam_host_queue();
    }

    /// Agrega una cancion para que suene despues de la actual (y despues de las que ya se
    /// agregaron asi). Devuelve donde quedo y si hay que tocarla ya: si la cola estaba
    /// vacia o, con `play_if_idle` (Jam), si no esta sonando nada.
    pub(super) fn insert_next(&self, track: Track, play_if_idle: bool) -> (usize, bool) {
        let mut st = self.state.borrow_mut();
        let was_empty = st.index.is_none() || st.queue.is_empty();
        let len = st.queue.len();
        let mut at = st.index.map(|i| i + 1).unwrap_or(len).min(len);
        while at < st.queue.len() && st.added.contains(&st.queue[at].id) {
            at += 1;
        }
        st.added.insert(track.id.clone());
        st.queue.insert(at, track);
        let idle = !self.engine.has_track() && !st.buffering;
        (at, was_empty || (play_if_idle && idle))
    }

    /// Seek; si el audio todavia se esta bajando se hace en cuanto termine (ver ticker).
    fn seek_to(&self, secs: f64) {
        let done = self.engine.seek(secs);
        let mut st = self.state.borrow_mut();
        st.pending_seek = (!done).then_some(secs);
        st.lyrics_idx = -2;
    }

    fn clear_prefetch(&self) {
        let mut st = self.state.borrow_mut();
        st.prefetch = None;
        st.prefetching = None;
    }

    fn play_index(self: &Rc<Self>, i: usize, autoplay: bool) {
        let (track, seq) = {
            let mut st = self.state.borrow_mut();
            let Some(t) = st.queue.get(i).cloned() else { return };
            if !std::mem::take(&mut st.resuming) {
                st.restore_position = None;
            }
            st.index = Some(i);
            st.play_seq += 1;
            st.buffering = true;
            st.pending_seek = None;
            st.jam_ready = false;
            st.added.remove(&t.id);
            (t, st.play_seq)
        };
        self.jam_host_hold();
        self.engine.stop();
        self.start_track_ui(&track);
        self.push_queue();
        spawn(load_and_play(self.clone(), track, seq, autoplay));
    }

    fn start_track_ui(self: &Rc<Self>, t: &Track) {
        {
            let mut st = self.state.borrow_mut();
            // Scrobble de la cancion anterior si corresponde.
            if let Some(prev) = st.current.clone() {
                if !st.scrobbled && scrobbler::should_scrobble(prev.duration as f64, st.listened) {
                    let (http, cfg, started) = (self.http.clone(), st.cfg.clone(), st.started_unix);
                    spawn(scrobbler::scrobble(http, cfg, prev, started));
                }
            }
            st.current = Some(t.clone());
            st.current_data = None;
            st.listened = 0.0;
            st.scrobbled = false;
            st.started_unix = crate::util::unix_now();
            st.lyrics.clear();
            st.lyrics_idx = -1;
            st.segments.clear();
        }
        let (title, artist, dur) = (t.title.clone(), t.artists.clone(), t.duration as f32);
        self.ui.state(move |s| {
            s.set_has_song(true);
            s.set_buffering(true);
            s.set_playing(false);
            s.set_now_title(title.into());
            s.set_now_artist(artist.into());
            s.set_position(0.0);
            s.set_duration(dur);
            s.set_position_text("0:00".into());
            s.set_duration_text(fmt_time(dur as f64).into());
            s.set_lyrics(Default::default());
            s.set_lyrics_plain("".into());
            s.set_lyrics_current(-1);
            s.set_lyrics_status("Buscando letra…".into());
        });
        self.ui.mark_current(Some(t.id.clone()));
        let tt = t.clone();
        self.ui.run(move |_| {
            desktop::set_now_playing(Some(&tt), true);
            // El panel de Windows cambia de inmediato (no hasta que termine de cargar).
            desktop::smtc_metadata(&tt);
        });
        // Portada grande
        if let Some(url) = t.thumb.clone() {
            let b = self.clone();
            let id = t.id.clone();
            spawn(async move {
                let big = model::resize_thumb(&url, 226);
                if let Some(p) = b.images.get(&big, 226).await {
                    let still = b.state.borrow().current.as_ref().map(|c| c.id == id).unwrap_or(false);
                    if still {
                        b.ui.state(move |s| s.set_now_cover(slint::Image::from_rgba8(p)));
                    }
                }
            });
        } else {
            self.ui.state(|s| s.set_now_cover(Default::default()));
        }
    }

    fn next(self: &Rc<Self>, user: bool) {
        let (idx, len, repeat, radio) = {
            let st = self.state.borrow();
            (st.index, st.queue.len(), st.repeat, st.cfg.autoplay_radio)
        };
        if !user && repeat == 2 {
            // Repetir una: se reusa el audio en memoria.
            let data = self.state.borrow().current_data.clone();
            if let Some(data) = data {
                if self.engine.load(&data, false).is_ok() {
                    self.state.borrow_mut().listened = 0.0;
                    self.state.borrow_mut().scrobbled = false;
                    // Los invitados ya tienen el audio: solo vuelven al principio.
                    self.jam_host_state();
                    return;
                }
            }
        }
        match idx {
            Some(i) if i + 1 < len => self.play_index(i + 1, true),
            _ if repeat == 1 && len > 0 => self.play_index(0, true),
            _ if radio => {
                let cur = self.state.borrow().current.clone();
                if let Some(t) = cur {
                    let b = self.clone();
                    spawn(async move {
                        extend_with_radio(b.clone(), t, false).await;
                        let (idx, len) = {
                            let st = b.state.borrow();
                            (st.index, st.queue.len())
                        };
                        if let Some(i) = idx {
                            if i + 1 < len {
                                b.play_index(i + 1, true);
                            }
                        }
                    });
                }
            }
            _ => {
                self.engine.stop();
                self.on_play_state();
            }
        }
    }

    fn on_play_state(&self) {
        let paused = self.engine.is_paused() || !self.engine.has_track();
        let pos = self.engine.position();
        self.state.borrow_mut().last_paused = paused;
        self.ui.state(move |s| s.set_playing(!paused));
        let cur = self.state.borrow().current.clone();
        self.ui.run(move |_| {
            desktop::smtc_playback(paused, pos);
            desktop::set_now_playing(cur.as_ref(), paused);
        });
        self.update_discord();
        self.jam_host_state();
    }

    fn update_discord(&self) {
        let (cfg, cur) = {
            let st = self.state.borrow();
            (st.cfg.clone(), st.current.clone())
        };
        if !cfg.discord {
            return;
        }
        if let Some(track) = cur {
            self.discord.send(discord::Msg::Update {
                track,
                paused: self.engine.is_paused(),
                position: self.engine.position(),
                hide_when_paused: cfg.discord_hide_paused,
            });
        }
    }

    fn prefetch_next(self: &Rc<Self>) {
        let next = {
            let st = self.state.borrow();
            let i = st.index.map(|i| i + 1).unwrap_or(0);
            st.queue.get(i).cloned()
        };
        let Some(t) = next else { return };
        {
            let mut st = self.state.borrow_mut();
            if st.prefetching.as_deref() == Some(t.id.as_str()) || st.prefetch.as_ref().map(|p| p.0 == t.id).unwrap_or(false) {
                return;
            }
            st.prefetching = Some(t.id.clone());
        }
        let b = self.clone();
        spawn(async move {
            let low = b.cfg().quality == 1;
            let id = t.id.clone();
            match fetch_audio(&b, &id, low).await {
                Ok(data) => {
                    let mut st = b.state.borrow_mut();
                    if st.prefetching.as_deref() == Some(id.as_str()) {
                        st.prefetch = Some((id, data));
                        st.prefetching = None;
                    }
                }
                Err(e) => {
                    log::warn!("precarga de {id}: {e}");
                    b.state.borrow_mut().prefetching = None;
                }
            }
        });
    }

    // ---------- listas en pantalla ----------

    fn show_tracks(&self, kind: ListKind, tracks: Vec<Track>, current: Option<String>) {
        let covers: Vec<_> = tracks.iter().map(|t| t.thumb.as_deref().and_then(|u| self.images.cached(u))).collect();
        self.ui.set_tracks(kind, &tracks, covers.clone(), current);
        for (i, t) in tracks.into_iter().enumerate() {
            if covers[i].is_some() {
                continue;
            }
            let Some(url) = t.thumb.clone() else { continue };
            let (images, ui) = (self.images.clone(), self.ui.clone());
            spawn(async move {
                if let Some(p) = images.get(&url, 60).await {
                    ui.set_track_cover(kind, i, t.id, p);
                }
            });
        }
    }

    fn set_list(&self, kind: ListKind, tracks: Vec<Track>) {
        let current = self.state.borrow().current.as_ref().map(|t| t.id.clone());
        self.state.borrow_mut().lists.insert(kind, tracks.clone());
        self.show_tracks(kind, tracks, current);
    }

    fn set_cards(&self, kind: CardListKind, cards: Vec<Card>) {
        let covers: Vec<_> = cards.iter().map(|c| c.thumb.as_deref().and_then(|u| self.images.cached(u))).collect();
        self.ui.set_cards(kind, &cards, covers.clone());
        for (i, c) in cards.iter().enumerate() {
            if covers[i].is_some() {
                continue;
            }
            let Some(url) = c.thumb.clone() else { continue };
            let (images, ui, id) = (self.images.clone(), self.ui.clone(), c.id.clone());
            spawn(async move {
                if let Some(p) = images.get(&url, 60).await {
                    ui.set_card_cover(kind, i, id, p);
                }
            });
        }
        self.state.borrow_mut().cards.insert(kind, cards);
    }

    // ---------- sesion ----------

    fn save_session(&self) {
        let st = self.state.borrow();
        if !st.cfg.restore_session {
            return;
        }
        let session = Session {
            queue: st.queue.iter().take(300).cloned().collect(),
            index: st.index,
            // De invitado en un Jam lo que suena no es tu cola: se guarda donde ibas.
            position: if self.engine.has_track() && !self.is_jam_guest() {
                self.engine.position()
            } else {
                st.restore_position.unwrap_or(0.0)
            },
        };
        if let Ok(s) = serde_json::to_vec(&session) {
            let _ = std::fs::write(self.dir.join("session.json"), s);
        }
    }

    fn restore_session(self: &Rc<Self>) {
        if !self.cfg().restore_session {
            return;
        }
        let Some(session) = std::fs::read(self.dir.join("session.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Session>(&b).ok())
        else {
            return;
        };
        // Colas guardadas por versiones con Spotify: ya no se pueden reproducir.
        if session.queue.iter().any(|t| t.id.starts_with("spotify:")) {
            return;
        }
        let Some(i) = session.index.filter(|i| *i < session.queue.len()) else {
            self.push_queue();
            return;
        };
        let t = session.queue[i].clone();
        {
            let mut st = self.state.borrow_mut();
            st.queue = session.queue;
            st.index = Some(i);
            st.restore_position = Some(session.position);
        }
        self.start_track_ui(&t);
        self.state.borrow_mut().buffering = false;
        let pos = session.position as f32;
        self.ui.state(move |s| {
            s.set_buffering(false);
            s.set_position(pos);
            s.set_position_text(fmt_time(pos as f64).into());
            s.set_lyrics_status("".into());
        });
        self.push_queue();
        // Se va bajando la cancion guardada, para que el primer Play sea inmediato.
        let b = self.clone();
        spawn(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            if b.engine.has_track() || b.state.borrow().index != Some(i) {
                return;
            }
            let low = b.cfg().quality == 1;
            if let Ok(g) = fetch_audio(&b, &t.id, low).await {
                let mut st = b.state.borrow_mut();
                if st.prefetch.is_none() && st.index == Some(i) {
                    st.prefetch = Some((t.id.clone(), g));
                }
            }
        });
    }
}

// ---------- tareas asincronas ----------

/// Resuelve con yt-dlp y empieza a descargar; vuelve con el primer pedazo.
async fn fetch_audio(b: &Backend, id: &str, low: bool) -> Result<Arc<Growing>, String> {
    let http = &b.http;
    let ui = b.ui.clone();
    stream::ensure_tools(http, move |m| ui.status(m)).await?;
    let resolve = |cookies: Option<PathBuf>| {
        let vid = id.to_string();
        async move {
            tokio::task::spawn_blocking(move || stream::resolve(&vid, low, cookies.as_deref()))
                .await
                .map_err(|e| e.to_string())?
        }
    };
    let resolved = match resolve(None).await {
        Ok(r) => r,
        // YouTube pide confirmar que no eres un robot (pasa al saltar muchas canciones
        // seguidas desde la misma IP): con la sesion iniciada se reintenta con tus cookies.
        Err(e) if stream::is_bot_check(&e) => {
            log::warn!("yt-dlp pidió verificación: {e}");
            let Some(header) = b.rp.user_auth_cookie_header() else {
                return Err("YouTube pidió confirmar que no eres un robot. Inicia sesión en Ajustes → Cuenta \
                    (una sola vez) y no volverá a pasar; o espera unos minutos."
                    .into());
            };
            let file = b.dir.join("yt-dlp-cookies.txt");
            stream::write_cookie_file(&header, &file)?;
            let r = resolve(Some(file.clone())).await;
            let _ = std::fs::remove_file(&file);
            r.map_err(|e| {
                if stream::is_bot_check(&e) {
                    "YouTube sigue pidiendo confirmar que no eres un robot. Espera unos minutos.".to_string()
                } else {
                    e
                }
            })?
        }
        Err(e) => return Err(e),
    };
    match stream::start_download(http, &resolved).await {
        Ok(g) => Ok(g),
        // YouTube a veces rechaza una URL recien resuelta (403): se pide otra una vez.
        Err(e) if e.contains("HTTP 403") => {
            log::info!("{id}: 403, se vuelve a resolver");
            stream::forget(id);
            let again = resolve(None).await?;
            stream::start_download(http, &again).await.inspect_err(|_| stream::forget(id))
        }
        Err(e) => {
            stream::forget(id);
            Err(e)
        }
    }
}

async fn play_failed(b: &B, track: &Track, seq: u64, e: String) {
    log::warn!("no se pudo cargar {}: {e}", track.id);
    b.ui.status(format!("No se pudo reproducir «{}»: {e}", track.title));
    let errors = {
        let mut st = b.state.borrow_mut();
        st.buffering = false;
        st.errors_in_row += 1;
        st.errors_in_row
    };
    b.ui.state(|s| s.set_buffering(false));
    // Si YouTube pide verificar, saltar a la siguiente solo lo empeora.
    if errors < 3 && !e.contains("robot") {
        tokio::time::sleep(Duration::from_secs(2)).await;
        if b.state.borrow().play_seq == seq {
            b.next(true);
        }
    }
}

async fn load_and_play(b: B, track: Track, seq: u64, autoplay: bool) {
    let restore = b.state.borrow_mut().restore_position.take().filter(|p| *p > 1.0);
    // Saltando canciones muy rapido: se espera un instante antes de correr yt-dlp, para
    // no resolver cada cancion por la que se pasa (YouTube lo toma como un robot).
    let quick = {
        let mut st = b.state.borrow_mut();
        let quick = st.last_play_at.elapsed() < Duration::from_millis(900);
        st.last_play_at = Instant::now();
        quick
    };
    let prefetched = b.state.borrow().prefetch.as_ref().is_some_and(|p| p.0 == track.id);
    if quick && !prefetched {
        tokio::time::sleep(Duration::from_millis(450)).await;
        if b.state.borrow().play_seq != seq {
            return;
        }
    }
    let pre = {
        let mut st = b.state.borrow_mut();
        match st.prefetch.take() {
            Some((id, data)) if id == track.id => Some(data),
            other => {
                st.prefetch = other;
                None
            }
        }
    };
    let low = b.cfg().quality == 1;
    let result = match pre {
        Some(d) => Ok(d),
        None => fetch_audio(&b, &track.id, low).await,
    };
    if b.state.borrow().play_seq != seq {
        return; // el usuario ya eligio otra cancion
    }
    let data = match result {
        Ok(d) => d,
        Err(e) => return play_failed(&b, &track, seq, e).await,
    };
    // Para retomar donde se quedo hace falta poder hacer seek: se espera la descarga.
    if restore.is_some() {
        let r = data.wait_done().await;
        if b.state.borrow().play_seq != seq {
            return;
        }
        if let Err(e) = r {
            return play_failed(&b, &track, seq, e).await;
        }
    }
    if let Err(e) = b.engine.load(&data, !autoplay) {
        b.ui.status(e);
        return;
    }
    if let Some(pos) = restore {
        b.engine.seek(pos);
    }
    b.jam_share_when_ready(&track, &data, seq);
    {
        let mut st = b.state.borrow_mut();
        st.current_data = Some(data);
        st.buffering = false;
        st.errors_in_row = 0;
        st.last_tick = Instant::now();
    }
    log::info!("sonando: {} - {}", track.artists, track.title);
    b.ui.state(|s| {
        s.set_buffering(false);
        s.set_status("".into());
    });
    b.on_play_state();
    let t = track.clone();
    b.ui.run(move |_| desktop::smtc_metadata(&t));

    let cfg = b.cfg();
    if cfg.notifications {
        let (title, artist) = (track.title.clone(), track.artists.clone());
        tokio::task::spawn_blocking(move || crate::util::notify(&title, &artist));
    }
    if cfg.lastfm.enabled || cfg.listenbrainz {
        spawn(scrobbler::now_playing(b.http.clone(), cfg.clone(), track.clone()));
    }
    if cfg.sponsorblock {
        let b2 = b.clone();
        let id = track.id.clone();
        spawn(async move {
            let segs = sponsorblock::fetch(&b2.http, "https://sponsor.ajay.app", &["music_offtopic".to_string()], &id).await;
            if b2.state.borrow().play_seq == seq {
                b2.state.borrow_mut().segments = segs;
            }
        });
    }
    spawn(load_lyrics(b.clone(), track.clone(), seq));

    // Si la cola se acaba pronto, se agrega la radio de esta cancion.
    let (idx, len) = {
        let st = b.state.borrow();
        (st.index.unwrap_or(0), st.queue.len())
    };
    if cfg.autoplay_radio && idx + 2 >= len {
        spawn(extend_with_radio(b.clone(), track, false));
    } else {
        b.prefetch_next();
    }
}

async fn extend_with_radio(b: B, seed: Track, replace_rest: bool) {
    if b.state.borrow().radio_loading {
        return;
    }
    b.state.borrow_mut().radio_loading = true;
    let queue_gen = b.state.borrow().queue_gen;
    let res = b.query().music_radio_track(&seed.id).await;
    b.state.borrow_mut().radio_loading = false;
    {
        let st = b.state.borrow();
        if st.queue_gen != queue_gen {
            return; // se cambio de cola mientras llegaba la radio
        }
    }
    match res {
        Ok(p) => {
            {
                let mut st = b.state.borrow_mut();
                let existing: std::collections::HashSet<String> =
                    if replace_rest { Default::default() } else { st.queue.iter().map(|t| t.id.clone()).collect() };
                if replace_rest {
                    let keep = st.index.map(|i| i + 1).unwrap_or(0).min(st.queue.len());
                    st.queue.truncate(keep);
                }
                let mut seen = existing;
                seen.insert(seed.id.clone());
                for it in p.items {
                    let t = model::from_track(it);
                    if seen.insert(t.id.clone()) {
                        st.queue.push(t);
                    }
                }
            }
            b.push_queue();
            b.clear_prefetch();
            b.prefetch_next();
        }
        Err(e) => log::warn!("radio: {e}"),
    }
}

async fn load_lyrics(b: B, t: Track, seq: u64) {
    let cfg = b.cfg();
    let first_artist = t.artists.split(", ").next().unwrap_or(&t.artists).to_string();
    let mut synced: Vec<(u64, String)> = Vec::new();
    let mut plain = String::new();
    if cfg.synced_lyrics {
        match lyrics::fetch(&b.http, &t.title, &first_artist, t.album.as_deref(), t.duration as f64, false).await {
            Ok(Some(l)) => {
                synced = l.lines;
                plain = l.plain.unwrap_or_default();
            }
            Ok(None) => {}
            Err(e) => log::warn!("LRCLIB: {e}"),
        }
    }
    if synced.is_empty() && plain.is_empty() {
        // Letra oficial de YouTube Music (sin sincronizar).
        if let Ok(d) = b.query().music_details(&t.id).await {
            if let Some(lid) = d.lyrics_id {
                if let Ok(l) = b.query().music_lyrics(lid).await {
                    plain = format!("{}\n\n{}", l.body, l.footer);
                }
            }
        }
    }
    if b.state.borrow().play_seq != seq {
        return;
    }
    let lines: Vec<String> = synced.iter().map(|(_, s)| if s.is_empty() { "♪".to_string() } else { s.clone() }).collect();
    let status = if lines.is_empty() && plain.trim().is_empty() { "No se encontró la letra" } else { "" };
    b.state.borrow_mut().lyrics = synced;
    b.state.borrow_mut().lyrics_idx = -2;
    b.ui.state(move |s| {
        let model: Vec<slint::SharedString> = lines.into_iter().map(Into::into).collect();
        s.set_lyrics(slint::ModelRc::from(Rc::new(slint::VecModel::from(model))));
        s.set_lyrics_plain(plain.trim().into());
        s.set_lyrics_status(status.into());
    });
}

async fn load_home(b: B) {
    b.ui.loading(true);
    let q = b.query();
    match q.music_charts(None).await {
        Ok(charts) => {
            log::info!("charts: {} top, {} tendencias, {} listas", charts.top_tracks.len(), charts.trending_tracks.len(), charts.playlists.len());
            let mut tracks: Vec<Track> = charts.top_tracks.into_iter().map(model::from_track).collect();
            if tracks.is_empty() {
                tracks = charts.trending_tracks.into_iter().map(model::from_track).collect();
            }
            if tracks.is_empty() {
                // YouTube Music ahora pone los tops como listas: se abre la primera.
                let pid = charts
                    .top_playlist_id
                    .clone()
                    .or_else(|| charts.trending_playlist_id.clone())
                    .or_else(|| charts.playlists.first().map(|p| p.id.clone()));
                if let Some(pid) = pid {
                    match q.music_playlist(&pid).await {
                        Ok(p) => tracks = p.tracks.items.into_iter().map(model::from_track).collect(),
                        Err(e) => log::warn!("lista top: {e}"),
                    }
                }
            }
            b.set_list(ListKind::Home, tracks);
            let mut cards: Vec<Card> = Vec::new();
            if let Ok(albums) = q.music_new_albums().await {
                cards.extend(albums.into_iter().take(40).map(model::from_album));
            }
            cards.extend(charts.playlists.into_iter().map(model::from_playlist));
            cards.extend(charts.artists.into_iter().take(20).map(model::from_artist));
            b.set_cards(CardListKind::Home, cards);
        }
        Err(e) => {
            log::warn!("inicio: {e}");
            b.ui.status(format!("No se pudo cargar el inicio: {e}"));
        }
    }
    b.ui.loading(false);
}

/// `generation`: si mientras tanto se escribio otra cosa, el resultado se descarta.
async fn search(b: B, q: String, generation: u64) {
    b.ui.loading(true);
    let query = b.query();
    let q = q.trim();
    // Canciones con el filtro dedicado (trae ~20 y bien etiquetadas) y el resto
    // (albumes, artistas, listas) de la busqueda general, en paralelo.
    let (tracks_res, main_res, albums_res, lists_res) = tokio::join!(
        query.music_search_tracks(q),
        query.music_search_main(q),
        query.music_search_albums(q),
        query.music_search_playlists(q, false)
    );
    if b.state.borrow().search_gen != generation {
        return; // la busqueda siguiente apaga el "Cargando…"
    }
    match main_res {
        Ok(res) => {
            let (fallback_tracks, mut cards) = model::split_items(res.items.items);
            let mut seen: std::collections::HashSet<String> = cards.iter().map(|c| c.id.clone()).collect();
            if let Ok(a) = albums_res {
                cards.extend(a.items.items.into_iter().map(model::from_album).filter(|c| seen.insert(c.id.clone())));
            }
            if let Ok(p) = lists_res {
                cards.extend(p.items.items.into_iter().map(model::from_playlist).filter(|c| seen.insert(c.id.clone())));
            }
            let tracks = match tracks_res {
                Ok(t) if !t.items.items.is_empty() => t.items.items.into_iter().map(model::from_track).collect(),
                _ => fallback_tracks,
            };
            b.set_list(ListKind::Search, tracks);
            b.set_cards(CardListKind::Search, cards);
            b.ui.status("");
        }
        Err(e) => b.ui.status(format!("Error al buscar: {e}")),
    }
    b.ui.loading(false);
}

/// `record`: es una pantalla nueva (queda en el historial); false al ir atras/adelante.
async fn open_card(b: B, kind: CardKind, id: String, record: bool) {
    b.ui.loading(true);
    // Nombre de la tarjeta que se toco: YouTube ya no siempre manda el de las listas.
    let card_title = b.state.borrow().cards.values().flatten().find(|c| c.id == id).map(|c| c.title.clone());
    let q = b.query();
    let result: Result<(String, String, Option<String>, Vec<Track>, Vec<Card>), String> = match kind {
        CardKind::Album => q.music_album(&id).await.map_err(|e| e.to_string()).map(|a| {
            let cover = model::pick_thumb(&a.cover, 60);
            let artists = a.artists.iter().map(|x| x.name.as_str()).collect::<Vec<_>>().join(", ");
            let mut sub = format!("Álbum • {artists}");
            if let Some(y) = a.year {
                sub.push_str(&format!(" • {y}"));
            }
            sub.push_str(&format!(" • {} canciones", a.tracks.len()));
            let tracks = a
                .tracks
                .into_iter()
                .map(|t| {
                    let mut t = model::from_track(t);
                    if t.thumb.is_none() {
                        t.thumb = cover.clone();
                    }
                    if t.album.is_none() {
                        t.album = Some(a.name.clone());
                    }
                    t
                })
                .collect();
            let cards = a.variants.into_iter().map(model::from_album).collect();
            (a.name, sub, cover, tracks, cards)
        }),
        CardKind::Playlist => match q.music_playlist(&id).await {
            Ok(mut p) => {
                let _ = p.tracks.extend_limit(&q, 300).await;
                let sub = format!(
                    "Lista{}{}",
                    p.channel.as_ref().map(|c| format!(" • {}", c.name)).unwrap_or_default(),
                    p.track_count.map(|n| format!(" • {n} canciones")).unwrap_or_default()
                );
                let cover = model::pick_thumb(&p.thumbnail, 60);
                let tracks = p.tracks.items.into_iter().map(model::from_track).collect();
                let cards = p.related_playlists.items.into_iter().map(model::from_playlist).collect();
                let name = if p.name.is_empty() { card_title.clone().unwrap_or_else(|| "Lista".into()) } else { p.name };
                Ok((name, sub, cover, tracks, cards))
            }
            Err(e) => Err(e.to_string()),
        },
        CardKind::Artist => q.music_artist(&id, false).await.map_err(|e| e.to_string()).map(|a| {
            let sub = a
                .subscriber_count
                .map(|n| format!("Artista • {} suscriptores", model::compact(n)))
                .unwrap_or_else(|| "Artista".into());
            let cover = model::pick_thumb(&a.header_image, 60);
            let tracks = a.tracks.into_iter().map(model::from_track).collect();
            let mut cards: Vec<Card> = a.albums.into_iter().map(model::from_album).collect();
            cards.extend(a.playlists.into_iter().map(model::from_playlist));
            cards.extend(a.similar_artists.into_iter().map(model::from_artist));
            (a.name, sub, cover, tracks, cards)
        }),
    };
    b.ui.loading(false);
    let (title, subtitle, cover, tracks, cards) = match result {
        Ok(r) => r,
        Err(e) => {
            b.ui.status(format!("No se pudo abrir: {e}"));
            return;
        }
    };
    if record {
        b.record_nav(Nav::Detail(kind, id.clone()));
    }
    b.state.borrow_mut().detail_tracks_id = Some((kind, id));
    b.set_list(ListKind::Detail, tracks);
    b.set_cards(CardListKind::Detail, cards);
    let k = kind.as_str();
    b.ui.state(move |s| {
        s.set_detail_kind(k.into());
        s.set_detail_title(title.into());
        s.set_detail_subtitle(subtitle.into());
        s.set_detail_cover(Default::default());
        s.set_page("detail".into());
        s.set_status("".into());
    });
    if let Some(url) = cover {
        let big = model::resize_thumb(&url, 300);
        if let Some(p) = b.images.get(&big, 300).await {
            b.ui.state(move |s| s.set_detail_cover(slint::Image::from_rgba8(p)));
        }
    }
}

async fn load_library(b: B) {
    if !b.cfg().logged_in {
        return;
    }
    let q = b.query().authenticated();
    let liked = match q.music_liked_tracks().await {
        Ok(mut p) => {
            let _ = p.tracks.extend_limit(&q, 500).await;
            Ok(p.tracks.items)
        }
        // Si la lista «Me gusta» (LM) no se puede leer, las canciones guardadas incluyen
        // los Me gusta (y las de albumes guardados).
        Err(e) => {
            log::warn!("me gusta: {e}");
            match q.music_saved_tracks().await {
                Ok(mut p) => {
                    let _ = p.extend_limit(&q, 500).await;
                    Ok(p.items)
                }
                Err(e2) => Err((e, e2)),
            }
        }
    };
    match liked {
        Ok(items) => b.set_list(ListKind::Library, items.into_iter().map(model::from_track).collect()),
        Err((e, e2)) => {
            log::warn!("biblioteca: {e} / {e2}");
            let msg = if matches!(e2, rustypipe::error::Error::Auth(_)) || e.to_string().contains("sectionListRenderer") {
                "Tu sesión de YouTube venció. Vuelve a iniciar sesión en Ajustes → Cuenta.".to_string()
            } else {
                format!("No se pudo cargar tu biblioteca: {e}")
            };
            b.ui.status(msg);
            return;
        }
    }
    let mut cards = Vec::new();
    if let Ok(p) = q.music_saved_playlists().await {
        cards.extend(p.items.into_iter().map(model::from_playlist));
    }
    if let Ok(a) = q.music_saved_albums().await {
        cards.extend(a.items.into_iter().map(model::from_album));
    }
    b.set_cards(CardListKind::Library, cards);
    b.ui.state(|s| s.set_logged_in(true));
}

/// Inicia sesion abriendo la ventana de Google en un proceso aparte (ver login.rs).
async fn login(b: B, _unused: String) {
    let set_status = |b: &B, m: String| b.ui.run(move |ui| ui.global::<Cfg>().set_account_status(m.into()));
    set_status(&b, "Inicia sesión en la ventana de Google…".into());
    let out = b.dir.join("login-cookie.tmp");
    let _ = std::fs::remove_file(&out);
    let out2 = out.clone();
    // Centro de la ventana principal, para abrir el login justo encima.
    let (tx, rx) = tokio::sync::oneshot::channel();
    b.ui.run(move |ui| {
        let pos = ui.window().position();
        let size = ui.window().size();
        let _ = tx.send((pos.x + size.width as i32 / 2, pos.y + size.height as i32 / 2));
    });
    let center = rx.await.ok();
    let ran = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut cmd = std::process::Command::new(exe);
        cmd.arg("--login").arg(&out2);
        if let Some((x, y)) = center {
            cmd.arg(x.to_string()).arg(y.to_string());
        }
        cmd.status()
            .map_err(|e| format!("no se pudo abrir la ventana de inicio de sesión: {e}"))?;
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|r| r);
    let cookie = std::fs::read_to_string(&out).unwrap_or_default();
    let _ = std::fs::remove_file(&out);
    // YouTube rota las cookies: la sesion del WebView no se debe volver a usar.
    remove_login_webview().await;

    let msg = match ran {
        Err(e) => e,
        Ok(()) if cookie.trim().is_empty() => "Inicio de sesión cancelado.".to_string(),
        Ok(()) => match b.rp.user_auth_set_cookie(cookie.trim()).await {
            Ok(()) => {
                b.state.borrow_mut().cfg.logged_in = true;
                config::save(&b.dir, &b.cfg());
                spawn(load_library(b.clone()));
                "Sesión iniciada. Tus Me gusta y listas están en «Biblioteca».".to_string()
            }
            Err(e) => format!("YouTube rechazó la sesión: {e}"),
        },
    };
    log::info!("login: {msg}");
    set_status(&b, msg);
}

async fn logout(b: B) {
    let _ = b.rp.user_auth_remove_cookie().await;
    b.state.borrow_mut().cfg.logged_in = false;
    config::save(&b.dir, &b.cfg());
    b.ui.state(|s| s.set_logged_in(false));
    b.ui.run(|ui| ui.global::<Cfg>().set_account_status("Sin sesión".into()));
}

async fn lastfm_connect(b: B) {
    b.ui.run(|ui| ui.global::<Cfg>().set_lastfm_status("Autoriza la app en el navegador…".into()));
    let cfg = b.cfg().lastfm;
    let msg = match scrobbler::connect_lastfm(b.http.clone(), cfg).await {
        Ok((sk, user)) => {
            {
                let mut st = b.state.borrow_mut();
                st.cfg.lastfm.session_key = Some(sk);
                st.cfg.lastfm.user = Some(user.clone());
                st.cfg.lastfm.enabled = true;
            }
            config::save(&b.dir, &b.cfg());
            b.ui.run(|ui| ui.global::<Cfg>().set_lastfm(true));
            format!("Conectado como {user}")
        }
        Err(e) => e,
    };
    b.ui.run(move |ui| ui.global::<Cfg>().set_lastfm_status(msg.into()));
}

/// Revisa si hay version nueva al abrir y cada 6 horas (el boton UpDate se pone rojo).
async fn update_loop(b: B) {
    tokio::time::sleep(Duration::from_secs(5)).await;
    loop {
        let _ = check_update(&b).await;
        tokio::time::sleep(Duration::from_secs(6 * 3600)).await;
    }
}

async fn check_update(b: &B) -> Result<bool, String> {
    let rel = crate::update::latest(&b.http).await.inspect_err(|e| log::info!("actualizaciones: {e}"))?;
    let newer = crate::update::is_newer(&rel.version, crate::update::CURRENT);
    let version = rel.version.clone();
    b.state.borrow_mut().update = newer.then_some(rel);
    b.ui.state(move |s| {
        s.set_update_available(newer);
        s.set_update_tip(
            if newer { format!("Hay una versión nueva ({version}): clic para actualizar") } else { format!("Versión {}", crate::update::CURRENT) }
                .into(),
        );
    });
    Ok(newer)
}

async fn update_click(b: B) {
    if b.state.borrow().updating {
        return;
    }
    let rel = b.state.borrow().update.clone();
    let Some(rel) = rel else {
        // En blanco: se revisa en el momento.
        b.ui.status("Buscando actualizaciones…");
        match check_update(&b).await {
            Ok(true) => b.ui.status("Hay una versión nueva: vuelve a pulsar UpDate para instalarla."),
            Ok(false) => b.ui.status(format!("Ya tienes la última versión ({}).", crate::update::CURRENT)),
            Err(e) => b.ui.status(format!("No se pudo revisar si hay actualizaciones: {e}")),
        }
        return;
    };
    b.state.borrow_mut().updating = true;
    b.ui.status(format!("Descargando la versión {}…", rel.version));
    match crate::update::install(&b.http, &rel).await {
        Ok(()) => {
            log::info!("actualizando a {}", rel.version);
            b.ui.status("Instalando… la app se vuelve a abrir sola en unos segundos.");
            b.save_session();
            tokio::time::sleep(Duration::from_millis(500)).await;
            b.ui.run(|_| {
                let _ = slint::quit_event_loop();
            });
        }
        Err(e) => {
            b.state.borrow_mut().updating = false;
            b.ui.status(format!("No se pudo actualizar: {e}"));
        }
    }
}

/// Cada 250 ms: posicion, fin de cancion, SponsorBlock, letra y scrobbling.
async fn ticker(b: B) {
    let mut interval = tokio::time::interval(Duration::from_millis(250));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut save_counter = 0u32;
    loop {
        interval.tick().await;
        save_counter += 1;
        if save_counter % 60 == 0 {
            b.save_session();
        }
        let guest = b.is_jam_guest();
        if guest {
            b.jam_guest_sync(false);
        }
        if !b.engine.has_track() || b.state.borrow().buffering {
            continue;
        }
        // Seek que se pidio mientras el audio bajaba: ya se puede.
        let pending = b.state.borrow().pending_seek;
        if let Some(p) = pending.filter(|_| b.engine.can_seek()) {
            b.state.borrow_mut().pending_seek = None;
            b.engine.seek(p);
            b.on_play_state();
        }
        if !guest && save_counter % 8 == 0 {
            // Latido del Jam cada 2 s (por si a un invitado se le paso algun cambio).
            b.jam_host_state();
        }
        let paused = b.engine.is_paused();
        let pos = b.engine.position();

        if b.engine.finished() {
            // El invitado no avanza solo: espera la siguiente cancion del anfitrion.
            if !guest {
                b.next(false);
            }
            continue;
        }
        if paused != b.state.borrow().last_paused {
            b.on_play_state();
        }

        // SponsorBlock
        let skip_to = b.state.borrow().segments.iter().find(|s| pos >= s[0] && pos < s[1] - 0.5).map(|s| s[1]);
        if let Some(to) = skip_to {
            b.seek_to(to);
            b.jam_host_state();
        }

        let (lyric_idx, changed, scrobble) = {
            let mut st = b.state.borrow_mut();
            let now = Instant::now();
            if !paused {
                st.listened += now.duration_since(st.last_tick).as_secs_f64().min(1.0);
            }
            st.last_tick = now;
            let scrobble = match st.current.clone() {
                Some(t) if !st.scrobbled && scrobbler::should_scrobble(t.duration as f64, st.listened) => {
                    st.scrobbled = true;
                    Some((t, st.started_unix))
                }
                _ => None,
            };
            let ms = (pos * 1000.0) as u64 + 300;
            let idx = st.lyrics.partition_point(|(t, _)| *t <= ms) as i32 - 1;
            let changed = idx != st.lyrics_idx;
            st.lyrics_idx = idx;
            (idx, changed, scrobble)
        };
        if let Some((t, started)) = scrobble {
            let cfg = b.cfg();
            if cfg.lastfm.enabled || cfg.listenbrainz {
                spawn(scrobbler::scrobble(b.http.clone(), cfg, t, started));
            }
        }

        if b.state.borrow().last_snapshot.elapsed() > Duration::from_secs(1) {
            b.state.borrow_mut().last_snapshot = Instant::now();
            let st = b.state.borrow();
            *b.api_snapshot.lock().unwrap() = serde_json::json!({
                "song": st.current,
                "isPaused": paused,
                "elapsedSeconds": pos,
            });
        }

        if b.visible.load(Ordering::Relaxed) {
            let text = fmt_time(pos);
            b.ui.state(move |s| {
                s.set_position(pos as f32);
                s.set_position_text(text.into());
                if changed {
                    s.set_lyrics_current(lyric_idx);
                }
            });
        } else if changed {
            b.ui.state(move |s| s.set_lyrics_current(lyric_idx));
        }
    }
}

/// Donde queda la cancion `cur` de la cola despues de mover la de `from` a `to`.
fn moved_index(cur: usize, from: usize, to: usize) -> usize {
    if cur == from {
        return to;
    }
    let c = if from < cur { cur - 1 } else { cur };
    if to <= c { c + 1 } else { c }
}

fn shuffle_vec<T>(v: &mut [T]) {
    // xorshift sencillo; no hace falta una libreria de numeros aleatorios.
    let mut x = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1))
        | 1;
    for i in (1..v.len()).rev() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        v.swap(i, (x % (i as u64 + 1)) as usize);
    }
}

/// Copia la configuracion a la pantalla de ajustes.
pub fn push_cfg_to_ui(ui: &crate::AppWindow, cfg: &Config) {
    let c = ui.global::<Cfg>();
    c.set_restore_session(cfg.restore_session);
    c.set_autoplay_radio(cfg.autoplay_radio);
    c.set_notifications(cfg.notifications);
    c.set_quality(cfg.quality as i32);
    c.set_sponsorblock(cfg.sponsorblock);
    c.set_discord(cfg.discord);
    c.set_lastfm(cfg.lastfm.enabled);
    c.set_eq_enabled(cfg.eq_enabled);
    c.set_eq(slint::ModelRc::from(Rc::new(slint::VecModel::from(cfg.eq.to_vec()))));
    c.set_lastfm_status(
        cfg.lastfm.user.as_ref().map(|u| format!("Conectado como {u}")).unwrap_or_else(|| "Sin conectar".into()).into(),
    );
    c.set_account_status(if cfg.logged_in { "Sesión iniciada" } else { "Sin sesión (todo funciona igual, solo no ves tu biblioteca)" }.into());
    let s = ui.global::<AppState>();
    s.set_volume(cfg.volume);
    s.set_logged_in(cfg.logged_in);
    s.set_jam_name(cfg.jam_name.clone().into());
    s.set_jam_room(cfg.jam_room.clone().into());
}

/// Lo que se puede cambiar en la pantalla de ajustes. Las opciones avanzadas
/// (atajos, API local, puertos...) solo se editan en config.json.
#[derive(Debug)]
pub struct UiSettings {
    restore_session: bool,
    autoplay_radio: bool,
    notifications: bool,
    quality: u8,
    sponsorblock: bool,
    discord: bool,
    lastfm: bool,
    eq_enabled: bool,
    eq: [f32; 10],
}

impl UiSettings {
    fn apply(self, cfg: &mut Config) {
        cfg.restore_session = self.restore_session;
        cfg.autoplay_radio = self.autoplay_radio;
        cfg.notifications = self.notifications;
        cfg.quality = self.quality;
        cfg.sponsorblock = self.sponsorblock;
        cfg.discord = self.discord;
        cfg.lastfm.enabled = self.lastfm;
        cfg.eq_enabled = self.eq_enabled;
        cfg.eq = self.eq;
    }
}

/// Lee la pantalla de ajustes.
pub fn read_cfg_from_ui(ui: &crate::AppWindow) -> UiSettings {
    let c = ui.global::<Cfg>();
    UiSettings {
        restore_session: c.get_restore_session(),
        autoplay_radio: c.get_autoplay_radio(),
        notifications: c.get_notifications(),
        quality: c.get_quality().clamp(0, 1) as u8,
        sponsorblock: c.get_sponsorblock(),
        discord: c.get_discord(),
        lastfm: c.get_lastfm(),
        eq_enabled: c.get_eq_enabled(),
        eq: {
            let m = c.get_eq();
            std::array::from_fn(|i| m.row_data(i).unwrap_or(0.0).clamp(-12.0, 12.0))
        },
    }
}

/// Borra los datos del WebView de inicio de sesion. WebView2 tarda unos segundos en
/// soltar sus archivos despues de cerrar la ventana, por eso se reintenta.
async fn remove_login_webview() {
    let dir = crate::login::webview_dir();
    for _ in 0..20 {
        if !dir.exists() || std::fs::remove_dir_all(&dir).is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    log::warn!("no se pudo borrar {}", dir.display());
}


#[cfg(test)]
mod tests {
    use super::moved_index;

    #[test]
    fn mover_en_la_cola_sigue_a_la_cancion_actual() {
        // Cola [a b c d e], suena c (2).
        assert_eq!(moved_index(2, 2, 0), 0); // se arrastra la actual
        assert_eq!(moved_index(2, 0, 4), 1); // a pasa al final: c sube
        assert_eq!(moved_index(2, 4, 0), 3); // e pasa al inicio: c baja
        assert_eq!(moved_index(2, 3, 4), 2); // se mueve algo despues: no cambia
        assert_eq!(moved_index(2, 0, 1), 2); // se mueve algo antes: no cambia
        assert_eq!(moved_index(2, 4, 2), 3); // e queda justo antes de c
    }
}
