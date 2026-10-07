//! Mediciones de rendimiento. Solo hace algo con `YIR_BENCH_FILE=<archivo>`: escribe eventos
//! en el mismo archivo donde el renderer parchado anota cada frame
//! (`vendor/i-slint-backend-winit/renderer/sw.rs`). Con `YIR_BENCH=<escenario>` ademas maneja
//! la app sola y la cierra al terminar:
//!
//! - `start`: espera a que Inicio tenga canciones y portadas, y queda 10 s en reposo.
//! - `scroll`: abre la Cola con 2000 canciones de prueba y la recorre con la rueda 5 s.
//! - `play`: reproduce la primera cancion de Inicio y mide 15 s de reproduccion.
//! - `latency`: reproduce la cancion `YIR_BENCH_INDEX` de Inicio y sale 2 s despues de que
//!   suena; los tiempos de clic a sonido son los eventos `lat_*`.
//! - `seek`: reproduce y prueba los botones de +10 / -10 s (eventos `seek_pos_ms`).
//!
//! `YIR_BENCH_SIZE=1920x1040` fija el tamano de la ventana. `scripts/bench.ps1` corre todo y resume los numeros.
//! Formato de cada linea: `e <unix us> <nombre> <valor>` (`f ...` son frames).

use crate::{AppState, AppWindow, TrackRow};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::io::Write;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static FILE: OnceLock<Option<Mutex<std::fs::File>>> = OnceLock::new();
static INVOKES: AtomicU64 = AtomicU64::new(0);
static ROW_SETS: AtomicU64 = AtomicU64::new(0);

fn file() -> Option<&'static Mutex<std::fs::File>> {
    FILE.get_or_init(|| {
        let path = std::env::var_os("YIR_BENCH_FILE")?;
        let f = std::fs::OpenOptions::new().create(true).append(true).open(path).ok()?;
        Some(Mutex::new(f))
    })
    .as_ref()
}

pub fn enabled() -> bool {
    file().is_some()
}

pub fn event(name: &str, value: u64) {
    let Some(f) = file() else { return };
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_micros()).unwrap_or(0);
    if let Ok(mut f) = f.lock() {
        // Una sola escritura por linea: el renderer escribe en el mismo archivo.
        let _ = f.write_all(format!("e {now} {name} {value}\n").as_bytes());
    }
}

/// Un `invoke_from_event_loop` del hilo de trabajo hacia la interfaz.
pub fn count_invoke() {
    INVOKES.fetch_add(1, Ordering::Relaxed);
}

/// Un `set_row_data` (cada uno marca la lista para repintar).
pub fn count_row_set() {
    ROW_SETS.fetch_add(1, Ordering::Relaxed);
}

#[derive(Clone, Copy, PartialEq)]
enum Step {
    WaitHome,
    WaitCovers,
    Idle,
    WaitPage,
    Settle,
    Scroll,
    WaitPlaying,
    Playing,
    Done,
    Finished,
}

/// Arranca el escenario de `YIR_BENCH` (si lo hay). Se llama despues de `ui.show()`.
pub fn start(ui: &AppWindow) {
    if !enabled() {
        return;
    }
    // Contadores por segundo.
    let counters = slint::Timer::default();
    counters.start(slint::TimerMode::Repeated, Duration::from_secs(1), || {
        event("invokes_1s", INVOKES.swap(0, Ordering::Relaxed));
        event("row_sets_1s", ROW_SETS.swap(0, Ordering::Relaxed));
    });
    std::mem::forget(counters);

    let Ok(scenario) = std::env::var("YIR_BENCH") else { return };
    let weak = ui.as_weak();
    let mut step = Step::WaitHome;
    let mut since = Instant::now();
    let mut last_wheel = Instant::now();
    // Tamano fisico de la ventana, p. ej. "1920x1040" (pantalla completa sin la barra de tareas).
    let mut size = std::env::var("YIR_BENCH_SIZE").ok().and_then(|s| {
        let (w, h) = s.split_once('x')?;
        Some(slint::PhysicalSize::new(w.parse().ok()?, h.parse().ok()?))
    });
    let begun = Instant::now();
    let mut seeks = 0usize;
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(2), move || {
        let Some(ui) = weak.upgrade() else { return };
        let s = ui.global::<AppState>();
        if let Some(sz) = size.take() {
            ui.window().set_position(slint::PhysicalPosition::new(0, 0));
            ui.window().set_size(sz);
        }
        let next = match step {
            Step::WaitHome if s.get_home_tracks().row_count() > 0 => {
                event("home_rows", s.get_home_tracks().row_count() as u64);
                Step::WaitCovers
            }
            Step::WaitHome if begun.elapsed() > Duration::from_secs(30) => {
                event("timeout_home", 0);
                Step::Done
            }
            Step::WaitCovers => {
                let m = s.get_home_tracks();
                let missing = (0..m.row_count())
                    .filter(|&i| m.row_data(i).is_some_and(|r| r.cover.size().width == 0))
                    .count();
                if missing == 0 || since.elapsed() > Duration::from_secs(20) {
                    event("home_covers_missing", missing as u64);
                    match scenario.as_str() {
                        "scroll" => {
                            event("click_queue", 0);
                            s.invoke_navigate("queue".into());
                            Step::WaitPage
                        }
                        "play" | "latency" | "seek" => {
                            // `latency` usa otra cancion en cada corrida (YIR_BENCH_INDEX).
                            let rows = s.get_home_tracks().row_count();
                            let i = std::env::var("YIR_BENCH_INDEX").ok().and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
                            event("play_cmd", (i % rows) as u64);
                            s.invoke_play_track("home".into(), (i % rows) as i32);
                            Step::WaitPlaying
                        }
                        _ => {
                            event("idle_begin", 0);
                            Step::Idle
                        }
                    }
                } else {
                    step
                }
            }
            Step::Idle if since.elapsed() > Duration::from_secs(10) => {
                event("idle_end", 0);
                Step::Done
            }
            Step::WaitPage if s.get_page() == "queue" => {
                event("page_changed", 0);
                s.set_queue(synthetic_rows(2000));
                Step::Settle
            }
            Step::Settle if since.elapsed() > Duration::from_millis(800) => {
                event("scroll_begin", 0);
                Step::Scroll
            }
            Step::Scroll => {
                if since.elapsed() > Duration::from_secs(5) {
                    event("scroll_end", 0);
                    Step::Done
                } else {
                    // Una muesca de rueda cada ~16 ms, sobre el centro de la lista.
                    if last_wheel.elapsed() >= Duration::from_millis(16) {
                        last_wheel = Instant::now();
                        let size = ui.window().size().to_logical(ui.window().scale_factor());
                        let position = slint::LogicalPosition::new(210.0 + (size.width - 210.0) / 2.0, size.height / 2.0);
                        let w = ui.window();
                        w.dispatch_event(slint::platform::WindowEvent::PointerMoved { position });
                        w.dispatch_event(slint::platform::WindowEvent::PointerScrolled { position, delta_x: 0.0, delta_y: -60.0 });
                    }
                    step
                }
            }
            Step::WaitPlaying if s.get_playing() && s.get_position() > 0.0 => {
                event("playing", 0);
                Step::Playing
            }
            Step::WaitPlaying if since.elapsed() > Duration::from_secs(40) => {
                event("timeout_play", 0);
                Step::Done
            }
            // `seek`: +10, +10 y -10 s, cada 2 s; se anota la posicion (ms) antes de cada salto y al final.
            Step::Playing if scenario == "seek" => {
                let at = [4u64, 6, 8, 10];
                if seeks < at.len() && since.elapsed() >= Duration::from_secs(at[seeks]) {
                    event("seek_pos_ms", (s.get_position() * 1000.0) as u64);
                    if seeks < 3 {
                        s.invoke_seek_by(if seeks == 2 { -10.0 } else { 10.0 });
                    }
                    seeks += 1;
                }
                if seeks == at.len() { Step::Done } else { step }
            }
            Step::Playing if since.elapsed() > Duration::from_secs(if scenario == "latency" { 2 } else { 15 }) => {
                event("play_end", 0);
                Step::Done
            }
            Step::Done => {
                event("bench_end", 0);
                let _ = slint::quit_event_loop();
                Step::Finished
            }
            _ => step,
        };
        if next != step {
            step = next;
            since = Instant::now();
        }
    });
    std::mem::forget(timer);
}

/// Canciones de prueba con una portada de 60 px compartida.
fn synthetic_rows(n: usize) -> ModelRc<TrackRow> {
    let mut px = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(60, 60);
    for (i, p) in px.make_mut_slice().iter_mut().enumerate() {
        let (x, y) = ((i % 60) as u8, (i / 60) as u8);
        *p = slint::Rgba8Pixel { r: x * 4, g: y * 4, b: 160, a: 255 };
    }
    let cover = slint::Image::from_rgba8(px);
    let rows: Vec<TrackRow> = (0..n)
        .map(|i| TrackRow {
            id: format!("bench{i}").into(),
            title: format!("Cancion de prueba numero {i} con un titulo largo").into(),
            artist: "Artista de prueba".into(),
            album: "Album de prueba".into(),
            duration: "3:45".into(),
            current: false,
            cover: cover.clone(),
        })
        .collect();
    ModelRc::from(Rc::new(VecModel::from(rows)))
}

/// Al soltarse anota cuanto duro, en microsegundos.
pub struct Timed(pub &'static str, pub Instant);

impl Drop for Timed {
    fn drop(&mut self) {
        event(self.0, self.1.elapsed().as_micros() as u64);
    }
}
