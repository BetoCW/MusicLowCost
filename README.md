# MusicLowCost (YoutubeInRustWeb)

A **native YouTube Music + Spotify desktop player written in Rust**, inspired by
[Pear Desktop](https://github.com/pear-devs/pear-desktop). The goal: the same experience while
using a fraction of the memory.

| | Private memory | Task Manager (working set) |
|---|---|---|
| Pear Desktop (Electron) | ~480–615 MB | ~700–835 MB |
| Previous WebView2 version (`version-webview/`) | ~290 MB | — |
| **This app, while playing** | **~21–25 MB** | **~42–55 MB** |

## Download

Grab `YoutubeInRustWeb-Setup.exe` from the [latest release](https://github.com/BetoCW/MusicLowCost/releases/latest).

- Installs per user (no administrator rights), with desktop/Start menu shortcuts and an uninstaller.
- Windows 10/11, x64. Nothing else to install.
- The installer is not code-signed, so SmartScreen will warn you:
  click **More info → Run anyway**.

## How it works

- **UI**: [Slint](https://slint.dev) with the software renderer (no OpenGL/DirectX; on the test machine
  the GPU driver alone used ~100 MB just to open an OpenGL context).
- **YouTube Music catalog** (home, search, artists, albums, playlists, radio, lyrics, library):
  [rustypipe](https://codeberg.org/ThetaDev/rustypipe) talking to YouTube Music's internal API.
- **YouTube audio**: the stream URL is resolved by [yt-dlp](https://github.com/yt-dlp/yt-dlp)
  (rustypipe can no longer decipher YouTube's current player). yt-dlp runs for ~4 s per song and exits.
  Downloading (AAC/M4A into memory), decoding (symphonia) and playback (rodio) are pure Rust.
  The next song is preloaded while the current one plays.
- **Self-contained**: on first launch the app downloads yt-dlp into `%APPDATA%\YoutubeInRustWeb\bin`
  and updates it every 3 days. yt-dlp needs a JavaScript runtime: Node.js is used if installed,
  otherwise deno is downloaded into the same folder.
- **Spotify** (Premium required): session and playback via
  [librespot](https://github.com/librespot-org/librespot); search and library via the Spotify Web API.
  librespot's decoded audio is fed into the same engine (`StreamBuf` in `src/audio.rs`), so volume,
  EQ and media controls behave identically for both sources.

## Features

- **YouTube / Spotify switch** in the sidebar: home, search, library and queue follow the active source,
  and the accent color changes (red for YouTube Music, green for Spotify). Each source keeps its own
  queue; the other one is parked and restored when you come back.
- Home (country charts / recently played), search (songs / albums, artists, playlists), artist,
  album and playlist pages, queue with automatic radio (YouTube Music), shuffle and repeat.
- Synced lyrics (LRCLIB) or YouTube Music's official lyrics.
- Windows media overlay and hardware media keys, tray icon, global hotkeys
  (`Ctrl+Shift+Space`, `Ctrl+Shift+←/→`, `Ctrl+Shift+Y`).
- 10-band equalizer and "skip leading silence" (custom DSP in `src/audio.rs`), exponential volume,
  mouse-wheel volume, high/low audio quality.
- SponsorBlock (non-music segments in music videos), Discord Rich Presence, Last.fm and ListenBrainz
  scrobbling, notifications, local HTTP API compatible with Pear (`/api/v1/...`, 127.0.0.1 only).
- Remembers the queue and position on exit (never autoplays on launch).
- No ads: the web page is never loaded, only the audio.
- Single instance: launching it again brings the running window to the front.

> The user interface is currently in Spanish.

### Accounts (optional)

- **YouTube Music**: Settings → *Iniciar sesión con Google* opens a Google window (a separate
  WebView2 process that closes by itself once you are signed in; its data is deleted afterwards).
  Without an account everything works except the library.
- **Spotify**: Settings → *Iniciar sesión con Spotify* opens Spotify's official page in your browser
  (OAuth). The session is stored in `%APPDATA%\YoutubeInRustWeb\spotify`.
  There is no automatic radio for Spotify (Spotify no longer offers it to third-party apps).

> **Spotify playback requires a Premium account.** Spotify only delivers audio to third-party
> clients for Premium users. With a Free account you can still sign in, search and browse your
> playlists, but trying to play shows a clear notice instead of playing.

#### Spotify "HTTP 429 Too Many Requests"

By default the Web API is called with the same Client ID used by librespot and other open-source
clients, so its rate limit is shared with everyone using them. The app caches responses for 10 minutes
and honours `Retry-After`, but if you still get 429 errors, use your own Client ID:

1. Go to <https://developer.spotify.com/dashboard> → **Create app**.
2. Redirect URI: `http://127.0.0.1:8898/login` — API: **Web API**.
3. In **User Management**, add the email of every Spotify account that will use it (development
   mode allows up to 25 users).
4. Copy the **Client ID** into Settings → Spotify → *Client ID propio*, click *Guardar* and sign in again.

Playback itself always goes through librespot and is not affected by this limit.

## Project layout

```
ui/app.slint              user interface
src/main.rs               window, callbacks, single instance
src/backend.rs            queue, playback, radio, search, lyrics, accounts
src/audio.rs              audio engine + equalizer + silence skipping + Spotify stream buffer
src/stream.rs             yt-dlp (auto-download/update) + chunked download
src/spotify.rs            Spotify: session/playback (librespot) + Web API
src/images.rs             small cover art with a bounded cache
src/desktop.rs            tray, global hotkeys, media overlay (SMTC)
src/login.rs              Google sign-in window (`--login` process)
src/integrations/         discord, scrobbler, lyrics (LRCLIB), sponsorblock, local API
installer/                Inno Setup script
vendor/librespot-core/    librespot-core 0.8.0 with one change: a Free account no longer calls exit(1)
version-webview/          previous version (Tauri + WebView2, ~290 MB)
```

## Building

Requirements: Rust (MSVC toolchain) and the WebView2 runtime (ships with Windows 11).

```powershell
cargo build --release
cargo test
```

The C runtime is linked statically (`.cargo/config.toml`), so the `.exe` does not need the
Visual C++ Redistributable.

### Installer

```powershell
cargo build --release
& "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe" installer\YoutubeInRustWeb.iss
```

Produces `installer\Output\YoutubeInRustWeb-Setup.exe` (~8 MB).

### Testing without touching your real settings

```powershell
$env:YIR_DATA_DIR = "C:\some\temp\folder"   # separate data folder, no single-instance lock
```

Data and log: `%APPDATA%\YoutubeInRustWeb\` (`config.json`, `session.json`, `app.log`).

## Icon

`icons/iconR.png` is the original. `icon.ico`, `icon.png` and the tray icons are generated from it;
the `.ico` is embedded in the `.exe` (`build.rs`). Previous icons are in `icons/old/`.

## Credits

- [Pear Desktop](https://github.com/pear-devs/pear-desktop) (MIT) — the original app this is based on.
- Performance scripts `cpu-tamer` and `rm3` by CY Fung (MIT), used by the WebView version.
- [rustypipe](https://codeberg.org/ThetaDev/rustypipe), [librespot](https://github.com/librespot-org/librespot),
  [yt-dlp](https://github.com/yt-dlp/yt-dlp), [Slint](https://slint.dev), [rodio](https://github.com/RustAudio/rodio),
  [LRCLIB](https://lrclib.net), [SponsorBlock](https://sponsor.ajay.app).

This is an unofficial client, not affiliated with YouTube, Google or Spotify.

## License

GPL-3.0 (see [LICENSE](LICENSE)), because it links [rustypipe](https://codeberg.org/ThetaDev/rustypipe),
which is GPL-3.0.
