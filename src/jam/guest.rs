//! Invitado: se conecta al anfitrion, sincroniza el reloj, recibe el audio de cada
//! cancion y le pasa al backend que tocar. Lo que el usuario pida se manda al anfitrion.

use super::net::{self, Keep, Rd, Wr};
use super::protocol::{self, Frame, MAX_AUDIO, Msg, Request, VERSION};
use super::{Clock, Event};
use crate::backend::Cmd;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::AbortHandle;

pub struct Guest {
    ctrl: UnboundedSender<Out>,
    pub clock: Rc<RefCell<Clock>>,
    task: AbortHandle,
}

enum Out {
    Bytes(Vec<u8>),
    /// Se le pone la hora al escribirlo.
    Ping,
}

impl Guest {
    /// Empieza a conectarse a la sala en segundo plano; el resultado llega como `Event`.
    pub fn start(room: String, password: String, name: String, tx: UnboundedSender<Cmd>, id: u32) -> Guest {
        Self::start_with(Dial::Room(room), password, name, tx, id)
    }

    /// Para las pruebas: se conecta por TCP local (sin internet).
    #[cfg(test)]
    pub fn start_tcp(addr: String, password: String, name: String, tx: UnboundedSender<Cmd>, id: u32) -> Guest {
        Self::start_with(Dial::Tcp(addr), password, name, tx, id)
    }

    fn start_with(dial: Dial, code: String, name: String, tx: UnboundedSender<Cmd>, id: u32) -> Guest {
        let (ctrl, rx) = unbounded_channel();
        let clock = Rc::new(RefCell::new(Clock::new()));
        let (clock2, ctrl2) = (clock.clone(), ctrl.clone());
        let task = tokio::task::spawn_local(async move {
            let reason = match session(dial, &code, &name, &tx, id, rx, ctrl2, clock2).await {
                Ok(()) => "El anfitrión terminó el Jam.".to_string(),
                Err(e) => e,
            };
            let _ = tx.send(Cmd::Jam(id, Event::Ended(reason)));
        })
        .abort_handle();
        Guest { ctrl, clock, task }
    }

    pub fn request(&self, r: Request) {
        let _ = self.ctrl.send(Out::Bytes(protocol::encode(&Msg::Req { r })));
    }
}

impl Drop for Guest {
    fn drop(&mut self) {
        // Cierra la conexion (y con ella el escritor y el reloj).
        self.task.abort();
    }
}

enum Dial {
    Room(String),
    #[cfg(test)]
    Tcp(String),
}

async fn dial(d: &Dial, code: &str) -> Result<(Rd, Wr, Keep), String> {
    match d {
        Dial::Room(room) => net::connect(room, code).await,
        #[cfg(test)]
        Dial::Tcp(addr) => {
            let s = tokio::net::TcpStream::connect(addr).await.map_err(|e| e.to_string())?;
            let (rd, wr) = s.into_split();
            Ok((Box::new(rd), Box::new(wr), Box::new(())))
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn session(
    d: Dial,
    code: &str,
    name: &str,
    tx: &UnboundedSender<Cmd>,
    id: u32,
    ctrl_rx: UnboundedReceiver<Out>,
    ctrl: UnboundedSender<Out>,
    clock: Rc<RefCell<Clock>>,
) -> Result<(), String> {
    let event = |e: Event| {
        let _ = tx.send(Cmd::Jam(id, e));
    };
    event(Event::Status("Buscando el Jam…".into()));
    let (mut rd, mut wr, _keep) = dial(&d, code).await?;
    protocol::write_msg(&mut wr, &Msg::Hello { v: VERSION, code: code.to_string(), name: name.to_string() })
        .await
        .map_err(|e| e.to_string())?;
    match tokio::time::timeout(Duration::from_secs(15), protocol::read_frame(&mut rd)).await {
        Ok(Ok(Frame::Msg(Msg::Welcome { host }))) => event(Event::Joined(host)),
        Ok(Ok(Frame::Msg(Msg::Reject { reason }))) => return Err(reason),
        Ok(Err(e)) => return Err(format!("El anfitrión cerró la conexión: {e}")),
        // Por internet el primer mensaje puede tardar mas (relay).
        _ => return Err("El anfitrión no respondió.".into()),
    }

    let writer = tokio::task::spawn_local(writer(wr, ctrl_rx, clock.clone())).abort_handle();
    let pinger = tokio::task::spawn_local(async move {
        // Rafaga al principio para sincronizar rapido; despues, de vez en cuando.
        for i in 0u32.. {
            if ctrl.send(Out::Ping).is_err() {
                return;
            }
            let wait = if i < 6 { 250 } else { 3000 };
            tokio::time::sleep(Duration::from_millis(wait)).await;
        }
    })
    .abort_handle();
    let r = read_loop(&mut rd, &event, &clock).await;
    writer.abort();
    pinger.abort();
    r
}

async fn read_loop(rd: &mut Rd, event: &impl Fn(Event), clock: &RefCell<Clock>) -> Result<(), String> {
    // Audio de la cancion actual mientras llega: (tag, bytes, tamano total).
    let mut incoming: Option<(u32, Vec<u8>, usize)> = None;
    loop {
        let frame = match protocol::read_frame(rd).await {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(format!("Se perdió la conexión con el anfitrión: {e}")),
        };
        match frame {
            Frame::Msg(Msg::Pong { t0, rx, tx }) => {
                let mut c = clock.borrow_mut();
                let t3 = c.now_us();
                c.add_sample(t0, rx, tx, t3);
            }
            Frame::Msg(Msg::Track { tag, mut track, size }) => {
                protocol::sanitize(&mut track);
                if size > MAX_AUDIO {
                    return Err("El anfitrión mandó una canción demasiado grande.".into());
                }
                incoming = (size > 0).then(|| (tag, Vec::with_capacity(size as usize), size as usize));
                event(Event::Track { tag, track, shared: size > 0 });
            }
            Frame::Audio { tag, len } => match incoming.as_mut() {
                Some((t, buf, size)) if *t == tag && buf.len() + len <= *size => {
                    protocol::read_into(rd, buf, len).await.map_err(|e| e.to_string())?;
                    if buf.len() == *size {
                        let (tag, buf, _) = incoming.take().unwrap();
                        event(Event::Audio { tag, data: Arc::from(buf) });
                    }
                }
                _ => protocol::skip(rd, len).await.map_err(|e| e.to_string())?,
            },
            Frame::Msg(Msg::State { s }) => event(Event::State(s)),
            Frame::Msg(Msg::Queue { tracks }) => event(Event::Queue(tracks)),
            Frame::Msg(Msg::Peers { names }) => event(Event::Peers(names)),
            Frame::Msg(_) => {}
        }
    }
}

async fn writer(mut wr: Wr, mut rx: UnboundedReceiver<Out>, clock: Rc<RefCell<Clock>>) {
    use tokio::io::AsyncWriteExt;
    while let Some(o) = rx.recv().await {
        let r = match o {
            Out::Bytes(b) => wr.write_all(&b).await,
            Out::Ping => {
                let t0 = clock.borrow().now_us();
                protocol::write_msg(&mut wr, &Msg::Ping { t0 }).await
            }
        };
        if r.is_err() {
            return;
        }
    }
}
