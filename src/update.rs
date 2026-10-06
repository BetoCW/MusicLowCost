//! Actualizaciones con un clic: se compara la version con el ultimo release de GitHub y,
//! si hay una nueva, se baja el instalador y se corre en silencio. El instalador cierra
//! la app, la actualiza y la vuelve a abrir (ver `[Run]` en installer/YoutubeInRustWeb.iss).

use serde::Deserialize;

const LATEST_URL: &str = "https://api.github.com/repos/BetoCW/MusicLowCost/releases/latest";
const ASSET: &str = "YoutubeInRustWeb-Setup.exe";

pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone)]
pub struct Release {
    pub version: String,
    pub url: String,
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

/// "v0.10.2" -> [0, 10, 2]
fn parse(v: &str) -> Vec<u32> {
    v.trim().trim_start_matches(['v', 'V']).split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

pub fn is_newer(latest: &str, current: &str) -> bool {
    parse(latest) > parse(current)
}

/// Ultimo release publicado (con su instalador).
pub async fn latest(http: &reqwest::Client) -> Result<Release, String> {
    let r: GhRelease = http
        .get(LATEST_URL)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let url = r
        .assets
        .into_iter()
        .find(|a| a.name == ASSET)
        .map(|a| a.browser_download_url)
        .ok_or("el release no trae el instalador")?;
    Ok(Release { version: r.tag_name.trim_start_matches(['v', 'V']).to_string(), url })
}

/// Baja el instalador y lo lanza en silencio. Despues hay que cerrar la app.
pub async fn install(http: &reqwest::Client, rel: &Release) -> Result<(), String> {
    if !rel.url.starts_with("https://github.com/BetoCW/MusicLowCost/releases/download/") {
        return Err("dirección de descarga inesperada".into());
    }
    let bytes = http
        .get(&rel.url)
        .timeout(std::time::Duration::from_secs(600))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .bytes()
        .await
        .map_err(|e| e.to_string())?;
    let path = std::env::temp_dir().join(format!("YoutubeInRustWeb-Setup-{}.exe", rel.version));
    std::fs::write(&path, &bytes).map_err(|e| format!("no se pudo guardar el instalador: {e}"))?;
    std::process::Command::new(&path)
        .args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/CLOSEAPPLICATIONS"])
        .spawn()
        .map_err(|e| format!("no se pudo abrir el instalador: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compara_versiones() {
        assert!(is_newer("v0.5.0", "0.4.0"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(!is_newer("v0.4.0", "0.4.0"));
        assert!(!is_newer("0.3.9", "0.4.0"));
    }
}
