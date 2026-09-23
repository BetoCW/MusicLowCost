//! Control del reproductor desde Rust (equivalente a providers/song-controls.ts de Pear).
//! Rust manda ordenes a la pagina ejecutando `window.__yir.command(...)`.

use tauri::{AppHandle, Manager, Runtime};

pub const MAIN: &str = "main";

pub fn command<R: Runtime>(app: &AppHandle<R>, cmd: &str, arg: serde_json::Value) {
    if let Some(w) = app.get_webview_window(MAIN) {
        let js = format!(
            "window.__yir && window.__yir.command({}, {})",
            serde_json::Value::String(cmd.to_string()),
            arg
        );
        if let Err(e) = w.eval(&js) {
            log::warn!("no se pudo enviar '{cmd}' a la pagina: {e}");
        }
    }
}

pub fn simple<R: Runtime>(app: &AppHandle<R>, cmd: &str) {
    command(app, cmd, serde_json::Value::Null);
}

pub fn toggle_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(w) = app.get_webview_window(MAIN) {
        if w.is_visible().unwrap_or(false) && !w.is_minimized().unwrap_or(false) {
            hide_window(app);
        } else {
            show_window(app);
        }
    }
}

pub fn show_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(w) = app.get_webview_window(MAIN) {
        crate::webview_tweaks::set_low_memory(&w, false);
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

pub fn hide_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(w) = app.get_webview_window(MAIN) {
        let _ = w.hide();
        let low = app.state::<crate::state::AppState>().config().general.low_memory_when_hidden;
        if low {
            crate::webview_tweaks::set_low_memory(&w, true);
        }
    }
}
