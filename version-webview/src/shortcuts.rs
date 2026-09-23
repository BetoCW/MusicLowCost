//! Atajos de teclado globales (port de plugins/shortcuts de Pear).
//! Las teclas multimedia fisicas ya las maneja media_controls (SMTC).

use crate::config::Shortcuts as ShortcutsConfig;
use crate::player;
use std::sync::Mutex;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutEvent, ShortcutState};

#[derive(Default)]
pub struct Bindings(Mutex<Vec<(Shortcut, &'static str)>>);

pub fn handler<R: Runtime>(app: &AppHandle<R>, shortcut: &Shortcut, event: ShortcutEvent) {
    if event.state != ShortcutState::Pressed {
        return;
    }
    let action = app
        .state::<Bindings>()
        .0
        .lock()
        .unwrap()
        .iter()
        .find(|(s, _)| s == shortcut)
        .map(|(_, a)| *a);
    match action {
        Some("showHide") => player::toggle_window(app),
        Some(cmd) => player::simple(app, cmd),
        None => {}
    }
}

pub fn apply<R: Runtime>(app: &AppHandle<R>, cfg: &ShortcutsConfig) {
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    let bindings = app.state::<Bindings>();
    let mut list = bindings.0.lock().unwrap();
    list.clear();
    if !cfg.enabled {
        return;
    }
    let wanted = [
        (&cfg.play_pause, "toggle"),
        (&cfg.next, "next"),
        (&cfg.previous, "previous"),
        (&cfg.show_hide, "showHide"),
    ];
    for (keys, action) in wanted {
        if keys.trim().is_empty() {
            continue;
        }
        match keys.parse::<Shortcut>() {
            Ok(sc) => match gs.register(sc) {
                Ok(()) => list.push((sc, action)),
                Err(e) => log::warn!("atajo '{keys}' no se pudo registrar: {e}"),
            },
            Err(e) => log::warn!("atajo '{keys}' invalido: {e}"),
        }
    }
}
