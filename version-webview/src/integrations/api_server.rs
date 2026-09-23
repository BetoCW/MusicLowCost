//! API HTTP local para controlar la app desde otros programas (port de plugins/api-server).
//! Solo escucha en 127.0.0.1 y usa las mismas rutas que Pear (/api/v1/...).

use crate::player;
use crate::state::AppState;
use serde_json::json;
use tauri::{AppHandle, Manager, Runtime};
use tiny_http::{Header, Method, Response, Server};

pub fn spawn<R: Runtime>(app: AppHandle<R>, port: u16) {
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
            let json_header = Header::from_bytes("Content-Type", "application/json").unwrap();
            for req in server.incoming_requests() {
                // Si trae Origin viene de una pagina web: se rechaza para que ningun sitio
                // pueda controlar el reproductor a traves del navegador.
                if req.headers().iter().any(|h| h.field.equiv("Origin")) {
                    let _ = req.respond(Response::empty(403));
                    continue;
                }
                // Si se desactivo en ajustes, el hilo sigue vivo hasta reiniciar pero no responde.
                if !app.state::<AppState>().config().api_server.enabled {
                    let _ = req.respond(Response::empty(503));
                    continue;
                }
                let path = req.url().split('?').next().unwrap_or("").to_string();
                let response = match (req.method(), path.as_str()) {
                    (Method::Get, "/api/v1/song") => {
                        let state = app.state::<AppState>();
                        let pb = state.playback.lock().unwrap();
                        let body = json!({
                            "song": pb.song,
                            "isPaused": pb.paused,
                            "elapsedSeconds": pb.position(),
                        });
                        Response::from_string(body.to_string()).with_header(json_header.clone())
                    }
                    (Method::Post, p) => {
                        let cmd = match p {
                            "/api/v1/play" => Some("play"),
                            "/api/v1/pause" => Some("pause"),
                            "/api/v1/toggle-play" => Some("toggle"),
                            "/api/v1/next" => Some("next"),
                            "/api/v1/previous" => Some("previous"),
                            "/api/v1/like" => Some("like"),
                            "/api/v1/dislike" => Some("dislike"),
                            _ => None,
                        };
                        match cmd {
                            Some(c) => {
                                player::simple(&app, c);
                                Response::from_string("").with_status_code(204)
                            }
                            None => Response::from_string("not found").with_status_code(404),
                        }
                    }
                    _ => Response::from_string("not found").with_status_code(404),
                };
                let _ = req.respond(response);
            }
        })
        .expect("hilo api");
}
