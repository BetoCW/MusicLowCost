use std::time::{SystemTime, UNIX_EPOCH};

pub fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Abre una URL en el navegador por defecto.
pub fn open_url(url: &str) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", url])
            .creation_flags(0x0800_0000)
            .spawn();
    }
}

/// Pais por defecto segun la configuracion regional de Windows (es-MX -> MX).
pub fn system_country() -> Option<String> {
    static CACHE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    CACHE.get_or_init(read_system_country).clone()
}

fn read_system_country() -> Option<String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Leer el registro es mas liviano que enlazar la API de Windows solo para esto.
        let out = std::process::Command::new("reg")
            .args(["query", r"HKCU\Control Panel\International", "/v", "LocaleName"])
            .creation_flags(0x0800_0000)
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let locale = text.split_whitespace().last()?;
        let country = locale.split('-').nth(1)?;
        if country.len() == 2 {
            return Some(country.to_uppercase());
        }
    }
    None
}

/// Notificacion de Windows (toast).
pub fn notify(title: &str, body: &str) {
    let r = tauri_winrt_notification::Toast::new(tauri_winrt_notification::Toast::POWERSHELL_APP_ID)
        .title(title)
        .text1(body)
        .sound(None)
        .show();
    if let Err(e) = r {
        log::warn!("notificación: {e}");
    }
}
