YoutubeInRustWeb 0.5.1  (MusicLowCost)
======================================

Native YouTube Music player written in Rust.
Uses ~20-60 MB of RAM (the official YouTube Music app / Pear: 300-700 MB).
Source code: https://github.com/BetoCW/MusicLowCost

FIRST RUN
---------
1. On first launch the app downloads "yt-dlp" by itself (the component that fetches
   YouTube audio). If Node.js is not installed it also downloads "deno" (~45 MB).
   This only happens once; files go to %APPDATA%\YoutubeInRustWeb\bin.

2. YOUTUBE MUSIC works without an account. To see your likes and playlists:
   Settings ("Ajustes") -> "Iniciar sesión con Google".

UPDATES
-------
No need to download each version again: when the "UpDate" button (top right) turns
red there is a new version. Click it once; the app updates itself and reopens.

JAM (LISTEN TOGETHER)
---------------------
Works over the internet from different places. The host opens "Jam", picks a name and
a password and clicks "Crear Jam"; the others type the same name and password and
click "Unirme". No IP addresses or router settings.

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
Closing the window (X) quits the app. To hide it without quitting, use Ctrl+Shift+Y
or "Mostrar / Ocultar" in the tray menu.
