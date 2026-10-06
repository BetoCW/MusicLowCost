//! Tipos propios (ligeros) a partir de los modelos de rustypipe.

use rustypipe::model::{
    AlbumItem, ArtistItem, MusicItem, MusicPlaylistItem, Thumbnail, TrackItem,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Track {
    pub id: String,
    pub title: String,
    pub artists: String,
    pub artist_id: Option<String>,
    pub album: Option<String>,
    pub duration: u32,
    pub thumb: Option<String>,
}

impl Track {
    pub fn url(&self) -> String {
        format!("https://music.youtube.com/watch?v={}", self.id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardKind {
    Album,
    Artist,
    Playlist,
}

impl CardKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CardKind::Album => "album",
            CardKind::Artist => "artist",
            CardKind::Playlist => "playlist",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            CardKind::Album => "Álbum",
            CardKind::Artist => "Artista",
            CardKind::Playlist => "Lista",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "album" => Some(CardKind::Album),
            "artist" => Some(CardKind::Artist),
            "playlist" => Some(CardKind::Playlist),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Card {
    pub kind: CardKind,
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub thumb: Option<String>,
}

/// Elige la miniatura mas chica que tenga al menos `min` px y la pide en ese tamano
/// cuando es de googleusercontent (las portadas de YouTube Music aceptan `=wN-hN`).
pub fn pick_thumb(thumbs: &[Thumbnail], min: u32) -> Option<String> {
    let t = thumbs
        .iter()
        .filter(|t| t.width >= min)
        .min_by_key(|t| t.width)
        .or_else(|| thumbs.iter().max_by_key(|t| t.width))?;
    Some(resize_thumb(&t.url, min))
}

pub fn resize_thumb(url: &str, size: u32) -> String {
    if url.contains("googleusercontent.com") {
        if let Some(pos) = url.rfind('=') {
            // -rj fuerza JPEG (no hace falta decodificador WebP).
            return format!("{}=w{size}-h{size}-l90-rj", &url[..pos]);
        }
    }
    url.to_string()
}

pub fn from_track(t: TrackItem) -> Track {
    let artists = t.artists.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ");
    Track {
        thumb: pick_thumb(&t.cover, 60),
        artist_id: t.artist_id.or_else(|| t.artists.iter().find_map(|a| a.id.clone())),
        album: t.album.map(|a| a.name),
        duration: t.duration.unwrap_or(0),
        title: t.name,
        artists,
        id: t.id,
    }
}

pub fn from_album(a: AlbumItem) -> Card {
    let artists = a.artists.iter().map(|x| x.name.as_str()).collect::<Vec<_>>().join(", ");
    let subtitle = match a.year {
        Some(y) if !artists.is_empty() => format!("{artists} • {y}"),
        Some(y) => y.to_string(),
        None => artists,
    };
    Card { kind: CardKind::Album, thumb: pick_thumb(&a.cover, 60), id: a.id, title: a.name, subtitle }
}

pub fn from_artist(a: ArtistItem) -> Card {
    let subtitle = a
        .subscriber_count
        .map(|n| format!("{} suscriptores", compact(n)))
        .unwrap_or_default();
    Card { kind: CardKind::Artist, thumb: pick_thumb(&a.avatar, 60), id: a.id, title: a.name, subtitle }
}

pub fn from_playlist(p: MusicPlaylistItem) -> Card {
    let mut subtitle = p.channel.map(|c| c.name).unwrap_or_default();
    if let Some(n) = p.track_count {
        if !subtitle.is_empty() {
            subtitle.push_str(" • ");
        }
        subtitle.push_str(&format!("{n} canciones"));
    }
    Card { kind: CardKind::Playlist, thumb: pick_thumb(&p.thumbnail, 60), id: p.id, title: p.name, subtitle }
}

/// Separa un resultado de busqueda mixto en canciones y tarjetas.
pub fn split_items(items: Vec<MusicItem>) -> (Vec<Track>, Vec<Card>) {
    let mut tracks = Vec::new();
    let mut cards = Vec::new();
    for it in items {
        match it {
            MusicItem::Track(t) => tracks.push(from_track(t)),
            MusicItem::Album(a) => cards.push(from_album(a)),
            MusicItem::Artist(a) => cards.push(from_artist(a)),
            MusicItem::Playlist(p) => cards.push(from_playlist(p)),
            _ => {}
        }
    }
    (tracks, cards)
}

pub fn compact(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1} mil", n as f64 / 1_000.0),
        _ => format!("{:.1} M", n as f64 / 1_000_000.0),
    }
}

pub fn fmt_time(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatea_tiempo() {
        assert_eq!(fmt_time(0.0), "0:00");
        assert_eq!(fmt_time(229.4), "3:49");
        assert_eq!(fmt_time(3725.0), "1:02:05");
    }

    #[test]
    fn redimensiona_portadas_de_google() {
        assert_eq!(
            resize_thumb("https://lh3.googleusercontent.com/abc=w60-h60-l90-rj", 120),
            "https://lh3.googleusercontent.com/abc=w120-h120-l90-rj"
        );
        assert_eq!(resize_thumb("https://i.ytimg.com/vi/x/hq.jpg", 120), "https://i.ytimg.com/vi/x/hq.jpg");
    }
}
