//! Spotify (requiere cuenta Premium): sesion y reproduccion con librespot, busqueda y
//! biblioteca con la Web API oficial. El audio que decodifica librespot entra a nuestro
//! mismo motor (StreamBuf), asi que volumen, ecualizador y controles funcionan igual.

use crate::audio::StreamBuf;
use crate::model::{Card, CardKind, Track};
use librespot_core::authentication::Credentials;
use librespot_core::cache::Cache;
use librespot_core::{Session, SessionConfig, SpotifyUri};
use librespot_playback::audio_backend::{Sink, SinkResult};
use librespot_playback::config::PlayerConfig;
use librespot_playback::convert::Converter;
use librespot_playback::decoder::AudioPacket;
use librespot_playback::mixer::NoOpVolume;
use librespot_playback::player::{Player, PlayerEvent};
use serde_json::Value;
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// Client id de escritorio que usa librespot (y spotify-player, ncspot...).
const CLIENT_ID: &str = "65b708073fc0480ea92a077233ca87bd";
const REDIRECT_URI: &str = "http://127.0.0.1:8898/login";
const SCOPES: &[&str] = &[
    "streaming",
    "user-read-private",
    "user-read-email",
    "user-library-read",
    "user-read-recently-played",
    "user-top-read",
    "user-follow-read",
    "playlist-read-private",
    "playlist-read-collaborative",
];
const API: &str = "https://api.spotify.com/v1";

/// Sink de librespot que escribe en nuestro buffer.
struct BufSink(Arc<StreamBuf>);

impl Sink for BufSink {
    fn write(&mut self, packet: AudioPacket, converter: &mut Converter) -> SinkResult<()> {
        if let AudioPacket::Samples(s) = packet {
            let samples = converter.f64_to_f32(&s);
            self.0.push(&samples);
        }
        Ok(())
    }
}

struct Inner {
    session: Session,
    player: Arc<Player>,
}

pub struct Spotify {
    dir: PathBuf,
    http: reqwest::Client,
    pub buf: Arc<StreamBuf>,
    inner: RefCell<Option<Inner>>,
    token: RefCell<Option<(String, Instant)>>,
    pub user: RefCell<Option<String>>,
}

pub fn is_spotify(id: &str) -> bool {
    id.starts_with("spotify:")
}

fn short_id(uri: &str) -> &str {
    uri.rsplit(':').next().unwrap_or(uri)
}

fn oauth_client() -> Result<librespot_oauth::OAuthClient, String> {
    librespot_oauth::OAuthClientBuilder::new(CLIENT_ID, REDIRECT_URI, SCOPES.to_vec())
        .open_in_browser()
        .with_custom_message("Listo: ya puedes cerrar esta pestaña y volver a YoutubeInRustWeb.")
        .build()
        .map_err(|e| e.to_string())
}

impl Spotify {
    pub fn new(dir: PathBuf, http: reqwest::Client) -> Self {
        Self {
            dir,
            http,
            buf: StreamBuf::new(),
            inner: RefCell::new(None),
            token: RefCell::new(None),
            user: RefCell::new(None),
        }
    }

    fn cache_dir(&self) -> PathBuf {
        self.dir.join("spotify")
    }

    fn refresh_path(&self) -> PathBuf {
        self.cache_dir().join("refresh-token")
    }

    fn cache(&self) -> Result<Cache, String> {
        Cache::new(Some(self.cache_dir()), None, None, None).map_err(|e| e.to_string())
    }

    pub fn has_saved_login(&self) -> bool {
        self.cache_dir().join("credentials.json").is_file() && self.refresh_path().is_file()
    }

    pub fn is_connected(&self) -> bool {
        self.inner.borrow().as_ref().map(|i| !i.session.is_invalid()).unwrap_or(false)
    }

    /// Abre el navegador en la pagina oficial de Spotify y espera la autorizacion.
    pub async fn login(&self) -> Result<String, String> {
        let token = oauth_client()?.get_access_token_async().await.map_err(|e| e.to_string())?;
        let _ = std::fs::create_dir_all(self.cache_dir());
        std::fs::write(self.refresh_path(), &token.refresh_token).map_err(|e| e.to_string())?;
        *self.token.borrow_mut() = Some((token.access_token.clone(), token.expires_at));
        self.connect(Credentials::with_access_token(token.access_token)).await?;
        self.load_user().await
    }

    /// Reconecta con las credenciales guardadas (sin abrir el navegador).
    pub async fn ensure(&self) -> Result<(), String> {
        if self.is_connected() {
            return Ok(());
        }
        let creds = self.cache()?.credentials().ok_or("Inicia sesión en Spotify (Ajustes → Spotify).")?;
        self.connect(creds).await?;
        if self.user.borrow().is_none() {
            let _ = self.load_user().await;
        }
        Ok(())
    }

    async fn connect(&self, creds: Credentials) -> Result<(), String> {
        let session = Session::new(SessionConfig::default(), Some(self.cache()?));
        session.connect(creds, true).await.map_err(|e| {
            let msg = e.to_string();
            if msg.to_lowercase().contains("premium") {
                "Spotify necesita una cuenta Premium para reproducir fuera de su app.".to_string()
            } else {
                format!("No se pudo conectar con Spotify: {msg}")
            }
        })?;

        // Un solo reproductor para toda la sesion (librespot crea sus propios hilos).
        let existing = self.inner.borrow_mut().take();
        let player = match existing {
            Some(old) => {
                old.player.set_session(session.clone());
                old.player
            }
            None => {
                let buf = self.buf.clone();
                let player = Player::new(PlayerConfig::default(), session.clone(), Box::new(NoOpVolume), move || {
                    Box::new(BufSink(buf))
                });
                let mut events = player.get_player_event_channel();
                let buf = self.buf.clone();
                tokio::task::spawn_local(async move {
                    while let Some(ev) = events.recv().await {
                        match ev {
                            PlayerEvent::Playing { .. } | PlayerEvent::Loading { .. } => buf.end_flush(),
                            PlayerEvent::EndOfTrack { .. } => buf.ended.store(true, Ordering::Relaxed),
                            PlayerEvent::Unavailable { track_id, .. } => {
                                log::warn!("Spotify: no disponible {track_id:?}");
                                buf.ended.store(true, Ordering::Relaxed);
                            }
                            _ => {}
                        }
                    }
                });
                player
            }
        };
        *self.inner.borrow_mut() = Some(Inner { session, player });
        log::info!("Spotify conectado");
        Ok(())
    }

    pub async fn load_user(&self) -> Result<String, String> {
        let me = self.api("/me").await?;
        let name = me["display_name"].as_str().or(me["id"].as_str()).unwrap_or("").to_string();
        if me["product"].as_str().is_some_and(|p| p != "premium") {
            return Err(format!("La cuenta «{name}» no es Premium: Spotify no permite reproducir fuera de su app."));
        }
        *self.user.borrow_mut() = Some(name.clone());
        Ok(name)
    }

    pub fn logout(&self) {
        if let Some(i) = self.inner.borrow_mut().take() {
            self.buf.begin_flush(0);
            i.player.stop();
            i.session.shutdown();
        }
        *self.token.borrow_mut() = None;
        *self.user.borrow_mut() = None;
        let _ = std::fs::remove_dir_all(self.cache_dir());
    }

    // ---------- reproduccion ----------

    pub async fn play(&self, uri: &str, position_ms: u32) -> Result<(), String> {
        self.ensure().await?;
        let uri = SpotifyUri::from_uri(uri).map_err(|e| e.to_string())?;
        self.buf.begin_flush(position_ms as u64);
        if let Some(i) = self.inner.borrow().as_ref() {
            i.player.load(uri, true, position_ms);
        }
        Ok(())
    }

    pub fn seek(&self, secs: f64) {
        let ms = (secs.max(0.0) * 1000.0) as u32;
        self.buf.begin_flush(ms as u64);
        if let Some(i) = self.inner.borrow().as_ref() {
            i.player.seek(ms);
        }
    }

    pub fn stop(&self) {
        self.buf.begin_flush(0);
        if let Some(i) = self.inner.borrow().as_ref() {
            i.player.stop();
        }
    }

    // ---------- Web API ----------

    async fn access_token(&self) -> Result<String, String> {
        if let Some((t, exp)) = self.token.borrow().as_ref() {
            if Instant::now() + Duration::from_secs(60) < *exp {
                return Ok(t.clone());
            }
        }
        let refresh = std::fs::read_to_string(self.refresh_path()).map_err(|_| "Inicia sesión en Spotify.".to_string())?;
        let tok = oauth_client()?
            .refresh_token_async(refresh.trim())
            .await
            .map_err(|e| format!("Spotify: la sesión expiró, vuelve a iniciar sesión ({e})"))?;
        if !tok.refresh_token.is_empty() {
            let _ = std::fs::write(self.refresh_path(), &tok.refresh_token);
        }
        *self.token.borrow_mut() = Some((tok.access_token.clone(), tok.expires_at));
        Ok(tok.access_token)
    }

    pub async fn api(&self, path: &str) -> Result<Value, String> {
        let url = if path.starts_with("http") { path.to_string() } else { format!("{API}{path}") };
        for attempt in 0..2 {
            let token = self.access_token().await?;
            let resp = self.http.get(&url).bearer_auth(&token).send().await.map_err(|e| e.to_string())?;
            if resp.status() == reqwest::StatusCode::UNAUTHORIZED && attempt == 0 {
                *self.token.borrow_mut() = None;
                continue;
            }
            if !resp.status().is_success() {
                return Err(format!("Spotify API {}: HTTP {}", path, resp.status()));
            }
            return resp.json().await.map_err(|e| e.to_string());
        }
        Err("Spotify API: sin autorización".into())
    }

    /// Recorre la paginacion (`next`) hasta `max` elementos.
    async fn paged(&self, first: &str, max: usize) -> Vec<Value> {
        let mut out = Vec::new();
        let mut next = Some(first.to_string());
        while let Some(url) = next.take() {
            let Ok(page) = self.api(&url).await else { break };
            if let Some(items) = page["items"].as_array() {
                out.extend(items.iter().cloned());
            }
            if out.len() < max {
                next = page["next"].as_str().map(String::from);
            }
        }
        out.truncate(max);
        out
    }

    pub async fn search(&self, q: &str) -> Result<(Vec<Track>, Vec<Card>), String> {
        let v = self
            .api(&format!("/search?type=track,album,artist,playlist&limit=20&q={}", urlencoding::encode(q)))
            .await?;
        let tracks = arr(&v["tracks"]["items"]).filter_map(track).collect();
        let mut cards: Vec<Card> = arr(&v["artists"]["items"]).filter_map(artist_card).take(5).collect();
        cards.extend(arr(&v["albums"]["items"]).filter_map(album_card));
        cards.extend(arr(&v["playlists"]["items"]).filter_map(playlist_card));
        Ok((tracks, cards))
    }

    /// Inicio: escuchado recientemente + tus listas y artistas favoritos.
    pub async fn home(&self) -> Result<(Vec<Track>, Vec<Card>), String> {
        let recent = self.api("/me/player/recently-played?limit=50").await?;
        let mut seen = std::collections::HashSet::new();
        let tracks = arr(&recent["items"])
            .filter_map(|i| track(&i["track"]))
            .filter(|t| seen.insert(t.id.clone()))
            .collect();
        let mut cards: Vec<Card> = self.paged("/me/playlists?limit=50", 50).await.iter().filter_map(playlist_card).collect();
        if let Ok(top) = self.api("/me/top/artists?limit=20").await {
            cards.extend(arr(&top["items"]).filter_map(artist_card));
        }
        Ok((tracks, cards))
    }

    /// Biblioteca: canciones que te gustan + listas y albumes guardados.
    pub async fn library(&self) -> Result<(Vec<Track>, Vec<Card>), String> {
        let liked = self.paged("/me/tracks?limit=50", 500).await;
        let tracks = liked.iter().filter_map(|i| track(&i["track"])).collect();
        let mut cards: Vec<Card> = self.paged("/me/playlists?limit=50", 100).await.iter().filter_map(playlist_card).collect();
        cards.extend(self.paged("/me/albums?limit=50", 100).await.iter().filter_map(|i| album_card(&i["album"])));
        Ok((tracks, cards))
    }

    /// (titulo, subtitulo, portada, canciones, tarjetas relacionadas)
    pub async fn open(&self, kind: CardKind, uri: &str) -> Result<(String, String, Option<String>, Vec<Track>, Vec<Card>), String> {
        let id = short_id(uri);
        match kind {
            CardKind::Album => {
                let a = self.api(&format!("/albums/{id}")).await?;
                let cover = thumb_big(&a["images"]);
                let name = a["name"].as_str().unwrap_or("").to_string();
                let artists = names(&a["artists"]);
                let year = a["release_date"].as_str().unwrap_or("").chars().take(4).collect::<String>();
                let tracks: Vec<Track> = arr(&a["tracks"]["items"])
                    .filter_map(track)
                    .map(|mut t| {
                        t.thumb = t.thumb.or_else(|| cover.clone());
                        t.album = t.album.or_else(|| Some(name.clone()));
                        t
                    })
                    .collect();
                let sub = format!("Álbum • {artists} • {year} • {} canciones", tracks.len());
                Ok((name, sub, cover, tracks, vec![]))
            }
            CardKind::Playlist => {
                let p = self.api(&format!("/playlists/{id}?fields=name,owner(display_name),images,tracks(total)")).await?;
                let items = self.paged(&format!("/playlists/{id}/tracks?limit=100"), 300).await;
                let tracks: Vec<Track> = items.iter().filter_map(|i| track(&i["track"])).collect();
                let sub = format!(
                    "Lista • {} • {} canciones",
                    p["owner"]["display_name"].as_str().unwrap_or(""),
                    p["tracks"]["total"].as_u64().unwrap_or(tracks.len() as u64)
                );
                Ok((p["name"].as_str().unwrap_or("").to_string(), sub, thumb_big(&p["images"]), tracks, vec![]))
            }
            CardKind::Artist => {
                let a = self.api(&format!("/artists/{id}")).await?;
                let top = self.api(&format!("/artists/{id}/top-tracks")).await.unwrap_or_default();
                let tracks = arr(&top["tracks"]).filter_map(track).collect();
                let albums = self
                    .api(&format!("/artists/{id}/albums?include_groups=album,single&limit=40"))
                    .await
                    .unwrap_or_default();
                let cards = arr(&albums["items"]).filter_map(album_card).collect();
                let sub = format!(
                    "Artista • {} seguidores",
                    crate::model::compact(a["followers"]["total"].as_u64().unwrap_or(0))
                );
                Ok((a["name"].as_str().unwrap_or("").to_string(), sub, thumb_big(&a["images"]), tracks, cards))
            }
        }
    }
}

// ---------- JSON de la Web API -> modelos propios ----------

fn arr(v: &Value) -> impl Iterator<Item = &Value> {
    v.as_array().map(|a| a.iter()).into_iter().flatten().filter(|x| !x.is_null())
}

fn names(artists: &Value) -> String {
    arr(artists).filter_map(|a| a["name"].as_str()).collect::<Vec<_>>().join(", ")
}

/// Imagen de ~300 px para portadas grandes.
fn thumb_big(images: &Value) -> Option<String> {
    let mut imgs: Vec<(u64, &str)> =
        arr(images).filter_map(|i| Some((i["width"].as_u64().unwrap_or(0), i["url"].as_str()?))).collect();
    imgs.sort_by_key(|(w, _)| *w);
    imgs.iter().find(|(w, _)| *w >= 250).or(imgs.last()).map(|(_, u)| u.to_string())
}

/// Imagen mas chica que tenga al menos 60 px.
fn thumb(images: &Value) -> Option<String> {
    let mut imgs: Vec<(u64, &str)> =
        arr(images).filter_map(|i| Some((i["width"].as_u64().unwrap_or(0), i["url"].as_str()?))).collect();
    imgs.sort_by_key(|(w, _)| *w);
    imgs.iter().find(|(w, _)| *w >= 60).or(imgs.last()).map(|(_, u)| u.to_string())
}

fn track(t: &Value) -> Option<Track> {
    if t["type"].as_str() != Some("track") || t["is_local"].as_bool() == Some(true) {
        return None;
    }
    Some(Track {
        id: t["uri"].as_str()?.to_string(),
        title: t["name"].as_str()?.to_string(),
        artists: names(&t["artists"]),
        artist_id: t["artists"][0]["uri"].as_str().map(String::from),
        album: t["album"]["name"].as_str().map(String::from),
        duration: (t["duration_ms"].as_u64().unwrap_or(0) / 1000) as u32,
        thumb: thumb(&t["album"]["images"]),
    })
}

fn album_card(a: &Value) -> Option<Card> {
    let year: String = a["release_date"].as_str().unwrap_or("").chars().take(4).collect();
    let artists = names(&a["artists"]);
    Some(Card {
        kind: CardKind::Album,
        id: a["uri"].as_str()?.to_string(),
        title: a["name"].as_str()?.to_string(),
        subtitle: if year.is_empty() { artists } else { format!("{artists} • {year}") },
        thumb: thumb(&a["images"]),
    })
}

fn artist_card(a: &Value) -> Option<Card> {
    Some(Card {
        kind: CardKind::Artist,
        id: a["uri"].as_str()?.to_string(),
        title: a["name"].as_str()?.to_string(),
        subtitle: a["followers"]["total"]
            .as_u64()
            .map(|n| format!("{} seguidores", crate::model::compact(n)))
            .unwrap_or_default(),
        thumb: thumb(&a["images"]),
    })
}

fn playlist_card(p: &Value) -> Option<Card> {
    Some(Card {
        kind: CardKind::Playlist,
        id: p["uri"].as_str()?.to_string(),
        title: p["name"].as_str()?.to_string(),
        subtitle: format!(
            "{} • {} canciones",
            p["owner"]["display_name"].as_str().unwrap_or(""),
            p["tracks"]["total"].as_u64().unwrap_or(0)
        ),
        thumb: thumb(&p["images"]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn convierte_cancion_de_la_web_api() {
        let v = json!({
            "type": "track", "uri": "spotify:track:abc", "name": "Tití Me Preguntó",
            "duration_ms": 243716,
            "artists": [{"name": "Bad Bunny", "uri": "spotify:artist:x"}],
            "album": {"name": "Un Verano Sin Ti", "images": [
                {"url": "g", "width": 640}, {"url": "p", "width": 300}, {"url": "c", "width": 64}]}
        });
        let t = track(&v).unwrap();
        assert_eq!(t.id, "spotify:track:abc");
        assert_eq!(t.artists, "Bad Bunny");
        assert_eq!(t.duration, 243);
        assert_eq!(t.thumb.as_deref(), Some("c"));
        assert!(is_spotify(&t.id));
        assert_eq!(short_id("spotify:album:XYZ"), "XYZ");
    }

    #[test]
    fn ignora_episodios_y_nulos() {
        assert!(track(&json!({"type": "episode", "uri": "spotify:episode:1", "name": "x"})).is_none());
        assert_eq!(arr(&json!([null, {"a": 1}])).count(), 1);
    }
}
