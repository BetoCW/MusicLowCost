# MusicLowCost (YoutubeInRustWeb)

A **native YouTube Music desktop player written in Rust**, inspired by
[Pear Desktop](https://github.com/pear-devs/pear-desktop). The goal: the same experience while
using a fraction of the memory.

| | Private memory | Task Manager (working set) |
|---|---|---|
| Pear Desktop (Electron) | ~480–615 MB | ~700–835 MB |
| Previous WebView2 version (`version-webview/`) | ~290 MB | — |
| **This app, while playing** | **~25–35 MB** | **~45–60 MB** |

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
  Resolved URLs are cached (they last ~6 h), so going back to a song doesn't run yt-dlp again, and
  yt-dlp is warmed up at launch (its first run after boot is slow).
  Downloading (AAC/M4A into memory, in chunks), decoding (symphonia) and playback (rodio) are pure Rust.
  **Playback starts with the first chunk** (~0.4 MB, usually < 0.2 s) while the rest keeps downloading;
  the first seek reopens the song once it is complete (YouTube's fragmented M4A can't be opened
  seekable until then). The next song is preloaded while the current one plays.
- **"Sign in to confirm you're not a bot"**: skipping many songs quickly makes YouTube ask for this.
  The app waits a moment before resolving when you skip fast, and if YouTube still asks, it retries
  once with your YouTube session (if you're signed in) instead of jumping to the next song.
- **Self-contained**: on first launch the app downloads yt-dlp into `%APPDATA%\YoutubeInRustWeb\bin\yt-dlp`
  and updates it every 3 days. yt-dlp needs a JavaScript runtime: Node.js is used if installed,
  otherwise deno is downloaded into the same folder.
- **One-click updates**: the **UpDate** button (top right) turns red when a newer release is on
  GitHub. Clicking it downloads the installer, runs it silently and reopens the app.

## Features

- Home (country charts), search as you type (songs / albums, artists, playlists), artist,
  album and playlist pages, back / forward arrows between screens.
- Queue: *+* adds a song right after the current one (after any songs you added before it);
  press and drag a song to reorder it; automatic radio ("similar songs", can be turned off),
  shuffle and repeat.
- Player bar buttons to jump **10 s back / forward** in the current song.
- Synced lyrics (LRCLIB) or YouTube Music's official lyrics.
- Windows media overlay and hardware media keys, tray icon, global hotkeys
  (`Ctrl+Shift+Space`, `Ctrl+Shift+←/→`, `Ctrl+Shift+Y`).
- 10-band equalizer with presets (Settings), "skip leading silence" (custom DSP in `src/audio.rs`),
  exponential volume, mouse-wheel volume, high/low audio quality.
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
  Before copying the cookies the window moves to a page without JavaScript, so YouTube doesn't
  rotate (and invalidate) them right away. Without an account everything works except the library
  (liked songs, saved playlists and albums); signed in, every request is authenticated, so private
  playlists and the "Liked Music" card work too.

### Settings

The Settings page only shows simple options (and the equalizer) and saves each change immediately.
Advanced options (exponential volume, skip leading silence, country for the charts, global hotkeys,
ListenBrainz, local HTTP API and its port) still work but are only edited in
`%APPDATA%\YoutubeInRustWeb\config.json` (close the app first).

### Jam (listen together)

YouTube Music has no Jam, and the YouTube Data API v3 isn't built for real-time sync (and its
quotas are tight), so the app orchestrates the session itself. YouTube is never asked to sync anything:

- The **host** already downloads each song into memory, so once it is complete it sends those same
  bytes to the guests. YouTube sees one download per song no matter how many people join.
  Only audio is shared, never video.
- The host tells the guests which position to play at which instant *of the host's clock*. Each
  guest estimates the host's clock the way NTP does (ping/pong, keeping the lowest-delay sample) and
  seeks when it drifts more than 120 ms.
- Guests can add songs, which go into the host's queue after earlier guest requests. They can also
  play/pause, skip and seek; those requests are sent to the host.
- While a Jam is running, "skip leading silence" is turned off, because it would shift the position.

How to use it: open **Jam** in the sidebar. The host picks a **Jam name and a password**, clicks
*Crear Jam* and tells them to the others; guests type the same two and click *Unirme*. It works over
the internet from different places: no IP addresses, no router ports, no firewall prompts.

Transport is [iroh](https://github.com/n0-computer/iroh) (QUIC). The host's key is derived from
the Jam name + password (SHA-256), so a guest computes the same endpoint id and finds the host through
iroh's public directory (n0's DNS/pkarr). iroh tries a direct connection (hole punching) and falls
back to its public relays; everything is end-to-end encrypted. Opening a Jam that already exists
(same name and password) is refused. The framing (`src/jam/protocol.rs`) runs over one bidirectional
QUIC stream; audio chunks are written straight from the host's buffer and read straight into the
guest's buffer, with no intermediate copies. Up to 8 guests.

## Project layout

```
ui/app.slint              user interface
src/main.rs               window, callbacks, single instance
src/backend.rs            queue, playback, radio, search, lyrics, accounts
src/backend/jam_link.rs   Jam <-> player glue (host broadcasts, guest drift correction)
src/jam/                  Jam: protocol, clock sync (NTP-style), host, guest, iroh transport (net.rs)
src/audio.rs              audio engine + equalizer + silence skipping
src/stream.rs             yt-dlp (auto-download/update, URL cache) + streaming chunked download
src/update.rs             UpDate button: checks GitHub releases, runs the installer silently
src/images.rs             small cover art with a bounded cache
src/desktop.rs            tray, global hotkeys, media overlay (SMTC)
src/login.rs              Google sign-in window (`--login` process)
src/integrations/         discord, scrobbler, lyrics (LRCLIB), sponsorblock, local API
src/bench.rs              dev only: performance measurements (does nothing unless YIR_BENCH_FILE is set)
installer/                Inno Setup script
vendor/                   two crates with a small patch each (search "YoutubeInRustWeb" in them):
                            rustypipe: user playlists without header (YouTube 2026-10), no panic
                            i-slint-backend-winit: always repaint/present the whole window
                              (and, with YIR_BENCH_FILE, log the time of each frame)
scripts/bench.ps1         dev only: runs the benchmark scenarios (see "Benchmarks")
scripts/grafo/            dev only: code graph + semantic search (see CLAUDE.md)
version-webview/          previous version (Tauri + WebView2, ~290 MB)
```

Why the Slint patch: with partial repaints, when Windows invalidated the window (Tab focus changes,
another window on top) and Slint saw no change, nothing was copied to the screen, so stale pixels
(an old song title, half-drawn rows) stayed until the window was minimized and restored.

## Building

Requirements: Rust (MSVC toolchain) and the WebView2 runtime (ships with Windows 11).

```powershell
cargo build --release
cargo test
cargo test -- --ignored   # needs network: Jam over the internet, real streaming, playlists/liked songs
```

The C runtime is linked statically (`.cargo/config.toml`), so the `.exe` does not need the
Visual C++ Redistributable.

### Installer

```powershell
cargo build --release
& "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe" installer\YoutubeInRustWeb.iss
```

Produces `installer\Output\YoutubeInRustWeb-Setup.exe` (~9 MB).

### Testing without touching your real settings

```powershell
$env:YIR_DATA_DIR = "C:\some\temp\folder"   # separate data folder, no single-instance lock
```

Data and log: `%APPDATA%\YoutubeInRustWeb\` (`config.json`, `session.json`, `app.log`).

## Benchmarks

`scriptsench.ps1` launches the release build with a separate data folder, drives it by itself
(`start`, `scroll`, `play`, `latency`, `seek` scenarios; see `src/bench.rs`) and reports frame times,
memory and click-to-sound latency:

```powershell
powershell -Command "& .scriptsench.ps1 -Runs 5 -Scenarios start,scroll,play"
powershell -Command "& .scriptsench.ps1 -Runs 10 -Scenarios latency"
```

Results so far: [`BENCHMARK_BASELINE.md`](BENCHMARK_BASELINE.md) (rendering, memory, startup, binary size)
and [`BENCHMARK_LATENCY.md`](BENCHMARK_LATENCY.md) (click to sound: ~2.2 s, ~89 % of it is yt-dlp).

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
