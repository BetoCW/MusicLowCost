//! Anfitrion: acepta invitados, les reparte el audio de la cancion actual y el estado
//! de reproduccion, y le pasa al backend lo que piden.

use super::net::{self, Keep, Rd, Wr};
use super::protocol::{self, CHUNK, Frame, Msg, Request, VERSION};
use super::{Event, MAX_GUESTS, PlayState, clean_name, clock::micros_since};
use crate::backend::Cmd;
use crate::model::Track;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::AbortHandle;

/// Cuantas canciones de la cola se muestran a los invitados.
const QUEUE_PREVIEW: usize = 30;

/// Cancion actual y su audio (None si no se reparte).
type Current = (Track, Option<Arc<[u8]>>);

pub struct Host {
    inner: Rc<Inner>,
    accept: AbortHandle,
}

struct Inner {
    id: u32,
    epoch: Instant,
    code: String,
    name: String,
    tx: UnboundedSender<Cmd>,
    peers: RefCell<Vec<Peer>>,
    /// Tareas de las conexiones (lectoras), para cerrarlas con el Jam.
    conns: RefCell<Vec<AbortHandle>>,
    next_peer: Cell<u64>,
    /// Cancion actual; los escritores dejan de mandar audio de un tag viejo.
    tag: Rc<Cell<u32>>,
    current: RefCell<Option<Current>>,
    state: Cell<Option<PlayState>>,
    queue: RefCell<Vec<Track>>,
    /// Para cerrarlo bien al terminar el Jam.
    endpoint: RefCell<Option<iroh::Endpoint>>,
}

struct Peer {
    id: u64,
    name: String,
    ctrl: UnboundedSender<Out>,
    audio: UnboundedSender<(u32, Arc<[u8]>)>,
    writer: AbortHandle,
}

/// Mensajes de control hacia un invitado. El Pong se arma al escribirlo para que su
/// hora de salida sea la real (aunque haya esperado detras de audio).
enum Out {
    Bytes(Vec<u8>),
    Pong { t0: i64, rx: i64 },
}

impl Host {
    /// Abre la sala `room` (con su contraseña) en internet y empieza a aceptar invitados.
    /// Si algo falla llega `Event::Ended`; cuando esta lista, `Event::Status`.
    pub fn start(room: &str, password: &str, name: &str, tx: UnboundedSender<Cmd>, id: u32) -> Host {
        let inner = Inner::new(password, name, tx, id);
        let (inner2, room, password) = (inner.clone(), room.to_string(), password.to_string());
        let accept = tokio::task::spawn_local(async move {
            let ep = match net::host_endpoint(&room, &password).await {
                Ok(ep) => ep,
                Err(e) => return inner2.event(Event::Ended(e)),
            };
            *inner2.endpoint.borrow_mut() = Some(ep.clone());
            log::info!("jam: sala lista ({})", ep.id());
            inner2.event(Event::Status("Jam listo. Comparte el nombre y la contraseña con quien quieras invitar.".into()));
            while let Some(conn) = net::accept(&ep).await {
                match conn {
                    Ok((rd, wr, keep)) => inner2.add_peer(rd, wr, keep),
                    Err(e) => log::info!("jam: conexión fallida: {e}"),
                }
            }
        })
        .abort_handle();
        Host { inner, accept }
    }

    /// Para las pruebas: anfitrion en un puerto TCP local (sin internet).
    #[cfg(test)]
    pub fn start_tcp(listener: tokio::net::TcpListener, password: &str, name: &str, tx: UnboundedSender<Cmd>, id: u32) -> Host {
        let inner = Inner::new(password, name, tx, id);
        let inner2 = inner.clone();
        let accept = tokio::task::spawn_local(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let (rd, wr) = stream.into_split();
                inner2.add_peer(Box::new(rd), Box::new(wr), Box::new(()));
            }
        })
        .abort_handle();
        Host { inner, accept }
    }

    /// Hora del anfitrion en microsegundos.
    fn now(&self) -> i64 {
        micros_since(self.inner.epoch)
    }

    /// Nueva cancion. `data` = None si no se puede repartir.
    pub fn set_track(&self, track: &Track, data: Option<Arc<[u8]>>) {
        let tag = self.inner.tag.get().wrapping_add(1);
        self.inner.tag.set(tag);
        *self.inner.current.borrow_mut() = Some((track.clone(), data.clone()));
        self.inner.state.set(None);
        for p in self.inner.peers.borrow().iter() {
            send_current(&self.inner, p);
        }
    }

    pub fn set_state(&self, playing: bool, pos_secs: f64) {
        if self.inner.current.borrow().is_none() {
            return;
        }
        let s = PlayState { tag: self.inner.tag.get(), playing, pos_ms: (pos_secs.max(0.0) * 1000.0) as u64, at: self.now() };
        self.inner.state.set(Some(s));
        self.inner.broadcast(&Msg::State { s });
    }

    /// Lo que viene despues de la cancion actual.
    pub fn set_queue(&self, upcoming: Vec<Track>) {
        let upcoming: Vec<Track> = upcoming.into_iter().take(QUEUE_PREVIEW).collect();
        if *self.inner.queue.borrow() == upcoming {
            return;
        }
        self.inner.broadcast(&Msg::Queue { tracks: upcoming.clone() });
        *self.inner.queue.borrow_mut() = upcoming;
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.accept.abort();
        // Cada conexion tiene su propia referencia a Inner: se abortan aqui.
        for h in self.inner.conns.borrow_mut().drain(..) {
            h.abort();
        }
        for p in self.inner.peers.borrow_mut().drain(..) {
            p.writer.abort();
        }
        if let Some(ep) = self.inner.endpoint.borrow_mut().take() {
            tokio::task::spawn_local(async move { ep.close().await });
        }
        log::info!("jam: anfitrion cerrado");
    }
}

impl Inner {
    fn new(password: &str, name: &str, tx: UnboundedSender<Cmd>, id: u32) -> Rc<Inner> {
        Rc::new(Inner {
            id,
            epoch: Instant::now(),
            code: password.to_string(),
            name: clean_name(name, "Anfitrión"),
            tx,
            peers: RefCell::new(Vec::new()),
            conns: RefCell::new(Vec::new()),
            next_peer: Cell::new(1),
            tag: Rc::new(Cell::new(0)),
            current: RefCell::new(None),
            state: Cell::new(None),
            queue: RefCell::new(Vec::new()),
            endpoint: RefCell::new(None),
        })
    }

    fn add_peer(self: &Rc<Self>, rd: Rd, wr: Wr, keep: Keep) {
        let h = tokio::task::spawn_local(handle_peer(self.clone(), rd, wr, keep)).abort_handle();
        let mut conns = self.conns.borrow_mut();
        conns.retain(|c| !c.is_finished());
        conns.push(h);
    }

    fn event(&self, e: Event) {
        let _ = self.tx.send(Cmd::Jam(self.id, e));
    }

    fn broadcast(&self, m: &Msg) {
        let bytes = protocol::encode(m);
        for p in self.peers.borrow().iter() {
            let _ = p.ctrl.send(Out::Bytes(bytes.clone()));
        }
    }

    fn peer_names(&self) -> Vec<String> {
        self.peers.borrow().iter().map(|p| p.name.clone()).collect()
    }

    fn peers_changed(&self) {
        let names = self.peer_names();
        let mut all = vec![format!("{} (anfitrión)", self.name)];
        all.extend(names.iter().cloned());
        self.broadcast(&Msg::Peers { names: all });
        self.event(Event::Peers(names));
    }
}

/// Manda al invitado la cancion actual con su audio y el ultimo estado.
fn send_current(inner: &Inner, p: &Peer) {
    let tag = inner.tag.get();
    if let Some((track, data)) = inner.current.borrow().as_ref() {
        let size = data.as_ref().map(|d| d.len() as u64).unwrap_or(0);
        let _ = p.ctrl.send(Out::Bytes(protocol::encode(&Msg::Track { tag, track: track.clone(), size })));
        if let Some(d) = data {
            let _ = p.audio.send((tag, d.clone()));
        }
    }
    if let Some(s) = inner.state.get() {
        let _ = p.ctrl.send(Out::Bytes(protocol::encode(&Msg::State { s })));
    }
}

async fn reject(mut wr: Wr, reason: &str) {
    let _ = protocol::write_msg(&mut wr, &Msg::Reject { reason: reason.into() }).await;
}

async fn handle_peer(inner: Rc<Inner>, mut rd: Rd, wr: Wr, _keep: Keep) {
    // Lo primero tiene que ser un Hello valido, y pronto.
    let hello = tokio::time::timeout(Duration::from_secs(5), protocol::read_frame(&mut rd)).await;
    let name = match hello {
        Ok(Ok(Frame::Msg(Msg::Hello { v, code, name }))) => {
            if v != VERSION {
                return reject(wr, "Versión distinta de la app: actualicen los dos a la última.").await;
            }
            if code != inner.code {
                return reject(wr, "Contraseña incorrecta.").await;
            }
            clean_name(&name, "Invitado")
        }
        _ => return,
    };
    if inner.peers.borrow().len() >= MAX_GUESTS {
        return reject(wr, "El Jam está lleno.").await;
    }

    let (ctrl_tx, ctrl_rx) = unbounded_channel();
    let (audio_tx, audio_rx) = unbounded_channel();
    let pid = inner.next_peer.get();
    inner.next_peer.set(pid + 1);
    let writer = tokio::task::spawn_local(writer(wr, ctrl_rx, audio_rx, inner.tag.clone(), inner.epoch)).abort_handle();
    let peer = Peer { id: pid, name: name.clone(), ctrl: ctrl_tx, audio: audio_tx, writer };
    let _ = peer.ctrl.send(Out::Bytes(protocol::encode(&Msg::Welcome { host: inner.name.clone() })));
    send_current(&inner, &peer);
    {
        let q = inner.queue.borrow().clone();
        let _ = peer.ctrl.send(Out::Bytes(protocol::encode(&Msg::Queue { tracks: q })));
    }
    inner.peers.borrow_mut().push(peer);
    inner.peers_changed();
    inner.event(Event::Status(format!("{name} se unió al Jam")));

    let reason = read_loop(&inner, &mut rd, pid, &name).await;
    log::info!("jam: {name} salio: {reason}");
    let removed = {
        let mut peers = inner.peers.borrow_mut();
        peers.iter().position(|p| p.id == pid).map(|i| peers.remove(i))
    };
    if let Some(p) = removed {
        p.writer.abort();
        inner.peers_changed();
        inner.event(Event::Status(format!("{name} salió del Jam")));
    }
}

async fn read_loop(inner: &Inner, rd: &mut Rd, pid: u64, name: &str) -> String {
    loop {
        let frame = match protocol::read_frame(rd).await {
            Ok(f) => f,
            Err(e) => return e.to_string(),
        };
        match frame {
            Frame::Msg(Msg::Ping { t0 }) => {
                let rx = micros_since(inner.epoch);
                let peers = inner.peers.borrow();
                if let Some(p) = peers.iter().find(|p| p.id == pid) {
                    let _ = p.ctrl.send(Out::Pong { t0, rx });
                }
            }
            Frame::Msg(Msg::Req { mut r }) => {
                match &mut r {
                    // Solo canciones de YouTube con un id valido (ver is_youtube_id).
                    Request::Add { track } if !protocol::is_youtube_id(&track.id) => continue,
                    Request::Add { track } => protocol::sanitize(track),
                    Request::Seek { secs } if !secs.is_finite() => continue,
                    _ => {}
                }
                inner.event(Event::Request(name.to_string(), r));
            }
            Frame::Msg(_) => {}
            // Un invitado no manda audio.
            Frame::Audio { .. } => return "trama inesperada".into(),
        }
    }
}

async fn write_out(wr: &mut Wr, o: Out, epoch: Instant) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;
    match o {
        Out::Bytes(b) => wr.write_all(&b).await,
        Out::Pong { t0, rx } => protocol::write_msg(wr, &Msg::Pong { t0, rx, tx: micros_since(epoch) }).await,
    }
}

/// Escribe al invitado: primero el control (para que el reloj y el estado no esperen
/// detras del audio) y luego el audio en pedazos, abandonando el de canciones viejas.
async fn writer(
    mut wr: Wr,
    mut ctrl: UnboundedReceiver<Out>,
    mut audio: UnboundedReceiver<(u32, Arc<[u8]>)>,
    tag: Rc<Cell<u32>>,
    epoch: Instant,
) {
    let mut job: Option<(u32, Arc<[u8]>, usize)> = None;
    loop {
        while let Ok(o) = ctrl.try_recv() {
            if write_out(&mut wr, o, epoch).await.is_err() {
                return;
            }
        }
        while let Ok((t, d)) = audio.try_recv() {
            job = Some((t, d, 0));
        }
        if let Some((t, data, off)) = job.as_mut() {
            if *t != tag.get() {
                job = None;
                continue;
            }
            let end = (*off + CHUNK).min(data.len());
            if protocol::write_audio(&mut wr, *t, &data[*off..end]).await.is_err() {
                return;
            }
            *off = end;
            if end >= data.len() {
                job = None;
            }
            continue;
        }
        tokio::select! {
            o = ctrl.recv() => match o {
                Some(o) => if write_out(&mut wr, o, epoch).await.is_err() { return },
                None => return,
            },
            j = audio.recv() => match j {
                Some((t, d)) => job = Some((t, d, 0)),
                None => return,
            },
        }
    }
}
