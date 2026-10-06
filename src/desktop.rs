//! Integracion con Windows, siempre en el hilo de la interfaz:
//! bandeja del sistema, atajos globales y panel multimedia (SMTC / teclas multimedia).

use crate::backend::Cmd;
use crate::config::Config;
use crate::model::Track;
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig, SeekDirection};
use std::cell::RefCell;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

const ICON_PLAYING: &[u8] = include_bytes!("../icons/tray.png");
const ICON_PAUSED: &[u8] = include_bytes!("../icons/tray-paused.png");

struct Desktop {
    tray: Option<TrayIcon>,
    now_item: Option<MenuItem>,
    ids: MenuIds,
    tray_paused: bool,
    hotkeys: Option<GlobalHotKeyManager>,
    registered: Vec<(HotKey, HotAction)>,
    smtc: Option<MediaControls>,
}

#[derive(Default)]
struct MenuIds {
    toggle: String,
    next: String,
    prev: String,
    show: String,
    quit: String,
}

#[derive(Clone, Copy)]
enum HotAction {
    Toggle,
    Next,
    Prev,
    Show,
}

thread_local! {
    static DESKTOP: RefCell<Option<Desktop>> = const { RefCell::new(None) };
}

fn icon(bytes: &[u8]) -> Option<Icon> {
    let img = image::load_from_memory(bytes).ok()?.thumbnail(32, 32).to_rgba8();
    let (w, h) = img.dimensions();
    Icon::from_rgba(img.into_raw(), w, h).ok()
}

/// Crear bandeja, atajos y SMTC. `hwnd` es la ventana principal (para SMTC).
pub fn init(
    tx: UnboundedSender<Cmd>,
    hwnd: Option<isize>,
    toggle_window: impl Fn() + 'static,
    quit: impl Fn() + 'static,
) {
    // Bandeja
    let now_item = MenuItem::new("Nada sonando", false, None);
    let toggle = MenuItem::new("Reproducir / Pausa", true, None);
    let next = MenuItem::new("Siguiente", true, None);
    let prev = MenuItem::new("Anterior", true, None);
    let show = MenuItem::new("Mostrar / Ocultar", true, None);
    let quit_item = MenuItem::new("Salir", true, None);
    let menu = Menu::new();
    let _ = menu.append_items(&[
        &now_item,
        &PredefinedMenuItem::separator(),
        &toggle,
        &next,
        &prev,
        &PredefinedMenuItem::separator(),
        &show,
        &quit_item,
    ]);
    let ids = MenuIds {
        toggle: toggle.id().0.clone(),
        next: next.id().0.clone(),
        prev: prev.id().0.clone(),
        show: show.id().0.clone(),
        quit: quit_item.id().0.clone(),
    };
    let mut builder = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip("YoutubeInRustWeb");
    if let Some(i) = icon(ICON_PAUSED) {
        builder = builder.with_icon(i);
    }
    let tray = match builder.build() {
        Ok(t) => Some(t),
        Err(e) => {
            log::warn!("bandeja: {e}");
            None
        }
    };

    // Panel multimedia de Windows + teclas multimedia
    let smtc = hwnd.and_then(|h| {
        let cfg = PlatformConfig {
            dbus_name: "youtube_in_rust_web",
            display_name: "YoutubeInRustWeb",
            hwnd: Some(h as *mut std::ffi::c_void),
        };
        let mut c = MediaControls::new(cfg).map_err(|e| log::warn!("SMTC: {e:?}")).ok()?;
        let tx = tx.clone();
        c.attach(move |ev| {
            let cmd = match ev {
                MediaControlEvent::Play => Some(Cmd::Play),
                MediaControlEvent::Pause | MediaControlEvent::Stop => Some(Cmd::Pause),
                MediaControlEvent::Toggle => Some(Cmd::TogglePlay),
                MediaControlEvent::Next => Some(Cmd::Next),
                MediaControlEvent::Previous => Some(Cmd::Previous),
                MediaControlEvent::SetPosition(MediaPosition(p)) => Some(Cmd::Seek(p.as_secs_f64())),
                MediaControlEvent::Seek(SeekDirection::Forward) => None,
                _ => None,
            };
            if let Some(c) = cmd {
                let _ = tx.send(c);
            }
        })
        .map_err(|e| log::warn!("SMTC attach: {e:?}"))
        .ok()?;
        Some(c)
    });

    DESKTOP.with(|d| {
        *d.borrow_mut() = Some(Desktop {
            tray,
            now_item: Some(now_item),
            ids,
            tray_paused: true,
            hotkeys: GlobalHotKeyManager::new().map_err(|e| log::warn!("atajos: {e}")).ok(),
            registered: Vec::new(),
            smtc,
        })
    });

    // Sondeo de eventos de bandeja y atajos (winit ya bombea los mensajes de Windows).
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(120), move || {
        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            let id = ev.id.0;
            let (is_toggle, is_next, is_prev, is_show, is_quit) = DESKTOP.with(|d| {
                let d = d.borrow();
                let ids = &d.as_ref().unwrap().ids;
                (id == ids.toggle, id == ids.next, id == ids.prev, id == ids.show, id == ids.quit)
            });
            if is_toggle {
                let _ = tx.send(Cmd::TogglePlay);
            } else if is_next {
                let _ = tx.send(Cmd::Next);
            } else if is_prev {
                let _ = tx.send(Cmd::Previous);
            } else if is_show {
                toggle_window();
            } else if is_quit {
                quit();
            }
        }
        while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                toggle_window();
            }
        }
        while let Ok(ev) = GlobalHotKeyEvent::receiver().try_recv() {
            if ev.state != HotKeyState::Pressed {
                continue;
            }
            let action = DESKTOP.with(|d| {
                d.borrow().as_ref().and_then(|d| d.registered.iter().find(|(h, _)| h.id() == ev.id).map(|(_, a)| *a))
            });
            match action {
                Some(HotAction::Toggle) => {
                    let _ = tx.send(Cmd::TogglePlay);
                }
                Some(HotAction::Next) => {
                    let _ = tx.send(Cmd::Next);
                }
                Some(HotAction::Prev) => {
                    let _ = tx.send(Cmd::Previous);
                }
                Some(HotAction::Show) => toggle_window(),
                None => {}
            }
        }
    });
    std::mem::forget(timer);
}

#[cfg(windows)]
#[link(name = "user32")]
unsafe extern "system" {
    fn GetForegroundWindow() -> isize;
    fn IsIconic(hwnd: isize) -> i32;
    fn IsZoomed(hwnd: isize) -> i32;
    fn IsWindowVisible(hwnd: isize) -> i32;
    fn GetWindowRect(hwnd: isize, rect: *mut [i32; 4]) -> i32;
    fn GetSystemMetrics(index: i32) -> i32;
    fn RedrawWindow(hwnd: isize, rect: *const std::ffi::c_void, rgn: isize, flags: u32) -> i32;
}

/// El renderer por software solo repinta lo que cambio y reutiliza el resto del buffer.
/// Si Windows descarta el contenido de la ventana (un juego en pantalla completa, otra app
/// maximizada encima, cambio de monitores...) la app no se entera y queda con basura.
/// Cuando cambia la ventana en primer plano o su posicion, la resolucion o el estado
/// maximizado, se repinta todo (tambien el marco) durante ~1 s, porque el otro programa
/// tarda en soltar la pantalla. Maximizada ademas se repinta cada ~2 s por si acaso:
/// tapa toda la pantalla y es donde mas se notaba.
#[cfg(windows)]
pub fn watch_repaint(weak: slint::Weak<crate::AppWindow>, hwnd: Option<isize>) {
    let Some(hwnd) = hwnd else { return };
    const SM_XVIRTUALSCREEN: i32 = 76;
    const SM_YVIRTUALSCREEN: i32 = 77;
    const SM_CXVIRTUALSCREEN: i32 = 78;
    const SM_CYVIRTUALSCREEN: i32 = 79;
    const RDW_INVALIDATE: u32 = 0x0001;
    const RDW_ERASE: u32 = 0x0004;
    const RDW_FRAME: u32 = 0x0400;
    // Todo lo que, si cambia, puede haber dejado la ventana sin pintar.
    let snapshot = move || unsafe {
        let fg = GetForegroundWindow();
        let mut fg_rect = [0; 4];
        GetWindowRect(fg, &mut fg_rect);
        let mut own_rect = [0; 4];
        GetWindowRect(hwnd, &mut own_rect);
        let screens = [SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN].map(|m| GetSystemMetrics(m));
        (fg, fg_rect, own_rect, screens, IsZoomed(hwnd) != 0)
    };
    let mut last = snapshot();
    let mut pending = 0u8;
    let mut ticks = 0u32;
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(250), move || {
        ticks = ticks.wrapping_add(1);
        let now = snapshot();
        if now != last {
            last = now;
            pending = 4;
        }
        let maximized = now.4;
        if pending == 0 && !(maximized && ticks % 8 == 0) {
            return;
        }
        pending = pending.saturating_sub(1);
        if unsafe { IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 } {
            return;
        }
        if let Some(ui) = weak.upgrade() {
            use slint::ComponentHandle;
            ui.set_repaint_flip(!ui.get_repaint_flip());
            ui.window().request_redraw();
        }
        unsafe { RedrawWindow(hwnd, std::ptr::null(), 0, RDW_INVALIDATE | RDW_ERASE | RDW_FRAME) };
    });
    std::mem::forget(timer);
}

pub fn apply_shortcuts(cfg: &Config) {
    DESKTOP.with(|d| {
        let mut d = d.borrow_mut();
        let Some(d) = d.as_mut() else { return };
        let Some(mgr) = d.hotkeys.as_ref() else { return };
        let old: Vec<HotKey> = d.registered.drain(..).map(|(h, _)| h).collect();
        let _ = mgr.unregister_all(&old);
        if !cfg.shortcuts {
            return;
        }
        for (keys, action) in [
            (&cfg.sc_play, HotAction::Toggle),
            (&cfg.sc_next, HotAction::Next),
            (&cfg.sc_prev, HotAction::Prev),
            (&cfg.sc_show, HotAction::Show),
        ] {
            if keys.trim().is_empty() {
                continue;
            }
            match keys.parse::<HotKey>() {
                Ok(hk) => match mgr.register(hk) {
                    Ok(()) => d.registered.push((hk, action)),
                    Err(e) => log::warn!("atajo '{keys}': {e}"),
                },
                Err(e) => log::warn!("atajo '{keys}' inválido: {e}"),
            }
        }
    });
}

pub fn set_now_playing(track: Option<&Track>, paused: bool) {
    DESKTOP.with(|d| {
        let mut d = d.borrow_mut();
        let Some(d) = d.as_mut() else { return };
        let text = track.map(|t| format!("{} - {}", t.title, t.artists)).unwrap_or_else(|| "Nada sonando".into());
        if let Some(item) = &d.now_item {
            item.set_text(&text);
        }
        if let Some(tray) = &d.tray {
            let tip: String = text.chars().take(120).collect();
            let _ = tray.set_tooltip(Some(tip));
            if d.tray_paused != paused {
                d.tray_paused = paused;
                let _ = tray.set_icon(icon(if paused { ICON_PAUSED } else { ICON_PLAYING }));
            }
        }
    });
}

pub fn smtc_metadata(t: &Track) {
    DESKTOP.with(|d| {
        if let Some(c) = d.borrow_mut().as_mut().and_then(|d| d.smtc.as_mut()) {
            let cover = t.thumb.as_deref().map(|u| crate::model::resize_thumb(u, 300));
            let _ = c.set_metadata(MediaMetadata {
                title: Some(&t.title),
                artist: Some(&t.artists),
                album: t.album.as_deref(),
                cover_url: cover.as_deref(),
                duration: (t.duration > 0).then(|| Duration::from_secs(t.duration as u64)),
            });
        }
    });
}

pub fn smtc_playback(paused: bool, pos: f64) {
    DESKTOP.with(|d| {
        if let Some(c) = d.borrow_mut().as_mut().and_then(|d| d.smtc.as_mut()) {
            let progress = Some(MediaPosition(Duration::from_secs_f64(pos.max(0.0))));
            let _ = c.set_playback(if paused { MediaPlayback::Paused { progress } } else { MediaPlayback::Playing { progress } });
        }
    });
}
