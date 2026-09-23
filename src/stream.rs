//! Obtener el audio de una cancion.
//!
//! rustypipe 0.11 ya no logra descifrar el reproductor actual de YouTube, asi que la
//! URL del audio la resuelve yt-dlp (que se actualiza seguido). yt-dlp corre solo unos
//! segundos por cancion y se cierra; la descarga y la reproduccion son 100% Rust.
//!
//! La app es autosuficiente: si no encuentra yt-dlp lo descarga a su propia carpeta
//! (%APPDATA%\YoutubeInRustWeb\bin). yt-dlp necesita un motor de JavaScript para
//! descifrar YouTube: se usa Node si esta instalado y, si no, se descarga deno ahi mismo.

use std::path::{Path, PathBuf};
use std::process::Command;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const YTDLP_URL: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe";
const DENO_URL: &str = "https://github.com/denoland/deno/releases/latest/download/deno-x86_64-pc-windows-msvc.zip";

pub struct Resolved {
    pub url: String,
    pub user_agent: String,
}

/// Herramientas externas ya localizadas.
#[derive(Debug, Clone)]
pub struct Tools {
    pub ytdlp: PathBuf,
    /// ("node" | "deno", ruta)
    pub js: Option<(&'static str, PathBuf)>,
    /// true si yt-dlp es la copia propia de la app (se puede autoactualizar).
    pub own_ytdlp: bool,
}

/// Herramientas localizadas; se vuelven a buscar si alguien borra los archivos.
static TOOLS: std::sync::Mutex<Option<Tools>> = std::sync::Mutex::new(None);

fn cached_tools() -> Option<Tools> {
    let mut t = TOOLS.lock().unwrap();
    if t.as_ref().is_some_and(|t| !t.ytdlp.is_file() || t.js.as_ref().is_some_and(|(_, p)| !p.is_file())) {
        *t = None;
    }
    t.clone()
}

pub fn bin_dir() -> PathBuf {
    crate::config::data_dir().join("bin")
}

fn in_path(exe: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(d) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(d).join("Microsoft").join("WinGet").join("Links"));
    }
    if let Some(paths) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&paths));
    }
    dirs.push(PathBuf::from(r"C:\Program Files\nodejs"));
    dirs.into_iter().map(|d| d.join(exe)).find(|p| p.is_file())
}

fn find_tools() -> (Option<(PathBuf, bool)>, Option<(&'static str, PathBuf)>) {
    let bin = bin_dir();
    let ytdlp = if bin.join("yt-dlp.exe").is_file() {
        Some((bin.join("yt-dlp.exe"), true))
    } else {
        in_path("yt-dlp.exe").map(|p| (p, false))
    };
    let js = if bin.join("deno.exe").is_file() {
        Some(("deno", bin.join("deno.exe")))
    } else if let Some(p) = in_path("deno.exe") {
        Some(("deno", p))
    } else {
        in_path("node.exe").map(|p| ("node", p))
    };
    (ytdlp, js)
}

async fn download_file(http: &reqwest::Client, url: &str, dest: &Path) -> Result<(), String> {
    let resp = http
        .get(url)
        .timeout(std::time::Duration::from_secs(900))
        .send().await.map_err(|e| format!("descarga de {url}: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("descarga de {url}: HTTP {}", resp.status()));
    }
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    let tmp = dest.with_extension("part");
    std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, dest).map_err(|e| e.to_string())
}

/// Localiza (o descarga la primera vez) yt-dlp y un motor de JavaScript.
/// `status` recibe mensajes para mostrar en la interfaz.
pub async fn ensure_tools(http: &reqwest::Client, status: impl Fn(&str)) -> Result<Tools, String> {
    if let Some(t) = cached_tools() {
        return Ok(t);
    }
    // Evita dos descargas simultaneas (arranque + primera cancion).
    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _guard = LOCK.lock().await;
    if let Some(t) = cached_tools() {
        return Ok(t);
    }
    let bin = bin_dir();
    let (mut ytdlp, mut js) = find_tools();
    if ytdlp.is_none() || js.is_none() {
        std::fs::create_dir_all(&bin).map_err(|e| e.to_string())?;
    }
    if ytdlp.is_none() {
        status("Descargando yt-dlp (solo la primera vez, ~18 MB)…");
        log::info!("descargando yt-dlp a {}", bin.display());
        download_file(http, YTDLP_URL, &bin.join("yt-dlp.exe")).await?;
        ytdlp = Some((bin.join("yt-dlp.exe"), true));
    }
    if js.is_none() {
        status("Descargando deno para yt-dlp (solo la primera vez, ~45 MB)…");
        log::info!("descargando deno a {}", bin.display());
        let zip = bin.join("deno.zip");
        download_file(http, DENO_URL, &zip).await?;
        // tar de Windows 10/11 descomprime .zip sin librerias extra.
        let mut cmd = Command::new("tar");
        cmd.arg("-xf").arg(&zip).arg("-C").arg(&bin);
        no_window(&mut cmd);
        let ok = cmd.status().map(|s| s.success()).unwrap_or(false);
        let _ = std::fs::remove_file(&zip);
        if !ok || !bin.join("deno.exe").is_file() {
            return Err("no se pudo descomprimir deno".into());
        }
        js = Some(("deno", bin.join("deno.exe")));
    }
    let (ytdlp, own_ytdlp) = ytdlp.unwrap();
    let tools = Tools { ytdlp, js, own_ytdlp };
    log::info!("herramientas: {tools:?}");
    *TOOLS.lock().unwrap() = Some(tools.clone());
    Ok(tools)
}

fn no_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

/// Comando yt-dlp ya configurado (requiere haber llamado a `ensure_tools`).
pub fn ytdlp_command() -> Result<Command, String> {
    let t = cached_tools().ok_or("yt-dlp todavía no está listo")?;
    let mut cmd = Command::new(&t.ytdlp);
    if let Some((name, path)) = &t.js {
        cmd.arg("--js-runtimes").arg(format!("{name}:{}", path.display()));
    }
    no_window(&mut cmd);
    Ok(cmd)
}

/// Actualiza la copia propia de yt-dlp como mucho una vez cada 3 dias
/// (YouTube cambia seguido y las versiones viejas dejan de funcionar).
pub fn maybe_self_update() {
    let Some(t) = cached_tools() else { return };
    if !t.own_ytdlp {
        return;
    }
    let stamp = bin_dir().join("last-update");
    let recent = std::fs::metadata(&stamp)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|m| m.elapsed().ok())
        .map(|e| e.as_secs() < 3 * 24 * 3600)
        .unwrap_or(false);
    if recent {
        return;
    }
    let _ = std::fs::write(&stamp, b"");
    let mut cmd = Command::new(&t.ytdlp);
    cmd.args(["-U", "--no-warnings"]);
    no_window(&mut cmd);
    match cmd.output() {
        Ok(o) => log::info!("yt-dlp -U: {}", String::from_utf8_lossy(&o.stdout).lines().last().unwrap_or("")),
        Err(e) => log::warn!("yt-dlp -U: {e}"),
    }
}

/// Bloqueante: llamar desde spawn_blocking.
pub fn resolve(video_id: &str, low_quality: bool) -> Result<Resolved, String> {
    // Solo AAC/M4A: es lo que decodifica symphonia sin librerias extra.
    let format = if low_quality {
        "worstaudio[ext=m4a]/bestaudio[ext=m4a]"
    } else {
        "bestaudio[ext=m4a]/bestaudio[acodec^=mp4a]"
    };
    let out = ytdlp_command()?
        .args([
            "-f",
            format,
            "--no-playlist",
            "--no-warnings",
            "--print",
            "%(url)s",
            "--print",
            "%(http_headers.User-Agent)s",
        ])
        .arg(format!("https://music.youtube.com/watch?v={video_id}"))
        .output()
        .map_err(|e| format!("no se pudo ejecutar yt-dlp: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let last = err.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("error desconocido");
        return Err(format!("yt-dlp: {last}"));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let url = lines.next().ok_or("yt-dlp no devolvió URL")?.trim().to_string();
    let user_agent = lines.next().unwrap_or("Mozilla/5.0").trim().to_string();
    Ok(Resolved { url, user_agent })
}

/// Descarga el archivo completo por partes (`&range=` evita el limite de velocidad).
pub async fn download(http: &reqwest::Client, r: &Resolved) -> Result<Vec<u8>, String> {
    const CHUNK: u64 = 9_000_000;
    let mut data: Vec<u8> = Vec::new();
    let mut pos = 0u64;
    loop {
        let end = pos + CHUNK - 1;
        let resp = http
            .get(format!("{}&range={pos}-{end}", r.url))
            .header("User-Agent", &r.user_agent)
            .header("Origin", "https://music.youtube.com")
            .header("Referer", "https://music.youtube.com/")
            .send()
            .await
            .map_err(|e| format!("descarga: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("descarga: HTTP {}", resp.status()));
        }
        let bytes = resp.bytes().await.map_err(|e| format!("descarga: {e}"))?;
        let n = bytes.len() as u64;
        if data.is_empty() {
            data.reserve(n as usize);
        }
        data.extend_from_slice(&bytes);
        if n < CHUNK {
            break;
        }
        pos += n;
    }
    if data.is_empty() {
        return Err("descarga vacía".into());
    }
    data.shrink_to_fit();
    Ok(data)
}
