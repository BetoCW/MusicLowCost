//! Ajustes directos sobre WebView2 (lo que en Pear hacia Electron/Chromium):
//! - Bloqueo de anuncios y rastreadores a nivel de red (equivalente a do-not-track/blocker.ts).
//! - Nivel de uso de memoria de WebView2 (bajar RAM cuando la ventana esta oculta).

use tauri::{Runtime, WebviewWindow};

/// Flags de Chromium para WebView2. Reemplazan las que pone Tauri por defecto,
/// por eso se repiten `msWebOOUI,msPdfOOUI,msSmartScreenProtection`.
pub fn browser_args(low_memory: bool) -> String {
    let mut disabled = vec![
        "msWebOOUI",
        "msPdfOOUI",
        "msSmartScreenProtection",
        // Las teclas multimedia y el panel de Windows los maneja Rust (souvlaki),
        // asi no salen controles duplicados.
        "HardwareMediaKeyHandling",
    ];
    let mut args = Vec::new();
    if low_memory {
        disabled.extend([
            // Proceso renderer "de repuesto" que Chromium deja precargado.
            "SpareRendererForSitePerProcess",
            // Guarda paginas anteriores en memoria para el boton "atras".
            "BackForwardCache",
            "msEdgeTranslate",
            "msShoppingExp",
            "msEdgeCollections",
            "OptimizationHints",
        ]);
        // Medido en esta PC con la portada de YouTube Music cargada (memoria privada total):
        //   sin flags ............................ ~585 MB
        //   --disable-gpu ........................ ~470 MB (proceso GPU 170 -> 25 MB)
        //   --js-flags=--optimize-for-size ....... ~470 MB (renderer 305 -> 155 MB)
        //   ambas ................................ ~287 MB
        // El costo: la interfaz se dibuja con CPU y el JS se optimiza menos.
        args.push("--disable-gpu".to_string());
        args.push("--js-flags=--optimize-for-size".to_string());
        args.push("--disable-gpu-shader-disk-cache".to_string());
        args.push("--renderer-process-limit=2".to_string());
    }
    args.insert(0, format!("--disable-features={}", disabled.join(",")));
    // Para experimentar sin recompilar: YIR_EXTRA_ARGS="--flag1 --flag2"
    if let Ok(extra) = std::env::var("YIR_EXTRA_ARGS") {
        args.push(extra);
    }
    args.join(" ")
}

/// Dominios y rutas que se bloquean (anuncios y analitica, no el historial de YouTube).
const BLOCK_FILTERS: &[&str] = &[
    "*://*.doubleclick.net/*",
    "*://*.googlesyndication.com/*",
    "*://*.googleadservices.com/*",
    "*://*.google-analytics.com/*",
    "*://www.googletagmanager.com/*",
    "*://www.youtube.com/pagead/*",
    "*://music.youtube.com/pagead/*",
    "*://www.youtube.com/api/stats/ads*",
    "*://music.youtube.com/api/stats/ads*",
    "*://*.youtube.com/ptracking*",
    "*://*.youtube.com/get_midroll_*",
];

fn is_blocked(uri: &str) -> bool {
    let Some(rest) = uri.split("://").nth(1) else { return false };
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.split(':').next().unwrap_or(host);
    let path = format!("/{path}");
    const HOSTS: &[&str] = &[
        "doubleclick.net",
        "googlesyndication.com",
        "googleadservices.com",
        "google-analytics.com",
    ];
    if HOSTS.iter().any(|h| host == *h || host.ends_with(&format!(".{h}"))) {
        return true;
    }
    if host == "www.googletagmanager.com" {
        return true;
    }
    if host.ends_with("youtube.com") {
        return path.starts_with("/pagead/")
            || path.starts_with("/api/stats/ads")
            || path.starts_with("/ptracking")
            || path.starts_with("/get_midroll_");
    }
    false
}

#[cfg(windows)]
mod win {
    use super::*;
    use webview2_com::Microsoft::Web::WebView2::Win32::*;
    use webview2_com::{WebResourceRequestedEventHandler, take_pwstr};
    use windows_core::{HSTRING, Interface, PWSTR};

    pub fn install_adblock<R: Runtime>(webview: &WebviewWindow<R>) {
        let res = webview.with_webview(|pw| unsafe {
            let result = (|| -> windows_core::Result<()> {
                let core = pw.controller().CoreWebView2()?;
                let env = core.cast::<ICoreWebView2_2>()?.Environment()?;
                for f in BLOCK_FILTERS {
                    core.AddWebResourceRequestedFilter(
                        &HSTRING::from(*f),
                        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
                    )?;
                }
                let handler = WebResourceRequestedEventHandler::create(Box::new(
                    move |_sender, args| {
                        let Some(args) = args else { return Ok(()) };
                        let mut uri = PWSTR::null();
                        args.Request()?.Uri(&mut uri)?;
                        let uri = take_pwstr(uri);
                        if is_blocked(&uri) {
                            let resp = env.CreateWebResourceResponse(
                                None,
                                403,
                                &HSTRING::from("Blocked"),
                                &HSTRING::new(),
                            )?;
                            args.SetResponse(&resp)?;
                        }
                        Ok(())
                    },
                ));
                let mut token = Default::default();
                core.add_WebResourceRequested(&handler, &mut token)?;
                Ok(())
            })();
            if let Err(e) = result {
                log::warn!("no se pudo activar el bloqueador de red: {e}");
            }
        });
        if let Err(e) = res {
            log::warn!("with_webview fallo: {e}");
        }
    }

    pub fn set_low_memory<R: Runtime>(webview: &WebviewWindow<R>, low: bool) {
        let _ = webview.with_webview(move |pw| unsafe {
            let controller = pw.controller();
            // Con IsVisible=false WebView2 deja de pintar y libera recursos de GPU;
            // el audio sigue sonando.
            let _ = controller.SetIsVisible(!low);
            if let Ok(core) = controller.CoreWebView2() {
                if let Ok(c19) = core.cast::<ICoreWebView2_19>() {
                    let level = if low {
                        COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW
                    } else {
                        COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL
                    };
                    let _ = c19.SetMemoryUsageTargetLevel(level);
                }
            }
        });
    }
}

#[cfg(windows)]
pub use win::{install_adblock, set_low_memory};

#[cfg(not(windows))]
pub fn install_adblock<R: Runtime>(_webview: &WebviewWindow<R>) {}
#[cfg(not(windows))]
pub fn set_low_memory<R: Runtime>(_webview: &WebviewWindow<R>, _low: bool) {}

#[cfg(test)]
mod tests {
    use super::is_blocked;

    #[test]
    fn bloquea_anuncios_pero_no_la_musica() {
        assert!(is_blocked("https://googleads.g.doubleclick.net/pagead/id"));
        assert!(is_blocked("https://music.youtube.com/api/stats/ads?ver=2"));
        assert!(is_blocked("https://www.youtube.com/pagead/viewthroughconversion/1"));
        assert!(!is_blocked("https://music.youtube.com/youtubei/v1/player"));
        assert!(!is_blocked("https://music.youtube.com/api/stats/watchtime?x=1"));
        assert!(!is_blocked("https://rr1---sn-abc.googlevideo.com/videoplayback?x"));
        assert!(!is_blocked("http://ipc.localhost/yir_log"));
    }
}
