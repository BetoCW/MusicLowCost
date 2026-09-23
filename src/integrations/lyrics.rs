//! Letras sincronizadas desde LRCLIB (port de plugins/synced-lyrics/providers/LRCLib.ts).

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Lyrics {
    /// (milisegundos, texto). Vacio si solo hay letra sin sincronizar.
    pub lines: Vec<(u64, String)>,
    pub plain: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Item {
    // LRCLIB a veces manda null en estos campos; no debe romper toda la lista.
    #[serde(default)]
    artist_name: Option<String>,
    #[serde(default)]
    duration: Option<f64>,
    #[serde(default)]
    instrumental: Option<bool>,
    plain_lyrics: Option<String>,
    synced_lyrics: Option<String>,
}

/// Diferencia de duracion; si LRCLIB no la trae, se considera muy distinta.
fn diff(d: Option<f64>, duration: f64) -> f64 {
    d.map(|d| (d - duration).abs()).unwrap_or(f64::INFINITY)
}

fn split_artists(s: &str) -> Vec<String> {
    let mut s = format!(" {} ", s.to_lowercase());
    for sep in [" feat. ", " ft. ", " feat ", " y ", " x ", " and ", " with "] {
        s = s.replace(sep, ",");
    }
    s.split(['&', ','])
        .map(|a| a.trim().to_string())
        .filter(|a| !a.is_empty())
        .collect()
}

fn artist_ratio(a: &str, b: &str) -> f64 {
    let xs = split_artists(a);
    let ys = split_artists(b);
    xs.iter()
        .flat_map(|x| ys.iter().map(move |y| strsim::jaro_winkler(x, y)))
        .fold(0.0, f64::max)
}

pub fn parse_lrc(raw: &str) -> Vec<(u64, String)> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let mut rest = line.trim();
        let mut stamps = Vec::new();
        // Una linea puede tener varias marcas: [00:12.34][01:02.00]texto
        while let Some(stripped) = rest.strip_prefix('[') {
            let Some(end) = stripped.find(']') else { break };
            let tag = &stripped[..end];
            rest = &stripped[end + 1..];
            let Some((m, s)) = tag.split_once(':') else { continue };
            let (Ok(m), Ok(s)) = (m.parse::<u64>(), s.parse::<f64>()) else { continue };
            stamps.push(m * 60_000 + (s * 1000.0).round() as u64);
        }
        for t in stamps {
            out.push((t, rest.trim().to_string()));
        }
    }
    out.sort_by_key(|(t, _)| *t);
    out
}

async fn search(http: &reqwest::Client, query: &[(&str, &str)]) -> Result<Vec<Item>, String> {
    let qs = query
        .iter()
        .map(|(k, v)| format!("{k}={}", urlencoding::encode(v)))
        .collect::<Vec<_>>()
        .join("&");
    http.get(format!("https://lrclib.net/api/search?{qs}"))
        .header("User-Agent", "YoutubeInRustWeb (https://github.com/pear-devs/pear-desktop port)")
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json::<Vec<Item>>()
        .await
        .map_err(|e| e.to_string())
}

pub async fn fetch(
    http: &reqwest::Client,
    title: &str,
    artist: &str,
    album: Option<&str>,
    duration: f64,
    show_inexact: bool,
) -> Result<Option<Lyrics>, String> {
    let mut q = vec![("artist_name", artist), ("track_name", title)];
    if let Some(a) = album {
        q.push(("album_name", a));
    }
    let mut items = search(http, &q).await?;
    if items.is_empty() && album.is_some() {
        items = search(http, &[("artist_name", artist), ("track_name", title)]).await?;
    }
    if items.is_empty() && show_inexact {
        items = search(http, &[("q", title)]).await?;
    }

    let mut matches: Vec<Item> = items
        .into_iter()
        .filter(|i| artist_ratio(artist, i.artist_name.as_deref().unwrap_or("")) > 0.9)
        .collect();
    matches.sort_by(|a, b| {
        diff(a.duration, duration).total_cmp(&diff(b.duration, duration))
    });

    let Some(best) = matches.into_iter().next() else { return Ok(None) };
    if best.instrumental == Some(true) {
        return Ok(None);
    }
    // Si la duracion no coincide (p. ej. el videoclip tiene intro) los tiempos no sirven,
    // pero la letra si: se muestra sin sincronizar en vez de "no encontrada".
    let synced_ok = diff(best.duration, duration) <= 15.0;
    let lines = if synced_ok {
        best.synced_lyrics.as_deref().map(parse_lrc).unwrap_or_default()
    } else {
        Vec::new()
    };
    if lines.is_empty() && best.plain_lyrics.is_none() {
        return Ok(None);
    }
    Ok(Some(Lyrics { lines, plain: best.plain_lyrics }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsea_lrc() {
        let l = parse_lrc("[ar:x]\n[00:01.50]Hola\n[00:10.00][01:00.00]Coro\n[00:05.25]");
        assert_eq!(
            l,
            vec![
                (1500, "Hola".to_string()),
                (5250, String::new()),
                (10000, "Coro".to_string()),
                (60000, "Coro".to_string()),
            ]
        );
    }

    #[test]
    fn compara_artistas() {
        assert!(artist_ratio("Bad Bunny & Jhay Cortez", "Bad Bunny") > 0.9);
        assert!(artist_ratio("Luis Fonsi y Daddy Yankee", "Luis Fonsi, y, Daddy Yankee") > 0.9);
        assert!(artist_ratio("Dua Lipa feat. DaBaby", "Dua Lipa") > 0.9);
        assert!(artist_ratio("Shakira", "Metallica") < 0.9);
    }
}
