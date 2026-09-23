//! Icono en la bandeja del sistema (port de src/tray.ts de Pear).

use crate::integrations::downloader;
use crate::player;
use crate::state::SongInfo;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Runtime};

const ICON_PLAYING: &[u8] = include_bytes!("../icons/tray.png");
const ICON_PAUSED: &[u8] = include_bytes!("../icons/tray-paused.png");

pub struct TrayState<R: Runtime> {
    tray: TrayIcon<R>,
    now_playing: MenuItem<R>,
    paused: std::sync::Mutex<bool>,
}

pub fn init<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let now_playing = MenuItem::with_id(app, "now", "Nada sonando", false, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &now_playing,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "toggle", "Reproducir / Pausa", true, None::<&str>)?,
            &MenuItem::with_id(app, "next", "Siguiente", true, None::<&str>)?,
            &MenuItem::with_id(app, "previous", "Anterior", true, None::<&str>)?,
            &MenuItem::with_id(app, "like", "Me gusta", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "show", "Mostrar / Ocultar", true, None::<&str>)?,
            &MenuItem::with_id(app, "settings", "Ajustes", true, None::<&str>)?,
            &MenuItem::with_id(app, "download", "Descargar cancion actual", true, None::<&str>)?,
            &MenuItem::with_id(app, "reload", "Recargar", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "quit", "Salir", true, None::<&str>)?,
        ],
    )?;

    let tray = TrayIconBuilder::with_id("main")
        .icon(Image::from_bytes(ICON_PAUSED)?)
        .tooltip("YoutubeInRustWeb")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "toggle" | "next" | "previous" | "like" => player::simple(app, event.id.as_ref()),
            "show" => player::toggle_window(app),
            "settings" => {
                player::show_window(app);
                player::simple(app, "openSettings");
            }
            "download" => {
                if let Err(e) = downloader::download_current(app) {
                    crate::util::notify(app, "Descarga", &e);
                }
            }
            "reload" => player::simple(app, "reload"),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                player::toggle_window(tray.app_handle());
            }
        })
        .build(app)?;

    app.manage(TrayState { tray, now_playing, paused: std::sync::Mutex::new(true) });
    Ok(())
}

pub fn set_now_playing<R: Runtime>(app: &AppHandle<R>, song: Option<&SongInfo>, paused: bool) {
    let Some(state) = app.try_state::<TrayState<R>>() else { return };
    let text = match song {
        Some(s) => format!("{} - {}", s.title, s.artist),
        None => "Nada sonando".into(),
    };
    // El tooltip de Windows admite como mucho 127 caracteres.
    let tooltip: String = text.chars().take(120).collect();
    let _ = state.tray.set_tooltip(Some(&tooltip));
    let _ = state.now_playing.set_text(&text);

    let mut last = state.paused.lock().unwrap();
    if *last != paused {
        *last = paused;
        let bytes = if paused { ICON_PAUSED } else { ICON_PLAYING };
        if let Ok(img) = Image::from_bytes(bytes) {
            let _ = state.tray.set_icon(Some(img));
        }
    }
}
