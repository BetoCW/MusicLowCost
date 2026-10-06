//! Jam: escuchar lo mismo, al mismo tiempo, con otras personas.
//!
//! YouTube no tiene Jam, y la API de datos v3 no sirve para sincronizar en tiempo real
//! (ni con sus cuotas). Aqui no se le pide nada a YouTube: la app del anfitrion es el
//! orquestador. El anfitrion ya tiene la cancion completa en memoria (ver stream.rs), asi
//! que reparte esos mismos bytes a los invitados y les dice que posicion tocar en que
//! instante de su reloj. Cada invitado estima el reloj del anfitrion (clock.rs) y corrige
//! su posicion si se desvia. YouTube ve una sola descarga por cancion, sin importar
//! cuantas personas haya en el Jam. Solo audio: el video nunca se pide.
//!
//! Transporte: iroh (QUIC) por internet, sin abrir puertos ni pasar direcciones IP. La sala
//! se identifica con nombre + contraseña: de ahi sale (con SHA-256) la llave del anfitrion,
//! asi que el invitado calcula el mismo id y lo busca en el directorio publico de iroh
//! (DNS/pkarr de n0). iroh intenta la conexion directa (hole punching) y si no puede usa sus
//! relays publicos. Todo va cifrado de punta a punta con esa llave. Las tramas (protocol.rs)
//! viajan por un stream bidireccional de QUIC (ver net.rs).

mod clock;
mod guest;
mod host;
mod net;
mod protocol;

pub use clock::Clock;
pub use guest::Guest;
pub use host::Host;
pub use protocol::{Request, is_youtube_id};

use crate::model::Track;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

/// Protocolo de la conexion (ALPN): solo se aceptan conexiones de esta app.
pub const ALPN: &[u8] = b"youtubeinrustweb/jam/2";
pub const MAX_GUESTS: usize = 8;

/// Que tocar: `pos_ms` de la cancion `tag` en el instante `at` (reloj del anfitrion, us).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PlayState {
    pub tag: u32,
    pub playing: bool,
    pub pos_ms: u64,
    pub at: i64,
}

impl PlayState {
    /// Posicion esperada (segundos) cuando el reloj del anfitrion marca `host_now`.
    pub fn expected(&self, host_now: Option<i64>) -> f64 {
        let base = self.pos_ms as f64 / 1000.0;
        match host_now {
            Some(now) if self.playing => base + now.saturating_sub(self.at).max(0) as f64 / 1e6,
            _ => base,
        }
    }
}

/// Avisos del Jam al backend (llegan como `Cmd::Jam(id, Event)`).
pub enum Event {
    Status(String),
    Peers(Vec<String>),
    /// Anfitrion: un invitado pide algo.
    Request(String, Request),
    /// Invitado: conectado al Jam de este anfitrion.
    Joined(String),
    /// Invitado: el anfitrion cambio de cancion; `shared` = false si no se reparte el audio.
    Track { tag: u32, track: Track, shared: bool },
    Audio { tag: u32, data: Arc<[u8]> },
    State(PlayState),
    Queue(Vec<Track>),
    /// Se cerro la conexion (invitado) o no se pudo abrir la sala (anfitrion).
    Ended(String),
}

impl std::fmt::Debug for Event {
    // A mano para no volcar el audio entero en el log.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Event::Audio { tag, data } => write!(f, "Audio {{ tag: {tag}, {} bytes }}", data.len()),
            Event::Track { tag, track, shared } => write!(f, "Track {{ tag: {tag}, id: {}, shared: {shared} }}", track.id),
            Event::Status(s) | Event::Joined(s) | Event::Ended(s) => write!(f, "{s:?}"),
            Event::Peers(p) => write!(f, "Peers({})", p.len()),
            Event::Request(who, r) => write!(f, "Request({who}, {r:?})"),
            Event::State(s) => write!(f, "{s:?}"),
            Event::Queue(q) => write!(f, "Queue({})", q.len()),
        }
    }
}

/// Cada Jam (crear o unirse) tiene su id: los avisos de uno anterior se ignoran.
pub fn next_id() -> u32 {
    static NEXT: AtomicU32 = AtomicU32::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Llave del anfitrion de la sala `room` con contraseña `password` (el nombre no distingue
/// mayusculas ni espacios de sobra; la contraseña si).
pub fn room_key(room: &str, password: &str) -> iroh::SecretKey {
    use sha2::{Digest, Sha256};
    let room = room.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    let mut h = Sha256::new();
    h.update(b"YoutubeInRustWeb jam v2\n");
    h.update(room.as_bytes());
    h.update(b"\n");
    h.update(password.as_bytes());
    iroh::SecretKey::from_bytes(&h.finalize().into())
}

/// Nombre visible: sin saltos de linea y como mucho 32 caracteres.
pub fn clean_name(name: &str, fallback: &str) -> String {
    let n: String = name.chars().filter(|c| !c.is_control()).take(32).collect();
    let n = n.trim();
    if n.is_empty() { fallback.to_string() } else { n.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posicion_esperada() {
        let s = PlayState { tag: 1, playing: true, pos_ms: 10_000, at: 1_000_000 };
        assert_eq!(s.expected(Some(3_500_000)), 12.5);
        assert_eq!(s.expected(None), 10.0);
        let paused = PlayState { playing: false, ..s };
        assert_eq!(paused.expected(Some(9_000_000)), 10.0);
    }

    use crate::backend::Cmd;
    use std::time::Duration;
    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

    fn track(id: &str) -> Track {
        Track { id: id.into(), title: "t".into(), artists: "a".into(), artist_id: None, album: None, duration: 200, thumb: None }
    }

    async fn next_event(rx: &mut UnboundedReceiver<Cmd>) -> (u32, Event) {
        match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
            Ok(Some(Cmd::Jam(id, ev))) => (id, ev),
            other => panic!("se esperaba un evento del Jam: {other:?}"),
        }
    }

    /// Espera el primer evento que cumpla `f` (los demas se ignoran).
    async fn wait_for<T>(rx: &mut UnboundedReceiver<Cmd>, mut f: impl FnMut(Event) -> Option<T>) -> T {
        loop {
            if let Some(t) = f(next_event(rx).await.1) {
                return t;
            }
        }
    }

    /// Anfitrion e invitado reales por 127.0.0.1 (el protocolo no depende de iroh): entrada,
    /// audio completo, estado, reloj, peticiones y cambio de cancion a mitad del envio.
    #[test]
    fn anfitrion_e_invitado_por_loopback() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            let (host_tx, mut host_rx) = unbounded_channel();
            let (guest_tx, mut guest_rx) = unbounded_channel();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap().to_string();
            let host = Host::start_tcp(listener, "secreta", "Ana", host_tx, 1);

            // Audio de mas de un pedazo, con contenido reconocible.
            let audio: Arc<[u8]> = (0..200_000u32).map(|i| (i % 251) as u8).collect::<Vec<_>>().into();
            host.set_track(&track("aaaaaaaaaaa"), Some(audio.clone()));
            host.set_state(true, 12.0);

            let guest = Guest::start_tcp(addr.clone(), "secreta".into(), "Beto".into(), guest_tx, 2);
            let who = wait_for(&mut guest_rx, |e| if let Event::Joined(h) = e { Some(h) } else { None }).await;
            assert_eq!(who, "Ana");
            let shared = wait_for(&mut guest_rx, |e| if let Event::Track { shared, .. } = e { Some(shared) } else { None }).await;
            assert!(shared);
            // El estado viaja por delante del audio (el control tiene prioridad).
            let s = wait_for(&mut guest_rx, |e| if let Event::State(s) = e { Some(s) } else { None }).await;
            assert!(s.playing && s.pos_ms == 12_000);
            let got = wait_for(&mut guest_rx, |e| if let Event::Audio { data, .. } = e { Some(data) } else { None }).await;
            assert_eq!(&*got, &*audio);

            // El anfitrion ve al invitado y recibe lo que pide (y descarta ids raros).
            let peers = wait_for(&mut host_rx, |e| if let Event::Peers(p) = e { Some(p) } else { None }).await;
            assert_eq!(peers, vec!["Beto".to_string()]);
            guest.request(Request::Add { track: track("abc&list=xx") });
            guest.request(Request::Add { track: track("bbbbbbbbbbb") });
            let (who, id) = wait_for(&mut host_rx, |e| match e {
                Event::Request(who, Request::Add { track }) => Some((who, track.id)),
                _ => None,
            })
            .await;
            assert_eq!((who.as_str(), id.as_str()), ("Beto", "bbbbbbbbbbb"));

            // El reloj se sincroniza (en la misma PC el desfase es casi el de los epoch).
            tokio::time::sleep(Duration::from_millis(600)).await;
            assert!(guest.clock.borrow().rtt_ms().is_some_and(|r| r < 100.0));

            // Cambio de cancion: llega la nueva completa.
            let audio2: Arc<[u8]> = vec![7u8; 70_000].into();
            host.set_track(&track("ccccccccccc"), Some(audio2.clone()));
            let (tag, got) = wait_for(&mut guest_rx, |e| if let Event::Audio { tag, data } = e { Some((tag, data)) } else { None }).await;
            assert_eq!(tag, 2);
            assert_eq!(&*got, &*audio2);

            // Contraseña incorrecta: rechazado.
            let (bad_tx, mut bad_rx) = unbounded_channel();
            let _bad = Guest::start_tcp(addr, "Secreta".into(), "X".into(), bad_tx, 3);
            let reason = wait_for(&mut bad_rx, |e| if let Event::Ended(r) = e { Some(r) } else { None }).await;
            assert_eq!(reason, "Contraseña incorrecta.");

            // Al cerrar el anfitrion, el invitado se entera.
            drop(host);
            let reason = wait_for(&mut guest_rx, |e| if let Event::Ended(r) = e { Some(r) } else { None }).await;
            assert!(!reason.is_empty());
        });
    }

    /// Sala real por internet (iroh + directorio y relays de n0). Necesita red:
    /// `cargo test -- --ignored jam_por_internet`
    #[test]
    #[ignore]
    fn jam_por_internet() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            let room = format!("prueba {}", crate::util::unix_now());
            let (host_tx, mut host_rx) = unbounded_channel();
            let (guest_tx, mut guest_rx) = unbounded_channel();
            let host = Host::start(&room, "clave1", "Ana", host_tx, 1);
            let t0 = std::time::Instant::now();
            let deadline = Duration::from_secs(60);
            // Abrir la sala tarda unos segundos (busca si ya existe y se publica).
            loop {
                match tokio::time::timeout(deadline, host_rx.recv()).await {
                    Ok(Some(Cmd::Jam(_, Event::Status(s)))) if s.starts_with("Jam listo") => break,
                    Ok(Some(Cmd::Jam(_, Event::Ended(e)))) => panic!("el anfitrión falló: {e}"),
                    Ok(Some(_)) => {}
                    other => panic!("sin respuesta del anfitrión: {other:?}"),
                }
            }
            println!("sala lista en {:?}", t0.elapsed());
            let audio: Arc<[u8]> = vec![3u8; 300_000].into();
            host.set_track(&track("aaaaaaaaaaa"), Some(audio.clone()));
            host.set_state(true, 1.0);

            let t1 = std::time::Instant::now();
            let _guest = Guest::start(room.to_uppercase(), "clave1".into(), "Beto".into(), guest_tx, 2);
            let got = loop {
                match tokio::time::timeout(deadline, guest_rx.recv()).await {
                    Ok(Some(Cmd::Jam(_, Event::Joined(h)))) => println!("unido a {h} en {:?}", t1.elapsed()),
                    Ok(Some(Cmd::Jam(_, Event::Audio { data, .. }))) => break data,
                    Ok(Some(Cmd::Jam(_, Event::Ended(e)))) => panic!("el invitado falló: {e}"),
                    Ok(Some(_)) => {}
                    other => panic!("sin respuesta del invitado: {other:?}"),
                }
            };
            println!("audio recibido en {:?}", t1.elapsed());
            assert_eq!(&*got, &*audio);
        });
    }

    #[test]
    fn nombres() {
        assert_eq!(clean_name("  Ana\n ", "x"), "Ana");
        assert_eq!(clean_name("", "Invitado"), "Invitado");
        assert_eq!(clean_name(&"a".repeat(50), "x").len(), 32);
    }
}
