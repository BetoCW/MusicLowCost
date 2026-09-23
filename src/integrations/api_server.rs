//! API HTTP local (port de plugins/api-server de Pear, mismas rutas /api/v1/...).
//! Solo escucha en 127.0.0.1 y rechaza peticiones hechas desde paginas web.

use crate::backend::Cmd;
use std::sync::{Arc, Mutex};
use tiny_http::{Header, Method, Response, Server};
use tokio::sync::mpsc::UnboundedSender;

pub type Snapshot = Arc<Mutex<serde_json::Value>>;

pub fn spawn(port: u16, tx: UnboundedSender<Cmd>, snapshot: Snapshot, enabled: Arc<std::sync::atomic::AtomicBool>) {
    let server = match Server::http(("127.0.0.1", port)) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("API local: no se pudo abrir el puerto {port}: {e}");
            return;
        }
    };
    log::info!("API local en http://127.0.0.1:{port}/api/v1/song");
    std::thread::Builder::new()
        .name("api-server".into())
        .stack_size(256 * 1024)
        .spawn(move || {
            let json = Header::from_bytes("Content-Type", "application/json").unwrap();
            for req in server.incoming_requests() {
                if req.headers().iter().any(|h| h.field.equiv("Origin")) {
                    let _ = req.respond(Response::empty(403));
                    continue;
                }
                if !enabled.load(std::sync::atomic::Ordering::Relaxed) {
                    let _ = req.respond(Response::empty(503));
                    continue;
                }
                let path = req.url().split('?').next().unwrap_or("").to_string();
                let resp = match (req.method(), path.as_str()) {
                    (Method::Get, "/api/v1/song") => {
                        let body = snapshot.lock().unwrap().to_string();
                        Response::from_string(body).with_header(json.clone())
                    }
                    (Method::Post, p) => {
                        let cmd = match p {
                            "/api/v1/play" => Some(Cmd::Play),
                            "/api/v1/pause" => Some(Cmd::Pause),
                            "/api/v1/toggle-play" => Some(Cmd::TogglePlay),
                            "/api/v1/next" => Some(Cmd::Next),
                            "/api/v1/previous" => Some(Cmd::Previous),
                            _ => None,
                        };
                        match cmd {
                            Some(c) => {
                                let _ = tx.send(c);
                                Response::from_string("").with_status_code(204)
                            }
                            None => Response::from_string("not found").with_status_code(404),
                        }
                    }
                    _ => Response::from_string("not found").with_status_code(404),
                };
                let _ = req.respond(resp);
            }
        })
        .expect("hilo api");
}
