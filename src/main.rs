// En release no abrir consola.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

slint::include_modules!();

mod audio;
mod backend;
mod config;
mod desktop;
mod images;
mod integrations;
mod jam;
mod logger;
#[cfg(windows)]
mod login;
mod model;
mod spotify;
mod stream;
mod ui;
mod util;

use backend::Cmd;
use slint::ComponentHandle;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const SINGLE_INSTANCE_PORT: u16 = 26540;

fn main() {
    // Software renderer: sin OpenGL/DirectX, la ventana entera gasta ~6 MB.
    // SAFETY: todavia no hay otros hilos.
    unsafe { std::env::set_var("SLINT_BACKEND", "winit-software") };

    // Proceso auxiliar de inicio de sesion (ver login.rs).
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--login") {
        if let Some(out) = args.get(2) {

            #[cfg(windows)]
            let center = match (args.get(3).and_then(|x| x.parse().ok()), args.get(4).and_then(|y| y.parse().ok())) {
                (Some(x), Some(y)) => Some((x, y)),
                _ => None,
            };
            login::run(std::path::PathBuf::from(out), center);
        }
        return;
    }

    // Una sola instancia: si ya hay una abierta, se le pide que se muestre y se sale.
    let port = if std::env::var_os("YIR_DATA_DIR").is_some() { 0 } else { SINGLE_INSTANCE_PORT };
    let instance = match std::net::TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(_) => {
            let _ = std::net::TcpStream::connect(("127.0.0.1", SINGLE_INSTANCE_PORT));
            return;
        }
    };

    let dir = config::data_dir();
    let _ = std::fs::create_dir_all(&dir);
    logger::init(&dir);
    let cfg = config::load(&dir);
    log::info!("iniciando; datos en {}", dir.display());

    let fx = audio::Effects::new(cfg.eq_enabled, cfg.eq, cfg.skip_silence);
    let engine = match audio::Engine::start(fx) {
        Ok(e) => e,
        Err(e) => {
            log::error!("sin salida de audio: {e}");
            return;
        }
    };

    let ui = AppWindow::new().expect("no se pudo crear la ventana");
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Cmd>();
    let visible = Arc::new(AtomicBool::new(true));
    wire_callbacks(&ui, &tx);

    // Hilo de trabajo (red, cola, audio, integraciones).
    {
        let (ui_bridge, cfg, dir, engine, tx, visible) =
            (ui::Ui::new(ui.as_weak()), cfg.clone(), dir.clone(), engine.clone(), tx.clone(), visible.clone());
        std::thread::Builder::new()
            .name("backend".into())
            .spawn(move || backend::run(ui_bridge, cfg, dir, engine, tx, rx, visible))
            .expect("hilo backend");
    }

    // La X cierra la app (para esconderla sin salir: atajo o menu de la bandeja).
    {
        let (tx, visible) = (tx.clone(), visible.clone());
        ui.window().on_close_requested(move || {
            visible.store(false, Ordering::Relaxed);
            quit(&tx);
            slint::CloseRequestResponse::HideWindow
        });
    }

    ui.show().expect("no se pudo mostrar la ventana");

    let hwnd = window_hwnd(&ui);
    let toggle = {
        let (weak, visible) = (ui.as_weak(), visible.clone());
        move || {
            let Some(u) = weak.upgrade() else { return };
            if visible.load(Ordering::Relaxed) {
                let _ = u.hide();
                visible.store(false, Ordering::Relaxed);
            } else {
                let _ = u.show();
                visible.store(true, Ordering::Relaxed);
            }
        }
    };
    // Otra instancia que intenta abrirse -> mostrar esta ventana.
    {
        let (weak, visible) = (ui.as_weak(), visible.clone());
        std::thread::Builder::new()
            .name("single-instance".into())
            .stack_size(64 * 1024)
            .spawn(move || {
                for _ in instance.incoming() {
                    let (weak, visible) = (weak.clone(), visible.clone());
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(u) = weak.upgrade() {
                            let _ = u.show();
                            visible.store(true, Ordering::Relaxed);
                        }
                    });
                }
            })
            .expect("hilo instancia");
    }

    let quit_tx = tx.clone();
    desktop::init(tx.clone(), hwnd, toggle, move || quit(&quit_tx));
    desktop::apply_shortcuts(&cfg);
    #[cfg(windows)]
    desktop::watch_repaint(ui.as_weak(), hwnd);

    slint::run_event_loop_until_quit().expect("error en el bucle de eventos");
    // Hay hilos que nunca terminan (salida de audio, backend, API local): se termina el
    // proceso aqui en vez de esperar a que se suelte todo.
    log::info!("saliendo");
    std::process::exit(0);
}

fn quit(tx: &tokio::sync::mpsc::UnboundedSender<Cmd>) {
    let _ = tx.send(Cmd::SaveSession);
    // Pequena espera para que se guarde la sesion.
    slint::Timer::single_shot(std::time::Duration::from_millis(300), || {
        let _ = slint::quit_event_loop();
    });
}

fn window_hwnd(ui: &AppWindow) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = ui.window().window_handle();
    match handle.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
}

fn wire_callbacks(ui: &AppWindow, tx: &tokio::sync::mpsc::UnboundedSender<Cmd>) {
    let s = ui.global::<AppState>();
    macro_rules! send {
        ($tx:expr, $cmd:expr) => {{
            let _ = $tx.send($cmd);
        }};
    }
    let t = tx.clone();
    s.on_navigate(move |p| send!(t, Cmd::Navigate(p.to_string())));
    let t = tx.clone();
    s.on_search(move |q| send!(t, Cmd::Search(q.to_string())));
    let t = tx.clone();
    s.on_play_track(move |l, i| send!(t, Cmd::PlayTrack(l.to_string(), i.max(0) as usize)));
    let t = tx.clone();
    s.on_enqueue(move |l, i| send!(t, Cmd::Enqueue(l.to_string(), i.max(0) as usize)));
    let t = tx.clone();
    s.on_open_card(move |k, id| send!(t, Cmd::OpenCard(k.to_string(), id.to_string())));
    let t = tx.clone();
    s.on_open_artist_of_current(move || send!(t, Cmd::OpenArtistOfCurrent));
    let t = tx.clone();
    s.on_play_detail(move |sh| send!(t, Cmd::PlayDetail(sh)));
    let t = tx.clone();
    s.on_toggle_play(move || send!(t, Cmd::TogglePlay));
    let t = tx.clone();
    s.on_next(move || send!(t, Cmd::Next));
    let t = tx.clone();
    s.on_previous(move || send!(t, Cmd::Previous));
    let t = tx.clone();
    s.on_seek(move |p| send!(t, Cmd::Seek(p as f64)));
    let t = tx.clone();
    s.on_set_volume(move |v| send!(t, Cmd::SetVolume(v)));
    let t = tx.clone();
    s.on_volume_wheel(move |d| send!(t, Cmd::VolumeWheel(d)));
    let t = tx.clone();
    s.on_toggle_shuffle(move || send!(t, Cmd::ToggleShuffle));
    let t = tx.clone();
    s.on_cycle_repeat(move || send!(t, Cmd::CycleRepeat));
    let t = tx.clone();
    s.on_queue_remove(move |i| send!(t, Cmd::QueueRemove(i.max(0) as usize)));
    let t = tx.clone();
    s.on_queue_clear(move || send!(t, Cmd::QueueClear));
    let t = tx.clone();
    let weak = ui.as_weak();
    s.on_save_settings(move || {
        if let Some(u) = weak.upgrade() {
            send!(t, Cmd::SaveSettings(Box::new(backend::read_cfg_from_ui(&u))));
        }
    });
    let t = tx.clone();
    s.on_login(move |b| send!(t, Cmd::Login(b.to_string())));
    let t = tx.clone();
    s.on_logout(move || send!(t, Cmd::Logout));
    let t = tx.clone();
    s.on_lastfm_connect(move || send!(t, Cmd::LastfmConnect));
    let t = tx.clone();
    s.on_set_source(move |src| send!(t, Cmd::SetSource(src.to_string())));
    let t = tx.clone();
    s.on_sp_login(move || send!(t, Cmd::SpotifyLogin));
    let t = tx.clone();
    s.on_sp_logout(move || send!(t, Cmd::SpotifyLogout));
    let t = tx.clone();
    s.on_jam_host(move |name, port| send!(t, Cmd::JamHost(name.to_string(), port.clamp(1024, 65535) as u16)));
    let t = tx.clone();
    s.on_jam_join(move |name, addr, code| send!(t, Cmd::JamJoin(name.to_string(), addr.to_string(), code.to_string())));
    let t = tx.clone();
    s.on_jam_leave(move || send!(t, Cmd::JamLeave));
}
