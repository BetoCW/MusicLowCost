# YoutubeInRustWeb — WebView2 version (previous)

The first version of this project: a **Rust + Tauri 2** port of
[Pear Desktop](https://github.com/pear-devs/pear-desktop) that loads music.youtube.com inside the
WebView2 runtime already shipped with Windows (instead of bundling Chromium like Electron), with the
app logic (tray, media keys, Discord, scrobbling, lyrics, API, downloads) written in Rust.

It was replaced by the native version in the repository root (~25 MB instead of ~290 MB),
but it is kept here for reference.

## Memory (measured on the test machine, total private memory of all processes)

| Scenario | Pear Desktop 3.12 | This version |
|---|---|---|
| Home page loaded | ~480–615 MB | **~290–305 MB** |
| Minimized to tray | — | **~270 MB** |
| Playing a music video | — | ~317 MB |

Most of the savings come from two Chromium flags ("low memory mode", enabled by default):

- `--disable-gpu`: the GPU process goes from ~170 MB to ~20 MB (the UI is drawn by the CPU).
- `--js-flags=--optimize-for-size`: the YouTube Music renderer goes from ~305 MB to ~155 MB.

When the window is hidden or minimized, WebView2 is asked for `MemoryUsageTargetLevel = Low`
and stops painting (`IsVisible = false`); music keeps playing.

## Pear features ported

| Pear plugin | Here | Where |
|---|---|---|
| do-not-track / adblocker | ✅ ad pruning in player responses + ad-domain blocking in WebView2 | `inject/adblock.js`, `webview_tweaks.rs` |
| performance-improvement (cpu-tamer, rm3) | ✅ same scripts (MIT, CY Fung) | `inject/vendor/` |
| taskbar-mediacontrol / media keys | ✅ Windows media overlay (SMTC) | `media_controls.rs` |
| tray | ✅ | `tray.rs` |
| shortcuts | ✅ configurable global hotkeys | `shortcuts.rs` |
| discord | ✅ Rich Presence | `integrations/discord.rs` |
| scrobbler | ✅ Last.fm (with login) and ListenBrainz | `integrations/scrobbler.rs` |
| synced-lyrics | ✅ LRCLIB, floating panel | `integrations/lyrics.rs` |
| sponsorblock | ✅ | `integrations/sponsorblock.rs` |
| notifications | ✅ | `integrations/mod.rs` |
| api-server | ✅ same `/api/v1/...` routes, 127.0.0.1 only | `integrations/api_server.rs` |
| downloader | ✅ via system `yt-dlp` | `integrations/downloader.rs` |
| precise-volume, exponential-volume | ✅ | `inject/app.js` |
| equalizer, audio-compressor, skip-silences | ✅ (single AudioContext) | `inject/app.js` |
| disable-autoplay, skip-disliked-songs, playback-speed | ✅ | `inject/app.js` |
| video-toggle (hide video), blur-nav-bar | ✅ | `inject/app.js` |
| in-app-menu | replaced by the ⚙ panel (or `Ctrl+,`) | `inject/app.js` |

Not ported (visual or rarely used): album-color-theme, ambient-mode, visualizer, crossfade,
custom-output-device, captions-selector, quality-changer, picture-in-picture, music-together,
transparent-player, unobtrusive-player, clock, lumiastream, tuna-obs, amuse, touchbar (macOS).

## Usage

- Settings: ⚙ button at the top right, `Ctrl+,` or the tray menu.
- Closing the window sends it to the tray (configurable); "Salir" in the tray menu quits.
- Default hotkeys: `Ctrl+Shift+Space` play/pause, `Ctrl+Shift+→/←` next/previous,
  `Ctrl+Shift+Y` show/hide.
- Downloads require `winget install yt-dlp.yt-dlp` (also installs ffmpeg).
- Config and log: `%APPDATA%\dev.portafolio.youtubeinrustweb\` (`config.json`, `app.log`).

## Building

Requirements: Rust (MSVC) and the WebView2 runtime (ships with Windows 11).

```powershell
cargo build --release
cargo test
```

To debug the page: `$env:YIR_EXTRA_ARGS="--remote-debugging-port=9229"` and open
`edge://inspect` in Edge.

## Security

The YouTube Music page can only call the 9 `yir_*` commands declared in
`capabilities/youtube-music.json` (no filesystem or shell access). The local API rejects
requests coming from web pages (`Origin` header).
