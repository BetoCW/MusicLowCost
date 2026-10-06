//! Tramas del Jam (sobre un stream de QUIC): `[u32 largo LE][u8 tipo][datos]`.
//! - tipo 0: mensaje de control en JSON (pequeno).
//! - tipo 1: pedazo de audio `[u32 tag LE][bytes]`; se lee directo al buffer de la
//!   cancion, sin copias intermedias.

use super::PlayState;
use crate::model::Track;
use serde::{Deserialize, Serialize};
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const VERSION: u32 = 2;
/// Ninguna trama de control puede pasar de esto (protege la memoria ante datos basura).
pub const MAX_FRAME: usize = 512 * 1024;
/// Tamano de cada pedazo de audio que se manda.
pub const CHUNK: usize = 64 * 1024;
/// Una cancion M4A no deberia pasar de esto (~2 h a 128 kbps).
pub const MAX_AUDIO: u64 = 128 * 1024 * 1024;

const KIND_MSG: u8 = 0;
const KIND_AUDIO: u8 = 1;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "t", rename_all = "camelCase")]
pub enum Msg {
    // invitado -> anfitrion
    Hello { v: u32, code: String, name: String },
    Ping { t0: i64 },
    Req { r: Request },
    // anfitrion -> invitado
    Welcome { host: String },
    Reject { reason: String },
    /// `rx`/`tx`: reloj del anfitrion al recibir el Ping y al mandar el Pong.
    Pong { t0: i64, rx: i64, tx: i64 },
    /// Empieza una cancion; despues llegan `size` bytes de audio con el mismo tag.
    /// `size == 0`: no se puede compartir.
    Track { tag: u32, track: Track, size: u64 },
    State { s: PlayState },
    Queue { tracks: Vec<Track> },
    Peers { names: Vec<String> },
}

/// Lo que un invitado le puede pedir al anfitrion.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "a", rename_all = "camelCase")]
pub enum Request {
    TogglePlay,
    Next,
    Previous,
    Seek { secs: f64 },
    Add { track: Track },
}

pub enum Frame {
    Msg(Msg),
    /// Hay que leer (o saltar) `len` bytes de audio a continuacion.
    Audio { tag: u32, len: usize },
}

pub fn encode(m: &Msg) -> Vec<u8> {
    let body = serde_json::to_vec(m).unwrap_or_default();
    let mut out = Vec::with_capacity(5 + body.len());
    out.extend_from_slice(&(body.len() as u32 + 1).to_le_bytes());
    out.push(KIND_MSG);
    out.extend_from_slice(&body);
    out
}

pub async fn write_msg<W: AsyncWrite + Unpin>(w: &mut W, m: &Msg) -> io::Result<()> {
    w.write_all(&encode(m)).await
}

pub async fn write_audio<W: AsyncWrite + Unpin>(w: &mut W, tag: u32, bytes: &[u8]) -> io::Result<()> {
    let mut head = [0u8; 9];
    head[..4].copy_from_slice(&(bytes.len() as u32 + 5).to_le_bytes());
    head[4] = KIND_AUDIO;
    head[5..].copy_from_slice(&tag.to_le_bytes());
    w.write_all(&head).await?;
    w.write_all(bytes).await
}

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<Frame> {
    let len = r.read_u32_le().await? as usize;
    if len == 0 {
        return Err(bad("trama vacía"));
    }
    match r.read_u8().await? {
        KIND_MSG => {
            if len - 1 > MAX_FRAME {
                return Err(bad("trama demasiado grande"));
            }
            let mut body = vec![0u8; len - 1];
            r.read_exact(&mut body).await?;
            serde_json::from_slice(&body).map(Frame::Msg).map_err(|e| bad(&e.to_string()))
        }
        KIND_AUDIO if len >= 5 && len - 5 <= CHUNK => {
            let tag = r.read_u32_le().await?;
            Ok(Frame::Audio { tag, len: len - 5 })
        }
        _ => Err(bad("trama desconocida")),
    }
}

/// Lee `len` bytes al final de `buf`.
pub async fn read_into<R: AsyncRead + Unpin>(r: &mut R, buf: &mut Vec<u8>, len: usize) -> io::Result<()> {
    let start = buf.len();
    buf.resize(start + len, 0);
    r.read_exact(&mut buf[start..]).await.map(|_| ())
}

/// Descarta `len` bytes (audio de una cancion que ya no suena).
pub async fn skip<R: AsyncRead + Unpin>(r: &mut R, mut len: usize) -> io::Result<()> {
    let mut tmp = [0u8; 4096];
    while len > 0 {
        let n = len.min(tmp.len());
        r.read_exact(&mut tmp[..n]).await?;
        len -= n;
    }
    Ok(())
}

/// Ids de YouTube: 11 caracteres [A-Za-z0-9_-]. Lo que mande un invitado termina en la
/// linea de comandos de yt-dlp, asi que no se acepta otra cosa.
pub fn is_youtube_id(id: &str) -> bool {
    id.len() == 11 && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

/// Limpia una cancion que llega del otro lado: la portada solo puede ser de los
/// servidores de imagenes de YouTube (si no, la app descargaria cualquier URL)
/// y el id de artista solo caracteres normales.
pub fn sanitize(t: &mut Track) {
    const HOSTS: [&str; 3] = ["googleusercontent.com", "ytimg.com", "ggpht.com"];
    let thumb_ok = t.thumb.as_deref().is_some_and(|u| {
        let host = u.strip_prefix("https://").and_then(|r| r.split(['/', '?', '#']).next()).unwrap_or("");
        HOSTS.iter().any(|d| host == *d || host.strip_suffix(d).is_some_and(|p| p.ends_with('.')))
    });
    if !thumb_ok {
        t.thumb = None;
    }
    t.artist_id = t
        .artist_id
        .take()
        .filter(|id| id.len() <= 64 && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track() -> Track {
        Track {
            id: "dQw4w9WgXcQ".into(),
            title: "t".into(),
            artists: "a".into(),
            artist_id: None,
            album: None,
            duration: 212,
            thumb: None,
        }
    }

    #[tokio::test]
    async fn ida_y_vuelta_de_mensajes_y_audio() {
        let mut buf: Vec<u8> = Vec::new();
        write_msg(&mut buf, &Msg::Track { tag: 7, track: track(), size: 3 }).await.unwrap();
        write_audio(&mut buf, 7, &[1, 2, 3]).await.unwrap();
        write_msg(&mut buf, &Msg::Ping { t0: -5 }).await.unwrap();

        let mut r = buf.as_slice();
        match read_frame(&mut r).await.unwrap() {
            Frame::Msg(Msg::Track { tag: 7, track: t, size: 3 }) => assert_eq!(t, track()),
            _ => panic!("se esperaba Track"),
        }
        let Frame::Audio { tag: 7, len: 3 } = read_frame(&mut r).await.unwrap() else { panic!("se esperaba audio") };
        let mut data = Vec::new();
        read_into(&mut r, &mut data, 3).await.unwrap();
        assert_eq!(data, [1, 2, 3]);
        assert!(matches!(read_frame(&mut r).await.unwrap(), Frame::Msg(Msg::Ping { t0: -5 })));
        assert!(read_frame(&mut r).await.is_err()); // fin
    }

    #[tokio::test]
    async fn rechaza_tramas_enormes_o_desconocidas() {
        let mut big = Vec::new();
        big.extend_from_slice(&(MAX_FRAME as u32 + 2).to_le_bytes());
        big.push(KIND_MSG);
        assert!(read_frame(&mut big.as_slice()).await.is_err());

        let mut audio = Vec::new();
        audio.extend_from_slice(&(CHUNK as u32 + 6).to_le_bytes());
        audio.push(KIND_AUDIO);
        assert!(read_frame(&mut audio.as_slice()).await.is_err());

        let raro = [1u8, 0, 0, 0, 9];
        assert!(read_frame(&mut raro.as_slice()).await.is_err());
    }

    #[test]
    fn limpia_portadas_y_artistas_ajenos() {
        let mut t = track();
        t.thumb = Some("https://lh3.googleusercontent.com/abc=w60".into());
        t.artist_id = Some("UC123_-x".into());
        sanitize(&mut t);
        assert!(t.thumb.is_some() && t.artist_id.is_some());
        for bad in ["http://lh3.googleusercontent.com/a", "https://evil.com/x", "https://evilytimg.com/x", "https://192.168.1.1/"] {
            t.thumb = Some(bad.into());
            sanitize(&mut t);
            assert_eq!(t.thumb, None, "{bad}");
        }
        t.artist_id = Some("../../x".into());
        sanitize(&mut t);
        assert_eq!(t.artist_id, None);
    }

    #[test]
    fn solo_ids_de_youtube_validos() {
        assert!(is_youtube_id("dQw4w9WgXcQ"));
        assert!(is_youtube_id("-abc_DEF123"));
        assert!(!is_youtube_id("spotify:track:x"));
        assert!(!is_youtube_id("abc&list=xyz"));
        assert!(!is_youtube_id("short"));
    }
}
