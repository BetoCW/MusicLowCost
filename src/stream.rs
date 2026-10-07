//! Obtener el audio de una cancion.
//!
//! rustypipe 0.11 ya no logra descifrar el reproductor actual de YouTube, asi que la
//! URL del audio la resuelve yt-dlp (que se actualiza seguido). yt-dlp corre solo unos
//! segundos por cancion y se cierra; la descarga y la reproduccion son 100% Rust.
//!
//! La app es autosuficiente: si no encuentra yt-dlp lo descarga a su propia carpeta
//! (%APPDATA%\YoutubeInRustWeb\bin\yt-dlp). yt-dlp necesita un motor de JavaScript para
//! descifrar YouTube: se usa Node si esta instalado y, si no, se descarga deno ahi mismo.
//!
//! Se usa la version "en carpeta" de yt-dlp (yt-dlp_win.zip) y no el yt-dlp.exe de un
//! solo archivo: ese se desempaqueta en cada ejecucion y tarda ~2.5 s extra por cancion.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use std::process::Command;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const YTDLP_ZIP_URL: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_win.zip";
const YTDLP_LATEST_URL: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest";
const DENO_URL: &str = "https://github.com/denoland/deno/releases/latest/download/deno-x86_64-pc-windows-msvc.zip";

const BROWSER_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/149.0.0.0 Safari/537.36";

#[derive(Clone)]
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

/// Carpeta con la copia propia de yt-dlp (version en carpeta).
fn own_ytdlp_dir() -> PathBuf {
    bin_dir().join("yt-dlp")
}

/// Mientras se reemplaza la carpeta de yt-dlp no se puede ejecutar (y viceversa).
static YTDLP_SWAP: std::sync::RwLock<()> = std::sync::RwLock::new(());

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
    let own = own_ytdlp_dir().join("yt-dlp.exe");
    let ytdlp = if own.is_file() { Some((own, true)) } else { in_path("yt-dlp.exe").map(|p| (p, false)) };
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

/// Descomprime un .zip con el tar de Windows 10/11 (sin librerias extra).
fn unzip(zip: &Path, dest: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let mut cmd = Command::new("tar");
    cmd.arg("-xf").arg(zip).arg("-C").arg(dest);
    no_window(&mut cmd);
    match cmd.status() {
        Ok(s) if s.success() => Ok(()),
        _ => Err(format!("no se pudo descomprimir {}", zip.display())),
    }
}

/// Descarga la ultima version en carpeta de yt-dlp y reemplaza la copia propia.
async fn install_ytdlp(http: &reqwest::Client) -> Result<(), String> {
    let bin = bin_dir();
    std::fs::create_dir_all(&bin).map_err(|e| e.to_string())?;
    let zip = bin.join("yt-dlp_win.zip");
    download_file(http, YTDLP_ZIP_URL, &zip).await?;
    tokio::task::spawn_blocking(move || {
        let fresh = bin.join("yt-dlp.new");
        let old = bin.join("yt-dlp.old");
        let _ = std::fs::remove_dir_all(&fresh);
        let _ = std::fs::remove_dir_all(&old);
        let r = unzip(&zip, &fresh);
        let _ = std::fs::remove_file(&zip);
        r?;
        if !fresh.join("yt-dlp.exe").is_file() {
            return Err("el zip de yt-dlp no trae yt-dlp.exe".to_string());
        }
        let _guard = YTDLP_SWAP.write().unwrap_or_else(|e| e.into_inner());
        let dir = own_ytdlp_dir();
        // Renombrar falla completo (sin dejar nada a medias) si algun archivo esta en uso.
        if dir.exists() {
            std::fs::rename(&dir, &old).map_err(|e| format!("yt-dlp en uso: {e}"))?;
        }
        std::fs::rename(&fresh, &dir).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_dir_all(&old);
        // Copia vieja de un solo archivo (lenta): ya no se usa.
        let _ = std::fs::remove_file(bin.join("yt-dlp.exe"));
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
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
    // Sin yt-dlp, o con la copia propia vieja de un solo archivo: bajar la version en carpeta.
    let old_single = bin.join("yt-dlp.exe");
    if ytdlp.is_none() || old_single.is_file() {
        status("Descargando yt-dlp (solo la primera vez, ~18 MB)…");
        log::info!("descargando yt-dlp a {}", own_ytdlp_dir().display());
        match install_ytdlp(http).await {
            Ok(()) => ytdlp = Some((own_ytdlp_dir().join("yt-dlp.exe"), true)),
            // Si falla pero hay una copia de antes, se sigue usando esa.
            Err(e) if ytdlp.is_some() || old_single.is_file() => {
                log::warn!("no se pudo instalar yt-dlp en carpeta: {e}");
                ytdlp = ytdlp.or(Some((old_single, false)));
            }
            Err(e) => return Err(e),
        }
    }
    if js.is_none() {
        status("Descargando deno para yt-dlp (solo la primera vez, ~45 MB)…");
        log::info!("descargando deno a {}", bin.display());
        let zip = bin.join("deno.zip");
        download_file(http, DENO_URL, &zip).await?;
        let ok = unzip(&zip, &bin).is_ok();
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

/// Corre yt-dlp una vez sin hacer nada: la primera ejecucion despues de encender la PC
/// tarda 10-20 s (Windows revisa sus archivos); asi no le toca a la primera cancion.
/// Bloqueante: llamar desde spawn_blocking.
pub fn warm_up() {
    let _guard = YTDLP_SWAP.read().unwrap_or_else(|e| e.into_inner());
    if let Ok(mut cmd) = ytdlp_command() {
        let t = Instant::now();
        let _ = cmd.arg("--version").output();
        log::info!("yt-dlp listo en {:?}", t.elapsed());
    }
}

/// Actualiza la copia propia de yt-dlp como mucho una vez cada 3 dias
/// (YouTube cambia seguido y las versiones viejas dejan de funcionar).
/// La version en carpeta no se actualiza con `-U`: si hay una nueva se vuelve a bajar.
pub async fn maybe_self_update(http: &reqwest::Client) {
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
    // .../releases/latest redirige a .../releases/tag/<version>
    let latest = match http.head(YTDLP_LATEST_URL).send().await {
        Ok(r) => r.url().path_segments().and_then(|mut s| s.next_back()).unwrap_or("").to_string(),
        Err(e) => return log::warn!("yt-dlp: no se pudo consultar la ultima version: {e}"),
    };
    let ytdlp = t.ytdlp.clone();
    let current = tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(ytdlp);
        cmd.arg("--version");
        no_window(&mut cmd);
        cmd.output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
    })
    .await
    .unwrap_or_default();
    if latest.is_empty() || latest == current {
        let _ = std::fs::write(&stamp, b"");
        return;
    }
    log::info!("actualizando yt-dlp {current} -> {latest}");
    match install_ytdlp(http).await {
        Ok(()) => {
            let _ = std::fs::write(&stamp, b"");
        }
        Err(e) => log::warn!("actualizar yt-dlp: {e}"),
    }
}

/// Error de yt-dlp cuando YouTube pide confirmar que no eres un robot.
pub fn is_bot_check(err: &str) -> bool {
    let e = err.to_lowercase();
    e.contains("not a bot") || e.contains("confirm you") || e.contains("sign in to confirm")
}

/// URLs ya resueltas (duran ~6 h): volver a una cancion no vuelve a correr yt-dlp.
/// (id, calidad baja, url, valida hasta)
type ResolvedEntry = (String, bool, Resolved, Instant);
static RESOLVED: std::sync::Mutex<Vec<ResolvedEntry>> = std::sync::Mutex::new(Vec::new());

fn cached_resolve(video_id: &str, low: bool) -> Option<Resolved> {
    let mut c = RESOLVED.lock().unwrap();
    let now = Instant::now();
    c.retain(|e| e.3 > now);
    c.iter().find(|e| e.0 == video_id && e.1 == low).map(|e| e.2.clone())
}

/// Olvida la URL de una cancion (si la descarga fallo, la proxima vez se resuelve de nuevo).
pub fn forget(video_id: &str) {
    RESOLVED.lock().unwrap().retain(|e| e.0 != video_id);
}

/// Hasta cuando sirve la URL (parametro `expire`, con 10 min de margen), como mucho 5 h.
fn valid_until(url: &str) -> Instant {
    let max = Duration::from_secs(5 * 3600);
    let left = url_param(url, "expire")
        .and_then(|e| e.parse::<u64>().ok())
        .map(|e| Duration::from_secs(e.saturating_sub(crate::util::unix_now() + 600)))
        .unwrap_or(max);
    Instant::now() + left.min(max)
}

fn url_param<'a>(url: &'a str, name: &str) -> Option<&'a str> {
    let query = url.split_once('?')?.1;
    query.split('&').find_map(|kv| kv.strip_prefix(name)?.strip_prefix('='))
}

/// Cookies de la sesion en formato Netscape (lo que lee `yt-dlp --cookies`).
pub fn write_cookie_file(header: &str, path: &Path) -> Result<(), String> {
    let mut out = String::from("# Netscape HTTP Cookie File\n");
    let expires = crate::util::unix_now() + 30 * 24 * 3600;
    for kv in header.split(';') {
        let Some((k, v)) = kv.trim().split_once('=') else { continue };
        let secure = if k.starts_with("__Secure-") || k.starts_with("__Host-") { "TRUE" } else { "FALSE" };
        out.push_str(&format!(".youtube.com\tTRUE\t/\t{secure}\t{expires}\t{k}\t{v}\n"));
    }
    std::fs::write(path, out).map_err(|e| e.to_string())
}

/// Bloqueante: llamar desde spawn_blocking. `cookies`: archivo de cookies para yt-dlp
/// (solo cuando YouTube pide confirmar que no eres un robot).
pub fn resolve(video_id: &str, low_quality: bool, cookies: Option<&Path>) -> Result<Resolved, String> {
    crate::bench::event("lat_t1_resolve_start", 0);
    if let Some(r) = cached_resolve(video_id, low_quality) {
        crate::bench::event("lat_t2_resolved", 1);
        return Ok(r);
    }
    // Solo AAC/M4A: es lo que decodifica symphonia sin librerias extra.
    let format = if low_quality {
        "worstaudio[ext=m4a]/bestaudio[ext=m4a]"
    } else {
        "bestaudio[ext=m4a]/bestaudio[acodec^=mp4a]"
    };
    let _guard = YTDLP_SWAP.read().unwrap_or_else(|e| e.into_inner());
    let mut cmd = ytdlp_command()?;
    cmd.args([
        "-f",
        format,
        "--no-playlist",
        "--no-warnings",
        "--print",
        "%(url)s",
        "--print",
        "%(http_headers.User-Agent)s",
    ]);
    if let Some(c) = cookies {
        cmd.arg("--cookies").arg(c);
    }
    let out = cmd
        .arg(format!("https://music.youtube.com/watch?v={video_id}"))
        .output()
        .map_err(|e| format!("no se pudo ejecutar yt-dlp: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        // La linea con "ERROR" es la que explica; las demas suelen ser avisos.
        let last = err
            .lines()
            .rfind(|l| l.contains("ERROR"))
            .or_else(|| err.lines().rfind(|l| !l.trim().is_empty()))
            .unwrap_or("error desconocido");
        return Err(format!("yt-dlp: {}", last.trim()));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let url = lines.next().ok_or("yt-dlp no devolvió URL")?.trim().to_string();
    // Con algunos clientes de yt-dlp el formato no trae User-Agent ("NA").
    let user_agent = match lines.next().map(str::trim) {
        Some(ua) if ua.starts_with("Mozilla/") => ua.to_string(),
        _ => BROWSER_UA.to_string(),
    };
    let r = Resolved { url, user_agent };
    crate::bench::event("lat_t2_resolved", 0);
    RESOLVED.lock().unwrap().push((video_id.to_string(), low_quality, r.clone(), valid_until(&r.url)));
    Ok(r)
}

/// Audio de una cancion que se va llenando mientras se descarga. Se puede empezar a
/// reproducir con el primer pedazo; al terminar queda en un `Arc<[u8]>` (sin copias extra
/// para repetirla o repartirla en el Jam).
pub struct Growing {
    state: Mutex<GrowState>,
    cv: Condvar,
    done_tx: tokio::sync::watch::Sender<bool>,
    /// Tamano total en bytes.
    pub total: u64,
}

struct GrowState {
    buf: Buf,
    done: bool,
    error: Option<String>,
}

enum Buf {
    Filling(Vec<u8>),
    Full(Arc<[u8]>),
}

impl Buf {
    fn bytes(&self) -> &[u8] {
        match self {
            Buf::Filling(v) => v,
            Buf::Full(a) => a,
        }
    }
}

impl Growing {
    fn with_buf(total: u64, buf: Buf, done: bool) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(GrowState { buf, done, error: None }),
            cv: Condvar::new(),
            done_tx: tokio::sync::watch::channel(done).0,
            total,
        })
    }

    fn new(total: u64) -> Arc<Self> {
        Self::with_buf(total, Buf::Filling(Vec::with_capacity(total.min(MAX_AUDIO) as usize)), false)
    }

    /// Audio que ya esta completo.
    pub fn complete(data: Arc<[u8]>) -> Arc<Self> {
        Self::with_buf(data.len() as u64, Buf::Full(data), true)
    }

    fn push(&self, bytes: &[u8]) {
        if let Buf::Filling(v) = &mut self.state.lock().unwrap().buf {
            v.extend_from_slice(bytes);
        }
        self.cv.notify_all();
    }

    fn finish(&self, error: Option<String>) {
        {
            let mut st = self.state.lock().unwrap();
            if error.is_none() {
                if let Buf::Filling(v) = &mut st.buf {
                    let data: Arc<[u8]> = Arc::from(std::mem::take(v));
                    st.buf = Buf::Full(data);
                }
            }
            st.done = true;
            st.error = error;
        }
        self.cv.notify_all();
        self.done_tx.send_replace(true);
    }

    fn len(&self) -> usize {
        self.state.lock().unwrap().buf.bytes().len()
    }

    /// true cuando ya se descargo completo (sin errores).
    pub fn is_done(&self) -> bool {
        let st = self.state.lock().unwrap();
        st.done && st.error.is_none()
    }

    /// Espera a que termine la descarga.
    pub async fn wait_done(&self) -> Result<(), String> {
        let mut rx = self.done_tx.subscribe();
        let _ = rx.wait_for(|d| *d).await;
        match &self.state.lock().unwrap().error {
            Some(e) => Err(e.clone()),
            None => Ok(()),
        }
    }

    /// El audio completo (None si todavia se esta descargando).
    pub fn full(&self) -> Option<Arc<[u8]>> {
        match &self.state.lock().unwrap().buf {
            Buf::Full(a) => Some(a.clone()),
            Buf::Filling(_) => None,
        }
    }

    pub fn reader(self: &Arc<Self>) -> GrowReader {
        GrowReader { g: self.clone(), pos: 0 }
    }
}

/// Lector para el decodificador: si pide bytes que todavia no llegan, espera.
pub struct GrowReader {
    g: Arc<Growing>,
    pos: u64,
}

impl std::io::Read for GrowReader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let mut st = self.g.state.lock().unwrap();
        loop {
            let bytes = st.buf.bytes();
            if (self.pos as usize) < bytes.len() {
                let start = self.pos as usize;
                let n = out.len().min(bytes.len() - start);
                out[..n].copy_from_slice(&bytes[start..start + n]);
                self.pos += n as u64;
                return Ok(n);
            }
            if st.done {
                return match &st.error {
                    Some(e) => Err(std::io::Error::other(e.clone())),
                    None => Ok(0),
                };
            }
            st = self.g.cv.wait(st).unwrap();
        }
    }
}

impl std::io::Seek for GrowReader {
    fn seek(&mut self, to: std::io::SeekFrom) -> std::io::Result<u64> {
        let new = match to {
            std::io::SeekFrom::Start(p) => p as i64,
            std::io::SeekFrom::Current(d) => self.pos as i64 + d,
            std::io::SeekFrom::End(d) => self.g.total as i64 + d,
        };
        if new < 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "seek antes del inicio"));
        }
        self.pos = new as u64;
        Ok(self.pos)
    }
}

/// Una cancion M4A no deberia pasar de esto (~2 h a 128 kbps).
const MAX_AUDIO: u64 = 128 * 1024 * 1024;
/// Con esto ya se puede empezar a decodificar y sonar.
const FIRST_CHUNK: usize = 384 * 1024;
/// Cada peticion pide hasta esto (`&range=` en la URL evita el limite de velocidad de YouTube).
/// Pocas peticiones grandes: YouTube responde 403 a las peticiones siguientes de algunas URLs.
const RANGE: u64 = 9_000_000;

async fn open_range(http: &reqwest::Client, r: &Resolved, from: u64, to: u64) -> Result<reqwest::Response, String> {
    let resp = http
        .get(format!("{}&range={from}-{to}", r.url))
        .header("User-Agent", &r.user_agent)
        .header("Origin", "https://music.youtube.com")
        .header("Referer", "https://music.youtube.com/")
        .send()
        .await
        .map_err(|e| format!("descarga: {}", error_chain(&e)))?;
    if !resp.status().is_success() {
        return Err(format!("descarga: HTTP {} ({})", resp.status(), resp.url().host_str().unwrap_or("")));
    }
    Ok(resp)
}

/// Error con sus causas ("error sending request" solo no dice nada).
fn error_chain(e: &dyn std::error::Error) -> String {
    let mut s = e.to_string();
    let mut cur = e.source();
    while let Some(c) = cur {
        s.push_str(&format!(": {c}"));
        cur = c.source();
    }
    // La URL completa (con la firma) no aporta y llena el log.
    match s.find("(https://") {
        Some(i) => format!("{}{}", &s[..i], s[i..].find(')').map(|j| &s[i + j + 1..]).unwrap_or("")),
        None => s,
    }
}

/// Empieza a descargar; vuelve en cuanto llegan los primeros bytes y el resto se sigue
/// leyendo de la misma respuesta en segundo plano (se deja si ya nadie usa el audio).
pub async fn start_download(http: &reqwest::Client, r: &Resolved) -> Result<Arc<Growing>, String> {
    let total = url_param(&r.url, "clen").and_then(|c| c.parse::<u64>().ok()).filter(|t| (1..=MAX_AUDIO).contains(t));
    let Some(total) = total else {
        // Sin el tamano no se puede ir por partes con seguridad: se baja completo.
        return download(http, r).await.map(|d| Growing::complete(Arc::from(d)));
    };
    let g = Growing::new(total);
    let mut resp = open_range(http, r, 0, RANGE.min(total) - 1).await?;
    crate::bench::event("lat_t3_http_headers", 0);
    while g.len() < FIRST_CHUNK.min(total as usize) {
        match resp.chunk().await.map_err(|e| format!("descarga: {}", error_chain(&e)))? {
            Some(c) => g.push(&c),
            None => break,
        }
    }
    if g.len() == 0 {
        return Err("descarga vacía".into());
    }
    crate::bench::event("lat_t4_first_chunk", g.len() as u64);
    let (g2, http, r) = (g.clone(), http.clone(), r.clone());
    tokio::task::spawn_local(async move {
        let mut resp = Some(resp);
        let mut tries = 0u64;
        loop {
            let pos = g2.len() as u64;
            if pos >= total {
                return g2.finish(None);
            }
            // Nadie mas tiene el audio (se salto la cancion): se deja de bajar.
            if Arc::strong_count(&g2) == 1 {
                return;
            }
            let (mut c, fresh) = match resp.take() {
                Some(c) => (c, false),
                // Se acabo esa peticion (o fallo): se pide lo que falta.
                None => match open_range(&http, &r, pos, (pos + RANGE).min(total) - 1).await {
                    Ok(c) => (c, true),
                    Err(e) => {
                        if tries >= 3 {
                            return g2.finish(Some(e));
                        }
                        tries += 1;
                        log::warn!("{e} (reintento {tries})");
                        tokio::time::sleep(Duration::from_millis(400 * tries)).await;
                        continue;
                    }
                },
            };
            let err = match c.chunk().await {
                Ok(Some(bytes)) => {
                    tries = 0;
                    g2.push(&bytes);
                    resp = Some(c);
                    continue;
                }
                // Termino esa peticion: en la siguiente vuelta se pide lo que falta.
                Ok(None) if !fresh => continue,
                Ok(None) => "descarga incompleta".to_string(),
                Err(e) => format!("descarga: {}", error_chain(&e)),
            };
            if tries >= 3 {
                return g2.finish(Some(err));
            }
            tries += 1;
            log::warn!("{err} (reintento {tries})");
            tokio::time::sleep(Duration::from_millis(400 * tries)).await;
        }
    });
    Ok(g)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parametros_de_url() {
        let u = "https://x.googlevideo.com/videoplayback?expire=1790000000&clen=3456789&mime=audio%2Fmp4";
        assert_eq!(url_param(u, "clen"), Some("3456789"));
        assert_eq!(url_param(u, "expire"), Some("1790000000"));
        assert_eq!(url_param(u, "len"), None);
        assert!(is_bot_check("ERROR: [youtube] x: Sign in to confirm you're not a bot."));
        assert!(!is_bot_check("ERROR: Video unavailable"));
    }

    #[test]
    fn el_lector_espera_los_bytes_que_faltan() {
        use std::io::Read;
        let g = Growing::new(6);
        g.push(&[1, 2, 3]);
        let mut r = g.reader();
        let g2 = g.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            g2.push(&[4, 5, 6]);
            g2.finish(None);
        });
        let mut out = Vec::new();
        r.read_to_end(&mut out).unwrap();
        t.join().unwrap();
        assert_eq!(out, [1, 2, 3, 4, 5, 6]);
        assert!(g.is_done() && g.full().is_some_and(|f| f.len() == 6));
    }

    /// Cancion real: yt-dlp + descarga por partes, y el audio se puede decodificar con
    /// solo el primer pedazo. Necesita red: `cargo test -- --ignored streaming_real`
    #[test]
    #[ignore]
    fn streaming_real() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            let http = reqwest::Client::new();
            ensure_tools(&http, |m| println!("{m}")).await.unwrap();
            let t0 = Instant::now();
            let r = tokio::task::spawn_blocking(|| resolve("dQw4w9WgXcQ", false, None)).await.unwrap().unwrap();
            println!("yt-dlp: {:?}", t0.elapsed());
            let t1 = Instant::now();
            let g = start_download(&http, &r).await.unwrap();
            println!("primer pedazo: {:?} ({} de {} bytes)", t1.elapsed(), g.len(), g.total);
            // Decodificar sin seek mientras sigue bajando.
            let g2 = g.clone();
            let probe = std::thread::spawn(move || {
                let d = rodio::Decoder::builder()
                    .with_data(g2.reader())
                    .with_byte_len(g2.total)
                    .with_hint("m4a")
                    .with_seekable(false)
                    .build()
                    .map_err(|e| e.to_string())?;
                Ok::<usize, String>(rodio::Source::take_duration(d, Duration::from_secs(2)).count())
            });
            let samples = probe.join().unwrap().unwrap();
            println!("decodificado: {samples} muestras a {:?}", t1.elapsed());
            assert!(samples > 0);
            g.wait_done().await.unwrap();
            println!("completo: {:?}", t1.elapsed());
            assert_eq!(g.full().unwrap().len() as u64, g.total);
            // La segunda vez no corre yt-dlp (URL en cache).
            let t2 = Instant::now();
            resolve("dQw4w9WgXcQ", false, None).unwrap();
            assert!(t2.elapsed() < Duration::from_millis(50));
        });
    }
}
