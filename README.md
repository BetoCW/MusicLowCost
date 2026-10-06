# MusicLowCost (YoutubeInRustWeb)

A **native YouTube Music desktop player written in Rust**, inspired by
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
  (rustypipe can no longer decipher YouTube's current player). yt-dlp runs for ~2 s per song and exits
  (the unpacked `yt-dlp_win.zip` build is used: the single-file .exe re-extracts itself on every run, +2.5 s).
  Downloading (AAC/M4A into memory), decoding (symphonia) and playback (rodio) are pure Rust.
  The next song is preloaded while the current one plays.
- **Self-contained**: on first launch the app downloads yt-dlp into `%APPDATA%\YoutubeInRustWeb\bin\yt-dlp`
  and updates it every 3 days. yt-dlp needs a JavaScript runtime: Node.js is used if installed,
  otherwise deno is downloaded into the same folder.
## Features

- Home (country charts), search (songs / albums, artists, playlists), artist,
  album and playlist pages, queue with automatic radio, shuffle and repeat.
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
- **Jam**: listen together in sync with up to 8 people (see below).

> The user interface is currently in Spanish.

### Accounts (optional)

- **YouTube Music**: Settings → *Iniciar sesión con Google* opens a Google window (a separate
  WebView2 process that closes by itself once you are signed in; its data is deleted afterwards).
  Without an account everything works except the library.

### Settings

The Settings page only shows simple options and saves each change immediately. Advanced options
(equalizer, exponential volume, skip leading silence, country for the charts, global hotkeys,
ListenBrainz, local HTTP API and its port, Jam port) still work but are only edited in
`%APPDATA%\YoutubeInRustWeb\config.json` (close the app first).

### Jam (listen together)

YouTube Music has no Jam, and the YouTube Data API v3 isn't built for real-time sync (and its
quotas are tight), so the app orchestrates the session itself. YouTube is never asked to sync anything:

- The **host** already downloads each song completely into memory, so it sends those same bytes
  to the guests over TCP. YouTube sees one download per song no matter how many people join.
  Only audio is shared, never video.
- The host tells the guests which position to play at which instant *of the host's clock*. Each
  guest estimates the host's clock the way NTP does (ping/pong, keeping the lowest-delay sample) and
  seeks when it drifts more than 120 ms.
- Guests can add songs, which go into the host's queue after earlier guest requests. They can also
  play/pause, skip and seek; those requests are sent to the host.
- While a Jam is running, "skip leading silence" is turned off, because it would shift the position.

How to use it: open **Jam** in the sidebar. The host clicks *Crear Jam* and shares the address
(`IP:port`, default port `26541`) and the 6-character code. Guests type both in and click *Unirse*.
It works on the same network or over a VPN such as Tailscale or ZeroTier. To invite people over the
internet, open the port on the host's router. Windows may ask for a firewall permission the first time.

Transport is plain TCP with a small length-prefixed framing (`src/jam/protocol.rs`): both ends are
this app, so WebSocket adds nothing. WebRTC (NAT traversal) would cost many dependencies and memory.
The protocol doesn't depend on the transport, so a WebRTC data channel can be added later.
Audio chunks are written straight from the host's buffer and read straight into the guest's buffer,
with no intermediate copies. Up to 8 guests.

## Project layout

```
ui/app.slint              user interface
src/main.rs               window, callbacks, single instance
src/backend.rs            queue, playback, radio, search, lyrics, accounts
src/backend/jam_link.rs   Jam <-> player glue (host broadcasts, guest drift correction)
src/jam/                  Jam: protocol, clock sync (NTP-style), host, guest
src/audio.rs              audio engine + equalizer + silence skipping
src/stream.rs             yt-dlp (auto-download/update) + chunked download
src/images.rs             small cover art with a bounded cache
src/desktop.rs            tray, global hotkeys, media overlay (SMTC)
src/login.rs              Google sign-in window (`--login` process)
src/integrations/         discord, scrobbler, lyrics (LRCLIB), sponsorblock, local API
installer/                Inno Setup script
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
- [rustypipe](https://codeberg.org/ThetaDev/rustypipe),
  [yt-dlp](https://github.com/yt-dlp/yt-dlp), [Slint](https://slint.dev), [rodio](https://github.com/RustAudio/rodio),
  [LRCLIB](https://lrclib.net), [SponsorBlock](https://sponsor.ajay.app).

This is an unofficial client, not affiliated with YouTube or Google.

## License

GPL-3.0 (see [LICENSE](LICENSE)), because it links [rustypipe](https://codeberg.org/ThetaDev/rustypipe),
which is GPL-3.0.
