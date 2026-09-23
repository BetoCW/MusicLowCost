//! Scrobbling a Last.fm y ListenBrainz (port de plugins/scrobbler de Pear).

use crate::config::{Config, LastFm};
use crate::state::{AppState, SongInfo};
use md5::{Digest, Md5};
use serde_json::json;
use tauri::{AppHandle, Manager, Runtime};

const LASTFM_API: &str = "https://ws.audioscrobbler.com/2.0/";
const LISTENBRAINZ_API: &str = "https://api.listenbrainz.org/1/submit-listens";

/// Regla oficial de Last.fm: la cancion dura mas de 30 s y se escucho la mitad o 4 minutos.
pub fn should_scrobble(duration: f64, listened: f64) -> bool {
    duration > 30.0 && listened >= (duration / 2.0).min(240.0)
}

fn sign(params: &mut Vec<(String, String)>, secret: &str) {
    params.sort_by(|a, b| a.0.cmp(&b.0));
    let mut raw = String::new();
    for (k, v) in params.iter() {
        raw.push_str(k);
        raw.push_str(v);
    }
    raw.push_str(secret);
    let hash = Md5::digest(raw.as_bytes());
    let sig: String = hash.iter().map(|b| format!("{b:02x}")).collect();
    params.push(("api_sig".into(), sig));
    params.push(("format".into(), "json".into()));
}

fn form_body(params: &[(String, String)]) -> String {
    params
        .iter()
        .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

async fn lastfm_call(
    http: &reqwest::Client,
    cfg: &LastFm,
    method: &str,
    extra: Vec<(&str, String)>,
) -> Result<serde_json::Value, String> {
    let mut params: Vec<(String, String)> = vec![
        ("method".into(), method.into()),
        ("api_key".into(), cfg.api_key.clone()),
    ];
    params.extend(extra.into_iter().map(|(k, v)| (k.to_string(), v)));
    sign(&mut params, &cfg.secret);
    let resp = http
        .post(LASTFM_API)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form_body(&params))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let value: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    if let Some(err) = value.get("message").filter(|_| value.get("error").is_some()) {
        return Err(err.to_string());
    }
    Ok(value)
}

fn track_params(song: &SongInfo) -> Vec<(&'static str, String)> {
    let mut p = vec![("artist", song.artist.clone()), ("track", song.title.clone())];
    if let Some(album) = &song.album {
        p.push(("album", album.clone()));
    }
    if song.duration > 0.0 {
        p.push(("duration", (song.duration as u64).to_string()));
    }
    p
}

async fn listenbrainz(http: &reqwest::Client, token: &str, kind: &str, song: &SongInfo, at: Option<u64>) {
    let mut listen = json!({
        "track_metadata": {
            "artist_name": song.artist,
            "track_name": song.title,
            "additional_info": {
                "duration_ms": (song.duration * 1000.0) as u64,
                "origin_url": song.url(),
                "media_player": "YoutubeInRustWeb",
                "submission_client": "YoutubeInRustWeb",
                "music_service": "music.youtube.com"
            }
        }
    });
    if let Some(album) = &song.album {
        listen["track_metadata"]["release_name"] = json!(album);
    }
    if let Some(at) = at {
        listen["listened_at"] = json!(at);
    }
    let body = json!({ "listen_type": kind, "payload": [listen] });
    let res = http
        .post(LISTENBRAINZ_API)
        .header("Authorization", format!("Token {token}"))
        .json(&body)
        .send()
        .await;
    if let Err(e) = res {
        log::warn!("ListenBrainz: {e}");
    }
}

pub async fn now_playing(http: reqwest::Client, cfg: Config, song: SongInfo) {
    if cfg.lastfm.enabled {
        if let Some(sk) = cfg.lastfm.session_key.clone() {
            let mut p = track_params(&song);
            p.push(("sk", sk));
            if let Err(e) = lastfm_call(&http, &cfg.lastfm, "track.updateNowPlaying", p).await {
                log::warn!("Last.fm nowPlaying: {e}");
            }
        }
    }
    if cfg.listenbrainz.enabled && !cfg.listenbrainz.token.is_empty() {
        listenbrainz(&http, &cfg.listenbrainz.token, "playing_now", &song, None).await;
    }
}

pub async fn scrobble(http: reqwest::Client, cfg: Config, song: SongInfo, started_unix: u64) {
    if cfg.lastfm.enabled {
        if let Some(sk) = cfg.lastfm.session_key.clone() {
            let mut p = track_params(&song);
            p.push(("timestamp", started_unix.to_string()));
            p.push(("sk", sk));
            match lastfm_call(&http, &cfg.lastfm, "track.scrobble", p).await {
                Ok(_) => log::info!("scrobble Last.fm: {} - {}", song.artist, song.title),
                Err(e) => log::warn!("Last.fm scrobble: {e}"),
            }
        }
    }
    if cfg.listenbrainz.enabled && !cfg.listenbrainz.token.is_empty() {
        listenbrainz(&http, &cfg.listenbrainz.token, "single", &song, Some(started_unix)).await;
    }
}

/// Flujo de autorizacion de Last.fm: pide un token, abre el navegador para que el usuario
/// acepte y espera (hasta 3 minutos) a que la sesion quede aprobada.
pub async fn connect_lastfm<R: Runtime>(app: AppHandle<R>) -> Result<String, String> {
    let state = app.state::<AppState>();
    let http = state.http.clone();
    let cfg = state.config().lastfm;

    let token = lastfm_call(&http, &cfg, "auth.getToken", vec![])
        .await?
        .get("token")
        .and_then(|t| t.as_str())
        .map(String::from)
        .ok_or("Last.fm no devolvio token")?;

    let url = format!("https://www.last.fm/api/auth/?api_key={}&token={}", cfg.api_key, token);
    crate::util::open_url(&url);

    for _ in 0..60 {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        if let Ok(v) = lastfm_call(&http, &cfg, "auth.getSession", vec![("token", token.clone())]).await {
            if let Some(sk) = v.pointer("/session/key").and_then(|k| k.as_str()) {
                let user = v.pointer("/session/name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                {
                    let mut c = state.config.write().unwrap();
                    c.lastfm.session_key = Some(sk.to_string());
                    c.lastfm.enabled = true;
                    let _ = crate::config::save(&state.config_dir, &c);
                }
                return Ok(user);
            }
        }
    }
    Err("Tiempo agotado esperando la autorizacion de Last.fm".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regla_de_scrobble() {
        assert!(!should_scrobble(20.0, 20.0));
        assert!(!should_scrobble(200.0, 99.0));
        assert!(should_scrobble(200.0, 100.0));
        assert!(should_scrobble(900.0, 240.0));
    }

    #[test]
    fn firma_lastfm_ordena_y_termina_en_json() {
        let mut p = vec![("method".into(), "auth.getToken".into()), ("api_key".into(), "k".into())];
        sign(&mut p, "s");
        // md5("api_keykmethodauth.getTokens")
        assert_eq!(p[2].0, "api_sig");
        assert_eq!(p[2].1.len(), 32);
        assert_eq!(p[3], ("format".into(), "json".into()));
    }
}
