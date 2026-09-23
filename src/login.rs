//! Inicio de sesion con Google en una ventana propia.
//!
//! Edge y Chrome cifran sus cookies (desde 2024) y ya no se pueden leer desde fuera, asi que
//! el login se hace aqui: la app se relanza a si misma con `--login <archivo>`, abre una ventana
//! con WebView2 en la pagina de Google, espera a que el usuario entre a YouTube Music y guarda
//! las cookies de youtube.com en `<archivo>`. Ese proceso se cierra al terminar, asi que el
//! WebView no ocupa memoria en el reproductor.

use crate::LoginWindow;
use slint::ComponentHandle;
use std::cell::RefCell;
use std::io::Write;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;
use wry::dpi::{PhysicalPosition, PhysicalSize};
use wry::{Rect, WebContext, WebView, WebViewBuilder};

const START_URL: &str =
    "https://accounts.google.com/ServiceLogin?service=youtube&passive=true&continue=https%3A%2F%2Fmusic.youtube.com%2F";

/// Cookies que indican que la sesion ya esta iniciada.
const SESSION_COOKIES: &[&str] = &["SAPISID", "__Secure-3PAPISID"];

pub fn webview_dir() -> PathBuf {
    crate::config::data_dir().join("login-webview")
}

fn trace(msg: &str) {
    let path = crate::config::data_dir().join("login.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{} {msg}", crate::util::unix_now());
    }
}

fn bounds(w: &LoginWindow) -> Rect {
    let s = w.window().size();
    Rect { position: PhysicalPosition::new(0, 0).into(), size: PhysicalSize::new(s.width, s.height).into() }
}

/// Punto de entrada del proceso `--login`.
pub fn run(out: PathBuf) {
    let _ = std::fs::remove_file(crate::config::data_dir().join("login.log"));
    let window = match LoginWindow::new() {
        Ok(w) => w,
        Err(e) => return trace(&format!("ventana: {e}")),
    };

    // El WebContext y el WebView tienen que vivir mientras corra el bucle de eventos.
    let ctx = Rc::new(RefCell::new(WebContext::new(Some(webview_dir()))));
    let webview: Rc<RefCell<Option<WebView>>> = Rc::new(RefCell::new(None));

    // La ventana real (HWND) recien existe cuando arranca el bucle de eventos,
    // por eso el WebView se crea un instante despues.
    {
        let (weak, ctx, webview) = (window.as_weak(), ctx.clone(), webview.clone());
        slint::Timer::single_shot(Duration::from_millis(100), move || {
            let Some(w) = weak.upgrade() else { return };
            let handle = w.window().window_handle();
            let mut ctx = ctx.borrow_mut();
            match WebViewBuilder::new_with_web_context(&mut ctx)
                .with_url(START_URL)
                .with_bounds(bounds(&w))
                .build_as_child(&handle)
            {
                Ok(v) => {
                    trace("WebView listo");
                    *webview.borrow_mut() = Some(v);
                }
                Err(e) => {
                    trace(&format!("WebView2: {e}"));
                    let _ = slint::quit_event_loop();
                }
            }
        });
    }

    let last_url: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    let weak = window.as_weak();
    let wv = webview.clone();
    let last = last_url.clone();
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(700), move || {
        let Some(w) = weak.upgrade() else { return };
        let guard = wv.borrow();
        let Some(webview) = guard.as_ref() else { return };
        let _ = webview.set_bounds(bounds(&w));
        if let Ok(u) = webview.url() {
            // Solo el dominio y la ruta (sin parametros) para el diagnostico.
            *last.borrow_mut() = u.split('?').next().unwrap_or("").to_string();
        }
        // No se exige estar ya en music.youtube.com: Google a veces se queda en una pagina
        // intermedia (consentimiento, etc.) aunque la sesion ya este creada.
        let Ok(cookies) = webview.cookies_for_url("https://music.youtube.com/") else { return };
        if !cookies.iter().any(|c| SESSION_COOKIES.contains(&c.name())) {
            return;
        }
        let header = cookies.iter().map(|c| format!("{}={}", c.name(), c.value())).collect::<Vec<_>>().join("; ");
        match std::fs::write(&out, header) {
            Ok(()) => trace(&format!("sesión obtenida ({} cookies)", cookies.len())),
            Err(e) => trace(&format!("no se pudo guardar: {e}")),
        }
        let _ = w.hide();
        let _ = slint::quit_event_loop();
    });

    if let Err(e) = window.run() {
        trace(&format!("bucle: {e}"));
    }
    trace(&format!("ventana cerrada (última página: {})", last_url.borrow()));
}
