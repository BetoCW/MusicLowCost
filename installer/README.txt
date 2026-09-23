YoutubeInRustWeb 0.2.0  (MusicLowCost)
======================================

Native YouTube Music + Spotify player written in Rust.
Uses ~20-60 MB of RAM (the official Spotify / YouTube Music apps: 300-700 MB).
Source code: https://github.com/BetoCW/MusicLowCost

FIRST RUN
---------
1. On first launch the app downloads "yt-dlp" by itself (the component that fetches
   YouTube audio). If Node.js is not installed it also downloads "deno" (~45 MB).
   This only happens once; files go to %APPDATA%\YoutubeInRustWeb\bin.

2. SPOTIFY (requires a PREMIUM account):
   - In the top-left corner choose "Spotify".
   - Open Settings ("Ajustes") -> "Iniciar sesión con Spotify".
   - Spotify's official page opens in your browser: accept and go back to the app.
   - The session is remembered for next time.

3. YOUTUBE MUSIC works without an account. To see your likes and playlists:
   Settings ("Ajustes") -> "Iniciar sesión con Google".

"WINDOWS PROTECTED YOUR PC"
---------------------------
Expected: the installer is not code-signed.
Click "More info" -> "Run anyway".

IF SOMETHING FAILS
------------------
Send the log file: %APPDATA%\YoutubeInRustWeb\app.log
(paste that path into the File Explorer address bar).

HOTKEYS
-------
Ctrl+Shift+Space     Play / Pause
Ctrl+Shift+Arrows    Next / Previous
Ctrl+Shift+Y         Show / hide the window
Keyboard media keys also work.
Closing the window sends it to the tray; "Salir" in the tray menu quits.
